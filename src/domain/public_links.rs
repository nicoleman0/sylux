//! Active bearer audiences for owner-side sharing warnings.

use rusqlite::{Connection, params};

use super::namespace::{self, Entry, NamespaceError};

pub(crate) fn inherited(
    db: &Connection,
    owner: &str,
    path: &[Entry],
    now: i64,
) -> Result<Vec<String>, NamespaceError> {
    let mut statement = db
        .prepare(
            "SELECT share_id FROM share_link WHERE entry_id=?1 AND owner_account_id=?2
             AND expires_at>?3 AND revision_id IS NULL ORDER BY share_id",
        )
        .map_err(|_| NamespaceError::Sql)?;
    let mut links = Vec::new();
    for folder in path {
        let rows = statement
            .query_map(params![folder.id, owner, now], |row| row.get(0))
            .map_err(|_| NamespaceError::Sql)?;
        links.extend(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|_| NamespaceError::Sql)?,
        );
    }
    Ok(links)
}

pub(crate) fn require_private_parent(
    db: &Connection,
    owner: &str,
    parent: &str,
    now: i64,
) -> Result<(), NamespaceError> {
    let path = namespace::folder_path(db, owner, parent)?;
    if inherited(db, owner, &path, now)?.is_empty() {
        Ok(())
    } else {
        Err(NamespaceError::SharingConfirmationRequired)
    }
}
