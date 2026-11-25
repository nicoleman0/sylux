//! Private library and entry metadata. Callers must authenticate the owner.
use super::names::{EntryName, NAME_KEY_VERSION};
use super::upload::folder_etag;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

pub(super) const MAX_ENTRY_DEPTH: i64 = 128;
const MAX_COPY_ENTRIES: usize = 10_000;
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
    CopyTooLarge,
    SharingConfirmationRequired,
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

pub(super) fn id() -> Result<String, NamespaceError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| NamespaceError::Sql)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn library_root(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let id: String = row.get(0)?;
    let generation: i64 = row.get(2)?;
    Ok(Entry {
        etag: folder_etag(&id, generation as u64),
        id,
        parent_id: None,
        name: row.get(1)?,
        kind: "folder".into(),
        generation,
    })
}

/// An unqualified listing keeps resolving to the original account root.
pub fn root_entry(db: &Connection, owner: &str) -> Result<Entry, NamespaceError> {
    db.query_row(
        "SELECT entry_id, original_name, generation FROM entry WHERE owner_account_id=?1
         AND parent_id IS NULL AND library_name IS NULL AND kind='folder' AND state='active'",
        [owner],
        library_root,
    )
    .optional()
    .map_err(|_| NamespaceError::Sql)?
    .ok_or(NamespaceError::NotFound)
}

pub fn libraries(db: &Connection, owner: &str) -> Result<Vec<Entry>, NamespaceError> {
    let mut statement = db.prepare(
        "SELECT entry_id, library_display_name, generation FROM entry
         WHERE owner_account_id=?1 AND parent_id IS NULL AND kind='folder' AND state='active'
         ORDER BY library_name IS NOT NULL, coalesce(library_name_key, CAST(library_display_name AS BLOB))",
    ).map_err(|_| NamespaceError::Sql)?;
    statement
        .query_map([owner], library_root)
        .map_err(|_| NamespaceError::Sql)?
        .collect::<Result<_, _>>()
        .map_err(|_| NamespaceError::Sql)
}

pub fn create_library(
    db: &Transaction<'_>,
    owner: &str,
    name: &str,
) -> Result<Entry, NamespaceError> {
    if name.trim().is_empty() {
        return Err(NamespaceError::InvalidName);
    }
    let name = EntryName::parse(name).map_err(|_| NamespaceError::InvalidName)?;
    let id = id()?;
    db.query_row(
        "INSERT INTO entry(entry_id, owner_account_id, kind, original_name, name_key,
                           name_key_version, library_name, library_name_key)
         VALUES(?1, ?2, 'folder', '', x'', ?3, ?4, ?5)
         ON CONFLICT DO NOTHING
         RETURNING entry_id, library_display_name, generation",
        params![id, owner, NAME_KEY_VERSION, name.display, name.key],
        library_root,
    )
    .optional()
    .map_err(|_| NamespaceError::Sql)?
    .ok_or(NamespaceError::Conflict)
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
            SELECT entry_id,parent_id,coalesce(library_name,original_name),generation,0 FROM entry
            WHERE entry_id=?1 AND owner_account_id=?2 AND kind='folder' AND state='active'
            UNION ALL
            SELECT e.entry_id,e.parent_id,coalesce(e.library_name,e.original_name),e.generation,a.depth+1
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSort {
    #[default]
    NameAsc,
    NameDesc,
    SizeAsc,
    SizeDesc,
    ModifiedAsc,
    ModifiedDesc,
    LocationAsc,
    LocationDesc,
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
    move_with_sharing_check(
        db,
        owner,
        entry,
        destination,
        if_match,
        |source, destination| require_private_tree_change(db, owner, entry, source, destination),
    )
}

