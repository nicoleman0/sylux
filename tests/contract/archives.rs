//! Real-route ZIP download contracts over validated loopback TLS.
#[path = "../common/mod.rs"]
mod common;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{
    FixtureRoot,
    server::{TlsApiServer, TlsFiles, tls_connect},
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sylux::{
    domain::authz::verifier,
    http::{self, AuthState},
    persistence::{
        payload::PayloadStore,
        root,
        sqlite::{self, SqliteWorkers, WorkerConfig},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const OWNER: &str = "00000000000000000000000000000001";
const OTHER: &str = "00000000000000000000000000000002";
const ROOT: &str = "00000000000000000000000000000101";
const OTHER_ROOT: &str = "00000000000000000000000000000102";
const SESSION: &str = "00000000000000000000000000000201";
const OTHER_SESSION: &str = "00000000000000000000000000000202";
const SECRET: &[u8; 32] = b"s1-owner-secret-0123456789abcdef";
const OTHER_SECRET: &[u8; 32] = b"s1-other-secret-0123456789abcdef";
const CSRF: &[u8; 32] = b"s1-owner-csrf-0123456789abcdef!!";
const OTHER_CSRF: &[u8; 32] = b"s1-other-csrf-0123456789abcdef!!";
const ORIGIN: &str = "https://127.0.0.1";
const BUSY: Duration = Duration::from_millis(375);

struct Actor {
    session: &'static str,
    secret: &'static [u8; 32],
    csrf: &'static [u8; 32],
}
const OWNER_ACTOR: Actor = Actor {
    session: SESSION,
    secret: SECRET,
    csrf: CSRF,
};
const OTHER_ACTOR: Actor = Actor {
    session: OTHER_SESSION,
    secret: OTHER_SECRET,
    csrf: OTHER_CSRF,
};

struct Reply {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    /// False when a chunked body ended without its terminating chunk.
    complete: bool,
}

impl Reply {
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

struct Fixture {
    root: FixtureRoot,
    product: PathBuf,
    server: Option<TlsApiServer>,
    workers: Option<SqliteWorkers>,
    activity: Arc<()>,
    ca: Vec<u8>,
}

impl Fixture {
    async fn new(case: &str, seed_tree: impl FnOnce(&Connection, &Path)) -> Self {
        let parent = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("sylux-archives-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let root = FixtureRoot::create(&parent, "archives", case).unwrap();
        let product = root.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).unwrap();
        let tls = TlsFiles::generate(&root, "tls").unwrap();
        let ca = fs::read(&tls.ca_der).unwrap();
        let db_path = product.join("metadata/sylux.sqlite3");
        let connection = sqlite::open_writer(&db_path, BUSY).unwrap();
        seed_accounts(&connection);
        seed_tree(&connection, &product);
        let workers = SqliteWorkers::from_writer(
            connection,
            &db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let state = AuthState::new(workers.clone(), ORIGIN.into())
            .with_storage_root(&product)
            .unwrap();
        let activity = Arc::clone(&state.archive_activity);
        let server = TlsApiServer::start(&root, &tls, http::router(state), Duration::from_secs(2))
            .await
            .unwrap();
        Self {
            root,
            product,
            server: Some(server),
            workers: Some(workers),
            activity,
            ca,
        }
    }

    async fn close(mut self) {
        self.server
            .take()
            .unwrap()
            .shutdown_and_join()
            .await
            .unwrap();
        self.workers.take().unwrap().shutdown().await.unwrap();
        self.root.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.root.path().parent().unwrap()).unwrap();
    }

    async fn connect(&self) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
        tls_connect(
            self.server.as_ref().unwrap().address(),
            &self.ca,
            "127.0.0.1",
        )
        .await
        .unwrap()
    }

    fn head(&self, method: &str, path: &str, actor: Option<&Actor>) -> String {
        let port = self.server.as_ref().unwrap().address().port();
        let mut request =
            format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
        if let Some(actor) = actor {
            request.push_str(&format!(
                "Cookie: __Host-sylux={}.{}\r\n",
                actor.session,
                URL_SAFE_NO_PAD.encode(actor.secret)
            ));
        }
        request
    }

    async fn send(&self, request: String, body: &[u8]) -> Reply {
        let mut stream = self.connect().await;
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        // An aborted response closes without TLS close_notify; keep what arrived.
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => bytes.extend_from_slice(&buffer[..read]),
            }
        }
        parse_reply(&bytes)
    }

    async fn issue(&self, ids: &[&str], actor: &Actor) -> Reply {
        let body = serde_json::to_vec(&serde_json::json!({ "entry_ids": ids })).unwrap();
        let mut request = self.head("POST", "/archives", Some(actor));
        request.push_str(&format!(
            "Origin: {ORIGIN}\r\nX-CSRF-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            URL_SAFE_NO_PAD.encode(actor.csrf),
            body.len()
        ));
        self.send(request, &body).await
    }

    async fn get(&self, path: &str, actor: &Actor) -> Reply {
        let mut request = self.head("GET", path, Some(actor));
        request.push_str("\r\n");
        self.send(request, &[]).await
    }

    async fn download(&self, ids: &[&str]) -> Reply {
        let issued = self.issue(ids, &OWNER_ACTOR).await;
        assert_eq!(
            issued.status,
            201,
            "{}",
            String::from_utf8_lossy(&issued.body)
        );
        let url = issued.json()["url"].as_str().unwrap().to_owned();
        self.get(&url, &OWNER_ACTOR).await
    }

    fn blob_path(&self, blob_id: &str) -> PathBuf {
        self.product
            .join("payloads")
            .join(&blob_id[..2])
            .join(blob_id)
    }
}

fn parse_reply(bytes: &[u8]) -> Reply {
    let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&bytes[..split]);
    let mut lines = head.lines();
    let status = lines.next().unwrap().split_whitespace().nth(1).unwrap();
    let headers: BTreeMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let raw = &bytes[split + 4..];
    let (body, complete) =
        if headers.get("transfer-encoding").map(String::as_str) == Some("chunked") {
            dechunk(raw)
        } else {
            (raw.to_vec(), true)
        };
    Reply {
        status: status.parse().unwrap(),
        headers,
        body,
        complete,
    }
}

