//! Account grants are separate from private namespace authority.

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

use super::{
    namespace::{self, Entry, NamespaceError},
    search::SearchHit,
    upload::folder_etag,
};

#[derive(Serialize)]
pub(crate) struct Grant {
    pub id: String,
    pub handle: String,
}

pub(crate) fn manage(
    db: &Connection,
    owner: &str,
    root: &str,
) -> Result<Vec<Grant>, NamespaceError> {
    namespace::folder_path(db, owner, root)?;
    db.prepare(
        "SELECT g.grant_id,a.handle FROM direct_grant g
         JOIN account a ON a.account_id=g.recipient_account_id
         WHERE g.owner_account_id=?1 AND g.root_id=?2 ORDER BY a.handle",
    )
    .and_then(|mut stmt| {
        stmt.query_map(params![owner, root], |row| {
            Ok(Grant {
                id: row.get(0)?,
                handle: row.get(1)?,
            })
        })?
        .collect()
    })
    .map_err(|_| NamespaceError::Sql)
}

pub(crate) fn grant(
    tx: &Transaction<'_>,
    owner: &str,
    root: &str,
    handle: &str,
) -> Result<Grant, NamespaceError> {
    namespace::folder_path(tx, owner, root)?;
    let recipient: String = tx
        .query_row(
            "SELECT account_id FROM account WHERE handle=?1 AND enabled=1 AND account_id<>?2",
            params![handle, owner],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| NamespaceError::Sql)?
        .ok_or(NamespaceError::NotFound)?;
    let proposed = namespace::id()?;
    tx.execute(
        "INSERT INTO direct_grant(grant_id,owner_account_id,recipient_account_id,root_id)
         VALUES (?1,?2,?3,?4) ON CONFLICT(root_id,recipient_account_id) DO NOTHING",
        params![proposed, owner, recipient, root],
    )
    .map_err(|_| NamespaceError::Sql)?;
    let id = tx
        .query_row(
            "SELECT grant_id FROM direct_grant WHERE root_id=?1 AND recipient_account_id=?2",
            params![root, recipient],
            |row| row.get(0),
        )
        .map_err(|_| NamespaceError::Sql)?;
    Ok(Grant {
        id,
        handle: handle.to_owned(),
    })
}

pub(crate) fn revoke(tx: &Transaction<'_>, owner: &str, id: &str) -> rusqlite::Result<bool> {
    tx.execute(
        "DELETE FROM direct_grant WHERE grant_id=?1 AND owner_account_id=?2",
        params![id, owner],
    )
    .map(|count| count == 1)
}

#[derive(Serialize)]
pub(crate) struct Incoming {
    pub entry: Entry,
    pub owner: String,
}

pub(crate) fn roots(db: &Connection, recipient: &str) -> Result<Vec<SharedPath>, NamespaceError> {
    let mut roots = Vec::new();
    for grant in incoming(db, recipient).map_err(|_| NamespaceError::Sql)? {
        let path = path(db, recipient, &grant.entry.id)?;
        if path.folders.len() == 1 {
            roots.push(path);
        }
    }
    Ok(roots)
}

pub(crate) fn incoming(db: &Connection, recipient: &str) -> rusqlite::Result<Vec<Incoming>> {
    let mut stmt = db.prepare(
        "SELECT e.entry_id,coalesce(e.library_display_name,e.original_name),e.generation,a.handle
         FROM direct_grant g
         JOIN account a ON a.account_id=g.owner_account_id AND a.enabled=1
         JOIN account r ON r.account_id=g.recipient_account_id AND r.enabled=1
         JOIN entry e ON e.entry_id=g.root_id AND e.owner_account_id=g.owner_account_id
         WHERE g.recipient_account_id=?1 AND e.state='active' AND e.kind='folder'
         ORDER BY 2,a.handle,e.entry_id",
    )?;
    stmt.query_map([recipient], |row| {
        let id: String = row.get(0)?;
        let generation: i64 = row.get(2)?;
        Ok(Incoming {
            entry: Entry {
                etag: folder_etag(&id, generation as u64),
                id,
                parent_id: None,
                name: row.get(1)?,
                kind: "folder".into(),
                generation,
            },
            owner: row.get(3)?,
        })
    })?
    .collect()
}

