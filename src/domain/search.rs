//! Owner-scoped search of the active private tree.
use std::collections::HashMap;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use super::{
    namespace::{Entry, MAX_ENTRY_DEPTH, NamespaceError, ResultSort},
    upload::folder_etag,
};

#[derive(Clone)]
pub struct SearchHit {
    pub entry: Entry,
    pub location: Vec<Entry>,
    pub size: Option<i64>,
    pub modified_at: Option<i64>,
}

impl serde::Serialize for SearchHit {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(serde::Serialize)]
        struct ResultEntry<'a> {
            #[serde(flatten)]
            entry: &'a Entry,
            size: Option<i64>,
            modified_at: Option<i64>,
        }
        #[derive(serde::Serialize)]
        struct Hit<'a> {
            entry: ResultEntry<'a>,
            location: &'a [Entry],
        }
        Hit {
            entry: ResultEntry {
                entry: &self.entry,
                size: self.size,
                modified_at: self.modified_at,
            },
            location: &self.location,
        }
        .serialize(serializer)
    }
}

pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub fingerprint: String,
}

pub fn fold(value: &str) -> String {
    value
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .nfc()
        .collect()
}

// Keep live outermost so each child lookup uses owner, parent and state.
const LIVE_SQL: &str = "WITH RECURSIVE live(entry_id,parent_id,original_name,kind,generation,depth) AS (\
         SELECT entry_id,parent_id,original_name,kind,generation,0 FROM entry \
         WHERE entry_id=?1 AND owner_account_id=?2 AND parent_id IS NULL AND state='active' \
         UNION ALL \
         SELECT child.entry_id,child.parent_id,child.original_name,child.kind,child.generation,live.depth+1 \
         FROM live CROSS JOIN entry child ON child.parent_id=live.entry_id \
         WHERE child.owner_account_id=?2 AND child.state='active' AND live.depth<?3) \
         SELECT live.entry_id,live.parent_id,live.original_name,live.kind,live.generation,\
         CASE live.kind WHEN 'file' THEN r.length END,\
         CASE live.kind WHEN 'file' THEN r.committed_at END \
         FROM live LEFT JOIN entry e ON e.entry_id=live.entry_id \
         LEFT JOIN revision r ON r.revision_id=e.current_revision_id AND r.entry_id=e.entry_id \
         ORDER BY live.entry_id";

pub fn find(db: &Connection, owner: &str, query: &str) -> Result<SearchResults, NamespaceError> {
    find_sorted(db, owner, query, ResultSort::NameAsc)
}

pub fn find_sorted(
    db: &Connection,
    owner: &str,
    query: &str,
    sort: ResultSort,
) -> Result<SearchResults, NamespaceError> {
    let root = super::namespace::private_root(db, owner)?;
    let mut stmt = db.prepare(LIVE_SQL).map_err(|_| NamespaceError::Sql)?;
    let rows: Vec<(Entry, Option<i64>, Option<i64>)> = stmt
        .query_map(params![root, owner, MAX_ENTRY_DEPTH], |r| {
            let id: String = r.get(0)?;
            let generation: i64 = r.get(4)?;
            Ok((
                Entry {
                    etag: folder_etag(&id, generation as u64),
                    id,
                    parent_id: r.get(1)?,
                    name: r.get(2)?,
                    kind: r.get(3)?,
                    generation,
                },
                r.get(5)?,
                r.get(6)?,
            ))
        })
        .map_err(|_| NamespaceError::Sql)?
        .collect::<Result<_, _>>()
        .map_err(|_| NamespaceError::Sql)?;
    let by_id: HashMap<&str, &Entry> = rows
        .iter()
        .map(|(entry, _, _)| (entry.id.as_str(), entry))
        .collect();
    let mut digest = Sha256::new();
    let needle = fold(query);
    let mut matches = Vec::new();
    for (entry, size, modified_at) in &rows {
        digest.update((entry.id.len() as u32).to_be_bytes());
        digest.update(entry.id.as_bytes());
        digest.update((entry.name.len() as u32).to_be_bytes());
        digest.update(entry.name.as_bytes());
        digest.update(entry.kind.as_bytes());
        digest.update(entry.generation.to_be_bytes());
        for value in [size, modified_at] {
            digest.update([u8::from(value.is_some())]);
            digest.update(value.unwrap_or_default().to_be_bytes());
        }
        digest.update(entry.parent_id.as_deref().unwrap_or("").as_bytes());
        if entry.id == root || !fold(&entry.name).contains(&needle) {
            continue;
        }
        let mut location = Vec::new();
        let mut parent = entry.parent_id.as_deref();
        while let Some(id) = parent {
            let folder = by_id.get(id).ok_or(NamespaceError::Sql)?;
            if folder.kind != "folder" || location.len() > MAX_ENTRY_DEPTH as usize {
                return Err(NamespaceError::Sql);
            }
            location.push((*folder).clone());
            parent = folder.parent_id.as_deref();
        }
        location.reverse();
        if location.first().map(|folder| folder.id.as_str()) != Some(root.as_str()) {
            return Err(NamespaceError::Sql);
        }
        let location_key = location
            .iter()
            .skip(1)
            .map(|part| fold(&part.name))
            .collect::<Vec<_>>()
            .join("\0");
        matches.push((
            fold(&entry.name),
            location_key,
            SearchHit {
                entry: entry.clone(),
                location,
                size: *size,
                modified_at: *modified_at,
            },
        ));
    }
    matches.sort_by(|a, b| {
        let name = || a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1));
        let folders = || (a.2.entry.kind != "folder").cmp(&(b.2.entry.kind != "folder"));
        let metric = |left: Option<i64>, right: Option<i64>, descending: bool| {
            left.is_none().cmp(&right.is_none()).then_with(|| {
                if descending {
                    right.cmp(&left)
                } else {
                    left.cmp(&right)
                }
            })
        };
        let order = match sort {
            ResultSort::NameAsc => name(),
            ResultSort::NameDesc => b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)),
            ResultSort::LocationAsc => a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)),
            ResultSort::LocationDesc => b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)),
            ResultSort::SizeAsc
            | ResultSort::SizeDesc
            | ResultSort::ModifiedAsc
            | ResultSort::ModifiedDesc => folders().then_with(|| {
                if a.2.entry.kind == "folder" {
                    return name();
                }
                match sort {
                    ResultSort::SizeAsc => metric(a.2.size, b.2.size, false),
                    ResultSort::SizeDesc => metric(a.2.size, b.2.size, true),
                    ResultSort::ModifiedAsc => metric(a.2.modified_at, b.2.modified_at, false),
                    ResultSort::ModifiedDesc => metric(a.2.modified_at, b.2.modified_at, true),
                    _ => unreachable!(),
                }
                .then_with(name)
            }),
        };
        order.then_with(|| a.2.entry.id.cmp(&b.2.entry.id))
    });
    let hits = matches.into_iter().map(|(_, _, hit)| hit).collect();
    Ok(SearchResults {
        hits,
        fingerprint: URL_SAFE_NO_PAD.encode(digest.finalize()),
    })
}

#[cfg(test)]
mod tests;
