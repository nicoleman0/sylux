//! PoC P3 real HTTP + SQLite local-auth contracts.
//!
//! The server is the real product router on an ephemeral loopback TCP
//! listener. The only harness-only operation is direct seeding/mutation of a
//! disposable product database. The staged-mutation revoke race belongs to P5
//! and is deliberately not claimed here.
#[path = "../common/mod.rs"]
mod common;

use argon2::{Argon2, password_hash::PasswordHasher};
use axum::body::Bytes;
use common::FixtureRoot;
use rusqlite::{Connection, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use sylux::{
    http::{self, AuthState},
    persistence::{
        root,
        sqlite::{self, SqliteWorkers, WorkerConfig},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

const BUSY: Duration = Duration::from_millis(375);
const PASSWORD: &str = "correct horse battery staple";
const ACCOUNT_ID: &str = "00000000000000000000000000000001";

struct TestFixture {
    owner: FixtureRoot,
    db_path: PathBuf,
    db: Option<Arc<Mutex<Connection>>>,
    workers: Option<SqliteWorkers>,
    origin: String,
    address: String,
    server: Option<JoinHandle<()>>,
}

impl TestFixture {
    async fn new(case: &str) -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-p3-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "auth", case).unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).expect("product initialization");
        let db_path = product.join("metadata").join("sylux.sqlite3");
        let connection = sqlite::open_writer(&db_path, BUSY).expect("product SQLite writer");
        let verifier = Argon2::default()
            .hash_password(PASSWORD.as_bytes())
            .unwrap()
            .to_string();
        connection
            .execute(
                "INSERT INTO account(account_id,handle,role,enabled,credential_epoch,password_verifier) VALUES(?1,'owner','administrator',1,0,?2)",
                params![ACCOUNT_ID, verifier],
            )
            .unwrap();

        let workers = SqliteWorkers::from_writer(
            connection,
            &db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let db = Arc::new(Mutex::new(sqlite::open_writer(&db_path, BUSY).unwrap()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let origin = format!("http://{address}");
        let state = AuthState::new(workers.clone(), origin.clone())
            .with_storage_root(&product)
            .unwrap();
        let app = http::router(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            owner,
            db_path,
            db: Some(db),
            workers: Some(workers),
            origin,
            address,
            server: Some(server),
        }
    }

    fn db(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(self.db.as_ref().unwrap())
    }

    async fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        if let Some(workers) = self.workers.take() {
            workers.shutdown().await.unwrap();
        }
        self.db.take();
    }

    async fn restart_with_storage(&mut self) {
        self.stop().await;
        let product = self.owner.path().join("product");
        let connection = sqlite::open_writer(&self.db_path, BUSY).unwrap();
        let workers = SqliteWorkers::from_writer(
            connection,
            &self.db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.address = listener.local_addr().unwrap().to_string();
        self.origin = format!("http://{}", self.address);
        let setup = fs::read_to_string(product.join("secrets/setup.authorization")).unwrap();
        let app = http::router(
            AuthState::new_with_setup(workers.clone(), self.origin.clone(), setup)
                .with_storage_root(&product)
                .unwrap(),
        );
        self.db = Some(Arc::new(Mutex::new(
            sqlite::open_writer(&self.db_path, BUSY).unwrap(),
        )));
        self.workers = Some(workers);
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
    }

    async fn close(mut self) {
        self.stop().await;
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        origin: Option<&str>,
        csrf: Option<&str>,
        json: Option<String>,
    ) -> TestResponse {
        let mut stream = TcpStream::connect(&self.address).await.unwrap();
        let body = json.unwrap_or_default();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n",
            self.address
        );
        if let Some(cookie) = cookie {
            request.push_str(&format!("Cookie: {cookie}\r\n"));
        }
        if let Some(origin) = origin {
            request.push_str(&format!("Origin: {origin}\r\n"));
        }
        if let Some(csrf) = csrf {
            request.push_str(&format!("X-CSRF-Token: {csrf}\r\n"));
        }
        if method == "POST" || method == "PATCH" {
            request.push_str("Content-Type: application/json\r\n");
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        TestResponse::parse(&bytes)
    }

    async fn bootstrap(&self) -> PreSession {
        let response = self
            .request("GET", "/auth/csrf", None, Some(&self.origin), None, None)
            .await;
        assert_eq!(response.status, 200);
        assert_eq!(response.header("cache-control"), Some("no-store"));
        assert_cookie_flags(&response);
        let cookie = response.cookie();
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        let csrf = body
            .get("csrf_token")
            .and_then(Value::as_str)
            .unwrap()
            .to_owned();
        assert!(body.as_object().unwrap().len() == 1);
        PreSession { cookie, csrf }
    }

    async fn login(&self, pre: &PreSession, handle: &str, password: &str) -> TestResponse {
        self.request(
            "POST",
            "/auth/local",
            Some(&pre.cookie),
            Some(&self.origin),
            Some(&pre.csrf),
            Some(serde_json::json!({"handle":handle,"password":password}).to_string()),
        )
        .await
    }

    async fn authenticated(&self) -> Authenticated {
        let pre = self.bootstrap().await;
        let response = self.login(&pre, "owner", PASSWORD).await;
        assert_eq!(response.status, 200);
        Authenticated::from_response(&response)
    }

    async fn me(&self, cookie: &str) -> TestResponse {
        self.request("GET", "/me", Some(cookie), None, None, None)
            .await
    }

    async fn logout(&self, auth: &Authenticated) -> TestResponse {
        self.request(
            "POST",
            "/auth/logout",
            Some(&auth.cookie),
            Some(&self.origin),
            Some(&auth.csrf),
            None,
        )
        .await
    }
}

struct PreSession {
    cookie: String,
    csrf: String,
}

struct Authenticated {
    cookie: String,
    csrf: String,
}

#[tokio::test]
async fn readiness_tracks_storage_worker_while_liveness_remains_process_only() {
    let mut fixture = TestFixture::new("ready-live").await;
    assert_eq!(
        fixture
            .request("GET", "/livez", None, None, None, None)
            .await
            .status,
        204
    );
    assert_eq!(
        fixture
            .request("GET", "/readyz", None, None, None, None)
            .await
            .status,
        204
    );
    fixture.workers.take().unwrap().shutdown().await.unwrap();
    assert_eq!(
        fixture
            .request("GET", "/readyz", None, None, None, None)
            .await
            .status,
        503
    );
    assert_eq!(
        fixture
            .request("GET", "/livez", None, None, None, None)
            .await
            .status,
        204
    );
    fixture.close().await;
}

impl Authenticated {
    fn from_response(response: &TestResponse) -> Self {
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        Self {
            cookie: response.cookie(),
            csrf: body
                .get("csrf_token")
                .and_then(Value::as_str)
                .unwrap()
                .to_owned(),
        }
    }
}

struct TestResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Bytes,
}

impl TestResponse {
    fn parse(bytes: &[u8]) -> Self {
        let split = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("HTTP header terminator");
        let head = std::str::from_utf8(&bytes[..split]).unwrap();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|status| status.parse().ok())
            .expect("HTTP status");
        let mut headers = BTreeMap::new();
        for line in lines {
            let (name, value) = line.split_once(':').expect("HTTP header");
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
        let raw = &bytes[split + 4..];
        let body = if headers
            .get("transfer-encoding")
            .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
        {
            decode_chunked(raw)
        } else {
            raw.to_vec()
        };
        Self {
            status,
            headers,
            body: Bytes::from(body),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    fn cookie(&self) -> String {
        self.header("set-cookie")
            .and_then(|value| value.split(';').next())
            .unwrap()
            .to_owned()
    }
}

fn decode_chunked(mut bytes: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    loop {
        let end = bytes
            .windows(2)
            .position(|window| window == b"\r\n")
            .expect("chunk size");
        let size = usize::from_str_radix(std::str::from_utf8(&bytes[..end]).unwrap(), 16).unwrap();
        bytes = &bytes[end + 2..];
        if size == 0 {
            break;
        }
        decoded.extend_from_slice(&bytes[..size]);
        assert_eq!(&bytes[size..size + 2], b"\r\n");
        bytes = &bytes[size + 2..];
    }
    decoded
}

fn assert_cookie_flags(response: &TestResponse) {
    let set_cookie = response.header("set-cookie").unwrap();
    for flag in ["Path=/", "Secure", "HttpOnly", "SameSite=Lax", "Max-Age="] {
        assert!(set_cookie.contains(flag));
    }
    assert!(!set_cookie.contains("Domain="));
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn file_contains(path: &Path, needle: &[u8]) -> bool {
    fs::read(path)
        .unwrap()
        .windows(needle.len())
        .any(|window| window == needle)
}

#[tokio::test]
async fn login_rotates_pre_session_and_persists_only_verifiers() {
    let fixture = TestFixture::new("rotation").await;
    let pre = fixture.bootstrap().await;
    let anonymous = pre.cookie.clone();
    let anonymous_csrf = pre.csrf.clone();
    assert_eq!(fixture.me(&anonymous).await.status, 401);

    let response = fixture.login(&pre, "owner", PASSWORD).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.header("cache-control"), Some("no-store"));
    assert_cookie_flags(&response);
    let auth = Authenticated::from_response(&response);
    assert!(auth.cookie.starts_with("__Host-sylux="));
    assert_ne!(auth.cookie, anonymous);
    assert!(!auth.cookie.contains("anon."));
    assert_eq!(fixture.me(&anonymous).await.status, 401);

    let replay = fixture.login(&pre, "owner", PASSWORD).await;
    assert_eq!(replay.status, 401);
    let me = fixture.me(&auth.cookie).await;
    assert_eq!(me.status, 200);
    let identity: Value = serde_json::from_slice(&me.body).unwrap();
    assert_eq!(
        identity.get("account_id").and_then(Value::as_str),
        Some(ACCOUNT_ID)
    );
    assert_eq!(
        identity.get("handle").and_then(Value::as_str),
        Some("owner")
    );
    assert_eq!(
        identity.get("role").and_then(Value::as_str),
        Some("administrator")
    );

    let db = fixture.db();
    let (session_count, session_verifier, csrf_verifier, password_verifier): (
        i64,
        Vec<u8>,
        Vec<u8>,
        String,
    ) = db.lock()
        .unwrap()
        .query_row(
            "SELECT (SELECT count(*) FROM session),(SELECT verifier FROM session),(SELECT csrf_verifier FROM session),(SELECT password_verifier FROM account WHERE account_id=?1)",
            [ACCOUNT_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(session_count, 1);
    assert_eq!(session_verifier.len(), 32);
    assert_eq!(csrf_verifier.len(), 32);
    assert!(!password_verifier.contains(PASSWORD));
    let cookie_secret = auth.cookie.split('.').nth(1).unwrap().as_bytes();
    let anonymous_secret = anonymous
        .strip_prefix("__Host-sylux=anon.")
        .unwrap()
        .as_bytes();
    assert_ne!(session_verifier.as_slice(), cookie_secret);
    assert_ne!(csrf_verifier.as_slice(), auth.csrf.as_bytes());
    db.lock()
        .unwrap()
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    for secret in [
        PASSWORD.as_bytes(),
        cookie_secret,
        auth.csrf.as_bytes(),
        anonymous_secret,
        anonymous_csrf.as_bytes(),
    ] {
        assert!(!file_contains(&fixture.db_path, secret));
    }
    drop(db);
    fixture.close().await;
}

#[tokio::test]
async fn wrong_and_unknown_login_have_identical_generic_failure() {
    let fixture = TestFixture::new("generic-login").await;
    let unknown_pre = fixture.bootstrap().await;
    let unknown = fixture.login(&unknown_pre, "nobody", PASSWORD).await;
    let wrong_pre = fixture.bootstrap().await;
    let wrong = fixture.login(&wrong_pre, "owner", "definitely wrong").await;
    assert_eq!(unknown.status, 401);
    assert_eq!(wrong.status, 401);
    assert_eq!(unknown.body, wrong.body);
    assert_eq!(unknown.header("set-cookie"), None);
    assert_eq!(wrong.header("set-cookie"), None);
    let sessions: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM session", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sessions, 0);
    fixture.close().await;
}

#[tokio::test]
async fn csrf_bootstrap_allows_absent_origin_for_same_origin_browser_get() {
    let fixture = TestFixture::new("csrf_absent_origin").await;
    // A same-origin browser GET omits the Origin header; the bootstrap must
    // still issue an anonymous pre-session + CSRF token (mutations stay strict).
    let response = fixture
        .request("GET", "/auth/csrf", None, None, None, None)
        .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.header("cache-control"), Some("no-store"));
    assert_cookie_flags(&response);
    let body: Value = serde_json::from_slice(&response.body).unwrap();
    assert!(body.get("csrf_token").and_then(Value::as_str).is_some());
    assert_eq!(body.as_object().unwrap().len(), 1);

    // A present-but-mismatched Origin is still rejected.
    let foreign = fixture
        .request(
            "GET",
            "/auth/csrf",
            None,
            Some("https://foreign.example"),
            None,
            None,
        )
        .await;
    assert_eq!(foreign.status, 403);
    fixture.close().await;
}

#[tokio::test]
async fn cookie_auth_mutations_require_exact_origin() {
    let fixture = TestFixture::new("origin").await;
    let pre = fixture.bootstrap().await;
    for origin in [None, Some("null"), Some("https://foreign.example")] {
        let response = fixture
            .request(
                "POST",
                "/auth/local",
                Some(&pre.cookie),
                origin,
                Some(&pre.csrf),
                Some(serde_json::json!({"handle":"owner","password":PASSWORD}).to_string()),
            )
            .await;
        assert_eq!(response.status, 403);
    }
    assert_eq!(fixture.login(&pre, "owner", PASSWORD).await.status, 200);

    let auth = fixture.authenticated().await;
    for origin in [None, Some("null"), Some("https://foreign.example")] {
        let response = fixture
            .request(
                "POST",
                "/auth/logout",
                Some(&auth.cookie),
                origin,
                Some(&auth.csrf),
                None,
            )
            .await;
        assert_eq!(response.status, 403);
        assert_eq!(fixture.me(&auth.cookie).await.status, 200);
    }
    fixture.close().await;
}

#[tokio::test]
async fn offline_recovery_preserves_setup_private_inventory_and_downloads_after_restart() {
    use sha2::{Digest, Sha256};
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let mut fixture = TestFixture::new("offline-recovery").await;
    let product = fixture.owner.path().join("product");
    const NEW: &str = "  recovered administrator password  ";
    const OTHER: &str = "00000000000000000000000000000002";
    let owner_file = "00000000000000000000000000000301";
    let other_file = "00000000000000000000000000000302";
    let bytes = b"private recovery fixture\x00\xff";
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    {
        let db = fixture.db();
        let db = db.lock().unwrap();
        db.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch,password_verifier) SELECT ?1,'other','user',1,0,password_verifier FROM account WHERE account_id=?2",params![OTHER,ACCOUNT_ID]).unwrap();
        db.execute_batch("UPDATE installation SET setup_consumed=1,setup_verifier=zeroblob(32)")
            .unwrap();
        let store = sylux::persistence::payload::PayloadStore::open(&product).unwrap();
        for (account, root_id, file_id, revision_id) in [
            (
                ACCOUNT_ID,
                "00000000000000000000000000000101",
                owner_file,
                "00000000000000000000000000000401",
            ),
            (
                OTHER,
                "00000000000000000000000000000102",
                other_file,
                "00000000000000000000000000000402",
            ),
        ] {
            let mut staged = store
                .start(&format!("{file_id}-{revision_id}"), bytes.len() as u64)
                .unwrap();
            staged.write_chunk(bytes).unwrap();
            let published = staged.publish(digest).unwrap();
            db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,NULL,'folder','',x'',1)",params![root_id,account]).unwrap();
            db.execute(
                "INSERT INTO blob(blob_id,shard,length,sha256) VALUES(?1,?2,?3,?4)",
                params![
                    published.blob_id,
                    &published.blob_id[..2],
                    bytes.len() as i64,
                    digest.as_slice()
                ],
            )
            .unwrap();
            db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'file','fixture.bin',x'666978747572652e62696e',1)",params![file_id,account,root_id]).unwrap();
            db.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?2,?3,?4,?5,1,?6)",params![revision_id,file_id,published.blob_id,bytes.len() as i64,digest.as_slice(),account]).unwrap();
            db.execute(
                "UPDATE entry SET current_revision_id=?1 WHERE entry_id=?2",
                params![revision_id, file_id],
            )
            .unwrap();
        }
    }
    fixture.restart_with_storage().await;
    let old_auth = fixture.authenticated().await;
    let pre = fixture.bootstrap().await;
    let other = fixture.login(&pre, "other", PASSWORD).await;
    assert_eq!(other.status, 200);
    let other_auth = Authenticated::from_response(&other);
    let mut listings = Vec::new();
    for (cookie, file_id) in [
        (&old_auth.cookie, owner_file),
        (&other_auth.cookie, other_file),
    ] {
        let listing = fixture
            .request("GET", "/folders/children", Some(cookie), None, None, None)
            .await;
        assert_eq!(listing.status, 200);
        listings.push(listing.body);
        let download = fixture
            .request(
                "GET",
                &format!("/entries/{file_id}/content"),
                Some(cookie),
                None,
                None,
                None,
            )
            .await;
        assert_eq!(download.status, 200);
        assert_eq!(download.body.as_ref(), bytes);
    }
    fixture.stop().await;
    let identity = fs::read(product.join("installation.json")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_sylux"))
        .args(["admin", "reset-password", "--root"])
        .arg(&product)
        .args(["--handle", "owner", "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{NEW}\n").as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stderr, b"sylux: administrator_password_reset\n");
    assert_eq!(
        fs::read(product.join("installation.json")).unwrap(),
        identity
    );
    fixture.restart_with_storage().await;
    assert_eq!(fixture.me(&old_auth.cookie).await.status, 401);
    assert_eq!(fixture.me(&other_auth.cookie).await.status, 200);
    let pre = fixture.bootstrap().await;
    assert_eq!(fixture.login(&pre, "owner", PASSWORD).await.status, 401);
    let pre = fixture.bootstrap().await;
    let response = fixture.login(&pre, "owner", NEW).await;
    assert_eq!(response.status, 200);
    let new_auth = Authenticated::from_response(&response);
    for (index, (cookie, file_id, foreign_id)) in [
        (&new_auth.cookie, owner_file, other_file),
        (&other_auth.cookie, other_file, owner_file),
    ]
    .into_iter()
    .enumerate()
    {
        let listing = fixture
            .request("GET", "/folders/children", Some(cookie), None, None, None)
            .await;
        assert_eq!(listing.status, 200);
        assert_eq!(listing.body, listings[index]);
        let download = fixture
            .request(
                "GET",
                &format!("/entries/{file_id}/content"),
                Some(cookie),
                None,
                None,
                None,
            )
            .await;
        assert_eq!(download.status, 200);
        assert_eq!(download.body.as_ref(), bytes);
        assert_eq!(
            fixture
                .request(
                    "GET",
                    &format!("/entries/{foreign_id}/content"),
                    Some(cookie),
                    None,
                    None,
                    None
                )
                .await
                .status,
            404
        );
    }
    let setup = fs::read_to_string(product.join("secrets/setup.authorization")).unwrap();
    for (method, path, body) in [
        ("GET", "/setup", String::new()),
        (
            "POST",
            "/setup/admin",
            serde_json::json!({"handle":"new-admin","password":PASSWORD}).to_string(),
        ),
    ] {
        let mut stream = TcpStream::connect(&fixture.address).await.unwrap();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nOrigin: {}\r\nX-Setup-Authorization: {setup}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            fixture.address,
            fixture.origin,
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        assert_eq!(TestResponse::parse(&response).status, 404);
    }
    let consumed: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT setup_consumed FROM installation", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(consumed, 1);
    fixture.close().await;
}

#[tokio::test]
async fn cookie_auth_mutations_require_csrf() {
    let fixture = TestFixture::new("csrf").await;
    for csrf in [
        None,
        Some("wrong-token"),
        Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
    ] {
        // Use a fresh pre-session for every attempt: a well-formed but wrong
        // token is rejected after the one-time anonymous session is consumed.
        let pre = fixture.bootstrap().await;
        let response = fixture
            .request(
                "POST",
                "/auth/local",
                Some(&pre.cookie),
                Some(&self_origin(&fixture)),
                csrf,
                Some(serde_json::json!({"handle":"owner","password":PASSWORD}).to_string()),
            )
            .await;
        assert_eq!(response.status, 403);
    }

    let auth = fixture.authenticated().await;
    for csrf in [
        None,
        Some("wrong-token"),
        Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
    ] {
        let response = fixture
            .request(
                "POST",
                "/auth/logout",
                Some(&auth.cookie),
                Some(&self_origin(&fixture)),
                csrf,
                None,
            )
            .await;
        assert_eq!(response.status, 403);
        assert_eq!(fixture.me(&auth.cookie).await.status, 200);
    }
    fixture.close().await;
}

fn self_origin(fixture: &TestFixture) -> String {
    fixture.origin.clone()
}

#[tokio::test]
async fn logout_commits_revocation_before_no_content() {
    let fixture = TestFixture::new("logout").await;
    let auth = fixture.authenticated().await;
    let response = fixture.logout(&auth).await;
    assert_eq!(response.status, 204);
    assert_cookie_flags(&response);
    assert!(response.header("set-cookie").unwrap().contains("Max-Age=0"));
    let revoked: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT revoked FROM session", [], |row| row.get(0))
        .unwrap();
    assert_eq!(revoked, 1);
    assert_eq!(fixture.me(&auth.cookie).await.status, 401);
    fixture.close().await;
}

#[tokio::test]
async fn me_rechecks_enabled_epoch_revocation_and_both_expiries() {
    for (case, statement) in [
        ("disabled", "UPDATE account SET enabled=0".to_owned()),
        ("epoch", "UPDATE account SET credential_epoch=1".to_owned()),
        ("revoked", "UPDATE session SET revoked=1".to_owned()),
        ("idle", "UPDATE session SET idle_expires_at=1".to_owned()),
        (
            "absolute",
            format!(
                "UPDATE session SET absolute_expires_at={},idle_expires_at={}",
                unix_now() - 1,
                unix_now() + 3600
            ),
        ),
    ] {
        let fixture = TestFixture::new(case).await;
        let auth = fixture.authenticated().await;
        assert_eq!(fixture.me(&auth.cookie).await.status, 200);
        fixture
            .db()
            .lock()
            .unwrap()
            .execute(&statement, [])
            .unwrap();
        assert_eq!(fixture.me(&auth.cookie).await.status, 401, "case {case}");
        fixture.close().await;
    }
}

#[tokio::test]
async fn instance_name_is_public_but_only_admin_can_save_with_csrf() {
    let fixture = TestFixture::new("instance-name").await;
    let response = fixture
        .request("GET", "/instance", None, None, None, None)
        .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.header("cache-control"), Some("no-store"));
    assert_eq!(
        serde_json::from_slice::<Value>(&response.body).unwrap(),
        serde_json::json!({"name":"Sylux"})
    );
    let body = Some(serde_json::json!({"name":"Private files"}).to_string());
    assert_eq!(
        fixture
            .request(
                "PATCH",
                "/instance",
                None,
                Some(&fixture.origin),
                None,
                body.clone()
            )
            .await
            .status,
        401
    );
    let auth = fixture.authenticated().await;
    assert_eq!(
        fixture
            .request(
                "PATCH",
                "/instance",
                Some(&auth.cookie),
                Some(&fixture.origin),
                None,
                body.clone()
            )
            .await
            .status,
        403
    );
    assert_eq!(
        fixture
            .request(
                "PATCH",
                "/instance",
                Some(&auth.cookie),
                Some("https://untrusted.test"),
                Some(&auth.csrf),
                body.clone()
            )
            .await
            .status,
        403
    );
    for name in ["", "   ", "line\nfeed", "control\u{0000}", &"x".repeat(41)] {
        let response = fixture
            .request(
                "PATCH",
                "/instance",
                Some(&auth.cookie),
                Some(&fixture.origin),
                Some(&auth.csrf),
                Some(serde_json::json!({"name":name}).to_string()),
            )
            .await;
        assert_eq!(response.status, 400);
    }
    let response = fixture
        .request(
            "PATCH",
            "/instance",
            Some(&auth.cookie),
            Some(&fixture.origin),
            Some(&auth.csrf),
            Some(serde_json::json!({"name":"  Private files  "}).to_string()),
        )
        .await;
    assert_eq!(response.status, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&response.body).unwrap(),
        serde_json::json!({"name":"Private files"})
    );
    let reopened =
        sylux::persistence::instance::InstanceSettings::open(&fixture.owner.path().join("product"))
            .unwrap();
    assert_eq!(reopened.name().unwrap(), "Private files");
    let response = fixture
        .request("GET", "/instance", None, None, None, None)
        .await;
    assert_eq!(
        serde_json::from_slice::<Value>(&response.body).unwrap(),
        serde_json::json!({"name":"Private files"})
    );
    fixture
        .db()
        .lock()
        .unwrap()
        .execute(
            "UPDATE account SET role='user' WHERE account_id=?1",
            [ACCOUNT_ID],
        )
        .unwrap();
    assert_eq!(
        fixture
            .request(
                "PATCH",
                "/instance",
                Some(&auth.cookie),
                Some(&fixture.origin),
                Some(&auth.csrf),
                body
            )
            .await
            .status,
        403
    );
    assert_eq!(reopened.name().unwrap(), "Private files");
    fixture.close().await;
}

