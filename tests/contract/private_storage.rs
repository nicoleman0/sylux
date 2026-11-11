//! S1 private namespace contracts over the validated loopback-TLS product router.
#[path = "../common/mod.rs"]
mod common;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{
    FixtureRoot,
    server::{TlsApiServer, TlsFiles, tls_connect},
};
use rusqlite::params;
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
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
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

const ACCOUNT: &str = "00000000000000000000000000000001";
const ROOT: &str = "00000000000000000000000000000101";
const SESSION: &str = "00000000000000000000000000000201";
const SECRET: &[u8; 32] = b"s1-owner-secret-0123456789abcdef";
const CSRF: &[u8; 32] = b"s1-owner-csrf-0123456789abcdef!!";
const BUSY: Duration = Duration::from_millis(375);

struct Fixture {
    owner: FixtureRoot,
    parent: PathBuf,
    workers: Option<SqliteWorkers>,
    server: TlsApiServer,
    ca: Vec<u8>,
}
impl Fixture {
    async fn new(label: &str) -> Self {
        Self::new_with_entries(label, false).await
    }
    async fn new_with_entries(label: &str, with_entries: bool) -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-s1-ns-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "namespace", label).unwrap();
        let tls = TlsFiles::generate(&owner, "tls").unwrap();
        let ca = fs::read(&tls.ca_der).unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).unwrap();
        let db_path = product.join("metadata/sylux.sqlite3");
        let db = sqlite::open_writer(&db_path, BUSY).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        db.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'synthetic-owner','user',1,0)", [ACCOUNT]).unwrap();
        db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)", params![ROOT, ACCOUNT]).unwrap();
        if with_entries {
            db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES('00000000000000000000000000000111',?1,?2,'folder','alpha',x'616c706861',1,1),('00000000000000000000000000000112',?1,?2,'folder','beta',x'62657461',1,1)", params![ACCOUNT, ROOT]).unwrap();
        }
        db.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?4,?5,?6,?7,0)", params![SESSION, ACCOUNT, verifier(SECRET).as_slice(), verifier(CSRF).as_slice(), now, now+3600, now+7200]).unwrap();
        let workers = SqliteWorkers::from_writer(
            db,
            &db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let state = AuthState::new(workers.clone(), "https://127.0.0.1".to_owned())
            .with_storage_root(&product)
            .unwrap();
        let server = TlsApiServer::start(&owner, &tls, http::router(state), Duration::from_secs(2))
            .await
            .unwrap();
        Self {
            owner,
            parent,
            workers: Some(workers),
            server,
            ca,
        }
    }
    async fn request(&self, method: &str, path: &str, extra: &str, body: &str) -> (u16, Vec<u8>) {
        let mut stream = tls_connect(self.server.address(), &self.ca, "127.0.0.1")
            .await
            .unwrap();
        let cookie = format!(
            "__Host-sylux={}.{}",
            SESSION,
            URL_SAFE_NO_PAD.encode(SECRET)
        );
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nCookie: {cookie}\r\nAccept: application/json\r\nConnection: close\r\n{extra}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(3), stream.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let status = std::str::from_utf8(&bytes[..split])
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (status, bytes[split + 4..].to_vec())
    }
    async fn close(mut self) {
        self.server.shutdown_and_join().await.unwrap();
        self.workers.take().unwrap().shutdown().await.unwrap();
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.parent).unwrap();
    }
}
fn mutation_headers() -> String {
    format!(
        "Origin: https://127.0.0.1\r\nX-CSRF-Token: {}\r\nContent-Type: application/json\r\n",
        URL_SAFE_NO_PAD.encode(CSRF)
    )
}

#[tokio::test]
async fn missing_folder_is_not_disclosed_as_an_empty_listing() {
    let fixture = Fixture::new("missing-folder").await;
    let (status, body) = fixture
        .request(
            "GET",
            "/folders/00000000000000000000000000000999/children",
            "",
            "",
        )
        .await;
    assert_eq!(
        status, 404,
        "unknown folder must be reported as not found, not an empty listing"
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "not_found"
    );
    fixture.close().await;
}

#[tokio::test]
async fn private_root_listing_returns_an_empty_page_with_generation() {
    let fixture = Fixture::new("empty-listing").await;
    let (status, body) = fixture.request("GET", "/folders/children", "", "").await;
    assert_eq!(status, 200, "an existing empty private root is listable");
    let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 0);
    assert_eq!(page["root"]["id"], ROOT);
    assert_eq!(page["root"]["generation"], 1);
    assert_eq!(page["has_more"], false);
    fixture.close().await;
}

