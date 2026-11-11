//! Private-root metadata operations. Callers must authenticate the owner.
use super::names::{EntryName, NAME_KEY_VERSION};
use super::upload::folder_etag;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

pub(super) const MAX_ENTRY_DEPTH: i64 = 128;

#[derive(Debug)]
pub enum NamespaceError {
    Sql,
    NotFound,
    InvalidName,
    Conflict,
    Stale,
    NotEmpty,
    ParentUnavailable,
    InvalidDestination,
    DepthLimit,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Entry {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub generation: i64,
    pub etag: String,
}

fn id() -> Result<String, NamespaceError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| NamespaceError::Sql)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn private_root(db: &Connection, owner: &str) -> Result<String, NamespaceError> {
    db.query_row("SELECT entry_id FROM entry WHERE owner_account_id=?1 AND parent_id IS NULL AND state='active'", [owner], |r| r.get(0)).optional().map_err(|_| NamespaceError::Sql)?.ok_or(NamespaceError::NotFound)
}

/// Full private-root descriptor (id, name, kind, generation, etag) so an
/// authenticated client can seed folder creation and upload admission without
/// a separate discovery route. etag uses the `"{id}:{generation}"` form
/// (consistent with `domain::upload::folder_etag`).
pub fn root_entry(db: &Connection, owner: &str) -> Result<Entry, NamespaceError> {
    db.query_row(
        "SELECT entry_id, original_name, generation FROM entry WHERE owner_account_id=?1 AND parent_id IS NULL AND state='active'",
        [owner],
        |r| {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            let generation: i64 = r.get(2)?;
            let etag = folder_etag(&id, generation as u64);
            Ok(Entry {
                id,
                parent_id: None,
                name,
                kind: "folder".into(),
                generation,
                etag,
            })
        },
    )
    .optional()
    .map_err(|_| NamespaceError::Sql)?
    .ok_or(NamespaceError::NotFound)
}

