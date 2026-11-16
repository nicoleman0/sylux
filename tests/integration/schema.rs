//! Root binding schema checks through product functions. Plain `rusqlite`
//! connections are harness negative controls, not product-policy evidence.
#[path = "../common/mod.rs"]
mod common;

use common::FixtureRoot;
use rusqlite::{Connection, ErrorCode, params};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    time::Duration,
};
use sylux::persistence::{schema, sqlite};

const BUSY: Duration = Duration::from_millis(375);

fn fixture(case: &str) -> (FixtureRoot, PathBuf) {
    let parent = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("sylux-p2-{}", common::unique_token()));
    fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
    let owner = FixtureRoot::create(&parent, "persistence", case).unwrap();
    let metadata = owner.path().join("metadata");
    fs::DirBuilder::new().mode(0o700).create(&metadata).unwrap();
    let db = metadata.join("sylux.sqlite3");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&db)
        .unwrap();
    (owner, db)
}

fn cleanup(owner: FixtureRoot) {
    owner.cleanup_after_workers_exit().unwrap();
    fs::remove_dir(owner.path().parent().unwrap()).unwrap();
}

/// A product writer connection on a freshly migrated core schema.
fn product_schema(case: &str) -> (FixtureRoot, Connection) {
    let (owner, db) = fixture(case);
    let connection = sqlite::open_writer(&db, BUSY).expect("product writer connection");
    assert_eq!(
        schema::apply_migrations(&connection).unwrap(),
        schema::MigrationOutcome::Applied
    );
    (owner, connection)
}

fn id(n: u64) -> String {
    format!("{n:032x}")
}

fn digest() -> [u8; 32] {
    [7; 32]
}

fn insert_account(connection: &Connection, account_id: &str, handle: &str, role: &str) {
    connection
        .execute(
            "INSERT INTO account (account_id, handle, role, enabled, credential_epoch) VALUES (?1, ?2, ?3, 1, 0)",
            params![account_id, handle, role],
        )
        .unwrap();
}

fn insert_root(connection: &Connection, entry_id: &str, owner: &str) {
    connection
        .execute(
            "INSERT INTO entry (entry_id, owner_account_id, parent_id, kind, original_name, name_key, name_key_version)
             VALUES (?1, ?2, NULL, 'folder', '', ?, 1)",
            params![entry_id, owner, &b""[..]],
        )
        .unwrap();
}

fn insert_child(
    connection: &Connection,
    entry_id: &str,
    owner: &str,
    parent: &str,
    kind: &str,
    name: &str,
    key: &[u8],
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO entry (entry_id, owner_account_id, parent_id, kind, original_name, name_key, name_key_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
        params![entry_id, owner, parent, kind, name, key],
    )
}

#[test]
fn product_writer_reader_and_reopened_connections_have_verified_pragmas() {
    let (owner, db) = fixture("pragma-readback");
    let writer = sqlite::open_writer(&db, BUSY).expect("product writer connection");
    assert_eq!(
        schema::apply_migrations(&writer).unwrap(),
        schema::MigrationOutcome::Applied
    );
    let reader_a = sqlite::open_reader(&db, BUSY).expect("product reader connection");
    let reader_b = sqlite::open_reader(&db, BUSY).expect("product reader connection");
    let required = sqlite::PolicyReadback {
        journal_mode: sqlite::REQUIRED_JOURNAL_MODE.to_owned(),
        synchronous: sqlite::REQUIRED_SYNCHRONOUS,
        foreign_keys: sqlite::REQUIRED_FOREIGN_KEYS,
        busy_timeout_ms: 375,
    };
    let actual: Vec<_> = [&writer, &reader_a, &reader_b]
        .into_iter()
        .map(|connection| sqlite::read_back_policy(connection).unwrap())
        .collect();
    eprintln!("T007 actual per-connection PRAGMAs={actual:?}");
    assert_eq!(
        actual,
        vec![required.clone(), required.clone(), required.clone()]
    );
    drop((writer, reader_a, reader_b));
    let reopened = sqlite::open_writer(&db, BUSY).expect("reopened product writer");
    assert_eq!(sqlite::read_back_policy(&reopened).unwrap(), required);
    drop(reopened);

    // HARNESS negative control only: a plain connection is not policy proof,
    // even though WAL and FULL happen to persist in the database file.
    let raw = Connection::open(&db).unwrap();
    let raw_policy = sqlite::read_back_policy(&raw).unwrap();
    eprintln!("HARNESS ONLY raw connection read-back={raw_policy:?}");
    assert_ne!(
        raw_policy,
        sqlite::PolicyReadback {
            journal_mode: sqlite::REQUIRED_JOURNAL_MODE.to_owned(),
            synchronous: sqlite::REQUIRED_SYNCHRONOUS,
            foreign_keys: sqlite::REQUIRED_FOREIGN_KEYS,
            busy_timeout_ms: 375,
        },
        "raw connection must not satisfy the per-connection policy"
    );
    drop(raw);
    cleanup(owner);
}

