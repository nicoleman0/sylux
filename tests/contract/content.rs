//! S1 A07/A08 real-route content contracts over validated loopback TLS.
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
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use sylux::{
    domain::authz::verifier,
    http::{self, AuthState},
    persistence::{
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
const BUSY: Duration = Duration::from_millis(375);

struct Fixture {
    root: FixtureRoot,
    server: Option<TlsApiServer>,
    workers: Option<SqliteWorkers>,
    ca: Vec<u8>,
}
impl Fixture {
    async fn new(case: &str) -> Self {
        let parent = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("sylux-content-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let root = FixtureRoot::create(&parent, "content", case).unwrap();
        let product = root.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).unwrap();
        let tls = TlsFiles::generate(&root, "tls").unwrap();
        let ca = fs::read(&tls.ca_der).unwrap();
        let db_path = product.join("metadata/sylux.sqlite3");
        let connection = sqlite::open_writer(&db_path, BUSY).unwrap();
        seed(&connection);
        seed_file(&connection, &product);
        let workers = SqliteWorkers::from_writer(
            connection,
            &db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let app = http::router(
            AuthState::new(workers.clone(), "https://127.0.0.1".into())
                .with_storage_root(&product)
                .unwrap(),
        );
        let server = TlsApiServer::start(&root, &tls, app, Duration::from_secs(2))
            .await
            .unwrap();
        Self {
            root,
            server: Some(server),
            workers: Some(workers),
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
    async fn request(
        &self,
        method: &str,
        path: &str,
        actor: Option<(&str, &[u8; 32])>,
        headers: &[(&str, String)],
    ) -> (u16, BTreeMap<String, String>, Vec<u8>) {
        let server = self.server.as_ref().unwrap();
        let mut stream = tls_connect(server.address(), &self.ca, "127.0.0.1")
            .await
            .unwrap();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n",
            server.address().port()
        );
        if let Some((id, secret)) = actor {
            req.push_str(&format!(
                "Cookie: __Host-sylux={id}.{}\r\n",
                URL_SAFE_NO_PAD.encode(secret)
            ));
        }
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&bytes[..split]);
        let mut lines = head.lines();
        let status = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let mut hs = BTreeMap::new();
        for l in lines {
            if let Some((k, v)) = l.split_once(':') {
                hs.insert(k.to_ascii_lowercase(), v.trim().to_owned());
            }
        }
        (status, hs, bytes[split + 4..].to_vec())
    }
}
fn seed(c: &Connection) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
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
        c.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,?2,'user',1,0)",params![account,handle]).unwrap();
        c.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)",params![root,account]).unwrap();
        c.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?4,?5,?6,?7,0)",params![sid,account,verifier(secret).as_slice(),verifier(csrf).as_slice(),now,now+3600,now+7200]).unwrap();
    }
}
fn seed_file(c: &Connection, product: &std::path::Path) {
    let bytes = b"synthetic\0content\xff\x80";
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let blob_id = "abcdef0123456789abcdef0123456789";
    let attempt_id = "1234567890abcdef1234567890abcdef";
    let store = sylux::persistence::payload::PayloadStore::open(product).unwrap();
    let mut staged = store
        .start(&format!("{attempt_id}-{blob_id}"), bytes.len() as u64)
        .unwrap();
    staged.write_chunk(bytes).unwrap();
    let published = staged.publish(digest).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let tx = c.unchecked_transaction().unwrap();
    tx.execute(
        "INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,?2,?3,?4,'live')",
        params![
            published.blob_id,
            &published.blob_id[..2],
            bytes.len() as i64,
            digest.as_slice()
        ],
    )
    .unwrap();
    tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,current_revision_id,generation) VALUES('00000000000000000000000000000301',?1,?2,'file','unsafe <name>.bin',x'756e73616665203c6e616d653e2e62696e',1,NULL,1)", params![OWNER, ROOT]).unwrap();
    tx.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES('00000000000000000000000000000302','00000000000000000000000000000301',?1,?2,?3,?4,?5)", params![published.blob_id, bytes.len() as i64, digest.as_slice(), now, OWNER]).unwrap();
    tx.execute("UPDATE entry SET current_revision_id='00000000000000000000000000000302' WHERE entry_id='00000000000000000000000000000301'", []).unwrap();
    tx.commit().unwrap();
}

