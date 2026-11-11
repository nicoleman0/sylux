//! T032 local-session lifecycle over the real loopback-TLS router and SQLite.
#[path = "../common/mod.rs"]
mod common;

use argon2::{Argon2, password_hash::PasswordHasher};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{
    FixtureRoot,
    server::{TlsApiServer, TlsFiles, tls_connect},
};
use rusqlite::params;
use serde_json::Value;
use std::{
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

const ACCOUNT: &str = "00000000000000000000000000000001";
const HANDLE: &str = "lifecycle-owner";
const PASSWORD: &str = "local lifecycle test password";
const BUSY: Duration = Duration::from_millis(375);

struct Fixture {
    root: FixtureRoot,
    workers: Option<SqliteWorkers>,
    server: Option<TlsApiServer>,
    ca: Vec<u8>,
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn cookie(&self) -> String {
        self.header("set-cookie")
            .expect("response must set cookie")
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

struct Session {
    cookie: String,
    csrf: String,
}

impl Fixture {
    async fn new() -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-lifecycle-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let root = FixtureRoot::create(&parent, "lifecycle", "auth").unwrap();
        let tls = TlsFiles::generate(&root, "tls").unwrap();
        let ca = fs::read(&tls.ca_der).unwrap();
        let product = root.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).unwrap();
        let db_path = product.join("metadata/sylux.sqlite3");
        let db = sqlite::open_writer(&db_path, BUSY).unwrap();
        let password_verifier = Argon2::default()
            .hash_password(PASSWORD.as_bytes())
            .unwrap()
            .to_string();
        db.execute(
            "INSERT INTO account(account_id,handle,role,enabled,credential_epoch,password_verifier) VALUES(?1,?2,'user',1,0,?3)",
            params![ACCOUNT, HANDLE, password_verifier],
        )
        .unwrap();
        let workers = SqliteWorkers::from_writer(
            db,
            &db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let server = TlsApiServer::start(
            &root,
            &tls,
            http::router(AuthState::new(workers.clone(), "https://127.0.0.1".into())),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        Self {
            root,
            workers: Some(workers),
            server: Some(server),
            ca,
        }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        csrf: Option<&str>,
        body: Option<&str>,
    ) -> Response {
        let server = self.server.as_ref().unwrap();
        let mut stream = tls_connect(server.address(), &self.ca, "127.0.0.1")
            .await
            .unwrap();
        let body = body.unwrap_or("");
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nAccept: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
        if let Some(cookie) = cookie {
            request.push_str(&format!("Cookie: {cookie}\r\n"));
        }
        if let Some(csrf) = csrf {
            request.push_str(&format!("X-CSRF-Token: {csrf}\r\n"));
        }
        if method == "POST" {
            request.push_str("Origin: https://127.0.0.1\r\nContent-Type: application/json\r\n");
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        let split = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
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
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
            .collect();
        Response {
            status,
            headers,
            body: bytes[split + 4..].to_vec(),
        }
    }

    async fn login(&self) -> Session {
        let bootstrap = self.request("GET", "/auth/csrf", None, None, None).await;
        assert_eq!(bootstrap.status, 200);
        let anonymous_cookie = bootstrap.cookie();
        let anonymous_csrf = bootstrap.json()["csrf_token"].as_str().unwrap().to_owned();
        let login = self
            .request(
                "POST",
                "/auth/local",
                Some(&anonymous_cookie),
                Some(&anonymous_csrf),
                Some(&serde_json::json!({"handle": HANDLE, "password": PASSWORD}).to_string()),
            )
            .await;
        assert_eq!(
            login.status,
            200,
            "{}",
            String::from_utf8_lossy(&login.body)
        );
        let cookie = login.cookie();
        let csrf = login.json()["csrf_token"].as_str().unwrap().to_owned();
        assert_ne!(
            cookie, anonymous_cookie,
            "login must rotate the pre-session cookie"
        );
        assert_ne!(csrf, anonymous_csrf, "login must rotate the CSRF token");
        Session { cookie, csrf }
    }

    fn workers(&self) -> &SqliteWorkers {
        self.workers.as_ref().unwrap()
    }

    async fn close(mut self) {
        self.server
            .take()
            .unwrap()
            .shutdown_and_join()
            .await
            .unwrap();
        self.workers.take().unwrap().shutdown().await.unwrap();
        let parent = self.root.path().parent().unwrap().to_owned();
        self.root.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(parent).unwrap();
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn cookie_secret(cookie: &str) -> ([u8; 32], String) {
    let value = cookie.split_once('=').unwrap().1;
    let value = value.split(';').next().unwrap();
    let (session_id, encoded) = value.split_once('.').unwrap();
    let secret = URL_SAFE_NO_PAD.decode(encoded).unwrap().try_into().unwrap();
    (secret, session_id.to_owned())
}

#[tokio::test]
async fn login_rotates_pre_session_and_persists_only_session_verifiers() {
    let fixture = Fixture::new().await;
    let session = fixture.login().await;
    let (secret, session_id) = cookie_secret(&session.cookie);
    let expected = verifier(&secret);
    let stored: (Vec<u8>, Vec<u8>, i64) = fixture
        .workers()
        .read(move |db| {
            db.query_row(
                "SELECT verifier,csrf_verifier,credential_epoch FROM session WHERE session_id=?1",
                [&session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
        })
        .await
        .unwrap();
    assert_eq!(stored.0, expected);
    assert_eq!(stored.1.len(), 32);
    assert_eq!(stored.2, 0);
    assert_ne!(stored.0.as_slice(), secret.as_slice());
    assert_eq!(
        fixture
            .request("GET", "/me", Some(&session.cookie), None, None)
            .await
            .status,
        200
    );
    fixture.close().await;
}

#[tokio::test]
async fn logout_revokes_only_the_cookie_session_and_expiry_denies_next_request() {
    let fixture = Fixture::new().await;
    let first = fixture.login().await;
    let second = fixture.login().await;
    assert_ne!(first.cookie, second.cookie);
    let logout = fixture
        .request(
            "POST",
            "/auth/logout",
            Some(&first.cookie),
            Some(&first.csrf),
            None,
        )
        .await;
    assert_eq!(logout.status, 204);
    assert_eq!(
        fixture
            .request("GET", "/me", Some(&first.cookie), None, None)
            .await
            .status,
        401
    );
    assert_eq!(
        fixture
            .request("GET", "/me", Some(&second.cookie), None, None)
            .await
            .status,
        200,
        "logout must revoke only the current browser session"
    );

    let (_, second_id) = cookie_secret(&second.cookie);
    fixture
        .workers()
        .transact(move |tx| {
            tx.execute(
                "UPDATE session SET idle_expires_at=?2 WHERE session_id=?1",
                params![second_id, now() - 1],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        fixture
            .request("GET", "/me", Some(&second.cookie), None, None)
            .await
            .status,
        401,
        "expired idle session must fail on the next request"
    );
    fixture.close().await;
}