#[test]
fn instance_settings_reject_symlinks_and_corrupt_persisted_names() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = std::env::temp_dir().join(format!("sylux-instance-{}", common::unique_token()));
    fs::create_dir(&directory).unwrap();
    fs::create_dir(directory.join("metadata")).unwrap();
    let store = sylux::persistence::instance::InstanceSettings::open(&directory).unwrap();
    assert_eq!(store.name().unwrap(), "Sylux");
    store.save("Files").unwrap();
    let path = directory.join("metadata/instance.json");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::write(&path, br#"{"name":""}"#).unwrap();
    assert!(sylux::persistence::instance::InstanceSettings::open(&directory).is_err());
    fs::remove_file(&path).unwrap();
    symlink("outside", &path).unwrap();
    assert!(store.name().is_err());
    store.save("Restored").unwrap();
    assert!(
        !fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(store.name().unwrap(), "Restored");
    fs::remove_dir_all(directory).unwrap();
}

const SECOND_ACCOUNT: &str = "00000000000000000000000000000002";
const DISABLED_ACCOUNT: &str = "00000000000000000000000000000003";
const OTHER_ADMIN: &str = "00000000000000000000000000000004";
const PRIVATE_ROOT: &str = "00000000000000000000000000000011";
const PRIVATE_FILE: &str = "00000000000000000000000000000012";
const PRIVATE_REVISION: &str = "00000000000000000000000000000013";

impl TestFixture {
    fn seed_account(&self, id: &str, handle: &str, role: &str, enabled: bool) {
        self.db().lock().unwrap().execute(
            "INSERT INTO account(account_id,handle,role,enabled,credential_epoch,password_verifier)
             SELECT ?1,?2,?3,?4,0,password_verifier FROM account WHERE account_id=?5",
            params![id, handle, role, enabled, ACCOUNT_ID],
        ).unwrap();
    }

    async fn sign_in(&self, handle: &str) -> Authenticated {
        let response = self.login(&self.bootstrap().await, handle, PASSWORD).await;
        assert_eq!(response.status, 200);
        Authenticated::from_response(&response)
    }

    async fn account_action(
        &self,
        auth: &Authenticated,
        target: &str,
        action: &str,
        role: Option<&str>,
    ) -> TestResponse {
        self.request(
            "POST",
            &format!("/accounts/{target}/{action}"),
            Some(&auth.cookie),
            Some(&self.origin),
            Some(&auth.csrf),
            role.map(|role| serde_json::json!({"role":role}).to_string()),
        )
        .await
    }

    fn account_state(&self, id: &str) -> (String, bool, i64, String, i64) {
        self.db().lock().unwrap().query_row(
            "SELECT role,enabled,credential_epoch,password_verifier,(SELECT count(*) FROM session WHERE account_id=?1 AND revoked=0) FROM account WHERE account_id=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
        ).unwrap()
    }

    fn event_count(&self) -> i64 {
        self.db()
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM event", [], |r| r.get(0))
            .unwrap()
    }
}

#[tokio::test]
async fn account_authority_changes_revoke_sessions_preserve_passwords_and_private_files() {
    let fixture = TestFixture::new("account-authority").await;
    fixture.seed_account(SECOND_ACCOUNT, "second", "user", true);
    fixture.db().lock().unwrap().execute(
        "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,NULL,'folder','',x'',1)",
        params![PRIVATE_ROOT,ACCOUNT_ID],
    ).unwrap();
    let bytes = b"synthetic private content";
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let store =
        sylux::persistence::payload::PayloadStore::open(&fixture.owner.path().join("product"))
            .unwrap();
    let mut staged = store
        .start(
            "1234567890abcdef1234567890abcdef-abcdef0123456789abcdef0123456789",
            bytes.len() as u64,
        )
        .unwrap();
    staged.write_chunk(bytes).unwrap();
    let published = staged.publish(digest).unwrap();
    {
        let db = fixture.db();
        let connection = db.lock().unwrap();
        let tx = connection.unchecked_transaction().unwrap();
        tx.execute(
            "INSERT INTO blob(blob_id,shard,length,sha256) VALUES(?1,?2,?3,?4)",
            params![
                published.blob_id,
                &published.blob_id[..2],
                bytes.len() as i64,
                digest.as_slice()
            ],
        )
        .unwrap();
        tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'file','private.txt',x'707269766174652e747874',1)", params![PRIVATE_FILE,ACCOUNT_ID,PRIVATE_ROOT]).unwrap();
        tx.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![PRIVATE_REVISION,PRIVATE_FILE,published.blob_id,bytes.len() as i64,digest.as_slice(),unix_now(),ACCOUNT_ID]).unwrap();
        tx.execute(
            "UPDATE entry SET current_revision_id=?1 WHERE entry_id=?2",
            params![PRIVATE_REVISION, PRIVATE_FILE],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let owner = fixture.authenticated().await;
    let original = fixture.sign_in("second").await;
    let password = fixture.account_state(SECOND_ACCOUNT).3;
    assert_eq!(
        fixture
            .account_action(&owner, SECOND_ACCOUNT, "role", Some("administrator"))
            .await
            .status,
        204
    );
    assert_eq!(fixture.me(&original.cookie).await.status, 401);
    let promoted = fixture.sign_in("second").await;
    assert_eq!(
        fixture
            .request("GET", "/accounts", Some(&promoted.cookie), None, None, None)
            .await
            .status,
        200
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/folders/{PRIVATE_ROOT}/children"),
                Some(&owner.cookie),
                None,
                None,
                None
            )
            .await
            .status,
        200
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/folders/{PRIVATE_ROOT}/children"),
                Some(&promoted.cookie),
                None,
                None,
                None
            )
            .await
            .status,
        404
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/entries/{PRIVATE_FILE}/content"),
                Some(&promoted.cookie),
                None,
                None,
                None
            )
            .await
            .status,
        404
    );

    let content = fixture
        .request(
            "GET",
            &format!("/entries/{PRIVATE_FILE}/content"),
            Some(&owner.cookie),
            None,
            None,
            None,
        )
        .await;
    assert_eq!(content.status, 200);
    assert_eq!(content.body.as_ref(), bytes);
    assert_eq!(
        fixture
            .account_action(&owner, SECOND_ACCOUNT, "disable", None)
            .await
            .status,
        204
    );
    assert_eq!(fixture.me(&promoted.cookie).await.status, 401);
    assert_eq!(
        fixture
            .login(&fixture.bootstrap().await, "second", PASSWORD)
            .await
            .status,
        401
    );
    assert_eq!(
        fixture
            .account_action(&owner, SECOND_ACCOUNT, "enable", None)
            .await
            .status,
        204
    );
    assert_eq!(fixture.me(&original.cookie).await.status, 401);
    assert_eq!(fixture.me(&promoted.cookie).await.status, 401);
    let enabled = fixture.sign_in("second").await;
    let identity: Value = serde_json::from_slice(&fixture.me(&enabled.cookie).await.body).unwrap();
    assert_eq!(identity["role"], "administrator");
    assert_eq!(
        fixture
            .account_action(&owner, SECOND_ACCOUNT, "role", Some("user"))
            .await
            .status,
        204
    );
    assert_eq!(fixture.me(&enabled.cookie).await.status, 401);
    let demoted = fixture.sign_in("second").await;
    assert_eq!(
        fixture
            .request("GET", "/accounts", Some(&demoted.cookie), None, None, None)
            .await
            .status,
        403
    );
    let (role, enabled, epoch, verifier, _) = fixture.account_state(SECOND_ACCOUNT);
    assert_eq!((role.as_str(), enabled, epoch), ("user", true, 4));
    assert_eq!(verifier, password);
    let events: Vec<(String, String, String, i64)> = fixture
        .db()
        .lock()
        .unwrap()
        .prepare("SELECT actor_account_id,action,outcome,occurred_at FROM event ORDER BY event_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        events.iter().map(|e| e.1.as_str()).collect::<Vec<_>>(),
        [
            "account.promoted",
            "account.disabled",
            "account.enabled",
            "account.demoted"
        ]
    );
    assert!(
        events
            .iter()
            .all(|e| e.0 == ACCOUNT_ID && e.2 == "ok" && e.3 > 0)
    );
    fixture.close().await;
}