fn auth<'a>(id: &'a str, secret: &'a [u8; 32]) -> (&'a str, &'a [u8; 32]) {
    (id, secret)
}

#[tokio::test]
async fn a07_current_content_has_revision_etag_safe_attachment_and_exact_bytes() {
    let f = Fixture::new("a07-current-content").await;
    let id = "00000000000000000000000000000301";
    let bytes = b"synthetic\0content\xff\x80";
    let (status, headers, body) = f
        .request(
            "GET",
            &format!("/entries/{id}/content"),
            Some(auth(SESSION, SECRET)),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        body, bytes,
        "download must preserve original binary bytes exactly"
    );
    let etag = headers.get("etag").expect("strong revision ETag").clone();
    assert!(
        etag.starts_with('"') && etag.ends_with('"') && !etag.starts_with("W/"),
        "ETag must be strong: {etag}"
    );
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/octet-stream")
    );
    assert_eq!(
        headers.get("x-content-type-options").map(String::as_str),
        Some("nosniff")
    );
    assert_eq!(
        headers.get("referrer-policy").map(String::as_str),
        Some("no-referrer")
    );
    assert_eq!(
        headers.get("cache-control").map(String::as_str),
        Some("no-store"),
        "private content responses must not be stored by caches"
    );
    let disposition = headers
        .get("content-disposition")
        .expect("attachment disposition");
    assert!(disposition.starts_with("attachment;") && disposition.contains("filename*=UTF-8''"));
    let (status, headers, body) = f
        .request(
            "GET",
            &format!("/entries/{id}/content"),
            Some(auth(SESSION, SECRET)),
            &[("If-None-Match", etag)],
        )
        .await;
    assert_eq!(
        status, 304,
        "matching current revision validator must return Not Modified"
    );
    assert!(body.is_empty());
    assert!(headers.contains_key("etag"));
    let (status, headers, body) = f
        .request(
            "HEAD",
            &format!("/entries/{id}/content"),
            Some(auth(SESSION, SECRET)),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert!(body.is_empty(), "HEAD has no body");
    assert_eq!(
        headers.get("content-length").map(String::as_str),
        Some(bytes.len().to_string().as_str())
    );
    f.close().await;
}

#[tokio::test]
async fn a08_authorizes_before_conditional_response_disclosure() {
    let f = Fixture::new("a08-auth-before-condition").await;
    // Both a guessed private ID and the owner's root fail as content resources; a cache
    // validator must not turn either request into a revealing conditional response.
    let id = "0000000000000000000000000000eeee";
    for (method, path, actor) in [
        (
            "GET",
            format!("/entries/{id}/content"),
            Some(auth(OTHER_SESSION, OTHER_SECRET)),
        ),
        (
            "HEAD",
            format!("/entries/{ROOT}/content"),
            Some(auth(SESSION, SECRET)),
        ),
    ] {
        let (status, _, _) = f
            .request(method, &path, actor, &[("If-None-Match", "*".into())])
            .await;
        assert_eq!(
            status, 404,
            "{method} must authorize/resolve before evaluating validators"
        );
    }
    let (status, _, _) = f
        .request(
            "GET",
            &format!("/entries/{id}/content"),
            None,
            &[("If-None-Match", "*".into())],
        )
        .await;
    assert_eq!(
        status, 401,
        "anonymous conditional content requests require authentication"
    );
    f.close().await;
}

#[tokio::test]
async fn a08_missing_precondition_and_out_of_slice_methods_are_not_content_reads() {
    let f = Fixture::new("a08-content-boundary").await;
    // The endpoint is read-only: write methods and future history routes stay absent.
    let id = "0000000000000000000000000000dddd";
    let (status, _, _) = f
        .request(
            "PUT",
            &format!("/entries/{id}/content"),
            Some(auth(SESSION, SECRET)),
            &[],
        )
        .await;
    assert!(
        status == 404 || status == 405,
        "content route must not expose a write operation: {status}"
    );
    let (status,_,_)=f.request("GET","/entries/00000000000000000000000000000001/versions/00000000000000000000000000000002/content",Some(auth(SESSION,SECRET)),&[]).await;
    assert_eq!(status, 404, "history content is outside S1");
    f.close().await;
}
