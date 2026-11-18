//! Per-account storage use, as logical revision bytes.
//!
//! Files: revisions of file entries reachable from the account root through
//! active entries. Trash: revisions of every other file entry that is not
//! permanently removed, which includes files replaced by uploads. Each
//! retained revision falls in exactly one category. Payloads awaiting
//! removal cleanup, upload staging, SQLite and filesystem overhead are not
//! counted, so these figures are not physical disk use.

use rusqlite::{Connection, params};
use serde::Serialize;

/// `None` marks a category containing a file without a content revision:
/// invalid inventory, reported as unknown rather than as a smaller total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub files_bytes: Option<i64>,
    pub trash_bytes: Option<i64>,
}

impl Usage {
    const EMPTY: Self = Self {
        files_bytes: Some(0),
        trash_bytes: Some(0),
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountUsage {
    pub account_id: String,
    #[serde(flatten)]
    pub usage: Usage,
}

// One statement keeps both categories in the same snapshot. Only fixed
// owner filters vary between the two queries.
macro_rules! usage_sql {
    ($root_filter:literal, $file_filter:literal) => {
        concat!(
            "WITH RECURSIVE live(entry_id,owner_account_id,kind) AS (
                SELECT entry_id,owner_account_id,kind FROM entry
                WHERE parent_id IS NULL AND state='active'",
            $root_filter,
            "
                UNION ALL
                SELECT e.entry_id,e.owner_account_id,e.kind FROM live l
                JOIN entry e ON e.owner_account_id=l.owner_account_id
                    AND e.parent_id=l.entry_id AND e.state='active'
                WHERE l.kind='folder'
            ), live_files AS MATERIALIZED (SELECT entry_id FROM live WHERE kind='file')
            SELECT e.owner_account_id,
                COALESCE(SUM(CASE WHEN f.entry_id IS NOT NULL THEN r.length END),0),
                COALESCE(SUM(f.entry_id IS NOT NULL AND e.current_revision_id IS NULL),0),
                COALESCE(SUM(CASE WHEN f.entry_id IS NULL THEN r.length END),0),
                COALESCE(SUM(f.entry_id IS NULL AND e.current_revision_id IS NULL),0)
            FROM entry e
            LEFT JOIN live_files f ON f.entry_id=e.entry_id
            LEFT JOIN revision r ON r.entry_id=e.entry_id
            WHERE e.kind='file'",
            $file_filter,
            "
                AND NOT EXISTS(SELECT 1 FROM removal_member m WHERE m.entry_id=e.entry_id)
            GROUP BY e.owner_account_id"
        )
    };
}

const ACCOUNT_SQL: &str = usage_sql!(" AND owner_account_id=?1", " AND e.owner_account_id=?1");
const ALL_SQL: &str = usage_sql!("", "");

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, Usage)> {
    let known = |bytes: i64, missing: i64| (missing == 0).then_some(bytes);
    Ok((
        row.get(0)?,
        Usage {
            files_bytes: known(row.get(1)?, row.get(2)?),
            trash_bytes: known(row.get(3)?, row.get(4)?),
        },
    ))
}

/// Usage for one account. An account with no files reports zero.
pub fn account(db: &Connection, owner: &str) -> rusqlite::Result<Usage> {
    let mut statement = db.prepare(ACCOUNT_SQL)?;
    let mut rows = statement.query_map(params![owner], row)?;
    let usage = rows.next().transpose()?.map(|(_, usage)| usage);
    assert!(rows.next().is_none(), "usage grouped by one owner");
    Ok(usage.unwrap_or(Usage::EMPTY))
}