fn dechunk(mut raw: &[u8]) -> (Vec<u8>, bool) {
    let mut body = Vec::new();
    loop {
        let Some(line) = raw.windows(2).position(|w| w == b"\r\n") else {
            return (body, false);
        };
        let size = usize::from_str_radix(std::str::from_utf8(&raw[..line]).unwrap(), 16).unwrap();
        raw = &raw[line + 2..];
        if size == 0 {
            return (body, raw.starts_with(b"\r\n"));
        }
        if raw.len() < size + 2 {
            body.extend_from_slice(&raw[..raw.len().min(size)]);
            return (body, false);
        }
        body.extend_from_slice(&raw[..size]);
        raw = &raw[size + 2..];
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn seed_accounts(c: &Connection) {
    let now = now();
    for (account, root, handle, sid, secret, csrf) in [
        (OWNER, ROOT, "owner", SESSION, SECRET, CSRF),
        (
            OTHER,
            OTHER_ROOT,
            "other",
            OTHER_SESSION,
            OTHER_SECRET,
            OTHER_CSRF,
        ),
    ] {
        c.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,?2,'user',1,0)", params![account, handle]).unwrap();
        c.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)", params![root, account]).unwrap();
        c.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?4,?5,?6,?7,0)", params![sid, account, verifier(secret).as_slice(), verifier(csrf).as_slice(), now, now + 3600, now + 7200]).unwrap();
    }
}

fn folder(c: &Connection, owner: &str, parent: &str, id: &str, name: &str) {
    c.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,?3,'folder',?4,?5,1,1)", params![id, owner, parent, name, name.as_bytes()]).unwrap();
}

