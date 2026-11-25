//! Instance-wide read access stays attached to an active library root.

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::namespace::{self, Entry, NamespaceError};

pub(crate) fn active_id(
    db: &Connection,
    owner: &str,
    root: &str,
) -> Result<Option<String>, NamespaceError> {
    db.query_row(
        "SELECT p.publication_id FROM library_publication p
         JOIN account a ON a.account_id=p.owner_account_id AND a.enabled=1
         JOIN entry e ON e.entry_id=p.root_id AND e.owner_account_id=p.owner_account_id
         WHERE p.root_id=?1 AND p.owner_account_id=?2
           AND e.state='active' AND e.library_display_name IS NOT NULL",
        params![root, owner],
        |row| row.get(0),
    )
    .optional()
    .map_err(|_| NamespaceError::Sql)
}

pub(crate) fn publish(
    tx: &Transaction<'_>,
    owner: &str,
    root: &str,
) -> Result<String, NamespaceError> {
    let proposed = namespace::id()?;
    tx.execute(
        "INSERT INTO library_publication(root_id,publication_id,owner_account_id)
         SELECT e.entry_id,?3,e.owner_account_id FROM entry e
         JOIN account a ON a.account_id=e.owner_account_id AND a.enabled=1
         WHERE e.entry_id=?1 AND e.owner_account_id=?2
           AND e.state='active' AND e.library_display_name IS NOT NULL
         ON CONFLICT(root_id) DO NOTHING",
        params![root, owner, proposed],
    )
    .map_err(|_| NamespaceError::Sql)?;
    active_id(tx, owner, root)?.ok_or(NamespaceError::NotFound)
}

pub(crate) fn withdraw(tx: &Transaction<'_>, owner: &str, id: &str) -> Result<(), NamespaceError> {
    let removed = tx
        .execute(
            "DELETE FROM library_publication WHERE publication_id=?1 AND owner_account_id=?2",
            params![id, owner],
        )
        .map_err(|_| NamespaceError::Sql)?;
    if removed == 1 {
        Ok(())
    } else {
        Err(NamespaceError::NotFound)
    }
}

pub(crate) fn inherited(
    db: &Connection,
    owner: &str,
    path: &[Entry],
) -> Result<Option<String>, NamespaceError> {
    match path.first() {
        Some(root) => active_id(db, owner, &root.id),
        None => Ok(None),
    }
}

pub(crate) fn require_unpublished_parent(
    db: &Connection,
    owner: &str,
    parent: &str,
) -> Result<(), NamespaceError> {
    let path = namespace::folder_path(db, owner, parent)?;
    if inherited(db, owner, &path)?.is_some() {
        return Err(NamespaceError::SharingConfirmationRequired);
    }
    Ok(())
}
