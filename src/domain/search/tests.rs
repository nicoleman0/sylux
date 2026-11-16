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