/// This result is valid only for the snapshot that resolved it.
pub(crate) struct SharedPath {
    pub owner: String,
    pub folders: Vec<Entry>,
    pub depth: i64,
}

pub(crate) fn path(
    db: &Connection,
    recipient: &str,
    folder: &str,
) -> Result<SharedPath, NamespaceError> {
    let owner: String = db
        .query_row(
            "SELECT e.owner_account_id FROM entry e
         JOIN account o ON o.account_id=e.owner_account_id AND o.enabled=1
         JOIN account r ON r.account_id=?2 AND r.enabled=1
         WHERE e.entry_id=?1 AND EXISTS(
             SELECT 1 FROM direct_grant g
             WHERE g.recipient_account_id=?2 AND g.owner_account_id=e.owner_account_id
         )",
            params![folder, recipient],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| NamespaceError::Sql)?
        .ok_or(NamespaceError::NotFound)?;
    let mut folders = namespace::folder_path(db, &owner, folder)?;
    let depth = folders.len() as i64 - 1;
    let mut granted = db.prepare(
        "SELECT 1 FROM direct_grant WHERE root_id=?1 AND recipient_account_id=?2 AND owner_account_id=?3",
    ).map_err(|_| NamespaceError::Sql)?;
    let mut first = None;
    for (index, entry) in folders.iter().enumerate() {
        if granted
            .exists(params![entry.id, recipient, owner])
            .map_err(|_| NamespaceError::Sql)?
        {
            first = Some(index);
            break;
        }
    }
    folders.drain(..first.ok_or(NamespaceError::NotFound)?);
    folders[0].parent_id = None;
    if folders[0].name.is_empty() {
        folders[0].name = "Files".into();
    }
    Ok(SharedPath {
        owner,
        folders,
        depth,
    })
}

pub(crate) fn file(
    db: &Connection,
    recipient: &str,
    id: &str,
) -> Result<SearchHit, NamespaceError> {
    let (entry, size, modified_at) = db.query_row(
        "SELECT e.parent_id,e.original_name,e.generation,r.length,r.committed_at
         FROM entry e LEFT JOIN revision r ON r.revision_id=e.current_revision_id AND e.entry_id=r.entry_id
         WHERE e.entry_id=?1 AND e.kind='file' AND e.state='active'",
        [id],
        |row| {
            let generation: i64 = row.get(2)?;
            Ok((Entry {
                id: id.to_owned(), kind: "file".into(), name: row.get(1)?,
                parent_id: Some(row.get(0)?), generation,
                etag: folder_etag(id, generation as u64),
            }, row.get(3)?, row.get(4)?))
        },
    ).optional().map_err(|_| NamespaceError::Sql)?.ok_or(NamespaceError::NotFound)?;
    let path = path(
        db,
        recipient,
        entry.parent_id.as_deref().expect("file has a parent"),
    )?;
    Ok(SearchHit {
        entry,
        size,
        modified_at,
        location: path.folders,
    })
}

#[derive(Serialize)]
struct AudienceGrant {
    id: String,
    handle: String,
    root: String,
}

#[derive(Serialize)]
pub(crate) struct TreeImpact {
    source: Vec<(String, String)>,
    destination: Vec<(String, String)>,
    previous_grants: Vec<AudienceGrant>,
    destination_grants: Vec<AudienceGrant>,
    carried_grants: Vec<AudienceGrant>,
    pub new_recipients: BTreeSet<String>,
    pub lost_recipients: BTreeSet<String>,
    pub carried_recipients: BTreeSet<String>,
}

impl TreeImpact {
    pub(crate) fn requires_confirmation(&self) -> bool {
        !self.new_recipients.is_empty() || !self.carried_recipients.is_empty()
    }
}

