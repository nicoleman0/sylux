//! T029/T030 redacted operator status API checks over real HTTP and SQLite.
#[path = "../common/mod.rs"]
mod common;

use common::FixtureRoot;
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
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

const ADMIN: &str = "00000000000000000000000000000001";
const USER: &str = "00000000000000000000000000000002";
const SECRET: [u8; 32] = [7; 32];
const BUSY: Duration = Duration::from_millis(300);

struct Fixture {
    owner: FixtureRoot,
    address: String,
    admin_cookie: String,
    user_cookie: String,
    workers: Option<SqliteWorkers>,
    server: Option<JoinHandle<()>>,
}

impl Fixture {
    async fn new() -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-operator-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "operator", "status").unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).unwrap();
        let db_path: PathBuf = product.join("metadata/sylux.sqlite3");
        let connection = sqlite::open_writer(&db_path, BUSY).unwrap();
        connection.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'admin','administrator',1,0)", [ADMIN]).unwrap();
        connection.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'user','user',1,0)", [USER]).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let verifier: [u8; 32] = Sha256::digest(SECRET).into();
        for (account, id) in [
            (ADMIN, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            (USER, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ] {
            connection.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?3,?4,?5,?5,0)", params![id, account, verifier.as_slice(), now, now + 3600]).unwrap();
        }
        connection.execute("INSERT INTO event(actor_account_id,action,occurred_at,outcome) VALUES(?1,'status.probe',?2,'ok')", params![ADMIN, now]).unwrap();
        let workers = SqliteWorkers::from_writer(
            connection,
            &db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let app = http::router(AuthState::new(workers.clone(), format!("http://{address}")));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let admin_cookie = "__Host-sylux=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc".to_owned();
        let user_cookie = "__Host-sylux=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc".to_owned();
        Self {
            owner,
            address,
            admin_cookie,
            user_cookie,
            workers: Some(workers),
            server: Some(server),
        }
    }

    async fn get(&self, cookie: Option<&str>) -> (u16, String, String) {
        let mut stream = TcpStream::connect(&self.address).await.unwrap();
        let mut request = format!(
            "GET /operator/status HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
            self.address
        );
        if let Some(cookie) = cookie {
            request.push_str(&format!("Cookie: {cookie}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        let response = String::from_utf8(bytes).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let cache = head
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("cache-control: ")
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        (status, cache, body.to_owned())
    }

    async fn close(mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        if let Some(workers) = self.workers.take() {
            workers.shutdown().await.unwrap();
        }
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }
}

#[tokio::test]
async fn operator_status_is_admin_only_redacted_and_reports_database_policy() {
    let fixture = Fixture::new().await;
    let (status, _, body) = fixture.get(None).await;
    assert_eq!(status, 401);
    assert!(body.contains("authentication_required"));

    let (status, _, body) = fixture.get(Some(&fixture.user_cookie)).await;
    assert_eq!(status, 403);
    assert!(body.contains("administrator_required"));

    let (status, cache, body) = fixture.get(Some(&fixture.admin_cookie)).await;
    assert_eq!(status, 200);
    assert_eq!(cache, "no-store");
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["database_policy"], "verified");
    assert_eq!(value["recent_events"][0]["action"], "status.probe");
    assert_eq!(value["recent_events"][0]["actor_account_id"], ADMIN);
    assert!(!body.contains("product"));
    assert!(!body.contains("sylux.sqlite3"));
    fixture.close().await;
}