#[test]
fn product_reader_connections_cannot_mutate_domain_state() {
    let (owner, db) = fixture("reader-readonly");
    let writer = sqlite::open_writer(&db, BUSY).unwrap();
    schema::apply_migrations(&writer).unwrap();
    let reader = sqlite::open_reader(&db, BUSY).expect("product reader connection");
    let error = reader
        .execute(
            "INSERT INTO event (actor_account_id, action, occurred_at, outcome) VALUES (NULL, 'probe', 1, 'ok')",
            [],
        )
        .unwrap_err();
    assert_eq!(error.sqlite_error_code(), Some(ErrorCode::ReadOnly));
    let count: i64 = writer
        .query_row("SELECT count(*) FROM event", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop((reader, writer));
    cleanup(owner);
}

#[test]
fn migration_applies_once_is_idempotent_and_refuses_inconsistent_state() {
    let (owner, db) = fixture("migration-idempotent");
    let connection = sqlite::open_writer(&db, BUSY).unwrap();
    assert_eq!(
        schema::apply_migrations(&connection).unwrap(),
        schema::MigrationOutcome::Applied
    );
    assert_eq!(
        schema::apply_migrations(&connection).unwrap(),
        schema::MigrationOutcome::AlreadyCurrent
    );
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, schema::SUPPORTED_SCHEMA_VERSION);
    schema::verify_core_schema(&connection).unwrap();
    drop(connection);

    // Tables without a migration version refuse instead of being "repaired".
    let (owner2, db2) = fixture("migration-inconsistent");
    let connection = sqlite::open_writer(&db2, BUSY).unwrap();
    connection
        .execute_batch("CREATE TABLE foreign_table (x INTEGER)")
        .unwrap();
    match schema::apply_migrations(&connection) {
        Err(schema::SchemaError::Inconsistent(_)) => (),
        other => panic!("inconsistent database was not refused: {other:?}"),
    }
    let tables: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1, "refused migration must not change the schema");
    drop(connection);
    cleanup(owner2);
    cleanup(owner);
}

#[test]
fn constraint_parent_must_be_active_same_owner_folder() {
    let (owner, connection) = product_schema("parent-same-owner");
    insert_account(&connection, &id(10), "owner-a", "user");
    insert_account(&connection, &id(11), "owner-b", "user");
    insert_root(&connection, &id(20), &id(10));
    insert_root(&connection, &id(21), &id(11));
    insert_child(
        &connection,
        &id(22),
        &id(10),
        &id(20),
        "folder",
        "docs",
        b"docs",
    )
    .unwrap();

    match insert_child(
        &connection,
        &id(23),
        &id(11),
        &id(20),
        "folder",
        "evil",
        b"evil",
    ) {
        Err(error) => assert!(error.to_string().contains("same-owner"), "{error}"),
        Ok(_) => panic!("cross-owner parent accepted"),
    }
    insert_child(
        &connection,
        &id(24),
        &id(10),
        &id(22),
        "file",
        "f.bin",
        b"f.bin",
    )
    .unwrap();
    insert_child(
        &connection,
        &id(26),
        &id(10),
        &id(24),
        "folder",
        "inside",
        b"inside",
    )
    .unwrap_err();
    insert_child(&connection, &id(25), &id(10), &id(99), "folder", "x", b"x").unwrap_err();
    // Owner change to a foreign owner is re-validated by trigger.
    connection
        .execute(
            "UPDATE entry SET owner_account_id = ?1 WHERE entry_id = ?2",
            params![id(11), id(22)],
        )
        .unwrap_err();
    drop(connection);
    cleanup(owner);
}