pub(crate) fn move_with_sharing_check(
    db: &Transaction<'_>,
    owner: &str,
    entry: &str,
    destination: &str,
    if_match: &str,
    check: impl FnOnce(&[Entry], &[Entry]) -> Result<(), NamespaceError>,
) -> Result<Entry, NamespaceError> {
    let row: Option<(Option<String>, String, String, i64)> = db.query_row(
        "SELECT parent_id,original_name,kind,generation FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='active'",
        params![entry, owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    ).optional().map_err(|_| NamespaceError::Sql)?;
    let Some((Some(parent), name, kind, generation)) = row else {
        return Err(NamespaceError::NotFound);
    };
    let source_path = folder_path(db, owner, &parent)?;
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
    let destination_path = folder_path(db, owner, destination)?;
    check(&source_path, &destination_path)?;
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

/// Copy the reachable live tree inside a transaction that rolls back on any error.
pub(crate) fn copy_entry(
    db: &Transaction<'_>,
    owner: &str,
    source: &str,
    destination: &str,
    if_match: &str,
    copy_root: &str,
    now: i64,
) -> Result<(), NamespaceError> {
    let (parent, generation): (String, i64) = db
        .query_row(
            "SELECT parent_id,generation FROM entry
             WHERE entry_id=?1 AND owner_account_id=?2 AND state='active' AND parent_id IS NOT NULL",
            params![source, owner],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| NamespaceError::Sql)?
        .ok_or(NamespaceError::NotFound)?;
    folder_path(db, owner, &parent)?;
    if if_match != folder_etag(source, generation as u64) {
        return Err(NamespaceError::Stale);
    }
    let path = folder_path(db, owner, destination)?;
    if path.iter().any(|folder| folder.id == source) {
        return Err(NamespaceError::InvalidDestination);
    }
    let conflict: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM entry source CROSS JOIN entry sibling ON sibling.name_key=source.name_key
         WHERE source.entry_id=?1 AND sibling.owner_account_id=?2 AND sibling.parent_id=?3 AND sibling.state='active')",
        params![source, owner, destination],
        |row| row.get(0),
    ).map_err(|_| NamespaceError::Sql)?;
    if conflict {
        return Err(NamespaceError::Conflict);
    }

    // Each indexed child lookup stops at the remaining budget, even for wide folders.
    let mut children = db
        .prepare(
            "SELECT entry_id,parent_id,kind='folder' FROM entry
         WHERE owner_account_id=?1 AND parent_id=?2 AND state='active'
         ORDER BY name_key,entry_id LIMIT ?3",
        )
        .map_err(|_| NamespaceError::Sql)?;
    let source_is_folder: bool = db
        .query_row(
            "SELECT kind='folder' FROM entry WHERE entry_id=?1",
            [source],
            |row| row.get(0),
        )
        .map_err(|_| NamespaceError::Sql)?;
    let mut members = vec![(source.to_owned(), parent, source_is_folder, 0i64)];
    let mut index = 0;
    while index < members.len() {
        let (ref folder, _, is_folder, depth) = members[index];
        if is_folder {
            let remaining = MAX_COPY_ENTRIES + 1 - members.len();
            let found: Vec<(String, String, bool, i64)> = children
                .query_map(params![owner, folder, remaining as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, depth + 1))
                })
                .and_then(|rows| rows.collect())
                .map_err(|_| NamespaceError::Sql)?;
            members.extend(found);
            if members.len() > MAX_COPY_ENTRIES {
                return Err(NamespaceError::CopyTooLarge);
            }
        }
        index += 1;
    }
    if members
        .iter()
        .any(|member| path.len() as i64 + member.3 > MAX_ENTRY_DEPTH)
    {
        return Err(NamespaceError::DepthLimit);
    }

    let mut create_entry = db.prepare(
        "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
         SELECT ?2,owner_account_id,?3,kind,original_name,name_key,name_key_version FROM entry WHERE entry_id=?1",
    ).map_err(|_| NamespaceError::Sql)?;
    let mut create_revision = db.prepare(
        "INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id)
         SELECT ?2,?3,r.blob_id,r.length,r.sha256,?4,e.owner_account_id FROM entry e
         JOIN revision r ON r.revision_id=e.current_revision_id AND r.entry_id=e.entry_id WHERE e.entry_id=?1",
    ).map_err(|_| NamespaceError::Sql)?;
    let mut set_revision = db
        .prepare("UPDATE entry SET current_revision_id=?2 WHERE entry_id=?1")
        .map_err(|_| NamespaceError::Sql)?;
    let mut copied_folders = std::collections::HashMap::new();
    for (entry, parent, is_folder, depth) in members {
        let (copy, copy_parent) = if depth == 0 {
            (copy_root.to_owned(), destination)
        } else {
            let copy_parent: &String = copied_folders.get(&parent).ok_or(NamespaceError::Sql)?;
            (id()?, copy_parent.as_str())
        };
        create_entry
            .execute(params![entry, copy, copy_parent])
            .map_err(|_| NamespaceError::Sql)?;
        if is_folder {
            copied_folders.insert(entry, copy);
        } else {
            let revision = id()?;
            let inserted = create_revision
                .execute(params![entry, revision, copy, now])
                .map_err(|_| NamespaceError::Sql)?;
            if inserted != 1 {
                return Err(NamespaceError::Sql);
            }
            set_revision
                .execute(params![copy, revision])
                .map_err(|_| NamespaceError::Sql)?;
        }
    }
    db.execute(
        "UPDATE entry SET generation=generation+1 WHERE entry_id=?1",
        [destination],
    )
    .map_err(|_| NamespaceError::Sql)?;
    Ok(())
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_name: Option<String>,
    pub size: Option<i64>,
    pub modified_at: Option<i64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub needs_reference: bool,
}

