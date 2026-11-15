//! Private-root metadata operations. Callers must authenticate the owner.
use super::names::{EntryName, NAME_KEY_VERSION};
use super::upload::folder_etag;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

pub(super) const MAX_ENTRY_DEPTH: i64 = 128;
pub const MAX_TRASH_PAGE_SIZE: usize = 200;

#[derive(Debug)]
pub enum NamespaceError {
    Sql,
    NotFound,
    InvalidName,
    InvalidLimit,
    Conflict,
    Stale,
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

/// Resolve active ancestry in one SQL snapshot without exposing foreign names.
pub fn folder_path(
    db: &Connection,
    owner: &str,
    folder: &str,
) -> Result<Vec<Entry>, NamespaceError> {
    let mut statement = db
        .prepare(
            "WITH RECURSIVE ancestry(entry_id,parent_id,original_name,generation,depth) AS (
            SELECT entry_id,parent_id,original_name,generation,0 FROM entry
            WHERE entry_id=?1 AND owner_account_id=?2 AND kind='folder' AND state='active'
            UNION ALL
            SELECT e.entry_id,e.parent_id,e.original_name,e.generation,a.depth+1
            FROM entry e JOIN ancestry a ON e.entry_id=a.parent_id
            WHERE e.owner_account_id=?2 AND e.kind='folder' AND e.state='active' AND a.depth<?3
        ) SELECT entry_id,parent_id,original_name,generation FROM ancestry ORDER BY depth DESC",
        )
        .map_err(|_| NamespaceError::Sql)?;
    let rows = statement
        .query_map(params![folder, owner, MAX_ENTRY_DEPTH], |row| {
            let id: String = row.get(0)?;
            let generation: i64 = row.get(3)?;
            Ok(Entry {
                etag: folder_etag(&id, generation as u64),
                id,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                kind: "folder".into(),
                generation,
            })
        })
        .map_err(|_| NamespaceError::Sql)?;
    let path = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| NamespaceError::Sql)?;
    if path.first().is_none_or(|entry| entry.parent_id.is_some()) {
        return Err(NamespaceError::NotFound);
    }
    Ok(path)
}

