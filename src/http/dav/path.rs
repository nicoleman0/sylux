use axum::http::{StatusCode, Uri};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

use crate::domain::{names::EntryName, namespace::MAX_ENTRY_DEPTH};

const MAX_URI_PATH_BYTES: usize = 8 * 1024;

#[derive(Clone)]
pub(super) struct DavPath {
    pub(super) names: Vec<EntryName>,
    pub(super) trailing_slash: bool,
}

impl DavPath {
    pub(super) fn parse(uri: &Uri) -> Result<Self, StatusCode> {
        let invalid = StatusCode::BAD_REQUEST;
        let path = uri.path();
        if uri.query().is_some() || path.len() > MAX_URI_PATH_BYTES {
            return Err(invalid);
        }
        let tail = if path == "/dav" {
            ""
        } else {
            path.strip_prefix("/dav/").ok_or(invalid)?
        };
        let trailing_slash = path.ends_with('/');
        let names = if tail.is_empty() {
            Vec::new()
        } else {
            tail.strip_suffix('/')
                .unwrap_or(tail)
                .split('/')
                .map(|segment| {
                    let mut bytes = segment.as_bytes().iter();
                    while let Some(byte) = bytes.next() {
                        if *byte == b'%'
                            && !matches!((bytes.next(), bytes.next()), (Some(a), Some(b))
                            if a.is_ascii_hexdigit() && b.is_ascii_hexdigit())
                        {
                            return Err(invalid);
                        }
                    }
                    let name = percent_decode_str(segment)
                        .decode_utf8()
                        .map_err(|_| invalid)?;
                    EntryName::parse(&name).map_err(|_| invalid)
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        if names.len() > MAX_ENTRY_DEPTH as usize + 1 {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(Self {
            names,
            trailing_slash,
        })
    }

    pub(super) fn destination(raw: &str, origin: &str) -> Result<Self, StatusCode> {
        let invalid = StatusCode::BAD_REQUEST;
        if raw.contains('#') || raw.starts_with("//") {
            return Err(invalid);
        }
        let uri: Uri = raw.parse().map_err(|_| invalid)?;
        if uri.scheme().is_some() || uri.authority().is_some() {
            let expected: Uri = origin
                .parse()
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            let authority = uri
                .authority()
                .filter(|a| !a.as_str().contains('@'))
                .ok_or(invalid)?;
            let expected = expected
                .authority()
                .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
            if uri.scheme_str() != Some("https")
                || !authority.host().eq_ignore_ascii_case(expected.host())
                || authority.port_u16().unwrap_or(443) != expected.port_u16().unwrap_or(443)
            {
                return Err(StatusCode::BAD_GATEWAY);
            }
        }
        Self::parse(&uri)
    }
}

pub(super) fn append_href(parent: &str, name: &str, folder: bool) -> String {
    // Encode each name once so a literal %2F is distinct from a separator.
    const NAME_ENCODE: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    let name = utf8_percent_encode(name, NAME_ENCODE);
    let suffix = if folder { "/" } else { "" };
    format!("{parent}{name}{suffix}")
}
