use super::namespace::Resource;
use axum::http::StatusCode;
use std::{
    fmt::Write,
    time::{Duration, UNIX_EPOCH},
};

const DAV: &str = "DAV:";
const SUPPORTED: &[&str] = &[
    "displayname",
    "resourcetype",
    "getcontentlength",
    "getcontenttype",
    "getetag",
    "getlastmodified",
    "supportedlock",
    "lockdiscovery",
];

pub(super) struct Property {
    namespace: String,
    name: String,
}

pub(super) enum Query {
    All(Vec<Property>),
    Names,
    Explicit(Vec<Property>),
}

impl Query {
    pub(super) fn parse(body: &[u8]) -> Result<Self, StatusCode> {
        let invalid = StatusCode::BAD_REQUEST;
        let text = std::str::from_utf8(body).map_err(|_| invalid)?;
        if text.trim().is_empty() {
            return Ok(Self::All(Vec::new()));
        }
        let xml = roxmltree::Document::parse_with_options(
            text,
            roxmltree::ParsingOptions {
                nodes_limit: 256,
                ..Default::default()
            },
        )
        .map_err(|_| invalid)?;
        let root = xml.root_element();
        if !root.has_tag_name((DAV, "propfind")) {
            return Err(invalid);
        }
        let mut elements = root.children().filter(roxmltree::Node::is_element);
        let query = elements.next().ok_or(invalid)?;
        let properties = |parent: roxmltree::Node<'_, '_>| {
            parent
                .children()
                .filter(roxmltree::Node::is_element)
                .map(|property| Property {
                    namespace: property.tag_name().namespace().unwrap_or("").to_owned(),
                    name: property.tag_name().name().to_owned(),
                })
                .collect()
        };
        let result = if query.has_tag_name((DAV, "allprop")) {
            let includes = match elements.next() {
                Some(node) if node.has_tag_name((DAV, "include")) => properties(node),
                None => Vec::new(),
                Some(_) => return Err(invalid),
            };
            Self::All(includes)
        } else if query.has_tag_name((DAV, "propname")) {
            Self::Names
        } else if query.has_tag_name((DAV, "prop")) {
            Self::Explicit(properties(query))
        } else {
            return Err(invalid);
        };
        if elements.next().is_some() {
            return Err(invalid);
        }
        Ok(result)
    }

    pub(super) fn multistatus(&self, resources: &[Resource]) -> Result<String, StatusCode> {
        let mut document = String::from(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus xmlns:D=\"DAV:\">",
        );
        let standard = || {
            SUPPORTED.iter().map(|name| Property {
                namespace: DAV.to_owned(),
                name: (*name).to_owned(),
            })
        };
        for resource in resources {
            let mut found = String::new();
            let mut missing = String::new();
            match self {
                Self::Explicit(properties) => {
                    for property in properties {
                        append_selected(resource, property, &mut found, &mut missing);
                    }
                }
                Self::All(includes) => {
                    for property in standard() {
                        if let Some(value) = property_value(resource, &property) {
                            append_property(&mut found, &property, &value);
                        }
                    }
                    for property in includes.iter().filter(|p| {
                        p.namespace != DAV
                            || !SUPPORTED.contains(&p.name.as_str())
                            || property_value(resource, p).is_none()
                    }) {
                        append_selected(resource, property, &mut found, &mut missing);
                    }
                }
                Self::Names => {
                    for property in standard() {
                        if property_value(resource, &property).is_some() {
                            append_property(&mut found, &property, "");
                        }
                    }
                }
            }
            let _ = write!(
                document,
                "<D:response><D:href>{}</D:href>",
                escape(&resource.href)
            );
            if !found.is_empty() || missing.is_empty() {
                propstat(&mut document, &found, "200 OK");
            }
            if !missing.is_empty() {
                propstat(&mut document, &missing, "404 Not Found");
            }
            document.push_str("</D:response>");
            if document.len() > 8 * 1024 * 1024 {
                return Err(StatusCode::INSUFFICIENT_STORAGE);
            }
        }
        document.push_str("</D:multistatus>");
        Ok(document)
    }
}

fn propstat(document: &mut String, properties: &str, status: &str) {
    let _ = write!(
        document,
        "<D:propstat><D:prop>{properties}</D:prop><D:status>HTTP/1.1 {status}</D:status></D:propstat>"
    );
}

fn append_selected(
    resource: &Resource,
    property: &Property,
    found: &mut String,
    missing: &mut String,
) {
    match property_value(resource, property) {
        Some(value) => append_property(found, property, &value),
        None => append_property(missing, property, ""),
    }
}

fn append_property(output: &mut String, property: &Property, value: &str) {
    let name = &property.name;
    match property.namespace.as_str() {
        DAV => {
            let _ = write!(output, "<D:{name}>{value}</D:{name}>");
        }
        "" => {
            let _ = write!(output, "<{name}/>");
        }
        "http://www.w3.org/XML/1998/namespace" => {
            let _ = write!(output, "<xml:{name}/>");
        }
        namespace => {
            let _ = write!(output, "<P:{name} xmlns:P=\"{}\"/>", escape(namespace));
        }
    }
}

fn property_value(resource: &Resource, property: &Property) -> Option<String> {
    if property.namespace != DAV {
        return None;
    }
    let entry = resource.entry.as_ref();
    let content = entry.and_then(|entry| entry.content.as_ref());
    match property.name.as_str() {
        "displayname" => Some(escape(entry.map_or("Libraries", |entry| &entry.name))),
        "resourcetype" if resource.folder() => Some("<D:collection/>".into()),
        "resourcetype" | "supportedlock" | "lockdiscovery" => Some(String::new()),
        "getcontentlength" => content.map(|content| content.length.to_string()),
        "getcontenttype" => content.map(|_| "application/octet-stream".into()),
        "getetag" => entry.map(|entry| escape(&entry.http_etag())),
        "getlastmodified" => {
            let seconds = u64::try_from(entry?.modified?).ok()?;
            (seconds < 253_402_300_800)
                .then(|| httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(seconds)))
        }
        _ => None,
    }
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\u{fffe}' | '\u{ffff}' => escaped.push('\u{fffd}'),
            other => escaped.push(other),
        }
    }
    escaped
}