pub fn create_folder(
    db: &Transaction<'_>,
    owner: &str,
    parent: &str,
    name: &str,
) -> Result<Entry, NamespaceError> {
    let name = EntryName::parse(name).map_err(|_| NamespaceError::InvalidName)?;
    let path = folder_path(db, owner, parent)?;
    if path.len() as i64 > MAX_ENTRY_DEPTH {
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
    ModifiedAsc,
    ModifiedDesc,
}

#[derive(Debug)]
pub struct ListingPosition {
    pub kind_rank: u8,
    pub name_key: Vec<u8>,
    pub size: i64,
    pub modified_at: Option<i64>,
    pub id: String,
}

#[derive(Debug)]
pub struct ListedEntry {
    pub entry: Entry,
    pub size: Option<i64>,
    pub modified_at: Option<i64>,
    pub position: ListingPosition,
}

pub fn list(
    db: &Connection,
    owner: &str,
    folder: &str,
    after: Option<(&[u8], &str)>,
    limit: usize,
) -> Result<Vec<Entry>, NamespaceError> {
    folder_path(db, owner, folder)?;
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
    folder_path(db, owner, folder)?;
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
                after.and_then(|position| position.modified_at),
                (limit.min(500) + 1) as i64,
                limit.min(500) as i64
            ],
            |r| {
                let id: String = r.get(0)?;
                let generation: i64 = r.get(4)?;
                Ok(ListedEntry {
                    size: r.get(8)?,
                    modified_at: r.get(9)?,
                    position: ListingPosition {
                        kind_rank: r.get(6)?,
                        name_key: r.get(5)?,
                        size: r.get(7)?,
                        modified_at: r.get(9)?,
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
        ListingSort::ModifiedAsc => (
            "((?7 IS NOT NULL AND (modified_at IS NULL OR modified_at>?7 OR (modified_at=?7 AND (name_key>?4 OR (name_key=?4 AND entry_id>?6))))) OR (?7 IS NULL AND modified_at IS NULL AND (name_key>?4 OR (name_key=?4 AND entry_id>?6))))",
            "kind_rank,modified_at IS NULL,modified_at ASC,name_key,entry_id",
        ),
        ListingSort::ModifiedDesc => (
            "((?7 IS NOT NULL AND (modified_at IS NULL OR modified_at<?7 OR (modified_at=?7 AND (name_key>?4 OR (name_key=?4 AND entry_id>?6))))) OR (?7 IS NULL AND modified_at IS NULL AND (name_key>?4 OR (name_key=?4 AND entry_id>?6))))",
            "kind_rank,modified_at IS NULL,modified_at DESC,name_key,entry_id",
        ),
        ListingSort::SizeDesc => (
            "(byte_size<?5 OR (byte_size=?5 AND (name_key>?4 OR (name_key=?4 AND entry_id>?6))))",
            "kind_rank,byte_size DESC,name_key,entry_id",
        ),
    };
    // Only fixed SQL fragments vary; filtering and ordering cover every sibling before LIMIT.
    format!(
        "WITH RECURSIVE ancestors(entry_id,parent_id) AS (
            SELECT entry_id,parent_id FROM entry
            WHERE entry_id=?2 AND owner_account_id=?1 AND kind='folder' AND state='active'
            UNION ALL
            SELECT e.entry_id,e.parent_id FROM entry e JOIN ancestors a ON e.entry_id=a.parent_id
            WHERE e.owner_account_id=?1 AND e.kind='folder' AND e.state='active'
        ), listing AS (
            SELECT e.entry_id,e.parent_id,e.original_name,e.kind,e.generation,e.name_key,
                CASE e.kind WHEN 'folder' THEN 0 ELSE 1 END AS kind_rank,
                CASE e.kind WHEN 'folder' THEN 0 ELSE r.length END AS byte_size,
                CASE e.kind WHEN 'file' THEN r.committed_at END AS modified_at
            FROM entry e LEFT JOIN revision r ON r.revision_id=e.current_revision_id
            WHERE e.owner_account_id=?1 AND e.parent_id=?2 AND e.state='active'
                AND EXISTS(SELECT 1 FROM ancestors WHERE parent_id IS NULL)
        ), page AS MATERIALIZED (
        SELECT * FROM listing WHERE ?3 IS NULL OR kind_rank>?3 OR (kind_rank=?3 AND (
            (kind_rank=0 AND {ascending_name}) OR (kind_rank=1 AND {file_after})
        )) ORDER BY {order} LIMIT ?8
        ), visible AS (SELECT * FROM page ORDER BY {order} LIMIT ?9),
        descendants(folder_id,entry_id,kind,current_revision_id) AS (
            SELECT e.entry_id,e.entry_id,e.kind,e.current_revision_id
            FROM visible v JOIN entry e ON e.entry_id=v.entry_id
            WHERE v.kind_rank=0 AND e.owner_account_id=?1 AND e.state='active'
            UNION ALL
            SELECT d.folder_id,e.entry_id,e.kind,e.current_revision_id
            FROM descendants d JOIN entry e ON e.parent_id=d.entry_id
            WHERE d.kind='folder' AND e.owner_account_id=?1 AND e.state='active'
        ), totals AS (
            SELECT d.folder_id,
                CASE WHEN SUM(CASE WHEN d.kind='file' AND r.length IS NULL THEN 1 ELSE 0 END)>0
                    THEN NULL ELSE COALESCE(SUM(CASE WHEN d.kind='file' THEN r.length ELSE 0 END),0) END AS total_bytes
            FROM descendants d LEFT JOIN revision r ON r.revision_id=d.current_revision_id AND r.entry_id=d.entry_id
            GROUP BY d.folder_id
        ) SELECT page.entry_id,page.parent_id,page.original_name,page.kind,page.generation,
            page.name_key,page.kind_rank,page.byte_size,
            CASE WHEN page.kind_rank=0 THEN totals.total_bytes ELSE page.byte_size END,page.modified_at
        FROM page LEFT JOIN totals ON totals.folder_id=page.entry_id ORDER BY {order}"
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
    let Some(parent_id) = &parent else {
        return Err(NamespaceError::NotFound);
    };
    folder_path(db, owner, parent_id)?;
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
    folder_path(db, owner, &parent)?;
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

/// Hide a tree behind its root marker while retaining payloads and earlier deletions.
pub fn trash(
    db: &Transaction<'_>,
    owner: &str,
    entry: &str,
    if_match: &str,
) -> Result<(), NamespaceError> {
    let row: Option<(Option<String>, i64)> = db.query_row(
        "SELECT parent_id,generation FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='active'",
        params![entry, owner], |r| Ok((r.get(0)?,r.get(1)?))
    ).optional().map_err(|_| NamespaceError::Sql)?;
    let Some((Some(parent), generation)) = row else {
        return Err(NamespaceError::NotFound);
    };
    folder_path(db, owner, &parent)?;
    if if_match != folder_etag(entry, generation as u64) {
        return Err(NamespaceError::Stale);
    }
    db.execute(
        "WITH RECURSIVE tree(entry_id,kind) AS (
            SELECT entry_id,kind FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='active'
            UNION ALL
            SELECT e.entry_id,e.kind FROM entry e JOIN tree t ON e.parent_id=t.entry_id
            WHERE t.kind='folder' AND e.owner_account_id=?2 AND e.state='active'
        ) UPDATE entry SET generation=generation+1 WHERE entry_id IN (SELECT entry_id FROM tree)",
        params![entry, owner],
    )
    .map_err(|_| NamespaceError::Sql)?;
    db.execute(
        "UPDATE entry SET state='deleted' WHERE entry_id=?1",
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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub needs_reference: bool,
}

pub struct TrashPosition {
    pub name: String,
    pub id: String,
}

pub struct TrashPage {
    pub items: Vec<DeletedEntry>,
    pub has_more: bool,
}

fn deleted_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let id: String = row.get(0)?;
    let generation: i64 = row.get(4)?;
    Ok(Entry {
        etag: folder_etag(&id, generation as u64),
        id,
        parent_id: row.get(1)?,
        name: row.get(2)?,
        kind: row.get(3)?,
        generation,
    })
}

pub fn trashed(db: &Connection, owner: &str) -> Result<Vec<DeletedEntry>, NamespaceError> {
    let mut stmt = db.prepare("SELECT entry_id,parent_id,original_name,kind,generation FROM entry WHERE owner_account_id=?1 AND state='deleted' ORDER BY original_name,entry_id").map_err(|_| NamespaceError::Sql)?;
    let rows = stmt
        .query_map([owner], deleted_entry)
        .map_err(|_| NamespaceError::Sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| NamespaceError::Sql)?;
    deleted_paths(db, owner, rows)
}

/// Page deleted roots before resolving any original paths.
pub fn trashed_page(
    db: &Connection,
    owner: &str,
    after: Option<&TrashPosition>,
    limit: usize,
) -> Result<TrashPage, NamespaceError> {
    if limit == 0 || limit > MAX_TRASH_PAGE_SIZE {
        return Err(NamespaceError::InvalidLimit);
    }
    let mut stmt = db
        .prepare(
            "SELECT entry_id,parent_id,original_name,kind,generation FROM entry
         WHERE owner_account_id=?1 AND state='deleted'
         AND (?2 IS NULL OR (original_name,entry_id)>(?2,?3))
         ORDER BY original_name,entry_id LIMIT ?4",
        )
        .map_err(|_| NamespaceError::Sql)?;
    let mut rows = stmt
        .query_map(
            params![
                owner,
                after.map(|p| &p.name),
                after.map(|p| &p.id),
                (limit + 1) as i64
            ],
            deleted_entry,
        )
        .map_err(|_| NamespaceError::Sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| NamespaceError::Sql)?;
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    let mut items = deleted_paths(db, owner, rows)?;
    mark_deleted_references(db, owner, &mut items)?;
    Ok(TrashPage { items, has_more })
}

/// Include ancestors and hidden entries without retaining the account inventory.
pub fn trash_fingerprint(db: &Connection, owner: &str) -> Result<String, NamespaceError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};

    let mut stmt = db
        .prepare(
            "SELECT entry_id,parent_id,original_name,kind,state,generation,current_revision_id
         FROM entry WHERE owner_account_id=?1 ORDER BY parent_id,state,name_key,entry_id",
        )
        .map_err(|_| NamespaceError::Sql)?;
    let mut rows = stmt.query([owner]).map_err(|_| NamespaceError::Sql)?;
    let mut digest = Sha256::new();
    while let Some(row) = rows.next().map_err(|_| NamespaceError::Sql)? {
        for column in [0, 1, 2, 3, 4, 6] {
            let value: Option<String> = row.get(column).map_err(|_| NamespaceError::Sql)?;
            match value {
                Some(value) => {
                    digest.update([1]);
                    digest.update((value.len() as u32).to_be_bytes());
                    digest.update(value.as_bytes());
                }
                None => digest.update([0]),
            }
        }
        let generation: i64 = row.get(5).map_err(|_| NamespaceError::Sql)?;
        digest.update(generation.to_be_bytes());
    }
    Ok(URL_SAFE_NO_PAD.encode(digest.finalize()))
}

type DeletedAncestors = std::collections::HashMap<String, (Option<String>, String)>;

fn deleted_path(
    db: &Connection,
    owner: &str,
    parent_id: Option<&str>,
    ancestors: &mut DeletedAncestors,
) -> Result<Vec<String>, NamespaceError> {
    let mut path = Vec::new();
    let mut current = parent_id.ok_or(NamespaceError::Sql)?.to_owned();
    let mut seen = std::collections::HashSet::new();
    loop {
        if !seen.insert(current.clone()) || seen.len() > (MAX_ENTRY_DEPTH + 1) as usize {
            return Err(NamespaceError::Sql);
        }
        if !ancestors.contains_key(&current) {
            let ancestor: (Option<String>, String) = db.query_row(
                "SELECT parent_id,original_name FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND kind='folder'",
                params![current, owner], |r| Ok((r.get(0)?, r.get(1)?)),
            ).map_err(|_| NamespaceError::Sql)?;
            ancestors.insert(current.clone(), ancestor);
        }
        let (parent, name) = ancestors.get(&current).expect("loaded ancestor");
        match parent {
            Some(parent) => {
                path.push(name.clone());
                current = parent.clone();
            }
            None => break,
        }
    }
    path.reverse();
    Ok(path)
}

fn deleted_paths(
    db: &Connection,
    owner: &str,
    rows: Vec<Entry>,
) -> Result<Vec<DeletedEntry>, NamespaceError> {
    let mut ancestors = DeletedAncestors::new();
    let mut result = Vec::with_capacity(rows.len());
    let mut counts = std::collections::HashMap::new();
    for entry in rows {
        let original_path = deleted_path(db, owner, entry.parent_id.as_deref(), &mut ancestors)?;
        *counts
            .entry((entry.name.clone(), original_path.clone()))
            .or_insert(0) += 1;
        result.push(DeletedEntry {
            entry,
            original_path,
            needs_reference: false,
        });
    }
    for item in &mut result {
        item.needs_reference = counts[&(item.entry.name.clone(), item.original_path.clone())] > 1;
    }
    Ok(result)
}

fn mark_deleted_references(
    db: &Connection,
    owner: &str,
    items: &mut [DeletedEntry],
) -> Result<(), NamespaceError> {
    if items.is_empty() {
        return Ok(());
    }
    let names = items
        .iter()
        .map(|item| item.entry.name.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let placeholders = vec!["?"; names.len()].join(",");
    let mut stmt = db
        .prepare(&format!(
            "SELECT entry_id,parent_id,original_name FROM entry WHERE owner_account_id=?
         AND state='deleted' AND original_name IN ({placeholders})"
        ))
        .map_err(|_| NamespaceError::Sql)?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(
            std::iter::once(owner).chain(names.iter().map(String::as_str)),
        ))
        .map_err(|_| NamespaceError::Sql)?;
    while let Some(row) = rows.next().map_err(|_| NamespaceError::Sql)? {
        let id: String = row.get(0).map_err(|_| NamespaceError::Sql)?;
        let parent: Option<String> = row.get(1).map_err(|_| NamespaceError::Sql)?;
        let name: String = row.get(2).map_err(|_| NamespaceError::Sql)?;
        if !items
            .iter()
            .any(|item| !item.needs_reference && item.entry.name == name && item.entry.id != id)
        {
            continue;
        }
        // Historical parent IDs can differ while the displayed paths coincide.
        let path = deleted_path(db, owner, parent.as_deref(), &mut DeletedAncestors::new())?;
        for item in items.iter_mut() {
            if item.entry.id != id && item.entry.name == name && item.original_path == path {
                item.needs_reference = true;
            }
        }
        if items.iter().all(|item| item.needs_reference) {
            break;
        }
    }
    Ok(())
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
    folder_path(db, owner, &parent).map_err(|failure| match failure {
        NamespaceError::NotFound => NamespaceError::ParentUnavailable,
        other => other,
    })?;
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
    fn sorted_listing_uses_active_sibling_index() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("../../migrations/sqlite/0001_s1.sql"))
            .unwrap();
        for sort in [
            ListingSort::NameAsc,
            ListingSort::NameDesc,
            ListingSort::SizeAsc,
            ListingSort::SizeDesc,
            ListingSort::ModifiedAsc,
            ListingSort::ModifiedDesc,
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
                        Option::<i64>::None,
                        101,
                        100
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