#[test]
fn constraint_no_ancestry_cycles() {
    let (owner, connection) = product_schema("no-cycles");
    insert_account(&connection, &id(10), "owner-a", "user");
    insert_root(&connection, &id(20), &id(10));
    insert_child(&connection, &id(21), &id(10), &id(20), "folder", "a", b"a").unwrap();
    insert_child(&connection, &id(22), &id(10), &id(21), "folder", "b", b"b").unwrap();
    insert_child(&connection, &id(23), &id(10), &id(22), "file", "c", b"c").unwrap();

    connection
        .execute(
            "UPDATE entry SET parent_id = ?1 WHERE entry_id = ?2",
            params![id(20), id(23)],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE entry SET parent_id = ?1 WHERE entry_id = ?2",
            params![id(22), id(21)],
        )
        .unwrap_err();
    // Moving the root under its own folder descendant is refused by the
    // cycle trigger (the parent is a valid active same-owner folder).
    connection
        .execute(
            "UPDATE entry SET parent_id = ?1 WHERE entry_id = ?2",
            params![id(22), id(20)],
        )
        .unwrap_err();
    insert_child(
        &connection,
        &id(24),
        &id(10),
        &id(24),
        "folder",
        "self",
        b"self",
    )
    .unwrap_err();
    drop(connection);
    cleanup(owner);
}

#[test]
fn constraint_current_pointer_belongs_to_same_file() {
    let (owner, connection) = product_schema("current-pointer");
    insert_account(&connection, &id(10), "owner-a", "user");
    insert_root(&connection, &id(20), &id(10));
    insert_child(
        &connection,
        &id(21),
        &id(10),
        &id(20),
        "folder",
        "docs",
        b"docs",
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO blob (blob_id, shard, length, sha256) VALUES (?1, ?2, 0, ?3)",
            params![id(30), id(30)[..2].to_owned(), digest()],
        )
        .unwrap();
    connection.execute("BEGIN IMMEDIATE", []).unwrap();
    insert_child(
        &connection,
        &id(22),
        &id(10),
        &id(21),
        "file",
        "a.bin",
        b"a.bin",
    )
    .unwrap();
    insert_child(
        &connection,
        &id(23),
        &id(10),
        &id(21),
        "file",
        "b.bin",
        b"b.bin",
    )
    .unwrap();
    for (revision, entry) in [(id(32), id(22)), (id(33), id(23))] {
        connection
            .execute(
                "INSERT INTO revision (revision_id, entry_id, blob_id, length, sha256, committed_at, actor_account_id)
                 VALUES (?1, ?2, ?3, 0, ?4, 1, ?5)",
                params![revision, entry, id(30), digest(), id(10)],
            )
            .unwrap();
    }
    connection
        .execute(
            "UPDATE entry SET current_revision_id = ?1 WHERE entry_id = ?2",
            params![id(32), id(22)],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE entry SET current_revision_id = ?1 WHERE entry_id = ?2",
            params![id(33), id(22)],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO entry (entry_id, owner_account_id, parent_id, kind, original_name, name_key, name_key_version, current_revision_id)
             VALUES (?1, ?2, ?3, 'file', 'c.bin', ?4, 1, ?5)",
            params![id(24), id(10), id(21), b"c.bin", id(33)],
        )
        .unwrap_err();
    // The folder CHECK is isolated with a revision that genuinely belongs to
    // the folder entry: folders can never carry a current pointer.
    connection
        .execute(
            "INSERT INTO revision (revision_id, entry_id, blob_id, length, sha256, committed_at, actor_account_id)
             VALUES (?1, ?2, ?3, 0, ?4, 1, ?5)",
            params![id(34), id(21), id(30), digest(), id(10)],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE entry SET current_revision_id = ?1 WHERE entry_id = ?2",
            params![id(34), id(21)],
        )
        .unwrap_err();
    connection.execute("ROLLBACK", []).unwrap();
    drop(connection);
    cleanup(owner);
}

