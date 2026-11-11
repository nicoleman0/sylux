//! Bounded independent personal-storage journey through real HTTP routes.
//! Restart reopens SQLite and runs product reconciliation before the router.
#![cfg(feature = "test-harness")]
#[path = "../common/mod.rs"]
mod common;

use axum::body::Bytes;
use common::{FixtureRoot, sha256_hex, unique_token};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
use sylux::http::{self, AuthState};
use sylux::persistence::{
    reconcile, root,
    sqlite::{self, SqliteWorkers, WorkerConfig},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

const BUSY: Duration = Duration::from_millis(375);
const HANDLE: &str = "owner";
const PASSWORD: &str = "correct horse battery staple";

struct Auth {
    cookie: String,
    csrf: String,
}

struct Fixture {
    owner: FixtureRoot,
    product: PathBuf,
    db_path: PathBuf,
    setup_authorization: String,
    workers: Option<SqliteWorkers>,
    origin: String,
    address: String,
    server: Option<JoinHandle<()>>,
}

impl Fixture {
    async fn new() -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-personal-{}", unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "personal-storage", "restart").unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).expect("host initialization");
        let setup_authorization = fs::read_to_string(
            product
                .join(root::SECRETS_DIR)
                .join(root::SETUP_AUTHORIZATION_NAME),
        )
        .expect("host reads setup authorization from protected file");
        let db_path = product.join(root::METADATA_DIR).join(root::DB_NAME);
        let mut fixture = Self {
            owner,
            product,
            db_path,
            setup_authorization,
            workers: None,
            origin: String::new(),
            address: String::new(),
            server: None,
        };
        fixture.restart_and_reconcile().await;
        fixture
    }

    async fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        if let Some(workers) = self.workers.take() {
            workers.shutdown().await.expect("drain database workers");
        }
    }

    async fn restart_and_reconcile(&mut self) {
        self.stop().await;
        let connection = sqlite::open_writer(&self.db_path, BUSY).expect("reopen product SQLite");
        let workers = SqliteWorkers::from_writer(
            connection,
            &self.db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .expect("start product workers");
        let report = reconcile::reconcile(&workers, &self.product)
            .await
            .expect("reconcile existing product state");
        assert!(
            report.ready,
            "restart reconciliation must complete: {report:?}"
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.address = listener.local_addr().unwrap().to_string();
        self.origin = format!("http://{}", self.address);
        let state = AuthState::new_with_setup(
            workers.clone(),
            self.origin.clone(),
            self.setup_authorization.clone(),
        )
        .with_storage_root(&self.product)
        .expect("open product payload storage");
        let app = http::router(state);
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        self.workers = Some(workers);
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
        body: &[u8],
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
        wire.extend_from_slice(body);
        stream.write_all(&wire).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        assert!(!bytes.is_empty(), "empty response for {method} {path}");
        Response::parse(&bytes)
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
                &serde_json::to_vec(&json!({"handle": HANDLE, "password": PASSWORD})).unwrap(),
            )
            .await;
        assert_eq!(response.status, 201, "{}", response.text());
    }

    async fn login(&self) -> Auth {
        let pre = self.request("GET", "/auth/csrf", None, &[], &[]).await;
        assert_eq!(pre.status, 200, "{}", pre.text());
        let pre_cookie = pre.cookie();
        let pre_csrf = pre.json()["csrf_token"].as_str().unwrap().to_owned();
        let response = self
            .request(
                "POST",
                "/auth/local",
                Some(&pre_cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", pre_csrf),
                    ("Content-Type", "application/json".to_owned()),
                ],
                &serde_json::to_vec(&json!({"handle": HANDLE, "password": PASSWORD})).unwrap(),
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.text());
        Auth {
            cookie: response.cookie(),
            csrf: response.json()["csrf_token"].as_str().unwrap().to_owned(),
        }
    }

    async fn root_listing(&self, auth: &Auth) -> Value {
        let response = self
            .request(
                "GET",
                "/folders/children?limit=5",
                Some(&auth.cookie),
                &[],
                &[],
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.text());
        response.json()
    }

    async fn upload(&self, auth: &Auth, parent_id: &str, name: &str, bytes: &[u8]) -> Value {
        let parent = self.root_listing(auth).await;
        let parent_etag = parent["root"]["etag"].as_str().unwrap().to_owned();
        let digest = sha256_hex(bytes);
        let ticket = self
            .request(
                "POST",
                "/mutation-tickets",
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("Content-Type", "application/json".to_owned()),
                ],
                &serde_json::to_vec(&json!({
                    "scope":"upload_create", "parent_id":parent_id,
                    "parent_etag":parent_etag, "name":name,
                    "length":bytes.len(), "sha256":digest,
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(ticket.status, 201, "{}", ticket.text());
        let mutation_id = ticket.header("x-mutation-id").unwrap().to_owned();

        let admission = self
            .request(
                "POST",
                "/uploads",
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("X-Mutation-ID", mutation_id.clone()),
                    ("If-None-Match", "*".to_owned()),
                    ("Content-Type", "application/json".to_owned()),
                ],
                &serde_json::to_vec(&json!({
                    "parent_id":parent_id, "parent_etag":parent_etag,
                    "name":name, "length":bytes.len(), "sha256":digest,
                }))
                .unwrap(),
            )
            .await;
        assert_eq!(admission.status, 201, "{}", admission.text());
        let upload_id = admission.json()["upload_id"].as_str().unwrap().to_owned();

        let content = self
            .request(
                "PUT",
                &format!("/uploads/{upload_id}/content"),
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("X-Mutation-ID", mutation_id.clone()),
                    ("Content-Type", "application/octet-stream".to_owned()),
                ],
                bytes,
            )
            .await;
        assert_eq!(content.status, 200, "{}", content.text());
        assert_eq!(content.json()["sha256"], digest);

        let commit = self
            .request(
                "POST",
                &format!("/uploads/{upload_id}/commit"),
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("X-Mutation-ID", mutation_id),
                    ("If-None-Match", "*".to_owned()),
                    ("Content-Type", "application/json".to_owned()),
                ],
                &serde_json::to_vec(&json!({"sha256":digest, "parent_etag":parent_etag})).unwrap(),
            )
            .await;
        assert_eq!(commit.status, 201, "{}", commit.text());
        let result = commit.json();
        assert_eq!(result["sha256"], digest);
        assert_eq!(result["size"], bytes.len() as u64);
        result
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
                &serde_json::to_vec(&json!({"name":name})).unwrap(),
            )
            .await;
        assert_eq!(response.status, 201, "{}", response.text());
        response.json()
    }

    async fn rename(&self, auth: &Auth, entry_id: &str, old_etag: &str, name: &str) -> Value {
        let response = self
            .request(
                "PATCH",
                &format!("/entries/{entry_id}"),
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                    ("If-Match", old_etag.to_owned()),
                    ("Content-Type", "application/json".to_owned()),
                ],
                &serde_json::to_vec(&json!({"name":name})).unwrap(),
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
                &[],
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.text());
        response
    }

    async fn logout(&self, auth: &Auth) {
        let response = self
            .request(
                "POST",
                "/auth/logout",
                Some(&auth.cookie),
                &[
                    ("Origin", self.origin.clone()),
                    ("X-CSRF-Token", auth.csrf.clone()),
                ],
                &[],
            )
            .await;
        assert_eq!(response.status, 204, "{}", response.text());
        let me = self
            .request("GET", "/me", Some(&auth.cookie), &[], &[])
            .await;
        assert_eq!(me.status, 401, "logout must revoke the session");
    }
}

