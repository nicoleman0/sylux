//! Process-level root binding scenarios. Only allowlisted events count as
//! refusal evidence; unknown errors do not. No request reaches the scaffold's
//! plaintext listener.
#[path = "../common/mod.rs"]
mod common;

use common::{FixtureRoot, server::TlsFiles};
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const WATCHDOG: Duration = Duration::from_secs(15);
const FORBIDDEN_ON_REFUSAL: &[&str] = &["migration.start", "recovery.start", "listener.bind"];

// Fixed test-owned names only; keep the generic component validator strict.
fn private_file(parent: &Path, name: &str) -> File {
    use rustix::fs::{Mode, OFlags, open, openat};
    assert!(matches!(
        name,
        "writer.lock" | "installation.json" | "installation.json.tmp" | "mutation.key"
    ));
    let directory = open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .unwrap();
    openat(
        &directory,
        name,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .unwrap()
    .into()
}

struct Case {
    owner: FixtureRoot,
    store: PathBuf,
    tls: TlsFiles,
}
impl Case {
    fn new(name: &str) -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-t005-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "binding", name).unwrap();
        let store = owner.path().join("store");
        fs::DirBuilder::new().mode(0o700).create(&store).unwrap();
        private_file(&store, "writer.lock");
        let tls = TlsFiles::generate(&owner, "tls").unwrap();
        Self { owner, store, tls }
    }
    // Use product initialization so each case mutates real bound state.
    fn bound(&self) {
        let observed = probe(self, "init");
        assert!(
            observed.exited_successfully && !observed.watchdog_expired && !observed.trace_overflow,
            "product init must complete cleanly: {observed:?}"
        );
        assert!(
            observed.events.iter().any(|e| e == "init.complete"),
            "product init must emit init.complete: {observed:?}"
        );
        assert!(
            !observed.events.iter().any(|e| e == "listener.bind"),
            "init must never bind a listener: {observed:?}"
        );
    }
    fn update_db(&self, sql: &str) {
        Connection::open(self.store.join("metadata/sylux.sqlite3"))
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
    fn cleanup(self) {
        self.owner.cleanup_after_workers_exit().unwrap();
        // Only remove the empty generated parent after descriptor-verified cleanup.
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }
}

