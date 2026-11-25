use super::path::append_href;
use crate::domain::{
    names::EntryName,
    namespace::NamespaceError,
    upload::{folder_etag, revision_etag},
};
use axum::http::StatusCode;
use rusqlite::{Connection, OptionalExtension, Row, params};

const MAX_CHILDREN: i64 = 2048;
macro_rules! select_entries {
    ($filter:literal) => {
        concat!(
            "SELECT e.entry_id,e.parent_id,coalesce(e.library_display_name,e.original_name), ",
            "e.kind,e.generation,r.revision_id,b.blob_id,b.length,b.sha256,r.committed_at ",
            "FROM entry e LEFT JOIN revision r ON r.revision_id=e.current_revision_id AND r.entry_id=e.entry_id ",
            "LEFT JOIN blob b ON b.blob_id=r.blob_id AND b.lifecycle='live' ",
            "WHERE e.owner_account_id=?1 AND e.state='active' AND ", $filter
        )
    };
}

pub(super) struct Resource {
    pub(super) entry: Option<Entry>,
    pub(super) href: String,
}

pub(super) struct Entry {
    pub(super) id: String,
    pub(super) parent_id: Option<String>,
    pub(super) name: String,
    pub(super) folder: bool,
    pub(super) etag: String,
    pub(super) content: Option<super::super::content::ContentRef>,
    pub(super) modified: Option<i64>,
}

impl Entry {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let id: String = row.get(0)?;
        let name: String = row.get(2)?;
        let folder = row.get::<_, String>(3)? == "folder";
        let generation =
            u64::try_from(row.get::<_, i64>(4)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let etag = folder_etag(&id, generation);
        let content = if folder {
            None
        } else {
            Some(super::super::content::ContentRef {
                name: name.clone(),
                revision_id: row.get(5)?,
                blob_id: row.get(6)?,
                length: u64::try_from(row.get::<_, i64>(7)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                sha256: row
                    .get::<_, Vec<u8>>(8)?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
            })
        };
        Ok(Self {
            id,
            parent_id: row.get(1)?,
            name,
            folder,
            etag,
            content,
            modified: row.get(9)?,
        })
    }

    pub(super) fn http_etag(&self) -> String {
        self.content
            .as_ref()
            .map_or_else(|| self.etag.clone(), |c| revision_etag(&c.revision_id))
    }
}

impl Resource {
    pub(super) fn folder(&self) -> bool {
        self.entry.as_ref().is_none_or(|entry| entry.folder)
    }
}

pub(super) fn unavailable(_: rusqlite::Error) -> StatusCode {
    StatusCode::SERVICE_UNAVAILABLE
}

pub(super) fn namespace_error(error: NamespaceError) -> StatusCode {
    match error {
        NamespaceError::NotFound => StatusCode::NOT_FOUND,
        NamespaceError::Stale => StatusCode::PRECONDITION_FAILED,
        NamespaceError::Conflict | NamespaceError::InvalidDestination => StatusCode::CONFLICT,
        NamespaceError::InvalidName => StatusCode::BAD_REQUEST,
        NamespaceError::DepthLimit | NamespaceError::SharingConfirmationRequired => {
            StatusCode::FORBIDDEN
        }
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub(super) fn resolve(
    db: &Connection,
    owner: &str,
    names: &[EntryName],
) -> Result<Resource, StatusCode> {
    let mut resource = Resource {
        entry: None,
        href: "/dav/".into(),
    };
    for name in names {
        if !resource.folder() {
            return Err(StatusCode::NOT_FOUND);
        }
        let entry = child(
            db,
            owner,
            resource.entry.as_ref().map(|e| e.id.as_str()),
            name,
        )?
        .ok_or(StatusCode::NOT_FOUND)?;
        resource.href = append_href(&resource.href, &entry.name, entry.folder);
        resource.entry = Some(entry);
    }
    Ok(resource)
}

pub(super) fn child(
    db: &Connection,
    owner: &str,
    parent: Option<&str>,
    name: &EntryName,
) -> Result<Option<Entry>, StatusCode> {
    let query = if parent.is_some() {
        select_entries!("e.parent_id=?2 AND e.name_key=?3")
    } else {
        select_entries!(
            "e.parent_id IS ?2 AND e.kind='folder' AND coalesce(e.library_name_key,CAST(e.library_display_name AS BLOB))=?3"
        )
    };
    db.query_row(query, params![owner, parent, name.key], Entry::from_row)
        .optional()
        .map_err(unavailable)
}

pub(super) fn children(
    db: &Connection,
    owner: &str,
    directory: &Resource,
) -> Result<Vec<Resource>, StatusCode> {
    let parent = directory.entry.as_ref().map(|entry| entry.id.as_str());
    let mut statement = db
        .prepare(select_entries!(
            "e.parent_id IS ?2 ORDER BY e.name_key, e.entry_id LIMIT ?3"
        ))
        .map_err(unavailable)?;
    let mut children = Vec::new();
    let rows = statement
        .query_map(params![owner, parent, MAX_CHILDREN + 1], Entry::from_row)
        .map_err(unavailable)?;
    for row in rows {
        if children.len() == MAX_CHILDREN as usize {
            return Err(StatusCode::INSUFFICIENT_STORAGE);
        }
        let entry = row.map_err(unavailable)?;
        children.push(Resource {
            href: append_href(&directory.href, &entry.name, entry.folder),
            entry: Some(entry),
        });
    }
    Ok(children)
}

pub(super) fn writable_parent<'a>(
    db: &Connection,
    owner: &str,
    names: &'a [EntryName],
) -> Result<(Entry, &'a EntryName), StatusCode> {
    let (name, ancestors) = names.split_last().ok_or(StatusCode::FORBIDDEN)?;
    let parent = resolve(db, owner, ancestors)
        .map_err(|status| {
            if status == StatusCode::NOT_FOUND {
                StatusCode::CONFLICT
            } else {
                status
            }
        })?
        .entry
        .filter(|entry| entry.folder)
        .ok_or(StatusCode::FORBIDDEN)?;
    Ok((parent, name))
}
