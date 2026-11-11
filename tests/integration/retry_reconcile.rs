//! Restart/unknown-result integration cases (T020; design §5).
//! Each recovery assertion calls the product reconciliation API over a fresh
//! SQLite worker. The stub is deliberately RED until T021 supplies recovery.

#[path = "../common/mod.rs"]
mod common;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use common::FixtureRoot;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use sylux::{
    domain::authz::verifier,
    http::{self, AuthState},
    persistence::{
        reconcile::{self, RecoveryDisposition},
        root,
        sqlite::{self, SqliteWorkers, WorkerConfig},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

const OWNER: &str = "00000000000000000000000000000001";
const OWNER_ROOT: &str = "00000000000000000000000000000101";
const SESSION: &str = "00000000000000000000000000000201";
const SECRET: &[u8; 32] = b"p5-owner-secret-0123456789abcdef";
const CSRF: &[u8; 32] = b"p5-owner-csrf-0123456789abcdef!!";
const SETUP_SECRET: &str = "t020 one-use setup authorization";
const BUSY: Duration = Duration::from_millis(300);

struct App {
    owner: FixtureRoot,
    product: PathBuf,
    db_path: PathBuf,
    db: Option<Arc<Mutex<Connection>>>,
    workers: Option<SqliteWorkers>,
    address: String,
    origin: String,
    server: Option<JoinHandle<()>>,
}

impl App {
    async fn seeded(case: &str) -> Self {
        Self::new(case, true, None).await
    }

    async fn setup(case: &str) -> Self {
        Self::new(case, false, Some(SETUP_SECRET)).await
    }

    async fn new(case: &str, seeded: bool, setup_secret: Option<&str>) -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("t020-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "retry-reconcile", case).unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).unwrap();
        let db_path = product.join("metadata/sylux.sqlite3");
        let connection = sqlite::open_writer(&db_path, BUSY).unwrap();
        if seeded {
            seed_inventory(&connection);
        }
        if let Some(secret) = setup_secret {
            connection
                .execute(
                    "UPDATE installation SET setup_verifier=?1",
                    [Sha256::digest(secret.as_bytes()).as_slice()],
                )
                .unwrap();
        }
        let mut app = Self {
            owner,
            product,
            db_path,
            db: None,
            workers: None,
            address: String::new(),
            origin: String::new(),
            server: None,
        };
        app.start(connection, setup_secret).await;
        app
    }

    async fn start(&mut self, connection: Connection, setup_secret: Option<&str>) {
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
        let state = AuthState::new_with_setup(
            workers.clone(),
            self.origin.clone(),
            setup_secret.unwrap_or_default().to_owned(),
        )
        .with_storage_root(&self.product)
        .unwrap();
        let router = http::router(state);
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }));
        self.db = Some(db);
        self.workers = Some(workers);
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: &[u8],
    ) -> Response {
        let mut stream = TcpStream::connect(&self.address).await.unwrap();
        let mut wire = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.address,
            body.len()
        );
        for (name, value) in headers {
            wire.push_str(&format!("{name}: {value}\r\n"));
        }
        wire.push_str("\r\n");
        stream.write_all(wire.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        Response::parse(&bytes)
    }

    fn auth_headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("Origin", self.origin.clone()),
            (
                "Cookie",
                format!("__Host-sylux={SESSION}.{}", URL_SAFE_NO_PAD.encode(SECRET)),
            ),
            ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(CSRF)),
        ]
    }

    async fn stage(&self, name: &str, bytes: &[u8]) -> (String, String) {
        let digest = hex(&Sha256::digest(bytes));
        let mut headers = self.auth_headers();
        headers.push(("Content-Type", "application/json".into()));
        let ticket_response = self
            .request(
                "POST",
                "/mutation-tickets",
                &headers,
                json!({
                    "scope":"upload_create", "parent_id":OWNER_ROOT,
                    "parent_etag":etag(OWNER_ROOT, 1), "name":name,
                    "length":bytes.len(), "sha256":digest
                })
                .to_string()
                .as_bytes(),
            )
            .await;
        assert_eq!(ticket_response.status, 201, "{}", ticket_response.text());
        let ticket = ticket_response
            .header("x-mutation-id")
            .expect("signed ticket")
            .to_owned();
        let mut admit_headers = self.auth_headers();
        admit_headers.extend([
            ("Content-Type", "application/json".into()),
            ("X-Mutation-ID", ticket.clone()),
            ("If-None-Match", "*".into()),
        ]);
        let admitted = self
            .request(
                "POST",
                "/uploads",
                &admit_headers,
                json!({
                    "parent_id":OWNER_ROOT,"parent_etag":etag(OWNER_ROOT, 1),"name":name,
                    "length":bytes.len(),"sha256":digest
                })
                .to_string()
                .as_bytes(),
            )
            .await;
        assert_eq!(admitted.status, 201, "{}", admitted.text());
        let upload_id = admitted.json()["upload_id"].as_str().unwrap().to_owned();
        let mut content_headers = self.auth_headers();
        content_headers.extend([
            ("Content-Type", "application/octet-stream".into()),
            ("X-Mutation-ID", ticket.clone()),
        ]);
        let staged = self
            .request(
                "PUT",
                &format!("/uploads/{upload_id}/content"),
                &content_headers,
                bytes,
            )
            .await;
        assert_eq!(staged.status, 200, "{}", staged.text());
        (upload_id, ticket)
    }

    async fn commit(&self, upload_id: &str, ticket: &str, bytes: &[u8]) -> Response {
        let mut headers = self.auth_headers();
        headers.extend([
            ("Content-Type", "application/json".into()),
            ("X-Mutation-ID", ticket.into()),
            ("If-None-Match", "*".into()),
        ]);
        self.request(
            "POST",
            &format!("/uploads/{upload_id}/commit"),
            &headers,
            json!({
                "sha256":hex(&Sha256::digest(bytes)),"parent_etag":etag(OWNER_ROOT, 1)
            })
            .to_string()
            .as_bytes(),
        )
        .await
    }

    async fn restart_and_reconcile(&mut self) -> reconcile::ReconcileReport {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        if let Some(workers) = self.workers.take() {
            workers.shutdown().await.unwrap();
        }
        self.db.take();
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
        self.db = Some(Arc::new(Mutex::new(
            sqlite::open_writer(&self.db_path, BUSY).unwrap(),
        )));
        match reconcile::reconcile(&workers, &self.product).await {
            Ok(report) => {
                if report.ready {
                    workers.shutdown().await.unwrap();
                    let connection = sqlite::open_writer(&self.db_path, BUSY).unwrap();
                    self.start(connection, None).await;
                } else {
                    self.workers = Some(workers);
                }
                report
            }
            Err(error) => {
                workers.shutdown().await.unwrap();
                self.cleanup_root();
                panic!("reconcile returned error: {error:?}");
            }
        }
    }

    fn cleanup_root(&self) {
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
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
}

