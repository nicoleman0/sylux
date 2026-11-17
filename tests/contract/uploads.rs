//! HTTP and SQLite upload, publication and content contracts.
#[path = "../common/mod.rs"]
mod common;

use axum::body::Bytes;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use common::FixtureRoot;
use rusqlite::{Connection, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use sylux::{
    domain::authz::verifier,
    http::{self, AuthState, uploads::UploadCancelTestBarrier},
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
const OWNER_ID: &str = "00000000000000000000000000000001";
const OTHER_ID: &str = "00000000000000000000000000000002";
const OWNER_ROOT: &str = "00000000000000000000000000000101";
const OTHER_ROOT: &str = "00000000000000000000000000000102";
const OWNER_SESSION: &str = "00000000000000000000000000000201";
const OTHER_SESSION: &str = "00000000000000000000000000000202";
const OWNER_SECRET: &[u8; 32] = b"p5-owner-secret-0123456789abcdef";
const OTHER_SECRET: &[u8; 32] = b"p5-other-secret-0123456789abcdef";
const OWNER_CSRF: &[u8; 32] = b"p5-owner-csrf-0123456789abcdef!!";
const OTHER_CSRF: &[u8; 32] = b"p5-other-csrf-0123456789abcdef!!";

struct Auth {
    cookie: String,
    csrf: String,
}

fn auth(session_id: &str, secret: &[u8; 32], csrf: &[u8; 32]) -> Auth {
    Auth {
        cookie: format!(
            "__Host-sylux={session_id}.{}",
            URL_SAFE_NO_PAD.encode(secret)
        ),
        csrf: URL_SAFE_NO_PAD.encode(csrf),
    }
}

fn owner_auth() -> Auth {
    auth(OWNER_SESSION, OWNER_SECRET, OWNER_CSRF)
}

fn other_auth() -> Auth {
    auth(OTHER_SESSION, OTHER_SECRET, OTHER_CSRF)
}

struct Fixture {
    owner: FixtureRoot,
    product: PathBuf,
    db_path: PathBuf,
    db: Option<Arc<Mutex<Connection>>>,
    workers: Option<SqliteWorkers>,
    origin: String,
    address: String,
    server: Option<JoinHandle<()>>,
    cleanup: Option<sylux::persistence::cleanup::Runner>,
    cleanup_fault: Option<sylux::persistence::payload::PublishFaultHook>,
    removal_commit_hook: Option<Arc<dyn Fn(&'static str) -> rusqlite::Result<()> + Send + Sync>>,
}

impl Fixture {
    async fn new(case: &str) -> Self {
        Self::new_with_cancel_barrier(case, None).await
    }

    async fn new_with_cancel_barrier(
        case: &str,
        cancel_barrier: Option<Arc<UploadCancelTestBarrier>>,
    ) -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-p5-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "uploads", case).unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).expect("product initialization");
        let db_path = product.join("metadata").join("sylux.sqlite3");
        let connection = sqlite::open_writer(&db_path, BUSY).expect("product SQLite writer");
        seed_inventory(&connection);
        let mut fixture = Self {
            owner,
            product,
            db_path,
            db: None,
            workers: None,
            origin: String::new(),
            address: String::new(),
            server: None,
            cleanup: None,
            cleanup_fault: None,
            removal_commit_hook: None,
        };
        fixture.start(connection, cancel_barrier).await;
        fixture
    }

    async fn start(
        &mut self,
        connection: Connection,
        cancel_barrier: Option<Arc<UploadCancelTestBarrier>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.address = listener.local_addr().unwrap().to_string();
        self.origin = format!("http://{}", self.address);
        let workers = SqliteWorkers::from_writer(
            connection,
            &self.db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let db = Arc::new(Mutex::new(
            sqlite::open_writer(&self.db_path, BUSY).unwrap(),
        ));
        let mut state = AuthState::new(workers.clone(), self.origin.clone())
            .with_storage_root(&self.product)
            .expect("payload storage");
        if let Some(barrier) = cancel_barrier {
            state = state.with_upload_cancel_test_barrier(barrier);
        }
        if let Some(hook) = &self.cleanup_fault {
            Arc::get_mut(
                &mut Arc::get_mut(state.storage.as_mut().unwrap())
                    .unwrap()
                    .payloads,
            )
            .unwrap()
            .set_publish_fault_hook(Some(hook.clone()));
        }
        state.removal_commit_hook = self.removal_commit_hook.clone();
        self.cleanup = state.start_cleanup();
        let app = http::router(state);
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        self.db = Some(db);
        self.workers = Some(workers);
    }

    fn db(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(self.db.as_ref().unwrap())
    }

    async fn restart(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        if let Some(cleanup) = self.cleanup.take() {
            cleanup.shutdown().await.unwrap();
        }
        if let Some(workers) = self.workers.take() {
            workers.shutdown().await.unwrap();
        }
        self.db.take();
        let connection = sqlite::open_writer(&self.db_path, BUSY).unwrap();
        self.start(connection, None).await;
    }

    async fn close(mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        if let Some(cleanup) = self.cleanup.take() {
            cleanup.shutdown().await.unwrap();
        }
        if let Some(workers) = self.workers.take() {
            workers.shutdown().await.unwrap();
        }
        self.db.take();
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        auth: Option<&Auth>,
        extra_headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Response {
        let mut stream = TcpStream::connect(&self.address).await.unwrap();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n",
            self.address
        );
        if let Some(auth) = auth {
            request.push_str(&format!("Cookie: {}\r\n", auth.cookie));
        }
        for (name, value) in extra_headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let mut wire = request.into_bytes();
        wire.extend_from_slice(&body);
        match stream.write_all(&wire).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(error) => panic!("request write failed: {error}"),
        }
        let mut bytes = Vec::new();
        match stream.read_to_end(&mut bytes).await {
            Ok(_) => {}
            // macOS can reset a connection after a complete rejection with unread request bytes.
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {
                let response = Response::parse(&bytes);
                let length = response
                    .header("content-length")
                    .and_then(|value| value.parse::<usize>().ok());
                assert_eq!(
                    length,
                    Some(response.body.len()),
                    "incomplete response before reset"
                );
                return response;
            }
            Err(error) => panic!("response read failed: {error}"),
        }
        if bytes.is_empty() {
            panic!(
                "empty HTTP response; server_finished={}",
                self.server.as_ref().is_some_and(JoinHandle::is_finished)
            );
        }
        Response::parse(&bytes)
    }

    async fn mutate(
        &self,
        method: &str,
        path: &str,
        auth: &Auth,
        ticket: Option<&str>,
        extra_headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Response {
        let mut headers = vec![
            ("Origin", self.origin.clone()),
            ("X-CSRF-Token", auth.csrf.clone()),
        ];
        if let Some(ticket) = ticket {
            headers.push(("X-Mutation-ID", ticket.to_owned()));
        }
        headers.extend_from_slice(extra_headers);
        self.request(method, path, Some(auth), &headers, body).await
    }

    async fn ticket(
        &self,
        auth: &Auth,
        parent: &str,
        generation: u64,
        name: &str,
        bytes: &[u8],
    ) -> String {
        let response = self
            .mutate(
                "POST",
                "/mutation-tickets",
                auth,
                None,
                &[("Content-Type", "application/json".to_owned())],
                serde_json::to_vec(&serde_json::json!({
                    "scope":"upload_create",
                    "parent_id":parent,
                    "parent_etag":folder_etag(parent, generation),
                    "name":name,
                    "length":bytes.len(),
                    "sha256":hex(&Sha256::digest(bytes)),
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(response.status, 201, "{}", response.text());
        response
            .header("x-mutation-id")
            .expect("signed mutation ticket header")
            .to_owned()
    }

    async fn admit(
        &self,
        auth: &Auth,
        parent: &str,
        generation: u64,
        name: &str,
        bytes: &[u8],
    ) -> (String, String) {
        let ticket = self.ticket(auth, parent, generation, name, bytes).await;
        let digest = hex(&Sha256::digest(bytes));
        let response = self
            .mutate(
                "POST",
                "/uploads",
                auth,
                Some(&ticket),
                &[
                    ("Content-Type", "application/json".to_owned()),
                    ("If-None-Match", "*".to_owned()),
                ],
                serde_json::to_vec(&serde_json::json!({
                    "parent_id":parent,
                    "parent_etag":folder_etag(parent, generation),
                    "name":name,
                    "length":bytes.len(),
                    "sha256":digest,
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(response.status, 201, "{}", response.text());
        let upload_id = response.json()["upload_id"].as_str().unwrap().to_owned();
        (upload_id, ticket)
    }

    async fn stage(&self, name: &str, bytes: &[u8]) -> (String, String, Value) {
        let auth = owner_auth();
        let (upload_id, ticket) = self.admit(&auth, OWNER_ROOT, 1, name, bytes).await;
        let staged = self
            .mutate(
                "PUT",
                &format!("/uploads/{upload_id}/content"),
                &auth,
                Some(&ticket),
                &[("Content-Type", "application/octet-stream".to_owned())],
                bytes.to_vec(),
            )
            .await;
        assert_eq!(staged.status, 200, "{}", staged.text());
        let body = staged.json();
        assert_eq!(body["sha256"], hex(&Sha256::digest(bytes)));
        assert_eq!(body["length"], bytes.len());
        (upload_id, ticket, body)
    }

    async fn stage_without_declared_digest(
        &self,
        name: &str,
        bytes: &[u8],
    ) -> (String, String, Value) {
        let auth = owner_auth();
        let ticket = {
            let response = self
                .mutate(
                    "POST",
                    "/mutation-tickets",
                    &auth,
                    None,
                    &[("Content-Type", "application/json".to_owned())],
                    serde_json::to_vec(&serde_json::json!({
                        "scope":"upload_create",
                        "parent_id":OWNER_ROOT,
                        "parent_etag":folder_etag(OWNER_ROOT, 1),
                        "name":name,
                        "length":bytes.len(),
                        "sha256":Value::Null,
                    }))
                    .unwrap(),
                )
                .await;
            assert_eq!(response.status, 201, "{}", response.text());
            response.header("x-mutation-id").unwrap().to_owned()
        };
        let admitted = self
            .mutate(
                "POST",
                "/uploads",
                &auth,
                Some(&ticket),
                &[
                    ("Content-Type", "application/json".to_owned()),
                    ("If-None-Match", "*".to_owned()),
                ],
                serde_json::to_vec(&serde_json::json!({
                    "parent_id":OWNER_ROOT,
                    "parent_etag":folder_etag(OWNER_ROOT, 1),
                    "name":name,
                    "length":bytes.len(),
                    "sha256":Value::Null,
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(admitted.status, 201, "{}", admitted.text());
        let upload_id = admitted.json()["upload_id"].as_str().unwrap().to_owned();
        let staged = self
            .mutate(
                "PUT",
                &format!("/uploads/{upload_id}/content"),
                &auth,
                Some(&ticket),
                &[("Content-Type", "application/octet-stream".to_owned())],
                bytes.to_vec(),
            )
            .await;
        assert_eq!(staged.status, 200, "{}", staged.text());
        let body = staged.json();
        assert_eq!(body["sha256"], hex(&Sha256::digest(bytes)));
        (upload_id, ticket, body)
    }

    async fn commit_staged(&self, upload_id: &str, ticket: &str, bytes: &[u8]) -> Response {
        self.mutate(
            "POST",
            &format!("/uploads/{upload_id}/commit"),
            &owner_auth(),
            Some(ticket),
            &[
                ("Content-Type", "application/json".to_owned()),
                ("If-None-Match", "*".to_owned()),
            ],
            serde_json::to_vec(&serde_json::json!({
                "sha256":hex(&Sha256::digest(bytes)),
                "parent_etag":folder_etag(OWNER_ROOT, 1),
            }))
            .unwrap(),
        )
        .await
    }

    async fn upload(&self, name: &str, bytes: &[u8]) -> Value {
        let (upload_id, ticket, _) = self.stage(name, bytes).await;
        let committed = self.commit_staged(&upload_id, &ticket, bytes).await;
        assert_eq!(committed.status, 201, "{}", committed.text());
        committed.json()
    }
}

fn seed_inventory(connection: &Connection) {
    let now = unix_now();
    for (account, root, handle, session, secret, csrf) in [
        (
            OWNER_ID,
            OWNER_ROOT,
            "owner",
            OWNER_SESSION,
            OWNER_SECRET,
            OWNER_CSRF,
        ),
        (
            OTHER_ID,
            OTHER_ROOT,
            "other",
            OTHER_SESSION,
            OTHER_SECRET,
            OTHER_CSRF,
        ),
    ] {
        connection
            .execute(
                "INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,?2,'user',1,0)",
                params![account, handle],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)",
                params![root, account],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?4,?5,?6,?7,0)",
                params![session, account, verifier(secret).as_slice(), verifier(csrf).as_slice(), now, now + 3600, now + 7200],
            )
            .unwrap();
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn folder_etag(id: &str, generation: u64) -> String {
    format!("\"{id}:{generation}\"")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Response {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Bytes,
}

impl Response {
    fn parse(bytes: &[u8]) -> Self {
        let split = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap_or_else(|| {
                panic!(
                    "invalid HTTP response ({} bytes): {:?}",
                    bytes.len(),
                    String::from_utf8_lossy(bytes)
                )
            });
        let head = std::str::from_utf8(&bytes[..split]).unwrap();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let mut headers = BTreeMap::new();
        for line in lines {
            let (name, value) = line.split_once(':').unwrap();
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
        let raw = &bytes[split + 4..];
        let body = if headers
            .get("transfer-encoding")
            .is_some_and(|value| value == "chunked")
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

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn decode_chunked(mut bytes: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    loop {
        let end = bytes
            .windows(2)
            .position(|window| window == b"\r\n")
            .unwrap();
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

#[tokio::test]
async fn binary_upload_commits_fixed_metadata_and_downloads_exact_bytes() {
    let fixture = Fixture::new("binary-happy-path").await;
    let bytes = (0..=255).cycle().take(256 * 1024 + 17).collect::<Vec<u8>>();
    let probe = fixture
        .request("GET", "/me", Some(&owner_auth()), &[], Vec::new())
        .await;
    assert_eq!(probe.status, 200);
    let committed = fixture.upload("binary.bin", &bytes).await;
    let entry_id = committed["id"].as_str().unwrap();
    assert_eq!(committed["size"], bytes.len());
    assert_eq!(committed["length"], bytes.len());
    assert_eq!(committed["sha256"], hex(&Sha256::digest(&bytes)));
    assert_eq!(committed["kind"], "file");
    assert!(committed["revision_id"].as_str().unwrap().len() == 32);

    let downloaded = fixture
        .request(
            "GET",
            &format!("/entries/{entry_id}/content"),
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(downloaded.status, 200, "{}", downloaded.text());
    assert_eq!(downloaded.body.as_ref(), bytes);
    assert_eq!(
        downloaded.header("content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(downloaded.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(downloaded.header("cache-control"), Some("no-store"));
    assert!(
        downloaded
            .header("content-disposition")
            .unwrap()
            .starts_with("attachment;")
    );
    assert_eq!(downloaded.header("etag"), committed["etag"].as_str());
    fixture.close().await;
}

#[tokio::test]
async fn staged_progress_is_not_publication_and_admission_is_create_only() {
    let fixture = Fixture::new("staged-create-only").await;
    let auth = owner_auth();
    let bytes = b"body complete is not committed";
    let (upload_id, ticket, staged) = fixture.stage("staged.bin", bytes).await;

    // A successful body response reports completed staging, not publication.
    assert_eq!(staged["length"], bytes.len());
    assert_eq!(staged["sha256"], hex(&Sha256::digest(bytes)));
    let visible_before_commit: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM entry WHERE kind='file'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(visible_before_commit, 0, "completed body is not published");

    let duplicate = fixture
        .mutate(
            "POST",
            "/uploads",
            &auth,
            Some(&ticket),
            &[
                ("Content-Type", "application/json".to_owned()),
                ("If-None-Match", "*".to_owned()),
            ],
            serde_json::to_vec(&serde_json::json!({
                "parent_id":OWNER_ROOT,
                "parent_etag":folder_etag(OWNER_ROOT, 1),
                "name":"staged.bin",
                "length":bytes.len(),
                "sha256":hex(&Sha256::digest(bytes)),
            }))
            .unwrap(),
        )
        .await;
    assert_eq!(duplicate.status, 200, "{}", duplicate.text());
    assert_eq!(duplicate.json()["upload_id"], upload_id);

    let changed = fixture
        .mutate(
            "POST",
            "/uploads",
            &auth,
            Some(&ticket),
            &[
                ("Content-Type", "application/json".to_owned()),
                ("If-None-Match", "*".to_owned()),
            ],
            serde_json::to_vec(&serde_json::json!({
                "parent_id":OWNER_ROOT,
                "parent_etag":folder_etag(OWNER_ROOT, 1),
                "name":"different.bin",
                "length":bytes.len(),
                "sha256":hex(&Sha256::digest(bytes)),
            }))
            .unwrap(),
        )
        .await;
    assert_eq!(changed.status, 409, "{}", changed.text());
    let committed = fixture.commit_staged(&upload_id, &ticket, bytes).await;
    assert_eq!(committed.status, 201, "{}", committed.text());
    fixture.close().await;
}

#[tokio::test]
async fn stale_parent_etag_and_existing_sibling_never_admit_replacement() {
    let fixture = Fixture::new("stale-parent-collision").await;
    let existing = fixture.upload("occupied.bin", b"original").await;
    assert_eq!(existing["size"], 8);

    let auth = owner_auth();
    let bytes = b"replacement";
    let stale = fixture
        .mutate(
            "POST",
            "/mutation-tickets",
            &auth,
            None,
            &[("Content-Type", "application/json".to_owned())],
            serde_json::to_vec(&serde_json::json!({
                "scope":"upload_create",
                "parent_id":OWNER_ROOT,
                "parent_etag":folder_etag(OWNER_ROOT, 1),
                "name":"new.bin",
                "length":bytes.len(),
                "sha256":hex(&Sha256::digest(bytes)),
            }))
            .unwrap(),
        )
        .await;
    assert_eq!(stale.status, 201, "{}", stale.text());
    let stale_ticket = stale.header("x-mutation-id").unwrap().to_owned();
    let stale_admission = fixture
        .mutate(
            "POST",
            "/uploads",
            &auth,
            Some(&stale_ticket),
            &[
                ("Content-Type", "application/json".to_owned()),
                ("If-None-Match", "*".to_owned()),
            ],
            serde_json::to_vec(&serde_json::json!({
                "parent_id":OWNER_ROOT,
                "parent_etag":folder_etag(OWNER_ROOT, 1),
                "name":"new.bin",
                "length":bytes.len(),
                "sha256":hex(&Sha256::digest(bytes)),
            }))
            .unwrap(),
        )
        .await;
    assert_eq!(stale_admission.status, 412, "{}", stale_admission.text());

    let current_ticket = fixture
        .ticket(&auth, OWNER_ROOT, 2, "occupied.bin", bytes)
        .await;
    let collision = fixture
        .mutate(
            "POST",
            "/uploads",
            &auth,
            Some(&current_ticket),
            &[
                ("Content-Type", "application/json".to_owned()),
                ("If-None-Match", "*".to_owned()),
            ],
            serde_json::to_vec(&serde_json::json!({
                "parent_id":OWNER_ROOT,
                "parent_etag":folder_etag(OWNER_ROOT, 2),
                "name":"occupied.bin",
                "length":bytes.len(),
                "sha256":hex(&Sha256::digest(bytes)),
            }))
            .unwrap(),
        )
        .await;
    assert_eq!(collision.status, 409, "{}", collision.text());
    let (entry_count, stored_name): (i64, String) = fixture
        .db()
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*), min(original_name) FROM entry WHERE kind='file'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(entry_count, 1);
    assert_eq!(stored_name, "occupied.bin");
    fixture.close().await;
}

#[tokio::test]
async fn second_writer_is_excluded_and_completed_body_cannot_be_restaged() {
    let fixture = Fixture::new("second-writer").await;
    let auth = owner_auth();
    let bytes = (0..=255)
        .cycle()
        .take(4 * 1024 * 1024 + 19)
        .collect::<Vec<u8>>();
    let (upload_id, ticket) = fixture
        .admit(&auth, OWNER_ROOT, 1, "exclusive.bin", &bytes)
        .await;
    let path = format!("/uploads/{upload_id}/content");
    let content_headers = [("Content-Type", "application/octet-stream".to_owned())];
    let first = fixture.mutate(
        "PUT",
        &path,
        &auth,
        Some(&ticket),
        &content_headers,
        bytes.clone(),
    );
    let second = fixture.mutate(
        "PUT",
        &path,
        &auth,
        Some(&ticket),
        &content_headers,
        bytes.clone(),
    );
    let (first, second) = tokio::join!(first, second);
    let statuses = [first.status, second.status];
    assert!(statuses.contains(&200), "{statuses:?}");
    assert!(statuses.contains(&409), "{statuses:?}");

    let changed = vec![0x5a; bytes.len()];
    let restage = fixture
        .mutate(
            "PUT",
            &path,
            &auth,
            Some(&ticket),
            &[("Content-Type", "application/octet-stream".to_owned())],
            changed,
        )
        .await;
    assert_eq!(restage.status, 409);
    fixture.close().await;
}

#[tokio::test]
async fn cancellation_joins_the_active_worker_and_hides_all_bytes() {
    let fixture = Fixture::new("cancel-active").await;
    let auth = owner_auth();
    let bytes = vec![0x42; 8 * 1024 * 1024];
    let (upload_id, ticket) = fixture
        .admit(&auth, OWNER_ROOT, 1, "cancel.bin", &bytes)
        .await;
    let address = fixture.address.clone();
    let origin = fixture.origin.clone();
    let client_auth = owner_auth();
    let client_ticket = ticket.clone();
    let client_upload = upload_id.clone();
    let writer = tokio::spawn(async move {
        let mut stream = TcpStream::connect(&address).await.unwrap();
        let request = format!(
            "PUT /uploads/{client_upload}/content HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nCookie: {}\r\nOrigin: {origin}\r\nX-CSRF-Token: {}\r\nX-Mutation-ID: {client_ticket}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
            client_auth.cookie,
            client_auth.csrf,
            bytes.len(),
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(&[0x42; 1024]).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        Response::parse(&response)
    });
    let mut active = false;
    for _ in 0..100 {
        active = fixture.db().lock().unwrap().query_row(
            "SELECT writer_active FROM upload_attempt WHERE mutation_id=?1 ORDER BY attempt_no DESC LIMIT 1",
            [upload_id.as_str()],
            |row| row.get::<_,i64>(0),
        ).unwrap() == 1;
        if active {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(active, "body worker did not reach its active barrier");

    let canceled = fixture
        .mutate(
            "DELETE",
            &format!("/uploads/{upload_id}"),
            &auth,
            None,
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(canceled.status, 204, "{}", canceled.text());
    let writer = tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(writer.status, 409);
    let (state, entries): (String, i64) = fixture.db().lock().unwrap().query_row(
        "SELECT mutation.state,(SELECT count(*) FROM entry WHERE kind='file') FROM mutation WHERE mutation_id=?1",
        [upload_id.as_str()],
        |row| Ok((row.get(0)?,row.get(1)?)),
    ).unwrap();
    assert_eq!(state, "rejected");
    assert_eq!(entries, 0);
    fixture.close().await;
}

#[tokio::test]
async fn cancellation_serializes_against_a_writer_starting_after_no_writer_was_seen() {
    let barrier = Arc::new(UploadCancelTestBarrier::default());
    let fixture =
        Fixture::new_with_cancel_barrier("cancel-before-writer", Some(Arc::clone(&barrier))).await;
    let auth = owner_auth();
    let bytes = b"writer must not stage after cancellation".to_vec();
    let (upload_id, ticket) = fixture
        .admit(&auth, OWNER_ROOT, 1, "cancel-before-writer.bin", &bytes)
        .await;

    let cancel_path = format!("/uploads/{upload_id}");
    let cancel = fixture.mutate("DELETE", &cancel_path, &auth, None, &[], Vec::new());
    let mut cancel = Box::pin(cancel);
    tokio::select! {
        result = &mut cancel => panic!("cancel returned before deterministic barrier: {}", result.status),
        _ = async {
            tokio::time::timeout(Duration::from_secs(5), barrier.wait_cancel_paused())
                .await
                .expect("cancel barrier timed out");
        } => {}
    }

    let put_path = format!("/uploads/{upload_id}/content");
    let content_headers = [("Content-Type", "application/octet-stream".to_owned())];
    let put = fixture.mutate(
        "PUT",
        &put_path,
        &auth,
        Some(&ticket),
        &content_headers,
        bytes,
    );
    let mut put = Box::pin(put);
    let put_response = tokio::time::timeout(Duration::from_secs(2), &mut put)
        .await
        .expect("PUT did not resolve while cancellation was paused");
    assert_eq!(
        put_response.status,
        409,
        "writer must be rejected after cancellation wins: {}",
        put_response.text()
    );
    drop(put);

    barrier.release_cancel();
    let canceled = cancel.as_mut().await;
    assert_eq!(canceled.status, 204, "{}", canceled.text());
    drop(cancel);
    let (state, entries): (String, i64) = fixture.db().lock().unwrap().query_row(
        "SELECT mutation.state,(SELECT count(*) FROM entry WHERE kind='file') FROM mutation WHERE mutation_id=?1",
        [upload_id.as_str()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(state, "rejected");
    assert_eq!(entries, 0);
    fixture.close().await;
}

#[tokio::test]
async fn cross_account_cancel_does_not_interrupt_an_active_upload() {
    let fixture = Fixture::new("cross-account-cancel").await;
    let bytes = vec![0x37; 256 * 1024];
    let (upload_id, ticket) = fixture
        .admit(&owner_auth(), OWNER_ROOT, 1, "private.bin", &bytes)
        .await;
    let address = fixture.address.clone();
    let origin = fixture.origin.clone();
    let client_auth = owner_auth();
    let client_ticket = ticket.clone();
    let client_upload = upload_id.clone();
    let writer_bytes = bytes.clone();
    let content_length = bytes.len();
    let writer = tokio::spawn(async move {
        let mut stream = TcpStream::connect(&address).await.unwrap();
        let crlf = "\r\n";
        let request = format!(
            "PUT /uploads/{client_upload}/content HTTP/1.1{crlf}Host: {address}{crlf}Connection: close{crlf}Cookie: {}{crlf}Origin: {origin}{crlf}X-CSRF-Token: {}{crlf}X-Mutation-ID: {client_ticket}{crlf}Content-Type: application/octet-stream{crlf}Content-Length: {content_length}{crlf}{crlf}",
            client_auth.cookie, client_auth.csrf,
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(&writer_bytes[..1024]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = stream.write_all(&writer_bytes[1024..]).await;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        Response::parse(&response)
    });
    let mut active = false;
    for _ in 0..100 {
        active = fixture.db().lock().unwrap().query_row(
            "SELECT writer_active FROM upload_attempt WHERE mutation_id=?1 ORDER BY attempt_no DESC LIMIT 1",
            [upload_id.as_str()],
            |row| row.get::<_,i64>(0),
        ).unwrap() == 1;
        if active {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if !active {
        let response = tokio::time::timeout(Duration::from_secs(2), writer)
            .await
            .unwrap()
            .unwrap();
        panic!(
            "body worker did not reach its active barrier; PUT returned {}: {}",
            response.status,
            response.text()
        );
    }

    let denied = fixture
        .mutate(
            "DELETE",
            &format!("/uploads/{upload_id}"),
            &other_auth(),
            None,
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(denied.status, 404, "foreign uploads are undiscoverable");
    let completed = tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        completed.status, 200,
        "foreign cancellation must not stop writer"
    );
    let committed = fixture.commit_staged(&upload_id, &ticket, &bytes).await;
    assert_eq!(committed.status, 201, "{}", committed.text());
    fixture.close().await;
}

#[tokio::test]
async fn restart_preserves_acknowledged_inventory_and_exact_bytes() {
    let mut fixture = Fixture::new("restart-invariance").await;
    let bytes = (0..=255).cycle().take(512 * 1024 + 7).collect::<Vec<u8>>();
    let committed = fixture.upload("restart.bin", &bytes).await;
    let entry_id = committed["id"].as_str().unwrap().to_owned();
    let before = fixture.db().lock().unwrap().query_row(
        "SELECT entry.entry_id,revision.revision_id,blob.blob_id,blob.length,blob.sha256 FROM entry JOIN revision ON revision.revision_id=entry.current_revision_id JOIN blob ON blob.blob_id=revision.blob_id WHERE entry.entry_id=?1",
        [entry_id.as_str()],
        |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,i64>(3)?,row.get::<_,Vec<u8>>(4)?)),
    ).unwrap();
    fixture.restart().await;
    let after = fixture.db().lock().unwrap().query_row(
        "SELECT entry.entry_id,revision.revision_id,blob.blob_id,blob.length,blob.sha256 FROM entry JOIN revision ON revision.revision_id=entry.current_revision_id JOIN blob ON blob.blob_id=revision.blob_id WHERE entry.entry_id=?1",
        [entry_id.as_str()],
        |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,i64>(3)?,row.get::<_,Vec<u8>>(4)?)),
    ).unwrap();
    assert_eq!(before, after);
    let downloaded = fixture
        .request(
            "GET",
            &format!("/entries/{entry_id}/content"),
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(downloaded.status, 200);
    assert_eq!(downloaded.body.as_ref(), bytes);
    let status = fixture
        .request(
            "GET",
            &format!("/uploads/{}", committed["upload_id"].as_str().unwrap()),
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(status.status, 200);
    assert_eq!(status.json()["status"], "committed");
    assert_eq!(status.json()["length"], bytes.len());
    assert_eq!(status.json()["sha256"], hex(&Sha256::digest(&bytes)));
    fixture.close().await;
}

#[tokio::test]
async fn authorized_head_ranges_conditionals_and_cross_account_denial_are_exact() {
    let fixture = Fixture::new("content-semantics").await;
    let bytes = b"<script>window.mustNotRun=true</script>\0\xff".to_vec();
    let committed = fixture.upload("café 雪 active.html", &bytes).await;
    let entry_id = committed["id"].as_str().unwrap();
    let path = format!("/entries/{entry_id}/content");
    let etag = committed["etag"].as_str().unwrap().to_owned();

    for (method, headers) in [
        ("GET", vec![]),
        ("HEAD", vec![]),
        ("GET", vec![("Range", "bytes=0-3".to_owned())]),
    ] {
        let denied = fixture
            .request(method, &path, Some(&other_auth()), &headers, Vec::new())
            .await;
        assert_eq!(denied.status, 404, "cross-account {method}");
    }
    let head = fixture
        .request("HEAD", &path, Some(&owner_auth()), &[], Vec::new())
        .await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(
        head.header("content-length"),
        Some(bytes.len().to_string().as_str())
    );
    assert_eq!(head.header("referrer-policy"), Some("no-referrer"));
    assert!(
        head.header("content-disposition")
            .unwrap()
            .contains("filename*=UTF-8''")
    );

    let range = fixture
        .request(
            "GET",
            &path,
            Some(&owner_auth()),
            &[("Range", "bytes=2-6".to_owned())],
            Vec::new(),
        )
        .await;
    assert_eq!(range.status, 206);
    assert_eq!(range.body.as_ref(), &bytes[2..=6]);
    assert_eq!(
        range.header("content-range"),
        Some(format!("bytes 2-6/{}", bytes.len()).as_str())
    );
    let suffix = fixture
        .request(
            "GET",
            &path,
            Some(&owner_auth()),
            &[("Range", "bytes=-5".to_owned())],
            Vec::new(),
        )
        .await;
    assert_eq!(suffix.status, 206);
    assert_eq!(suffix.body.as_ref(), &bytes[bytes.len() - 5..]);
    let not_modified = fixture
        .request(
            "GET",
            &path,
            Some(&owner_auth()),
            &[("If-None-Match", etag.clone())],
            Vec::new(),
        )
        .await;
    assert_eq!(not_modified.status, 304);
    assert!(not_modified.body.is_empty());
    assert_eq!(not_modified.header("cache-control"), Some("no-store"));
    for method in ["GET", "HEAD"] {
        let weak = fixture
            .request(
                method,
                &path,
                Some(&owner_auth()),
                &[("If-None-Match", format!("W/{etag}"))],
                Vec::new(),
            )
            .await;
        assert_eq!(weak.status, 304, "weak If-None-Match on {method}");
        assert!(weak.body.is_empty());
    }
    let stale_if_range = fixture
        .request(
            "GET",
            &path,
            Some(&owner_auth()),
            &[
                ("Range", "bytes=0-0".to_owned()),
                ("If-Range", "\"stale\"".to_owned()),
            ],
            Vec::new(),
        )
        .await;
    assert_eq!(stale_if_range.status, 200);
    assert_eq!(stale_if_range.body.as_ref(), bytes);
    let invalid = fixture
        .request(
            "GET",
            &path,
            Some(&owner_auth()),
            &[(
                "Range",
                format!("bytes={}-{}", bytes.len(), bytes.len() + 1),
            )],
            Vec::new(),
        )
        .await;
    assert_eq!(invalid.status, 416);
    assert_eq!(
        invalid.header("content-range"),
        Some(format!("bytes */{}", bytes.len()).as_str())
    );
    let malformed_multi = fixture
        .request(
            "GET",
            &path,
            Some(&owner_auth()),
            &[("Range", "bytes=0-1,garbage".to_owned())],
            Vec::new(),
        )
        .await;
    assert_eq!(malformed_multi.status, 416);
    fixture.close().await;
}

#[tokio::test]
async fn revoked_session_cannot_commit_staged_bytes() {
    let fixture = Fixture::new("revoke-before-commit").await;
    let bytes = b"staged before logout".to_vec();
    let (upload_id, ticket, _) = fixture.stage("revoked.bin", &bytes).await;
    let logout = fixture
        .mutate("POST", "/auth/logout", &owner_auth(), None, &[], Vec::new())
        .await;
    assert_eq!(logout.status, 204);
    let committed = fixture.commit_staged(&upload_id, &ticket, &bytes).await;
    assert_eq!(committed.status, 401);
    let entries: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM entry WHERE kind='file'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(entries, 0);
    fixture.close().await;
}

#[tokio::test]
async fn cancellation_after_commit_never_undoes_the_entry_or_bytes() {
    let fixture = Fixture::new("commit-before-cancel").await;
    let bytes = b"committed before cancellation".to_vec();
    let committed = fixture.upload("committed.bin", &bytes).await;
    let upload_id = committed["upload_id"].as_str().unwrap();
    let canceled = fixture
        .mutate(
            "DELETE",
            &format!("/uploads/{upload_id}"),
            &owner_auth(),
            None,
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(canceled.status, 409);
    assert_eq!(
        canceled.header("location"),
        Some(format!("/uploads/{upload_id}").as_str())
    );
    let downloaded = fixture
        .request(
            "GET",
            &format!("/entries/{}/content", committed["id"].as_str().unwrap()),
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(downloaded.status, 200);
    assert_eq!(downloaded.body.as_ref(), bytes);
    fixture.close().await;
}

#[tokio::test]
async fn missing_referenced_payload_is_an_integrity_error_not_empty_success() {
    let fixture = Fixture::new("missing-payload").await;
    let committed = fixture.upload("missing.bin", b"referenced bytes").await;
    let entry_id = committed["id"].as_str().unwrap();
    let blob_id: String = fixture
        .db()
        .lock()
        .unwrap()
        .query_row(
            "SELECT blob_id FROM revision WHERE revision_id=?1",
            [committed["revision_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    let payload = fixture
        .product
        .join("payloads")
        .join(&blob_id[..2])
        .join(&blob_id);
    fs::remove_file(payload).unwrap();
    for method in ["GET", "HEAD"] {
        let response = fixture
            .request(
                method,
                &format!("/entries/{entry_id}/content"),
                Some(&owner_auth()),
                &[],
                Vec::new(),
            )
            .await;
        assert_eq!(response.status, 500, "{method}: {}", response.text());
        if method == "HEAD" {
            assert!(response.body.is_empty());
        }
        if method == "GET" {
            assert_eq!(response.json()["code"], "content_integrity_error");
        }
    }
    fixture.close().await;
}

#[tokio::test]
async fn empty_unicode_and_multi_megabyte_payloads_round_trip_exactly() {
    let cases = vec![
        ("empty", "empty.bin", Vec::new(), false),
        (
            "unicode",
            "cafe\u{301} 雪.txt",
            "Snowman ☃ — café\n".repeat(4096).into_bytes(),
            true,
        ),
        (
            "multi-mb",
            "large.bin",
            (0..=255)
                .cycle()
                .take(5 * 1024 * 1024 + 31)
                .collect::<Vec<u8>>(),
            true,
        ),
    ];
    for (case, name, bytes, declare_digest) in cases {
        let fixture = Fixture::new(case).await;
        let committed = if declare_digest {
            let (upload_id, ticket, _) = fixture.stage(name, &bytes).await;
            let committed = fixture.commit_staged(&upload_id, &ticket, &bytes).await;
            assert_eq!(committed.status, 201, "{}", committed.text());
            committed.json()
        } else {
            let (upload_id, ticket, _) = fixture.stage_without_declared_digest(name, &bytes).await;
            let committed = fixture.commit_staged(&upload_id, &ticket, &bytes).await;
            assert_eq!(committed.status, 201, "{}", committed.text());
            committed.json()
        };
        let entry_id = committed["id"].as_str().unwrap();
        let downloaded = fixture
            .request(
                "GET",
                &format!("/entries/{entry_id}/content"),
                Some(&owner_auth()),
                &[],
                Vec::new(),
            )
            .await;
        assert_eq!(downloaded.status, 200, "case {case}");
        assert_eq!(downloaded.body.as_ref(), bytes, "case {case}");
        let (length, digest): (i64, Vec<u8>) = fixture.db().lock().unwrap().query_row(
            "SELECT length,sha256 FROM blob WHERE blob_id=(SELECT blob_id FROM revision WHERE revision_id=?1)",
            [committed["revision_id"].as_str().unwrap()],
            |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(length, bytes.len() as i64);
        assert_eq!(digest, Sha256::digest(&bytes).as_slice());
        fixture.close().await;
    }
}

#[tokio::test]
async fn declared_length_and_client_digest_are_checked_before_byte_zero_retry() {
    let fixture = Fixture::new("checked-retry").await;
    let auth = owner_auth();
    let bytes = (0..=255).cycle().take(128 * 1024 + 3).collect::<Vec<u8>>();
    let (upload_id, ticket) = fixture
        .admit(&auth, OWNER_ROOT, 1, "retry.bin", &bytes)
        .await;
    let path = format!("/uploads/{upload_id}/content");
    let short = fixture
        .mutate(
            "PUT",
            &path,
            &auth,
            Some(&ticket),
            &[("Content-Type", "application/octet-stream".to_owned())],
            bytes[..bytes.len() - 1].to_vec(),
        )
        .await;
    assert_eq!(short.status, 422);
    let wrong_digest = fixture
        .mutate(
            "PUT",
            &path,
            &auth,
            Some(&ticket),
            &[("Content-Type", "application/octet-stream".to_owned())],
            vec![0x5a; bytes.len()],
        )
        .await;
    assert_eq!(wrong_digest.status, 422);
    let retry = fixture
        .mutate(
            "PUT",
            &path,
            &auth,
            Some(&ticket),
            &[("Content-Type", "application/octet-stream".to_owned())],
            bytes.clone(),
        )
        .await;
    assert_eq!(retry.status, 200, "{}", retry.text());
    assert_eq!(retry.json()["sha256"], hex(&Sha256::digest(&bytes)));
    let attempts: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM upload_attempt WHERE mutation_id=?1",
            [upload_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attempts, 2);
    let committed = fixture.commit_staged(&upload_id, &ticket, &bytes).await;
    assert_eq!(committed.status, 201, "{}", committed.text());
    fixture.close().await;
}

impl Fixture {
    async fn replacement(&self, existing: &Value, bytes: &[u8]) -> (String, String, Value) {
        let parent = existing["parent_id"].as_str().unwrap();
        let parent_generation: i64 = self
            .db()
            .lock()
            .unwrap()
            .query_row(
                "SELECT generation FROM entry WHERE entry_id=?1",
                [parent],
                |row| row.get(0),
            )
            .unwrap();
        let id = existing["id"].as_str().unwrap();
        let generation: i64 = self
            .db()
            .lock()
            .unwrap()
            .query_row(
                "SELECT generation FROM entry WHERE entry_id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        let admission = serde_json::json!({
            "parent_id": parent, "parent_etag": folder_etag(parent, parent_generation as u64),
            "name": existing["name"], "length": bytes.len(), "sha256": hex(&Sha256::digest(bytes)),
            "replacement": { "entry_id": id, "etag": folder_etag(id, generation as u64) },
        });
        let mut ticket_body = admission.clone();
        ticket_body["scope"] = "upload_create".into();
        let issued = self
            .mutate(
                "POST",
                "/mutation-tickets",
                &owner_auth(),
                None,
                &[("Content-Type", "application/json".to_owned())],
                serde_json::to_vec(&ticket_body).unwrap(),
            )
            .await;
        assert_eq!(issued.status, 201, "{}", issued.text());
        let ticket = issued.header("x-mutation-id").unwrap().to_owned();
        let id = issued.json()["mutation_id"].as_str().unwrap().to_owned();
        let missing_precondition = self
            .mutate(
                "POST",
                "/uploads",
                &owner_auth(),
                Some(&ticket),
                &[
                    ("Content-Type", "application/json".to_owned()),
                    ("If-None-Match", "*".to_owned()),
                ],
                serde_json::to_vec(&admission).unwrap(),
            )
            .await;
        assert_eq!(missing_precondition.status, 428);
        let admitted = self
            .mutate(
                "POST",
                "/uploads",
                &owner_auth(),
                Some(&ticket),
                &[
                    ("Content-Type", "application/json".to_owned()),
                    (
                        "If-Match",
                        admission["replacement"]["etag"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                    ),
                ],
                serde_json::to_vec(&admission).unwrap(),
            )
            .await;
        assert_eq!(admitted.status, 201, "{}", admitted.text());
        (id, ticket, admission)
    }

    async fn replacement_content(&self, id: &str, ticket: &str, bytes: &[u8]) -> Response {
        self.mutate(
            "PUT",
            &format!("/uploads/{id}/content"),
            &owner_auth(),
            Some(ticket),
            &[("Content-Type", "application/octet-stream".to_owned())],
            bytes.to_vec(),
        )
        .await
    }

    async fn replacement_commit(&self, id: &str, ticket: &str, admission: &Value) -> Response {
        self.mutate("POST", &format!("/uploads/{id}/commit"), &owner_auth(), Some(ticket),
            &[("Content-Type", "application/json".to_owned()), ("If-Match", admission["replacement"]["etag"].as_str().unwrap().to_owned())],
            serde_json::to_vec(&serde_json::json!({ "sha256": admission["sha256"], "parent_etag": admission["parent_etag"] })).unwrap()).await
    }

    async fn assert_content(&self, entry: &Value, expected: &[u8]) {
        let response = self
            .request(
                "GET",
                &format!("/entries/{}/content", entry["id"].as_str().unwrap()),
                Some(&owner_auth()),
                &[],
                Vec::new(),
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.text());
        assert_eq!(response.body.as_ref(), expected);
    }
}

#[tokio::test]
async fn explicit_replacement_preserves_prior_bytes_in_deleted_items_and_restores_them() {
    let fixture = Fixture::new("replacement-restore").await;
    let old_bytes = b"old bytes\x00\xff\r\n";
    let new_bytes = b"new bytes\x00\xfe";
    let old = fixture.upload("same.bin", old_bytes).await;
    let old_time = assert_listing_modified_at(&fixture, &old).await;
    let middle_bytes = b"middle bytes";
    let (middle_id, middle_ticket) = fixture
        .admit(&owner_auth(), OWNER_ROOT, 2, "middle.bin", middle_bytes)
        .await;
    let staged = fixture
        .mutate(
            "PUT",
            &format!("/uploads/{middle_id}/content"),
            &owner_auth(),
            Some(&middle_ticket),
            &[("Content-Type", "application/octet-stream".to_owned())],
            middle_bytes.to_vec(),
        )
        .await;
    assert_eq!(staged.status, 200, "{}", staged.text());
    let middle = fixture
        .mutate(
            "POST",
            &format!("/uploads/{middle_id}/commit"),
            &owner_auth(),
            Some(&middle_ticket),
            &[
                ("Content-Type", "application/json".to_owned()),
                ("If-None-Match", "*".to_owned()),
            ],
            serde_json::to_vec(&serde_json::json!({
                "sha256": hex(&Sha256::digest(middle_bytes)),
                "parent_etag": folder_etag(OWNER_ROOT, 2),
            }))
            .unwrap(),
        )
        .await;
    assert_eq!(middle.status, 201, "{}", middle.text());
    let middle = middle.json();
    let before = fixture
        .request(
            "GET",
            "/folders/children?sort=size_asc&limit=1",
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(before.status, 200, "{}", before.text());
    let before = before.json();
    assert_eq!(before["items"][0]["id"], middle["id"]);
    let cursor = before["next_cursor"].as_str().unwrap();
    let (id, ticket, admission) = fixture.replacement(&old, new_bytes).await;
    assert_eq!(
        fixture
            .replacement_content(&id, &ticket, new_bytes)
            .await
            .status,
        200
    );
    fixture.assert_content(&old, old_bytes).await;
    let committed = fixture.replacement_commit(&id, &ticket, &admission).await;
    assert_eq!(committed.status, 201, "{}", committed.text());
    let new = committed.json();
    let new_time = assert_listing_modified_at(&fixture, &new).await;
    assert_eq!(new["replaced_entry_id"], old["id"]);
    let stale = fixture
        .request(
            "GET",
            &format!("/folders/children?sort=size_asc&limit=1&cursor={cursor}"),
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(stale.status, 409, "{}", stale.text());
    assert_eq!(stale.json()["code"], "listing_changed");
    let restarted = fixture
        .request(
            "GET",
            "/folders/children?sort=size_asc",
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(restarted.status, 200, "{}", restarted.text());
    let restarted = restarted.json();
    assert_eq!(restarted["items"][0]["id"], new["id"]);
    assert_eq!(restarted["items"][0]["size"], new_bytes.len());
    assert_eq!(restarted["items"][1]["id"], middle["id"]);
    fixture.assert_content(&new, new_bytes).await;
    let deleted = fixture
        .request("GET", "/trash", Some(&owner_auth()), &[], Vec::new())
        .await;
    assert_eq!(deleted.status, 200);
    let prior = &deleted.json()["items"][0];
    assert_eq!(prior["id"], old["id"]);
    let collision = fixture
        .mutate(
            "POST",
            &format!("/trash/{}/restore", old["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[("If-Match", prior["etag"].as_str().unwrap().to_owned())],
            Vec::new(),
        )
        .await;
    assert_eq!(collision.status, 409);
    let renamed = fixture
        .mutate(
            "PATCH",
            &format!("/entries/{}", new["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[
                ("If-Match", folder_etag(new["id"].as_str().unwrap(), 1)),
                ("Content-Type", "application/json".to_owned()),
            ],
            br#"{"name":"replacement.bin"}"#.to_vec(),
        )
        .await;
    assert_eq!(renamed.status, 200, "{}", renamed.text());
    let restored = fixture
        .mutate(
            "POST",
            &format!("/trash/{}/restore", old["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[("If-Match", prior["etag"].as_str().unwrap().to_owned())],
            Vec::new(),
        )
        .await;
    assert_eq!(restored.status, 204, "{}", restored.text());
    fixture.assert_content(&old, old_bytes).await;
    fixture.assert_content(&new, new_bytes).await;
    assert_eq!(assert_listing_modified_at(&fixture, &old).await, old_time);
    assert_eq!(assert_listing_modified_at(&fixture, &new).await, new_time);
    fixture.close().await;
}

#[tokio::test]
async fn replacement_failure_cancellation_and_restart_keep_the_prior_content() {
    let mut fixture = Fixture::new("replacement-interrupted").await;
    let old = fixture.upload("same.bin", b"prior content").await;
    let (id, ticket, _) = fixture.replacement(&old, b"expected bytes").await;
    let invalid = fixture
        .replacement_content(&id, &ticket, b"incorrect byte")
        .await;
    assert_ne!(invalid.status, 200);
    fixture.assert_content(&old, b"prior content").await;
    let canceled = fixture
        .mutate(
            "DELETE",
            &format!("/uploads/{id}"),
            &owner_auth(),
            Some(&ticket),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(canceled.status, 204);
    fixture.assert_content(&old, b"prior content").await;
    let (id, ticket, admission) = fixture.replacement(&old, b"new bytes").await;
    assert_eq!(
        fixture
            .replacement_content(&id, &ticket, b"new bytes")
            .await
            .status,
        200
    );
    fixture.restart().await;
    fixture.assert_content(&old, b"prior content").await;
    fixture.db().lock().unwrap().execute_batch("CREATE TRIGGER fail_replacement_revision BEFORE INSERT ON revision BEGIN SELECT RAISE(ABORT, 'injected commit failure'); END;").unwrap();
    let failed = fixture.replacement_commit(&id, &ticket, &admission).await;
    assert_eq!(failed.status, 500, "{}", failed.text());
    fixture.assert_content(&old, b"prior content").await;
    let trashed: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM entry WHERE state='deleted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        trashed, 0,
        "failed transaction must roll back prior-file deletion"
    );
    fixture
        .db()
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_replacement_revision")
        .unwrap();
    let committed = fixture.replacement_commit(&id, &ticket, &admission).await;
    assert_eq!(committed.status, 201, "{}", committed.text());
    fixture
        .assert_content(&committed.json(), b"new bytes")
        .await;
    fixture.close().await;
}

#[tokio::test]
async fn simultaneous_replacements_cannot_replace_a_different_file() {
    let fixture = Fixture::new("replacement-race").await;
    let old = fixture.upload("same.bin", b"prior content").await;
    let (first, first_ticket, first_admission) =
        fixture.replacement(&old, b"first replacement").await;
    let (second, second_ticket, second_admission) =
        fixture.replacement(&old, b"second replacement").await;
    assert_eq!(
        fixture
            .replacement_content(&first, &first_ticket, b"first replacement")
            .await
            .status,
        200
    );
    assert_eq!(
        fixture
            .replacement_content(&second, &second_ticket, b"second replacement")
            .await
            .status,
        200
    );
    let committed = fixture
        .replacement_commit(&first, &first_ticket, &first_admission)
        .await;
    assert_eq!(committed.status, 201, "{}", committed.text());
    let stale = fixture
        .replacement_commit(&second, &second_ticket, &second_admission)
        .await;
    assert_eq!(stale.status, 412, "{}", stale.text());
    fixture
        .assert_content(&committed.json(), b"first replacement")
        .await;
    let deleted: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM entry WHERE state='deleted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deleted, 1);
    fixture.close().await;
}

#[tokio::test]
async fn replacement_revalidates_move_delete_and_target_generation() {
    for action in ["move", "delete", "generation"] {
        let fixture = Fixture::new(&format!("replacement-{action}-race")).await;
        let old = fixture.upload("same.bin", b"prior content").await;
        let folder = fixture
            .mutate(
                "POST",
                "/folders/children",
                &owner_auth(),
                None,
                &[("Content-Type", "application/json".to_owned())],
                serde_json::to_vec(
                    &serde_json::json!({"parent_id": OWNER_ROOT, "name": "destination"}),
                )
                .unwrap(),
            )
            .await;
        assert_eq!(folder.status, 201);
        let (id, ticket, admission) = fixture.replacement(&old, b"new bytes").await;
        assert_eq!(
            fixture
                .replacement_content(&id, &ticket, b"new bytes")
                .await
                .status,
            200
        );
        let old_id = old["id"].as_str().unwrap();
        if action == "generation" {
            fixture
                .db()
                .lock()
                .unwrap()
                .execute(
                    "UPDATE entry SET generation=generation+1 WHERE entry_id=?1",
                    [old_id],
                )
                .unwrap();
        } else if action == "move" {
            // Simulate a concurrent move independently of the move HTTP route.
            fixture
                .db()
                .lock()
                .unwrap()
                .execute(
                    "UPDATE entry SET parent_id=?1,generation=generation+1 WHERE entry_id=?2",
                    params![folder.json()["id"].as_str().unwrap(), old_id],
                )
                .unwrap();
        } else {
            let response = fixture
                .mutate(
                    "DELETE",
                    &format!("/entries/{old_id}"),
                    &owner_auth(),
                    None,
                    &[("If-Match", folder_etag(old_id, 1))],
                    Vec::new(),
                )
                .await;
            assert_eq!(response.status, 204, "{}", response.text());
        }
        let stale = fixture.replacement_commit(&id, &ticket, &admission).await;
        assert_eq!(stale.status, 412, "{}", stale.text());
        if action != "delete" {
            fixture.assert_content(&old, b"prior content").await;
        }
        let new_revisions: i64 = fixture
            .db()
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM revision", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            new_revisions, 1,
            "stale replacement must not create a revision"
        );
        fixture.close().await;
    }
}

#[tokio::test]
async fn folder_total_tracks_real_upload_replacement_trash_and_restore() {
    let fixture = Fixture::new("folder-size-replacement").await;
    let old_bytes = b"original bytes";
    let old = fixture.upload("same.bin", old_bytes).await;
    let folder = fixture
        .mutate(
            "POST",
            "/folders/children",
            &owner_auth(),
            None,
            &[("Content-Type", "application/json".to_owned())],
            serde_json::to_vec(&serde_json::json!({"parent_id": OWNER_ROOT, "name": "Archive"}))
                .unwrap(),
        )
        .await;
    assert_eq!(folder.status, 201, "{}", folder.text());
    let folder = folder.json();
    let moved = fixture
        .mutate(
            "POST",
            &format!("/entries/{}/move", old["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[
                ("Content-Type", "application/json".to_owned()),
                ("If-Match", folder_etag(old["id"].as_str().unwrap(), 1)),
            ],
            serde_json::to_vec(&serde_json::json!({"parent_id": folder["id"]})).unwrap(),
        )
        .await;
    assert_eq!(moved.status, 200, "{}", moved.text());
    assert_folder_total(&fixture, &folder, old_bytes.len()).await;
    let new_bytes = b"replacement";
    let (id, ticket, admission) = fixture.replacement(&moved.json(), new_bytes).await;
    assert_eq!(
        fixture
            .replacement_content(&id, &ticket, new_bytes)
            .await
            .status,
        200
    );
    assert_folder_total(&fixture, &folder, old_bytes.len()).await;
    let committed = fixture.replacement_commit(&id, &ticket, &admission).await;
    assert_eq!(committed.status, 201, "{}", committed.text());
    let new = committed.json();
    assert_folder_total(&fixture, &folder, new_bytes.len()).await;
    let trashed = fixture
        .mutate(
            "DELETE",
            &format!("/entries/{}", new["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[("If-Match", folder_etag(new["id"].as_str().unwrap(), 1))],
            Vec::new(),
        )
        .await;
    assert_eq!(trashed.status, 204, "{}", trashed.text());
    assert_folder_total(&fixture, &folder, 0).await;
    let deleted = fixture
        .request("GET", "/trash", Some(&owner_auth()), &[], Vec::new())
        .await;
    assert_eq!(deleted.status, 200);
    let deleted = deleted.json();
    let prior = deleted["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == old["id"])
        .unwrap();
    let restored = fixture
        .mutate(
            "POST",
            &format!("/trash/{}/restore", old["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[("If-Match", prior["etag"].as_str().unwrap().to_owned())],
            Vec::new(),
        )
        .await;
    assert_eq!(restored.status, 204, "{}", restored.text());
    assert_folder_total(&fixture, &folder, old_bytes.len()).await;
    fixture.close().await;
}

async fn assert_folder_total(fixture: &Fixture, folder: &Value, expected: usize) {
    let listing = fixture
        .request(
            "GET",
            "/folders/children",
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(listing.status, 200, "{}", listing.text());
    let listing = listing.json();
    let item = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == folder["id"])
        .unwrap();
    assert_eq!(item["size"], expected);
}

async fn assert_listing_modified_at(fixture: &Fixture, entry: &Value) -> i64 {
    let expected: i64 = fixture.db().lock().unwrap().query_row(
        "SELECT r.committed_at FROM entry e JOIN revision r ON r.revision_id=e.current_revision_id WHERE e.entry_id=?1",
        [entry["id"].as_str().unwrap()], |row| row.get(0),
    ).unwrap();
    let listing = fixture
        .request(
            "GET",
            "/folders/children?sort=modified_desc",
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(listing.status, 200, "{}", listing.text());
    let listing = listing.json();
    let item = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == entry["id"])
        .unwrap();
    assert_eq!(item["modified_at"], expected);
    expected
}

#[tokio::test]
async fn folder_recovery_blocks_fresh_admission_and_body_attempts_but_keeps_status_and_cancel() {
    let fixture = Fixture::new("tree-admission-body").await;
    let auth = owner_auth();
    let create = |parent: &str, name: &str| {
        serde_json::to_vec(&serde_json::json!({"parent_id":parent,"name":name})).unwrap()
    };
    let top = fixture
        .mutate(
            "POST",
            "/folders/children",
            &auth,
            None,
            &[("Content-Type", "application/json".into())],
            create(OWNER_ROOT, "Tree"),
        )
        .await
        .json();
    let top_id = top["id"].as_str().unwrap();
    let nested = fixture
        .mutate(
            "POST",
            "/folders/children",
            &auth,
            None,
            &[("Content-Type", "application/json".into())],
            create(top_id, "Nested"),
        )
        .await
        .json();
    let nested_id = nested["id"].as_str().unwrap();
    let bytes = b"never read hidden body";
    let (upload_id, ticket) = fixture.admit(&auth, nested_id, 1, "new.bin", bytes).await;
    let top_etag = folder_etag(top_id, 2);
    let trash = fixture
        .mutate(
            "DELETE",
            &format!("/entries/{top_id}"),
            &auth,
            None,
            &[("If-Match", top_etag)],
            vec![],
        )
        .await;
    assert_eq!(trash.status, 204, "{}", trash.text());
    let body = fixture
        .mutate(
            "PUT",
            &format!("/uploads/{upload_id}/content"),
            &auth,
            Some(&ticket),
            &[("Content-Type", "application/octet-stream".into())],
            bytes.to_vec(),
        )
        .await;
    assert_eq!(body.status, 404, "{}", body.text());
    let fresh_ticket = fixture
        .ticket(&auth, nested_id, 1, "fresh.bin", bytes)
        .await;
    let admission = fixture.mutate("POST", "/uploads", &auth, Some(&fresh_ticket), &[("Content-Type", "application/json".into()), ("If-None-Match", "*".into())], serde_json::to_vec(&serde_json::json!({"parent_id":nested_id,"parent_etag":folder_etag(nested_id,1),"name":"fresh.bin","length":bytes.len(),"sha256":hex(&Sha256::digest(bytes))})).unwrap()).await;
    assert_eq!(admission.status, 404, "{}", admission.text());
    let status = fixture
        .request(
            "GET",
            &format!("/uploads/{upload_id}"),
            Some(&auth),
            &[],
            vec![],
        )
        .await;
    assert_eq!(status.status, 200);
    assert_eq!(status.json()["writer_active"], false);
    assert_eq!(status.json()["bytes_received"], 0);
    let restore = fixture
        .mutate(
            "POST",
            &format!("/trash/{top_id}/restore"),
            &auth,
            None,
            &[("If-Match", folder_etag(top_id, 3))],
            vec![],
        )
        .await;
    assert_eq!(restore.status, 204, "{}", restore.text());
    let body = fixture
        .mutate(
            "PUT",
            &format!("/uploads/{upload_id}/content"),
            &auth,
            Some(&ticket),
            &[("Content-Type", "application/octet-stream".into())],
            bytes.to_vec(),
        )
        .await;
    assert_eq!(body.status, 412, "{}", body.text());
    let canceled = fixture
        .mutate(
            "DELETE",
            &format!("/uploads/{upload_id}"),
            &auth,
            Some(&ticket),
            &[],
            vec![],
        )
        .await;
    assert_eq!(canceled.status, 204, "{}", canceled.text());
    fixture.close().await;
}

#[tokio::test]
async fn folder_recovery_keeps_an_active_body_writer_bookkeeping_recoverable() {
    let mut fixture = Fixture::new("tree-active-writer").await;
    let auth = owner_auth();
    let create = |parent: &str, name: &str| {
        serde_json::to_vec(&serde_json::json!({"parent_id":parent,"name":name})).unwrap()
    };
    let top = fixture
        .mutate(
            "POST",
            "/folders/children",
            &auth,
            None,
            &[("Content-Type", "application/json".into())],
            create(OWNER_ROOT, "Tree"),
        )
        .await
        .json();
    let top_id = top["id"].as_str().unwrap();
    let nested = fixture
        .mutate(
            "POST",
            "/folders/children",
            &auth,
            None,
            &[("Content-Type", "application/json".into())],
            create(top_id, "Nested"),
        )
        .await
        .json();
    let nested_id = nested["id"].as_str().unwrap();
    let bytes = vec![0x83; 128 * 1024];
    let (upload_id, ticket) = fixture
        .admit(&auth, nested_id, 1, "stream.bin", &bytes)
        .await;
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let address = fixture.address.clone();
    let origin = fixture.origin.clone();
    let writer_auth = owner_auth();
    let writer_id = upload_id.clone();
    let writer_ticket = ticket.clone();
    let writer_bytes = bytes.clone();
    let writer = tokio::spawn(async move {
        let mut stream = TcpStream::connect(&address).await.unwrap();
        let request = format!(
            "PUT /uploads/{writer_id}/content HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nCookie: {}\r\nOrigin: {origin}\r\nX-CSRF-Token: {}\r\nX-Mutation-ID: {writer_ticket}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
            writer_auth.cookie,
            writer_auth.csrf,
            writer_bytes.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(&writer_bytes[..1024]).await.unwrap();
        release_rx.await.unwrap();
        stream.write_all(&writer_bytes[1024..]).await.unwrap();
        let mut response = vec![];
        stream.read_to_end(&mut response).await.unwrap();
        Response::parse(&response)
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let id = upload_id.clone();
            let active = fixture.workers.as_ref().unwrap().read(move |db| db.query_row("SELECT writer_active FROM upload_attempt WHERE mutation_id=?1 ORDER BY attempt_no DESC LIMIT 1", [id], |row| row.get::<_, i64>(0))).await.unwrap();
            if active == 1 { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("stream did not become active");
    let deleted = fixture
        .mutate(
            "DELETE",
            &format!("/entries/{top_id}"),
            &auth,
            None,
            &[("If-Match", folder_etag(top_id, 2))],
            vec![],
        )
        .await;
    assert_eq!(deleted.status, 204, "{}", deleted.text());
    release_tx.send(()).unwrap();
    let staged = tokio::time::timeout(Duration::from_secs(5), writer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(staged.status, 200, "{}", staged.text());
    let status_path = format!("/uploads/{upload_id}");
    let status = fixture
        .request("GET", &status_path, Some(&auth), &[], vec![])
        .await
        .json();
    assert_eq!(status["status"], "staged");
    assert_eq!(status["writer_active"], false);
    assert_eq!(status["bytes_received"], bytes.len());
    let server = fixture.server.take().unwrap();
    server.abort();
    let _ = server.await;
    fixture.workers.take().unwrap().shutdown().await.unwrap();
    fixture.db.take();
    let connection = sqlite::open_writer(&fixture.db_path, BUSY).unwrap();
    let workers = SqliteWorkers::from_writer(
        connection,
        &fixture.db_path,
        WorkerConfig {
            busy_timeout: BUSY,
            ..WorkerConfig::default()
        },
    )
    .unwrap();
    let report = sylux::persistence::reconcile::reconcile(&workers, &fixture.product)
        .await
        .unwrap();
    assert!(report.ready, "{report:?}");
    workers.shutdown().await.unwrap();
    fixture
        .start(sqlite::open_writer(&fixture.db_path, BUSY).unwrap(), None)
        .await;
    let status = fixture
        .request("GET", &status_path, Some(&auth), &[], vec![])
        .await
        .json();
    assert_eq!(status["status"], "staged");
    assert_eq!(status["writer_active"], false);
    let body = serde_json::to_vec(&serde_json::json!({"sha256":hex(&Sha256::digest(&bytes)),"parent_etag":folder_etag(nested_id,1)})).unwrap();
    let commit_path = format!("/uploads/{upload_id}/commit");
    let headers = [
        ("Content-Type", "application/json".into()),
        ("If-None-Match", "*".into()),
    ];
    let commit = fixture
        .mutate(
            "POST",
            &commit_path,
            &auth,
            Some(&ticket),
            &headers,
            body.clone(),
        )
        .await;
    assert_eq!(commit.status, 404, "{}", commit.text());
    let restore = fixture
        .mutate(
            "POST",
            &format!("/trash/{top_id}/restore"),
            &auth,
            None,
            &[("If-Match", folder_etag(top_id, 3))],
            vec![],
        )
        .await;
    assert_eq!(restore.status, 204, "{}", restore.text());
    let commit = fixture
        .mutate("POST", &commit_path, &auth, Some(&ticket), &headers, body)
        .await;
    assert_eq!(commit.status, 412, "{}", commit.text());
    let canceled = fixture
        .mutate("DELETE", &status_path, &auth, Some(&ticket), &[], vec![])
        .await;
    assert_eq!(canceled.status, 204, "{}", canceled.text());
    let canceled_status = fixture
        .request("GET", &status_path, Some(&auth), &[], vec![])
        .await
        .json();
    assert_eq!(canceled_status["status"], "canceled");
    assert_eq!(canceled_status["writer_active"], false);
    let count: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM entry WHERE kind='file'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
    fixture.close().await;
}

#[tokio::test]
async fn folder_recovery_accepts_fresh_create_and_replacement_with_exact_bytes() {
    let fixture = Fixture::new("tree-fresh-uploads").await;
    let auth = owner_auth();
    let create = |parent: &str, name: &str| {
        serde_json::to_vec(&serde_json::json!({"parent_id":parent,"name":name})).unwrap()
    };
    let top = fixture
        .mutate(
            "POST",
            "/folders/children",
            &auth,
            None,
            &[("Content-Type", "application/json".into())],
            create(OWNER_ROOT, "Tree"),
        )
        .await
        .json();
    let top_id = top["id"].as_str().unwrap();
    let nested = fixture
        .mutate(
            "POST",
            "/folders/children",
            &auth,
            None,
            &[("Content-Type", "application/json".into())],
            create(top_id, "Nested"),
        )
        .await
        .json();
    let nested_id = nested["id"].as_str().unwrap();
    assert_eq!(
        fixture
            .mutate(
                "DELETE",
                &format!("/entries/{top_id}"),
                &auth,
                None,
                &[("If-Match", folder_etag(top_id, 2))],
                vec![]
            )
            .await
            .status,
        204
    );
    assert_eq!(
        fixture
            .mutate(
                "POST",
                &format!("/trash/{top_id}/restore"),
                &auth,
                None,
                &[("If-Match", folder_etag(top_id, 3))],
                vec![]
            )
            .await
            .status,
        204
    );
    let path = fixture
        .request(
            "GET",
            &format!("/folders/{nested_id}/path"),
            Some(&auth),
            &[],
            vec![],
        )
        .await
        .json();
    let parent_etag = path["items"].as_array().unwrap().last().unwrap()["etag"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(parent_etag, folder_etag(nested_id, 2));
    let bytes = b"fresh restored create\x00\xff";
    let (id, ticket) = fixture.admit(&auth, nested_id, 2, "fresh.bin", bytes).await;
    assert_eq!(
        fixture
            .replacement_content(&id, &ticket, bytes)
            .await
            .status,
        200
    );
    let commit = fixture.mutate("POST", &format!("/uploads/{id}/commit"), &auth, Some(&ticket), &[("Content-Type", "application/json".into()), ("If-None-Match", "*".into())], serde_json::to_vec(&serde_json::json!({"sha256":hex(&Sha256::digest(bytes)),"parent_etag":parent_etag})).unwrap()).await;
    assert_eq!(commit.status, 201, "{}", commit.text());
    let created = commit.json();
    fixture.assert_content(&created, bytes).await;
    let listing = fixture
        .request(
            "GET",
            &format!("/folders/{nested_id}/children"),
            Some(&auth),
            &[],
            vec![],
        )
        .await
        .json();
    let target = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == created["id"])
        .unwrap();
    let replacement_bytes = b"fresh restored replacement\x00\xfe";
    let (id, ticket, admission) = fixture.replacement(target, replacement_bytes).await;
    assert_eq!(admission["replacement"]["etag"], target["etag"]);
    assert_eq!(admission["parent_etag"], folder_etag(nested_id, 3));
    assert_eq!(
        admission["replacement"]["etag"],
        folder_etag(created["id"].as_str().unwrap(), 1)
    );
    assert_eq!(
        fixture
            .replacement_content(&id, &ticket, replacement_bytes)
            .await
            .status,
        200
    );
    let replacement = fixture.replacement_commit(&id, &ticket, &admission).await;
    assert_eq!(replacement.status, 201, "{}", replacement.text());
    fixture
        .assert_content(&replacement.json(), replacement_bytes)
        .await;
    let old = fixture
        .request(
            "GET",
            &format!("/entries/{}/content", created["id"].as_str().unwrap()),
            Some(&auth),
            &[],
            vec![],
        )
        .await;
    assert_eq!(old.status, 404);
    fixture.close().await;
}

impl Fixture {
    async fn upload_current(&self, name: &str, bytes: &[u8]) -> Value {
        let generation: i64 = self
            .db()
            .lock()
            .unwrap()
            .query_row(
                "SELECT generation FROM entry WHERE entry_id=?1",
                [OWNER_ROOT],
                |r| r.get(0),
            )
            .unwrap();
        let (id, ticket) = self
            .admit(&owner_auth(), OWNER_ROOT, generation as u64, name, bytes)
            .await;
        let staged = self
            .mutate(
                "PUT",
                &format!("/uploads/{id}/content"),
                &owner_auth(),
                Some(&ticket),
                &[("Content-Type", "application/octet-stream".into())],
                bytes.to_vec(),
            )
            .await;
        assert_eq!(staged.status, 200, "{}", staged.text());
        let committed = self.mutate("POST", &format!("/uploads/{id}/commit"), &owner_auth(), Some(&ticket), &[("Content-Type", "application/json".into()), ("If-None-Match", "*".into())], serde_json::to_vec(&serde_json::json!({"sha256":hex(&Sha256::digest(bytes)),"parent_etag":folder_etag(OWNER_ROOT,generation as u64)})).unwrap()).await;
        assert_eq!(committed.status, 201, "{}", committed.text());
        committed.json()
    }
    async fn wait_cleanup(&self, operation: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let response = self
                    .request(
                        "GET",
                        &format!("/trash/removals/{operation}"),
                        Some(&owner_auth()),
                        &[],
                        Vec::new(),
                    )
                    .await;
                if response.json()["cleanup"] == "complete" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("runtime cleanup must finish without restart or another removal");
    }
    async fn removal_json(&self, path: &str, body: Value) -> Response {
        self.mutate(
            "POST",
            path,
            &owner_auth(),
            None,
            &[("Content-Type", "application/json".into())],
            serde_json::to_vec(&body).unwrap(),
        )
        .await
    }
    async fn trash_uploaded(&self, entry: &Value) -> Value {
        let response = self
            .mutate(
                "DELETE",
                &format!("/entries/{}", entry["entry_id"].as_str().unwrap()),
                &owner_auth(),
                None,
                &[(
                    "If-Match",
                    folder_etag(entry["entry_id"].as_str().unwrap(), 1),
                )],
                Vec::new(),
            )
            .await;
        assert_eq!(response.status, 204, "{}", response.text());
        let listing = self
            .request("GET", "/trash", Some(&owner_auth()), &[], Vec::new())
            .await
            .json();
        listing["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == entry["entry_id"])
            .unwrap()
            .clone()
    }
}

#[tokio::test]
async fn permanent_removal_replays_original_result_and_preserves_upload_reconciliation() {
    let mut fixture = Fixture::new("permanent-removal").await;
    let (upload_id, ticket, _) = fixture.stage("remove.txt", b"remove these bytes").await;
    let response = fixture
        .commit_staged(&upload_id, &ticket, b"remove these bytes")
        .await;
    assert_eq!(response.status, 201, "{}", response.text());
    let uploaded = response.json();
    let deleted = fixture.trash_uploaded(&uploaded).await;
    let preview=fixture.removal_json("/trash/removals/preview",serde_json::json!({"selection":{"kind":"selected","items":[{"id":deleted["id"],"etag":deleted["etag"]}]}})).await;
    assert_eq!(preview.status, 200, "{}", preview.text());
    assert_eq!(preview.json()["count"], 1);
    let operation = "a0000000000000000000000000000078";
    let body =
        serde_json::json!({"operation_id":operation,"confirmation":preview.json()["confirmation"]});
    let removed = fixture.removal_json("/trash/removals", body.clone()).await;
    assert_eq!(removed.status, 200, "{}", removed.text());
    assert_eq!(removed.json()["state"], "committed");
    assert_eq!(removed.json()["cleanup"], "pending");
    fixture.wait_cleanup(operation).await;
    let replay = fixture.removal_json("/trash/removals", body.clone()).await;
    assert_eq!(replay.json()["state"], "committed");
    assert_eq!(replay.json()["count"], removed.json()["count"]);
    assert_eq!(replay.json()["cleanup"], "complete");
    let listing = fixture
        .request("GET", "/trash", Some(&owner_auth()), &[], Vec::new())
        .await;
    assert_eq!(listing.json()["items"].as_array().unwrap().len(), 0);
    let restore = fixture
        .mutate(
            "POST",
            &format!("/trash/{}/restore", deleted["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[("If-Match", deleted["etag"].as_str().unwrap().into())],
            Vec::new(),
        )
        .await;
    assert_eq!(restore.status, 404, "{}", restore.text());
    let forbidden = fixture
        .request(
            "GET",
            &format!("/entries/{}/content", deleted["id"].as_str().unwrap()),
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(forbidden.status, 404);
    let foreign = fixture
        .request(
            "GET",
            &format!("/trash/removals/{operation}"),
            Some(&other_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(foreign.json()["state"], "unknown");
    fixture.restart().await;
    let recovery = sylux::persistence::reconcile::reconcile(
        fixture.workers.as_ref().unwrap(),
        &fixture.product,
    )
    .await
    .unwrap();
    assert!(recovery.ready, "{recovery:?}");
    let replay = fixture.removal_json("/trash/removals", body).await;
    assert_eq!(replay.json()["state"], "committed");
    assert_eq!(replay.json()["count"], removed.json()["count"]);
    assert_eq!(replay.json()["cleanup"], "complete");
    let upload_status = fixture
        .request(
            "GET",
            &format!("/uploads/{upload_id}"),
            Some(&owner_auth()),
            &[("X-Mutation-ID", ticket)],
            Vec::new(),
        )
        .await;
    assert_eq!(upload_status.status, 200, "{}", upload_status.text());
    assert_eq!(upload_status.json()["state"], "committed");
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_rejects_changed_namespace_and_unconfirmed_requests() {
    let fixture = Fixture::new("permanent-removal-stale").await;
    let uploaded = fixture.upload("first.txt", b"first").await;
    fixture.trash_uploaded(&uploaded).await;
    let preview = fixture
        .removal_json(
            "/trash/removals/preview",
            serde_json::json!({"selection":{"kind":"all"}}),
        )
        .await;
    assert_eq!(preview.status, 200, "{}", preview.text());
    let foreign=fixture.mutate("POST","/trash/removals",&other_auth(),None,&[("Content-Type","application/json".into())],serde_json::to_vec(&serde_json::json!({"operation_id":"b0000000000000000000000000000078","confirmation":preview.json()["confirmation"]})).unwrap()).await;
    assert_eq!(foreign.status, 400, "{}", foreign.text());
    let missing_csrf = fixture
        .request(
            "POST",
            "/trash/removals/preview",
            Some(&owner_auth()),
            &[
                ("Content-Type", "application/json".into()),
                ("Origin", fixture.origin.clone()),
            ],
            serde_json::to_vec(&serde_json::json!({"selection":{"kind":"all"}})).unwrap(),
        )
        .await;
    assert_eq!(missing_csrf.status, 403, "{}", missing_csrf.text());
    let changed = fixture
        .removal_json(
            "/folders/children",
            serde_json::json!({"parent_id":OWNER_ROOT,"name":"changed"}),
        )
        .await;
    assert_eq!(changed.status, 201, "{}", changed.text());
    let unrelated = fixture.removal_json("/trash/removals",serde_json::json!({"operation_id":"b0000000000000000000000000000078","confirmation":preview.json()["confirmation"]})).await;
    assert_eq!(
        unrelated.status,
        200,
        "unrelated active folder must not invalidate Trash: {}",
        unrelated.text()
    );
    let next = fixture.upload_current("next.txt", b"next").await;
    fixture.trash_uploaded(&next).await;
    let preview = fixture
        .removal_json(
            "/trash/removals/preview",
            serde_json::json!({"selection":{"kind":"all"}}),
        )
        .await;
    let changed = fixture.upload_current("changed.txt", b"changed").await;
    fixture.trash_uploaded(&changed).await;
    let stale=fixture.removal_json("/trash/removals",serde_json::json!({"operation_id":"b1000000000000000000000000000078","confirmation":preview.json()["confirmation"]})).await;
    assert_eq!(stale.status, 409, "{}", stale.text());
    let forged=fixture.removal_json("/trash/removals",serde_json::json!({"operation_id":"b2000000000000000000000000000078","confirmation":"unconfirmed"})).await;
    assert_eq!(forged.status, 400);
    let count: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM removal", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_resumes_each_authorized_cleanup_boundary() {
    use sylux::{
        domain::removal::{self, Selection},
        persistence::{
            cleanup,
            payload::{PayloadError, PayloadStore},
        },
    };
    for boundary in [
        "cleanup.before_unlink",
        "cleanup.before_directory_sync",
        "cleanup.before_finalize",
    ] {
        let mut fixture = Fixture::new(&boundary.replace(['.', '_'], "-")).await;
        let active = fixture
            .upload("active.txt", b"active bytes survive every cleanup boundary")
            .await;
        let uploaded = fixture
            .upload_current("boundary.txt", b"owned cleanup bytes")
            .await;
        fixture.cleanup.take().unwrap().shutdown().await.unwrap();
        fixture.trash_uploaded(&uploaded).await;
        let workers = fixture.workers.as_ref().unwrap();
        workers
            .transact(|tx| {
                let preview =
                    removal::preview(tx, OWNER_ID, Selection::All, &[8; 32], unix_now()).unwrap();
                removal::execute(
                    tx,
                    OWNER_ID,
                    "c0000000000000000000000000000078",
                    &preview.confirmation,
                    &[8; 32],
                    unix_now(),
                )
                .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        {
            let db = fixture.db();
            let db = db.lock().unwrap();
            db.execute_batch(&format!("SAVEPOINT retained_reference; INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES('30000000000000000000000000000078','{OTHER_ID}','{OTHER_ROOT}','file','new.txt',x'6e65772e747874',1)")).unwrap();
            let inserted=db.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) SELECT '40000000000000000000000000000078','30000000000000000000000000000078',blob_id,length,sha256,1,?1 FROM blob",[OTHER_ID]);
            assert!(
                inserted.is_err(),
                "deleting blobs must reject new retained references"
            );
            db.execute_batch("ROLLBACK TO retained_reference; RELEASE retained_reference")
                .unwrap();
            assert!(
                db.execute(
                    "UPDATE entry SET state='active' WHERE entry_id=?1",
                    [uploaded["id"].as_str().unwrap()]
                )
                .is_err(),
                "removed entries cannot reactivate"
            );
        }
        let mut store = PayloadStore::open(&fixture.product).unwrap();
        store.set_publish_fault_hook(Some(Arc::new(move |at| {
            if at == boundary {
                Err(PayloadError::Io("injected_cleanup_fault"))
            } else {
                Ok(())
            }
        })));
        let store = Arc::new(store);
        assert!(cleanup::resume(workers, &store).await.is_err());
        let status = workers
            .read(|db| {
                Ok(removal::status(db, OWNER_ID, "c0000000000000000000000000000078").unwrap())
            })
            .await
            .unwrap();
        assert_eq!(status.cleanup, Some("pending"));
        drop(store);
        fixture.restart().await;
        let workers = fixture.workers.as_ref().unwrap();
        let recovery = sylux::persistence::reconcile::reconcile(workers, &fixture.product)
            .await
            .unwrap();
        assert!(recovery.ready, "{boundary}: {recovery:?}");
        let status = workers
            .read(|db| {
                Ok(removal::status(db, OWNER_ID, "c0000000000000000000000000000078").unwrap())
            })
            .await
            .unwrap();
        assert_eq!(status.cleanup, Some("complete"));
        let content = fixture
            .request(
                "GET",
                &format!("/entries/{}/content", active["entry_id"].as_str().unwrap()),
                Some(&owner_auth()),
                &[],
                Vec::new(),
            )
            .await;
        assert_eq!(content.status, 200);
        assert_eq!(
            content.body.as_ref(),
            b"active bytes survive every cleanup boundary"
        );
        fixture.close().await;
    }
}

#[tokio::test]
async fn permanent_removal_keeps_payload_referenced_by_another_accounts_revision() {
    let mut fixture = Fixture::new("removal-shared-payload").await;
    let bytes = b"shared immutable payload";
    let uploaded = fixture.upload("shared.txt", bytes).await;
    let entry_id = uploaded["entry_id"].as_str().unwrap();
    fixture.db().lock().unwrap().execute_batch(&format!("BEGIN;
        INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES('d0000000000000000000000000000078','{OTHER_ID}','{OTHER_ROOT}','file','retained.txt',x'72657461696e65642e747874',1);
        INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) SELECT 'e0000000000000000000000000000078','d0000000000000000000000000000078',blob_id,length,sha256,committed_at,'{OTHER_ID}' FROM revision WHERE entry_id='{entry_id}';
        UPDATE entry SET current_revision_id='e0000000000000000000000000000078' WHERE entry_id='d0000000000000000000000000000078'; COMMIT;")).unwrap();
    fixture.trash_uploaded(&uploaded).await;
    let preview = fixture
        .removal_json(
            "/trash/removals/preview",
            serde_json::json!({"selection":{"kind":"all"}}),
        )
        .await;
    let removed=fixture.removal_json("/trash/removals",serde_json::json!({"operation_id":"f0000000000000000000000000000078","confirmation":preview.json()["confirmation"]})).await;
    assert_eq!(removed.status, 200, "{}", removed.text());
    let count: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM removal_cleanup", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let retained = fixture
        .request(
            "GET",
            "/entries/d0000000000000000000000000000078/content",
            Some(&other_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(retained.status, 200, "{}", retained.text());
    assert_eq!(retained.body.as_ref(), bytes);
    fixture.restart().await;
    let retained_after_restart = fixture
        .request(
            "GET",
            "/entries/d0000000000000000000000000000078/content",
            Some(&other_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(retained_after_restart.status, 200);
    assert_eq!(retained_after_restart.body.as_ref(), bytes);
    let recovery = sylux::persistence::reconcile::reconcile(
        fixture.workers.as_ref().unwrap(),
        &fixture.product,
    )
    .await
    .unwrap();
    assert!(recovery.ready, "{recovery:?}");
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_of_folder_preserves_independently_trashed_child_and_history() {
    let fixture = Fixture::new("removal-folder").await;
    let uploaded = fixture
        .upload("nested.txt", b"hidden active descendant")
        .await;
    let top = fixture
        .removal_json(
            "/folders/children",
            serde_json::json!({"parent_id":OWNER_ROOT,"name":"top"}),
        )
        .await
        .json();
    let top_id = top["id"].as_str().unwrap();
    let earlier = fixture
        .removal_json(
            "/folders/children",
            serde_json::json!({"parent_id":top_id,"name":"earlier"}),
        )
        .await
        .json();
    let earlier_id = earlier["id"].as_str().unwrap();
    let moved = fixture
        .mutate(
            "POST",
            &format!("/entries/{}/move", uploaded["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[
                ("Content-Type", "application/json".into()),
                ("If-Match", folder_etag(uploaded["id"].as_str().unwrap(), 1)),
            ],
            serde_json::to_vec(&serde_json::json!({"parent_id":top_id})).unwrap(),
        )
        .await;
    assert_eq!(moved.status, 200, "{}", moved.text());
    for id in [earlier_id, top_id] {
        let generation: i64 = fixture
            .db()
            .lock()
            .unwrap()
            .query_row(
                "SELECT generation FROM entry WHERE entry_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        let trashed = fixture
            .mutate(
                "DELETE",
                &format!("/entries/{id}"),
                &owner_auth(),
                None,
                &[("If-Match", folder_etag(id, generation as u64))],
                Vec::new(),
            )
            .await;
        assert_eq!(trashed.status, 204, "{}", trashed.text());
    }
    let listing = fixture
        .request("GET", "/trash", Some(&owner_auth()), &[], Vec::new())
        .await
        .json();
    let selected = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == top_id)
        .unwrap();
    let preview=fixture.removal_json("/trash/removals/preview",serde_json::json!({"selection":{"kind":"selected","items":[{"id":top_id,"etag":selected["etag"]}]}})).await;
    assert_eq!(preview.status, 200, "{}", preview.text());
    let removed=fixture.removal_json("/trash/removals",serde_json::json!({"operation_id":"10000000000000000000000000000078","confirmation":preview.json()["confirmation"]})).await;
    assert_eq!(removed.status, 200, "{}", removed.text());
    let hidden = fixture
        .request(
            "GET",
            &format!(
                "/entries/{}/content",
                uploaded["entry_id"].as_str().unwrap()
            ),
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(hidden.status, 404);
    let search = fixture
        .request(
            "GET",
            "/entries/search?q=nested",
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(search.status, 200, "{}", search.text());
    assert!(search.json()["items"].as_array().unwrap().is_empty());
    let archive = fixture
        .removal_json(
            "/archives",
            serde_json::json!({"entry_ids":[uploaded["entry_id"]]}),
        )
        .await;
    assert_eq!(archive.status, 404, "{}", archive.text());
    let listing = fixture
        .request("GET", "/trash", Some(&owner_auth()), &[], Vec::new())
        .await
        .json();
    assert_eq!(listing["items"].as_array().unwrap().len(), 1);
    assert_eq!(listing["items"][0]["id"], earlier_id);
    assert_eq!(
        listing["items"][0]["original_path"],
        serde_json::json!(["top"])
    );
    let count: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM removal_member", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
    let restore = fixture
        .mutate(
            "POST",
            &format!("/trash/{earlier_id}/restore"),
            &owner_auth(),
            None,
            &[(
                "If-Match",
                listing["items"][0]["etag"].as_str().unwrap().into(),
            )],
            Vec::new(),
        )
        .await;
    assert_eq!(restore.status, 409, "{}", restore.text());
    let preview = fixture
        .removal_json(
            "/trash/removals/preview",
            serde_json::json!({"selection":{"kind":"all"}}),
        )
        .await;
    assert_eq!(preview.json()["count"], 1);
    let removed=fixture.removal_json("/trash/removals",serde_json::json!({"operation_id":"20000000000000000000000000000078","confirmation":preview.json()["confirmation"]})).await;
    assert_eq!(removed.status, 200, "{}", removed.text());
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_real_replay_auth_expiry_and_stale_previews() {
    use sylux::domain::removal::{self, Selection};
    let fixture = Fixture::new("removal-replay-rules").await;
    let first = fixture.upload("first.txt", b"first").await;
    let deleted = fixture.trash_uploaded(&first).await;
    let selected = serde_json::json!({"selection":{"kind":"selected","items":[{"id":deleted["id"],"etag":deleted["etag"]}]}});
    let preview = fixture
        .removal_json("/trash/removals/preview", selected.clone())
        .await
        .json();
    let operation = "31000000000000000000000000000078";
    let body = serde_json::json!({"operation_id":operation,"confirmation":preview["confirmation"]});
    for headers in [
        vec![("Origin", fixture.origin.clone())],
        vec![
            ("Origin", fixture.origin.clone()),
            ("X-CSRF-Token", other_auth().csrf),
        ],
        vec![
            ("Origin", "https://foreign.invalid".into()),
            ("X-CSRF-Token", owner_auth().csrf),
        ],
        vec![("X-CSRF-Token", owner_auth().csrf)],
    ] {
        let mut headers = headers;
        headers.push(("Content-Type", "application/json".into()));
        let rejected = fixture
            .request(
                "POST",
                "/trash/removals",
                Some(&owner_auth()),
                &headers,
                serde_json::to_vec(&body).unwrap(),
            )
            .await;
        assert_eq!(rejected.status, 403, "{}", rejected.text());
    }
    let restored = fixture
        .mutate(
            "POST",
            &format!("/trash/{}/restore", deleted["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[("If-Match", deleted["etag"].as_str().unwrap().into())],
            Vec::new(),
        )
        .await;
    assert_eq!(restored.status, 204, "{}", restored.text());
    let stale = fixture
        .removal_json("/trash/removals/preview", selected)
        .await;
    assert_eq!(stale.status, 404);
    let empty = fixture
        .removal_json(
            "/trash/removals/preview",
            serde_json::json!({"selection":{"kind":"all"}}),
        )
        .await;
    assert_eq!(empty.status, 404);
    let stale_execute = fixture.removal_json("/trash/removals", body).await;
    assert_eq!(stale_execute.status, 409);
    let second = fixture.upload_current("second.txt", b"second").await;
    fixture.trash_uploaded(&second).await;
    let key: [u8; 32] = fs::read(fixture.product.join("secrets/mutation.key"))
        .unwrap()
        .try_into()
        .unwrap();
    let expired = fixture
        .workers
        .as_ref()
        .unwrap()
        .read(move |db| {
            Ok(
                removal::preview(db, OWNER_ID, Selection::All, &key, unix_now() - 601)
                    .unwrap()
                    .confirmation,
            )
        })
        .await
        .unwrap();
    let expired_body = serde_json::json!({"operation_id":operation,"confirmation":expired});
    let rejected = fixture
        .removal_json("/trash/removals", expired_body.clone())
        .await;
    assert_eq!(rejected.status, 400);
    // Commit with the original clock, then prove HTTP replay ignores expiry of that result.
    let confirmation = expired.clone();
    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(move |tx| {
            removal::execute(
                tx,
                OWNER_ID,
                operation,
                &confirmation,
                &key,
                unix_now() - 601,
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let replay = fixture
        .removal_json("/trash/removals", expired_body.clone())
        .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(replay.json()["state"], "committed");
    let conflict = fixture
        .removal_json(
            "/trash/removals",
            serde_json::json!({"operation_id":operation,"confirmation":"different-confirmation"}),
        )
        .await;
    assert_eq!(conflict.status, 409);
    assert_eq!(conflict.json()["code"], "operation_conflict");
    let foreign = fixture
        .mutate(
            "POST",
            "/trash/removals",
            &other_auth(),
            None,
            &[("Content-Type", "application/json".into())],
            serde_json::to_vec(&expired_body).unwrap(),
        )
        .await;
    assert_eq!(foreign.status, 200);
    assert_eq!(
        foreign.json(),
        serde_json::json!({"operation_id":operation,"state":"unknown"})
    );
    let count: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM removal", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_runtime_cleanup_retries_without_blocking_other_trash_actions() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use sylux::persistence::payload::PayloadError;
    let mut fixture = Fixture::new("removal-runtime-retry").await;
    let fail = Arc::new(AtomicBool::new(true));
    let fail_hook = Arc::clone(&fail);
    fixture.cleanup_fault = Some(Arc::new(move |boundary| {
        if boundary == "cleanup.before_unlink" && fail_hook.load(Ordering::SeqCst) {
            Err(PayloadError::Io("injected_cleanup_fault"))
        } else {
            Ok(())
        }
    }));
    fixture.restart().await;
    let removed = fixture.upload_current("remove.txt", b"removed bytes").await;
    let deleted = fixture.trash_uploaded(&removed).await;
    let other = fixture
        .upload_current("restore.txt", b"retained active bytes")
        .await;
    let restorable = fixture.trash_uploaded(&other).await;
    let preview = fixture.removal_json("/trash/removals/preview", serde_json::json!({"selection":{"kind":"selected","items":[{"id":deleted["id"],"etag":deleted["etag"]}]}})).await.json();
    let operation = "32000000000000000000000000000078";
    let result = fixture
        .removal_json(
            "/trash/removals",
            serde_json::json!({"operation_id":operation,"confirmation":preview["confirmation"]}),
        )
        .await;
    assert_eq!(result.status, 200);
    assert_eq!(result.json()["state"], "committed");
    assert_eq!(result.json()["cleanup"], "pending");
    let restored = fixture
        .mutate(
            "POST",
            &format!("/trash/{}/restore", restorable["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[("If-Match", restorable["etag"].as_str().unwrap().into())],
            Vec::new(),
        )
        .await;
    assert_eq!(restored.status, 204, "{}", restored.text());
    // No second removal or restart is needed to reclaim the first operation's bytes.
    fail.store(false, Ordering::SeqCst);
    fixture.wait_cleanup(operation).await;
    let content = fixture
        .request(
            "GET",
            &format!("/entries/{}/content", other["entry_id"].as_str().unwrap()),
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(content.status, 200);
    assert_eq!(content.body.as_ref(), b"retained active bytes");
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_commit_rollback_and_lost_socket_reply_recover_same_operation() {
    let mut fixture = Fixture::new("removal-commit-fault").await;
    let active = fixture
        .upload_current("active.txt", b"active content")
        .await;
    let doomed = fixture
        .upload_current("doomed.txt", b"removed content")
        .await;
    fixture.trash_uploaded(&doomed).await;
    let preview = fixture
        .removal_json(
            "/trash/removals/preview",
            serde_json::json!({"selection":{"kind":"all"}}),
        )
        .await
        .json();
    let operation = "33000000000000000000000000000078";
    let body = serde_json::json!({"operation_id":operation,"confirmation":preview["confirmation"]});
    fixture.removal_commit_hook = Some(Arc::new(|_| Err(rusqlite::Error::InvalidQuery)));
    fixture.restart().await;
    let rejected = fixture.removal_json("/trash/removals", body.clone()).await;
    assert_eq!(rejected.status, 409);
    assert_eq!(rejected.json()["code"], "removal_not_committed");
    assert_eq!(
        fixture
            .db()
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM removal_member", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    fixture.removal_commit_hook = None;
    fixture.restart().await;
    let recovery = sylux::persistence::reconcile::reconcile(
        fixture.workers.as_ref().unwrap(),
        &fixture.product,
    )
    .await
    .unwrap();
    assert!(recovery.ready);
    let (reached, observed) = tokio::sync::oneshot::channel();
    let reached = Mutex::new(Some(reached));
    let (release, paused) = std::sync::mpsc::channel();
    let paused = Mutex::new(paused);
    fixture.removal_commit_hook = Some(Arc::new(move |_| {
        if let Some(reached) = reached.lock().unwrap().take() {
            reached.send(()).unwrap();
            paused.lock().unwrap().recv().unwrap();
        }
        Ok(())
    }));
    fixture.restart().await;
    let mut stream = TcpStream::connect(&fixture.address).await.unwrap();
    let json = serde_json::to_vec(&body).unwrap();
    let auth = owner_auth();
    let headers = format!(
        "POST /trash/removals HTTP/1.1\r\nHost: {}\r\nOrigin: {}\r\nCookie: {}\r\nX-CSRF-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        fixture.address,
        fixture.origin,
        auth.cookie,
        auth.csrf,
        json.len()
    );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&json).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), observed)
        .await
        .unwrap()
        .unwrap();
    drop(stream);
    release.send(()).unwrap();
    let replay = fixture.removal_json("/trash/removals", body.clone()).await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(replay.json()["state"], "committed");
    fixture.wait_cleanup(operation).await;
    fixture.removal_commit_hook = None;
    fixture.restart().await;
    let recovery = sylux::persistence::reconcile::reconcile(
        fixture.workers.as_ref().unwrap(),
        &fixture.product,
    )
    .await
    .unwrap();
    assert!(recovery.ready);
    let replay = fixture.removal_json("/trash/removals", body).await;
    assert_eq!(replay.json()["state"], "committed");
    assert_eq!(
        fixture
            .db()
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM removal", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let content = fixture
        .request(
            "GET",
            &format!("/entries/{}/content", active["entry_id"].as_str().unwrap()),
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(content.status, 200);
    assert_eq!(content.body.as_ref(), b"active content");
    fixture.cleanup.take().unwrap().shutdown().await.unwrap();
    if let Some(server) = fixture.server.take() {
        server.abort();
        let _ = server.await;
    }
    fixture.workers.take().unwrap().shutdown().await.unwrap();
    fixture.db.take();
    // Offline verification must accept the missing authorized payload and still check active bytes.
    let report = sylux::admin::verify_payloads(&fixture.product, |_, _| {
        panic!("retained payload must verify")
    })
    .unwrap();
    assert_eq!(report.checked, 1);
    assert_eq!(report.damaged, 0);
    let cli = std::process::Command::new(env!("CARGO_BIN_EXE_sylux"))
        .args(["admin", "verify-payloads", "--root"])
        .arg(&fixture.product)
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    assert_eq!(
        String::from_utf8(cli.stdout).unwrap(),
        "payload_verification checked=1 verified=1 damaged=0\n"
    );
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_confirmation_tracks_effect_content_and_ancestry_not_unrelated_items() {
    let fixture = Fixture::new("removal-fingerprint-scope").await;
    let first = fixture.upload_current("same.txt", b"first").await;
    let first_deleted = fixture.trash_uploaded(&first).await;
    let selected = serde_json::json!({"selection":{"kind":"selected","items":[{"id":first_deleted["id"],"etag":first_deleted["etag"]}]}});
    let preview = fixture
        .removal_json("/trash/removals/preview", selected.clone())
        .await
        .json();
    let second = fixture.upload_current("same.txt", b"second").await;
    let second_deleted = fixture.trash_uploaded(&second).await;
    let listing = fixture
        .request("GET", "/trash", Some(&owner_auth()), &[], Vec::new())
        .await
        .json();
    assert!(
        listing["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["needs_reference"] == true)
    );
    let result = fixture.removal_json("/trash/removals", serde_json::json!({"operation_id":"34000000000000000000000000000078","confirmation":preview["confirmation"]})).await;
    assert_eq!(
        result.status,
        200,
        "unrelated upload and Trash root must not invalidate selected removal: {}",
        result.text()
    );
    let listing = fixture
        .request("GET", "/trash", Some(&owner_auth()), &[], Vec::new())
        .await
        .json();
    assert_eq!(listing["items"].as_array().unwrap().len(), 1);
    assert_eq!(listing["items"][0]["id"], second_deleted["id"]);
    assert_ne!(listing["items"][0]["needs_reference"], true);
    let preview = fixture
        .removal_json(
            "/trash/removals/preview",
            serde_json::json!({"selection":{"kind":"all"}}),
        )
        .await
        .json();
    // A retained revision added without changing the current revision still changes the effect.
    let second_id = second["entry_id"].as_str().unwrap();
    fixture.db().lock().unwrap().execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) SELECT '35000000000000000000000000000078',entry_id,blob_id,length,sha256,committed_at,actor_account_id FROM revision WHERE entry_id=?1", [second_id]).unwrap();
    let rejected = fixture.removal_json("/trash/removals", serde_json::json!({"operation_id":"36000000000000000000000000000078","confirmation":preview["confirmation"]})).await;
    assert_eq!(rejected.status, 409);
    let preview = fixture
        .removal_json(
            "/trash/removals/preview",
            serde_json::json!({"selection":{"kind":"all"}}),
        )
        .await
        .json();
    let _unrelated = fixture.upload_current("unrelated.txt", b"unrelated").await;
    let removed = fixture.removal_json("/trash/removals", serde_json::json!({"operation_id":"37000000000000000000000000000078","confirmation":preview["confirmation"]})).await;
    assert_eq!(
        removed.status,
        200,
        "unrelated upload must not invalidate Empty Trash: {}",
        removed.text()
    );
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_reconcile_rejects_completed_cleanup_with_deleting_blob() {
    use sylux::domain::removal::{self, Selection};
    let mut fixture = Fixture::new("removal-cleanup-invariant").await;
    let uploaded = fixture.upload_current("removed.txt", b"removed").await;
    fixture.trash_uploaded(&uploaded).await;
    fixture.cleanup.take().unwrap().shutdown().await.unwrap();
    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            let preview =
                removal::preview(tx, OWNER_ID, Selection::All, &[8; 32], unix_now()).unwrap();
            removal::execute(
                tx,
                OWNER_ID,
                "38000000000000000000000000000078",
                &preview.confirmation,
                &[8; 32],
                unix_now(),
            )
            .unwrap();
            tx.execute("UPDATE removal_cleanup SET state='complete'", [])?;
            Ok(())
        })
        .await
        .unwrap();
    fixture.restart().await;
    let report = sylux::persistence::reconcile::reconcile(
        fixture.workers.as_ref().unwrap(),
        &fixture.product,
    )
    .await
    .unwrap();
    assert!(
        !report.ready,
        "inconsistent cleanup metadata must refuse readiness"
    );
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_cleanup_shutdown_joins_blocking_io_before_database_shutdown() {
    let mut fixture = Fixture::new("removal-cleanup-join").await;
    let (reached, observed) = tokio::sync::oneshot::channel();
    let reached = Mutex::new(Some(reached));
    let (release, paused) = std::sync::mpsc::channel();
    let paused = Mutex::new(paused);
    fixture.cleanup_fault = Some(Arc::new(move |at| {
        if at == "cleanup.before_unlink"
            && let Some(reached) = reached.lock().unwrap().take()
        {
            let _ = reached.send(());
            paused
                .lock()
                .unwrap()
                .recv()
                .map_err(|_| sylux::persistence::payload::PayloadError::Cancelled)?;
        }
        Ok(())
    }));
    fixture.restart().await;
    let uploaded = fixture.upload_current("removed.txt", b"removed").await;
    fixture.trash_uploaded(&uploaded).await;
    let preview = fixture
        .removal_json(
            "/trash/removals/preview",
            serde_json::json!({"selection":{"kind":"all"}}),
        )
        .await
        .json();
    let removed = fixture.removal_json("/trash/removals", serde_json::json!({"operation_id":"39000000000000000000000000000078","confirmation":preview["confirmation"]})).await;
    assert_eq!(removed.status, 200);
    tokio::time::timeout(Duration::from_secs(3), observed)
        .await
        .unwrap()
        .unwrap();
    let shutdown = tokio::spawn(fixture.cleanup.take().unwrap().shutdown());
    tokio::task::yield_now().await;
    assert!(
        !shutdown.is_finished(),
        "shutdown must join in-flight blocking cleanup"
    );
    let status = fixture
        .request(
            "GET",
            "/trash/removals/39000000000000000000000000000078",
            Some(&owner_auth()),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(status.json()["cleanup"], "pending");
    release.send(()).unwrap();
    shutdown.await.unwrap().unwrap();
    fixture
        .wait_cleanup("39000000000000000000000000000078")
        .await;
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_confirmation_rejects_changed_ancestor_path() {
    let fixture = Fixture::new("removal-ancestor-snapshot").await;
    let parent = fixture
        .removal_json(
            "/folders/children",
            serde_json::json!({"parent_id":OWNER_ROOT,"name":"Parent"}),
        )
        .await
        .json();
    let child = fixture
        .removal_json(
            "/folders/children",
            serde_json::json!({"parent_id":parent["id"],"name":"Child"}),
        )
        .await
        .json();
    let trashed = fixture
        .mutate(
            "DELETE",
            &format!("/entries/{}", child["id"].as_str().unwrap()),
            &owner_auth(),
            None,
            &[("If-Match", child["etag"].as_str().unwrap().into())],
            Vec::new(),
        )
        .await;
    assert_eq!(trashed.status, 204);
    let preview = fixture
        .removal_json(
            "/trash/removals/preview",
            serde_json::json!({"selection":{"kind":"all"}}),
        )
        .await
        .json();
    let parent_id = parent["id"].as_str().unwrap();
    let generation: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row(
            "SELECT generation FROM entry WHERE entry_id=?1",
            [parent_id],
            |r| r.get(0),
        )
        .unwrap();
    let renamed = fixture
        .mutate(
            "PATCH",
            &format!("/entries/{parent_id}"),
            &owner_auth(),
            None,
            &[
                ("Content-Type", "application/json".into()),
                ("If-Match", folder_etag(parent_id, generation as u64)),
            ],
            serde_json::to_vec(&serde_json::json!({"name":"Renamed"})).unwrap(),
        )
        .await;
    assert_eq!(renamed.status, 200, "{}", renamed.text());
    let rejected = fixture.removal_json("/trash/removals", serde_json::json!({"operation_id":"3a000000000000000000000000000078","confirmation":preview["confirmation"]})).await;
    assert_eq!(rejected.status, 409);
    assert_eq!(
        fixture
            .db()
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM removal", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    fixture.close().await;
}

impl Fixture {
    async fn append_chunk(&self, id: &str, ticket: &str, offset: u64, bytes: &[u8]) -> Response {
        self.mutate(
            "PATCH",
            &format!("/uploads/{id}/content"),
            &owner_auth(),
            Some(ticket),
            &[
                ("Content-Type", "application/octet-stream".into()),
                ("Upload-Offset", offset.to_string()),
                ("Upload-Checksum", hex(&Sha256::digest(bytes))),
            ],
            bytes.to_vec(),
        )
        .await
    }
    async fn upload_status(&self, id: &str) -> Value {
        self.request(
            "GET",
            &format!("/uploads/{id}"),
            Some(&owner_auth()),
            &[],
            vec![],
        )
        .await
        .json()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sequential_append_resumes_acknowledged_prefix_and_rejects_stale_offsets() {
    let mut fixture = Fixture::new("append-resume").await;
    let bytes = b"original bytes across chunks";
    let (id, ticket) = fixture
        .admit(&owner_auth(), OWNER_ROOT, 1, "resume.bin", bytes)
        .await;
    let first = fixture.append_chunk(&id, &ticket, 0, &bytes[..9]).await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(first.json()["bytes_received"], 9);
    assert_eq!(
        fixture
            .append_chunk(&id, &ticket, 0, &bytes[..9])
            .await
            .status,
        409
    );
    assert_eq!(
        fixture
            .append_chunk(&id, &ticket, 10, &bytes[9..])
            .await
            .status,
        422
    );
    assert_eq!(fixture.upload_status(&id).await["bytes_received"], 9);
    let legacy = fixture
        .mutate(
            "PUT",
            &format!("/uploads/{id}/content"),
            &owner_auth(),
            Some(&ticket),
            &[("Content-Type", "application/octet-stream".into())],
            bytes.to_vec(),
        )
        .await;
    assert_eq!(legacy.status, 409);
    assert_eq!(legacy.json()["code"], "upload_requires_resume");
    // Simulate an unacknowledged tail left after a write but before SQL checkpoint.
    let name: String = fixture
        .db()
        .lock()
        .unwrap()
        .query_row(
            "SELECT staging_path FROM upload_attempt WHERE mutation_id=?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(fixture.product.join("staging").join(name))
        .unwrap()
        .write_all(b"discard this tail")
        .unwrap();
    fixture.restart().await;
    assert_eq!(fixture.upload_status(&id).await["bytes_received"], 9);
    let last = fixture.append_chunk(&id, &ticket, 9, &bytes[9..]).await;
    assert_eq!(last.status, 200, "{}", last.text());
    assert_eq!(last.json()["sha256"], hex(&Sha256::digest(bytes)));
    assert_eq!(last.json()["status"], "staged");
    let committed = fixture.commit_staged(&id, &ticket, bytes).await;
    assert_eq!(committed.status, 201, "{}", committed.text());
    let entry = committed.json()["id"].as_str().unwrap().to_owned();
    let download = fixture
        .request(
            "GET",
            &format!("/entries/{entry}/content"),
            Some(&owner_auth()),
            &[],
            vec![],
        )
        .await;
    assert_eq!(download.body.as_ref(), bytes);
    assert_eq!(fixture.upload_status(&id).await["state"], "committed");
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_append_preserves_offset_isolation_checksum_and_expiry() {
    let fixture = Fixture::new("append-interrupt").await;
    let bytes = b"abcdefghijklmno";
    let (id, ticket) = fixture
        .admit(&owner_auth(), OWNER_ROOT, 1, "partial.bin", bytes)
        .await;
    assert_eq!(
        fixture
            .append_chunk(&id, &ticket, 0, &bytes[..4])
            .await
            .status,
        200
    );
    let mut stream = TcpStream::connect(&fixture.address).await.unwrap();
    let request = format!(
        "PATCH /uploads/{id}/content HTTP/1.1\r\nHost: {}\r\nOrigin: {}\r\nCookie: {}\r\nX-CSRF-Token: {}\r\nX-Mutation-ID: {ticket}\r\nUpload-Offset: 4\r\nUpload-Checksum: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: 11\r\n\r\nef",
        fixture.address,
        fixture.origin,
        owner_auth().cookie,
        owner_auth().csrf,
        hex(&Sha256::digest(&bytes[4..]))
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    for _ in 0..100 {
        if fixture.upload_status(&id).await["writer_active"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(fixture.upload_status(&id).await["writer_active"], true);
    assert_eq!(
        fixture
            .append_chunk(&id, &ticket, 4, &bytes[4..])
            .await
            .status,
        409
    );
    stream.shutdown().await.unwrap();
    drop(stream);
    for _ in 0..100 {
        if fixture.upload_status(&id).await["writer_active"] == false {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(fixture.upload_status(&id).await["bytes_received"], 4);
    let other = fixture
        .mutate(
            "PATCH",
            &format!("/uploads/{id}/content"),
            &other_auth(),
            Some(&ticket),
            &[
                ("Upload-Offset", "4".into()),
                ("Content-Type", "application/octet-stream".into()),
                ("Upload-Checksum", hex(&Sha256::digest(&bytes[4..]))),
            ],
            bytes[4..].to_vec(),
        )
        .await;
    assert_eq!(other.status, 401);
    let bad = fixture
        .mutate(
            "PATCH",
            &format!("/uploads/{id}/content"),
            &owner_auth(),
            Some(&ticket),
            &[
                ("Upload-Offset", "4".into()),
                ("Content-Type", "application/octet-stream".into()),
                ("Upload-Checksum", "0".repeat(64)),
            ],
            bytes[4..].to_vec(),
        )
        .await;
    assert_eq!(bad.status, 422);
    assert_eq!(fixture.upload_status(&id).await["bytes_received"], 4);
    let db = fixture.db();
    db.lock()
        .unwrap()
        .execute(
            "UPDATE mutation SET issued_at=0,expires_at=0 WHERE mutation_id=?1",
            [&id],
        )
        .unwrap();
    let storage = Arc::new(sylux::domain::upload::UploadStorage::open(&fixture.product).unwrap());
    let mut cursor = String::new();
    assert_eq!(
        sylux::domain::upload::cleanup_expired_partials(
            fixture.workers.as_ref().unwrap(),
            storage,
            &mut cursor,
            1
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        fs::read_dir(fixture.product.join("staging"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(fixture.upload_status(&id).await["state"], "rejected");
    fixture.close().await;
}

#[tokio::test]
async fn upload_limit_accepts_64_gib_and_rejects_one_byte_over() {
    let fixture = Fixture::new("large-admission-cap").await;
    for (length, expected) in [(64 * 1024u64.pow(3), 201), (64 * 1024u64.pow(3) + 1, 413)] {
        let response = fixture.mutate("POST", "/mutation-tickets", &owner_auth(), None, &[("Content-Type", "application/json".into())], serde_json::to_vec(&serde_json::json!({"scope":"upload_create","parent_id":OWNER_ROOT,"parent_etag":folder_etag(OWNER_ROOT,1),"name":"large.bin","length":length})).unwrap()).await;
        assert_eq!(response.status, expected, "{}", response.text());
        if expected == 201 {
            assert_eq!(response.json()["max_upload_length"], length);
            assert_eq!(response.json()["max_chunk_length"], 8 * 1024 * 1024);
        }
    }
    fixture.close().await;
}

#[tokio::test]
async fn restart_recovers_prepared_partial_before_and_after_publication() {
    use sylux::persistence::payload::{PayloadError, PayloadStore};
    for boundary in ["publish.before_noreplace", "publish.before_final_dir_sync"] {
        let fixture = Fixture::new("append-publication-fault").await;
        let bytes = b"a partial with its final publication interrupted";
        let (id, ticket) = fixture
            .admit(&owner_auth(), OWNER_ROOT, 1, "recover.bin", bytes)
            .await;
        assert_eq!(
            fixture
                .append_chunk(&id, &ticket, 0, &bytes[..4])
                .await
                .status,
            200
        );
        let db = fixture.db();
        let name: String = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT staging_path FROM upload_attempt WHERE mutation_id=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        let mut store = PayloadStore::open(&fixture.product).unwrap();
        store
            .append_at(&name, 4, bytes.len() as u64, &bytes[4..])
            .unwrap();
        store.set_publish_fault_hook(Some(Arc::new(move |at| {
            if at == boundary {
                Err(PayloadError::Io("injected_publish_fault"))
            } else {
                Ok(())
            }
        })));
        let staging = store
            .prepare_partial(&name, bytes.len() as u64, None, || false)
            .unwrap();
        let digest = staging.summary().sha256;
        db.lock().unwrap().execute("UPDATE mutation SET state='publishing',completed_length=?2,completed_sha256=?3 WHERE mutation_id=?1", params![id,bytes.len() as i64,digest.as_slice()]).unwrap();
        db.lock().unwrap().execute("UPDATE upload_attempt SET bytes_received=?2,completed_length=?2,completed_sha256=?3,writer_active=1 WHERE mutation_id=?1", params![id,bytes.len() as i64,digest.as_slice()]).unwrap();
        assert_eq!(
            staging.publish(digest).unwrap_err(),
            PayloadError::Io("injected_publish_fault")
        );
        assert_eq!(fixture.commit_staged(&id, &ticket, bytes).await.status, 409);
        let report = sylux::persistence::reconcile::reconcile(
            fixture.workers.as_ref().unwrap(),
            &fixture.product,
        )
        .await
        .unwrap();
        assert!(report.ready, "{boundary}: {report:?}");
        assert_eq!(fixture.upload_status(&id).await["writer_active"], false);
        let commit = fixture.commit_staged(&id, &ticket, bytes).await;
        assert_eq!(commit.status, 201, "{}", commit.text());
        let replay = fixture.upload_status(&id).await;
        assert_eq!(replay["sha256"], hex(&digest));
        assert_eq!(replay["state"], "committed");
        fixture.close().await;
    }
}

#[tokio::test]
async fn empty_append_commits_and_wrong_declared_digest_rejects_without_visibility() {
    let fixture = Fixture::new("append-empty").await;
    let (id, ticket) = fixture
        .admit(&owner_auth(), OWNER_ROOT, 1, "empty.bin", b"")
        .await;
    assert_eq!(fixture.append_chunk(&id, &ticket, 0, b"").await.status, 200);
    assert_eq!(fixture.commit_staged(&id, &ticket, b"").await.status, 201);
    fixture.close().await;
    let fixture = Fixture::new("append-final-digest").await;
    let (id, ticket) = fixture
        .admit(&owner_auth(), OWNER_ROOT, 1, "wrong.bin", b"expected")
        .await;
    assert_eq!(
        fixture
            .append_chunk(&id, &ticket, 0, b"wrong___")
            .await
            .status,
        422
    );
    assert_eq!(fixture.upload_status(&id).await["state"], "rejected");
    let visible: i64 = fixture
        .db()
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM entry WHERE kind='file'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(visible, 0);
    fixture.close().await;
}

#[tokio::test]
async fn expired_partial_cleanup_continues_past_an_invalid_candidate() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("partial-cleanup-failure").await;
    for name in ["one.bin", "two.bin"] {
        let (id, ticket) = fixture
            .admit(&owner_auth(), OWNER_ROOT, 1, name, b"abcdef")
            .await;
        assert_eq!(
            fixture.append_chunk(&id, &ticket, 0, b"abc").await.status,
            200
        );
    }
    let db = fixture.db();
    let mut paths: Vec<_> = fs::read_dir(fixture.product.join("staging"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    fs::set_permissions(&paths[0], fs::Permissions::from_mode(0o644)).unwrap();
    db.lock()
        .unwrap()
        .execute("UPDATE mutation SET issued_at=0,expires_at=0", [])
        .unwrap();
    let storage = Arc::new(sylux::domain::upload::UploadStorage::open(&fixture.product).unwrap());
    let workers = fixture.workers.as_ref().unwrap();
    let mut cursor = String::new();
    assert_eq!(
        sylux::domain::upload::cleanup_expired_partials(workers, storage.clone(), &mut cursor, 1)
            .await,
        Err("partial_cleanup_failed")
    );
    assert!(paths[0].exists());
    assert!(!paths[1].exists());
    assert!(!cursor.is_empty());
    assert_eq!(
        sylux::domain::upload::cleanup_expired_partials(workers, storage.clone(), &mut cursor, 1)
            .await
            .unwrap(),
        0
    );
    fs::set_permissions(&paths[0], fs::Permissions::from_mode(0o600)).unwrap();
    sylux::domain::upload::cleanup_expired_partials(workers, storage, &mut cursor, 1)
        .await
        .unwrap();
    assert!(!paths[0].exists());
    fixture.close().await;
}
