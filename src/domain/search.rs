//! Owner-scoped search within a folder or across private libraries.
use std::collections::HashMap;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use super::{
    namespace::{self, Entry, MAX_ENTRY_DEPTH, NamespaceError, ResultSort},
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
    pub folder_id: Option<String>,
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
         SELECT entry_id,parent_id,coalesce(library_name,original_name),kind,generation,0 FROM entry \
         WHERE entry_id=?1 AND owner_account_id=?2 AND kind='folder' AND state='active' \
         UNION ALL \
         SELECT entry_id,parent_id,coalesce(library_name,original_name),kind,generation,0 FROM entry \
         WHERE ?1 IS NULL AND owner_account_id=?2 AND parent_id IS NULL AND kind='folder' AND state='active' \
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
    find_sorted(db, owner, None, query, ResultSort::NameAsc)
}

fn fingerprint_entry(
    digest: &mut Sha256,
    entry: &Entry,
    size: Option<i64>,
    modified_at: Option<i64>,
) {
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
}

pub fn find_sorted(
    db: &Connection,
    owner: &str,
    folder_id: Option<&str>,
    query: &str,
    sort: ResultSort,
) -> Result<SearchResults, NamespaceError> {
    let folder_id = match folder_id {
        Some(id) if id == namespace::root_entry(db, owner)?.id => None,
        scope => scope,
    };
    let mut ancestors = Vec::new();
    if let Some(folder_id) = folder_id {
        ancestors = namespace::folder_path(db, owner, folder_id)?;
        ancestors.pop();
    }
    let remaining_depth = MAX_ENTRY_DEPTH - ancestors.len() as i64;
    let mut stmt = db.prepare(LIVE_SQL).map_err(|_| NamespaceError::Sql)?;
    let rows: Vec<(Entry, Option<i64>, Option<i64>)> = stmt
        .query_map(params![folder_id, owner, remaining_depth], |r| {
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
    for ancestor in &ancestors {
        fingerprint_entry(&mut digest, ancestor, None, None);
    }
    let needle = fold(query);
    let mut matches = Vec::new();
    for (entry, size, modified_at) in &rows {
        fingerprint_entry(&mut digest, entry, *size, *modified_at);
        if entry.parent_id.is_none()
            || folder_id == Some(entry.id.as_str())
            || !fold(&entry.name).contains(&needle)
        {
            continue;
        }
        let mut location = Vec::new();
        let mut parent = entry.parent_id.as_deref();
        while let Some(id) = parent {
            let folder = by_id.get(id).ok_or(NamespaceError::Sql)?;
            if folder.kind != "folder" || location.len() > remaining_depth as usize {
                return Err(NamespaceError::Sql);
            }
            location.push((*folder).clone());
            if folder_id == Some(id) {
                break;
            }
            parent = folder.parent_id.as_deref();
        }
        if location.last().is_none_or(|last| {
            folder_id.map_or(last.parent_id.is_some(), |anchor| last.id != anchor)
        }) {
            return Err(NamespaceError::Sql);
        }
        location.extend(ancestors.iter().rev().cloned());
        location.reverse();
        let location_key = location
            .iter()
            .map(|part| {
                fold(if part.name.is_empty() {
                    "Files"
                } else {
                    &part.name
                })
            })
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
        folder_id: folder_id.map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{domain::names::EntryName, persistence::schema::MIGRATION_0001_S1};

    const OWNER: &str = "00000000000000000000000000000001";
    const ROOT: &str = "00000000000000000000000000000002";
    const OTHER: &str = "00000000000000000000000000000003";
    const OTHER_ROOT: &str = "00000000000000000000000000000004";

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        db.execute_batch(MIGRATION_0001_S1).unwrap();
        crate::persistence::schema::migrate_v1(&db, None).unwrap();
        crate::persistence::schema::migrate_v2(&db, None).unwrap();
        crate::persistence::schema::migrate_v3(&db, None).unwrap();
        for (owner, root) in [(OWNER, ROOT), (OTHER, OTHER_ROOT)] {
            db.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,?1,'user',1,0)", [owner]).unwrap();
            db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,NULL,'folder','',x'',1)", params![root,owner]).unwrap();
        }
        db
    }

    fn insert(db: &Connection, number: usize, owner: &str, parent: &str, name: &str) -> String {
        let id = format!("{number:032x}");
        let name = EntryName::parse(name).unwrap();
        db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder',?4,?5,1)", params![id,owner,parent,name.display,name.key]).unwrap();
        id
    }

    #[test]
    fn location_sort_uses_the_default_library_display_name() {
        let mut db = database();
        let tx = db.transaction().unwrap();
        let studio = namespace::create_library(&tx, OWNER, "Studio").unwrap();
        let alpha = insert(&tx, 0x10, OWNER, ROOT, "Alpha");
        let zeta = insert(&tx, 0x11, OWNER, ROOT, "Zeta");
        let projects = insert(&tx, 0x12, OWNER, &studio.id, "Projects");
        for (index, parent) in [&alpha, &zeta, &projects].into_iter().enumerate() {
            insert(&tx, 0x20 + index, OWNER, parent, "Match");
        }
        tx.commit().unwrap();

        for (sort, expected) in [
            (ResultSort::LocationAsc, [&alpha, &zeta, &projects]),
            (ResultSort::LocationDesc, [&projects, &zeta, &alpha]),
        ] {
            for scope in [None, Some(ROOT)] {
                let found = find_sorted(&db, OWNER, scope, "Match", sort).unwrap();
                let parents: Vec<_> = found
                    .hits
                    .iter()
                    .map(|hit| &hit.location.last().unwrap().id)
                    .collect();
                assert_eq!(parents, expected);
            }
        }
        let named = find_sorted(
            &db,
            OWNER,
            Some(&studio.id),
            "Match",
            ResultSort::LocationAsc,
        )
        .unwrap();
        assert_eq!(named.hits.len(), 1);
        assert_eq!(named.hits[0].location.last().unwrap().id, projects);
    }

    #[test]
    fn recursive_search_child_plan_constrains_owner_parent_and_state_at_both_scales() {
        for count in [10_000, 100_000] {
            let mut db = database();
            let tx = db.transaction().unwrap();
            let mut parent = ROOT.to_owned();
            let mut deep = Vec::new();
            for n in 0..16 {
                parent = insert(&tx, 0x100 + n, OWNER, &parent, &format!("deep-{n:02}"));
                deep.push(parent.clone());
            }
            for n in 16..count {
                let parent = if n < count * 4 / 5 {
                    ROOT
                } else {
                    &deep[n % 16]
                };
                insert(&tx, 0x1000 + n, OWNER, parent, &format!("item-{n:06}"));
            }
            tx.commit().unwrap();
            for analyzed in [false, true] {
                if analyzed {
                    db.execute_batch("ANALYZE").unwrap();
                }
                let plan = db
                    .prepare(&format!("EXPLAIN QUERY PLAN {LIVE_SQL}"))
                    .unwrap()
                    .query_map(params![ROOT, OWNER, MAX_ENTRY_DEPTH], |row| {
                        row.get::<_, String>(3)
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                let child = plan
                    .iter()
                    .find(|line| line.contains("SEARCH child "))
                    .unwrap();
                assert!(
                    child.contains("owner_account_id=? AND parent_id=? AND state=?"),
                    "{count} entries, analyzed={analyzed}: {plan:?}"
                );
            }
            let rows = db
                .prepare(LIVE_SQL)
                .unwrap()
                .query_map(params![ROOT, OWNER, MAX_ENTRY_DEPTH], |_| Ok(()))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .len();
            assert_eq!(rows, count + 1);
        }
    }

    type Row = (String, Option<String>, String, String, i64);
    fn rows(db: &Connection, sql: &str, owner: &str, root: &str) -> Vec<Row> {
        db.prepare(sql)
            .unwrap()
            .query_map(params![root, owner, MAX_ENTRY_DEPTH], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn recursive_search_preserves_ordered_tree_inputs_for_wide_deep_and_hidden_entries() {
        let mut db = database();
        let tx = db.transaction().unwrap();
        for n in 0..256 {
            insert(&tx, 0x1000 + n, OWNER, ROOT, &format!("wide-{n:03}"));
        }
        let mut parent = ROOT.to_owned();
        for n in 0..MAX_ENTRY_DEPTH as usize {
            parent = insert(&tx, 0x2000 + n, OWNER, &parent, &format!("deep-{n:03}"));
        }
        let beyond_depth = insert(&tx, 0x3000, OWNER, &parent, "outside-depth");
        let literal = insert(&tx, 0x3001, OWNER, ROOT, "Notes%_Å");
        insert(&tx, 0x3002, OTHER, OTHER_ROOT, "Notes%_Å");
        let deleted = insert(&tx, 0x3003, OWNER, ROOT, "deleted-parent");
        let hidden = insert(&tx, 0x3004, OWNER, &deleted, "Notes%_Å");
        tx.execute(
            "UPDATE entry SET state='deleted' WHERE entry_id=?1",
            [&deleted],
        )
        .unwrap();
        let mut reports = Vec::new();
        for n in 0..3 {
            let parent = format!("{:032x}", 0x1000 + n);
            let id = insert(
                &tx,
                0x4000 + n,
                OWNER,
                &parent,
                if n == 2 { "report.TXT" } else { "Report.txt" },
            );
            tx.execute("UPDATE entry SET kind='file' WHERE entry_id=?1", [&id])
                .unwrap();
            reports.push(id);
        }
        tx.commit().unwrap();

        let legacy = LIVE_SQL.replace(
            "FROM live CROSS JOIN entry child ON child.parent_id=live.entry_id",
            "FROM entry child JOIN live ON child.parent_id=live.entry_id",
        );
        assert_ne!(legacy, LIVE_SQL);
        for (owner, root) in [(OWNER, ROOT), (OTHER, OTHER_ROOT)] {
            // These ordered rows are every input to matching, locations and fingerprinting.
            assert_eq!(
                rows(&db, LIVE_SQL, owner, root),
                rows(&db, &legacy, owner, root)
            );
        }
        let ordered_reports = find(&db, OWNER, "REPORT").unwrap();
        assert_eq!(
            ordered_reports
                .hits
                .iter()
                .map(|hit| hit.entry.id.clone())
                .collect::<Vec<_>>(),
            reports
        );
        let all = find(&db, OWNER, "").unwrap();
        assert_eq!(all.hits.len(), 256 + MAX_ENTRY_DEPTH as usize + 4);
        for excluded in [&deleted, &hidden, &beyond_depth] {
            assert!(!all.hits.iter().any(|hit| &hit.entry.id == excluded));
        }
        let deep = all.hits.iter().find(|hit| hit.entry.id == parent).unwrap();
        assert_eq!(deep.location.len(), MAX_ENTRY_DEPTH as usize);
        assert_eq!(deep.location[0].id, ROOT);
        for query in ["notes%_å", "NOTES%_A\u{030a}", "%_"] {
            let found = find(&db, OWNER, query).unwrap();
            assert_eq!(found.hits.len(), 1);
            assert_eq!(found.hits[0].entry.id, literal);
            assert_eq!(found.hits[0].location.len(), 1);
            assert_eq!(found.fingerprint, all.fingerprint);
        }
        let mut previous = all.fingerprint;
        for update in [
            "UPDATE entry SET original_name='renamed',name_key=x'72656e616d6564' WHERE entry_id=?1",
            "UPDATE entry SET generation=generation+1 WHERE entry_id=?1",
            "UPDATE entry SET parent_id='00000000000000000000000000001005' WHERE entry_id=?1",
            "UPDATE entry SET state='deleted' WHERE entry_id=?1",
        ] {
            db.execute(update, [&literal]).unwrap();
            assert_eq!(
                rows(&db, LIVE_SQL, OWNER, ROOT),
                rows(&db, &legacy, OWNER, ROOT)
            );
            let current = find(&db, OWNER, "").unwrap().fingerprint;
            assert_ne!(previous, current);
            previous = current;
        }
    }
}
