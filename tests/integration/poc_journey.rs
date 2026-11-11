//! PoC P7: scripted end-to-end personal-storage journey on a fresh root.
//!
//! Exercises the real product router and `root::initialize_empty_root` (the
//! actual host-init path, not a seeded database) end to end: one-use setup →
//! local login → folder creation → uploads (binary, Unicode, empty, multi-MB)
//! → listing/rename → exact download with digest verification → logout/login →
//! process-equivalent restart → acknowledged inventory unchanged. A second
//! seeded account and an unrelated administrator are then denied read access
//! to the first account's files by every S1 route.
//!
//! This is the PoC bar (docs/poc-milestone.md), not S1 acceptance: no
//! move/delete, no sharing, no admin content privilege is claimed here.
#![cfg(feature = "test-harness")]
#[path = "../common/mod.rs"]
mod common;

use argon2::{Argon2, PasswordHasher};
use axum::body::Bytes;
use common::{FixtureRoot, sha256_hex, unique_token};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
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
const OWNER_HANDLE: &str = "owner";
const OWNER_PASSWORD: &str = "correct horse battery staple";
const OTHER_HANDLE: &str = "owner-b";
const OTHER_PASSWORD: &str = "other account password";
const ADMIN_HANDLE: &str = "unrelated-admin";
const ADMIN_PASSWORD: &str = "admin account password";

struct Auth {
    cookie: String,
    csrf: String,
}

/// A disposable product root driving the real router on an ephemeral loopback
/// listener. Host init uses the real `root::initialize_empty_root`; setup uses
/// the real one-use authorization read back out of the protected secrets file.
struct Journey {
    owner: FixtureRoot,
    product: PathBuf,
    db_path: PathBuf,
    setup_authorization: String,
    db: Option<Arc<Mutex<Connection>>>,
    workers: Option<SqliteWorkers>,
    origin: String,
    address: String,
    server: Option<JoinHandle<()>>,
    commit_barrier: Arc<sylux::http::uploads::UploadCommitTestBarrier>,
}