#[tokio::test]
async fn account_authority_rejects_invalid_targets_states_and_self_changes_without_audit() {
    let fixture = TestFixture::new("account-invalid").await;
    fixture.seed_account(SECOND_ACCOUNT, "second", "user", true);
    fixture.seed_account(DISABLED_ACCOUNT, "disabled", "user", false);
    let owner = fixture.authenticated().await;
    for (target, action, role, status) in [
        (ACCOUNT_ID, "disable", None, 409),
        (ACCOUNT_ID, "role", Some("user"), 409),
        (SECOND_ACCOUNT, "role", Some("user"), 409),
        (SECOND_ACCOUNT, "role", Some("root"), 400),
        (SECOND_ACCOUNT, "enable", None, 409),
        (DISABLED_ACCOUNT, "disable", None, 409),
        (DISABLED_ACCOUNT, "role", Some("administrator"), 409),
        ("unknown", "disable", None, 404),
        ("unknown", "enable", None, 404),
        ("unknown", "role", Some("user"), 404),
    ] {
        assert_eq!(
            fixture
                .account_action(&owner, target, action, role)
                .await
                .status,
            status,
            "{target}/{action}"
        );
    }
    assert_eq!(fixture.event_count(), 0);
    assert_eq!(fixture.account_state(SECOND_ACCOUNT).2, 0);
    assert_eq!(fixture.account_state(DISABLED_ACCOUNT).2, 0);
    assert_eq!(fixture.account_state(ACCOUNT_ID).0, "administrator");
    assert_eq!(
        fixture
            .account_action(&owner, SECOND_ACCOUNT, "disable", None)
            .await
            .status,
        204
    );
    assert_eq!(
        fixture
            .account_action(&owner, SECOND_ACCOUNT, "disable", None)
            .await
            .status,
        409
    );
    assert_eq!(
        fixture
            .account_action(&owner, SECOND_ACCOUNT, "enable", None)
            .await
            .status,
        204
    );
    assert_eq!(
        fixture
            .account_action(&owner, SECOND_ACCOUNT, "enable", None)
            .await
            .status,
        409
    );
    assert_eq!(
        fixture
            .account_action(&owner, SECOND_ACCOUNT, "role", Some("administrator"))
            .await
            .status,
        204
    );
    assert_eq!(
        fixture
            .account_action(&owner, SECOND_ACCOUNT, "role", Some("administrator"))
            .await
            .status,
        409
    );
    assert_eq!(fixture.event_count(), 3);
    fixture.close().await;
}