#[tokio::test]
async fn rename_without_name_precondition_is_rejected_before_mutation() {
    let fixture = Fixture::new("rename-precondition").await;
    let (status, body) = fixture
        .request(
            "PATCH",
            &format!("/entries/{ROOT}"),
            &mutation_headers(),
            r#"{"name":"renamed"}"#,
        )
        .await;
    assert_eq!(status, 428, "name-only rename still requires If-Match");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "if_match_required"
    );
    fixture.close().await;
}

#[tokio::test]
async fn stale_if_match_returns_412_without_renaming_or_advancing_generation() {
    let fixture = Fixture::new_with_entries("stale-if-match", true).await;
    let path = "/entries/00000000000000000000000000000111";
    let extra = format!(
        "{}If-Match: \"00000000000000000000000000000111:99\"\r\n",
        mutation_headers()
    );
    let (status, body) = fixture
        .request("PATCH", path, &extra, r#"{"name":"renamed"}"#)
        .await;
    assert_eq!(status, 412);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "stale_etag"
    );
    let (status, body) = fixture.request("GET", "/folders/children", "", "").await;
    assert_eq!(status, 200);
    let listing: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(listing["root"]["generation"], 1);
    assert_eq!(listing["items"][0]["name"], "alpha");
    assert_eq!(listing["items"][0]["generation"], 1);
    fixture.close().await;
}

#[tokio::test]
async fn duplicate_sibling_name_returns_409_without_changing_listing() {
    let fixture = Fixture::new_with_entries("duplicate-name", true).await;
    let path = "/entries/00000000000000000000000000000112";
    let extra = format!(
        "{}If-Match: \"00000000000000000000000000000112:1\"\r\n",
        mutation_headers()
    );
    let (status, body) = fixture
        .request("PATCH", path, &extra, r#"{"name":"alpha"}"#)
        .await;
    assert_eq!(status, 409);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "name_conflict"
    );
    let (status, body) = fixture.request("GET", "/folders/children", "", "").await;
    assert_eq!(status, 200);
    let listing: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(listing["root"]["generation"], 1);
    assert_eq!(listing["items"].as_array().unwrap().len(), 2);
    assert_eq!(listing["items"][0]["name"], "alpha");
    assert_eq!(listing["items"][1]["name"], "beta");
    assert_eq!(listing["items"][0]["generation"], 1);
    assert_eq!(listing["items"][1]["generation"], 1);
    fixture.close().await;
}

#[tokio::test]
async fn direct_overwrite_and_legacy_move_methods_are_unavailable() {
    let fixture = Fixture::new("excluded-operations").await;
    for (method, path, headers, body) in [
        (
            "PUT",
            "/entries/00000000000000000000000000000111/content",
            "",
            "",
        ),
        ("MOVE", "/entries/00000000000000000000000000000111", "", ""),
    ] {
        let (status, _) = fixture.request(method, path, headers, body).await;
        assert_eq!(status, 405, "unsupported {method} {path}");
    }
    fixture.close().await;
}

#[tokio::test]
async fn move_and_trash_require_if_match_without_changing_the_namespace() {
    let fixture = Fixture::new_with_entries("move-trash-precondition", true).await;
    let path = "/entries/00000000000000000000000000000111";
    for (method, path, body) in [
        ("DELETE", path.to_owned(), ""),
        (
            "POST",
            format!("{path}/move"),
            r#"{"parent_id":"00000000000000000000000000000112"}"#,
        ),
    ] {
        let (status, body) = fixture
            .request(method, &path, &mutation_headers(), body)
            .await;
        assert_eq!(status, 428, "{method} {path} must require If-Match");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "if_match_required"
        );
    }
    let (status, body) = fixture.request("GET", "/folders/children", "", "").await;
    assert_eq!(status, 200);
    let listing: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(listing["root"]["generation"], 1);
    assert_eq!(listing["items"].as_array().unwrap().len(), 2);
    assert_eq!(listing["items"][0]["name"], "alpha");
    assert_eq!(listing["items"][0]["generation"], 1);
    assert_eq!(listing["items"][1]["name"], "beta");
    assert_eq!(listing["items"][1]["generation"], 1);
    fixture.close().await;
}
