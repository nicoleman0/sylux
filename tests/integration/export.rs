//! The offline command runs against disposable, real metadata and payloads.
#[path = "../common/mod.rs"]
mod common;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use sylux::persistence::{ownership, payload::PayloadStore, root, sqlite::DEFAULT_BUSY_TIMEOUT};
const OWNER: &str = "11111111111111111111111111111111";
const ROOT: &str = "22222222222222222222222222222222";
struct Case {
    dir: PathBuf,
    source: PathBuf,
    target: PathBuf,
    next: u64,
}
impl Case {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("syl-74-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let source = dir.join("source");
        fs::DirBuilder::new().mode(0o700).create(&source).unwrap();
        root::initialize_empty_root(&source, DEFAULT_BUSY_TIMEOUT).unwrap();
        let c = Self {
            target: dir.join("target"),
            dir,
            source,
            next: 100,
        };
        c.db().execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'alice','administrator',1,0)",[OWNER]).unwrap();
        c.db().execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,NULL,'folder','',x'',1)",params![ROOT,OWNER]).unwrap();
        c
    }
    fn db(&self) -> Connection {
        Connection::open(self.source.join("metadata/sylux.sqlite3")).unwrap()
    }
    fn folder(&mut self, parent: &str, name: &str) -> String {
        let id = format!("{:032x}", self.next);
        self.next += 1;
        self.db().execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder',?4,?5,1)",params![id,OWNER,parent,name,id.as_bytes()]).unwrap();
        id
    }
    fn file(&mut self, parent: &str, name: &str, bytes: &[u8]) -> String {
        let id = self.folder(parent, name);
        self.db()
            .execute("UPDATE entry SET kind='file' WHERE entry_id=?1", [&id])
            .unwrap();
        self.revision(&id, bytes)
    }
    fn revision(&mut self, id: &str, bytes: &[u8]) -> String {
        let blob = format!("{:032x}", self.next);
        self.next += 1;
        let rev = format!("{:032x}", self.next);
        self.next += 1;
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let store = PayloadStore::open(&self.source).unwrap();
        let mut staging = store
            .start(&format!("{rev}-{blob}"), bytes.len() as u64)
            .unwrap();
        staging.write_chunk(bytes).unwrap();
        staging.publish(digest).unwrap();
        let db = self.db();
        db.execute(
            "INSERT INTO blob(blob_id,shard,length,sha256) VALUES(?1,substr(?1,1,2),?2,?3)",
            params![blob, bytes.len() as i64, digest.to_vec()],
        )
        .unwrap();
        db.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?2,?3,?4,?5,0,?6)",params![rev,id,blob,bytes.len() as i64,digest.to_vec(),OWNER]).unwrap();
        db.execute(
            "UPDATE entry SET current_revision_id=?1 WHERE entry_id=?2",
            params![rev, id],
        )
        .unwrap();
        blob
    }
    fn run_to(&self, target: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sylux"))
            .args(["admin", "export", "--root"])
            .arg(&self.source)
            .args(["--handle", "alice", "--destination"])
            .arg(target)
            .env("SYLUX_LISTEN", "invalid")
            .output()
            .unwrap()
    }
    fn run(&self) -> Output {
        self.run_to(&self.target)
    }
    fn payload(&self, id: &str) -> PathBuf {
        self.source.join("payloads").join(&id[..2]).join(id)
    }
}
impl Drop for Case {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, m: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                visit(root, &p, m)
            } else {
                m.insert(
                    p.strip_prefix(root).unwrap().to_owned(),
                    fs::read(p).unwrap(),
                );
            }
        }
    }
    let mut m = BTreeMap::new();
    visit(root, root, &mut m);
    m
}
fn failed(o: Output, reason: &str) {
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    assert!(String::from_utf8_lossy(&o.stderr).contains(reason), "{o:?}");
}
#[test]
fn nested_unicode_empty_and_current_revision_only_with_strict_source_isolation() {
    let mut c = Case::new();
    let f = c.folder(ROOT, "nested");
    c.folder(&f, "empty");
    let blob = c.file(&f, "résumé-猫.txt", b"old");
    let id: String = c
        .db()
        .query_row(
            "SELECT entry_id FROM revision WHERE blob_id=?1",
            [blob],
            |r| r.get(0),
        )
        .unwrap();
    c.revision(&id, b"current");
    c.file(ROOT, "zero", b"");
    let trash = c.folder(ROOT, "trash");
    c.file(&trash, "hidden", b"trash");
    c.db()
        .execute(
            "UPDATE entry SET state='deleted' WHERE entry_id=?1",
            [trash],
        )
        .unwrap();
    let db = c.db();
    db.execute_batch("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES('eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee','bob','user',1,0); INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES('ffffffffffffffffffffffffffffffff','eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',NULL,'folder','',x'',1);").unwrap();
    drop(db);
    let before = inventory(&c.source);
    let o = c.run();
    assert!(o.status.success(), "{o:?}");
    assert_eq!(
        fs::read(c.target.join("nested/résumé-猫.txt")).unwrap(),
        b"current"
    );
    assert!(c.target.join("nested/empty").is_dir());
    assert_eq!(fs::read_dir(&c.target).unwrap().count(), 2);
    assert_eq!(before, inventory(&c.source));
}
#[test]
fn nonempty_and_overlap_are_refused_without_overwrite() {
    let c = Case::new();
    fs::create_dir(&c.target).unwrap();
    fs::write(c.target.join("keep"), b"keep").unwrap();
    failed(c.run(), "export_destination_not_empty");
    assert_eq!(fs::read(c.target.join("keep")).unwrap(), b"keep");
    failed(c.run_to(&c.source.join("out")), "overlaps_source");
    failed(c.run_to(&c.dir), "overlaps_source");
}
#[test]
fn held_writer_lock_refuses() {
    let c = Case::new();
    let _guard = ownership::acquire_writer_lock(&c.source).unwrap();
    failed(c.run(), "service_running_stop_before_export");
    assert!(!c.target.exists());
}
#[test]
fn corruption_missing_truncated_and_digest_are_nonzero() {
    for mode in 0..3 {
        let mut c = Case::new();
        let blob = c.file(ROOT, "file", b"correct");
        let p = c.payload(&blob);
        match mode {
            0 => fs::remove_file(p).unwrap(),
            1 => fs::write(p, b"bad").unwrap(),
            _ => fs::write(p, b"corrupt").unwrap(),
        };
        failed(c.run(), "export_payload_invalid");
    }
}
#[test]
fn symlink_payload_and_destination_are_refused() {
    let mut c = Case::new();
    let blob = c.file(ROOT, "file", b"correct");
    let p = c.payload(&blob);
    let other = c.dir.join("other");
    fs::rename(&p, &other).unwrap();
    std::os::unix::fs::symlink(&other, &p).unwrap();
    failed(c.run(), "export_payload_invalid");
    fs::remove_dir_all(&c.target).unwrap();
    std::os::unix::fs::symlink(&c.source, &c.target).unwrap();
    failed(c.run(), "destination_symlink");
}
#[test]
fn unsafe_component_cannot_escape_target() {
    for name in ["../escape", "/absolute", ".", "..", "bad\0name"] {
        let mut c = Case::new();
        c.folder(ROOT, name);
        failed(c.run(), "export_name_invalid");
        assert!(!c.dir.join("escape").exists());
    }
}
#[test]
fn actual_filesystem_case_alias_is_refused_or_both_names_survive() {
    let mut c = Case::new();
    c.file(ROOT, "Case", b"first");
    c.file(ROOT, "case", b"second");
    let probe = c.dir.join("probe");
    fs::create_dir(&probe).unwrap();
    fs::write(probe.join("Case"), b"").unwrap();
    let aliases = probe.join("case").exists();
    let o = c.run();
    if aliases {
        failed(o, "collision");
        assert_eq!(fs::read(c.target.join("Case")).unwrap(), b"first");
    } else {
        assert!(o.status.success(), "{o:?}");
        assert_eq!(fs::read(c.target.join("Case")).unwrap(), b"first");
        assert_eq!(fs::read(c.target.join("case")).unwrap(), b"second");
    }
}
#[test]
fn unclean_wal_is_exported_without_source_sidecar_changes() {
    let mut c = Case::new();
    let blob = c.file(ROOT, "wal-file", b"wal");
    let db = c.db();
    db.execute_batch("PRAGMA wal_autocheckpoint=0; UPDATE entry SET original_name='new-name' WHERE original_name='wal-file';").unwrap();
    assert!(
        fs::metadata(c.source.join("metadata/sylux.sqlite3-wal"))
            .unwrap()
            .len()
            > 0
    );
    let before = inventory(&c.source);
    let o = c.run();
    assert!(o.status.success(), "{o:?}");
    assert_eq!(
        fs::read(c.target.join("new-name")).unwrap(),
        fs::read(c.payload(&blob)).unwrap()
    );
    assert_eq!(inventory(&c.source), before);
    drop(db);
}
#[test]
fn cli_rejects_missing_and_unrelated_arguments() {
    for args in [
        vec!["admin", "export", "--root", "/x"],
        vec!["admin", "export", "--root", "/x", "--handle", "alice"],
        vec!["admin", "export", "--root", "/x", "--listen", "x"],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_sylux"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2));
    }
}
#[test]
fn large_streamed_payload_exports_exact_bytes() {
    let mut c = Case::new();
    let blob = c.file(ROOT, "large", b"");
    let size = 128 * 1024 * 1024;
    fs::OpenOptions::new()
        .write(true)
        .open(c.payload(&blob))
        .unwrap()
        .set_len(size)
        .unwrap();
    let mut hash = Sha256::new();
    let mut f = fs::File::open(c.payload(&blob)).unwrap();
    let mut buf = [0; 65536];
    loop {
        let n = f.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    let digest: Vec<u8> = hash.finalize().to_vec();
    let db = c.db();
    let trigger: String = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='revision_rows_are_immutable'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    db.execute_batch("DROP TRIGGER revision_rows_are_immutable;")
        .unwrap();
    db.execute(
        "UPDATE blob SET length=?1,sha256=?2",
        params![size as i64, digest],
    )
    .unwrap();
    db.execute(
        "UPDATE revision SET length=?1,sha256=?2",
        params![size as i64, digest],
    )
    .unwrap();
    db.execute_batch(&trigger).unwrap();
    drop(db);
    let o = c.run();
    assert!(o.status.success(), "{o:?}");
    assert_eq!(fs::metadata(c.target.join("large")).unwrap().len(), size);
    let mut result = Sha256::new();
    let mut exported = fs::File::open(c.target.join("large")).unwrap();
    loop {
        let n = exported.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        result.update(&buf[..n]);
    }
    assert_eq!(result.finalize().to_vec(), digest);
}

#[test]
fn target_creation_rejects_collision_without_overwriting_first_file() {
    let mut c = Case::new();
    c.file(ROOT, "same-name", b"first");
    c.file(ROOT, "same-name", b"second");
    failed(c.run(), "export_name_unrepresentable_or_collision");
    assert_eq!(fs::read(c.target.join("same-name")).unwrap(), b"first");
}

#[test]
fn actual_target_accepts_portability_sensitive_names_without_renaming() {
    let mut c = Case::new();
    for name in [
        "CON",
        "colon:name",
        "trailing.",
        " spaces ",
        &"x".repeat(255),
    ] {
        c.file(ROOT, name, name.as_bytes());
    }
    let o = c.run();
    assert!(o.status.success(), "{o:?}");
    for name in [
        "CON",
        "colon:name",
        "trailing.",
        " spaces ",
        &"x".repeat(255),
    ] {
        assert_eq!(fs::read(c.target.join(name)).unwrap(), name.as_bytes());
    }
}

#[test]
fn normalization_alias_or_spelling_change_is_refused_on_actual_target() {
    let mut c = Case::new();
    c.file(ROOT, "é", b"first");
    c.file(ROOT, "e\u{301}", b"second");
    let probe = c.dir.join("probe");
    fs::create_dir(&probe).unwrap();
    fs::write(probe.join("é"), b"").unwrap();
    let aliases = probe.join("e\u{301}").exists();
    let spelling_preserved = fs::read_dir(&probe)
        .unwrap()
        .any(|e| e.unwrap().file_name() == "é");
    let o = c.run();
    if aliases || !spelling_preserved {
        assert!(!o.status.success(), "{o:?}");
        assert_eq!(
            fs::read(c.target.join("é")).unwrap(),
            if spelling_preserved {
                b"first".as_slice()
            } else {
                b"".as_slice()
            }
        );
    } else {
        assert!(o.status.success(), "{o:?}");
        assert_eq!(fs::read(c.target.join("é")).unwrap(), b"first");
        assert_eq!(fs::read(c.target.join("e\u{301}")).unwrap(), b"second");
    }
}

#[test]
fn existing_empty_target_is_accepted_and_source_alias_is_refused() {
    let mut c = Case::new();
    c.file(ROOT, "file", b"content");
    fs::create_dir(&c.target).unwrap();
    let o = c.run();
    assert!(o.status.success(), "{o:?}");
    let alias = c.dir.join("source-alias");
    std::os::unix::fs::symlink(&c.source, &alias).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_sylux"))
        .args(["admin", "export", "--root"])
        .arg(alias)
        .args(["--handle", "alice", "--destination"])
        .arg(c.dir.join("alias-export"))
        .output()
        .unwrap();
    failed(o, "export_source_symlink");
}
