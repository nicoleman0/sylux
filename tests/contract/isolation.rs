//! T027 A03 unrelated-identity isolation over the real loopback-TLS router.
#[path = "../common/mod.rs"]
mod common;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{
    FixtureRoot,
    server::{TlsApiServer, TlsFiles, tls_connect},
};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
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

const OWNER: &str = "00000000000000000000000000000001";
const OTHER: &str = "00000000000000000000000000000002";
const ADMIN: &str = "00000000000000000000000000000003";
const ROOT: &str = "00000000000000000000000000000101";
const OTHER_ROOT: &str = "00000000000000000000000000000102";
const SESSION: &str = "00000000000000000000000000000201";
const OTHER_SESSION: &str = "00000000000000000000000000000202";
const ADMIN_SESSION: &str = "00000000000000000000000000000203";
const SECRET: &[u8; 32] = b"s1-owner-secret-0123456789abcdef";
const OTHER_SECRET: &[u8; 32] = b"s1-other-secret-0123456789abcdef";
const ADMIN_SECRET: &[u8; 32] = b"s1-admin-secret-0123456789abcdef";
const CSRF: &[u8; 32] = b"s1-owner-csrf-0123456789abcdef!!";
const OTHER_CSRF: &[u8; 32] = b"s1-other-csrf-0123456789abcdef!!";
const ADMIN_CSRF: &[u8; 32] = b"s1-admin-csrf-0123456789abcdef!!";
const BUSY: Duration = Duration::from_millis(375);