#[test]
fn constraint_unique_sibling_keys_across_kinds_and_one_root_per_account() {
    let (owner, connection) = product_schema("sibling-keys");
    insert_account(&connection, &id(10), "owner-a", "user");
    insert_account(&connection, &id(11), "owner-b", "user");
    insert_root(&connection, &id(20), &id(10));
    insert_root(&connection, &id(21), &id(11));
    insert_child(
        &connection,
        &id(22),
        &id(10),
        &id(20),
        "file",
        "A.bin",
        b"A.bin",
    )
    .unwrap();
    insert_child(
        &connection,
        &id(23),
        &id(10),
        &id(20),
        "folder",
        "a.bin",
        b"a.bin",
    )
    .unwrap();
    insert_child(
        &connection,
        &id(24),
        &id(10),
        &id(20),
        "folder",
        "A.bin",
        b"A.bin",
    )
    .unwrap_err();
    connection
        .execute(
            "UPDATE entry SET state='deleted' WHERE entry_id = ?1",
            params![id(22)],
        )
        .unwrap();
    insert_child(
        &connection,
        &id(25),
        &id(10),
        &id(20),
        "folder",
        "A.bin",
        b"A.bin",
    )
    .unwrap();
    insert_child(
        &connection,
        &id(26),
        &id(11),
        &id(21),
        "file",
        "a.bin",
        b"a.bin",
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO entry (entry_id, owner_account_id, parent_id, kind, original_name, name_key, name_key_version)
             VALUES (?1, ?2, NULL, 'folder', '', ?, 1)",
            params![id(27), id(10), &b""[..]],
        )
        .unwrap_err();
    insert_child(&connection, &id(28), &id(10), &id(20), "file", "", b"").unwrap_err();
    drop(connection);
    cleanup(owner);
}

#[test]
fn constraint_checked_columns_reject_invalid_values() {
    let (owner, connection) = product_schema("checked-columns");
    insert_account(&connection, &id(10), "owner-a", "user");
    insert_root(&connection, &id(20), &id(10));

    for (offset, handle) in ["b", "owner-x", "0123456789._-ab"].into_iter().enumerate() {
        connection
            .execute(
                "INSERT INTO account (account_id, handle, role, enabled, credential_epoch) VALUES (?1, ?2, 'user', 1, 0)",
                params![id(40 + offset as u64), handle],
            )
            .unwrap_or_else(|error| panic!("valid handle {handle} rejected: {error}"));
    }
    for handle in ["Owner-a", ".dot", "-dash", "UPPER", "has space", ""] {
        assert!(
            connection
                .execute(
                    "INSERT INTO account (account_id, handle, role, enabled, credential_epoch) VALUES (?1, ?2, 'user', 1, 0)",
                    params![id(45), handle],
                )
                .is_err(),
            "invalid handle {handle:?} accepted"
        );
    }
    let long_handle = "a".repeat(65);
    connection
        .execute(
            "INSERT INTO account (account_id, handle, role, enabled, credential_epoch) VALUES (?1, ?2, 'user', 1, 0)",
            params![id(45), long_handle],
        )
        .unwrap_err();

    connection
        .execute(
            "INSERT INTO account (account_id, handle, role, enabled, credential_epoch) VALUES (?1, 'x1', 'superuser', 1, 0)",
            params![id(42)],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO blob (blob_id, shard, length, sha256) VALUES (?1, ?2, 0, ?3)",
            params![id(31), id(31)[..2].to_owned(), [0u8; 31]],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO blob (blob_id, shard, length, sha256, lifecycle) VALUES (?1, ?2, -1, ?3, 'live')",
            params![id(31), id(31)[..2].to_owned(), digest()],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO blob (blob_id, shard, length, sha256, lifecycle) VALUES (?1, 'zz', 0, ?2, 'live')",
            params![id(31), digest()],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO blob (blob_id, shard, length, sha256, lifecycle) VALUES (?1, ?2, 0, ?3, 'vaporized')",
            params![id(31), id(31)[..2].to_owned(), digest()],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO entry (entry_id, owner_account_id, parent_id, kind, original_name, name_key, name_key_version)
             VALUES (?1, ?2, ?3, 'symlink', 'n', ?4, 1)",
            params![id(22), id(10), id(20), b"n"],
        )
        .unwrap_err();

    // ID format: 31 characters, uppercase and non-hex are all rejected.
    connection
        .execute(
            "INSERT INTO mutation (mutation_id, principal_account_id, scope, ticket_key_id, ticket_epoch, issued_at, expires_at, admission_fingerprint, state)
             VALUES (?1, ?2, 'upload', 'key-1', 0, 1, 2, ?3, 'admitted')",
            params![format!("{:031x}", 5), id(10), digest()],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO mutation (mutation_id, principal_account_id, scope, ticket_key_id, ticket_epoch, issued_at, expires_at, admission_fingerprint, state)
             VALUES (?1, ?2, 'upload', 'key-1', 0, 1, 2, ?3, 'admitted')",
            params![id(0xabcdef).to_uppercase(), id(10), digest()],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO mutation (mutation_id, principal_account_id, scope, ticket_key_id, ticket_epoch, issued_at, expires_at, admission_fingerprint, state)
             VALUES (?1, ?2, 'upload', 'key-1', 0, 5, 4, ?3, 'admitted')",
            params![id(5), id(10), digest()],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO event (actor_account_id, action, occurred_at, outcome) VALUES (NULL, 'probe', -1, 'ok')",
            [],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO event (actor_account_id, action, occurred_at, outcome) VALUES (NULL, 'probe', 1, 'maybe')",
            [],
        )
        .unwrap_err();
    drop(connection);
    cleanup(owner);
}

