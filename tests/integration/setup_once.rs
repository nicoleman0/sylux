#[path = "../common/mod.rs"]
mod common;
use common::FixtureRoot;
use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
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
const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const PASSWORD: &str = "one-use setup password";
struct App {
    owner: FixtureRoot,
    db: Arc<Mutex<Connection>>,
    workers: SqliteWorkers,
    addr: String,
    origin: String,
    task: JoinHandle<()>,
}
impl App {
    async fn new() -> Self {
        let parent = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("p4-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "p4", "journey").unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, Duration::from_millis(300)).unwrap();
        let db = sqlite::open_writer(
            &product.join("metadata/sylux.sqlite3"),
            Duration::from_millis(300),
        )
        .unwrap();
        let verifier: [u8; 32] = Sha256::digest(SECRET.as_bytes()).into();
        db.execute(
            "UPDATE installation SET setup_verifier=?1",
            [verifier.as_slice()],
        )
        .unwrap();
        let db_path = product.join("metadata/sylux.sqlite3");
        let workers = SqliteWorkers::from_writer(
            db,
            &db_path,
            WorkerConfig {
                busy_timeout: Duration::from_millis(300),
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let db = Arc::new(Mutex::new(
            sqlite::open_writer(&db_path, Duration::from_millis(300)).unwrap(),
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let origin = format!("http://{addr}");
        let app = http::router(AuthState::new_with_setup(
            workers.clone(),
            origin.clone(),
            SECRET.into(),
        ));
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            owner,
            db,
            workers,
            addr,
            origin,
            task,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, Vec<(String, String)>) {
        let mut s = TcpStream::connect(&self.addr).await.unwrap();
        let mut raw = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.addr,
            body.len()
        );
        for (k, v) in headers {
            raw.push_str(&format!("{k}: {v}\r\n"));
        }
        raw.push_str("\r\n");
        raw.push_str(body);
        s.write_all(raw.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
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
        (status, body.to_owned(), headers)
    }
    async fn close(self) {
        self.task.abort();
        let _ = self.task.await;
        self.workers.shutdown().await.unwrap();
        drop(self.db);
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }
}
#[tokio::test]
async fn setup_rejects_database_verifier_mismatch_without_creating_admin() {
    let app = App::new().await;
    app.db
        .lock()
        .unwrap()
        .execute("UPDATE installation SET setup_verifier=zeroblob(32)", [])
        .unwrap();
    let (status, _, _) = app
        .request(
            "POST",
            "/setup/admin",
            &[
                ("origin", &app.origin),
                ("x-setup-authorization", SECRET),
                ("content-type", "application/json"),
            ],
            &json!({"handle":"owner","password":PASSWORD}).to_string(),
        )
        .await;
    assert_eq!(status, 404);
    let (accounts, roots, consumed): (i64, i64, i64) = app
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT (SELECT count(*) FROM account),(SELECT count(*) FROM entry WHERE parent_id IS NULL),setup_consumed FROM installation WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((accounts, roots, consumed), (0, 0, 0));
    app.close().await;
}

#[tokio::test]
async fn setup_once_login_create_list_rename_and_stale_preconditions() {
    let app = App::new().await;
    let auth = [
        ("origin", app.origin.as_str()),
        ("x-setup-authorization", SECRET),
    ];
    let (status, _, _) = app
        .request(
            "POST",
            "/setup/admin",
            &[auth[0], auth[1], ("content-type", "application/json")],
            &json!({"handle":"owner","password":PASSWORD}).to_string(),
        )
        .await;
    assert_eq!(status, 201);
    let (status, _, _) = app.request("GET", "/setup", &[auth[0], auth[1]], "").await;
    assert_eq!(status, 404);
    let (status, _, _) = app
        .request(
            "POST",
            "/setup/admin",
            &[auth[0], auth[1], ("content-type", "application/json")],
            &json!({"handle":"other","password":PASSWORD}).to_string(),
        )
        .await;
    assert_eq!(status, 404);
    let (status, csrf_body, csrf_headers) = app
        .request("GET", "/auth/csrf", &[("origin", &app.origin)], "")
        .await;
    assert_eq!(status, 200);
    let anon = csrf_headers
        .iter()
        .find(|(k, _)| k == "set-cookie")
        .unwrap()
        .1
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf: String = serde_json::from_str::<Value>(&csrf_body).unwrap()["csrf_token"]
        .as_str()
        .unwrap()
        .into();
    let (status, login_body, login_headers) = app
        .request(
            "POST",
            "/auth/local",
            &[
                ("origin", &app.origin),
                ("cookie", &anon),
                ("x-csrf-token", &csrf),
                ("content-type", "application/json"),
            ],
            &json!({"handle":"owner","password":PASSWORD}).to_string(),
        )
        .await;
    assert_eq!(status, 200, "{login_body}");
    let cookie = login_headers
        .iter()
        .find(|(k, _)| k == "set-cookie")
        .unwrap()
        .1
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf: String = serde_json::from_str::<Value>(&login_body).unwrap()["csrf_token"]
        .as_str()
        .unwrap()
        .into();
    let root_id: String = app
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT entry_id FROM entry WHERE parent_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // The browser cannot query the DB; it must discover the root id/etag via
    // the no-folder_id listing so it can seed folder creation and upload.
    let (status, body, _) = app
        .request("GET", "/folders/children", &[("cookie", &cookie)], "")
        .await;
    assert_eq!(status, 200);
    let listing = serde_json::from_str::<Value>(&body).unwrap();
    assert_eq!(listing["root"]["id"].as_str().unwrap(), root_id);
    assert_eq!(
        listing["root"]["etag"].as_str().unwrap(),
        format!("\"{root_id}:1\""),
        "root etag uses the id:generation form usable as upload parent_etag"
    );
    for name in ["bravo", "alpha"] {
        let (status, _, _) = app
            .request(
                "POST",
                &format!("/folders/{root_id}/children"),
                &[
                    ("origin", &app.origin),
                    ("cookie", &cookie),
                    ("x-csrf-token", &csrf),
                    ("content-type", "application/json"),
                ],
                &json!({"name":name}).to_string(),
            )
            .await;
        assert_eq!(status, 201);
    }
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/folders/{root_id}/children"),
            &[
                ("origin", &app.origin),
                ("cookie", &cookie),
                ("x-csrf-token", &csrf),
                ("content-type", "application/json"),
            ],
            &json!({"name":"ALPHA"}).to_string(),
        )
        .await;
    assert_eq!(status, 201, "case variants remain distinct");
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/folders/{root_id}/children"),
            &[
                ("origin", &app.origin),
                ("cookie", &cookie),
                ("x-csrf-token", &csrf),
                ("content-type", "application/json"),
            ],
            &json!({"name":"alpha"}).to_string(),
        )
        .await;
    assert_eq!(status, 409, "identical normalized sibling key conflicts");
    let (status, list, _headers) = app
        .request(
            "GET",
            &format!("/folders/{root_id}/children?limit=1"),
            &[("cookie", &cookie)],
            "",
        )
        .await;
    assert_eq!(status, 200);
    let value: Value = serde_json::from_str(&list).unwrap();
    assert_eq!(value["items"][0]["name"], "ALPHA");
    assert!(value["has_more"].as_bool().unwrap());
    let cursor = value["next_cursor"].as_str().unwrap();
    let entry: String = value["items"][0]["id"].as_str().unwrap().into();
    let etag: String = value["items"][0]["etag"].as_str().unwrap().into();
    let (status, _, _) = app
        .request(
            "PATCH",
            &format!("/entries/{entry}"),
            &[
                ("origin", &app.origin),
                ("cookie", &cookie),
                ("x-csrf-token", &csrf),
                ("if-match", "\"g999\""),
                ("content-type", "application/json"),
            ],
            r#"{"name":"renamed"}"#,
        )
        .await;
    assert_eq!(status, 412);
    let (status, _, _) = app
        .request(
            "PATCH",
            &format!("/entries/{entry}"),
            &[
                ("origin", &app.origin),
                ("cookie", &cookie),
                ("x-csrf-token", &csrf),
                ("if-match", &etag),
                ("content-type", "application/json"),
            ],
            r#"{"name":"renamed"}"#,
        )
        .await;
    assert_eq!(status, 200);
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/folders/{root_id}/children?limit=1&cursor={cursor}"),
            &[("cookie", &cookie)],
            "",
        )
        .await;
    assert_eq!(status, 409);
    let (status, body, _) = app
        .request(
            "PATCH",
            &format!("/entries/{root_id}"),
            &[
                ("origin", &app.origin),
                ("cookie", &cookie),
                ("x-csrf-token", &csrf),
                ("if-match", "\"g1\""),
                ("content-type", "application/json"),
            ],
            r#"{"name":"renamed-root"}"#,
        )
        .await;
    assert_eq!(status, 404, "private root is immutable: {body}");
    let counts: (i64, i64) = app
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*),(SELECT count(*) FROM entry WHERE parent_id IS NULL) FROM account",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 1));
    app.close().await;
}