#[tokio::test]
async fn account_authority_requires_admin_current_session_origin_and_csrf() {
    let fixture = TestFixture::new("account-denial").await;
    fixture.seed_account(SECOND_ACCOUNT, "second", "user", true);
    fixture.seed_account(DISABLED_ACCOUNT, "disabled", "user", false);
    let owner = fixture.authenticated().await;
    let user = fixture.sign_in("second").await;
    let expired = fixture.authenticated().await;
    fixture
        .db()
        .lock()
        .unwrap()
        .execute(
            "UPDATE session SET idle_expires_at=0 WHERE session_id=?1",
            [expired
                .cookie
                .strip_prefix("__Host-sylux=")
                .unwrap()
                .split('.')
                .next()
                .unwrap()],
        )
        .unwrap();
    let revoked = fixture.authenticated().await;
    assert_eq!(fixture.logout(&revoked).await.status, 204);
    for (target, action, role) in [
        (SECOND_ACCOUNT, "role", Some("administrator")),
        (SECOND_ACCOUNT, "disable", None),
        (DISABLED_ACCOUNT, "enable", None),
    ] {
        let path = format!("/accounts/{target}/{action}");
        let body = role.map(|role| serde_json::json!({"role":role}).to_string());
        for (cookie, origin, csrf, status) in [
            (
                Some(user.cookie.as_str()),
                Some(fixture.origin.as_str()),
                Some(user.csrf.as_str()),
                403,
            ),
            (
                Some(expired.cookie.as_str()),
                Some(fixture.origin.as_str()),
                Some(expired.csrf.as_str()),
                401,
            ),
            (
                Some(revoked.cookie.as_str()),
                Some(fixture.origin.as_str()),
                Some(revoked.csrf.as_str()),
                401,
            ),
            (
                None,
                Some(fixture.origin.as_str()),
                Some(owner.csrf.as_str()),
                401,
            ),
            (
                Some(owner.cookie.as_str()),
                None,
                Some(owner.csrf.as_str()),
                403,
            ),
            (
                Some(owner.cookie.as_str()),
                Some("https://foreign.example"),
                Some(owner.csrf.as_str()),
                403,
            ),
            (
                Some(owner.cookie.as_str()),
                Some(fixture.origin.as_str()),
                None,
                403,
            ),
            (
                Some(owner.cookie.as_str()),
                Some(fixture.origin.as_str()),
                Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
                403,
            ),
        ] {
            assert_eq!(
                fixture
                    .request("POST", &path, cookie, origin, csrf, body.clone())
                    .await
                    .status,
                status,
                "{path}"
            );
        }
    }
    assert_eq!(fixture.event_count(), 0);
    assert_eq!(fixture.account_state(SECOND_ACCOUNT).2, 0);
    assert!(!fixture.account_state(DISABLED_ACCOUNT).1);
    fixture.close().await;
}