#[test]
fn constraint_upload_attempt_single_active_writer() {
    let (owner, connection) = product_schema("upload-attempt");
    insert_account(&connection, &id(10), "owner-a", "user");
    connection
        .execute(
            "INSERT INTO mutation (mutation_id, principal_account_id, scope, ticket_key_id, ticket_epoch, issued_at, expires_at, admission_fingerprint, state)
             VALUES (?1, ?2, 'upload', 'key-1', 0, 1, 2, ?3, 'admitted')",
            params![id(5), id(10), digest()],
        )
        .unwrap();
    let insert_attempt = |attempt: &str, attempt_no: i64, active: i64| {
        connection.execute(
            "INSERT INTO upload_attempt (attempt_id, mutation_id, attempt_no, staging_path, writer_active)
             VALUES (?1, ?2, ?3, 'blobs/ab/staging', ?4)",
            params![attempt, id(5), attempt_no, active],
        )
    };
    insert_attempt(&id(50), 1, 1).unwrap();
    insert_attempt(&id(51), 2, 1).unwrap_err();
    insert_attempt(&id(52), 2, 0).unwrap();
    connection
        .execute(
            "INSERT INTO upload_attempt (attempt_id, mutation_id, attempt_no, staging_path, writer_active)
             VALUES (?1, ?2, 2, 'blobs/ab/staging2', 0)",
            params![id(53), id(5)],
        )
        .unwrap_err();
    drop(connection);
    cleanup(owner);
}

#[test]
fn revision_rows_are_immutable() {
    let (owner, connection) = product_schema("revision-immutable");
    insert_account(&connection, &id(10), "owner-a", "user");
    insert_root(&connection, &id(20), &id(10));
    connection
        .execute(
            "INSERT INTO blob (blob_id, shard, length, sha256) VALUES (?1, ?2, 0, ?3)",
            params![id(30), id(30)[..2].to_owned(), digest()],
        )
        .unwrap();
    insert_child(
        &connection,
        &id(22),
        &id(10),
        &id(20),
        "file",
        "a.bin",
        b"a.bin",
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO revision (revision_id, entry_id, blob_id, length, sha256, committed_at, actor_account_id)
             VALUES (?1, ?2, ?3, 0, ?4, 1, ?5)",
            params![id(32), id(22), id(30), digest(), id(10)],
        )
        .unwrap();
    connection
        .execute("UPDATE revision SET length = 1", [])
        .unwrap_err();
    connection.execute("DELETE FROM revision", []).unwrap_err();
    drop(connection);
    cleanup(owner);
}

