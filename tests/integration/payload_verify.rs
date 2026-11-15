//! Offline payload verification against disposable roots and the real CLI.
#[path = "../common/mod.rs"]
mod common;

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use sylux::persistence::{ownership, payload::PayloadStore, root, sqlite::DEFAULT_BUSY_TIMEOUT};

const ACCOUNT: &str = "11111111111111111111111111111111";
const HANDLE: &str = "secret-handle-name";
const FILE_NAME: &str = "private-report-name.txt";

struct Case {
    owner: common::FixtureRoot,
    root: PathBuf,
    next: u32,
}

impl Case {
    fn new() -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-verify-{}", common::unique_token()));
        let owner = common::FixtureRoot::create(&parent, "verify", "cli").unwrap();
        let root = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        root::initialize_empty_root(&root, DEFAULT_BUSY_TIMEOUT).unwrap();
        let db = Connection::open(root.join("metadata/sylux.sqlite3")).unwrap();
        // Names that must never appear in output.
        db.execute(
            "INSERT INTO account(account_id,handle,role,enabled,credential_epoch,password_verifier) VALUES(?1,?2,'administrator',1,1,'$argon2id$synthetic')",
            params![ACCOUNT, HANDLE],
        )
        .unwrap();
        db.execute_batch(&format!(
            "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES('{r}','{ACCOUNT}',NULL,'folder','',x'',1);
             INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES('{f}','{ACCOUNT}','{r}','folder','{FILE_NAME}',x'01',1);",
            r = "22222222222222222222222222222222",
            f = "33333333333333333333333333333333",
        ))
        .unwrap();
        Self {
            owner,
            root,
            next: 1,
        }
    }

    /// Publish through the real payload store and record the blob row.
    fn add(&mut self, bytes: &[u8]) -> String {
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        self.record(bytes.len() as u64, digest, |staging| {
            for chunk in bytes.chunks(8192) {
                staging.write_chunk(chunk).unwrap();
            }
        })
    }

    fn record(
        &mut self,
        length: u64,
        digest: [u8; 32],
        fill: impl FnOnce(&mut sylux::persistence::payload::StagingPayload),
    ) -> String {
        let blob = format!("a{:031x}", self.next);
        let attempt = format!("{:032x}", self.next);
        self.next += 1;
        let store = PayloadStore::open(&self.root).unwrap();
        let mut staging = store.start(&format!("{attempt}-{blob}"), length).unwrap();
        fill(&mut staging);
        staging.publish(digest).unwrap();
        let db = Connection::open(self.root.join("metadata/sylux.sqlite3")).unwrap();
        db.execute(
            "INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,substr(?1,1,2),?2,?3,'live')",
            params![blob, length as i64, digest.to_vec()],
        )
        .unwrap();
        blob
    }

    fn path(&self, blob: &str) -> PathBuf {
        self.root.join("payloads").join(&blob[..2]).join(blob)
    }

    fn run(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sylux"))
            .args(["admin", "verify-payloads", "--root"])
            .arg(&self.root)
            .env("SYLUX_LISTEN", "invalid-serve-setting")
            .output()
            .unwrap()
    }

    fn cleanup(self) {
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn assert_redacted(case: &Case, output: &Output) {
    for secret in [
        HANDLE.as_bytes(),
        FILE_NAME.as_bytes(),
        case.root.as_os_str().as_encoded_bytes(),
        b"/tmp",
        b"/var/",
        b"os error",
        b"No such file",
    ] {
        for stream in [&output.stdout, &output.stderr] {
            assert!(
                !stream.windows(secret.len()).any(|part| part == secret),
                "output leaked {:?}",
                String::from_utf8_lossy(secret)
            );
        }
    }
}

fn inventory(base: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, current: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for item in fs::read_dir(current).unwrap() {
            let path = item.unwrap().path();
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            if kind.is_dir() {
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

/// Data is unchanged. A read-only connection to a WAL database may leave an
/// empty `-wal` and a `-shm` shared-memory index; neither holds data.
fn assert_unchanged(case: &Case, before: &BTreeMap<PathBuf, Vec<u8>>) {
    let mut after = inventory(&case.root);
    let wal = Path::new("metadata/sylux.sqlite3-wal");
    let shm = Path::new("metadata/sylux.sqlite3-shm");
    if !before.contains_key(wal) && after.get(wal).is_some_and(Vec::is_empty) {
        after.remove(wal);
    }
    if !before.contains_key(shm) {
        after.remove(shm);
    }
    let changed: Vec<_> = after
        .keys()
        .chain(before.keys())
        .filter(|path| after.get(*path) != before.get(*path))
        .collect();
    assert!(changed.is_empty(), "verification wrote: {changed:?}");
}

#[test]
fn clean_root_reports_all_verified_and_changes_nothing() {
    let mut case = Case::new();
    case.add(b"");
    case.add(b"small payload");
    case.add(&b"multi buffer payload ".repeat(20_000));
    let before = inventory(&case.root);

    let output = case.run();

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "payload_verification checked=3 verified=3 damaged=0\n"
    );
    assert!(output.stderr.is_empty());
    assert_redacted(&case, &output);
    assert_unchanged(&case, &before);
    assert!(ownership::acquire_writer_lock(&case.root).is_ok());
    case.cleanup();
}

#[test]
fn flipped_truncated_and_missing_payloads_are_reported_distinctly() {
    let mut case = Case::new();
    let good = case.add(b"intact payload");
    let flipped = case.add(b"payload that will have one byte flipped");
    let truncated = case.add(b"payload that will be cut short");
    let missing = case.add(b"payload that will disappear");
    let mut bytes = fs::read(case.path(&flipped)).unwrap();
    bytes[3] ^= 0x01;
    fs::write(case.path(&flipped), bytes).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(case.path(&truncated))
        .unwrap()
        .set_len(5)
        .unwrap();
    fs::remove_file(case.path(&missing)).unwrap();
    let before = inventory(&case.root);

    let output = case.run();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stdout),
        format!(
            "damaged blob={flipped} reason=digest_mismatch\n\
             damaged blob={truncated} reason=length_mismatch\n\
             damaged blob={missing} reason=missing\n\
             payload_verification checked=4 verified=1 damaged=3\n"
        )
    );
    assert!(!text(&output.stdout).contains(&good));
    assert!(output.stderr.is_empty());
    assert_redacted(&case, &output);
    assert_unchanged(&case, &before);
    case.cleanup();
}

#[test]
fn non_private_payload_file_is_reported() {
    let mut case = Case::new();
    let blob = case.add(b"payload with loosened mode");
    fs::set_permissions(case.path(&blob), fs::Permissions::from_mode(0o644)).unwrap();

    let output = case.run();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stdout),
        format!(
            "damaged blob={blob} reason=not_private_file\n\
             payload_verification checked=1 verified=0 damaged=1\n"
        )
    );
    case.cleanup();
}