pub struct TrashPosition {
    pub name: String,
    pub id: String,
}

struct TrashSortPosition {
    kind_rank: u8,
    name: String,
    size: Option<i64>,
    modified_at: Option<i64>,
    location: String,
}

pub struct TrashPage {
    pub items: Vec<DeletedEntry>,
    pub has_more: bool,
}

fn deleted_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<(Entry, Option<i64>, Option<i64>)> {
    let id: String = row.get(0)?;
    let generation: i64 = row.get(4)?;
    Ok((
        Entry {
            etag: folder_etag(&id, generation as u64),
            id,
            parent_id: row.get(1)?,
            name: row.get(2)?,
            kind: row.get(3)?,
            generation,
        },
        row.get(5)?,
        row.get(6)?,
    ))
}

pub fn trashed(db: &Connection, owner: &str) -> Result<Vec<DeletedEntry>, NamespaceError> {
    let mut stmt = db.prepare("SELECT e.entry_id,e.parent_id,e.original_name,e.kind,e.generation,CASE e.kind WHEN 'file' THEN r.length END,CASE e.kind WHEN 'file' THEN r.committed_at END FROM entry e LEFT JOIN revision r ON r.revision_id=e.current_revision_id AND r.entry_id=e.entry_id WHERE e.owner_account_id=?1 AND e.state='deleted' AND NOT EXISTS(SELECT 1 FROM removal_member rm WHERE rm.entry_id=e.entry_id) ORDER BY e.original_name,e.entry_id").map_err(|_| NamespaceError::Sql)?;
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
    trashed_page_sorted(db, owner, ResultSort::NameAsc, after, limit)
}