struct Fixture {
    root: FixtureRoot,
    server: Option<TlsApiServer>,
    workers: Option<SqliteWorkers>,
    ca: Vec<u8>,
}
impl Fixture {
    async fn new() -> Self {
        let parent = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("sylux-isolation-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let root = FixtureRoot::create(&parent, "isolation", "a03").unwrap();
        let product = root.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).unwrap();
        let tls = TlsFiles::generate(&root, "tls").unwrap();
        let ca = fs::read(&tls.ca_der).unwrap();
        let db_path = product.join("metadata/sylux.sqlite3");
        let db = sqlite::open_writer(&db_path, BUSY).unwrap();
        seed(&db, &product);
        let workers = SqliteWorkers::from_writer(
            db,
            &db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let app = http::router(
            AuthState::new(workers.clone(), "https://127.0.0.1".into())
                .with_storage_root(&product)
                .unwrap(),
        );
        let server = TlsApiServer::start(&root, &tls, app, Duration::from_secs(2))
            .await
            .unwrap();
        Self {
            root,
            server: Some(server),
            workers: Some(workers),
            ca,
        }
    }
    async fn close(mut self) {
        self.server
            .take()
            .unwrap()
            .shutdown_and_join()
            .await
            .unwrap();
        self.workers.take().unwrap().shutdown().await.unwrap();
        self.root.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.root.path().parent().unwrap()).unwrap();
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        actor: Option<(&str, &[u8; 32])>,
        headers: &[(&str, String)],
        body: &str,
    ) -> (u16, BTreeMap<String, String>, Vec<u8>) {
        let server = self.server.as_ref().unwrap();
        let mut s = tls_connect(server.address(), &self.ca, "127.0.0.1")
            .await
            .unwrap();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\nAccept: application/json\r\n",
            server.address().port()
        );
        if let Some((id, secret)) = actor {
            req.push_str(&format!(
                "Cookie: __Host-sylux={id}.{}\r\n",
                URL_SAFE_NO_PAD.encode(secret)
            ));
        }
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        {
            req.push_str("Content-Type: application/json\r\n");
        }
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        s.write_all(req.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        s.read_to_end(&mut bytes).await.unwrap();
        let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
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
        let mut hs = BTreeMap::new();
        for l in lines {
            if let Some((k, v)) = l.split_once(':') {
                hs.insert(k.to_ascii_lowercase(), v.trim().to_owned());
            }
        }
        (status, hs, bytes[split + 4..].to_vec())
    }
}
fn seed(c: &rusqlite::Connection, product: &std::path::Path) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for (id, root, handle, role, sid, secret, csrf) in [
        (OWNER, ROOT, "owner", "user", SESSION, SECRET, CSRF),
        (
            OTHER,
            OTHER_ROOT,
            "other",
            "user",
            OTHER_SESSION,
            OTHER_SECRET,
            OTHER_CSRF,
        ),
        (
            ADMIN,
            "00000000000000000000000000000103",
            "synthetic-admin",
            "administrator",
            ADMIN_SESSION,
            ADMIN_SECRET,
            ADMIN_CSRF,
        ),
    ] {
        c.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,?2,?3,1,0)",params![id,handle,role]).unwrap();
        c.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)",params![root,id]).unwrap();
        c.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?4,?5,?6,?7,0)",params![sid,id,verifier(secret).as_slice(),verifier(csrf).as_slice(),now,now+3600,now+7200]).unwrap();
    }
    c.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES('00000000000000000000000000000301',?1,?2,'folder','private-name',x'707269766174652d6e616d65',1,1)",params![OWNER,ROOT]).unwrap();
    let bytes = b"synthetic isolation payload";
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let store = sylux::persistence::payload::PayloadStore::open(product).unwrap();
    let mut staged = store
        .start(
            "1234567890abcdef1234567890abcdef-abcdef0123456789abcdef0123456789",
            bytes.len() as u64,
        )
        .unwrap();
    staged.write_chunk(bytes).unwrap();
    let published = staged.publish(digest).unwrap();
    c.execute(
        "INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,?2,?3,?4,'live')",
        params![
            published.blob_id,
            &published.blob_id[..2],
            bytes.len() as i64,
            digest.as_slice()
        ],
    )
    .unwrap();
    c.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,current_revision_id,generation) VALUES('00000000000000000000000000000302',?1,?2,'file','private-file.bin',x'707269766174652d66696c652e62696e',1,NULL,1)",params![OWNER,ROOT]).unwrap();
    c.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES('00000000000000000000000000000303','00000000000000000000000000000302',?1,?2,?3,?4,?5)",params![published.blob_id,bytes.len() as i64,digest.as_slice(),now,OWNER]).unwrap();
    c.execute("UPDATE entry SET current_revision_id='00000000000000000000000000000303' WHERE entry_id='00000000000000000000000000000302'",[]).unwrap();
    let check:i64=c.query_row("SELECT count(*) FROM entry JOIN revision ON revision.revision_id=entry.current_revision_id JOIN blob ON blob.blob_id=revision.blob_id WHERE entry.entry_id='00000000000000000000000000000302' AND entry.owner_account_id=?1 AND entry.kind='file' AND blob.lifecycle='live'",params![OWNER],|row|row.get(0)).unwrap();
    assert_eq!(
        check, 1,
        "synthetic entry/revision/blob fixture must satisfy the production content query"
    );
}
fn cookie(actor: usize) -> Option<(&'static str, &'static [u8; 32])> {
    match actor {
        0 => Some((SESSION, SECRET)),
        1 => Some((OTHER_SESSION, OTHER_SECRET)),
        2 => Some((ADMIN_SESSION, ADMIN_SECRET)),
        _ => None,
    }
}