pub fn create_folder(
    db: &Transaction<'_>,
    owner: &str,
    parent: &str,
    name: &str,
) -> Result<Entry, NamespaceError> {
    let name = EntryName::parse(name).map_err(|_| NamespaceError::InvalidName)?;
    let parent_row: Option<String> = db
        .query_row(
            "SELECT kind FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='active'",
            params![parent, owner],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| NamespaceError::Sql)?;
    if parent_row.as_deref() != Some("folder") {
        return Err(NamespaceError::NotFound);
    }
    // The root is depth zero; the submitted entry is one level below its parent.
    // A recursive ancestor walk runs inside the caller's write transaction.
    let parent_depth: i64 = db
        .query_row(
            "WITH RECURSIVE ancestors(entry_id,parent_id,depth) AS (\
                 SELECT entry_id,parent_id,0 FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='active' \
                 UNION ALL \
                 SELECT parent.entry_id,parent.parent_id,ancestors.depth+1 \
                 FROM entry AS parent JOIN ancestors ON ancestors.parent_id=parent.entry_id \
                 WHERE parent.owner_account_id=?2 AND parent.state='active'\
             ) SELECT COALESCE(MAX(depth),-1) FROM ancestors WHERE parent_id IS NULL",
            params![parent, owner],
            |row| row.get(0),
        )
        .map_err(|_| NamespaceError::Sql)?;
    if parent_depth < 0 || parent_depth + 1 > MAX_ENTRY_DEPTH {
        return Err(NamespaceError::InvalidName);
    }
    let id = id()?;
    let result = db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder',?4,?5,?6)", params![id,owner,parent,name.display,name.key,NAME_KEY_VERSION]);
    if let Err(error) = result {
        return Err(
            if matches!(error, rusqlite::Error::SqliteFailure(ref e,_) if e.code == rusqlite::ErrorCode::ConstraintViolation)
            {
                NamespaceError::Conflict
            } else {
                NamespaceError::Sql
            },
        );
    }
    db.execute(
        "UPDATE entry SET generation=generation+1 WHERE entry_id=?1",
        [parent],
    )
    .map_err(|_| NamespaceError::Sql)?;
    let etag = folder_etag(&id, 1);
    Ok(Entry {
        id,
        parent_id: Some(parent.to_owned()),
        name: name.display,
        kind: "folder".into(),
        generation: 1,
        etag,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingSort {
    #[default]
    NameAsc,
    NameDesc,
    SizeAsc,
    SizeDesc,
}

#[derive(Debug)]
pub struct ListingPosition {
    pub kind_rank: u8,
    pub name_key: Vec<u8>,
    pub size: i64,
    pub id: String,
}

#[derive(Debug)]
pub struct ListedEntry {
    pub entry: Entry,
    pub position: ListingPosition,
}

pub fn list(
    db: &Connection,
    owner: &str,
    folder: &str,
    after: Option<(&[u8], &str)>,
    limit: usize,
) -> Result<Vec<Entry>, NamespaceError> {
    let mut stmt = db.prepare("SELECT entry_id,parent_id,original_name,kind,generation FROM entry WHERE owner_account_id=?1 AND parent_id=?2 AND state='active' AND (?3 IS NULL OR name_key>?3 OR (name_key=?3 AND entry_id>?4)) ORDER BY name_key,entry_id LIMIT ?5").map_err(|_| NamespaceError::Sql)?;
    let rows = stmt
        .query_map(
            params![
                owner,
                folder,
                after.map(|a| a.0),
                after.map(|a| a.1).unwrap_or(""),
                (limit.min(500) + 1) as i64
            ],
            |r| {
                let id: String = r.get(0)?;
                let parent: Option<String> = r.get(1)?;
                let name: String = r.get(2)?;
                let kind: String = r.get(3)?;
                let generation: i64 = r.get(4)?;
                let etag = folder_etag(&id, generation as u64);
                Ok(Entry {
                    id,
                    parent_id: parent,
                    name,
                    kind,
                    generation,
                    etag,
                })
            },
        )
        .map_err(|_| NamespaceError::Sql)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| NamespaceError::Sql)
}

pub fn list_sorted(
    db: &Connection,
    owner: &str,
    folder: &str,
    sort: ListingSort,
    after: Option<&ListingPosition>,
    limit: usize,
) -> Result<Vec<ListedEntry>, NamespaceError> {
    let sql = listing_sql(sort);
    let mut stmt = db.prepare(&sql).map_err(|_| NamespaceError::Sql)?;
    let rows = stmt
        .query_map(
            params![
                owner,
                folder,
                after.map(|position| position.kind_rank),
                after.map(|position| position.name_key.as_slice()),
                after.map(|position| position.size),
                after.map(|position| position.id.as_str()),
                (limit.min(500) + 1) as i64
            ],
            |r| {
                let id: String = r.get(0)?;
                let generation: i64 = r.get(4)?;
                Ok(ListedEntry {
                    position: ListingPosition {
                        kind_rank: r.get(6)?,
                        name_key: r.get(5)?,
                        size: r.get(7)?,
                        id: id.clone(),
                    },
                    entry: Entry {
                        etag: folder_etag(&id, generation as u64),
                        id,
                        parent_id: r.get(1)?,
                        name: r.get(2)?,
                        kind: r.get(3)?,
                        generation,
                    },
                })
            },
        )
        .map_err(|_| NamespaceError::Sql)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| NamespaceError::Sql)
}

fn listing_sql(sort: ListingSort) -> String {
    let ascending_name = "(name_key>?4 OR (name_key=?4 AND entry_id>?6))";
    let (file_after, order) = match sort {
        ListingSort::NameAsc => (ascending_name, "kind_rank,name_key,entry_id"),
        ListingSort::NameDesc => (
            "(name_key<?4 OR (name_key=?4 AND entry_id>?6))",
            "kind_rank,CASE WHEN kind_rank=0 THEN name_key END ASC,CASE WHEN kind_rank=1 THEN name_key END DESC,entry_id",
        ),
        ListingSort::SizeAsc => (
            "(byte_size>?5 OR (byte_size=?5 AND (name_key>?4 OR (name_key=?4 AND entry_id>?6))))",
            "kind_rank,byte_size ASC,name_key,entry_id",
        ),
        ListingSort::SizeDesc => (
            "(byte_size<?5 OR (byte_size=?5 AND (name_key>?4 OR (name_key=?4 AND entry_id>?6))))",
            "kind_rank,byte_size DESC,name_key,entry_id",
        ),
    };
    // Only fixed SQL fragments vary; filtering and ordering cover every sibling before LIMIT.
    format!(
        "WITH listing AS (
            SELECT e.entry_id,e.parent_id,e.original_name,e.kind,e.generation,e.name_key,
                CASE e.kind WHEN 'folder' THEN 0 ELSE 1 END AS kind_rank,
                CASE e.kind WHEN 'folder' THEN 0 ELSE r.length END AS byte_size
            FROM entry e LEFT JOIN revision r ON r.revision_id=e.current_revision_id
            WHERE e.owner_account_id=?1 AND e.parent_id=?2 AND e.state='active'
        ) SELECT entry_id,parent_id,original_name,kind,generation,name_key,kind_rank,byte_size
        FROM listing WHERE ?3 IS NULL OR kind_rank>?3 OR (kind_rank=?3 AND (
            (kind_rank=0 AND {ascending_name}) OR (kind_rank=1 AND {file_after})
        )) ORDER BY {order} LIMIT ?7"
    )
}