fn seed_inventory(connection: &Connection) {
    let now = now();
    connection.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'owner','user',1,0)", [OWNER]).unwrap();
    connection.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)", params![OWNER_ROOT, OWNER]).unwrap();
    connection.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?4,?5,?6,?7,0)", params![SESSION, OWNER, verifier(SECRET).as_slice(), verifier(CSRF).as_slice(), now, now + 3600, now + 7200]).unwrap();
}

fn etag(id: &str, generation: u64) -> String {
    format!("\"{id}:{generation}\"")
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct Response {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
impl Response {
    fn parse(bytes: &[u8]) -> Self {
        let text = String::from_utf8_lossy(bytes);
        let (head, body) = text.split_once("\r\n\r\n").expect("HTTP response");
        let status = head
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = head
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        Self {
            status,
            headers,
            body: body.as_bytes().to_vec(),
        }
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

fn assert_disposition(report: &reconcile::ReconcileReport, expected: RecoveryDisposition) {
    assert!(
        report.dispositions.contains(&expected),
        "missing {expected:?} from {report:?}"
    );
}

#[tokio::test]
async fn restart_publishes_staging_candidate_before_same_ticket_commit() {
    let mut app = App::seeded("staging-candidate-retry").await;
    let bytes = b"staged bytes, not a visible entry";
    let (upload_id, ticket) = app.stage("staged.bin", bytes).await;
    let staging_name: String = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT staging_path FROM upload_attempt WHERE mutation_id=?1",
            [upload_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let blob_id = staging_name.split_once('-').unwrap().1;
    let final_path = app
        .product
        .join("payloads")
        .join(&blob_id[..2])
        .join(blob_id);
    let staging_path = app.product.join("staging").join(&staging_name);
    fs::rename(&final_path, &staging_path).unwrap();

    let report = app.restart_and_reconcile().await;
    assert!(report.ready, "{report:?}");
    assert_disposition(&report, RecoveryDisposition::FinalNeedsResync);
    assert!(
        final_path.is_file(),
        "recovery must publish before readiness"
    );
    assert_eq!(fs::read(&final_path).unwrap(), bytes);
    assert!(!staging_path.exists());

    app.db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .execute(
            "UPDATE session SET revoked=1 WHERE session_id=?1",
            [SESSION],
        )
        .unwrap();
    let unauthorized = app.commit(&upload_id, &ticket, bytes).await;
    assert_eq!(unauthorized.status, 401, "{}", unauthorized.text());
    let committed: i64 = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM blob WHERE blob_id=?1",
            [blob_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        committed, 0,
        "revoked authorization cannot reference payload"
    );

    app.db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .execute(
            "UPDATE session SET revoked=0 WHERE session_id=?1",
            [SESSION],
        )
        .unwrap();
    app.db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .execute(
            "UPDATE entry SET generation=2 WHERE entry_id=?1",
            [OWNER_ROOT],
        )
        .unwrap();
    let stale = app.commit(&upload_id, &ticket, bytes).await;
    assert_eq!(stale.status, 412, "{}", stale.text());
    let committed: i64 = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM blob WHERE blob_id=?1",
            [blob_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(committed, 0, "stale precondition cannot reference payload");

    app.db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .execute(
            "UPDATE entry SET generation=1 WHERE entry_id=?1",
            [OWNER_ROOT],
        )
        .unwrap();
    let committed = app.commit(&upload_id, &ticket, bytes).await;
    assert_eq!(committed.status, 201, "{}", committed.text());
    assert_eq!(fs::read(&final_path).unwrap(), bytes);
    let references: i64 = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM blob b JOIN revision r ON r.blob_id=b.blob_id WHERE b.blob_id=?1",
            [blob_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(references, 1);
    app.close().await;
}

#[tokio::test]
async fn restart_preserves_valid_final_candidate_for_same_operation() {
    let mut app = App::seeded("staged-preserve").await;
    let bytes = b"staged bytes, not a visible entry";
    let _ = app.stage("staged.bin", bytes).await;
    let report = app.restart_and_reconcile().await;
    assert!(report.ready);
    assert_disposition(&report, RecoveryDisposition::FinalNeedsResync);
    app.close().await;
}

#[tokio::test]
async fn restart_replays_committed_result_after_caller_drops_response() {
    let mut app = App::seeded("response-drop").await;
    let bytes = b"durable result survives a lost response";
    let (upload_id, ticket) = app.stage("response.bin", bytes).await;
    // The HTTP request/commit completes, but the caller deliberately discards
    // its response value: deterministic application-level result loss.
    let lost_response = app.commit(&upload_id, &ticket, bytes).await;
    assert_eq!(lost_response.status, 201, "{}", lost_response.text());
    drop(lost_response);
    let report = app.restart_and_reconcile().await;
    assert!(report.ready);
    assert_disposition(&report, RecoveryDisposition::CommittedReplay);
    let count: i64 = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM entry WHERE kind='file' AND original_name='response.bin'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "response loss must not duplicate the domain effect"
    );
    app.close().await;
}

#[tokio::test]
async fn changed_staged_digest_fails_closed_without_removing_candidate() {
    let mut app = App::seeded("changed-digest").await;
    let bytes = b"candidate bytes";
    let (upload_id, _) = app.stage("digest.bin", bytes).await;
    let staging_path: String = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT staging_path FROM upload_attempt WHERE mutation_id=?1",
            [upload_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    let blob_id = staging_path.split_once('-').unwrap().1;
    let path = app
        .product
        .join("payloads")
        .join(&blob_id[..2])
        .join(blob_id);
    fs::write(&path, b"tampered candidate").unwrap();
    let before = fs::read(&path).unwrap();
    let report = app.restart_and_reconcile().await;
    assert!(!report.ready);
    assert_disposition(&report, RecoveryDisposition::RecoveryRequired);
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "recovery must preserve suspect bytes"
    );
    app.close().await;
}

#[tokio::test]
async fn expired_unresolved_upload_is_retained_and_never_reexecuted() {
    let mut app = App::seeded("expired-pending").await;
    let bytes = b"expires but is unresolved";
    let (upload_id, _) = app.stage("expired.bin", bytes).await;
    let staging_name: String = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT staging_path FROM upload_attempt WHERE mutation_id=?1",
            [upload_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    let blob_id = staging_name.split_once('-').unwrap().1;
    let candidate = app
        .product
        .join("payloads")
        .join(&blob_id[..2])
        .join(blob_id);
    let candidate_before = fs::read(&candidate).unwrap();
    let issued_at: i64 = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT issued_at FROM mutation WHERE mutation_id=?1",
            [upload_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    app.db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .execute(
            "UPDATE mutation SET expires_at=?2 WHERE mutation_id=?1",
            rusqlite::params![upload_id.as_str(), issued_at],
        )
        .unwrap();
    let report = app.restart_and_reconcile().await;
    assert_disposition(&report, RecoveryDisposition::ExpiredRejected);
    let retained: i64 = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM upload_attempt WHERE mutation_id=?1",
            [upload_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(retained, 1, "expiry is not a cleanup clock");
    assert_eq!(
        fs::read(&candidate).unwrap(),
        candidate_before,
        "unresolved candidate survives ticket expiry"
    );
    app.close().await;
}

#[tokio::test]
async fn setup_response_loss_restart_keeps_single_protected_administrator() {
    let mut app = App::setup("setup-lost-response").await;
    let response = app
        .request(
            "POST",
            "/setup/admin",
            &[
                ("Origin", app.origin.clone()),
                ("X-Setup-Authorization", SETUP_SECRET.into()),
                ("Content-Type", "application/json".into()),
            ],
            br#"{"handle":"owner","password":"deterministic test password"}"#,
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.text());
    // The caller discards the response and restarts without consuming its result.
    drop(response);
    let report = app.restart_and_reconcile().await;
    assert!(report.ready);
    let (accounts, roots, consumed): (i64,i64,i64) = app.db.as_ref().unwrap().lock().unwrap().query_row("SELECT (SELECT count(*) FROM account),(SELECT count(*) FROM entry WHERE parent_id IS NULL),setup_consumed FROM installation WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!((accounts, roots, consumed), (1, 1, 1));
    app.close().await;
}

#[tokio::test]
async fn namespace_response_loss_does_not_duplicate_create_after_restart() {
    let mut app = App::seeded("namespace-response-loss").await;
    let headers = [
        ("Origin", app.origin.clone()),
        (
            "Cookie",
            format!("__Host-sylux={SESSION}.{}", URL_SAFE_NO_PAD.encode(SECRET)),
        ),
        ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(CSRF)),
        ("Content-Type", "application/json".into()),
    ];
    let created = app
        .request(
            "POST",
            &format!("/folders/{OWNER_ROOT}/children"),
            &headers,
            br#"{"name":"lost-result-folder"}"#,
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.text());
    drop(created);
    let report = app.restart_and_reconcile().await;
    assert!(report.ready);
    let count: i64 = app.db.as_ref().unwrap().lock().unwrap().query_row("SELECT count(*) FROM entry WHERE parent_id=?1 AND original_name='lost-result-folder' AND state='active'", [OWNER_ROOT], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
    app.close().await;
}

#[tokio::test]
async fn missing_published_candidate_is_recovery_required_and_record_is_retained() {
    let mut app = App::seeded("missing-candidate").await;
    let bytes = b"candidate removed while stopped";
    let (upload_id, _) = app.stage("missing.bin", bytes).await;
    let staging_name: String = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT staging_path FROM upload_attempt WHERE mutation_id=?1",
            [upload_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    let blob_id = staging_name.split_once('-').unwrap().1;
    let candidate = app
        .product
        .join("payloads")
        .join(&blob_id[..2])
        .join(blob_id);
    fs::remove_file(&candidate).unwrap();
    let report = app.restart_and_reconcile().await;
    assert!(!report.ready);
    assert_disposition(&report, RecoveryDisposition::RecoveryRequired);
    assert!(!candidate.exists());
    let rows: i64 = app.db.as_ref().unwrap().lock().unwrap().query_row(
        "SELECT count(*) FROM mutation WHERE mutation_id=?1 AND state IN ('admitted','publishing')", [upload_id.as_str()], |r| r.get(0)
    ).unwrap();
    assert_eq!(
        rows, 1,
        "missing evidence cannot be converted into a compensating rejection"
    );
    app.close().await;
}

#[tokio::test]
async fn duplicate_staged_and_final_candidate_paths_are_both_preserved() {
    let mut app = App::seeded("duplicate-candidate").await;
    let bytes = b"duplicate candidate inventory";
    let (upload_id, _) = app.stage("duplicate.bin", bytes).await;
    let staging_name: String = app
        .db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row(
            "SELECT staging_path FROM upload_attempt WHERE mutation_id=?1",
            [upload_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    let blob_id = staging_name.split_once('-').unwrap().1;
    let final_path = app
        .product
        .join("payloads")
        .join(&blob_id[..2])
        .join(blob_id);
    let staged_path = app.product.join("staging").join(&staging_name);
    fs::copy(&final_path, &staged_path).unwrap();
    let before = (
        fs::read(&staged_path).unwrap(),
        fs::read(&final_path).unwrap(),
    );
    let report = app.restart_and_reconcile().await;
    assert!(!report.ready);
    assert_disposition(&report, RecoveryDisposition::RecoveryRequired);
    assert_eq!(fs::read(&staged_path).unwrap(), before.0);
    assert_eq!(fs::read(&final_path).unwrap(), before.1);
    app.close().await;
}

#[tokio::test]
async fn revoked_principal_cannot_retry_final_candidate_after_restart() {
    let mut app = App::seeded("revoked-retry").await;
    let bytes = b"staged before revocation";
    let (upload_id, ticket) = app.stage("revoked.bin", bytes).await;
    app.db
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .execute(
            "UPDATE session SET revoked=1 WHERE session_id=?1",
            [SESSION],
        )
        .unwrap();
    let mut headers = app.auth_headers();
    headers.extend([
        ("Content-Type", "application/json".into()),
        ("X-Mutation-ID", ticket),
        ("If-None-Match", "*".into()),
    ]);
    let retry = app
        .request(
            "POST",
            &format!("/uploads/{upload_id}/commit"),
            &headers,
            json!({
                "sha256":hex(&Sha256::digest(bytes)),"parent_etag":etag(OWNER_ROOT, 1)
            })
            .to_string()
            .as_bytes(),
        )
        .await;
    assert_eq!(
        retry.status, 401,
        "revoked authorization cannot replay or execute"
    );
    let report = app.restart_and_reconcile().await;
    assert_disposition(&report, RecoveryDisposition::FinalNeedsResync);
    assert!(
        app.product.join("payloads").exists(),
        "no cleanup follows revoked retry"
    );
    app.close().await;
}
