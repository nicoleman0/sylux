//! PoC P5 real HTTP + SQLite upload/publication/content contracts.
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
        let parent_generation: i64 = self
            .db()
            .lock()
            .unwrap()
            .query_row(
                "SELECT generation FROM entry WHERE entry_id=?1",
                [OWNER_ROOT],
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
            "parent_id": OWNER_ROOT, "parent_etag": folder_etag(OWNER_ROOT, parent_generation as u64),
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