/// Publish `bytes` and insert an active file; the blob ID is derived from the entry ID.
fn file(
    c: &Connection,
    product: &Path,
    owner: &str,
    parent: &str,
    id: &str,
    name: &str,
    bytes: &[u8],
) {
    let blob_id = blob_of(id);
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let store = PayloadStore::open(product).unwrap();
    let mut staged = store
        .start(&format!("a{}-{blob_id}", &id[1..]), bytes.len() as u64)
        .unwrap();
    for chunk in bytes.chunks(1 << 20) {
        staged.write_chunk(chunk).unwrap();
    }
    staged.publish(digest).unwrap();
    let revision = format!("c{}", &id[1..]);
    let tx = c.unchecked_transaction().unwrap();
    tx.execute(
        "INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,?2,?3,?4,'live')",
        params![
            blob_id,
            &blob_id[..2],
            bytes.len() as i64,
            digest.as_slice()
        ],
    )
    .unwrap();
    tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,current_revision_id,generation) VALUES(?1,?2,?3,'file',?4,?5,1,NULL,1)", params![id, owner, parent, name, name.as_bytes()]).unwrap();
    tx.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![revision, id, blob_id, bytes.len() as i64, digest.as_slice(), 1_709_214_358_i64, owner]).unwrap();
    tx.execute(
        "UPDATE entry SET current_revision_id=?1 WHERE entry_id=?2",
        params![revision, id],
    )
    .unwrap();
    tx.commit().unwrap();
}

fn blob_of(id: &str) -> String {
    format!("b{}", &id[1..])
}

fn trash(c: &Connection, id: &str) {
    c.execute("UPDATE entry SET state='deleted' WHERE entry_id=?1", [id])
        .unwrap();
}