#[test]
fn foreign_keys_enforced_across_core_tables() {
    let (owner, connection) = product_schema("foreign-keys");
    insert_account(&connection, &id(10), "owner-a", "user");
    insert_root(&connection, &id(20), &id(10));
    connection
        .execute(
            "INSERT INTO session (session_id, account_id, credential_epoch, verifier, csrf_verifier, created_at, idle_expires_at, absolute_expires_at)
             VALUES (?1, ?2, 0, ?3, ?3, 1, 2, 3)",
            params![id(60), id(99), digest()],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO mutation (mutation_id, principal_account_id, scope, ticket_key_id, ticket_epoch, issued_at, expires_at, admission_fingerprint, state)
             VALUES (?1, ?2, 'upload', 'key-1', 0, 1, 2, ?3, 'admitted')",
            params![id(5), id(99), digest()],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO upload_attempt (attempt_id, mutation_id, attempt_no, staging_path, writer_active)
             VALUES (?1, ?2, 1, 'blobs/ab/staging', 1)",
            params![id(50), id(99)],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO event (actor_account_id, action, occurred_at, outcome) VALUES (?1, 'probe', 1, 'ok')",
            params![id(99)],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO revision (revision_id, entry_id, blob_id, length, sha256, committed_at, actor_account_id)
             VALUES (?1, ?2, ?3, 0, ?4, 1, ?5)",
            params![id(32), id(99), id(30), digest(), id(10)],
        )
        .unwrap_err();
    insert_child(
        &connection,
        &id(22),
        &id(10),
        &id(20),
        "file",
        "a.bin",
        b"a.bin",
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO blob (blob_id, shard, length, sha256) VALUES (?1, ?2, 0, ?3)",
            params![id(30), id(30)[..2].to_owned(), digest()],
        )
        .unwrap();
    // The entry exists and the blob exists: only the actor is foreign.
    connection
        .execute(
            "INSERT INTO revision (revision_id, entry_id, blob_id, length, sha256, committed_at, actor_account_id)
             VALUES (?1, ?2, ?3, 0, ?4, 1, ?5)",
            params![id(32), id(22), id(30), digest(), id(99)],
        )
        .unwrap_err();
    connection
        .execute(
            "INSERT INTO revision (revision_id, entry_id, blob_id, length, sha256, committed_at, actor_account_id)
             VALUES (?1, ?2, ?3, 0, ?4, 1, ?5)",
            params![id(32), id(22), id(31), digest(), id(10)],
        )
        .unwrap_err();
    drop(connection);
    cleanup(owner);
}