#[derive(Debug, PartialEq, Eq)]
struct EntrySnapshot {
    device: u64,
    inode: u64,
    links: u64,
    mode: u32,
    content_digest_or_link: String,
}
fn inventory(root: &Path) -> BTreeMap<PathBuf, EntrySnapshot> {
    fn visit(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, EntrySnapshot>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let content = if meta.file_type().is_symlink() {
                fs::read_link(&path).unwrap().to_string_lossy().into_owned()
            } else if meta.is_file() {
                // Only small, generated fixtures; secret bodies never enter diagnostics.
                common::sha256_hex(&fs::read(&path).unwrap())
            } else {
                String::new()
            };
            out.insert(
                path.strip_prefix(base).unwrap().to_path_buf(),
                EntrySnapshot {
                    device: meta.dev(),
                    inode: meta.ino(),
                    links: meta.nlink(),
                    mode: meta.mode(),
                    content_digest_or_link: content,
                },
            );
            if meta.is_dir() {
                visit(base, &path, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[derive(Debug)]
struct Observation {
    events: Vec<String>,
    exited_successfully: bool,
    exited_unsuccessfully: bool,
    watchdog_expired: bool,
    trace_overflow: bool,
}
fn event(line: &str, expected_binding_reason: Option<&str>) -> &'static str {
    // No raw stderr, credentials, paths or unknown JSON values are copied into evidence.
    let parsed = serde_json::from_str::<serde_json::Value>(line).ok();
    let name = parsed
        .as_ref()
        .and_then(|v| v.get("contract_event"))
        .and_then(|v| v.as_str());
    const ALLOWED: &[&str] = &[
        "ownership.lock_acquire",
        "ownership.acquired",
        "ownership.refused",
        "binding.verified",
        "binding.refused",
        "database.open",
        "migration.start",
        "migration.complete",
        "removal.cleanup_pending",
        "recovery.start",
        "startup.failed",
        "listener.bind",
        "init.complete",
        "init.refused",
    ];
    if let Some(fields) = parsed.as_ref().and_then(|v| v.as_object())
        && let Some(name) = name
        && let Some(&known) = ALLOWED.iter().find(|&&known| known == name)
    {
        let reason = fields.get("reason").and_then(|v| v.as_str());
        let valid = match known {
            "ownership.refused"
            | "binding.refused"
            | "removal.cleanup_pending"
            | "startup.failed"
            | "init.refused" => {
                fields.len() == 2
                    && reason.is_some()
                    && (known != "binding.refused"
                        || expected_binding_reason.is_none_or(|expected| reason == Some(expected)))
            }
            _ => fields.len() == 1,
        };
        if valid {
            return known;
        }
    }
    if line.starts_with("Sylux static assets listening on ") {
        "listener.bind" // actual scaffold violation, NOT a fabricated production trace
    } else {
        "interface.unsupported"
    }
}
fn probe(case: &Case, command: &str) -> Observation {
    probe_with_binding_reason(case, command, None)
}

fn probe_with_binding_reason(
    case: &Case,
    command: &str,
    expected_binding_reason: Option<&'static str>,
) -> Observation {
    let lease = case.owner.register_worker().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_sylux"))
        .arg(command)
        .arg("--root")
        .arg(&case.store)
        .args(["--listen", "127.0.0.1:0", "--tls-cert"])
        .arg(&case.tls.leaf_cert)
        .arg("--tls-key")
        .arg(&case.tls.leaf_key)
        .env("SYLUX_LISTEN", "127.0.0.1:0")
        .env(
            "SYLUX_ASSETS",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("web/dist"),
        )
        .current_dir(case.owner.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(256);
    let overflow = Arc::new(AtomicBool::new(false));
    let reader_overflow = overflow.clone();
    let reader = thread::spawn(move || {
        let mut stderr = BufReader::new(stderr);
        loop {
            let mut line = String::new();
            // Bound a single line as well as the event queue. Continue draining after
            // the event cap so a noisy child cannot deadlock the join/kill path.
            match stderr.by_ref().take(4097).read_line(&mut line) {
                Ok(0) => break,
                Err(_) => {
                    reader_overflow.store(true, Ordering::SeqCst);
                    break;
                }
                Ok(_) => {
                    if line.len() > 4096
                        || tx
                            .try_send(event(&line, expected_binding_reason).to_owned())
                            .is_err()
                    {
                        reader_overflow.store(true, Ordering::SeqCst);
                    }
                }
            }
        }
    });
    let deadline = Instant::now() + WATCHDOG;
    let mut events = Vec::new();
    let mut watchdog_expired = false;
    let status = loop {
        for e in rx.try_iter() {
            if events.len() < 256 {
                events.push(e);
            } else {
                overflow.store(true, Ordering::SeqCst);
            }
        }
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if events.iter().any(|e| e == "listener.bind") {
            break None;
        }
        if Instant::now() >= deadline {
            watchdog_expired = true;
            break None;
        }
        // Polling is only a kill watchdog, never evidence that a refusal occurred.
        thread::sleep(Duration::from_millis(5));
    };
    if status.is_none() {
        let _ = child.kill();
    }
    child.wait().unwrap();
    reader.join().unwrap();
    for e in rx.try_iter() {
        if events.len() < 256 {
            events.push(e);
        } else {
            overflow.store(true, Ordering::SeqCst);
        }
    }
    lease.joined().unwrap();
    Observation {
        events,
        exited_successfully: status.is_some_and(|s| s.success()),
        exited_unsuccessfully: status.is_some_and(|s| !s.success()),
        watchdog_expired,
        trace_overflow: overflow.load(Ordering::SeqCst),
    }
}
fn assert_refused_unchanged(case: Case, command: &str, refusal: &str) {
    let before = inventory(case.owner.path());
    let observed = probe(&case, command);
    let after = inventory(case.owner.path());
    eprintln!(
        "A09/A10 actual={:?}; expected=lock_acquire -> {refusal} -> nonzero_exit; no migrate/recover/listen",
        observed
    );
    eprintln!(
        "fixture (retained on failure)={}",
        case.owner.path().display()
    );
    assert_eq!(
        after, before,
        "refusal must preserve every fixture byte and identity, including alias targets"
    );
    assert!(
        !observed.trace_overflow,
        "truncated trace cannot prove refusal ordering"
    );
    assert!(
        !observed.watchdog_expired,
        "timeout is not a binding refusal"
    );
    assert!(
        observed.exited_unsuccessfully && observed.events.iter().any(|e| e == refusal),
        "A09/A10: missing explicit fail-closed {refusal}; actual={observed:?}"
    );
    assert!(
        !observed
            .events
            .iter()
            .any(|e| FORBIDDEN_ON_REFUSAL.contains(&e.as_str())),
        "refusal must precede migration/recovery/listener: {observed:?}"
    );
    case.cleanup(); // failed roots are deliberately retained, never unsafe remove_dir_all
}

#[test]
fn a09_serve_empty_root_never_initializes() {
    assert_refused_unchanged(Case::new("serve-empty"), "serve", "binding.refused");
}

#[test]
fn startup_refuses_missing_referenced_payload_before_listener() {
    let case = Case::new("missing-payload");
    case.bound();
    case.update_db("INSERT INTO blob (blob_id, shard, length, sha256) VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'aa', 1, zeroblob(32))");
    let observed = probe(&case, "serve");
    assert!(
        observed.exited_unsuccessfully && !observed.watchdog_expired,
        "{observed:?}"
    );
    assert!(
        observed
            .events
            .iter()
            .any(|event| event == "recovery.start"),
        "{observed:?}"
    );
    assert!(
        observed
            .events
            .iter()
            .any(|event| event == "startup.failed"),
        "{observed:?}"
    );
    assert!(
        !observed.events.iter().any(|event| event == "listener.bind"),
        "{observed:?}"
    );
    case.cleanup();
}
#[test]
fn a09_explicit_empty_root_init_creates_bound_state_without_listener() {
    let case = Case::new("explicit-init");
    let observed = probe(&case, "init");
    assert!(
        observed.exited_successfully && !observed.watchdog_expired && !observed.trace_overflow,
        "init requires clean successful completion, not a premature event: {observed:?}"
    );
    assert!(
        observed.events.iter().any(|e| e == "init.complete"),
        "A09 explicit host init is missing: {observed:?}"
    );
    assert!(!observed.events.iter().any(|e| e == "listener.bind"));
    for file in [
        "installation.json",
        "metadata/sylux.sqlite3",
        "secrets/mutation.key",
        "secrets/setup.authorization",
    ] {
        assert!(
            case.store.join(file).is_file(),
            "explicit init must create {file}"
        );
    }
    require_valid_root(&case);
    case.cleanup();
}
#[test]
fn a09_init_never_resets_an_existing_installation() {
    let case = Case::new("init-existing");
    case.bound();
    require_valid_root(&case);
    assert_refused_unchanged(case, "init", "init.refused");
}
#[test]
fn a09_valid_binding_precedes_migration_recovery_and_listener() {
    let case = Case::new("valid-control");
    case.bound();
    require_valid_root(&case);
    case.cleanup();
}
fn require_valid_root(case: &Case) {
    let observed = probe(case, "serve");
    assert!(
        !observed.trace_overflow && !observed.watchdog_expired,
        "valid-root control requires a complete observed trace: {observed:?}"
    );
    for (earlier, later) in [
        ("ownership.acquired", "database.open"),
        ("binding.verified", "migration.start"),
        ("migration.start", "recovery.start"),
        ("recovery.start", "listener.bind"),
    ] {
        let a = observed.events.iter().position(|e| e == earlier);
        let b = observed.events.iter().position(|e| e == later);
        assert!(
            matches!((a,b), (Some(a),Some(b)) if a < b),
            "valid control must prove {earlier} before {later}, not generic rejection: {observed:?}"
        );
    }
}

macro_rules! refusal_case {
    ($name:ident, $prepare:expr) => {
        #[test]
        fn $name() {
            let case = Case::new(stringify!($name));
            case.bound();
            require_valid_root(&case);
            ($prepare)(&case);
            assert_refused_unchanged(case, "serve", "binding.refused");
        }
    };
}
refusal_case!(a09_missing_manifest, |c: &Case| fs::remove_file(
    c.store.join("installation.json")
)
.unwrap());
refusal_case!(a09_missing_database, |c: &Case| fs::remove_file(
    c.store.join("metadata/sylux.sqlite3")
)
.unwrap());
refusal_case!(a09_missing_key, |c: &Case| fs::remove_file(
    c.store.join("secrets/mutation.key")
)
.unwrap());
refusal_case!(a09_malformed_manifest, |c: &Case| fs::write(
    c.store.join("installation.json"),
    b"{"
)
.unwrap());
refusal_case!(a09_wrong_key_length, |c: &Case| fs::write(
    c.store.join("secrets/mutation.key"),
    [0; 31]
)
.unwrap());
refusal_case!(a09_mixed_root_identity, |c: &Case| c.update_db(
    "UPDATE installation SET root_id='00000000000000000000000000000004'"
));
refusal_case!(a09_mixed_database_identity, |c: &Case| c.update_db(
    "UPDATE installation SET db_id='00000000000000000000000000000004'"
));
refusal_case!(a09_mixed_installation_identity, |c: &Case| c.update_db(
    "UPDATE installation SET installation_id='00000000000000000000000000000004'"
));
refusal_case!(a09_unsupported_schema, |c: &Case| c
    .update_db("UPDATE installation SET schema_version=999"));
refusal_case!(a09_unsupported_key_version, |c: &Case| c
    .update_db("UPDATE installation SET key_version=999"));
refusal_case!(a09_unsupported_layout, |c: &Case| c
    .update_db("UPDATE installation SET layout_version=999"));
refusal_case!(a09_uninitialized_database, |c: &Case| c
    .update_db("UPDATE installation SET initialized=0"));

fn alias(c: &Case, relative: &str, hard: bool) {
    let path = c.store.join(relative);
    let retained = c.owner.path().join("retained-authoritative");
    fs::rename(&path, &retained).unwrap();
    if hard {
        fs::hard_link(&retained, &path).unwrap();
    } else {
        symlink(&retained, &path).unwrap();
    }
}
refusal_case!(a09_manifest_symlink, |c: &Case| alias(
    c,
    "installation.json",
    false
));
refusal_case!(a09_database_symlink, |c: &Case| alias(
    c,
    "metadata/sylux.sqlite3",
    false
));
refusal_case!(a09_key_symlink, |c: &Case| alias(
    c,
    "secrets/mutation.key",
    false
));
refusal_case!(a09_lock_symlink, |c: &Case| alias(c, "writer.lock", false));
refusal_case!(a09_manifest_hardlink, |c: &Case| alias(
    c,
    "installation.json",
    true
));
refusal_case!(a09_database_hardlink, |c: &Case| alias(
    c,
    "metadata/sylux.sqlite3",
    true
));
refusal_case!(a09_key_hardlink, |c: &Case| alias(
    c,
    "secrets/mutation.key",
    true
));
refusal_case!(a09_lock_hardlink, |c: &Case| alias(c, "writer.lock", true));
refusal_case!(a09_metadata_directory_symlink, |c: &Case| alias(
    c, "metadata", false
));
refusal_case!(a09_secrets_directory_symlink, |c: &Case| alias(
    c, "secrets", false
));

macro_rules! incomplete_case {
    ($name:ident, $stage:expr) => {
        #[test]
        fn $name() {
            let control = Case::new("incomplete-positive-control");
            control.bound();
            require_valid_root(&control);
            control.cleanup();
            let case = Case::new(stringify!($name));
            if $stage >= 1 {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(case.store.join("secrets"))
                    .unwrap();
                let mut key = [0; 32];
                getrandom::fill(&mut key).unwrap();
                private_file(&case.store.join("secrets"), "mutation.key")
                    .write_all(&key)
                    .unwrap();
            }
            if $stage >= 2 {
                private_file(&case.store, "installation.json.tmp")
                    .write_all(b"{}")
                    .unwrap();
            }
            if $stage >= 3 {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(case.store.join("metadata"))
                    .unwrap();
                drop(Connection::open(case.store.join("metadata/sylux.sqlite3")).unwrap());
            }
            assert_refused_unchanged(case, "serve", "binding.refused");
        }
    };
}
incomplete_case!(a09_interrupted_after_lock, 0);
incomplete_case!(a09_interrupted_after_key, 1);
incomplete_case!(a09_interrupted_before_manifest_publish, 2);
incomplete_case!(a09_interrupted_before_database_binding, 3);

#[test]
fn a10_second_process_refused_before_database_migration_recovery_listener() {
    use rustix::fs::{FlockOperation, flock};
    let case = Case::new("second-process");
    case.bound();
    require_valid_root(&case);
    let held = File::open(case.store.join("writer.lock")).unwrap();
    flock(&held, FlockOperation::NonBlockingLockExclusive).unwrap();
    let before = inventory(case.owner.path());
    let observed = probe(&case, "serve");
    assert_eq!(
        inventory(case.owner.path()),
        before,
        "second process touched bound state"
    );
    assert_eq!(
        observed.events,
        ["ownership.lock_acquire", "ownership.refused"],
        "A10 must refuse before DB open, including while the first descriptor is held: {observed:?}"
    );
    assert!(
        observed.exited_unsuccessfully && !observed.watchdog_expired && !observed.trace_overflow
    );
    drop(held);
    case.cleanup();
}

fn legacy_installation(case: &Case) -> (sylux::persistence::root::InstallationIdentity, PathBuf) {
    use std::os::unix::fs::OpenOptionsExt;
    use sylux::persistence::{root, schema, sqlite};

    case.bound();
    let mut legacy = root::read_manifest(&case.store).unwrap();
    legacy.schema_version = 1;
    fs::write(
        case.store.join(root::MANIFEST_NAME),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    let path = case.store.join("metadata/sylux.sqlite3");
    fs::remove_file(&path).unwrap();
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    let db = sqlite::open_writer(&path, Duration::from_millis(375)).unwrap();
    db.execute_batch(schema::MIGRATION_0001_S1).unwrap();
    schema::insert_installation_row(&db, &legacy, [7; 32]).unwrap();
    db.execute_batch("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES('11111111111111111111111111111111','legacy-owner','administrator',1,0);
        INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES('22222222222222222222222222222222','11111111111111111111111111111111',NULL,'folder','',x'',1);").unwrap();
    (legacy, path)
}

#[test]
fn serve_upgrades_v1_and_recovers_committed_transition_with_partial_manifest_temp() {
    use sha2::{Digest, Sha256};
    use sylux::persistence::{root, schema, sqlite};
    for interruption in ["none", "database", "manifest-temp"] {
        let case = Case::new(&format!("legacy-{interruption}"));
        let (legacy, path) = legacy_installation(&case);
        if interruption != "none" {
            let db = sqlite::open_writer(&path, Duration::from_millis(375)).unwrap();
            let hash = Sha256::digest(serde_json::to_vec(&legacy).unwrap());
            schema::migrate_v1(&db, Some(hash.as_slice())).unwrap();
        }
        if interruption == "manifest-temp" {
            private_file(&case.store, "installation.json.tmp")
                .write_all(b"{partial")
                .unwrap();
        }
        let admin = Command::new(env!("CARGO_BIN_EXE_sylux"))
            .args(["admin", "verify-payloads", "--root"])
            .arg(&case.store)
            .output()
            .unwrap();
        assert!(!admin.status.success());
        assert!(
            String::from_utf8_lossy(&admin.stderr)
                .contains("verify_schema_upgrade_required_run_serve")
        );
        let observed = probe(&case, "serve");
        assert_eq!(
            observed.events,
            [
                "ownership.lock_acquire",
                "ownership.acquired",
                "database.open",
                "migration.start",
                "binding.verified",
                "migration.complete",
                "recovery.start",
                "listener.bind",
            ],
            "{interruption}: {observed:?}"
        );
        assert!(!observed.watchdog_expired && !observed.trace_overflow);
        let upgraded = root::read_manifest(&case.store).unwrap();
        assert_eq!(upgraded.schema_version, 2);
        assert_eq!(upgraded.installation_id, legacy.installation_id);
        assert!(!case.store.join("installation.json.tmp").exists());
        let db = Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM account WHERE handle='legacy-owner'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(db);
        require_valid_root(&case);
        case.cleanup();
    }
}

#[test]
fn serve_reports_v1_upgrade_failures_without_verifying_binding() {
    use sylux::persistence::root;

    for (failure, reason, database_version) in [
        ("sql", "schema_migration_failed", 1),
        ("publication", "manifest_not_regular", 2),
    ] {
        let case = Case::new(failure);
        let (_, path) = legacy_installation(&case);
        if failure == "sql" {
            case.update_db("CREATE TABLE removal (synthetic_fixture INTEGER)");
        } else {
            fs::create_dir(case.store.join("installation.json.tmp")).unwrap();
        }

        let observed = probe_with_binding_reason(&case, "serve", Some(reason));
        assert_eq!(
            observed.events,
            [
                "ownership.lock_acquire",
                "ownership.acquired",
                "database.open",
                "migration.start",
                "binding.refused",
            ],
            "{failure}: {observed:?}"
        );
        assert!(
            observed.exited_unsuccessfully
                && !observed.watchdog_expired
                && !observed.trace_overflow,
            "{failure}: {observed:?}"
        );
        assert_eq!(root::read_manifest(&case.store).unwrap().schema_version, 1);
        let db = Connection::open(path).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            database_version
        );
        drop(db);
        case.cleanup();
    }
}