/// Sort every deleted root in SQL before bounding the page and resolving its paths.
pub fn trashed_page_sorted(
    db: &Connection,
    owner: &str,
    sort: ResultSort,
    after: Option<&TrashPosition>,
    limit: usize,
) -> Result<TrashPage, NamespaceError> {
    if limit == 0 || limit > MAX_TRASH_PAGE_SIZE {
        return Err(NamespaceError::InvalidLimit);
    }
    let name_after = "(original_name>?3 OR (original_name=?3 AND entry_id>?6))";
    let (predicate, order, location) = match sort {
        ResultSort::NameAsc => (name_after.to_owned(), "original_name,entry_id", false),
        ResultSort::NameDesc => (
            "(original_name<?3 OR (original_name=?3 AND entry_id>?6))".to_owned(),
            "original_name DESC,entry_id",
            false,
        ),
        ResultSort::LocationAsc | ResultSort::LocationDesc => {
            let comparison = if sort == ResultSort::LocationAsc {
                ">"
            } else {
                "<"
            };
            (
                format!("(location_key{comparison}?7 OR (location_key=?7 AND {name_after}))"),
                if sort == ResultSort::LocationAsc {
                    "location_key,original_name,entry_id"
                } else {
                    "location_key DESC,original_name,entry_id"
                },
                true,
            )
        }
        ResultSort::SizeAsc
        | ResultSort::SizeDesc
        | ResultSort::ModifiedAsc
        | ResultSort::ModifiedDesc => {
            let (column, parameter, comparison, order) = match sort {
                ResultSort::SizeAsc => (
                    "byte_size",
                    "?4",
                    ">",
                    "kind_rank,CASE WHEN kind_rank=0 THEN original_name END,byte_size IS NULL,byte_size,original_name,entry_id",
                ),
                ResultSort::SizeDesc => (
                    "byte_size",
                    "?4",
                    "<",
                    "kind_rank,CASE WHEN kind_rank=0 THEN original_name END,byte_size IS NULL,byte_size DESC,original_name,entry_id",
                ),
                ResultSort::ModifiedAsc => (
                    "modified_at",
                    "?5",
                    ">",
                    "kind_rank,CASE WHEN kind_rank=0 THEN original_name END,modified_at IS NULL,modified_at,original_name,entry_id",
                ),
                ResultSort::ModifiedDesc => (
                    "modified_at",
                    "?5",
                    "<",
                    "kind_rank,CASE WHEN kind_rank=0 THEN original_name END,modified_at IS NULL,modified_at DESC,original_name,entry_id",
                ),
                _ => unreachable!(),
            };
            (
                format!(
                    "(kind_rank>?2 OR (kind_rank=?2 AND ((kind_rank=0 AND {name_after}) OR (kind_rank=1 AND (({parameter} IS NOT NULL AND ({column} IS NULL OR {column}{comparison}{parameter} OR ({column}={parameter} AND {name_after}))) OR ({parameter} IS NULL AND {column} IS NULL AND {name_after}))))))"
                ),
                order,
                false,
            )
        }
    };
    let paths = if location {
        format!("paths(entry_id,location_key,child_key,depth) AS (
            SELECT entry_id,'',coalesce(CAST(library_name_key AS TEXT),library_display_name) || char(0),0
            FROM entry WHERE owner_account_id=?1 AND parent_id IS NULL
            UNION ALL
            SELECT e.entry_id,p.child_key,p.child_key || CAST(e.name_key AS TEXT) || char(0),p.depth+1
            FROM paths p CROSS JOIN entry e ON e.parent_id=p.entry_id
            WHERE e.owner_account_id=?1 AND p.depth<{MAX_ENTRY_DEPTH}
        ),")
    } else {
        String::new()
    };
    let location_column = if location { "p.location_key" } else { "''" };
    let location_join = if location {
        "JOIN paths p ON p.entry_id=e.entry_id"
    } else {
        ""
    };
    let listing = format!("WITH RECURSIVE {paths} listing AS (
        SELECT e.entry_id,e.parent_id,e.original_name,e.kind,e.generation,
            CASE e.kind WHEN 'file' THEN r.length END AS byte_size,
            CASE e.kind WHEN 'file' THEN r.committed_at END AS modified_at,
            CASE e.kind WHEN 'folder' THEN 0 ELSE 1 END AS kind_rank,
            {location_column} AS location_key
        FROM entry e LEFT JOIN revision r ON r.revision_id=e.current_revision_id AND r.entry_id=e.entry_id
        {location_join} WHERE e.owner_account_id=?1 AND e.state='deleted' AND NOT EXISTS(SELECT 1 FROM removal_member rm WHERE rm.entry_id=e.entry_id)
    )");
    let pivot = if let Some(after) = after {
        // nosemgrep: sylux.rust.sql-built-with-format -- listing is a constant CTE; values bind as ?1/?2
        Some(db.query_row(
            &format!("{listing} SELECT kind_rank,original_name,byte_size,modified_at,location_key FROM listing WHERE entry_id=?2"),
            params![owner, after.id],
            |row| Ok(TrashSortPosition {
                kind_rank: row.get(0)?,
                name: row.get(1)?,
                size: row.get(2)?,
                modified_at: row.get(3)?,
                location: row.get(4)?,
            }),
        ).map_err(|_| NamespaceError::Sql)?)
    } else {
        None
    };
    let sql = format!(
        "{listing} SELECT entry_id,parent_id,original_name,kind,generation,byte_size,modified_at FROM listing WHERE ?6 IS NULL OR {predicate} ORDER BY {order} LIMIT ?8"
    );
    let mut stmt = db.prepare(&sql).map_err(|_| NamespaceError::Sql)?;
    let mut rows = stmt
        .query_map(
            params![
                owner,
                pivot.as_ref().map(|p| p.kind_rank),
                pivot.as_ref().map(|p| p.name.as_str()),
                pivot.as_ref().and_then(|p| p.size),
                pivot.as_ref().and_then(|p| p.modified_at),
                after.map(|p| p.id.as_str()),
                pivot.as_ref().map(|p| p.location.as_str()),
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
            "SELECT e.entry_id,parent_id,original_name,e.kind,e.state,e.generation,e.current_revision_id,r.length,r.committed_at
         FROM entry e LEFT JOIN revision r ON r.revision_id=e.current_revision_id AND r.entry_id=e.entry_id WHERE owner_account_id=?1 ORDER BY parent_id,state,name_key,e.entry_id",
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
        for column in [7, 8] {
            let value: Option<i64> = row.get(column).map_err(|_| NamespaceError::Sql)?;
            digest.update([u8::from(value.is_some())]);
            digest.update(value.unwrap_or_default().to_be_bytes());
        }
    }
    Ok(URL_SAFE_NO_PAD.encode(digest.finalize()))
}

type DeletedAncestors = std::collections::HashMap<String, (Option<String>, String)>;

fn deleted_path(
    db: &Connection,
    owner: &str,
    parent_id: Option<&str>,
    ancestors: &mut DeletedAncestors,
) -> Result<(Option<String>, Vec<String>), NamespaceError> {
    let mut path = Vec::new();
    let mut current = parent_id.ok_or(NamespaceError::Sql)?.to_owned();
    let mut seen = std::collections::HashSet::new();
    let library = loop {
        if !seen.insert(current.clone()) || seen.len() > (MAX_ENTRY_DEPTH + 1) as usize {
            return Err(NamespaceError::Sql);
        }
        if !ancestors.contains_key(&current) {
            let ancestor: (Option<String>, String) = db.query_row(
                "SELECT parent_id,coalesce(library_name,original_name) FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND kind='folder'",
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
            None => break (!name.is_empty()).then(|| name.clone()),
        }
    };
    path.reverse();
    Ok((library, path))
}

fn deleted_paths(
    db: &Connection,
    owner: &str,
    rows: Vec<(Entry, Option<i64>, Option<i64>)>,
) -> Result<Vec<DeletedEntry>, NamespaceError> {
    let mut ancestors = DeletedAncestors::new();
    let mut result = Vec::with_capacity(rows.len());
    let mut counts = std::collections::HashMap::new();
    for (entry, size, modified_at) in rows {
        let (library_name, original_path) =
            deleted_path(db, owner, entry.parent_id.as_deref(), &mut ancestors)?;
        *counts
            .entry((
                entry.name.clone(),
                library_name.clone(),
                original_path.clone(),
            ))
            .or_insert(0) += 1;
        result.push(DeletedEntry {
            entry,
            original_path,
            library_name,
            size,
            modified_at,
            needs_reference: false,
        });
    }
    for item in &mut result {
        item.needs_reference = counts[&(
            item.entry.name.clone(),
            item.library_name.clone(),
            item.original_path.clone(),
        )] > 1;
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
    // nosemgrep: sylux.rust.sql-built-with-format -- only ? placeholders are interpolated
    let mut stmt = db
        .prepare(&format!(
            "SELECT entry_id,parent_id,original_name FROM entry WHERE owner_account_id=?
         AND state='deleted' AND NOT EXISTS(SELECT 1 FROM removal_member rm WHERE rm.entry_id=entry.entry_id) AND original_name IN ({placeholders})"
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
        let (library, path) =
            deleted_path(db, owner, parent.as_deref(), &mut DeletedAncestors::new())?;
        for item in items.iter_mut() {
            if item.entry.id != id
                && item.entry.name == name
                && item.library_name == library
                && item.original_path == path
            {
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
    restore_with_sharing_check(db, owner, entry, if_match, |destination| {
        require_private_tree_change(db, owner, entry, &[], destination)
    })
}

pub(crate) fn restore_with_sharing_check(
    db: &Transaction<'_>,
    owner: &str,
    entry: &str,
    if_match: &str,
    check: impl FnOnce(&[Entry]) -> Result<(), NamespaceError>,
) -> Result<(), NamespaceError> {
    let row: Option<(String, i64)> = db.query_row("SELECT parent_id,generation FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='deleted' AND NOT EXISTS(SELECT 1 FROM removal_member rm WHERE rm.entry_id=entry.entry_id)", params![entry,owner], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_| NamespaceError::Sql)?;
    let Some((parent, generation)) = row else {
        return Err(NamespaceError::NotFound);
    };
    if if_match != folder_etag(entry, generation as u64) {
        return Err(NamespaceError::Stale);
    }
    let destination = folder_path(db, owner, &parent).map_err(|failure| match failure {
        NamespaceError::NotFound => NamespaceError::ParentUnavailable,
        other => other,
    })?;
    check(&destination)?;
    db.execute("UPDATE entry SET state='active',generation=generation+1 WHERE entry_id=?1", [entry]).map_err(|e| if matches!(e,rusqlite::Error::SqliteFailure(ref x,_) if x.code==rusqlite::ErrorCode::ConstraintViolation) { NamespaceError::Conflict } else { NamespaceError::Sql })?;
    db.execute(
        "UPDATE entry SET generation=generation+1 WHERE entry_id=?1",
        [parent],
    )
    .map_err(|_| NamespaceError::Sql)?;
    Ok(())
}

fn require_private_tree_change(
    db: &Transaction<'_>,
    owner: &str,
    entry: &str,
    source: &[Entry],
    destination: &[Entry],
) -> Result<(), NamespaceError> {
    let impact = super::direct_grants::tree_impact(db, owner, entry, source, destination)?;
    if impact.requires_confirmation() {
        return Err(NamespaceError::SharingConfirmationRequired);
    }
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
        crate::persistence::schema::migrate_v1(&db, None).unwrap();
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
    use crate::{domain::names::EntryName, persistence::schema::MIGRATION_0001_S1};
    use rusqlite::Connection;
    const OWNER: &str = "00000000000000000000000000000001";
    const ROOT: &str = "00000000000000000000000000000002";

    fn hex(n: usize) -> String {
        format!("{n:032x}")
    }

    fn fixture(files: usize) -> (Connection, String, String, String) {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        db.execute_batch(MIGRATION_0001_S1).unwrap();
        crate::persistence::schema::migrate_v1(&db, None).unwrap();
        crate::persistence::schema::migrate_v2(&db, None).unwrap();
        crate::persistence::schema::migrate_v3(&db, None).unwrap();
        db.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,?1,'user',1,0)", [OWNER]).unwrap();
        db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,NULL,'folder','',x'',1)", params![ROOT,OWNER]).unwrap();
        let folder = |id: &str, parent: &str, name: &str| {
            let n = EntryName::parse(name).unwrap();
            db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder',?4,?5,1)", params![id,OWNER,parent,n.display,n.key]).unwrap();
        };
        let src = hex(0x10);
        let dst = hex(0x11);
        folder(&src, ROOT, "source");
        folder(&dst, ROOT, "dest");
        db.execute("INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,substr(?1,1,2),1,zeroblob(32),'live')", [hex(0xb)]).unwrap();
        db.execute_batch("BEGIN").unwrap();
        for i in 0..files {
            let e = hex(0x100000 + i);
            let r = hex(0x900000 + i);
            let n = EntryName::parse(&format!("f{i:07}")).unwrap();
            db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'file',?4,?5,1)", params![e,OWNER,src,n.display,n.key]).unwrap();
            db.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?2,?3,1,zeroblob(32),1,?4)", params![r,e,hex(0xb),OWNER]).unwrap();
            db.execute(
                "UPDATE entry SET current_revision_id=?2 WHERE entry_id=?1",
                params![e, r],
            )
            .unwrap();
        }
        db.execute_batch("COMMIT").unwrap();
        let generation: i64 = db
            .query_row(
                "SELECT generation FROM entry WHERE entry_id=?1",
                [&src],
                |r| r.get(0),
            )
            .unwrap();
        let etag = folder_etag(&src, generation as u64);
        (db, src, dst, etag)
    }

    fn chain(db: &Connection, start: &str, base: usize, len: usize) -> String {
        let mut parent = start.to_owned();
        for i in 0..len {
            let id = hex(base + i);
            let n = EntryName::parse(&format!("d{i:03}")).unwrap();
            db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder',?4,?5,1)", params![id,OWNER,parent,n.display,n.key]).unwrap();
            parent = id;
        }
        parent
    }

    #[test]
    fn copy_size_boundary_is_atomic() {
        for files in [9_999usize, 10_000] {
            let (mut db, src, dst, etag) = fixture(files);
            let tx = db.transaction().unwrap();
            let before: i64 = tx
                .query_row("SELECT COUNT(*) FROM entry", [], |r| r.get(0))
                .unwrap();
            let generation: i64 = tx
                .query_row(
                    "SELECT generation FROM entry WHERE entry_id=?1",
                    [&dst],
                    |r| r.get(0),
                )
                .unwrap();
            let result = copy_entry(&tx, OWNER, &src, &dst, &etag, &hex(0xc0ffee), 2);
            if files == 9_999 {
                result.unwrap();
                let after: i64 = tx
                    .query_row(
                        "SELECT generation FROM entry WHERE entry_id=?1",
                        [&dst],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(after, generation + 1);
            } else {
                assert!(matches!(result, Err(NamespaceError::CopyTooLarge)));
                let after: i64 = tx
                    .query_row("SELECT COUNT(*) FROM entry", [], |r| r.get(0))
                    .unwrap();
                let after_generation: i64 = tx
                    .query_row(
                        "SELECT generation FROM entry WHERE entry_id=?1",
                        [&dst],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(before, after);
                assert_eq!(generation, after_generation);
            }
        }
    }

    #[test]
    fn copy_depth_boundary_is_atomic() {
        // Source subtree depth 9 (root + 9 nested folders below it), destination at depth D below Files.
        for (dest_depth, expect_ok) in [(118usize, true), (119usize, false)] {
            let (mut db, src, _dst, _etag) = fixture(0);
            chain(&db, &src, 0x200000, 9);
            let dest = chain(&db, ROOT, 0x300000, dest_depth);
            let generation: i64 = db
                .query_row(
                    "SELECT generation FROM entry WHERE entry_id=?1",
                    [&src],
                    |r| r.get(0),
                )
                .unwrap();
            let etag = folder_etag(&src, generation as u64);
            let tx = db.transaction().unwrap();
            let before: i64 = tx
                .query_row("SELECT COUNT(*) FROM entry", [], |r| r.get(0))
                .unwrap();
            let result = copy_entry(&tx, OWNER, &src, &dest, &etag, &hex(0xc0ffee), 2);
            let after: i64 = tx
                .query_row("SELECT COUNT(*) FROM entry", [], |r| r.get(0))
                .unwrap();
            assert_eq!(result.is_ok(), expect_ok);
            if !expect_ok {
                assert!(matches!(result, Err(NamespaceError::DepthLimit)));
                assert_eq!(before, after);
            }
        }
    }
}