pub fn rename(
    db: &Transaction<'_>,
    owner: &str,
    entry: &str,
    new_name: &str,
    if_match: &str,
) -> Result<Entry, NamespaceError> {
    let name = EntryName::parse(new_name).map_err(|_| NamespaceError::InvalidName)?;
    let row:Option<(Option<String>,String,i64)>=db.query_row("SELECT parent_id,kind,generation FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='active'",params![entry,owner],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|_|NamespaceError::Sql)?;
    let Some((parent, kind, generation)) = row else {
        return Err(NamespaceError::NotFound);
    };
    if parent.is_none() {
        return Err(NamespaceError::NotFound);
    }
    if if_match != folder_etag(entry, generation as u64) {
        return Err(NamespaceError::Stale);
    }
    let changed=db.execute("UPDATE entry SET original_name=?1,name_key=?2,name_key_version=?3,generation=generation+1 WHERE entry_id=?4 AND owner_account_id=?5 AND generation=?6",params![name.display,name.key,NAME_KEY_VERSION,entry,owner,generation]).map_err(|e|if matches!(e,rusqlite::Error::SqliteFailure(ref x,_) if x.code==rusqlite::ErrorCode::ConstraintViolation){NamespaceError::Conflict}else{NamespaceError::Sql})?;
    if changed != 1 {
        return Err(NamespaceError::Stale);
    }
    if let Some(parent) = &parent {
        db.execute(
            "UPDATE entry SET generation=generation+1 WHERE entry_id=?1",
            [parent],
        )
        .map_err(|_| NamespaceError::Sql)?;
    }

    Ok(Entry {
        id: entry.into(),
        parent_id: parent,
        name: name.display,
        kind,
        generation: generation + 1,
        etag: folder_etag(entry, (generation + 1) as u64),
    })
}