/// Deterministic bytes that differ across every 64 KiB buffer boundary.
fn pattern(length: usize, seed: u8) -> Vec<u8> {
    (0..length)
        .map(|index| (index as u32).wrapping_mul(2_654_435_761).to_le_bytes()[3] ^ seed)
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum Member {
    Directory,
    File(Vec<u8>),
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// Independent reader: central directory, ZIP64 extras, local headers, CRC and descriptors.
fn extract(archive: &[u8]) -> BTreeMap<String, Member> {
    let end = archive.len() - 22;
    assert_eq!(
        u32_at(archive, end),
        0x0605_4b50,
        "end of central directory"
    );
    let (mut count, mut size, mut offset) = (
        u64::from(u16_at(archive, end + 10)),
        u64::from(u32_at(archive, end + 12)),
        u64::from(u32_at(archive, end + 16)),
    );
    let mut directory_end = end as u64;
    if count == 0xffff || size == 0xffff_ffff || offset == 0xffff_ffff {
        let locator = end - 20;
        assert_eq!(u32_at(archive, locator), 0x0706_4b50);
        let zip64 = u64_at(archive, locator + 8) as usize;
        assert_eq!(u32_at(archive, zip64), 0x0606_4b50);
        count = u64_at(archive, zip64 + 32);
        size = u64_at(archive, zip64 + 40);
        offset = u64_at(archive, zip64 + 48);
        directory_end = zip64 as u64;
    }
    assert_eq!(
        offset + size,
        directory_end,
        "central directory is contiguous"
    );
    let mut members = BTreeMap::new();
    let mut at = offset as usize;
    for _ in 0..count {
        assert_eq!(u32_at(archive, at), 0x0201_4b50);
        let flags = u16_at(archive, at + 8);
        assert_eq!(u16_at(archive, at + 10), 0, "stored");
        assert_ne!(flags & (1 << 11), 0, "UTF-8 names");
        let crc = u32_at(archive, at + 16);
        let mut length = u64::from(u32_at(archive, at + 24));
        let name_length = usize::from(u16_at(archive, at + 28));
        let extra_length = usize::from(u16_at(archive, at + 30));
        let comment_length = usize::from(u16_at(archive, at + 32));
        let mut local = u64::from(u32_at(archive, at + 42));
        let name = String::from_utf8(archive[at + 46..at + 46 + name_length].to_vec()).unwrap();
        let mut extra = &archive[at + 46 + name_length..at + 46 + name_length + extra_length];
        while !extra.is_empty() {
            let (tag, field) = (u16_at(extra, 0), usize::from(u16_at(extra, 2)));
            if tag == 1 {
                let mut values = extra[4..4 + field].chunks(8).map(|v| u64_at(v, 0));
                if length == 0xffff_ffff {
                    length = values.next().unwrap();
                    values.next().unwrap();
                }
                if local == 0xffff_ffff {
                    local = values.next().unwrap();
                }
            }
            extra = &extra[4 + field..];
        }
        let local = local as usize;
        assert_eq!(u32_at(archive, local), 0x0403_4b50);
        let local_name = u16_at(archive, local + 26) as usize;
        let local_extra = u16_at(archive, local + 28) as usize;
        assert_eq!(
            &archive[local + 30..local + 30 + local_name],
            name.as_bytes()
        );
        let data = local + 30 + local_name + local_extra;
        let bytes = &archive[data..data + length as usize];
        assert_eq!(crc32fast::hash(bytes), crc, "{name} CRC");
        let member = if name.ends_with('/') {
            assert_eq!(length, 0);
            Member::Directory
        } else {
            assert_ne!(flags & (1 << 3), 0, "data descriptor flag");
            let descriptor = data + length as usize;
            assert_eq!(u32_at(archive, descriptor), 0x0807_4b50);
            assert_eq!(u32_at(archive, descriptor + 4), crc);
            Member::File(bytes.to_vec())
        };
        assert!(
            members.insert(name, member).is_none(),
            "duplicate archive path"
        );
        at += 46 + name_length + extra_length + comment_length;
    }
    members
}

const DOCS: &str = "00000000000000000000000000000301";
const DOCS_A: &str = "00000000000000000000000000000302";
const EMPTY: &str = "00000000000000000000000000000303";
const NESTED: &str = "00000000000000000000000000000304";
const NESTED_B: &str = "00000000000000000000000000000305";
const ONE: &str = "00000000000000000000000000000306";
const TWO: &str = "00000000000000000000000000000307";
const GONE: &str = "00000000000000000000000000000308";

fn tree(c: &Connection, product: &Path) {
    folder(c, OWNER, ROOT, DOCS, "Docs");
    file(c, product, OWNER, DOCS, DOCS_A, "a.txt", b"alpha\n");
    folder(c, OWNER, DOCS, EMPTY, "Empty");
    folder(c, OWNER, DOCS, NESTED, "Nested ü");
    file(
        c,
        product,
        OWNER,
        NESTED,
        NESTED_B,
        "b.bin",
        &pattern(200_000, 1),
    );
    file(
        c,
        product,
        OWNER,
        ROOT,
        ONE,
        "one.bin",
        &pattern(1 << 20, 2),
    );
    file(c, product, OWNER, ROOT, TWO, "two.txt", b"");
    file(c, product, OWNER, DOCS, GONE, "gone.txt", b"trashed");
    trash(c, GONE);
}

#[tokio::test]
async fn folder_and_files_extract_to_the_same_names_structure_and_bytes() {
    let f = Fixture::new("selection", tree).await;
    // DOCS_A is inside DOCS: the folder covers it and it appears once.
    let issued = f.issue(&[DOCS, ONE, TWO, DOCS_A], &OWNER_ACTOR).await;
    assert_eq!(issued.status, 201);
    assert_eq!(issued.json()["name"], "download.zip");
    assert_eq!(issued.json()["entry_count"], 7);
    let url = issued.json()["url"].as_str().unwrap().to_owned();
    let reply = f.get(&url, &OWNER_ACTOR).await;
    assert_eq!(reply.status, 200);
    assert!(reply.complete, "archive response must finish cleanly");
    assert_eq!(
        reply.headers.get("content-type").map(String::as_str),
        Some("application/zip")
    );
    assert_eq!(
        reply.headers.get("cache-control").map(String::as_str),
        Some("no-store")
    );
    assert!(reply.headers["content-disposition"].starts_with("attachment;"));
    let members = extract(&reply.body);
    let expected = BTreeMap::from([
        ("Docs/".to_owned(), Member::Directory),
        ("Docs/a.txt".to_owned(), Member::File(b"alpha\n".to_vec())),
        ("Docs/Empty/".to_owned(), Member::Directory),
        ("Docs/Nested ü/".to_owned(), Member::Directory),
        (
            "Docs/Nested ü/b.bin".to_owned(),
            Member::File(pattern(200_000, 1)),
        ),
        ("one.bin".to_owned(), Member::File(pattern(1 << 20, 2))),
        ("two.txt".to_owned(), Member::File(Vec::new())),
    ]);
    assert_eq!(members, expected);

    let again = f.get(&url, &OWNER_ACTOR).await;
    assert_eq!(again.status, 404, "tickets are single use");

    let single = f.download(&[NESTED]).await;
    assert!(single.complete);
    assert!(single.headers["content-disposition"].contains("filename*=UTF-8''Nested%20%C3%BC.zip"));
    assert_eq!(
        extract(&single.body).into_keys().collect::<Vec<_>>(),
        ["Nested ü/", "Nested ü/b.bin"]
    );
    f.close().await;
}

const FOREIGN: &str = "00000000000000000000000000000401";
const HIDDEN_PARENT: &str = "00000000000000000000000000000402";
const HIDDEN_CHILD: &str = "00000000000000000000000000000403";
const UNKNOWN: &str = "0000000000000000000000000000eeee";

#[tokio::test]
async fn foreign_trashed_and_hidden_items_are_indistinguishable_from_unknown_ids() {
    let f = Fixture::new("access", |c, product| {
        tree(c, product);
        file(
            c,
            product,
            OTHER,
            OTHER_ROOT,
            FOREIGN,
            "theirs.txt",
            b"private",
        );
        folder(c, OWNER, ROOT, HIDDEN_PARENT, "Hidden");
        file(
            c,
            product,
            OWNER,
            HIDDEN_PARENT,
            HIDDEN_CHILD,
            "inside.txt",
            b"x",
        );
        trash(c, HIDDEN_PARENT);
    })
    .await;
    let unknown = f.issue(&[UNKNOWN], &OWNER_ACTOR).await;
    assert_eq!(unknown.status, 404);
    for refused in [
        FOREIGN,
        GONE,
        HIDDEN_PARENT,
        HIDDEN_CHILD,
        ROOT,
        "not-an-id",
    ] {
        let reply = f.issue(&[ONE, refused], &OWNER_ACTOR).await;
        assert_eq!(
            (reply.status, &reply.body),
            (unknown.status, &unknown.body),
            "{refused} must look like an unknown ID"
        );
    }
    // Existing content routes answer foreign IDs the same way.
    let content = f
        .get(&format!("/entries/{FOREIGN}/content"), &OWNER_ACTOR)
        .await;
    assert_eq!(content.status, 404);

    // A ticket is bound to the issuing session.
    let issued = f.issue(&[ONE], &OWNER_ACTOR).await;
    let url = issued.json()["url"].as_str().unwrap().to_owned();
    assert_eq!(f.get(&url, &OTHER_ACTOR).await.status, 404);
    assert_eq!(f.get(&url, &OWNER_ACTOR).await.status, 200);

    // Mutation-style checks apply before any resolution.
    let body = br#"{"entry_ids":["00000000000000000000000000000306"]}"#;
    let mut no_csrf = f.head("POST", "/archives", Some(&OWNER_ACTOR));
    no_csrf.push_str(&format!(
        "Origin: {ORIGIN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    ));
    assert_eq!(f.send(no_csrf, body).await.status, 403);
    let mut foreign_origin = f.head("POST", "/archives", Some(&OWNER_ACTOR));
    foreign_origin.push_str(&format!(
        "Origin: https://evil.example\r\nX-CSRF-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        URL_SAFE_NO_PAD.encode(CSRF),
        body.len()
    ));
    assert_eq!(f.send(foreign_origin, body).await.status, 403);
    let mut anonymous = f.head("GET", &url, None);
    anonymous.push_str("\r\n");
    assert_eq!(f.send(anonymous, &[]).await.status, 401);
    f.close().await;
}

const LEFT: &str = "00000000000000000000000000000501";
const RIGHT: &str = "00000000000000000000000000000502";
const LEFT_SAME: &str = "00000000000000000000000000000503";
const RIGHT_SAME: &str = "00000000000000000000000000000504";
const BIG: &str = "00000000000000000000000000000505";
const EXACT: &str = "00000000000000000000000000000506";

fn many_folders(c: &Connection, parent: &str, count: usize, base: u32) {
    let tx = c.unchecked_transaction().unwrap();
    for index in 0..count {
        let id = format!("{:032x}", base as usize + index);
        folder(&tx, OWNER, parent, &id, &format!("f{index}"));
    }
    tx.commit().unwrap();
}

#[tokio::test]
async fn name_conflicts_and_entry_limit_are_refused_before_streaming() {
    let f = Fixture::new("limits", |c, product| {
        folder(c, OWNER, ROOT, LEFT, "Left");
        folder(c, OWNER, ROOT, RIGHT, "Right");
        file(c, product, OWNER, LEFT, LEFT_SAME, "same.txt", b"left");
        file(c, product, OWNER, RIGHT, RIGHT_SAME, "same.txt", b"right");
        folder(c, OWNER, ROOT, BIG, "Big");
        many_folders(c, BIG, 10_000, 0x10_0000);
        folder(c, OWNER, ROOT, EXACT, "Exact");
        many_folders(c, EXACT, 9_999, 0x20_0000);
    })
    .await;
    // Equal top-level names from different folders would collide in the archive.
    let conflict = f.issue(&[LEFT_SAME, RIGHT_SAME], &OWNER_ACTOR).await;
    assert_eq!(conflict.status, 409);
    assert_eq!(conflict.json()["code"], "archive_name_conflict");
    // Their parents are distinct top-level names, so selecting the folders works.
    let folders = f.download(&[LEFT, RIGHT]).await;
    assert!(folders.complete);
    assert_eq!(extract(&folders.body).len(), 4);

    let over = f.issue(&[BIG], &OWNER_ACTOR).await;
    assert_eq!(over.status, 422);
    assert_eq!(over.json()["code"], "archive_entry_limit");
    assert_eq!(over.json()["limit"], 10_000);
    let exact = f.issue(&[EXACT], &OWNER_ACTOR).await;
    assert_eq!(exact.status, 201);
    assert_eq!(exact.json()["entry_count"], 10_000);
    f.close().await;
}

#[tokio::test]
async fn corrupted_or_missing_payload_fails_the_transfer_without_a_central_directory() {
    let f = Fixture::new("integrity", tree).await;
    let path = f.blob_path(&blob_of(ONE));
    let mut bytes = fs::read(&path).unwrap();
    bytes[1000] ^= 0xff;
    fs::write(&path, &bytes).unwrap();
    // DOCS streams first (path order), then the corrupted file.
    let reply = f.download(&[DOCS, ONE]).await;
    assert_eq!(reply.status, 200, "headers were sent before the failure");
    assert!(!reply.complete, "chunked body must not terminate cleanly");
    assert!(
        reply.body.starts_with(&0x0403_4b50_u32.to_le_bytes()),
        "earlier members were streamed"
    );
    for signature in [0x0201_4b50_u32, 0x0605_4b50] {
        assert!(
            !reply
                .body
                .windows(4)
                .any(|window| window == signature.to_le_bytes()),
            "no central directory may follow a failure"
        );
    }

    fs::remove_file(f.blob_path(&blob_of(TWO))).unwrap();
    let reply = f.download(&[DOCS, TWO]).await;
    assert_eq!(reply.status, 200);
    assert!(!reply.complete);
    f.close().await;
}

const LARGE: &str = "00000000000000000000000000000601";

#[tokio::test]
async fn cancelled_download_drops_the_archive_stream() {
    let f = Fixture::new("cancel", |c, product| {
        file(
            c,
            product,
            OWNER,
            ROOT,
            LARGE,
            "large.bin",
            &pattern(48 << 20, 3),
        );
    })
    .await;
    let issued = f.issue(&[LARGE], &OWNER_ACTOR).await;
    // The router's own state clones hold the token too.
    let idle = Arc::strong_count(&f.activity);
    let url = issued.json()["url"].as_str().unwrap().to_owned();
    let mut request = f.head("GET", &url, Some(&OWNER_ACTOR));
    request.push_str("\r\n");
    let mut stream = f.connect().await;
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut received = 0;
    let mut buffer = [0u8; 64 * 1024];
    while received < 256 * 1024 {
        received += stream.read(&mut buffer).await.unwrap();
    }
    assert!(
        Arc::strong_count(&f.activity) > idle,
        "the archive body is live while streaming"
    );
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(&f.activity) > idle {
        assert!(
            Instant::now() < deadline,
            "archive work continued after the client went away"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f.close().await;
}