pub(crate) fn tree_impact(
    db: &Connection,
    owner: &str,
    entry: &str,
    source: &[Entry],
    destination: &[Entry],
) -> Result<TreeImpact, NamespaceError> {
    let mut grants = db
        .prepare(
            "SELECT g.grant_id,a.handle,g.root_id FROM direct_grant g
         JOIN account a ON a.account_id=g.recipient_account_id AND a.enabled=1
         WHERE g.owner_account_id=?1 AND g.root_id=?2 ORDER BY g.grant_id",
        )
        .map_err(|_| NamespaceError::Sql)?;
    let mut inherited = |path: &[Entry]| -> rusqlite::Result<Vec<AudienceGrant>> {
        let mut result = Vec::new();
        for folder in path {
            let rows = grants.query_map(params![owner, folder.id], audience_grant)?;
            result.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        Ok(result)
    };
    let previous_grants = inherited(source).map_err(|_| NamespaceError::Sql)?;
    let destination_grants = inherited(destination).map_err(|_| NamespaceError::Sql)?;
    let mut carried_grants = Vec::new();
    if !source.is_empty() {
        // Walk up from grants so moving a large private tree does not scan its files.
        carried_grants = db.prepare(
            "WITH RECURSIVE ancestors(grant_id,recipient_account_id,root_id,entry_id,parent_id,depth) AS (
                SELECT g.grant_id,g.recipient_account_id,g.root_id,e.entry_id,e.parent_id,0
                FROM direct_grant g JOIN entry e ON e.entry_id=g.root_id
                WHERE g.owner_account_id=?1 AND e.state='active'
                UNION ALL
                SELECT p.grant_id,p.recipient_account_id,p.root_id,e.entry_id,e.parent_id,p.depth+1
                FROM ancestors p JOIN entry e ON e.entry_id=p.parent_id
                WHERE e.state='active' AND e.owner_account_id=?1 AND p.depth<?3 AND p.entry_id<>?2
             ) SELECT p.grant_id,a.handle,p.root_id FROM ancestors p
             JOIN account a ON a.account_id=p.recipient_account_id AND a.enabled=1
             WHERE p.entry_id=?2 ORDER BY p.grant_id",
        ).and_then(|mut stmt| {
            stmt.query_map(params![owner, entry, namespace::MAX_ENTRY_DEPTH], audience_grant)?.collect()
        }).map_err(|_| NamespaceError::Sql)?;
    }
    let root_recipients = carried_grants.iter().filter(|grant| grant.root == entry);
    let previous: BTreeSet<_> = previous_grants
        .iter()
        .chain(root_recipients.clone())
        .map(|grant| &grant.handle)
        .collect();
    let retained: BTreeSet<_> = destination_grants
        .iter()
        .chain(root_recipients)
        .map(|grant| &grant.handle)
        .collect();
    let new_recipients = destination_grants
        .iter()
        .filter(|grant| !previous.contains(&grant.handle))
        .map(|grant| grant.handle.clone())
        .collect();
    let lost_recipients = previous_grants
        .iter()
        .filter(|grant| !retained.contains(&grant.handle))
        .map(|grant| grant.handle.clone())
        .collect();
    let carried_recipients = carried_grants
        .iter()
        .map(|grant| grant.handle.clone())
        .collect();
    let location = |path: &[Entry]| {
        path.iter()
            .map(|part| (part.id.clone(), part.name.clone()))
            .collect()
    };
    Ok(TreeImpact {
        source: location(source),
        destination: location(destination),
        previous_grants,
        destination_grants,
        carried_grants,
        new_recipients,
        lost_recipients,
        carried_recipients,
    })
}

fn audience_grant(row: &rusqlite::Row<'_>) -> rusqlite::Result<AudienceGrant> {
    Ok(AudienceGrant {
        id: row.get(0)?,
        handle: row.get(1)?,
        root: row.get(2)?,
    })
}