#[test]
fn valid_seed_passes_product_recovery_checks() {
    let (owner, connection) = product_schema("valid-seed");
    let identity = sylux::persistence::root::InstallationIdentity {
        installation_id: id(1),
        root_id: id(2),
        db_id: id(3),
        schema_version: 1,
        layout_version: 1,
        name_key_version: 1,
        key_version: 1,
        initialized: true,
    };
    schema::insert_installation_row(&connection, &identity, digest()).unwrap();
    insert_account(&connection, &id(10), "owner-a", "user");
    insert_account(&connection, &id(12), "admin", "administrator");
    insert_root(&connection, &id(20), &id(10));
    insert_child(
        &connection,
        &id(21),
        &id(10),
        &id(20),
        "folder",
        "docs",
        b"docs",
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO session (session_id, account_id, credential_epoch, verifier, csrf_verifier, created_at, idle_expires_at, absolute_expires_at)
             VALUES (?1, ?2, 0, ?3, ?3, 1, 2, 3)",
            params![id(60), id(10), digest()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO blob (blob_id, shard, length, sha256) VALUES (?1, ?2, 5, ?3)",
            params![id(30), id(30)[..2].to_owned(), digest()],
        )
        .unwrap();
    connection.execute("BEGIN IMMEDIATE", []).unwrap();
    insert_child(
        &connection,
        &id(22),
        &id(10),
        &id(21),
        "file",
        "a.bin",
        b"a.bin",
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO revision (revision_id, entry_id, blob_id, length, sha256, committed_at, actor_account_id)
             VALUES (?1, ?2, ?3, 5, ?4, 1, ?5)",
            params![id(32), id(22), id(30), digest(), id(10)],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE entry SET current_revision_id = ?1 WHERE entry_id = ?2",
            params![id(32), id(22)],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mutation (mutation_id, principal_account_id, scope, ticket_key_id, ticket_epoch, issued_at, expires_at, admission_fingerprint, completed_sha256, completed_length, state, result_revision_id)
             VALUES (?1, ?2, 'upload', 'key-1', 0, 1, 2, ?3, ?3, 5, 'committed', ?4)",
            params![id(5), id(10), digest(), id(32)],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO upload_attempt (attempt_id, mutation_id, attempt_no, staging_path, writer_active, bytes_received, completed_length, completed_sha256)
             VALUES (?1, ?2, 1, 'blobs/00/staging', 0, 5, 5, ?3)",
            params![id(50), id(5), digest()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO event (actor_account_id, action, occurred_at, outcome) VALUES (?1, 'upload.commit', 1, 'ok')",
            params![id(10)],
        )
        .unwrap();
    connection.execute("COMMIT", []).unwrap();

    schema::verify_core_schema(&connection).unwrap();
    schema::recovery_checks(&connection).unwrap();
    let foreign_key_rows: i64 = connection
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(foreign_key_rows, 0);
    drop(connection);
    cleanup(owner);
}

#[test]
fn legacy_binding_upgrade_recovers_database_commit_before_manifest_publication() {
    use sylux::persistence::root;
    for interrupted in [false, true] {
        let (owner, path) = fixture(if interrupted {
            "legacy-interrupted"
        } else {
            "legacy-upgrade"
        });
        fs::remove_file(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        let path = product.join("metadata/sylux.sqlite3");
        let initialized = root::initialize_empty_root(&product, BUSY).unwrap();
        let mut legacy = initialized.identity;
        legacy.schema_version = 1;
        fs::write(
            product.join(root::MANIFEST_NAME),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        fs::remove_file(&path).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        let db = sqlite::open_writer(&path, BUSY).unwrap();
        db.execute_batch(schema::MIGRATION_0001_S1).unwrap();
        schema::insert_installation_row(&db, &legacy, digest()).unwrap();
        insert_account(&db, &id(10), "owner-a", "user");
        insert_root(&db, &id(20), &id(10));
        if interrupted {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(serde_json::to_vec(&legacy).unwrap());
            schema::migrate_v1(&db, Some(hash.as_slice())).unwrap();
        }
        drop(db);
        let bound = root::open_bound_database(&product, BUSY).unwrap();
        let upgraded = root::upgrade_binding(&bound, &product, &legacy).unwrap();
        assert_eq!(upgraded.schema_version, 2);
        assert_eq!(root::read_manifest(&product).unwrap(), upgraded);
        root::verify_binding(&bound, &product, &upgraded).unwrap();
        assert_eq!(
            bound
                .connection
                .query_row("SELECT count(*) FROM account", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            root::upgrade_binding(&bound, &product, &upgraded).unwrap(),
            upgraded
        );
        drop(bound);
        cleanup(owner);
    }
}

#[test]
fn legacy_binding_transition_rejects_unrecorded_or_foreign_manifest() {
    use sylux::persistence::root;
    let (owner, path) = fixture("legacy-foreign");
    fs::remove_file(&path).unwrap();
    fs::remove_dir(path.parent().unwrap()).unwrap();
    let product = owner.path().join("product");
    fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
    let initialized = root::initialize_empty_root(&product, BUSY).unwrap();
    let mut legacy = initialized.identity;
    legacy.schema_version = 1;
    let bound = root::open_bound_database(&product, BUSY).unwrap();
    assert!(
        root::upgrade_binding(&bound, &product, &legacy).is_err(),
        "v2 database without the committed transition must not accept a v1 manifest"
    );
    drop(bound);
    cleanup(owner);
}

#[test]
fn unbound_v1_migration_fails_without_writing_a_partial_transition() {
    let (owner, path) = fixture("unbound-v1");
    let db = sqlite::open_writer(&path, BUSY).unwrap();
    db.execute_batch(schema::MIGRATION_0001_S1).unwrap();
    assert!(schema::apply_migrations(&db).is_err());
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='removal'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    drop(db);
    cleanup(owner);
}
