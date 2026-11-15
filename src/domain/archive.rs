//! Resolve a selection into the fixed member list of one ZIP download.
//!
//! Archive paths are unique: each surviving selected item is a top-level member named by itself,
//! roots with equal sibling keys are refused, and active siblings have unique keys below them.
use super::namespace::{self, MAX_ENTRY_DEPTH, NamespaceError};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

/// Files plus folders, including the selected items themselves.
pub const MAX_ARCHIVE_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveContent {
    Folder,
    File {
        blob_id: String,
        length: u64,
        sha256: [u8; 32],
        modified_at: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Slash-separated path without a trailing slash.
    pub path: String,
    pub content: ArchiveContent,
}

#[derive(Debug)]
pub struct ArchivePlan {
    pub name: String,
    pub entries: Vec<ArchiveEntry>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ArchiveError {
    Sql,
    /// Unknown, foreign, deleted, under a deleted ancestor, or the private root.
    NotFound,
    TooManyEntries,
    NameConflict,
    /// An active file without a live current revision.
    Integrity,
}

impl From<NamespaceError> for ArchiveError {
    fn from(error: NamespaceError) -> Self {
        match error {
            NamespaceError::NotFound => Self::NotFound,
            _ => Self::Sql,
        }
    }
}

struct Selected {
    id: String,
    name: String,
    name_key: Vec<u8>,
    ancestors: Vec<String>,
}

/// Resolve `selected` for `owner` in the caller's read snapshot.
pub fn plan(
    db: &Connection,
    owner: &str,
    selected: &[String],
) -> Result<ArchivePlan, ArchiveError> {
    let requested: HashSet<&str> = selected.iter().map(String::as_str).collect();
    let mut roots = Vec::new();
    for id in &requested {
        let row: Option<(Option<String>, String, String, Vec<u8>)> = db
            .query_row(
                "SELECT parent_id,kind,original_name,name_key FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='active'",
                params![id, owner],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|_| ArchiveError::Sql)?;
        let Some((Some(parent), _, name, name_key)) = row else {
            return Err(ArchiveError::NotFound);
        };
        let ancestors = namespace::folder_path(db, owner, &parent)?
            .into_iter()
            .map(|folder| folder.id)
            .collect();
        roots.push(Selected {
            id: (*id).to_owned(),
            name,
            name_key,
            ancestors,
        });
    }
    // A selected folder covers selected items inside it, as Trash does.
    roots.retain(|root| {
        !root
            .ancestors
            .iter()
            .any(|ancestor| requested.contains(ancestor.as_str()))
    });
    roots.sort_by(|left, right| (&left.name_key, &left.id).cmp(&(&right.name_key, &right.id)));
    if roots
        .windows(2)
        .any(|pair| pair[0].name_key == pair[1].name_key)
    {
        return Err(ArchiveError::NameConflict);
    }
    let name = match roots.as_slice() {
        [only] => format!("{}.zip", only.name),
        _ => "download.zip".to_owned(),
    };
    let mut entries = Vec::new();
    for root in &roots {
        let budget = MAX_ARCHIVE_ENTRIES - entries.len();
        entries.extend(tree(db, owner, &root.id, budget)?);
    }
    Ok(ArchivePlan { name, entries })
}

type TreeRow = (
    String,
    String,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<Vec<u8>>,
);

/// The active tree under `root`, parents before children, or `TooManyEntries` beyond `budget`.
fn tree(
    db: &Connection,
    owner: &str,
    root: &str,
    budget: usize,
) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let mut statement = db
        .prepare(
            "WITH RECURSIVE tree(entry_id,kind,path,depth,current_revision_id) AS (
                SELECT entry_id,kind,original_name,0,current_revision_id FROM entry
                WHERE entry_id=?1 AND owner_account_id=?2 AND state='active'
                UNION ALL
                SELECT e.entry_id,e.kind,t.path||'/'||e.original_name,t.depth+1,e.current_revision_id
                FROM entry e JOIN tree t ON e.parent_id=t.entry_id
                WHERE t.kind='folder' AND e.owner_account_id=?2 AND e.state='active' AND t.depth<?3
                LIMIT ?4
            ) SELECT t.kind,t.path,r.committed_at,b.blob_id,b.length,b.sha256
            FROM tree t
            LEFT JOIN revision r ON t.kind='file' AND r.revision_id=t.current_revision_id AND r.entry_id=t.entry_id
            LEFT JOIN blob b ON b.blob_id=r.blob_id AND b.lifecycle='live'
            ORDER BY t.path",
        )
        .map_err(|_| ArchiveError::Sql)?;
    let rows = statement
        .query_map(
            params![root, owner, MAX_ENTRY_DEPTH, budget as i64 + 1],
            |row| -> rusqlite::Result<TreeRow> {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map_err(|_| ArchiveError::Sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ArchiveError::Sql)?;
    if rows.len() > budget {
        return Err(ArchiveError::TooManyEntries);
    }
    rows.into_iter()
        .map(|(kind, path, committed_at, blob_id, length, sha256)| {
            let content = match kind.as_str() {
                "folder" => ArchiveContent::Folder,
                "file" => {
                    let (Some(modified_at), Some(blob_id), Some(length), Some(sha256)) =
                        (committed_at, blob_id, length, sha256)
                    else {
                        return Err(ArchiveError::Integrity);
                    };
                    ArchiveContent::File {
                        blob_id,
                        length: u64::try_from(length).map_err(|_| ArchiveError::Integrity)?,
                        sha256: sha256.try_into().map_err(|_| ArchiveError::Integrity)?,
                        modified_at,
                    }
                }
                _ => return Err(ArchiveError::Integrity),
            };
            Ok(ArchiveEntry { path, content })
        })
        .collect()
}