pub fn move_entry(
    db: &Transaction<'_>,
    owner: &str,
    entry: &str,
    destination: &str,
    if_match: &str,
) -> Result<Entry, NamespaceError> {
    let row: Option<(Option<String>, String, String, i64)> = db.query_row(
        "SELECT parent_id,original_name,kind,generation FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='active'",
        params![entry, owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    ).optional().map_err(|_| NamespaceError::Sql)?;
    let Some((Some(parent), name, kind, generation)) = row else {
        return Err(NamespaceError::NotFound);
    };
    if if_match != folder_etag(entry, generation as u64) {
        return Err(NamespaceError::Stale);
    }
    let mut ancestor = Some(destination.to_owned());
    let mut destination_depth = -1;
    let mut seen = std::collections::HashSet::new();
    while let Some(current) = ancestor {
        if current == entry || !seen.insert(current.clone()) {
            return Err(NamespaceError::InvalidDestination);
        }
        let row: Option<Option<String>> = db.query_row(
            "SELECT parent_id FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND kind='folder' AND state='active'",
            params![current, owner], |r| r.get(0)
        ).optional().map_err(|_| NamespaceError::Sql)?;
        ancestor = row.ok_or(NamespaceError::NotFound)?;
        destination_depth += 1;
        if destination_depth > MAX_ENTRY_DEPTH {
            return Err(NamespaceError::DepthLimit);
        }
    }
    if parent == destination {
        return Err(NamespaceError::InvalidDestination);
    }
    // Include deleted descendants so moving a parent cannot break later restoration.
    let subtree_depth: i64 = db.query_row(
        "WITH RECURSIVE subtree(entry_id,depth) AS (SELECT entry_id,0 FROM entry WHERE entry_id=?1 AND owner_account_id=?2 UNION ALL SELECT child.entry_id,subtree.depth+1 FROM entry child JOIN subtree ON child.parent_id=subtree.entry_id WHERE child.owner_account_id=?2) SELECT MAX(depth) FROM subtree",
        params![entry, owner], |r| r.get(0)
    ).map_err(|_| NamespaceError::Sql)?;
    if destination_depth + 1 + subtree_depth > MAX_ENTRY_DEPTH {
        return Err(NamespaceError::DepthLimit);
    }
    let changed = db.execute(
        "UPDATE entry SET parent_id=?1,generation=generation+1 WHERE entry_id=?2 AND owner_account_id=?3 AND generation=?4",
        params![destination, entry, owner, generation]
    ).map_err(|error| if matches!(error, rusqlite::Error::SqliteFailure(ref e, _) if e.code == rusqlite::ErrorCode::ConstraintViolation) { NamespaceError::Conflict } else { NamespaceError::Sql })?;
    if changed != 1 {
        return Err(NamespaceError::Stale);
    }
    db.execute(
        "UPDATE entry SET generation=generation+1 WHERE entry_id IN (?1,?2) AND owner_account_id=?3",
        params![parent, destination, owner],
    ).map_err(|_| NamespaceError::Sql)?;
    Ok(Entry {
        id: entry.to_owned(),
        parent_id: Some(destination.to_owned()),
        name,
        kind,
        generation: generation + 1,
        etag: folder_etag(entry, (generation + 1) as u64),
    })
}

/// Trash keeps payloads and revisions indefinitely. Folders must be empty.
pub fn trash(
    db: &Transaction<'_>,
    owner: &str,
    entry: &str,
    if_match: &str,
) -> Result<(), NamespaceError> {
    let row: Option<(Option<String>, String, i64)> = db.query_row(
        "SELECT parent_id,kind,generation FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='active'",
        params![entry, owner], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))
    ).optional().map_err(|_| NamespaceError::Sql)?;
    let Some((Some(parent), kind, generation)) = row else {
        return Err(NamespaceError::NotFound);
    };
    if if_match != folder_etag(entry, generation as u64) {
        return Err(NamespaceError::Stale);
    }
    if kind == "folder" {
        let children: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM entry WHERE parent_id=?1 AND state='active')",
                [entry],
                |r| r.get(0),
            )
            .map_err(|_| NamespaceError::Sql)?;
        if children {
            return Err(NamespaceError::NotEmpty);
        }
    }
    db.execute(
        "UPDATE entry SET state='deleted',generation=generation+1 WHERE entry_id=?1",
        [entry],
    )
    .map_err(|_| NamespaceError::Sql)?;
    db.execute(
        "UPDATE entry SET generation=generation+1 WHERE entry_id=?1",
        [parent],
    )
    .map_err(|_| NamespaceError::Sql)?;
    Ok(())
}

#[derive(serde::Serialize)]
pub struct DeletedEntry {
    #[serde(flatten)]
    pub entry: Entry,
    pub original_path: Vec<String>,
}

