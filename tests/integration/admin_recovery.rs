//! Offline recovery against disposable bound installations and the real CLI.
#[path = "../common/mod.rs"]
mod common;

use rusqlite::{Connection, params, types::Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use sylux::{
    domain::authz::PasswordLimiter,
    persistence::{ownership, root, sqlite::DEFAULT_BUSY_TIMEOUT},
};

const OLD: &str = "old administrator password";
const NEW: &str = "  new administrator password  ";
const ADMIN: &str = "11111111111111111111111111111111";
const OTHER: &str = "22222222222222222222222222222222";

struct Case {
    owner: common::FixtureRoot,
    root: PathBuf,
}
impl Case {
    fn new() -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-recovery-{}", common::unique_token()));
        let owner = common::FixtureRoot::create(&parent, "recovery", "cli").unwrap();
        let root = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        root::initialize_empty_root(&root, DEFAULT_BUSY_TIMEOUT).unwrap();
        let db = Connection::open(root.join("metadata/sylux.sqlite3")).unwrap();
        let verifier =
            argon2::PasswordHasher::hash_password(&argon2::Argon2::default(), OLD.as_bytes())
                .unwrap()
                .to_string();
        for (id, handle, role) in [(ADMIN, "owner", "administrator"), (OTHER, "other", "user")] {
            db.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch,password_verifier) VALUES(?1,?2,?3,1,3,?4)", params![id,handle,role,verifier]).unwrap();
            db.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?1,3,zeroblob(32),zeroblob(32),1,9999999999,9999999999,0)",[id]).unwrap();
        }
        db.execute_batch("UPDATE installation SET setup_consumed=1,setup_verifier=zeroblob(32); UPDATE account SET enabled=0 WHERE handle='owner'").unwrap();
        drop(db);
        fs::create_dir(root.join("payloads")).unwrap();
        fs::write(
            root.join("payloads/untouched"),
            b"synthetic private payload",
        )
        .unwrap();
        Self { owner, root }
    }
    fn db(&self) -> Connection {
        Connection::open(self.root.join("metadata/sylux.sqlite3")).unwrap()
    }
    fn run(&self, handle: &str, password: &[u8]) -> Output {
        run(&self.root, handle, password)
    }
    fn cleanup(self) {
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }
}
fn run(root: &Path, handle: &str, password: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sylux"))
        .args(["admin", "reset-password", "--root"])
        .arg(root)
        .args(["--handle", handle, "--password-stdin"])
        .env("SYLUX_LISTEN", "invalid-serve-setting")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(password);
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("recovery did not exit");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    for secret in [
        OLD.as_bytes(),
        NEW.as_bytes(),
        handle.as_bytes(),
        root.as_os_str().as_encoded_bytes(),
    ] {
        assert!(
            !output
                .stdout
                .windows(secret.len())
                .any(|part| part == secret)
        );
        assert!(
            !output
                .stderr
                .windows(secret.len())
                .any(|part| part == secret)
        );
    }
    assert!(output.stdout.is_empty());
    output
}
fn rows(db: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut statement = db.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| (0..columns).map(|index| row.get(index)).collect())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn inventory(base: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, current: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for item in fs::read_dir(current).unwrap() {
            let path = item.unwrap().path();
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            if kind.is_symlink() {
                result.insert(
                    path.strip_prefix(base).unwrap().to_owned(),
                    fs::read_link(&path)
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes()
                        .to_vec(),
                );
            } else if kind.is_dir() {
                visit(base, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(base).unwrap().to_owned(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(base, base, &mut result);
    result
}
fn refused_unchanged(case: &Case, handle: &str, password: &[u8], reason: &str) {
    let before = inventory(&case.root);
    let output = case.run(handle, password);
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        format!("sylux: {reason}")
    );
    assert_eq!(inventory(&case.root), before);
}
#[test]
fn recovery_changes_only_target_credentials_and_sessions() {
    let case = Case::new();
    let identity = fs::read(case.root.join("installation.json")).unwrap();
    let installation = rows(&case.db(), "SELECT * FROM installation");
    let other = rows(&case.db(), "SELECT * FROM account WHERE handle='other'");
    let other_session = rows(
        &case.db(),
        "SELECT * FROM session WHERE account_id='22222222222222222222222222222222'",
    );
    let output = case.run("owner", format!("{NEW}\r\n").as_bytes());
    assert!(output.status.success());
    let db = case.db();
    let (verifier, enabled, epoch): (String, i64, i64) = db
        .query_row(
            "SELECT password_verifier,enabled,credential_epoch FROM account WHERE handle='owner'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let limiter = PasswordLimiter::default();
    assert!(limiter.verify(NEW.as_bytes(), &verifier));
    assert!(!limiter.verify(OLD.as_bytes(), &verifier));
    assert_eq!((enabled, epoch), (1, 4));
    assert_eq!(rows(&db, "SELECT * FROM installation"), installation);
    assert_eq!(
        rows(&db, "SELECT * FROM account WHERE handle='other'"),
        other
    );
    assert_eq!(
        rows(
            &db,
            "SELECT * FROM session WHERE account_id='22222222222222222222222222222222'"
        ),
        other_session
    );
    assert_eq!(
        rows(
            &db,
            "SELECT revoked FROM session WHERE account_id='11111111111111111111111111111111'"
        ),
        vec![vec![Value::Integer(1)]]
    );
    let event: (Option<String>, String, String) = db
        .query_row(
            "SELECT actor_account_id,action,outcome FROM event",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(event, (None, "admin.password_reset".into(), "ok".into()));
    drop(db);
    assert_eq!(
        fs::read(case.root.join("installation.json")).unwrap(),
        identity
    );
    assert_eq!(
        fs::read(case.root.join("payloads/untouched")).unwrap(),
        b"synthetic private payload"
    );
    assert!(ownership::acquire_writer_lock(&case.root).is_ok());
    case.cleanup();
}
#[test]
fn invalid_secret_and_target_leave_state_unchanged() {
    let case = Case::new();
    for input in [
        b"".to_vec(),
        b"too short".to_vec(),
        b"valid recovery password\n\n".to_vec(),
        b"valid recovery\npassword".to_vec(),
        b"valid recovery password\xff".to_vec(),
        vec![b'x'; 1027],
        "x".repeat(129).into_bytes(),
    ] {
        refused_unchanged(&case, "owner", &input, "password_input_invalid");
    }
    for handle in ["missing", "other"] {
        refused_unchanged(&case, handle, NEW.as_bytes(), "recovery_target_invalid");
    }
    case.cleanup();
}
#[test]
fn lock_refuses_before_any_database_access() {
    let case = Case::new();
    let guard = ownership::acquire_writer_lock(&case.root).unwrap();
    fs::remove_file(case.root.join("metadata/sylux.sqlite3")).unwrap();
    refused_unchanged(
        &case,
        "owner",
        NEW.as_bytes(),
        "service_running_stop_before_recovery",
    );
    drop(guard);
    case.cleanup();
}
#[test]
fn all_transaction_stages_roll_back_on_failure() {
    for table in ["account", "session", "event"] {
        let case = Case::new();
        let operation = if table == "event" { "INSERT" } else { "UPDATE" };
        case.db().execute_batch(&format!("CREATE TRIGGER reject_recovery BEFORE {operation} ON {table} BEGIN SELECT RAISE(ABORT,'synthetic secret raw error'); END")).unwrap();
        refused_unchanged(
            &case,
            "owner",
            NEW.as_bytes(),
            "recovery_transaction_failed",
        );
        case.cleanup();
    }
}
#[test]
fn invalid_setup_schema_and_integrity_refuse_without_repairs() {
    for (sql, reason) in [
        (
            "UPDATE installation SET setup_consumed=0",
            "recovery_setup_incomplete",
        ),
        ("PRAGMA user_version=0", "recovery_schema_unsupported"),
        ("PRAGMA user_version=999", "recovery_schema_unsupported"),
        ("DROP INDEX session_account", "recovery_schema_invalid"),
        (
            "UPDATE installation SET root_id='00000000000000000000000000000000'",
            "recovery_binding_invalid",
        ),
        (
            "UPDATE installation SET schema_version=999",
            "recovery_binding_invalid",
        ),
        (
            "PRAGMA foreign_keys=OFF; UPDATE session SET account_id='00000000000000000000000000000000'",
            "recovery_integrity_failed",
        ),
        (
            "PRAGMA ignore_check_constraints=ON; UPDATE account SET enabled=4",
            "recovery_integrity_failed",
        ),
        (
            "UPDATE account SET credential_epoch=9223372036854775807 WHERE handle='owner'",
            "recovery_target_invalid",
        ),
    ] {
        let case = Case::new();
        case.db().execute_batch(sql).unwrap();
        refused_unchanged(&case, "owner", NEW.as_bytes(), reason);
        case.cleanup();
    }
}
#[test]
fn missing_and_aliased_roots_refuse_without_initialization() {
    let case = Case::new();
    let missing = case.owner.path().join("missing");
    assert!(!run(&missing, "owner", NEW.as_bytes()).status.success());
    assert!(!missing.exists());
    let alias = case.owner.path().join("alias");
    symlink(&case.root, &alias).unwrap();
    assert!(!run(&alias, "owner", NEW.as_bytes()).status.success());
    case.cleanup();
    for name in [
        "installation.json",
        "metadata/sylux.sqlite3",
        "secrets/mutation.key",
        "writer.lock",
        "metadata",
        "secrets",
    ] {
        let case = Case::new();
        let path = case.root.join(name);
        let retained = case.owner.path().join("retained");
        fs::rename(&path, &retained).unwrap();
        symlink(&retained, &path).unwrap();
        let before = inventory(&case.root);
        assert!(!case.run("owner", NEW.as_bytes()).status.success());
        assert_eq!(inventory(&case.root), before);
        case.cleanup();
    }
}
#[test]
fn terminal_stdin_is_refused_without_mutation() {
    let case = Case::new();
    let before = inventory(&case.root);
    let output = Command::new("python3").arg("-c").arg("import os,pty,subprocess,sys; m,s=pty.openpty(); p=subprocess.run(sys.argv[1:],stdin=s,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=10); os.close(s); os.close(m); sys.stdout.buffer.write(p.stderr); sys.exit(p.returncode)")
        .arg(env!("CARGO_BIN_EXE_sylux")).args(["admin","reset-password","--root"]).arg(&case.root).args(["--handle","owner","--password-stdin"]).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(
        output.stdout,
        b"sylux: password_requires_nonterminal_stdin\n"
    );
    assert_eq!(inventory(&case.root), before);
    case.cleanup();
}

#[test]
fn commit_failure_is_reported_as_uncertain_without_retry() {
    let case = Case::new();
    case.db().execute_batch("CREATE TABLE recovery_commit_guard(account_id TEXT REFERENCES account(account_id) DEFERRABLE INITIALLY DEFERRED);
        CREATE TRIGGER reject_commit AFTER INSERT ON event BEGIN INSERT INTO recovery_commit_guard VALUES('missing'); END;").unwrap();
    refused_unchanged(
        &case,
        "owner",
        NEW.as_bytes(),
        "recovery_commit_uncertain_restart_and_test_credentials_before_retry",
    );
    case.cleanup();
}

#[test]
fn missing_and_hardlinked_protected_files_are_refused() {
    for name in [
        "installation.json",
        "metadata/sylux.sqlite3",
        "secrets/mutation.key",
        "writer.lock",
    ] {
        for hardlink in [false, true] {
            let case = Case::new();
            let path = case.root.join(name);
            let retained = case.owner.path().join("retained");
            fs::rename(&path, &retained).unwrap();
            if hardlink {
                fs::hard_link(&retained, &path).unwrap();
            }
            let before = inventory(&case.root);
            assert!(!case.run("owner", NEW.as_bytes()).status.success());
            assert_eq!(inventory(&case.root), before);
            case.cleanup();
        }
    }
}