fn inventory(items: &[Value]) -> Vec<(String, String, String, String)> {
    let mut rows: Vec<_> = items
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
            .expect("HTTP header terminator");
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
            .is_some_and(|v| v == "chunked")
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
async fn personal_storage_routes_preserve_acknowledged_inventory_after_reconcile_restart() {
    let mut fixture = Fixture::new().await;
    fixture.setup_admin().await;
    let auth = fixture.login().await;
    let root = fixture.root_listing(&auth).await;
    let root_id = root["root"]["id"].as_str().unwrap().to_owned();

    let fixtures: [(&str, &[u8]); 3] = [
        ("empty.bin", &[]),
        ("binary.bin", &[0, 255, 1, 128, b'\n', 42]),
        ("café 雪.txt", "Snowman ☃ — café\n".as_bytes()),
    ];
    let mut uploaded = Vec::new();
    for (name, bytes) in fixtures {
        let result = fixture.upload(&auth, &root_id, name, bytes).await;
        uploaded.push((
            name.to_owned(),
            bytes.to_vec(),
            result["id"].as_str().unwrap().to_owned(),
        ));
    }
    let folder = fixture.create_folder(&auth, &root_id, "docs").await;
    let listing = fixture.root_listing(&auth).await;
    let first = &uploaded[1].2;
    let etag = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"].as_str() == Some(first.as_str()))
        .unwrap()["etag"]
        .as_str()
        .unwrap()
        .to_owned();
    let renamed = fixture
        .rename(&auth, first, &etag, "binary-renamed.bin")
        .await;
    assert_eq!(renamed["name"], "binary-renamed.bin");
    assert_eq!(folder["kind"], "folder");

    for (name, bytes, id) in &uploaded {
        let response = fixture.download(&auth, id).await;
        assert_eq!(
            response.header("content-type"),
            Some("application/octet-stream")
        );
        assert_eq!(
            response.body.as_ref(),
            bytes.as_slice(),
            "exact bytes for {name}"
        );
        assert_eq!(
            sha256_hex(&response.body),
            sha256_hex(bytes),
            "digest for {name}"
        );
    }
    let before = inventory(
        fixture.root_listing(&auth).await["items"]
            .as_array()
            .unwrap(),
    );
    assert_eq!(before.len(), 4, "3 files and one folder");
    fixture.logout(&auth).await;
    let _fresh_auth = fixture.login().await;

    fixture.restart_and_reconcile().await;
    let after_auth = fixture.login().await;
    let after = inventory(
        fixture.root_listing(&after_auth).await["items"]
            .as_array()
            .unwrap(),
    );
    assert_eq!(
        before, after,
        "acknowledged inventory and ETags survive reconciliation restart"
    );
    for (_, bytes, id) in &uploaded {
        let response = fixture.download(&after_auth, id).await;
        assert_eq!(response.body.as_ref(), bytes.as_slice());
        assert_eq!(sha256_hex(&response.body), sha256_hex(bytes));
    }
    fixture.close().await;
}

#[tokio::test]
async fn moves_enforce_owner_boundaries_and_preserve_subtrees_and_bytes_after_restart() {
    let mut fixture = Fixture::new().await;
    fixture.setup_admin().await;
    let auth = fixture.login().await;
    let initial = fixture.root_listing(&auth).await;
    let root = initial["root"]["id"].as_str().unwrap();
    let bytes = &[0, 255, 128, 42, 13, 10];
    let file = fixture.upload(&auth, root, "café.bin", bytes).await;
    let source = fixture.create_folder(&auth, root, "Source").await;
    let destination = fixture.create_folder(&auth, root, "Destination").await;
    let nested = fixture
        .create_folder(&auth, source["id"].as_str().unwrap(), "Nested")
        .await;
    let file_id = file["id"].as_str().unwrap();
    let file_entry = fixture.root_listing(&auth).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == file_id)
        .unwrap()
        .clone();
    let source_id = source["id"].as_str().unwrap();
    let destination_id = destination["id"].as_str().unwrap();
    let mut source_etag = String::new();
    let foreign_root = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    fixture.workers.as_ref().unwrap().transact(move |tx| {
        tx.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','other-owner','user',1,0)", [])?;
        tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',NULL,'folder','',x'',1)", [foreign_root])?;
        Ok(())
    }).await.unwrap();
    let move_request = |id: &str, tag: &str, parent: &str| {
        (
            format!("/entries/{id}/move"),
            vec![
                ("Origin", fixture.origin.clone()),
                ("X-CSRF-Token", auth.csrf.clone()),
                ("If-Match", tag.to_owned()),
                ("Content-Type", "application/json".to_owned()),
            ],
            serde_json::to_vec(&json!({"parent_id": parent})).unwrap(),
        )
    };
    for (id, tag, parent, expected_status, expected_code) in [
        (file_id, "stale", source_id, 412, "stale_etag"),
        (
            file_id,
            file_entry["etag"].as_str().unwrap(),
            foreign_root,
            404,
            "not_found",
        ),
        (foreign_root, "irrelevant", source_id, 404, "not_found"),
        (
            root,
            initial["root"]["etag"].as_str().unwrap(),
            source_id,
            404,
            "not_found",
        ),
    ] {
        let (path, headers, body) = move_request(id, tag, parent);
        let response = fixture
            .request("POST", &path, Some(&auth.cookie), &headers, &body)
            .await;
        assert_eq!(response.status, expected_status, "{}", response.text());
        assert_eq!(response.json()["code"], expected_code);
    }
    let (path, mut headers, body) =
        move_request(file_id, file_entry["etag"].as_str().unwrap(), source_id);
    headers.retain(|(name, _)| *name != "If-Match");
    assert_eq!(
        fixture
            .request("POST", &path, Some(&auth.cookie), &headers, &body)
            .await
            .status,
        428
    );
    let (path, mut headers, body) =
        move_request(file_id, file_entry["etag"].as_str().unwrap(), source_id);
    headers.retain(|(name, _)| *name != "X-CSRF-Token");
    assert_eq!(
        fixture
            .request("POST", &path, Some(&auth.cookie), &headers, &body)
            .await
            .status,
        403
    );
    let (path, headers, body) = move_request(
        file_id,
        file_entry["etag"].as_str().unwrap(),
        nested["id"].as_str().unwrap(),
    );
    let moved_file = fixture
        .request("POST", &path, Some(&auth.cookie), &headers, &body)
        .await;
    assert_eq!(moved_file.status, 200, "{}", moved_file.text());
    assert_eq!(moved_file.json()["parent_id"], nested["id"]);
    for entry in fixture.root_listing(&auth).await["items"]
        .as_array()
        .unwrap()
    {
        if entry["id"] == source_id {
            source_etag = entry["etag"].as_str().unwrap().to_owned();
        }
    }
    for parent in [source_id, nested["id"].as_str().unwrap()] {
        let (path, headers, body) = move_request(source_id, &source_etag, parent);
        let response = fixture
            .request("POST", &path, Some(&auth.cookie), &headers, &body)
            .await;
        assert_eq!(response.status, 409);
        assert_eq!(response.json()["code"], "invalid_destination");
    }
    fixture.create_folder(&auth, destination_id, "Source").await;
    let (path, headers, body) = move_request(source_id, &source_etag, destination_id);
    let collision = fixture
        .request("POST", &path, Some(&auth.cookie), &headers, &body)
        .await;
    assert_eq!(collision.status, 409);
    assert_eq!(collision.json()["code"], "name_conflict");
    let renamed = fixture
        .rename(&auth, source_id, &source_etag, "Moved tree")
        .await;
    let (path, headers, body) =
        move_request(source_id, renamed["etag"].as_str().unwrap(), destination_id);
    let moved_tree = fixture
        .request("POST", &path, Some(&auth.cookie), &headers, &body)
        .await;
    assert_eq!(moved_tree.status, 200, "{}", moved_tree.text());
    let tree_before = fixture
        .request(
            "GET",
            &format!("/folders/{destination_id}/children"),
            Some(&auth.cookie),
            &[],
            &[],
        )
        .await
        .json();
    let nested_before = fixture
        .request(
            "GET",
            &format!("/folders/{source_id}/children"),
            Some(&auth.cookie),
            &[],
            &[],
        )
        .await
        .json();
    let files_before = fixture
        .request(
            "GET",
            &format!("/folders/{}/children", nested["id"].as_str().unwrap()),
            Some(&auth.cookie),
            &[],
            &[],
        )
        .await
        .json();
    assert_eq!(tree_before["items"][0]["parent_id"], destination_id);
    assert_eq!(nested_before["items"][0]["id"], nested["id"]);
    assert_eq!(files_before["items"][0]["id"], file_id);
    assert_eq!(fixture.download(&auth, file_id).await.body.as_ref(), bytes);
    fixture.restart_and_reconcile().await;
    let auth = fixture.login().await;
    for (id, expected) in [
        (destination_id, tree_before),
        (source_id, nested_before),
        (nested["id"].as_str().unwrap(), files_before),
    ] {
        let response = fixture
            .request(
                "GET",
                &format!("/folders/{id}/children"),
                Some(&auth.cookie),
                &[],
                &[],
            )
            .await;
        assert_eq!(response.status, 200);
        assert_eq!(response.json(), expected);
    }
    assert_eq!(fixture.download(&auth, file_id).await.body.as_ref(), bytes);
    fixture.close().await;
}

