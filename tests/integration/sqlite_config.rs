//! T005/T007 connection-policy scenarios through the product SQLite workers.
//! Raw SQLite controls below are HARNESS ONLY, never product-policy RED/PASS.
#[path = "../common/mod.rs"]
mod common;

use common::FixtureRoot;
use rusqlite::{Connection, ErrorCode, OpenFlags};
use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
use sylux::persistence::{
    root, schema,
    sqlite::{self, ConnectionRole, SqliteWorkers, WorkerConfig},
};

const BUSY: Duration = Duration::from_millis(375);
#[derive(Debug, PartialEq, Eq)]
struct ReadBack {
    journal: String,
    synchronous: i64,
    foreign_keys: i64,
    busy_timeout_ms: i64,
}
impl ReadBack {
    fn read(db: &Connection) -> rusqlite::Result<Self> {
        Ok(Self {
            journal: db.query_row("PRAGMA journal_mode", [], |r| r.get(0))?,
            synchronous: db.query_row("PRAGMA synchronous", [], |r| r.get(0))?,
            foreign_keys: db.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?,
            busy_timeout_ms: db.query_row("PRAGMA busy_timeout", [], |r| r.get(0))?,
        })
    }
    fn required() -> Self {
        Self {
            journal: "wal".into(),
            synchronous: 2,
            foreign_keys: 1,
            busy_timeout_ms: 375,
        }
    }
}
fn fixture(case: &str) -> (FixtureRoot, PathBuf) {
    let parent = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("sylux-t005-{}", common::unique_token()));
    fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
    let owner = FixtureRoot::create(&parent, "sqlite", case).unwrap();
    let product = owner.path().join("product");
    fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
    root::initialize_empty_root(&product, BUSY).unwrap();
    let db = product.join("metadata").join("sylux.sqlite3");
    (owner, db)
}
fn cleanup(fixture: FixtureRoot) {
    fixture.cleanup_after_workers_exit().unwrap();
    fs::remove_dir(fixture.path().parent().unwrap()).unwrap();
}

#[tokio::test]
async fn product_writer_reader_and_reopened_connections_have_verified_pragmas() {
    let (owner, path) = fixture("all-product-connections");
    let config = WorkerConfig {
        busy_timeout: BUSY,
        ..WorkerConfig::default()
    };
    let workers = SqliteWorkers::open(&path, config).unwrap();
    let actual = vec![
        workers
            .policy_readback(ConnectionRole::Writer)
            .await
            .unwrap(),
        workers
            .policy_readback(ConnectionRole::Reader)
            .await
            .unwrap(),
    ];
    eprintln!("T007 actual per-connection PRAGMAs={actual:?}");
    for policy in actual {
        assert_eq!(policy.journal_mode, ReadBack::required().journal);
        assert_eq!(policy.synchronous, ReadBack::required().synchronous);
        assert_eq!(policy.foreign_keys, ReadBack::required().foreign_keys);
        assert_eq!(policy.busy_timeout_ms, ReadBack::required().busy_timeout_ms);
    }
    workers.shutdown().await.unwrap();
    let reopened = SqliteWorkers::open(&path, config).unwrap();
    assert_eq!(
        reopened
            .policy_readback(ConnectionRole::Writer)
            .await
            .unwrap()
            .journal_mode,
        "wal"
    );
    assert_eq!(
        reopened
            .policy_readback(ConnectionRole::Reader)
            .await
            .unwrap()
            .busy_timeout_ms,
        375
    );
    reopened.shutdown().await.unwrap();
    cleanup(owner);
}

#[test]
fn product_read_connections_cannot_mutate_domain_state() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (owner, path) = fixture("product-reader");
        let config = WorkerConfig {
            busy_timeout: BUSY,
            ..WorkerConfig::default()
        };
        let workers = SqliteWorkers::open(&path, config).unwrap();
        workers
            .transact(|tx| tx.execute_batch("CREATE TABLE contract_probe (id INTEGER PRIMARY KEY)"))
            .await
            .unwrap();
        let error = workers
            .read(|reader| reader.execute("INSERT INTO contract_probe VALUES (1)", []))
            .await
            .unwrap_err();
        match error {
            sqlite::WorkerError::Query(error) => {
                assert_eq!(error.sqlite_error_code(), Some(ErrorCode::ReadOnly))
            }
            other => panic!("expected SQLite read-only refusal, got {other:?}"),
        }
        let count: i64 = workers
            .read(|reader| {
                reader.query_row("SELECT count(*) FROM contract_probe", [], |r| r.get(0))
            })
            .await
            .unwrap();
        assert_eq!(count, 0);
        workers.shutdown().await.unwrap();
        cleanup(owner);
    });
}

#[test]
fn product_migration_rejects_unverified_policy_before_any_schema_change() {
    let (owner, _path) = fixture("migration-gate");
    let unverified = owner.path().join("unverified.sqlite3");
    let db = Connection::open(&unverified).unwrap();
    let actual = ReadBack::read(&db).unwrap();
    assert_ne!(
        actual,
        ReadBack::required(),
        "negative control must truly lack the policy"
    );
    let outcome = schema::apply_migrations(&db);
    let tables: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
    assert!(
        outcome.is_err(),
        "unverified migration unexpectedly succeeded"
    );
    drop(db);
    cleanup(owner);
}

#[test]
fn harness_raw_connections_and_ignored_pragma_are_not_policy_proof() {
    let (owner, path) = fixture("raw-negative-control");
    let a = Connection::open(&path).unwrap();
    let b = Connection::open(&path).unwrap();
    a.execute_batch("PRAGMA sylux_deliberately_unknown_setting=ON")
        .unwrap();
    let actual: Vec<_> = [&a, &b]
        .into_iter()
        .map(|db| ReadBack::read(db).unwrap())
        .collect();
    eprintln!("HARNESS ONLY raw/ignored PRAGMA read-back={actual:?}");
    assert!(actual.iter().all(|policy| policy != &ReadBack::required()));
    drop((a, b));
    cleanup(owner);
}

#[test]
fn harness_detects_connection_local_policy_loss_despite_persistent_wal() {
    let (owner, path) = fixture("connection-local-policy");
    let a = Connection::open(&path).unwrap();
    // Reference control only. These statements must NOT be reused as ProductAdapter.
    a.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")
        .unwrap();
    a.busy_timeout(BUSY).unwrap();
    assert_eq!(ReadBack::read(&a).unwrap(), ReadBack::required());
    let b = Connection::open(&path).unwrap();
    let actual = ReadBack::read(&b).unwrap();
    assert_eq!(actual.journal, "wal");
    assert_ne!(
        actual,
        ReadBack::required(),
        "WAL alone must not satisfy per-connection policy"
    );
    drop((a, b));
    cleanup(owner);
}

#[test]
fn harness_read_only_negative_write_reports_the_actual_sqlite_error() {
    let (owner, path) = fixture("read-only-control");
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch("CREATE TABLE probe (id INTEGER PRIMARY KEY)")
        .unwrap();
    let reader = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let error = reader
        .execute("INSERT INTO probe VALUES (1)", [])
        .unwrap_err();
    assert_eq!(error.sqlite_error_code(), Some(ErrorCode::ReadOnly));
    drop((reader, writer));
    cleanup(owner);
}