#[tokio::test]
async fn account_authority_audit_failure_rolls_back_account_epoch_and_sessions() {
    let fixture = TestFixture::new("account-audit-failure").await;
    fixture.seed_account(SECOND_ACCOUNT, "second", "user", true);
    fixture.seed_account(DISABLED_ACCOUNT, "disabled", "user", false);
    fixture.seed_account(OTHER_ADMIN, "otheradmin", "administrator", true);
    let owner = fixture.authenticated().await;
    let second = fixture.sign_in("second").await;
    let otheradmin = fixture.sign_in("otheradmin").await;
    fixture.db().lock().unwrap().execute_batch("CREATE TRIGGER fail_security_event BEFORE INSERT ON event BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END;").unwrap();
    for (target, action, role) in [
        (SECOND_ACCOUNT, "role", Some("administrator")),
        (SECOND_ACCOUNT, "disable", None),
        (DISABLED_ACCOUNT, "enable", None),
        (OTHER_ADMIN, "role", Some("user")),
    ] {
        let before = fixture.account_state(target);
        assert_eq!(
            fixture
                .account_action(&owner, target, action, role)
                .await
                .status,
            500
        );
        assert_eq!(fixture.account_state(target), before);
        assert_eq!(fixture.event_count(), 0);
    }
    assert_eq!(fixture.me(&second.cookie).await.status, 200);
    assert_eq!(fixture.me(&otheradmin.cookie).await.status, 200);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_account_authority_changes_preserve_an_enabled_admin_and_reject_revoked_actor() {
    for (first, second) in [
        ("role", "role"),
        ("disable", "disable"),
        ("role", "disable"),
        ("disable", "role"),
    ] {
        let fixture = TestFixture::new("account-concurrent").await;
        fixture.seed_account(OTHER_ADMIN, "otheradmin", "administrator", true);
        fixture.seed_account(SECOND_ACCOUNT, "second", "user", true);
        fixture.seed_account(DISABLED_ACCOUNT, "disabled", "user", false);
        let owner = fixture.authenticated().await;
        let other = fixture.sign_in("otheradmin").await;
        let (a, b) = tokio::join!(
            fixture.account_action(
                &owner,
                OTHER_ADMIN,
                first,
                (first == "role").then_some("user")
            ),
            fixture.account_action(
                &other,
                ACCOUNT_ID,
                second,
                (second == "role").then_some("user")
            ),
        );
        let mut statuses = [a.status, b.status];
        statuses.sort();
        assert_eq!(statuses, [204, 401]);
        let count: i64 = fixture
            .db()
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM account WHERE role='administrator' AND enabled=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(fixture.event_count(), 1);
        let loser = if a.status == 204 { &other } else { &owner };
        for (target, action, role) in [
            (SECOND_ACCOUNT, "role", Some("administrator")),
            (SECOND_ACCOUNT, "disable", None),
            (DISABLED_ACCOUNT, "enable", None),
        ] {
            let before = fixture.account_state(target);
            assert_eq!(
                fixture
                    .account_action(loser, target, action, role)
                    .await
                    .status,
                401
            );
            assert_eq!(fixture.account_state(target), before);
        }
        assert_eq!(fixture.event_count(), 1);
        fixture.close().await;
    }
}

#[tokio::test]
async fn account_authority_maximum_epoch_conflicts_without_mutation_or_audit() {
    let fixture = TestFixture::new("account-maximum-epoch").await;
    fixture.seed_account(SECOND_ACCOUNT, "second", "user", true);
    fixture.seed_account(DISABLED_ACCOUNT, "disabled", "user", false);
    fixture.seed_account(OTHER_ADMIN, "otheradmin", "administrator", true);
    let owner = fixture.authenticated().await;
    let second = fixture.sign_in("second").await;
    let otheradmin = fixture.sign_in("otheradmin").await;
    {
        let db = fixture.db();
        let connection = db.lock().unwrap();
        connection
            .execute(
                "UPDATE account SET credential_epoch=?1 WHERE account_id!=?2",
                params![i64::MAX, ACCOUNT_ID],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE session SET credential_epoch=?1 WHERE account_id!=?2",
                params![i64::MAX, ACCOUNT_ID],
            )
            .unwrap();
    }
    for (target, action, role) in [
        (SECOND_ACCOUNT, "role", Some("administrator")),
        (OTHER_ADMIN, "role", Some("user")),
        (SECOND_ACCOUNT, "disable", None),
        (OTHER_ADMIN, "disable", None),
        (DISABLED_ACCOUNT, "enable", None),
    ] {
        let before = fixture.account_state(target);
        assert_eq!(before.2, i64::MAX);
        assert_eq!(
            fixture
                .account_action(&owner, target, action, role)
                .await
                .status,
            409
        );
        assert_eq!(fixture.account_state(target), before);
        assert_eq!(fixture.event_count(), 0);
        let kind: String = fixture
            .db()
            .lock()
            .unwrap()
            .query_row(
                "SELECT typeof(credential_epoch) FROM account WHERE account_id=?1",
                [target],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, "integer");
    }
    assert_eq!(fixture.me(&second.cookie).await.status, 200);
    assert_eq!(fixture.me(&otheradmin.cookie).await.status, 200);
    let fresh = fixture.sign_in("second").await;
    assert_eq!(fixture.me(&fresh.cookie).await.status, 200);
    fixture.close().await;
}