/// Usage for every account, ordered by handle.
pub fn all(db: &Connection) -> rusqlite::Result<Vec<AccountUsage>> {
    let mut usage = db
        .prepare(ALL_SQL)?
        .query_map([], row)?
        .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()?;
    let mut statement = db.prepare("SELECT account_id FROM account ORDER BY handle")?;
    let accounts = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|account_id| {
            let account_id = account_id?;
            let usage = usage.remove(&account_id).unwrap_or(Usage::EMPTY);
            Ok(AccountUsage { account_id, usage })
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(usage.is_empty(), "entries reference existing accounts");
    Ok(accounts)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "0000000000000000000000000000000a";
    const OTHER: &str = "0000000000000000000000000000000b";

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("../../migrations/sqlite/0001_s1.sql"))
            .unwrap();
        crate::persistence::schema::migrate_v1(&db, None).unwrap();
        for (account, handle) in [(OWNER, "owner"), (OTHER, "other")] {
            db.execute(
                "INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,?2,'user',1,0)",
                params![account, handle],
            )
            .unwrap();
            db.execute(
                "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?1,NULL,'folder','','',1)",
                [account],
            )
            .unwrap();
        }
        db
    }

    /// Account IDs pass through; short labels become padded hex IDs.
    fn id(label: &str) -> String {
        if label.len() == 32 {
            return label.to_owned();
        }
        let hex = label
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        format!("{hex:0>32}")
    }

    fn insert(db: &Connection, label: &str, parent: &str, kind: &str) -> (String, String) {
        let (entry, parent) = (id(label), id(parent));
        let owner: String = db
            .query_row(
                "SELECT owner_account_id FROM entry WHERE entry_id=?1",
                [&parent],
                |r| r.get(0),
            )
            .unwrap();
        db.execute(
            "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,?4,?5,?5,1)",
            params![entry, owner, parent, kind, label],
        )
        .unwrap();
        (entry, owner)
    }

    fn folder(db: &Connection, label: &str, parent: &str) {
        insert(db, label, parent, "folder");
    }

    fn trash(db: &Connection, label: &str) {
        db.execute(
            "UPDATE entry SET state='deleted' WHERE entry_id=?1",
            [id(label)],
        )
        .unwrap();
    }

    fn file(db: &Connection, label: &str, parent: &str, length: i64) {
        let (id, owner) = insert(db, label, parent, "file");
        db.execute(
            "INSERT INTO blob(blob_id,shard,length,sha256) VALUES(?1,substr(?1,1,2),?2,zeroblob(32))",
            params![id, length],
        )
        .unwrap();
        db.execute(
            "INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?1,?1,?2,zeroblob(32),1,?3)",
            params![id, length, owner],
        )
        .unwrap();
        db.execute(
            "UPDATE entry SET current_revision_id=?1 WHERE entry_id=?1",
            [id],
        )
        .unwrap();
    }

    fn known(files: i64, trash: i64) -> Usage {
        Usage {
            files_bytes: Some(files),
            trash_bytes: Some(trash),
        }
    }

    #[test]
    fn categories_follow_reachability_and_exclude_removed_entries() {
        let db = database();
        folder(&db, "f1", OWNER);
        file(&db, "a", "f1", 10);
        file(&db, "b", OWNER, 20);
        folder(&db, "f2", "f1");
        file(&db, "c", "f2", 40);
        file(&db, "x", OTHER, 1000);
        assert_eq!(account(&db, OWNER).unwrap(), known(70, 0));

        // Trashing a folder hides its active descendants behind the marker.
        trash(&db, "f2");
        assert_eq!(account(&db, OWNER).unwrap(), known(30, 40));
        trash(&db, "a");
        assert_eq!(account(&db, OWNER).unwrap(), known(20, 50));

        // Permanent removal of f2 releases its tree; the separately trashed file stays.

        db.execute(
            "INSERT INTO removal(operation_id,owner_account_id,confirmation_hash,removed_count,committed_at) VALUES(?1,?2,zeroblob(32),2,1)",
            params!["00000000000000000000000000000001", OWNER],
        )
        .unwrap();
        for entry in ["f2", "c"] {
            db.execute(
                "INSERT INTO removal_member(entry_id,operation_id) VALUES(?1,'00000000000000000000000000000001')",
                [id(entry)],
            )
            .unwrap();
        }
        assert_eq!(account(&db, OWNER).unwrap(), known(20, 10));
        assert_eq!(account(&db, OTHER).unwrap(), known(1000, 0));
        assert_eq!(
            account(&db, "0000000000000000000000000000000c").unwrap(),
            known(0, 0)
        );

        let rows = all(&db).unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.account_id.as_str(), row.usage))
                .collect::<Vec<_>>(),
            vec![(OTHER, known(1000, 0)), (OWNER, known(20, 10))]
        );
    }

    #[test]
    fn missing_content_revision_is_unknown_not_zero() {
        let db = database();
        file(&db, "a", OWNER, 5);
        // The schema permits this only transiently inside a commit.
        db.execute(
            "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?2,?1,?1,'file','broken','broken',1)",
            params![OWNER, id("broken")],
        )
        .unwrap();
        assert_eq!(
            account(&db, OWNER).unwrap(),
            Usage {
                files_bytes: None,
                trash_bytes: Some(0)
            }
        );
    }

    #[test]
    fn owner_query_uses_owner_indexes() {
        let db = database();
        let mut statement = db
            .prepare(&format!("EXPLAIN QUERY PLAN {ACCOUNT_SQL}"))
            .unwrap();
        let details = statement
            .query_map([OWNER], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("USING INDEX entry_active_listing")
                    && detail.contains("owner_account_id=? AND parent_id=? AND state=?")),
            "{details:?}"
        );
        assert!(
            !details.iter().any(|detail| detail.starts_with("SCAN e")),
            "{details:?}"
        );
    }
}