#[test]
fn symlinked_payload_is_reported_without_a_path() {
    let mut case = Case::new();
    let blob = case.add(b"payload replaced by a link");
    let target = case.root.join("elsewhere");
    fs::rename(case.path(&blob), &target).unwrap();
    std::os::unix::fs::symlink(&target, case.path(&blob)).unwrap();

    let output = case.run();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stdout),
        format!(
            "damaged blob={blob} reason=unreadable\n\
             payload_verification checked=1 verified=0 damaged=1\n"
        )
    );
    assert!(output.stderr.is_empty());
    assert_redacted(&case, &output);
    case.cleanup();
}

#[test]
fn unclean_wal_is_read_and_left_byte_identical() {
    let mut case = Case::new();
    case.add(b"first payload");
    // Leak the connection so SQLite never checkpoints or removes the WAL.
    let db = Connection::open(case.root.join("metadata/sylux.sqlite3")).unwrap();
    db.query_row("PRAGMA wal_autocheckpoint=0", [], |_| Ok(()))
        .unwrap();
    let bytes = b"payload committed only to the WAL";
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let store = PayloadStore::open(&case.root).unwrap();
    let blob = format!("a{:031x}", 99);
    let mut staging = store
        .start(&format!("{:032x}-{blob}", 99), bytes.len() as u64)
        .unwrap();
    staging.write_chunk(bytes).unwrap();
    staging.publish(digest).unwrap();
    db.execute(
        "INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,substr(?1,1,2),?2,?3,'live')",
        params![blob, bytes.len() as i64, digest.to_vec()],
    )
    .unwrap();
    let wal = case.root.join("metadata/sylux.sqlite3-wal");
    assert!(fs::metadata(&wal).unwrap().len() > 0, "fixture needs a WAL");
    let main_before = fs::read(case.root.join("metadata/sylux.sqlite3")).unwrap();
    let wal_before = fs::read(&wal).unwrap();

    let output = case.run();
    std::mem::forget(db);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "payload_verification checked=2 verified=2 damaged=0\n"
    );
    assert_eq!(
        fs::read(case.root.join("metadata/sylux.sqlite3")).unwrap(),
        main_before
    );
    assert_eq!(fs::read(&wal).unwrap(), wal_before);
    case.cleanup();
}