#[tokio::test]
async fn a03_unrelated_and_admin_route_matrix_denies_owner_private_state() {
    let f = Fixture::new().await;
    // Keep unsupported methods distinct from authenticated namespace mutations.
    let private_entry_path = "/entries/00000000000000000000000000000301";
    let negative_inventory = [
        ("PUT", private_entry_path.to_owned(), 405),
        (
            "PATCH",
            "/entries/00000000000000000000000000000302/content".to_owned(),
            405,
        ),
    ];
    let mut negative_route_rows = Vec::new();
    for (method, path, expected_status) in negative_inventory {
        let (status, _, _) = f.request(method, &path, cookie(0), &[], "{}").await;
        assert_eq!(
            status, expected_status,
            "unsupported {method} {path} must be rejected by the real router"
        );
        assert!(!(200..300).contains(&status));
        let safe_path = path
            .replace("00000000000000000000000000000301", "<seeded-folder>")
            .replace("00000000000000000000000000000302", "<seeded-file>");
        negative_route_rows.push(format!(
            "{method} {safe_path} owner={status} expected={expected_status}"
        ));
    }
    // Route-level methods/statuses: unrelated and administrator should be indistinguishable on private reads and writes.
    // Create a real owner-owned upload attempt through the S1 ticket/admission routes.
    let owner_headers = [
        ("Origin", "https://127.0.0.1".to_owned()),
        ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(CSRF)),
        ("Content-Type", "application/json".to_owned()),
    ];
    let upload_bytes = b"synthetic cross-account upload";
    let digest = Sha256::digest(upload_bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let etag = format!("\"{ROOT}:1\"");
    let ticket_body = serde_json::json!({
        "scope": "upload_create",
        "parent_id": ROOT,
        "parent_etag": etag,
        "name": "private-upload.bin",
        "length": upload_bytes.len(),
        "sha256": digest,
    })
    .to_string();
    let (ticket_status, ticket_headers, _) = f
        .request(
            "POST",
            "/mutation-tickets",
            cookie(0),
            &owner_headers,
            &ticket_body,
        )
        .await;
    assert_eq!(ticket_status, 201);
    let ticket = ticket_headers
        .get("x-mutation-id")
        .expect("owner ticket")
        .clone();
    let mut admission_headers = owner_headers.to_vec();
    admission_headers.push(("X-Mutation-ID", ticket.clone()));
    admission_headers.push(("If-None-Match", "*".to_owned()));
    let admission_body = serde_json::json!({
        "parent_id": ROOT,
        "parent_etag": etag,
        "name": "private-upload.bin",
        "length": upload_bytes.len(),
        "sha256": digest,
    })
    .to_string();
    let (admit_status, _, admit_body) = f
        .request(
            "POST",
            "/uploads",
            cookie(0),
            &admission_headers,
            &admission_body,
        )
        .await;
    assert_eq!(
        admit_status,
        201,
        "{}",
        String::from_utf8_lossy(&admit_body)
    );
    let upload_id = serde_json::from_slice::<serde_json::Value>(&admit_body).unwrap()["upload_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let upload_path = format!("/uploads/{upload_id}");
    let upload_content_path = format!("{upload_path}/content");
    let upload_bytes_text = std::str::from_utf8(upload_bytes).unwrap();
    let mut content_headers = owner_headers.to_vec();
    content_headers.push(("X-Mutation-ID", ticket.clone()));
    content_headers.push(("Content-Type", "application/octet-stream".to_owned()));
    let (stage_status, _, stage_body) = f
        .request(
            "PUT",
            &upload_content_path,
            cookie(0),
            &content_headers,
            upload_bytes_text,
        )
        .await;
    assert_eq!(
        stage_status,
        200,
        "{}",
        String::from_utf8_lossy(&stage_body)
    );

    let mut upload_rows = Vec::new();
    let mut owner_status_body = None;
    for method in ["GET", "DELETE"] {
        let mut foreign_outcomes = Vec::new();
        for actor in [1, 2] {
            let csrf = if actor == 1 { OTHER_CSRF } else { ADMIN_CSRF };
            let actor_headers = [
                ("Origin", "https://127.0.0.1".to_owned()),
                ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(csrf)),
            ];
            let (status, _, body) = f
                .request(method, &upload_path, cookie(actor), &actor_headers, "")
                .await;
            foreign_outcomes.push((status, body));
        }
        assert_eq!(
            foreign_outcomes[0], foreign_outcomes[1],
            "admin differs from unrelated on seeded upload {method}"
        );
        assert_eq!(
            foreign_outcomes[0].0, 404,
            "foreign seeded upload must be opaque"
        );
        let text = String::from_utf8_lossy(&foreign_outcomes[0].1);
        for field in ["private-upload.bin", "sha256", "length", "owner_account_id"] {
            assert!(
                !text.contains(field),
                "private upload field leaked: {field}"
            );
        }
        let (anonymous_status, _, anonymous_body) =
            f.request(method, &upload_path, None, &[], "").await;
        assert!(
            anonymous_status == 401 || anonymous_status == 403,
            "anonymous upload access must be denied, got {anonymous_status}"
        );
        let anonymous_text = String::from_utf8_lossy(&anonymous_body);
        assert!(!anonymous_text.contains("private-upload.bin"));
        let owner_control = if method == "GET" {
            let (status, _, body) = f
                .request(method, &upload_path, cookie(0), &owner_headers, "")
                .await;
            assert_eq!(status, 200, "owner can read own seeded upload status");
            let text = String::from_utf8_lossy(&body);
            assert!(
                !text.contains("private-upload.bin"),
                "status should not return names"
            );
            assert!(
                text.contains("staged"),
                "owner status should expose its staged state"
            );
            owner_status_body = Some(body);
            status
        } else {
            let (status, _, body) = f
                .request("DELETE", &upload_path, cookie(0), &owner_headers, "")
                .await;
            assert_eq!(status, 204, "{}", String::from_utf8_lossy(&body));
            status
        };
        upload_rows.push(format!(
            "{method} /uploads/<seeded-owner-attempt> owner={owner_control} unrelated={} admin={} anonymous={anonymous_status}",
            foreign_outcomes[0].0, foreign_outcomes[1].0
        ));
    }
    assert!(!owner_status_body.expect("owner status checked").is_empty());
    let cases = vec![
        (
            "GET",
            "/folders/children?folder_id=00000000000000000000000000000101".to_owned(),
            vec![],
        ),
        (
            "GET",
            "/folders/00000000000000000000000000000101/children".to_owned(),
            vec![],
        ),
        (
            "GET",
            "/entries/00000000000000000000000000000302/content".to_owned(),
            vec![],
        ),
        (
            "HEAD",
            "/entries/00000000000000000000000000000302/content".to_owned(),
            vec![],
        ),
        ("PUT", upload_content_path.clone(), vec![]),
        ("POST", format!("{upload_path}/commit"), vec![]),
        (
            "PATCH",
            "/entries/00000000000000000000000000000301".to_owned(),
            vec![],
        ),
    ];
    let mut matrix = Vec::new();
    for (method, path, headers) in cases {
        let mut outcomes = Vec::new();
        for actor in 0..4 {
            let actor_headers = if matches!(method, "PATCH" | "PUT" | "POST") {
                let csrf = match actor {
                    0 => CSRF,
                    1 => OTHER_CSRF,
                    2 => ADMIN_CSRF,
                    _ => CSRF,
                };
                let mut request_headers = vec![
                    ("Origin", "https://127.0.0.1".to_owned()),
                    ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(csrf)),
                ];
                if method == "PATCH" {
                    request_headers.push((
                        "If-Match",
                        "\"00000000000000000000000000000301:1\"".to_owned(),
                    ));
                } else {
                    request_headers.push(("X-Mutation-ID", ticket.clone()));
                    request_headers.push((
                        "Content-Type",
                        if method == "PUT" {
                            "application/octet-stream".to_owned()
                        } else {
                            "application/json".to_owned()
                        },
                    ));
                    if method == "POST" {
                        request_headers.push(("If-None-Match", "*".to_owned()));
                    }
                }
                request_headers
            } else {
                headers.clone()
            };
            let commit_body =
                serde_json::json!({"sha256": digest, "parent_etag": etag}).to_string();
            let body = if method == "PATCH" {
                r#"{"name":"private-renamed"}"#
            } else if method == "PUT" {
                upload_bytes_text
            } else if method == "POST" && path.ends_with("/commit") {
                commit_body.as_str()
            } else {
                ""
            };
            let (status, _, response_body) = f
                .request(method, &path, cookie(actor), &actor_headers, body)
                .await;
            outcomes.push((status, String::from_utf8_lossy(&response_body).into_owned()));
        }
        if path.starts_with("/entries/") && path.ends_with("/content") {
            assert_eq!(
                outcomes[0].0, 200,
                "seeded owner content fixture invalid: {:?}",
                outcomes[0]
            );
        }
        if path.starts_with("/folders/") {
            assert_eq!(
                outcomes[0].0, 200,
                "owner listing control failed: {:?}",
                outcomes[0]
            );
            assert!(
                outcomes[0].1.contains("private-name"),
                "owner listing omitted seeded name"
            );
            assert!(
                outcomes[0].1.contains("private-file.bin"),
                "owner listing omitted seeded file name"
            );
            assert_eq!(outcomes[1].0, 404, "unrelated root listing must be opaque");
            assert_eq!(
                outcomes[2].0, 404,
                "administrator root listing must be opaque"
            );
        }
        if method == "PATCH" {
            assert_eq!(
                outcomes[0].0, 200,
                "owner rename control failed: {:?}",
                outcomes[0]
            );
            assert!(
                outcomes[0].1.contains("private-renamed"),
                "owner rename response omitted new name"
            );
        }
        assert_eq!(
            outcomes[1], outcomes[2],
            "unrelated/admin outcome differs for {method} {path}"
        );
        assert!(
            outcomes[1].0 == 401 || outcomes[1].0 == 404 || outcomes[1].0 == 403,
            "unexpected unrelated response {method} {path}: {:?}",
            outcomes[1]
        );
        assert!(
            !outcomes[1].1.contains("private-name")
                && !outcomes[1].1.contains("private-file.bin")
                && !outcomes[1].1.contains("private-renamed")
                && !outcomes[1].1.contains("private-upload.bin")
                && !outcomes[1].1.contains(OWNER)
                && !outcomes[1].1.contains("length")
                && !outcomes[1].1.contains("sha256"),
            "private fields leaked: {:?}",
            outcomes[1]
        );
        matrix.push((method, path, outcomes));
    }
    // Conditional, range, unsatisfiable and malformed/double-encoded direct IDs are opaque to unrelated users.
    let private_file_id = "00000000000000000000000000000302";
    let content_path = format!("/entries/{private_file_id}/content");
    let (_, owner_headers, _) = f.request("HEAD", &content_path, cookie(0), &[], "").await;
    let etag = owner_headers
        .get("etag")
        .expect("owner gets current ETag")
        .clone();
    let double_encoded_id = private_file_id
        .bytes()
        .map(|byte| format!("%25{byte:02X}"))
        .collect::<String>();
    let paths = [
        content_path.clone(),
        format!("/entries/{double_encoded_id}/content"),
        "/entries/not-an-id/content".to_owned(),
    ];
    let mut extra_rows = Vec::new();
    for path in &paths {
        for (label, method, headers) in [
            ("range", "GET", vec![("Range", "bytes=0-0".into())]),
            ("conditional", "GET", vec![("If-None-Match", etag.clone())]),
            (
                "head_conditional",
                "HEAD",
                vec![("If-None-Match", etag.clone())],
            ),
            (
                "unsatisfied",
                "GET",
                vec![("Range", "bytes=999999-".into())],
            ),
        ] {
            let (status, _, body) = f.request(method, path, cookie(1), &headers, "").await;
            let (admin_status, _, admin_body) =
                f.request(method, path, cookie(2), &headers, "").await;
            let (anonymous_status, _, anonymous_body) =
                f.request(method, path, None, &headers, "").await;
            assert_eq!(
                (status, &body),
                (admin_status, &admin_body),
                "admin differs for {label} {path}"
            );
            assert!(
                status == 401 || status == 404 || status == 400,
                "{label} disclosed conditional/range status {status}"
            );
            assert!(
                anonymous_status == 401 || anonymous_status == 404 || anonymous_status == 400,
                "anonymous {label} disclosed conditional/range status {anonymous_status}"
            );
            let owner_status = if path == &content_path {
                let (owner_status, response_headers, owner_body) =
                    f.request(method, path, cookie(0), &headers, "").await;
                match label {
                    "range" => assert_eq!(owner_status, 206, "owner range control"),
                    "conditional" | "head_conditional" => {
                        assert_eq!(owner_status, 304, "owner conditional control");
                        assert!(owner_body.is_empty(), "304 response body must be empty");
                    }
                    "unsatisfied" => {
                        assert_eq!(owner_status, 416, "owner unsatisfied range control");
                        assert!(response_headers.contains_key("content-range"));
                    }
                    _ => unreachable!(),
                }
                owner_status.to_string()
            } else {
                "not-applicable".to_owned()
            };
            extra_rows.push(format!(
                "{method} {path} {label} owner={owner_status} unrelated={status} admin={admin_status} anonymous={anonymous_status}"
            ));
            let text = String::from_utf8_lossy(&body);
            let anonymous_text = String::from_utf8_lossy(&anonymous_body);
            for key in [
                "private-name",
                "private-file.bin",
                "private-renamed",
                "private-upload.bin",
                "sha256",
                "length",
                "owner_account_id",
                "blob_id",
                "path",
            ] {
                assert!(
                    !text.contains(key),
                    "private field {key} leaked for {label}: {text}"
                );
                assert!(
                    !anonymous_text.contains(key),
                    "private field {key} leaked to anonymous for {label}: {anonymous_text}"
                );
            }
        }
    }
    let mut namespace_rows = Vec::new();
    let (status, _, namespace_before) = f
        .request("GET", "/folders/children", cookie(0), &[], "")
        .await;
    assert_eq!(status, 200);
    for id in ["00000000000000000000000000000301", private_file_id] {
        let generation = if id == private_file_id { 1 } else { 2 };
        for method in ["DELETE", "POST"] {
            let path = if method == "POST" {
                format!("/entries/{id}/move")
            } else {
                format!("/entries/{id}")
            };
            let mut foreign_outcomes = Vec::new();
            for (actor, csrf, destination) in [
                (1, OTHER_CSRF, OTHER_ROOT),
                (2, ADMIN_CSRF, "00000000000000000000000000000103"),
                (3, CSRF, ROOT),
            ] {
                let headers = [
                    ("Origin", "https://127.0.0.1".to_owned()),
                    ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(csrf)),
                    ("If-Match", format!("\"{id}:{generation}\"")),
                ];
                let body = if method == "POST" {
                    serde_json::json!({"parent_id": destination}).to_string()
                } else {
                    String::new()
                };
                let (status, headers, body) = f
                    .request(method, &path, cookie(actor), &headers, &body)
                    .await;
                assert_eq!(
                    status,
                    if actor == 3 { 401 } else { 404 },
                    "{method} {path} actor={actor}"
                );
                assert!(!headers.contains_key("etag"));
                let text = String::from_utf8_lossy(&body);
                for private in [
                    "private-name",
                    "private-renamed",
                    "private-file.bin",
                    "synthetic isolation payload",
                    OWNER,
                    "sha256",
                    "length",
                ] {
                    assert!(!text.contains(private), "{method} {path} leaked {private}");
                }
                foreign_outcomes.push((status, body));
            }
            assert_eq!(
                foreign_outcomes[0], foreign_outcomes[1],
                "admin namespace access differs from unrelated account"
            );
            let (status, _, body) = f.request("GET", &content_path, cookie(0), &[], "").await;
            assert_eq!(status, 200);
            assert_eq!(
                body, b"synthetic isolation payload",
                "denied mutation changed owner bytes"
            );
            let (status, _, namespace_after) = f
                .request("GET", "/folders/children", cookie(0), &[], "")
                .await;
            assert_eq!(status, 200);
            assert_eq!(
                namespace_after, namespace_before,
                "denied mutation changed owner names or parents"
            );
            namespace_rows.push(format!(
                "{method} {path} unrelated=404 admin=404 anonymous=401"
            ));
        }
    }
    let move_headers = [
        ("Origin", "https://127.0.0.1".to_owned()),
        ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(CSRF)),
        ("If-Match", format!("\"{private_file_id}:1\"")),
    ];
    let (status, _, body) = f
        .request(
            "POST",
            &format!("/entries/{private_file_id}/move"),
            cookie(0),
            &move_headers,
            r#"{"parent_id":"00000000000000000000000000000301"}"#,
        )
        .await;
    assert_eq!(
        status,
        200,
        "owner move control: {}",
        String::from_utf8_lossy(&body)
    );
    let moved: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(moved["parent_id"], "00000000000000000000000000000301");
    let mut trash_headers = move_headers.to_vec();
    trash_headers[2].1 = moved["etag"].as_str().unwrap().to_owned();
    let (status, _, body) = f
        .request(
            "DELETE",
            &format!("/entries/{private_file_id}"),
            cookie(0),
            &trash_headers,
            "",
        )
        .await;
    assert_eq!(
        status,
        204,
        "owner trash control: {}",
        String::from_utf8_lossy(&body)
    );
    let restore_path = format!("/trash/{private_file_id}/restore");
    for (actor, csrf) in [(1, OTHER_CSRF), (2, ADMIN_CSRF), (3, CSRF), (0, CSRF)] {
        let headers = [
            ("Origin", "https://127.0.0.1".to_owned()),
            ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(csrf)),
            ("If-Match", format!("\"{private_file_id}:3\"")),
        ];
        let (status, _, body) = f
            .request("POST", &restore_path, cookie(actor), &headers, "")
            .await;
        assert_eq!(
            status,
            match actor {
                0 => 204,
                3 => 401,
                _ => 404,
            },
            "restore actor={actor}: {}",
            String::from_utf8_lossy(&body)
        );
        if actor != 0 {
            assert!(!String::from_utf8_lossy(&body).contains("private-file.bin"));
            let (owner_status, _, _) = f.request("GET", &content_path, cookie(0), &[], "").await;
            assert_eq!(owner_status, 404, "denied restore changed deleted state");
        }
    }
    let (status, _, body) = f.request("GET", &content_path, cookie(0), &[], "").await;
    assert_eq!(status, 200);
    assert_eq!(body, b"synthetic isolation payload");
    namespace_rows
        .push("owner controls: move=200 trash=204 restore=204 exact_bytes=true".to_owned());
    namespace_rows.push(format!(
        "POST {restore_path} unrelated=404 admin=404 anonymous=401"
    ));

    let mut evidence =
        String::from("scope=synthetic-loopback-tls-s1\nactors=owner,unrelated,admin,anonymous\n");
    for (method, path, outcomes) in &matrix {
        let sanitized_path = path.replace(&upload_id, "<seeded-owner-attempt>");
        evidence.push_str(&format!(
            "{method} {sanitized_path} owner={} unrelated={} admin={} anonymous={}\n",
            outcomes[0].0, outcomes[1].0, outcomes[2].0, outcomes[3].0
        ));
    }
    for row in &upload_rows {
        evidence.push_str(row);
        evidence.push('\n');
    }
    for row in &extra_rows {
        evidence.push_str(row);
        evidence.push('\n');
    }
    for row in &namespace_rows {
        evidence.push_str(row);
        evidence.push('\n');
    }
    evidence.push_str("unsupported_method_inventory (owner-authenticated, real router):\n");
    for row in &negative_route_rows {
        evidence.push_str(row);
        evidence.push('\n');
    }
    let evidence_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    fs::create_dir_all(&evidence_dir).unwrap();
    fs::write(evidence_dir.join("t027-isolation-matrix.txt"), evidence).unwrap();
    // Keep matrix generated locally and sanitized; test output contains method/path/status only.
    println!(
        "A03 matrix rows={} actors=owner,unrelated,admin,anonymous sanitized=true",
        matrix.len() * 4
    );
    f.close().await;
}
