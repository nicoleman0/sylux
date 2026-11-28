use super::{AuthState, Download, Failure, namespace, now, secured, validate_id};
use axum::{
    http::{HeaderValue, header},
    response::{Html, IntoResponse, Response},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::fmt::Write;

const PAGE_SIZE: usize = 100;

struct Scope {
    root: String,
    owner: String,
}

impl Scope {
    fn resolve(db: &Connection, verifier: [u8; 32]) -> Result<Self, Failure> {
        db.query_row(
            "SELECT s.entry_id,s.owner_account_id FROM share_link s
             JOIN account a ON a.account_id=s.owner_account_id AND a.enabled=1
             WHERE s.verifier=?1 AND s.expires_at>?2 AND s.revision_id IS NULL",
            params![verifier.as_slice(), now()],
            |row| {
                Ok(Self {
                    root: row.get(0)?,
                    owner: row.get(1)?,
                })
            },
        )
        .optional()?
        .ok_or(Failure::Missing)
    }

    fn relative_path(
        &self,
        db: &Connection,
        folder: &str,
    ) -> Result<Vec<namespace::Entry>, Failure> {
        let mut path =
            namespace::folder_path(db, &self.owner, folder).map_err(namespace_failure)?;
        let start = path
            .iter()
            .position(|part| part.id == self.root)
            .ok_or(Failure::Missing)?;
        Ok(path.split_off(start))
    }
}

fn namespace_failure(error: namespace::NamespaceError) -> Failure {
    match error {
        namespace::NamespaceError::NotFound => Failure::Missing,
        _ => Failure::Unavailable,
    }
}

pub(super) fn download(db: &Connection, verifier: [u8; 32], id: &str) -> Result<Download, Failure> {
    let snapshot = db.unchecked_transaction()?;
    let scope = Scope::resolve(&snapshot, verifier)?;
    let (file, parent) = snapshot
        .query_row(
            "SELECT e.original_name,b.blob_id,b.length,b.sha256,e.parent_id
             FROM entry e
             JOIN revision r ON r.revision_id=e.current_revision_id AND r.entry_id=e.entry_id
             JOIN blob b ON b.blob_id=r.blob_id AND b.lifecycle='live'
             WHERE e.entry_id=?1 AND e.owner_account_id=?2 AND e.kind='file' AND e.state='active'",
            params![id, scope.owner],
            |row| Ok((Download::from_row(row)?, row.get::<_, String>(4)?)),
        )
        .optional()?
        .ok_or(Failure::Missing)?;
    scope.relative_path(&snapshot, &parent)?;
    snapshot.commit()?;
    Ok(file)
}

struct Listing {
    path: Vec<namespace::Entry>,
    entries: Vec<namespace::Entry>,
}

fn listing(
    db: &Connection,
    verifier: [u8; 32],
    folder: Option<&str>,
    after: Option<&str>,
) -> Result<Listing, Failure> {
    let snapshot = db.unchecked_transaction()?;
    let scope = Scope::resolve(&snapshot, verifier)?;
    let folder = folder.unwrap_or(&scope.root);
    let path = scope.relative_path(&snapshot, folder)?;
    let cursor = after
        .map(|after| {
            snapshot
                .query_row(
                    "SELECT name_key FROM entry WHERE entry_id=?1 AND owner_account_id=?2
                 AND parent_id=?3 AND state='active'",
                    params![after, scope.owner, folder],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()?
                .map(|name| (name, after))
                .ok_or(Failure::Missing)
        })
        .transpose()?;
    let entries = namespace::list(
        &snapshot,
        &scope.owner,
        folder,
        cursor.as_ref().map(|(name, id)| (name.as_slice(), *id)),
        PAGE_SIZE,
    )
    .map_err(namespace_failure)?;
    snapshot.commit()?;
    Ok(Listing { path, entries })
}

pub(super) async fn browse(
    state: AuthState,
    token: &str,
    verifier: [u8; 32],
    folder: Option<String>,
    query: Option<&str>,
) -> Response {
    let after = match query {
        None => None,
        Some(query) => {
            let Some(after) = query
                .strip_prefix("after=")
                .filter(|id| validate_id(id).is_ok())
            else {
                return Failure::Missing.into_response();
            };
            Some(after.to_owned())
        }
    };
    let listing = state
        .workers
        .read(move |db| Ok(listing(db, verifier, folder.as_deref(), after.as_deref())))
        .await;
    let listing = match listing {
        Ok(Ok(listing)) => listing,
        Ok(Err(error)) => return error.into_response(),
        Err(_) => return Failure::Unavailable.into_response(),
    };
    let mut response = secured(Html(render(token, listing)).into_response());
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
        "default-src 'none'; style-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
    ));
    response
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn display_name(entry: &namespace::Entry) -> &str {
    if entry.parent_id.is_none() && entry.name.is_empty() {
        "Files"
    } else {
        &entry.name
    }
}

fn render(token: &str, listing: Listing) -> String {
    let base = format!("/share/{token}");
    let current = listing
        .path
        .last()
        .expect("authorized listing has a folder");
    let name = escape(display_name(current));
    let mut html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name} · Shared folder</title><link rel="stylesheet" href="/public-folder.css">
</head><body><main><p class="context">Shared folder</p><h1>{name}</h1>"#
    );
    if listing.path.len() > 1 {
        html.push_str(r#"<nav aria-label="Folder path"><ol>"#);
        for (index, part) in listing.path.iter().enumerate() {
            let name = escape(display_name(part));
            if part.id == current.id {
                write!(html, r#"<li aria-current="page">{name}</li>"#).expect("write to String");
            } else {
                let url = if index == 0 {
                    format!("{base}/browse")
                } else {
                    format!("{base}/folders/{}", part.id)
                };
                write!(html, r#"<li><a href="{url}">{name}</a></li>"#).expect("write to String");
            }
        }
        html.push_str("</ol></nav>");
    }
    if listing.entries.is_empty() {
        html.push_str("<p>This folder is empty.</p>");
    } else {
        html.push_str(r#"<ul class="entries" aria-label="Folder contents" role="list">"#);
        for entry in listing.entries.iter().take(PAGE_SIZE) {
            let name = escape(&entry.name);
            let id = &entry.id;
            if entry.kind == "folder" {
                write!(html, r#"<li><a class="entry" href="{base}/folders/{id}"><span>{name}</span><span class="action">Open folder</span></a></li>"#).expect("write to String");
            } else {
                write!(html, r#"<li><a class="entry" href="{base}/files/{id}" download><span>{name}</span><span class="action">Download</span></a></li>"#).expect("write to String");
            }
        }
        html.push_str("</ul>");
    }
    if listing.entries.len() > PAGE_SIZE {
        let id = &listing.entries[PAGE_SIZE - 1].id;
        write!(
            html,
            r#"<a class="next" href="{base}/folders/{}?after={id}">Next page</a>"#,
            current.id
        )
        .expect("write to String");
    }
    html.push_str("</main></body></html>");
    html
}