#[tokio::test]
async fn moving_a_subtree_enforces_the_depth_limit_without_partial_changes() {
    use sylux::domain::{namespace, upload::folder_etag};
    let fixture = Fixture::new().await;
    fixture.setup_admin().await;
    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            let owner: String = tx.query_row(
                "SELECT account_id FROM account WHERE handle='owner'",
                [],
                |row| row.get(0),
            )?;
            let root = namespace::private_root(tx, &owner).unwrap();
            let source = namespace::create_folder(tx, &owner, &root, "Subtree").unwrap();
            namespace::create_folder(tx, &owner, &source.id, "Child").unwrap();
            let mut parent = root.clone();
            for level in 1..=127 {
                parent = namespace::create_folder(tx, &owner, &parent, &format!("Level {level}"))
                    .unwrap()
                    .id;
            }
            let result =
                namespace::move_entry(tx, &owner, &source.id, &parent, &folder_etag(&source.id, 2));
            assert!(matches!(result, Err(namespace::NamespaceError::DepthLimit)));
            let actual: (String, i64) = tx.query_row(
                "SELECT parent_id,generation FROM entry WHERE entry_id=?1",
                [&source.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(actual, (root.clone(), 2));
            let leaf = namespace::create_folder(tx, &owner, &root, "Leaf").unwrap();
            let moved = namespace::move_entry(tx, &owner, &leaf.id, &parent, &leaf.etag).unwrap();
            assert_eq!(moved.parent_id.as_deref(), Some(parent.as_str()));
            Ok(())
        })
        .await
        .unwrap();
    fixture.close().await;
}