#[test]
fn missing_payload_directory_reports_every_blob_missing() {
    let mut case = Case::new();
    let blob = case.add(b"payload in a vanished directory");
    fs::remove_dir_all(case.root.join("payloads")).unwrap();

    let output = case.run();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stdout),
        format!(
            "damaged blob={blob} reason=missing\n\
             payload_verification checked=1 verified=0 damaged=1\n"
        )
    );
    case.cleanup();
}

#[test]
fn running_service_lock_refuses_before_reading_anything() {
    let case = Case::new();
    let guard = ownership::acquire_writer_lock(&case.root).unwrap();

    let output = case.run();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        text(&output.stderr),
        "sylux: service_running_stop_before_verify\n"
    );
    drop(guard);
    case.cleanup();
}

#[test]
fn rejects_unknown_arguments() {
    let output = Command::new(env!("CARGO_BIN_EXE_sylux"))
        .args(["admin", "verify-payloads", "--root", "/x", "--handle", "h"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(text(&output.stderr), "sylux: unknown_argument\n");
}

/// Run the CLI, sampling its resident set with `ps` (portable across macOS and Linux).
fn run_sampling_rss(case: &Case) -> (Output, u64) {
    let child = Command::new(env!("CARGO_BIN_EXE_sylux"))
        .args(["admin", "verify-payloads", "--root"])
        .arg(&case.root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id().to_string();
    let (sender, receiver) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || sender.send(child.wait_with_output().unwrap()));
    let mut peak_kib = 0u64;
    let output = loop {
        let sample = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid])
            .output()
            .unwrap();
        if let Ok(kib) = text(&sample.stdout).trim().parse::<u64>() {
            peak_kib = peak_kib.max(kib);
        }
        match receiver.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(output) => break output,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => panic!("waiter ended without output"),
        }
    };
    waiter.join().unwrap().unwrap();
    (output, peak_kib * 1024)
}

#[test]
fn large_payload_is_hashed_in_bounded_memory() {
    const SIZE: u64 = 256 << 20;
    let mut case = Case::new();
    // A sparse file reads as zeros; the digest is computed from a fixed buffer too.
    let mut hasher = Sha256::new();
    let zeros = vec![0u8; 1 << 20];
    for _ in 0..(SIZE >> 20) {
        hasher.update(&zeros);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    let blob = case.record(0, Sha256::digest([]).into(), |_| {});
    let path = case.path(&blob);
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(SIZE)
        .unwrap();
    Connection::open(case.root.join("metadata/sylux.sqlite3"))
        .unwrap()
        .execute(
            "UPDATE blob SET length=?2,sha256=?3 WHERE blob_id=?1",
            params![blob, SIZE as i64, digest.to_vec()],
        )
        .unwrap();

    let (output, peak) = run_sampling_rss(&case);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "payload_verification checked=1 verified=1 damaged=0\n"
    );
    assert!(peak > 0, "no RSS sample was taken");
    assert!(
        peak < 64 << 20,
        "verifier peak RSS {peak} bytes is not bounded"
    );
    case.cleanup();
}
