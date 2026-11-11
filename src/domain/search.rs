//! Owner-scoped search of the active private tree.
use std::collections::HashMap;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use super::{
    namespace::{Entry, MAX_ENTRY_DEPTH, NamespaceError},
    upload::folder_etag,
};

#[derive(Clone, serde::Serialize)]
pub struct SearchHit {
    pub entry: Entry,
    pub location: Vec<Entry>,
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

pub fn find(db: &Connection, owner: &str, query: &str) -> Result<SearchResults, NamespaceError> {
    let root = super::namespace::private_root(db, owner)?;
    let mut stmt = db.prepare(
        "WITH RECURSIVE live(entry_id,parent_id,original_name,kind,generation,depth) AS (\
         SELECT entry_id,parent_id,original_name,kind,generation,0 FROM entry \
         WHERE entry_id=?1 AND owner_account_id=?2 AND parent_id IS NULL AND state='active' \
         UNION ALL \
         SELECT child.entry_id,child.parent_id,child.original_name,child.kind,child.generation,live.depth+1 \
         FROM entry child JOIN live ON child.parent_id=live.entry_id \
         WHERE child.owner_account_id=?2 AND child.state='active' AND live.depth<?3) \
         SELECT entry_id,parent_id,original_name,kind,generation FROM live ORDER BY entry_id"
    ).map_err(|_| NamespaceError::Sql)?;
    let entries: Vec<Entry> = stmt
        .query_map(params![root, owner, MAX_ENTRY_DEPTH], |r| {
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
        .collect::<Result<_, _>>()
        .map_err(|_| NamespaceError::Sql)?;
    let by_id: HashMap<&str, &Entry> = entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let mut digest = Sha256::new();
    let needle = fold(query);
    let mut matches = Vec::new();
    for entry in &entries {
        digest.update((entry.id.len() as u32).to_be_bytes());
        digest.update(entry.id.as_bytes());
        digest.update((entry.name.len() as u32).to_be_bytes());
        digest.update(entry.name.as_bytes());
        digest.update(entry.kind.as_bytes());
        digest.update(entry.generation.to_be_bytes());
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
            },
        ));
    }
    matches.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.entry.id.cmp(&b.2.entry.id))
    });
    let hits = matches.into_iter().map(|(_, _, hit)| hit).collect();
    Ok(SearchResults {
        hits,
        fingerprint: URL_SAFE_NO_PAD.encode(digest.finalize()),
    })
}