impl Journey {
    async fn new(case: &str) -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-p7-{}", unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "journey", case).unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).expect("host init");
        let setup_authorization = fs::read_to_string(
            product
                .join(root::SECRETS_DIR)
                .join(root::SETUP_AUTHORIZATION_NAME),
        )
        .expect("setup authorization readable after init");
        assert_eq!(setup_authorization.len(), 64);
        assert!(setup_authorization.bytes().all(|b| b.is_ascii_hexdigit()));
        let db_path = product.join(root::METADATA_DIR).join("sylux.sqlite3");
        let connection = sqlite::open_writer(&db_path, BUSY).expect("product SQLite writer");
        let mut journey = Self {
            owner,
            product,
            db_path,
            setup_authorization,
            db: None,
            workers: None,
            origin: String::new(),
            address: String::new(),
            server: None,
            commit_barrier: Arc::default(),
        };
        journey.start(connection).await;
        journey
    }

    async fn start(&mut self, connection: Connection) {
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
        let mut state = AuthState::new_with_setup(
            workers.clone(),
            self.origin.clone(),
            self.setup_authorization.clone(),
        )
        .with_storage_root(&self.product)
        .expect("payload storage");
        state.upload_commit_test_barrier = Some(self.commit_barrier.clone());
        let app = http::router(state);
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        self.db = Some(db);
        self.workers = Some(workers);
    }

    /// Stop the server and close the database (drops the last writer).
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

    /// Reopen the database from disk and rebuild the router (restart).
    async fn restart(&mut self) {
        self.stop().await;
        let connection = sqlite::open_writer(&self.db_path, BUSY).unwrap();
        self.start(connection).await;
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
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Response {
        let mut stream = TcpStream::connect(&self.address).await.unwrap();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n",
            self.address
        );
        if let Some(cookie) = cookie {
            request.push_str(&format!("Cookie: {cookie}\r\n"));
        }
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let mut wire = request.into_bytes();
        wire.extend_from_slice(&body);
        stream.write_all(&wire).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        if bytes.is_empty() {
            panic!("empty HTTP response; server may have exited");
        }
        Response::parse(&bytes)
    }

    async fn bootstrap(&self) -> Auth {
        let response = self
            .request("GET", "/auth/csrf", None, &[], Vec::new())
            .await;
        assert_eq!(response.status, 200, "{}", response.text());
        let csrf = response.json()["csrf_token"].as_str().unwrap().to_owned();
        Auth {
            cookie: response.cookie(),
            csrf,
        }
    }

    async fn login(&self, handle: &str, password: &str) -> Auth {
        let pre = self.bootstrap().await;
        let response = self
            .request(
                "POST",
                "/auth/local",
                Some(&pre.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", pre.csrf.clone()),
                    ("Content-Type", "application/json".to_owned()),
                ],
                serde_json::to_vec(&json!({"handle": handle, "password": password})).unwrap(),
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.text());
        Auth {
            cookie: response.cookie(),
            csrf: response.json()["csrf_token"].as_str().unwrap().to_owned(),
        }
    }

    async fn setup_admin(&self) {
        let response = self
            .request(
                "POST",
                "/setup/admin",
                None,
                &[
                    ("X-Setup-Authorization", self.setup_authorization.clone()),
                    ("Origin", self.origin.clone()),
                    ("Content-Type", "application/json".to_owned()),
                ],
                serde_json::to_vec(&json!({
                    "handle": OWNER_HANDLE,
                    "password": OWNER_PASSWORD,
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(response.status, 201, "{}", response.text());
        assert_eq!(response.json()["code"].as_str().unwrap(), "setup_complete");
    }

    async fn list_root(&self, auth: &Auth) -> Value {
        let response = self
            .request(
                "GET",
                "/folders/children",
                Some(&auth.cookie),
                &[],
                Vec::new(),
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.text());
        response.json()
    }

    async fn create_folder(&self, auth: &Auth, parent_id: &str, name: &str) -> Value {
        let response = self
            .request(
                "POST",
                &format!("/folders/{parent_id}/children"),
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("Content-Type", "application/json".to_owned()),
                ],
                serde_json::to_vec(&json!({"name": name})).unwrap(),
            )
            .await;
        assert_eq!(response.status, 201, "{}", response.text());
        response.json()
    }

    async fn upload(
        &self,
        auth: &Auth,
        parent_id: &str,
        parent_etag: &str,
        name: &str,
        bytes: &[u8],
    ) -> Value {
        let digest = sha256_hex(bytes);
        let ticket_response = self
            .request(
                "POST",
                "/mutation-tickets",
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("Content-Type", "application/json".to_owned()),
                ],
                serde_json::to_vec(&json!({
                    "scope": "upload_create",
                    "parent_id": parent_id,
                    "parent_etag": parent_etag,
                    "name": name,
                    "length": bytes.len(),
                    "sha256": digest,
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(ticket_response.status, 201, "{}", ticket_response.text());
        let ticket = ticket_response.header("x-mutation-id").unwrap().to_owned();

        let admit_response = self
            .request(
                "POST",
                "/uploads",
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("X-Mutation-ID", ticket.clone()),
                    ("Content-Type", "application/json".to_owned()),
                    ("If-None-Match", "*".to_owned()),
                ],
                serde_json::to_vec(&json!({
                    "parent_id": parent_id,
                    "parent_etag": parent_etag,
                    "name": name,
                    "length": bytes.len(),
                    "sha256": digest,
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(admit_response.status, 201, "{}", admit_response.text());
        let upload_id = admit_response.json()["upload_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let content_response = self
            .request(
                "PUT",
                &format!("/uploads/{upload_id}/content"),
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("X-Mutation-ID", ticket.clone()),
                    ("Content-Type", "application/octet-stream".to_owned()),
                ],
                bytes.to_vec(),
            )
            .await;
        assert_eq!(content_response.status, 200, "{}", content_response.text());
        assert_eq!(
            content_response.json()["sha256"].as_str().unwrap(),
            digest.as_str()
        );

        let commit_response = self
            .request(
                "POST",
                &format!("/uploads/{upload_id}/commit"),
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("X-Mutation-ID", ticket.clone()),
                    ("Content-Type", "application/json".to_owned()),
                    ("If-None-Match", "*".to_owned()),
                ],
                serde_json::to_vec(&json!({
                    "sha256": digest,
                    "parent_etag": parent_etag,
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(commit_response.status, 201, "{}", commit_response.text());
        commit_response.json()
    }

    async fn stage_upload(
        &self,
        auth: &Auth,
        parent_id: &str,
        parent_etag: &str,
        name: &str,
        bytes: &[u8],
    ) -> (String, String, String) {
        let digest = sha256_hex(bytes);
        let ticket_response = self
            .request(
                "POST",
                "/mutation-tickets",
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("Content-Type", "application/json".to_owned()),
                ],
                serde_json::to_vec(&json!({
                    "scope": "upload_create",
                    "parent_id": parent_id,
                    "parent_etag": parent_etag,
                    "name": name,
                    "length": bytes.len(),
                    "sha256": digest,
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(ticket_response.status, 201, "{}", ticket_response.text());
        let ticket = ticket_response.header("x-mutation-id").unwrap().to_owned();

        let admit_response = self
            .request(
                "POST",
                "/uploads",
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("X-Mutation-ID", ticket.clone()),
                    ("Content-Type", "application/json".to_owned()),
                    ("If-None-Match", "*".to_owned()),
                ],
                serde_json::to_vec(&json!({
                    "parent_id": parent_id,
                    "parent_etag": parent_etag,
                    "name": name,
                    "length": bytes.len(),
                    "sha256": digest,
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(admit_response.status, 201, "{}", admit_response.text());
        let upload_id = admit_response.json()["upload_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let content_response = self
            .request(
                "PUT",
                &format!("/uploads/{upload_id}/content"),
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("X-Mutation-ID", ticket.clone()),
                    ("Content-Type", "application/octet-stream".to_owned()),
                ],
                bytes.to_vec(),
            )
            .await;
        assert_eq!(content_response.status, 200, "{}", content_response.text());
        assert_eq!(
            content_response.json()["sha256"].as_str().unwrap(),
            digest.as_str()
        );

        (upload_id, ticket, digest)
    }

    async fn rename(&self, auth: &Auth, entry_id: &str, etag: &str, new_name: &str) -> Value {
        let response = self
            .request(
                "PATCH",
                &format!("/entries/{entry_id}"),
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("If-Match", etag.to_owned()),
                    ("Content-Type", "application/json".to_owned()),
                ],
                serde_json::to_vec(&json!({"name": new_name})).unwrap(),
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.text());
        response.json()
    }

    async fn download(&self, auth: &Auth, entry_id: &str) -> Response {
        let response = self
            .request(
                "GET",
                &format!("/entries/{entry_id}/content"),
                Some(&auth.cookie),
                &[],
                Vec::new(),
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.text());
        response
    }
}

fn seed_account(
    connection: &Connection,
    account_id: &str,
    root_id: &str,
    handle: &str,
    role: &str,
    password: &str,
) {
    let verifier = Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    connection
        .execute(
            "INSERT INTO account(account_id,handle,role,enabled,credential_epoch,password_verifier) VALUES(?1,?2,?3,1,0,?4)",
            params![account_id, handle, role, verifier],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,NULL,'folder','',x'',1)",
            params![root_id, account_id],
        )
        .unwrap();
}

/// Canonical form of the acknowledged inventory: sorted (name, kind, id, etag).
fn inventory(items: &[Value]) -> Vec<(String, String, String, String)> {
    let mut rows: Vec<(String, String, String, String)> = items
        .iter()
        .map(|item| {
            (
                item["name"].as_str().unwrap().to_owned(),
                item["kind"].as_str().unwrap().to_owned(),
                item["id"].as_str().unwrap().to_owned(),
                item["etag"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    rows.sort();
    rows
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

    fn cookie(&self) -> String {
        self.header("set-cookie")
            .and_then(|value| value.split(';').next())
            .unwrap()
            .to_owned()
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
async fn full_journey_and_restart_invariance() {
    let mut journey = Journey::new("restart-invariance").await;

    // One-use setup: exactly one admin is created, then setup closes.
    journey.setup_admin().await;
    let setup_status = journey
        .request(
            "GET",
            "/setup",
            None,
            &[("X-Setup-Authorization", journey.setup_authorization.clone())],
            Vec::new(),
        )
        .await;
    assert_eq!(setup_status.status, 404, "setup closes after consumption");

    // Local login.
    let owner = journey.login(OWNER_HANDLE, OWNER_PASSWORD).await;
    let root = journey.list_root(&owner).await;
    let root_id = root["root"]["id"].as_str().unwrap().to_owned();
    let base_generation: u64 = root["root"]["generation"].as_u64().unwrap();
    assert_eq!(root["root"]["kind"].as_str().unwrap(), "folder");
    assert_eq!(
        root["items"].as_array().unwrap().len(),
        0,
        "empty root listing"
    );

    // Upload the full fixture matrix to the root.
    let binary: Vec<u8> = (0..=255u8).cycle().take(256 * 1024 + 17).collect();
    let unicode: Vec<u8> = "Snowman ☃ — café 雪\n".repeat(4096).into_bytes();
    let empty: Vec<u8> = Vec::new();
    let multi_mb: Vec<u8> = (0..=255u8).cycle().take(5 * 1024 * 1024 + 31).collect();
    let fixtures: Vec<(&str, &[u8])> = vec![
        ("binary.bin", &binary),
        ("café 雪.txt", &unicode),
        ("empty.bin", &empty),
        ("large.bin", &multi_mb),
    ];
    let mut uploaded: Vec<(String, String)> = Vec::new();
    for (index, (name, bytes)) in fixtures.iter().enumerate() {
        // Each upload commit bumps the parent folder's generation, so derive
        // each admission's parent etag from the base generation plus the index.
        let parent_etag = format!("\"{root_id}:{}\"", base_generation + index as u64);
        let committed = journey
            .upload(&owner, &root_id, &parent_etag, name, bytes)
            .await;
        assert_eq!(committed["kind"].as_str().unwrap(), "file");
        assert_eq!(committed["size"].as_u64().unwrap(), bytes.len() as u64);
        assert_eq!(committed["sha256"].as_str().unwrap(), sha256_hex(bytes));
        uploaded.push((
            (*name).to_owned(),
            committed["id"].as_str().unwrap().to_owned(),
        ));
    }

    // A folder plus a name-only rename of the first uploaded file.
    let folder = journey.create_folder(&owner, &root_id, "docs").await;
    assert_eq!(folder["kind"].as_str().unwrap(), "folder");

    let listing = journey.list_root(&owner).await;
    let binary_id = &uploaded[0].1;
    let binary_etag = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"].as_str() == Some(binary_id.as_str()))
        .unwrap()["etag"]
        .as_str()
        .unwrap()
        .to_owned();
    let renamed = journey
        .rename(&owner, binary_id, &binary_etag, "binary-renamed.bin")
        .await;
    assert_eq!(renamed["name"].as_str().unwrap(), "binary-renamed.bin");

    // Exact download with digest verification for every uploaded file.
    for (name, id) in &uploaded {
        let response = journey.download(&owner, id).await;
        assert_eq!(
            response.header("content-type"),
            Some("application/octet-stream")
        );
        assert_eq!(response.header("cache-control"), Some("no-store"));
        let expected = fixtures.iter().find(|(n, _)| n == name).unwrap().1;
        assert_eq!(response.body.as_ref(), expected, "exact bytes for {name}");
        assert_eq!(
            sha256_hex(response.body.as_ref()),
            sha256_hex(expected),
            "digest for {name}"
        );
    }

    // Acknowledged inventory before restart (after the rename).
    let listing = journey.list_root(&owner).await;
    let before = inventory(listing["items"].as_array().unwrap());
    assert_eq!(before.len(), 5, "4 files + 1 folder");

    // Logout revokes the session, then a fresh login works.
    let logout = journey
        .request(
            "POST",
            "/auth/logout",
            Some(&owner.cookie),
            &[
                ("Origin", journey.origin.clone()),
                ("X-CSRF-Token", owner.csrf.clone()),
            ],
            Vec::new(),
        )
        .await;
    assert_eq!(logout.status, 204);
    let me = journey
        .request("GET", "/me", Some(&owner.cookie), &[], Vec::new())
        .await;
    assert_eq!(me.status, 401, "logout revokes the session");

    // Restart (close DB + rebuild router), then compare acknowledged inventory.
    journey.restart().await;
    let owner_after = journey.login(OWNER_HANDLE, OWNER_PASSWORD).await;
    let listing = journey.list_root(&owner_after).await;
    let after = inventory(listing["items"].as_array().unwrap());
    assert_eq!(
        before, after,
        "acknowledged inventory unchanged after restart"
    );

    // The renamed file still round-trips exactly after restart.
    let response = journey.download(&owner_after, binary_id).await;
    assert_eq!(response.body.as_ref(), binary.as_slice());

    journey.close().await;
}

#[tokio::test]
async fn isolation_second_account_and_admin_denied() {
    let mut journey = Journey::new("isolation").await;
    journey.setup_admin().await;
    let owner = journey.login(OWNER_HANDLE, OWNER_PASSWORD).await;
    let root = journey.list_root(&owner).await;
    let root_id = root["root"]["id"].as_str().unwrap().to_owned();
    let root_etag = root["root"]["etag"].as_str().unwrap().to_owned();
    let secret: Vec<u8> = b"account-a private payload".to_vec();
    let committed = journey
        .upload(&owner, &root_id, &root_etag, "secret.bin", &secret)
        .await;
    let secret_id = committed["id"].as_str().unwrap().to_owned();
    let folder = journey.create_folder(&owner, &root_id, "private").await;
    let folder_id = folder["id"].as_str().unwrap().to_owned();

    // Stop, seed a second account and an unrelated administrator, restart.
    journey.stop().await;
    let connection = sqlite::open_writer(&journey.db_path, BUSY).unwrap();
    seed_account(
        &connection,
        "00000000000000000000000000000002",
        "00000000000000000000000000000102",
        OTHER_HANDLE,
        "user",
        OTHER_PASSWORD,
    );
    seed_account(
        &connection,
        "00000000000000000000000000000003",
        "00000000000000000000000000000103",
        ADMIN_HANDLE,
        "administrator",
        ADMIN_PASSWORD,
    );
    journey.start(connection).await;

    for (handle, password) in [
        (OTHER_HANDLE, OTHER_PASSWORD),
        (ADMIN_HANDLE, ADMIN_PASSWORD),
    ] {
        let actor = journey.login(handle, password).await;

        // Own root is empty and contains none of the owner's entries.
        let own_root = journey.list_root(&actor).await;
        assert_eq!(
            own_root["items"].as_array().unwrap().len(),
            0,
            "{handle} sees an empty private root"
        );

        // The owner's file bytes are undiscoverable via content GET/HEAD.
        for method in ["GET", "HEAD"] {
            let response = journey
                .request(
                    method,
                    &format!("/entries/{secret_id}/content"),
                    Some(&actor.cookie),
                    &[],
                    Vec::new(),
                )
                .await;
            assert_eq!(response.status, 404, "{handle} {method} owner file");
        }

        // The owner's folder is undiscoverable via the folder-children route.
        let response = journey
            .request(
                "GET",
                &format!("/folders/{folder_id}/children"),
                Some(&actor.cookie),
                &[],
                Vec::new(),
            )
            .await;
        assert_eq!(response.status, 404, "{handle} list owner folder");

        // The owner's root is undiscoverable by explicit ID.
        let response = journey
            .request(
                "GET",
                &format!("/folders/{root_id}/children"),
                Some(&actor.cookie),
                &[],
                Vec::new(),
            )
            .await;
        assert_eq!(response.status, 404, "{handle} list owner root");
    }

    // The owner still reads their own bytes after the isolation seeding.
    let response = journey.download(&owner, &secret_id).await;
    assert_eq!(response.body.as_ref(), secret.as_slice());

    journey.close().await;
}

#[tokio::test]
async fn account_management_is_authorized_and_revokes_sessions() {
    let journey = Journey::new("account-management").await;
    journey.setup_admin().await;
    let admin = journey.login(OWNER_HANDLE, OWNER_PASSWORD).await;
    let headers = |auth: &Auth| {
        vec![
            ("Origin", journey.origin.clone()),
            ("X-CSRF-Token", auth.csrf.clone()),
            ("Content-Type", "application/json".to_owned()),
        ]
    };
    let create = journey
        .request(
            "POST",
            "/accounts",
            Some(&admin.cookie),
            &headers(&admin),
            serde_json::to_vec(&json!({"handle": OTHER_HANDLE, "password": OTHER_PASSWORD}))
                .unwrap(),
        )
        .await;
    assert_eq!(create.status, 204);
    let accounts = journey
        .request("GET", "/accounts", Some(&admin.cookie), &[], vec![])
        .await;
    assert_eq!(accounts.status, 200);
    let id = accounts
        .json()
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["handle"] == OTHER_HANDLE)
        .unwrap()["account_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let owner_root = journey.list_root(&admin).await;
    let root_id = owner_root["root"]["id"].as_str().unwrap();
    let root_etag = owner_root["root"]["etag"].as_str().unwrap();
    let secret = journey
        .upload(&admin, root_id, root_etag, "private.txt", b"private bytes")
        .await;
    let secret_id = secret["id"].as_str().unwrap();
    let user = journey.login(OTHER_HANDLE, OTHER_PASSWORD).await;
    assert_eq!(
        journey
            .request(
                "GET",
                &format!("/folders/{root_id}/children"),
                Some(&user.cookie),
                &[],
                vec![]
            )
            .await
            .status,
        404
    );
    assert_eq!(
        journey
            .request(
                "GET",
                &format!("/entries/{secret_id}/content"),
                Some(&user.cookie),
                &[],
                vec![]
            )
            .await
            .status,
        404
    );
    assert_eq!(
        journey.list_root(&user).await["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        journey
            .request("GET", "/accounts", Some(&user.cookie), &[], vec![])
            .await
            .status,
        403
    );
    assert_eq!(
        journey
            .request(
                "POST",
                "/accounts",
                Some(&user.cookie),
                &headers(&user),
                serde_json::to_vec(&json!({"handle":"intruder", "password":OTHER_PASSWORD}))
                    .unwrap()
            )
            .await
            .status,
        403
    );
    let new_password = "a new password for this account";
    let reset = journey
        .request(
            "POST",
            &format!("/accounts/{id}/reset"),
            Some(&admin.cookie),
            &headers(&admin),
            serde_json::to_vec(&json!({"password":new_password})).unwrap(),
        )
        .await;
    assert_eq!(reset.status, 204);
    assert_eq!(
        journey
            .request("GET", "/me", Some(&user.cookie), &[], vec![])
            .await
            .status,
        401
    );
    let user = journey.login(OTHER_HANDLE, new_password).await;
    let changed_password = "another password for this user";
    let change = journey
        .request(
            "POST",
            "/me/password",
            Some(&user.cookie),
            &headers(&user),
            serde_json::to_vec(
                &json!({"current_password":new_password,"new_password":changed_password}),
            )
            .unwrap(),
        )
        .await;
    assert_eq!(change.status, 204);
    assert_eq!(
        journey
            .request("GET", "/me", Some(&user.cookie), &[], vec![])
            .await
            .status,
        401
    );
    let user = journey.login(OTHER_HANDLE, changed_password).await;
    let disable = journey
        .request(
            "POST",
            &format!("/accounts/{id}/disable"),
            Some(&admin.cookie),
            &headers(&admin),
            vec![],
        )
        .await;
    assert_eq!(disable.status, 204);
    assert_eq!(
        journey
            .request("GET", "/me", Some(&user.cookie), &[], vec![])
            .await
            .status,
        401
    );
    journey.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_disable_and_reset_block_in_flight_upload_commits() {
    let journey = Journey::new("account-upload-revocation").await;
    journey.setup_admin().await;
    let admin = journey.login(OWNER_HANDLE, OWNER_PASSWORD).await;
    let headers = |auth: &Auth| {
        vec![
            ("Origin", journey.origin.clone()),
            ("X-CSRF-Token", auth.csrf.clone()),
            ("Content-Type", "application/json".to_owned()),
        ]
    };
    let created = journey
        .request(
            "POST",
            "/accounts",
            Some(&admin.cookie),
            &headers(&admin),
            serde_json::to_vec(&json!({"handle":OTHER_HANDLE,"password":OTHER_PASSWORD})).unwrap(),
        )
        .await;
    assert_eq!(created.status, 204);
    let list = journey
        .request("GET", "/accounts", Some(&admin.cookie), &[], vec![])
        .await
        .json();
    let id = list
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["handle"] == OTHER_HANDLE)
        .unwrap()["account_id"]
        .as_str()
        .unwrap();
    let mut password = OTHER_PASSWORD;
    for action in ["reset", "disable"] {
        let user = journey.login(OTHER_HANDLE, password).await;
        let root = journey.list_root(&user).await;
        let root_id = root["root"]["id"].as_str().unwrap();
        let etag = root["root"]["etag"].as_str().unwrap();
        let (upload_id, ticket, digest) = journey
            .stage_upload(&user, root_id, etag, "pending.txt", b"pending bytes")
            .await;
        journey.commit_barrier.arm();
        let commit = async {
            journey
                .request(
                    "POST",
                    &format!("/uploads/{upload_id}/commit"),
                    Some(&user.cookie),
                    &[
                        ("Origin", journey.origin.clone()),
                        ("X-CSRF-Token", user.csrf.clone()),
                        ("X-Mutation-ID", ticket.clone()),
                        ("Content-Type", "application/json".to_owned()),
                        ("If-None-Match", "*".to_owned()),
                    ],
                    serde_json::to_vec(&json!({"sha256":digest,"parent_etag":etag})).unwrap(),
                )
                .await
        };
        let revoke = async {
            tokio::time::timeout(Duration::from_secs(5), journey.commit_barrier.wait_paused())
                .await
                .expect("commit did not reach final authorization");
            let body = if action == "reset" {
                serde_json::to_vec(&json!({"password":"reset password for this user"})).unwrap()
            } else {
                vec![]
            };
            let result = journey
                .request(
                    "POST",
                    &format!("/accounts/{id}/{action}"),
                    Some(&admin.cookie),
                    &headers(&admin),
                    body,
                )
                .await;
            journey.commit_barrier.release();
            result
        };
        let (commit, revoke) = tokio::join!(commit, revoke);
        assert_eq!(revoke.status, 204);
        assert_eq!(commit.status, 401);
        let count: i64 = journey.db.as_ref().unwrap().lock().unwrap().query_row("SELECT count(*) FROM entry WHERE owner_account_id=?1 AND original_name='pending.txt'", [id], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
        password = "reset password for this user";
    }
    journey.close().await;
}

#[tokio::test]
async fn trash_restores_bytes_and_refuses_other_accounts_and_conflicts() {
    let mut journey = Journey::new("trash").await;
    journey.setup_admin().await;
    let owner = journey.login(OWNER_HANDLE, OWNER_PASSWORD).await;
    let root = journey.list_root(&owner).await;
    let root_id = root["root"]["id"].as_str().unwrap().to_owned();
    let etag = root["root"]["etag"].as_str().unwrap();
    let bytes = b"private recovery bytes";
    let file = journey
        .upload(&owner, &root_id, etag, "recover.bin", bytes)
        .await;
    let id = file["id"].as_str().unwrap().to_owned();
    let listing = journey.list_root(&owner).await;
    let etag = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == id)
        .unwrap()["etag"]
        .as_str()
        .unwrap()
        .to_owned();
    let headers = [
        ("Origin", journey.origin.clone()),
        ("X-CSRF-Token", owner.csrf.clone()),
        ("If-Match", etag),
    ];
    let result = journey
        .request(
            "DELETE",
            &format!("/entries/{id}"),
            Some(&owner.cookie),
            &headers,
            Vec::new(),
        )
        .await;
    assert_eq!(result.status, 204);
    assert_eq!(
        journey
            .request(
                "GET",
                &format!("/entries/{id}/content"),
                Some(&owner.cookie),
                &[],
                Vec::new()
            )
            .await
            .status,
        404
    );
    let trash = journey
        .request("GET", "/trash", Some(&owner.cookie), &[], Vec::new())
        .await;
    let trashed = trash.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == id)
        .unwrap()
        .clone();
    let trash_etag = trashed["etag"].as_str().unwrap().to_owned();
    let create_headers = [
        ("Origin", journey.origin.clone()),
        ("X-CSRF-Token", owner.csrf.clone()),
        ("Content-Type", "application/json".to_owned()),
    ];
    assert_eq!(
        journey
            .request(
                "POST",
                "/accounts",
                Some(&owner.cookie),
                &create_headers,
                serde_json::to_vec(&json!({"handle": OTHER_HANDLE, "password": OTHER_PASSWORD}))
                    .unwrap()
            )
            .await
            .status,
        204
    );
    journey.restart().await;
    let other = journey.login(OTHER_HANDLE, OTHER_PASSWORD).await;
    let other_headers = [
        ("Origin", journey.origin.clone()),
        ("X-CSRF-Token", other.csrf.clone()),
        ("If-Match", trash_etag.clone()),
    ];
    assert_eq!(
        journey
            .request(
                "POST",
                &format!("/trash/{id}/restore"),
                Some(&other.cookie),
                &other_headers,
                Vec::new()
            )
            .await
            .status,
        404
    );
    assert!(
        journey
            .request("GET", "/trash", Some(&other.cookie), &[], Vec::new())
            .await
            .json()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let owner = journey.login(OWNER_HANDLE, OWNER_PASSWORD).await;
    journey.create_folder(&owner, &root_id, "recover.bin").await;
    let restore_headers = [
        ("Origin", journey.origin.clone()),
        ("X-CSRF-Token", owner.csrf.clone()),
        ("If-Match", trash_etag),
    ];
    assert_eq!(
        journey
            .request(
                "POST",
                &format!("/trash/{id}/restore"),
                Some(&owner.cookie),
                &restore_headers,
                Vec::new()
            )
            .await
            .status,
        409
    );
    let conflict = journey.list_root(&owner).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "recover.bin")
        .unwrap()
        .clone();
    let conflict_headers = [
        ("Origin", journey.origin.clone()),
        ("X-CSRF-Token", owner.csrf.clone()),
        ("If-Match", conflict["etag"].as_str().unwrap().to_owned()),
    ];
    assert_eq!(
        journey
            .request(
                "DELETE",
                &format!("/entries/{}", conflict["id"].as_str().unwrap()),
                Some(&owner.cookie),
                &conflict_headers,
                Vec::new()
            )
            .await
            .status,
        204
    );
    assert_eq!(
        journey
            .request(
                "POST",
                &format!("/trash/{id}/restore"),
                Some(&owner.cookie),
                &restore_headers,
                Vec::new()
            )
            .await
            .status,
        204
    );
    assert_eq!(journey.download(&owner, &id).await.body.as_ref(), bytes);
    journey.restart().await;
    let owner = journey.login(OWNER_HANDLE, OWNER_PASSWORD).await;
    assert_eq!(journey.download(&owner, &id).await.body.as_ref(), bytes);
    journey.close().await;
}