pub fn trashed(db: &Connection, owner: &str) -> Result<Vec<DeletedEntry>, NamespaceError> {
    let mut stmt = db.prepare("SELECT entry_id,parent_id,original_name,kind,generation FROM entry WHERE owner_account_id=?1 AND state='deleted' ORDER BY original_name,entry_id").map_err(|_| NamespaceError::Sql)?;
    let rows = stmt
        .query_map([owner], |r| {
            let id: String = r.get(0)?;
            let generation: i64 = r.get(4)?;
            Ok(Entry {
                etag: folder_etag(&id, generation as u64),
                id,
                parent_id: r.get(1)?,
                name: r.get(2)?,
                kind: r.get(3)?,
                generation,
            })
        })
        .map_err(|_| NamespaceError::Sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| NamespaceError::Sql)?;
    let mut result = Vec::with_capacity(rows.len());
    for entry in rows {
        let mut path = Vec::new();
        let mut current = entry.parent_id.clone().ok_or(NamespaceError::Sql)?;
        let mut seen = std::collections::HashSet::new();
        loop {
            if !seen.insert(current.clone()) || seen.len() > (MAX_ENTRY_DEPTH + 1) as usize {
                return Err(NamespaceError::Sql);
            }
            let (parent, name): (Option<String>, String) = db.query_row(
                "SELECT parent_id,original_name FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND kind='folder'",
                params![current, owner], |r| Ok((r.get(0)?, r.get(1)?))
            ).map_err(|_| NamespaceError::Sql)?;
            match parent {
                Some(parent) => {
                    path.push(name);
                    current = parent;
                }
                None => break,
            }
        }
        path.reverse();
        result.push(DeletedEntry {
            entry,
            original_path: path,
        });
    }
    Ok(result)
}

pub fn restore(
    db: &Transaction<'_>,
    owner: &str,
    entry: &str,
    if_match: &str,
) -> Result<(), NamespaceError> {
    let row: Option<(String, i64)> = db.query_row("SELECT parent_id,generation FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='deleted'", params![entry,owner], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_| NamespaceError::Sql)?;
    let Some((parent, generation)) = row else {
        return Err(NamespaceError::NotFound);
    };
    if if_match != folder_etag(entry, generation as u64) {
        return Err(NamespaceError::Stale);
    }
    let parent_active: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='active' AND kind='folder')", params![parent,owner], |r| r.get(0)).map_err(|_| NamespaceError::Sql)?;
    if !parent_active {
        return Err(NamespaceError::ParentUnavailable);
    }
    db.execute("UPDATE entry SET state='active',generation=generation+1 WHERE entry_id=?1", [entry]).map_err(|e| if matches!(e,rusqlite::Error::SqliteFailure(ref x,_) if x.code==rusqlite::ErrorCode::ConstraintViolation) { NamespaceError::Conflict } else { NamespaceError::Sql })?;
    db.execute(
        "UPDATE entry SET generation=generation+1 WHERE entry_id=?1",
        [parent],
    )
    .map_err(|_| NamespaceError::Sql)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_listing_limits_sql_work_to_indexed_siblings() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("../../migrations/sqlite/0001_s1.sql"))
            .unwrap();
        for sort in [
            ListingSort::NameAsc,
            ListingSort::NameDesc,
            ListingSort::SizeAsc,
            ListingSort::SizeDesc,
        ] {
            let sql = format!("EXPLAIN QUERY PLAN {}", listing_sql(sort));
            let mut statement = db.prepare(&sql).unwrap();
            let details = statement
                .query_map(
                    params![
                        "owner",
                        "folder",
                        Option::<u8>::None,
                        Option::<Vec<u8>>::None,
                        Option::<i64>::None,
                        Option::<String>::None,
                        101
                    ],
                    |row| row.get::<_, String>(3),
                )
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(
                details.iter().any(|detail| detail
                    .contains("SEARCH e USING INDEX entry_active_listing")
                    && detail.contains("owner_account_id=? AND parent_id=? AND state=?")),
                "{sort:?}: {details:?}"
            );
            assert!(
                details
                    .iter()
                    .any(|detail| detail.contains("SEARCH r USING INDEX")),
                "{sort:?}: {details:?}"
            );
        }
    }
}
