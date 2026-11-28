//! Real-route content contracts over validated loopback TLS.
#[path = "../common/mod.rs"]
mod common;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{
    FixtureRoot,
    server::{TlsApiServer, TlsFiles, tls_connect},
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use sylux::{
    domain::authz::verifier,
    http::{self, AuthState},
    persistence::{
        root, schema,
        sqlite::{self, SqliteWorkers, WorkerConfig},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const OWNER: &str = "00000000000000000000000000000001";
const OTHER: &str = "00000000000000000000000000000002";
const ROOT: &str = "00000000000000000000000000000101";
const OTHER_ROOT: &str = "00000000000000000000000000000102";
const SESSION: &str = "00000000000000000000000000000201";
const OTHER_SESSION: &str = "00000000000000000000000000000202";
const SECRET: &[u8; 32] = b"s1-owner-secret-0123456789abcdef";
const OTHER_SECRET: &[u8; 32] = b"s1-other-secret-0123456789abcdef";
const CSRF: &[u8; 32] = b"s1-owner-csrf-0123456789abcdef!!";
const OTHER_CSRF: &[u8; 32] = b"s1-other-csrf-0123456789abcdef!!";
const BINARY_FILE: (&str, &[u8]) = ("unsafe <name>.bin", b"synthetic\0content\xff\x80");
const BUSY: Duration = Duration::from_millis(375);

struct Fixture {
    root: FixtureRoot,
    server: Option<TlsApiServer>,
    workers: Option<SqliteWorkers>,
    ca: Vec<u8>,
}
impl Fixture {
    async fn new(case: &str) -> Self {
        Self::start(case, None, BINARY_FILE).await
    }

    async fn start(case: &str, legacy: Option<&LegacyLinks>, file: (&str, &[u8])) -> Self {
        let parent = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("sylux-content-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let root = FixtureRoot::create(&parent, "content", case).unwrap();
        let product = root.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        let initialized = root::initialize_empty_root(&product, BUSY).unwrap();
        let tls = TlsFiles::generate(&root, "tls").unwrap();
        let ca = fs::read(&tls.ca_der).unwrap();
        let db_path = product.join("metadata/sylux.sqlite3");
        let legacy_identity = legacy.map(|_| {
            let mut identity = initialized.identity;
            identity.schema_version = 3;
            fs::write(
                product.join(root::MANIFEST_NAME),
                serde_json::to_vec(&identity).unwrap(),
            )
            .unwrap();
            fs::remove_file(&db_path).unwrap();
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&db_path)
                .unwrap();
            identity
        });
        let connection = sqlite::open_writer(&db_path, BUSY).unwrap();
        if let Some(identity) = &legacy_identity {
            connection.execute_batch(schema::MIGRATION_0001_S1).unwrap();
            schema::migrate_v1(&connection, None).unwrap();
            schema::migrate_v2(&connection, None).unwrap();
            schema::insert_installation_row(&connection, identity, [0; 32]).unwrap();
        }
        seed(&connection);
        seed_file(&connection, &product, file);
        let connection = if let Some(links) = legacy {
            links.seed(&connection);
            drop(connection);
            let bound = root::open_bound_database(&product, BUSY).unwrap();
            let identity = legacy_identity.as_ref().unwrap();
            let upgraded = root::upgrade_binding(&bound, &product, identity).unwrap();
            assert_eq!(upgraded.schema_version, schema::SUPPORTED_SCHEMA_VERSION);
            drop(bound);
            let rebound = root::open_bound_database(&product, BUSY).unwrap();
            assert_eq!(
                root::upgrade_binding(&rebound, &product, &upgraded).unwrap(),
                upgraded
            );
            rebound.connection
        } else {
            connection
        };
        let workers = SqliteWorkers::from_writer(
            connection,
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
    ) -> (u16, BTreeMap<String, String>, Vec<u8>) {
        self.request_with_body(method, path, actor, headers, &[])
            .await
    }

    async fn request_with_body(
        &self,
        method: &str,
        path: &str,
        actor: Option<(&str, &[u8; 32])>,
        headers: &[(&str, String)],
        body: &[u8],
    ) -> (u16, BTreeMap<String, String>, Vec<u8>) {
        let server = self.server.as_ref().unwrap();
        let mut stream = tls_connect(server.address(), &self.ca, "127.0.0.1")
            .await
            .unwrap();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n",
            server.address().port()
        );
        if let Some((id, secret)) = actor {
            req.push_str(&format!(
                "Cookie: __Host-sylux={id}.{}\r\n",
                URL_SAFE_NO_PAD.encode(secret)
            ));
        }
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if !body.is_empty() {
            req.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
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
fn seed(c: &Connection) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for (account, root, handle, sid, secret, csrf) in [
        (OWNER, ROOT, "owner", SESSION, SECRET, CSRF),
        (
            OTHER,
            OTHER_ROOT,
            "other",
            OTHER_SESSION,
            OTHER_SECRET,
            OTHER_CSRF,
        ),
    ] {
        c.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,?2,'user',1,0)",params![account,handle]).unwrap();
        c.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)",params![root,account]).unwrap();
        c.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?4,?5,?6,?7,0)",params![sid,account,verifier(secret).as_slice(),verifier(csrf).as_slice(),now,now+3600,now+7200]).unwrap();
    }
}
fn seed_file(c: &Connection, product: &std::path::Path, (name, bytes): (&str, &[u8])) {
    let name_key = sylux::domain::names::EntryName::parse(name).unwrap();

    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let blob_id = "abcdef0123456789abcdef0123456789";
    let attempt_id = "1234567890abcdef1234567890abcdef";
    let store = sylux::persistence::payload::PayloadStore::open(product).unwrap();
    let mut staged = store
        .start(&format!("{attempt_id}-{blob_id}"), bytes.len() as u64)
        .unwrap();
    staged.write_chunk(bytes).unwrap();
    let published = staged.publish(digest).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let tx = c.unchecked_transaction().unwrap();
    tx.execute(
        "INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,?2,?3,?4,'live')",
        params![
            published.blob_id,
            &published.blob_id[..2],
            bytes.len() as i64,
            digest.as_slice()
        ],
    )
    .unwrap();
    tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,current_revision_id,generation) VALUES('00000000000000000000000000000301',?1,?2,'file',?3,?4,1,NULL,1)", params![OWNER, ROOT, name, name_key.key]).unwrap();
    tx.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES('00000000000000000000000000000302','00000000000000000000000000000301',?1,?2,?3,?4,?5)", params![published.blob_id, bytes.len() as i64, digest.as_slice(), now, OWNER]).unwrap();
    tx.execute("UPDATE entry SET current_revision_id='00000000000000000000000000000302' WHERE entry_id='00000000000000000000000000000301'", []).unwrap();
    tx.commit().unwrap();
}

fn auth<'a>(id: &'a str, secret: &'a [u8; 32]) -> (&'a str, &'a [u8; 32]) {
    (id, secret)
}

#[tokio::test]
async fn a07_current_content_has_revision_etag_safe_attachment_and_exact_bytes() {
    let f = Fixture::new("a07-current-content").await;
    let id = "00000000000000000000000000000301";
    let bytes = b"synthetic\0content\xff\x80";
    let (status, headers, body) = f
        .request(
            "GET",
            &format!("/entries/{id}/content"),
            Some(auth(SESSION, SECRET)),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        body, bytes,
        "download must preserve original binary bytes exactly"
    );
    let etag = headers.get("etag").expect("strong revision ETag").clone();
    assert!(
        etag.starts_with('"') && etag.ends_with('"') && !etag.starts_with("W/"),
        "ETag must be strong: {etag}"
    );
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/octet-stream")
    );
    assert_eq!(
        headers.get("x-content-type-options").map(String::as_str),
        Some("nosniff")
    );
    assert_eq!(
        headers.get("referrer-policy").map(String::as_str),
        Some("no-referrer")
    );
    assert_eq!(
        headers.get("cache-control").map(String::as_str),
        Some("no-store"),
        "private content responses must not be stored by caches"
    );
    let disposition = headers
        .get("content-disposition")
        .expect("attachment disposition");
    assert!(disposition.starts_with("attachment;") && disposition.contains("filename*=UTF-8''"));
    let (status, headers, body) = f
        .request(
            "GET",
            &format!("/entries/{id}/content"),
            Some(auth(SESSION, SECRET)),
            &[("If-None-Match", etag)],
        )
        .await;
    assert_eq!(
        status, 304,
        "matching current revision validator must return Not Modified"
    );
    assert!(body.is_empty());
    assert!(headers.contains_key("etag"));
    let (status, headers, body) = f
        .request(
            "HEAD",
            &format!("/entries/{id}/content"),
            Some(auth(SESSION, SECRET)),
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert!(body.is_empty(), "HEAD has no body");
    assert_eq!(
        headers.get("content-length").map(String::as_str),
        Some(bytes.len().to_string().as_str())
    );
    f.close().await;
}

#[tokio::test]
async fn a08_authorizes_before_conditional_response_disclosure() {
    let f = Fixture::new("a08-auth-before-condition").await;
    // Both a guessed private ID and the owner's root fail as content resources; a cache
    // validator must not turn either request into a revealing conditional response.
    let id = "0000000000000000000000000000eeee";
    for (method, path, actor) in [
        (
            "GET",
            format!("/entries/{id}/content"),
            Some(auth(OTHER_SESSION, OTHER_SECRET)),
        ),
        (
            "HEAD",
            format!("/entries/{ROOT}/content"),
            Some(auth(SESSION, SECRET)),
        ),
    ] {
        let (status, _, _) = f
            .request(method, &path, actor, &[("If-None-Match", "*".into())])
            .await;
        assert_eq!(
            status, 404,
            "{method} must authorize/resolve before evaluating validators"
        );
    }
    let (status, _, _) = f
        .request(
            "GET",
            &format!("/entries/{id}/content"),
            None,
            &[("If-None-Match", "*".into())],
        )
        .await;
    assert_eq!(
        status, 401,
        "anonymous conditional content requests require authentication"
    );
    f.close().await;
}

#[tokio::test]
async fn a08_missing_precondition_and_out_of_slice_methods_are_not_content_reads() {
    let f = Fixture::new("a08-content-boundary").await;
    // The endpoint is read-only: write methods and future history routes stay absent.
    let id = "0000000000000000000000000000dddd";
    let (status, _, _) = f
        .request(
            "PUT",
            &format!("/entries/{id}/content"),
            Some(auth(SESSION, SECRET)),
            &[],
        )
        .await;
    assert!(
        status == 404 || status == 405,
        "content route must not expose a write operation: {status}"
    );
    let (status,_,_)=f.request("GET","/entries/00000000000000000000000000000001/versions/00000000000000000000000000000002/content",Some(auth(SESSION,SECRET)),&[]).await;
    assert_eq!(status, 404, "history content is outside S1");
    f.close().await;
}

const SHARE_FILE: &str = "00000000000000000000000000000301";
const SHARE_PARENT: &str = "00000000000000000000000000000303";
const SHARE_ORIGIN: &str = "https://127.0.0.1";
const LEGACY_REVISION: &str = "00000000000000000000000000000302";
const LEGACY_NESTED: &str = "00000000000000000000000000000304";

struct LegacyLinks {
    now: i64,
}

impl LegacyLinks {
    fn path(marker: u8) -> String {
        format!("/share/{}", URL_SAFE_NO_PAD.encode([marker; 32]))
    }

    fn seed(&self, db: &Connection) {
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            3
        );
        for (owner, root, parent, nested) in [
            (OWNER, ROOT, SHARE_PARENT, LEGACY_NESTED),
            (
                OTHER,
                OTHER_ROOT,
                "00000000000000000000000000000305",
                "00000000000000000000000000000306",
            ),
        ] {
            for (id, under, name) in [(parent, root, "Projects"), (nested, parent, "Nested")] {
                db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder',?4,?5,1)", params![id, owner, under, name, name.as_bytes()]).unwrap();
            }
        }
        db.execute(
            "UPDATE entry SET parent_id=?1 WHERE entry_id=?2",
            params![LEGACY_NESTED, SHARE_FILE],
        )
        .unwrap();
        let file = "00000000000000000000000000000307";
        let revision = "00000000000000000000000000000308";
        let parent = "00000000000000000000000000000306";
        db.execute(
            "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
             VALUES(?1,?2,?3,'file','second.bin',CAST('second.bin' AS BLOB),1)",
            params![file, OTHER, parent],
        ).unwrap();
        db.execute(
            "INSERT INTO revision(revision_id, entry_id, blob_id, length, sha256, committed_at, actor_account_id)
             SELECT ?1, ?2, blob_id, length, sha256, committed_at, ?3 FROM revision WHERE revision_id=?4",
            params![revision, file, OTHER, LEGACY_REVISION],
        ).unwrap();
        db.execute(
            "UPDATE entry SET current_revision_id=?1 WHERE entry_id=?2",
            params![revision, file],
        )
        .unwrap();
        for (marker, created) in [(1_u8, self.now), (2, self.now - 86401), (3, self.now)] {
            db.execute("INSERT INTO share_link(share_id,verifier,owner_account_id,entry_id,revision_id,created_at,expires_at)
                VALUES(?1,?2,?3,?4,?5,?6,?7)", params![format!("{marker:032x}"), verifier(&[marker; 32]).as_slice(), OWNER, SHARE_FILE, LEGACY_REVISION, created, created+86400]).unwrap();
        }
        db.execute(
            "DELETE FROM share_link WHERE share_id=?1",
            [format!("{:032x}", 3)],
        )
        .unwrap();
    }
}

fn mutation_headers(csrf: &[u8; 32]) -> [(&'static str, String); 3] {
    [
        ("Origin", SHARE_ORIGIN.to_owned()),
        ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(csrf)),
        ("Content-Type", "application/json".to_owned()),
    ]
}

const GRANT_ADMIN: &str = "00000000000000000000000000000003";
const GRANT_ADMIN_SESSION: &str = "00000000000000000000000000000203";
const GRANT_ADMIN_SECRET: &[u8; 32] = b"direct-grant-admin-session-12345";
const GRANT_ADMIN_CSRF: &[u8; 32] = b"direct-grant-admin-csrf-12345678";
const GRANT_OWNER_ACTOR: ShareActor = (SESSION, SECRET, CSRF);
const GRANT_RECIPIENT_ACTOR: ShareActor = (OTHER_SESSION, OTHER_SECRET, OTHER_CSRF);
const GRANT_ADMIN_ACTOR: ShareActor = (GRANT_ADMIN_SESSION, GRANT_ADMIN_SECRET, GRANT_ADMIN_CSRF);
type ShareActor = (&'static str, &'static [u8; 32], &'static [u8; 32]);

impl Fixture {
    async fn share_request(
        &self,
        method: &str,
        path: &str,
        actor: ShareActor,
        body: serde_json::Value,
        etag: Option<&str>,
        confirmation: Option<&str>,
    ) -> (u16, serde_json::Value) {
        let mut headers = mutation_headers(actor.2).to_vec();
        if let Some(etag) = etag {
            headers.push(("If-Match", etag.to_owned()));
        }
        if let Some(token) = confirmation {
            headers.push(("X-Share-Confirmation", token.to_owned()));
        }
        let body = (!body.is_null()).then(|| body.to_string());
        let (status, _, bytes) = self
            .request_with_body(
                method,
                path,
                Some((actor.0, actor.1)),
                &headers,
                body.as_deref().unwrap_or("").as_bytes(),
            )
            .await;
        let reply = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("expected JSON")
        };
        (status, reply)
    }

    async fn share_get(&self, path: &str, actor: ShareActor) -> (u16, serde_json::Value) {
        self.share_request("GET", path, actor, serde_json::Value::Null, None, None)
            .await
    }

    async fn share_folder(&self, parent: &str, name: &str) -> String {
        let (status, reply) = self
            .share_request(
                "POST",
                &format!("/folders/{parent}/children"),
                GRANT_OWNER_ACTOR,
                serde_json::json!({"name": name}),
                None,
                None,
            )
            .await;
        assert_eq!(status, 201, "{reply}");
        reply["id"].as_str().unwrap().to_owned()
    }

    async fn share_grant(&self, root: &str, handle: &str) -> String {
        let (status, reply) = self
            .share_request(
                "POST",
                &format!("/entries/{root}/direct-grants"),
                GRANT_OWNER_ACTOR,
                serde_json::json!({"handle": handle}),
                None,
                None,
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        assert_eq!(reply["handle"], handle);
        reply["id"].as_str().unwrap().to_owned()
    }

    async fn share_revoke(&self, id: &str) {
        let (status, _) = self
            .share_request(
                "DELETE",
                &format!("/direct-grants/{id}"),
                GRANT_OWNER_ACTOR,
                serde_json::Value::Null,
                None,
                None,
            )
            .await;
        assert_eq!(status, 204);
    }

    async fn seed_grant_admin(&self) {
        self.workers
            .as_ref()
            .unwrap()
            .transact(|tx| {
                tx.execute(
                    "INSERT INTO account(account_id,handle,role,enabled,credential_epoch)
                 VALUES(?1,'unrelated-admin','administrator',1,0)",
                    [GRANT_ADMIN],
                )?;
                fresh_grant_session(tx, GRANT_ADMIN, GRANT_ADMIN_ACTOR)
            })
            .await
            .unwrap();
    }

    async fn change_grant_authority(&self, account: &'static str, action: &str, actor: ShareActor) {
        let (status, _) = self
            .share_request(
                "POST",
                &format!("/accounts/{account}/{action}"),
                GRANT_ADMIN_ACTOR,
                serde_json::Value::Null,
                None,
                None,
            )
            .await;
        assert_eq!(status, 204);
        if action == "enable" {
            self.workers
                .as_ref()
                .unwrap()
                .transact(move |tx| fresh_grant_session(tx, account, actor))
                .await
                .unwrap();
        }
    }
}

fn fresh_grant_session(db: &Connection, account: &str, actor: ShareActor) -> rusqlite::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    db.execute("DELETE FROM session WHERE session_id=?1", [actor.0])?;
    db.execute(
        "INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,
                             created_at,idle_expires_at,absolute_expires_at,revoked)
         SELECT ?1,account_id,credential_epoch,?3,?4,?5,?6,?7,0 FROM account WHERE account_id=?2",
        params![
            actor.0,
            account,
            verifier(actor.1).as_slice(),
            verifier(actor.2).as_slice(),
            now,
            now + 3600,
            now + 7200
        ],
    )?;
    Ok(())
}

#[tokio::test]
async fn direct_grant_read_surface_is_live_independent_and_owner_write_only() {
    use serde_json::{Value, json};
    let f = Fixture::new("direct-grant-reads").await;
    f.seed_grant_admin().await;
    let private = f.share_folder(ROOT, "Private ancestor").await;
    let shared = f.share_folder(&private, "Shared project").await;
    let sibling = f.share_folder(&private, "Private sibling").await;
    let nested = f.share_folder(&shared, "Nested").await;
    let tag = sylux::domain::upload::folder_etag(SHARE_FILE, 1);
    let (status, moved) = f
        .share_request(
            "POST",
            &format!("/entries/{SHARE_FILE}/move"),
            GRANT_OWNER_ACTOR,
            json!({"parent_id": nested}),
            Some(&tag),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let entry_tag = moved["etag"].as_str().unwrap();
    let library_grant = f.share_grant(ROOT, "other").await;
    let folder_grant = f.share_grant(&shared, "other").await;
    assert_eq!(f.share_grant(&shared, "other").await, folder_grant);
    let (status, discover) = f.share_get("/shared", GRANT_RECIPIENT_ACTOR).await;
    assert_eq!(status, 200);
    let roots = discover["items"].as_array().unwrap();
    assert_eq!(roots.len(), 2);
    for root in [ROOT, shared.as_str()] {
        assert!(
            roots
                .iter()
                .any(|item| item["entry"]["id"] == root && item["entry"]["parent_id"].is_null())
        );
    }
    assert_eq!(
        f.share_get("/shared", GRANT_ADMIN_ACTOR).await.1["items"],
        json!([])
    );
    let folder_url = format!("/shared/folders/{nested}/path");
    let (status, overlapping_path) = f.share_get(&folder_url, GRANT_RECIPIENT_ACTOR).await;
    assert_eq!(status, 200);
    let ids: Vec<_> = overlapping_path["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [ROOT, private.as_str(), shared.as_str(), nested.as_str()]
    );
    let (status, overlapping_search) = f
        .share_get("/shared/search?q=unsafe", GRANT_RECIPIENT_ACTOR)
        .await;
    assert_eq!(status, 200);
    let hits = overlapping_search["items"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["entry"]["id"], SHARE_FILE);
    assert_eq!(hits[0]["location"], overlapping_path["items"]);

    f.share_revoke(&library_grant).await;
    let file_url = format!("/shared/entries/{SHARE_FILE}");
    let bytes_url = format!("{file_url}/content");
    let (status, path) = f.share_get(&folder_url, GRANT_RECIPIENT_ACTOR).await;
    assert_eq!(status, 200);
    assert_eq!(path["items"].as_array().unwrap().len(), 2);
    assert_eq!(path["items"][0]["id"], shared);
    assert_eq!(path["items"][0]["parent_id"], Value::Null);
    assert_eq!(path["items"][1]["id"], nested);
    for private_id in [&private, &sibling, &ROOT.to_owned()] {
        assert_eq!(
            f.share_get(
                &format!("/shared/folders/{private_id}/path"),
                GRANT_RECIPIENT_ACTOR
            )
            .await
            .0,
            404
        );
    }
    assert_eq!(
        f.share_get(
            &format!("/shared/folders/{nested}/children"),
            GRANT_RECIPIENT_ACTOR
        )
        .await
        .1["items"][0]["id"],
        SHARE_FILE
    );
    let (status, file) = f.share_get(&file_url, GRANT_RECIPIENT_ACTOR).await;
    assert_eq!(status, 200);
    assert_eq!(file["location"], path["items"]);
    let (status, search) = f
        .share_get("/shared/search?q=unsafe", GRANT_RECIPIENT_ACTOR)
        .await;
    assert_eq!(status, 200);
    assert_eq!(search["items"].as_array().unwrap().len(), 1);
    assert_eq!(search["items"][0]["location"], path["items"]);
    assert_eq!(
        f.share_get("/shared/search?q=Private", GRANT_RECIPIENT_ACTOR)
            .await
            .1["items"],
        json!([])
    );
    assert_eq!(f.share_get(&file_url, GRANT_ADMIN_ACTOR).await.0, 404);
    assert_eq!(f.request("GET", &bytes_url, None, &[]).await.0, 401);
    let recipient = Some(auth(OTHER_SESSION, OTHER_SECRET));
    let (status, headers, bytes) = f.request("GET", &bytes_url, recipient, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(bytes, b"synthetic\0content\xff\x80");
    let content_tag = headers["etag"].clone();
    let children_url = format!("/shared/folders/{nested}/children");
    for (method, url) in [
        ("GET", "/shared"),
        ("GET", folder_url.as_str()),
        ("GET", children_url.as_str()),
        ("GET", "/shared/search?q=unsafe"),
        ("GET", file_url.as_str()),
        ("HEAD", bytes_url.as_str()),
    ] {
        let (status, headers, _) = f.request(method, url, recipient, &[]).await;
        assert_eq!(status, 200, "{method} {url}");
        assert_eq!(headers["cache-control"], "no-store", "{method} {url}");
        assert_eq!(headers["referrer-policy"], "no-referrer", "{method} {url}");
    }
    assert_eq!(
        f.request(
            "GET",
            &bytes_url,
            recipient,
            &[("Range", "bytes=0-2".into())]
        )
        .await
        .2,
        b"syn"
    );
    assert_eq!(
        f.request(
            "GET",
            &format!("/entries/{SHARE_FILE}/content"),
            recipient,
            &[]
        )
        .await
        .0,
        404
    );
    assert_eq!(
        f.share_get(
            &format!("/folders/{nested}/children"),
            GRANT_RECIPIENT_ACTOR
        )
        .await
        .0,
        404
    );
    let (status, reply) = f
        .share_request(
            "POST",
            &format!("/folders/{nested}/children"),
            GRANT_RECIPIENT_ACTOR,
            json!({"name":"New folder"}),
            None,
            None,
        )
        .await;
    assert_eq!(status, 404, "{reply}");
    for (method, path, body) in [
        (
            "PATCH",
            format!("/entries/{SHARE_FILE}"),
            json!({"name":"changed"}),
        ),
        ("DELETE", format!("/entries/{SHARE_FILE}"), Value::Null),
        (
            "POST",
            format!("/entries/{SHARE_FILE}/move"),
            json!({"parent_id":OTHER_ROOT}),
        ),
        (
            "POST",
            format!("/entries/{shared}/direct-grants"),
            json!({"handle":"unrelated-admin"}),
        ),
        (
            "DELETE",
            format!("/direct-grants/{folder_grant}"),
            Value::Null,
        ),
        (
            "POST",
            "/share-links".into(),
            json!({"entry_id":SHARE_FILE}),
        ),
        (
            "POST",
            "/archives".into(),
            json!({"entry_ids":[SHARE_FILE]}),
        ),
    ] {
        assert_eq!(
            f.share_request(
                method,
                &path,
                GRANT_RECIPIENT_ACTOR,
                body,
                Some(entry_tag),
                None
            )
            .await
            .0,
            404,
            "{path}"
        );
    }
    let (status, copy) = f
        .share_request(
            "POST",
            &format!("/entries/{SHARE_FILE}/copy-ticket"),
            GRANT_RECIPIENT_ACTOR,
            json!({"parent_id":OTHER_ROOT}),
            Some(entry_tag),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        f.share_request("POST", "/copies", GRANT_RECIPIENT_ACTOR, copy, None, None)
            .await
            .0,
        404
    );
    let (status, ticket) = f
        .share_request(
            "POST",
            "/archives",
            GRANT_OWNER_ACTOR,
            json!({"entry_ids":[&shared]}),
            None,
            None,
        )
        .await;
    assert_eq!(status, 201);
    let zip = ticket["url"].as_str().unwrap();
    assert_eq!(f.request("GET", zip, recipient, &[]).await.0, 404);
    let (status, _, archive) = f
        .request("GET", zip, Some(auth(SESSION, SECRET)), &[])
        .await;
    assert_eq!(status, 200);
    assert!(archive.windows(4).any(|bytes| bytes == b"PK\x03\x04"));

    f.share_revoke(&folder_grant).await;
    assert_eq!(
        f.share_get("/shared", GRANT_RECIPIENT_ACTOR).await.1["items"],
        json!([])
    );
    assert_eq!(
        f.share_get("/shared/search?q=unsafe", GRANT_RECIPIENT_ACTOR)
            .await
            .1["items"],
        json!([])
    );
    assert_eq!(f.share_get(&folder_url, GRANT_RECIPIENT_ACTOR).await.0, 404);
    assert_eq!(f.share_get(&file_url, GRANT_RECIPIENT_ACTOR).await.0, 404);
    assert_eq!(
        f.request(
            "GET",
            &bytes_url,
            recipient,
            &[("If-None-Match", content_tag)]
        )
        .await
        .0,
        404
    );
    for (account, actor, disabled_status) in [
        (OWNER, GRANT_OWNER_ACTOR, 404),
        (OTHER, GRANT_RECIPIENT_ACTOR, 401),
    ] {
        f.share_grant(&shared, "other").await;
        f.change_grant_authority(account, "disable", actor).await;
        assert_eq!(
            f.share_get(&file_url, GRANT_RECIPIENT_ACTOR).await.0,
            disabled_status
        );
        f.change_grant_authority(account, "enable", actor).await;
        assert_eq!(f.share_get(&file_url, GRANT_RECIPIENT_ACTOR).await.0, 404);
        assert_eq!(
            f.share_get("/shared", GRANT_RECIPIENT_ACTOR).await.1["items"],
            json!([])
        );
    }
    f.close().await;
}

#[tokio::test]
async fn direct_grant_listing_and_search_paginate_with_isolated_cursors() {
    use serde_json::json;

    let f = Fixture::new("direct-grant-cursors").await;
    f.seed_grant_admin().await;
    let shared = f.share_folder(ROOT, "Project").await;
    let first = f.share_folder(&shared, "Report A").await;
    let second = f.share_folder(&shared, "Report B").await;
    f.share_grant(&shared, "other").await;
    f.share_grant(&shared, "unrelated-admin").await;

    let (status, library) = f
        .share_request(
            "POST",
            "/libraries",
            GRANT_OWNER_ACTOR,
            json!({"name":"Search publication"}),
            None,
            None,
        )
        .await;
    assert_eq!(status, 201);
    let published = library["id"].as_str().unwrap();
    f.share_folder(published, "Report C").await;
    f.share_folder(published, "Report D").await;
    assert_eq!(
        f.share_request(
            "POST",
            &format!("/libraries/{published}/publication"),
            GRANT_OWNER_ACTOR,
            json!(null),
            None,
            None
        )
        .await
        .0,
        200
    );
    for folder in [&shared, published] {
        let direct = format!("/shared/search?q=Report&folder_id={folder}&limit=1");
        let public = format!("/published/search?q=Report&folder_id={folder}&limit=1");
        let (status, direct_page) = f.share_get(&direct, GRANT_RECIPIENT_ACTOR).await;
        assert_eq!(status, 200);
        let (status, public_page) = f.share_get(&public, GRANT_RECIPIENT_ACTOR).await;
        assert_eq!(status, 200);
        for (url, page, other) in [
            (&direct, &direct_page, &public),
            (&public, &public_page, &direct),
        ] {
            let cursor = page["next_cursor"].as_str().unwrap();
            assert_eq!(
                f.share_get(&format!("{url}&cursor={cursor}"), GRANT_RECIPIENT_ACTOR)
                    .await
                    .0,
                200
            );
            let (status, error) = f
                .share_get(&format!("{other}&cursor={cursor}"), GRANT_RECIPIENT_ACTOR)
                .await;
            assert_eq!(status, 400);
            assert_eq!(error["code"], "invalid_cursor");
        }
    }
    for (url, expected) in [
        ("/shared/search?q=Report", &first),
        ("/published/search?q=Report", &published.to_string()),
    ] {
        let (status, page) = f.share_get(url, GRANT_RECIPIENT_ACTOR).await;
        assert_eq!(status, 200);
        let items = page["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        if url.starts_with("/shared/") {
            assert!(items.iter().any(|v| v["entry"]["id"] == *expected));
        } else {
            assert!(
                items
                    .iter()
                    .all(|v| v["entry"]["id"] != first && v["entry"]["id"] != second)
            );
        }
    }

    for (private, shared_url, other_scope, id_field) in [
        (
            format!("/folders/{shared}/children?limit=1"),
            format!("/shared/folders/{shared}/children?limit=1"),
            format!("/shared/folders/{first}/children?limit=1"),
            "/id",
        ),
        (
            format!("/entries/search?q=Report&folder_id={shared}&limit=1"),
            format!("/shared/search?q=Report&folder_id={shared}&limit=1"),
            format!("/shared/search?q=Report&folder_id={first}&limit=1"),
            "/entry/id",
        ),
    ] {
        let (status, page) = f.share_get(&shared_url, GRANT_RECIPIENT_ACTOR).await;
        assert_eq!(status, 200, "{shared_url}");
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0].pointer(id_field).unwrap(), &json!(first));
        assert_eq!(page["has_more"], true);
        let shared_cursor = page["next_cursor"].as_str().unwrap();
        let (status, next) = f
            .share_get(
                &format!("{shared_url}&cursor={shared_cursor}"),
                GRANT_RECIPIENT_ACTOR,
            )
            .await;
        assert_eq!(status, 200, "{next}");
        assert_eq!(next["items"].as_array().unwrap().len(), 1);
        assert_eq!(next["items"][0].pointer(id_field).unwrap(), &json!(second));
        assert_eq!(next["has_more"], false);
        assert!(next["next_cursor"].is_null());

        let (status, page) = f.share_get(&private, GRANT_OWNER_ACTOR).await;
        assert_eq!(status, 200, "{page}");
        let private_cursor = page["next_cursor"].as_str().unwrap();
        // Cross-route requests also change actor, so pin both token versions independently.
        for (cursor, version) in [(private_cursor, 3), (shared_cursor, 4)] {
            let (payload, _) = cursor.split_once('.').unwrap();
            let payload: serde_json::Value =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
            assert_eq!(payload["version"], version);
        }
        for (url, cursor, actor) in [
            (&private, shared_cursor, GRANT_OWNER_ACTOR),
            (&shared_url, private_cursor, GRANT_RECIPIENT_ACTOR),
            (&shared_url, shared_cursor, GRANT_ADMIN_ACTOR),
            (&other_scope, shared_cursor, GRANT_RECIPIENT_ACTOR),
        ] {
            let (status, reply) = f.share_get(&format!("{url}&cursor={cursor}"), actor).await;
            assert_eq!(status, 400, "{url}: {reply}");
            assert_eq!(reply["code"], "invalid_cursor");
        }
    }
    f.close().await;
}

#[tokio::test]
async fn direct_grant_does_not_authorize_upload_admission_or_text_edit() {
    use serde_json::json;
    use sylux::domain::upload::{folder_etag, hex, revision_etag};

    let f = Fixture::start("direct-grant-text", None, ("report.txt", b"Original text")).await;
    f.share_grant(ROOT, "other").await;
    let (status, file) = f
        .share_get(
            &format!("/shared/entries/{SHARE_FILE}"),
            GRANT_RECIPIENT_ACTOR,
        )
        .await;
    assert_eq!(status, 200);
    let revision_id = "00000000000000000000000000000302";
    let recipient = Some(auth(OTHER_SESSION, OTHER_SECRET));
    let (status, headers, _) = f
        .request(
            "HEAD",
            &format!("/shared/entries/{SHARE_FILE}/content"),
            recipient,
            &[],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(headers["etag"], revision_etag(revision_id));

    let mut admission = json!({
        "parent_id": ROOT,
        "parent_etag": folder_etag(ROOT, 1),
        "name": "new.txt",
        "length": b"Changed text".len(),
        "sha256": hex(&Sha256::digest(b"Changed text")),
    });
    let mut ticket_body = admission.clone();
    ticket_body["scope"] = json!("upload_create");
    let (status, ticket) = f
        .share_request(
            "POST",
            "/mutation-tickets",
            GRANT_RECIPIENT_ACTOR,
            ticket_body,
            None,
            None,
        )
        .await;
    assert_eq!(status, 201, "{ticket}");
    let mut headers = mutation_headers(OTHER_CSRF).to_vec();
    headers.push(("If-None-Match", "*".to_owned()));
    headers.push((
        "X-Mutation-ID",
        ticket["ticket"].as_str().unwrap().to_owned(),
    ));
    let (status, _, reply) = f
        .request_with_body(
            "POST",
            "/uploads",
            recipient,
            &headers,
            admission.to_string().as_bytes(),
        )
        .await;
    assert_eq!(status, 404);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&reply).unwrap()["code"],
        "parent_not_found"
    );

    admission["scope"] = json!("upload_create");
    admission["name"] = json!("report.txt");
    admission["replacement"] = json!({
        "entry_id": SHARE_FILE,
        "etag": file["entry"]["etag"],
        "text_revision": revision_id,
    });
    let (status, reply) = f
        .share_request(
            "POST",
            "/mutation-tickets",
            GRANT_RECIPIENT_ACTOR,
            admission.clone(),
            None,
            None,
        )
        .await;
    assert_eq!(status, 404, "{reply}");
    assert_eq!(reply["code"], "entry_not_found");
    let (status, reply) = f
        .share_request(
            "POST",
            "/mutation-tickets",
            GRANT_OWNER_ACTOR,
            admission,
            None,
            None,
        )
        .await;
    assert_eq!(status, 201, "{reply}");
    f.close().await;
}

#[tokio::test]
async fn direct_grant_tree_changes_recheck_confirmation_and_permanently_revoke_trashed_roots() {
    use serde_json::{Value, json};
    let f = Fixture::new("direct-grant-tree").await;
    f.seed_grant_admin().await;
    let private = f.share_folder(ROOT, "Private parent").await;
    let shared = f.share_folder(&private, "Team").await;
    let carried = f.share_folder(&shared, "Carried folder").await;
    let carried_child = f.share_folder(&carried, "Inside carried folder").await;
    let shared_path = format!("/shared/folders/{shared}/path");
    let shared_listing = format!("/shared/folders/{shared}/children");
    let carried_path = format!("/shared/folders/{carried}/path");
    let carried_child_path = format!("/shared/folders/{carried_child}/path");
    let metadata = format!("/shared/entries/{SHARE_FILE}");
    let content = format!("{metadata}/content");
    let move_url = format!("/entries/{SHARE_FILE}/move");
    let file_tag = sylux::domain::upload::folder_etag(SHARE_FILE, 1);
    let mut move_body = json!({"parent_id": shared});
    let other = f.share_grant(&shared, "other").await;
    let (status, first) = f
        .share_request(
            "POST",
            &move_url,
            GRANT_OWNER_ACTOR,
            move_body.clone(),
            Some(&file_tag),
            None,
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(first["code"], "sharing_confirmation_required");
    assert_eq!(first["confirmation"]["new_recipients"], json!(["other"]));
    assert_eq!(f.share_get(&metadata, GRANT_RECIPIENT_ACTOR).await.0, 404);

    let admin = f.share_grant(&shared, "unrelated-admin").await;
    let (status, expanded) = f
        .share_request(
            "POST",
            &move_url,
            GRANT_OWNER_ACTOR,
            move_body.clone(),
            Some(&file_tag),
            first["confirmation"]["token"].as_str(),
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(
        expanded["confirmation"]["new_recipients"],
        json!(["other", "unrelated-admin"])
    );
    f.share_revoke(&admin).await;
    let replacement = f.share_grant(&shared, "unrelated-admin").await;
    let (status, recreated) = f
        .share_request(
            "POST",
            &move_url,
            GRANT_OWNER_ACTOR,
            move_body.clone(),
            Some(&file_tag),
            expanded["confirmation"]["token"].as_str(),
        )
        .await;
    assert_eq!(status, 409, "re-created grant needs a fresh confirmation");
    assert_eq!(
        recreated["confirmation"]["new_recipients"],
        expanded["confirmation"]["new_recipients"]
    );
    let token = recreated["confirmation"]["token"].as_str();
    let (status, moved) = f
        .share_request(
            "POST",
            &move_url,
            GRANT_OWNER_ACTOR,
            move_body.clone(),
            Some(&file_tag),
            token,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        f.share_request(
            "POST",
            &move_url,
            GRANT_OWNER_ACTOR,
            move_body.clone(),
            Some(&file_tag),
            token
        )
        .await
        .0,
        412
    );
    assert_eq!(f.share_get(&metadata, GRANT_RECIPIENT_ACTOR).await.0, 200);
    assert_eq!(
        f.share_get(&shared_listing, GRANT_RECIPIENT_ACTOR).await.1["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let (status, moved_out) = f
        .share_request(
            "POST",
            &move_url,
            GRANT_OWNER_ACTOR,
            json!({"parent_id":ROOT}),
            moved["etag"].as_str(),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(f.share_get(&metadata, GRANT_RECIPIENT_ACTOR).await.0, 404);
    assert_eq!(
        f.request(
            "GET",
            &content,
            Some(auth(OTHER_SESSION, OTHER_SECRET)),
            &[]
        )
        .await
        .0,
        404
    );
    assert_eq!(
        f.share_get("/shared/search?q=unsafe", GRANT_RECIPIENT_ACTOR)
            .await
            .1["items"],
        json!([])
    );

    let carried_grant = f.share_grant(&carried, "other").await;
    let (_, owner_path) = f
        .share_get(&format!("/folders/{carried}/path"), GRANT_OWNER_ACTOR)
        .await;
    let (status, renamed) = f
        .share_request(
            "PATCH",
            &format!("/entries/{carried}"),
            GRANT_OWNER_ACTOR,
            json!({"name":"Renamed share"}),
            owner_path["items"][3]["etag"].as_str(),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let carried_move = format!("/entries/{carried}/move");
    let (status, carries) = f
        .share_request(
            "POST",
            &carried_move,
            GRANT_OWNER_ACTOR,
            json!({"parent_id":ROOT}),
            renamed["etag"].as_str(),
            None,
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(
        carries["confirmation"]["carried_recipients"],
        json!(["other"])
    );
    assert_eq!(carries["confirmation"]["new_recipients"], json!([]));
    assert_eq!(
        carries["confirmation"]["lost_recipients"],
        json!(["unrelated-admin"])
    );
    let (status, relocated) = f
        .share_request(
            "POST",
            &carried_move,
            GRANT_OWNER_ACTOR,
            json!({"parent_id":ROOT}),
            renamed["etag"].as_str(),
            carries["confirmation"]["token"].as_str(),
        )
        .await;
    assert_eq!(status, 200);
    let (status, path) = f.share_get(&carried_path, GRANT_RECIPIENT_ACTOR).await;
    assert_eq!(status, 200);
    assert_eq!(path["items"].as_array().unwrap().len(), 1);
    assert_eq!(path["items"][0]["name"], "Renamed share");
    assert_eq!(path["items"][0]["parent_id"], Value::Null);
    assert_eq!(
        f.share_get(&carried_child_path, GRANT_ADMIN_ACTOR).await.0,
        404
    );
    assert_eq!(
        f.share_get(&carried_child_path, GRANT_RECIPIENT_ACTOR)
            .await
            .0,
        200
    );
    let (status, returning) = f
        .share_request(
            "POST",
            &carried_move,
            GRANT_OWNER_ACTOR,
            json!({"parent_id":shared}),
            relocated["etag"].as_str(),
            None,
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(
        returning["confirmation"]["new_recipients"],
        json!(["unrelated-admin"])
    );
    assert_eq!(
        returning["confirmation"]["carried_recipients"],
        json!(["other"])
    );
    assert_eq!(
        f.share_get(&carried_child_path, GRANT_ADMIN_ACTOR).await.0,
        404
    );
    assert_eq!(
        f.share_request(
            "POST",
            &carried_move,
            GRANT_OWNER_ACTOR,
            json!({"parent_id":shared}),
            relocated["etag"].as_str(),
            returning["confirmation"]["token"].as_str(),
        )
        .await
        .0,
        200
    );
    assert_eq!(
        f.share_get(&carried_child_path, GRANT_ADMIN_ACTOR).await.0,
        200
    );
    f.share_revoke(&carried_grant).await;

    let inside = f.share_folder(&shared, "Stays available").await;
    move_body = json!({"parent_id": shared});
    let (_, moving_back) = f
        .share_request(
            "POST",
            &move_url,
            GRANT_OWNER_ACTOR,
            move_body.clone(),
            moved_out["etag"].as_str(),
            None,
        )
        .await;
    let (status, restored_location) = f
        .share_request(
            "POST",
            &move_url,
            GRANT_OWNER_ACTOR,
            move_body,
            moved_out["etag"].as_str(),
            moving_back["confirmation"]["token"].as_str(),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        f.share_request(
            "DELETE",
            &format!("/entries/{SHARE_FILE}"),
            GRANT_OWNER_ACTOR,
            Value::Null,
            restored_location["etag"].as_str(),
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(f.share_get(&metadata, GRANT_RECIPIENT_ACTOR).await.0, 404);
    assert_eq!(
        f.share_get(
            &format!("/shared/folders/{inside}/path"),
            GRANT_RECIPIENT_ACTOR
        )
        .await
        .0,
        200
    );
    let (_, trash) = f.share_get("/trash", GRANT_OWNER_ACTOR).await;
    let deleted_tag = trash["items"][0]["etag"].as_str();
    let restore = format!("/trash/{SHARE_FILE}/restore");
    let (status, denied) = f
        .share_request(
            "POST",
            &restore,
            GRANT_RECIPIENT_ACTOR,
            Value::Null,
            deleted_tag,
            None,
        )
        .await;
    assert_eq!(status, 404, "{denied}");
    let (status, preview) = f
        .share_request(
            "POST",
            &restore,
            GRANT_OWNER_ACTOR,
            Value::Null,
            deleted_tag,
            None,
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(
        preview["confirmation"]["new_recipients"],
        json!(["other", "unrelated-admin"])
    );
    assert_eq!(f.share_get(&metadata, GRANT_RECIPIENT_ACTOR).await.0, 404);
    f.share_revoke(&other).await;
    f.share_revoke(&replacement).await;
    let (status, changed) = f
        .share_request(
            "POST",
            &restore,
            GRANT_OWNER_ACTOR,
            Value::Null,
            deleted_tag,
            preview["confirmation"]["token"].as_str(),
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(changed["code"], "sharing_audience_changed");
    f.share_grant(&shared, "other").await;
    let (_, fresh) = f
        .share_request(
            "POST",
            &restore,
            GRANT_OWNER_ACTOR,
            Value::Null,
            deleted_tag,
            None,
        )
        .await;
    assert_eq!(fresh["confirmation"]["new_recipients"], json!(["other"]));
    assert_eq!(
        f.share_request(
            "POST",
            &restore,
            GRANT_OWNER_ACTOR,
            Value::Null,
            deleted_tag,
            fresh["confirmation"]["token"].as_str()
        )
        .await
        .0,
        204
    );
    assert_eq!(f.share_get(&metadata, GRANT_RECIPIENT_ACTOR).await.0, 200);

    for root in [&shared, &private] {
        let (_, owner_path) = f
            .share_get(&format!("/folders/{root}/path"), GRANT_OWNER_ACTOR)
            .await;
        let tag = owner_path["items"].as_array().unwrap().last().unwrap()["etag"].as_str();
        assert_eq!(
            f.share_request(
                "DELETE",
                &format!("/entries/{root}"),
                GRANT_OWNER_ACTOR,
                Value::Null,
                tag,
                None
            )
            .await
            .0,
            204
        );
        assert_eq!(
            f.share_get(&shared_path, GRANT_RECIPIENT_ACTOR).await.0,
            404
        );
        let (_, trash) = f.share_get("/trash", GRANT_OWNER_ACTOR).await;
        let tag = trash["items"][0]["etag"].as_str();
        assert_eq!(
            f.share_request(
                "POST",
                &format!("/trash/{root}/restore"),
                GRANT_OWNER_ACTOR,
                Value::Null,
                tag,
                None
            )
            .await
            .0,
            204
        );
        assert_eq!(
            f.share_get(&shared_path, GRANT_RECIPIENT_ACTOR).await.0,
            404
        );
        assert_eq!(
            f.share_get("/shared", GRANT_RECIPIENT_ACTOR).await.1["items"],
            json!([])
        );
        assert_eq!(
            f.share_get(
                &format!("/entries/{shared}/direct-grants"),
                GRANT_OWNER_ACTOR
            )
            .await
            .1["items"],
            json!([])
        );
        f.share_grant(&shared, "other").await;
    }
    f.close().await;
}

async fn issue_share(f: &Fixture, days: Option<u8>) -> (u16, serde_json::Value) {
    let mut payload = serde_json::json!({"entry_id": SHARE_FILE});
    if let Some(days) = days {
        payload["expiry_days"] = days.into();
    }
    let (status, _, body) = f
        .request_with_body(
            "POST",
            "/share-links",
            Some(auth(SESSION, SECRET)),
            &mutation_headers(CSRF),
            payload.to_string().as_bytes(),
        )
        .await;
    (status, serde_json::from_slice(&body).unwrap())
}

fn public_path(share: &serde_json::Value) -> &str {
    share["url"]
        .as_str()
        .unwrap()
        .strip_prefix(SHARE_ORIGIN)
        .unwrap()
}

async fn assert_share_missing(f: &Fixture, path: &str) {
    let (status, headers, body) = f.request("GET", path, None, &[]).await;
    assert_eq!(status, 404);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(body, br#"{"code":"share_not_found"}"#);
}

#[tokio::test]
async fn schema_three_upgrade_preserves_saved_private_and_public_file_urls() {
    let legacy = LegacyLinks {
        now: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
    };
    let f = Fixture::start("syl-110-existing-links", Some(&legacy), BINARY_FILE).await;
    let owner = Some(auth(SESSION, SECRET));
    let other = Some(auth(OTHER_SESSION, OTHER_SECRET));
    let parse = |body: &[u8]| serde_json::from_slice::<serde_json::Value>(body).unwrap();
    for (actor, root) in [(owner, ROOT), (other, OTHER_ROOT)] {
        let (status, _, body) = f.request("GET", "/libraries", actor, &[]).await;
        assert_eq!(status, 200);
        let body = parse(&body);
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["id"], root);
    }
    let folder_path = format!("/folders/{LEGACY_NESTED}/path");
    let (status, _, body) = f.request("GET", &folder_path, owner, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(
        parse(&body)["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [ROOT, SHARE_PARENT, LEGACY_NESTED]
    );
    let children = format!("/folders/{LEGACY_NESTED}/children");
    let (status, _, body) = f.request("GET", &children, owner, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(parse(&body)["items"][0]["id"], SHARE_FILE);
    let saved_content = format!("/entries/{SHARE_FILE}/content");
    let (status, headers, download) = f.request("GET", &saved_content, owner, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(
        headers["etag"],
        sylux::domain::upload::revision_etag(LEGACY_REVISION)
    );
    assert_eq!(download, b"synthetic\0content\xff\x80");
    assert_eq!(f.request("GET", &saved_content, other, &[]).await.0, 404);
    assert_eq!(f.request("GET", &folder_path, other, &[]).await.0, 404);
    assert_eq!(f.request("GET", &saved_content, None, &[]).await.0, 401);
    assert_eq!(f.request("GET", &children, None, &[]).await.0, 401);
    let (status, headers, public) = f.request("GET", &LegacyLinks::path(1), None, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(public, download);
    assert_eq!(headers["cache-control"], "no-store");
    assert_share_missing(&f, &LegacyLinks::path(2)).await;
    assert_share_missing(&f, &LegacyLinks::path(3)).await;
    assert_share_missing(&f, &format!("/share/{LEGACY_NESTED}")).await;
    let (status, _, body) = f.request("GET", "/share-links", owner, &[]).await;
    assert_eq!(status, 200);
    let body = parse(&body);
    let links = body.as_array().unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["entry_id"], SHARE_FILE);
    assert_eq!(links[0]["expires_at"], legacy.now + 86400);
    let (owner, entry, revision): (String, String, String) = f
        .workers
        .as_ref()
        .unwrap()
        .read(|db| {
            db.query_row(
                "SELECT owner_account_id,entry_id,revision_id FROM share_link WHERE share_id=?1",
                [format!("{:032x}", 1)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
        })
        .await
        .unwrap();
    assert_eq!(
        (owner.as_str(), entry.as_str(), revision.as_str()),
        (OWNER, SHARE_FILE, LEGACY_REVISION)
    );
    f.close().await;
}

#[tokio::test]
async fn share_create_requires_origin_csrf_and_obeys_active_limit() {
    let f = Fixture::new("syl-82-fix-create-guards").await;
    let body = serde_json::json!({"entry_id": SHARE_FILE}).to_string();
    for headers in [
        vec![
            ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(CSRF)),
            ("Content-Type", "application/json".into()),
        ],
        vec![
            ("Origin", "https://foreign.invalid".into()),
            ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(CSRF)),
            ("Content-Type", "application/json".into()),
        ],
        vec![
            ("Origin", SHARE_ORIGIN.into()),
            ("X-CSRF-Token", URL_SAFE_NO_PAD.encode([0u8; 32])),
            ("Content-Type", "application/json".into()),
        ],
    ] {
        assert_eq!(
            f.request_with_body(
                "POST",
                "/share-links",
                Some(auth(SESSION, SECRET)),
                &headers,
                body.as_bytes()
            )
            .await
            .0,
            403
        );
    }
    for _ in 0..50 {
        assert_eq!(issue_share(&f, None).await.0, 201);
    }
    let (status, error) = issue_share(&f, None).await;
    assert_eq!(status, 409);
    assert_eq!(error["code"], "share_link_limit");
    f.close().await;
}

#[tokio::test]
async fn missing_shared_payload_is_an_integrity_error() {
    let f = Fixture::new("syl-82-fix-missing-payload").await;
    let (_, share) = issue_share(&f, None).await;
    fs::remove_file(
        f.root
            .path()
            .join("product/payloads/ab/abcdef0123456789abcdef0123456789"),
    )
    .unwrap();
    let (status, _, body) = f.request("GET", public_path(&share), None, &[]).await;
    assert_eq!(status, 500);
    assert_eq!(body, br#"{"code":"content_integrity_error"}"#);
    f.close().await;
}

#[tokio::test]
async fn file_shares_are_hash_only_owner_managed_and_download_exact_bytes_without_a_session() {
    let f = Fixture::new("syl-82-build-share-access").await;
    let (status, share) = issue_share(&f, None).await;
    assert_eq!(status, 201);
    assert_eq!(
        share["expires_at"].as_i64().unwrap() - share["created_at"].as_i64().unwrap(),
        7 * 86_400
    );
    let path = public_path(&share);
    let encoded = path.strip_prefix("/share/").unwrap();
    let raw = URL_SAFE_NO_PAD.decode(encoded).unwrap();
    assert_eq!(raw.len(), 32);
    let hash = verifier(&raw);
    let (stored, audit_count): (Vec<u8>, i64) = f
        .workers
        .as_ref()
        .unwrap()
        .read(|db| {
            Ok((
                db.query_row("SELECT verifier FROM share_link", [], |row| row.get(0))?,
                db.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(stored, hash);
    assert_eq!(audit_count, 0);

    let (status, headers, body) = f
        .request("GET", path, None, &[("Range", "bytes=0-1".into())])
        .await;
    assert_eq!(status, 200);
    assert_eq!(body, b"synthetic\0content\xff\x80");
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert!(headers["content-disposition"].starts_with("attachment;"));
    for name in ["etag", "set-cookie", "location", "content-range"] {
        assert!(!headers.contains_key(name));
    }
    assert!(
        headers
            .values()
            .all(|value| !value.contains(encoded) && !value.contains(OWNER))
    );
    assert_eq!(f.request("HEAD", path, None, &[]).await.0, 405);
    assert_eq!(f.request("POST", path, None, &[]).await.0, 405);
    assert_share_missing(&f, &format!("{path}/sibling")).await;
    assert_share_missing(&f, "/share/unknown").await;
    assert_eq!(
        f.request("GET", &format!("/entries/{SHARE_FILE}/content"), None, &[])
            .await
            .0,
        401
    );
    assert_eq!(f.request("GET", "/share-links", None, &[]).await.0, 401);

    let (status, _, owner_list) = f
        .request("GET", "/share-links", Some(auth(SESSION, SECRET)), &[])
        .await;
    assert_eq!(status, 200);
    let list: serde_json::Value = serde_json::from_slice(&owner_list).unwrap();
    assert_eq!(list[0]["id"], share["id"]);
    assert!(list[0].get("url").is_none());
    assert!(!String::from_utf8(owner_list).unwrap().contains(encoded));
    let (status, _, other_list) = f
        .request(
            "GET",
            "/share-links",
            Some(auth(OTHER_SESSION, OTHER_SECRET)),
            &[],
        )
        .await;
    assert_eq!((status, other_list.as_slice()), (200, b"[]".as_slice()));

    let revoke_path = format!("/share-links/{}", share["id"].as_str().unwrap());
    assert_eq!(
        f.request("DELETE", &revoke_path, Some(auth(SESSION, SECRET)), &[])
            .await
            .0,
        403
    );
    assert_eq!(
        f.request(
            "DELETE",
            &revoke_path,
            Some(auth(OTHER_SESSION, OTHER_SECRET)),
            &mutation_headers(OTHER_CSRF)
        )
        .await
        .0,
        204
    );
    assert_eq!(f.request("GET", path, None, &[]).await.0, 200);
    assert_eq!(
        f.request(
            "DELETE",
            &revoke_path,
            Some(auth(SESSION, SECRET)),
            &mutation_headers(CSRF)
        )
        .await
        .0,
        204
    );
    assert_share_missing(&f, path).await;

    let (_, share) = issue_share(&f, Some(1)).await;
    f.workers
        .as_ref()
        .unwrap()
        .transact(|db| {
            db.execute("UPDATE share_link SET created_at=1, expires_at=86401", [])?;
            Ok(())
        })
        .await
        .unwrap();
    assert_share_missing(&f, public_path(&share)).await;
    assert_eq!(
        f.request("GET", "/share-links", Some(auth(SESSION, SECRET)), &[])
            .await
            .2,
        b"[]"
    );
    assert_eq!(issue_share(&f, Some(0)).await.0, 400);
    for (entry, actor, csrf) in [
        (OTHER_ROOT, auth(SESSION, SECRET), CSRF),
        (SHARE_FILE, auth(OTHER_SESSION, OTHER_SECRET), OTHER_CSRF),
    ] {
        let body = serde_json::json!({"entry_id":entry,"expiry_days":30}).to_string();
        assert_eq!(
            f.request_with_body(
                "POST",
                "/share-links",
                Some(actor),
                &mutation_headers(csrf),
                body.as_bytes()
            )
            .await
            .0,
            404
        );
    }
    f.close().await;
}

#[tokio::test]
async fn file_shares_do_not_reactivate_after_restore_replacement_or_account_reenable() {
    let f = Fixture::new("syl-82-build-share-lifecycle").await;
    let workers = f.workers.as_ref().unwrap();
    workers.transact(|db| {
        db.execute(
            "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
             VALUES (?1,?2,?3,'folder','private folder',x'7072697661746520666f6c646572',1)",
            params![SHARE_PARENT, OWNER, ROOT],
        )?;
        db.execute("UPDATE entry SET parent_id=?1 WHERE entry_id=?2", params![SHARE_PARENT, SHARE_FILE])?;
        db.execute("UPDATE account SET role='administrator' WHERE account_id=?1", [OTHER])?;
        Ok(())
    }).await.unwrap();
    let (_, trashed) = issue_share(&f, Some(30)).await;
    assert_eq!(
        trashed["expires_at"].as_i64().unwrap() - trashed["created_at"].as_i64().unwrap(),
        30 * 86_400
    );
    workers
        .transact(|db| {
            sylux::domain::namespace::trash(
                db,
                OWNER,
                SHARE_PARENT,
                &format!("\"{SHARE_PARENT}:1\""),
            )
            .map_err(|_| rusqlite::Error::InvalidQuery)
        })
        .await
        .unwrap();
    assert_share_missing(&f, public_path(&trashed)).await;
    assert_eq!(issue_share(&f, Some(1)).await.0, 404);
    workers
        .transact(|db| {
            sylux::domain::namespace::restore(
                db,
                OWNER,
                SHARE_PARENT,
                &format!("\"{SHARE_PARENT}:2\""),
            )
            .map(|_| ())
            .map_err(|_| rusqlite::Error::InvalidQuery)
        })
        .await
        .unwrap();
    assert_share_missing(&f, public_path(&trashed)).await;

    let (_, replaced) = issue_share(&f, Some(1)).await;
    workers.transact(|db| {
        let next = "00000000000000000000000000000304";
        db.execute(
            "INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id)
             SELECT ?1,r.entry_id,r.blob_id,r.length,r.sha256,r.committed_at+1,r.actor_account_id
             FROM entry e JOIN revision r ON e.current_revision_id=r.revision_id WHERE e.entry_id=?2",
            params![next, SHARE_FILE],
        )?;
        db.execute("UPDATE entry SET current_revision_id=?1 WHERE entry_id=?2", params![next, SHARE_FILE])?;
        Ok(())
    }).await.unwrap();
    assert_share_missing(&f, public_path(&replaced)).await;
    let (_, existing) = issue_share(&f, None).await;
    let disable = format!("/accounts/{OWNER}/disable");
    let admin = Some(auth(OTHER_SESSION, OTHER_SECRET));
    let headers = mutation_headers(OTHER_CSRF);
    let ((create_status, racing), (disable_status, _, _)) = tokio::join!(
        issue_share(&f, Some(1)),
        f.request("POST", &disable, admin, &headers),
    );
    assert_eq!(disable_status, 204);
    assert!(matches!(create_status, 201 | 401));
    if create_status == 201 {
        assert_share_missing(&f, public_path(&racing)).await;
    }
    assert_share_missing(&f, public_path(&existing)).await;
    assert_eq!(issue_share(&f, None).await.0, 401);
    assert_eq!(
        f.request(
            "POST",
            &format!("/accounts/{OWNER}/enable"),
            admin,
            &headers
        )
        .await
        .0,
        204
    );
    assert_share_missing(&f, public_path(&existing)).await;
    f.close().await;
}

#[tokio::test]
async fn file_share_admission_bounds_public_reads_before_sqlite() {
    let f = Fixture::new("syl-82-build-download-slots").await;
    let (status, share) = issue_share(&f, None).await;
    assert_eq!(status, 201);
    let path = public_path(&share);
    let (arrived, at_reader) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let workers = f.workers.as_ref().unwrap().clone();
    let blocking = tokio::spawn(async move {
        workers
            .read(move |_| {
                arrived.send(()).unwrap();
                held.recv_timeout(Duration::from_secs(10)).unwrap();
                Ok(())
            })
            .await
            .unwrap();
    });
    at_reader.await.unwrap();

    use futures_util::{StreamExt, stream::FuturesUnordered};
    let mut requests = (0..9)
        .map(|_| f.request("GET", path, None, &[]))
        .collect::<FuturesUnordered<_>>();
    let first = tokio::time::timeout(Duration::from_secs(5), requests.next()).await;
    release.send(()).unwrap();
    blocking.await.unwrap();
    let (status, headers, _) = first
        .expect("a saturated request completes before SQLite")
        .unwrap();
    assert_eq!(status, 429);
    assert_eq!(headers["retry-after"], "1");
    for (status, _, bytes) in requests.collect::<Vec<_>>().await {
        assert_eq!(status, 200);
        assert_eq!(bytes, b"synthetic\0content\xff\x80");
    }
    assert_eq!(f.request("GET", path, None, &[]).await.0, 200);
    f.close().await;
}

#[tokio::test]
async fn publication_lifecycle_and_independent_direct_grants() {
    use serde_json::{Value, json};

    let f = Fixture::new("library-publication").await;
    f.seed_grant_admin().await;
    let (status, library) = f
        .share_request(
            "POST",
            "/libraries",
            GRANT_OWNER_ACTOR,
            json!({"name": "Published library"}),
            None,
            None,
        )
        .await;
    assert_eq!(status, 201);
    let root = library["id"].as_str().unwrap();
    let folder = f.share_folder(root, "Published folder").await;
    let private = f.share_folder(ROOT, "Private sibling").await;
    let move_url = format!("/entries/{SHARE_FILE}/move");
    let (status, _) = f
        .share_request(
            "POST",
            &move_url,
            GRANT_OWNER_ACTOR,
            json!({"parent_id": folder}),
            Some(&sylux::domain::upload::folder_etag(SHARE_FILE, 1)),
            None,
        )
        .await;
    assert_eq!(status, 200);
    let publish_url = format!("/libraries/{root}/publication");
    let root_path = format!("/shared/folders/{root}/path");
    let folder_path = format!("/shared/folders/{folder}/path");
    let private_path = format!("/shared/folders/{private}/path");
    let bookmark = format!("/shared/entries/{SHARE_FILE}");
    let content = format!("{bookmark}/content");
    let publish = async || {
        let (status, value) = f
            .share_request(
                "POST",
                &publish_url,
                GRANT_OWNER_ACTOR,
                Value::Null,
                None,
                None,
            )
            .await;
        assert_eq!(status, 200);
        value["id"].as_str().unwrap().to_owned()
    };
    let withdraw = async |id: &str| {
        f.share_request(
            "DELETE",
            &format!("/library-publications/{id}"),
            GRANT_OWNER_ACTOR,
            Value::Null,
            None,
            None,
        )
        .await
        .0
    };
    for actor in [GRANT_RECIPIENT_ACTOR, GRANT_ADMIN_ACTOR] {
        assert_eq!(
            f.share_request("POST", &publish_url, actor, Value::Null, None, None)
                .await
                .0,
            404
        );
    }
    assert_eq!(
        f.share_request(
            "POST",
            &format!("/libraries/{folder}/publication"),
            GRANT_OWNER_ACTOR,
            Value::Null,
            None,
            None
        )
        .await
        .0,
        404
    );

    let visible = async |actor, ids: &[&str]| {
        let (status, result) = f.share_get("/published", actor).await;
        assert_eq!(status, 200);
        assert_eq!(result["items"].as_array().unwrap().len(), ids.len());
        for id in ids {
            assert!(
                result["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["entry"]["id"] == *id)
            );
        }
    };
    let first = publish().await;
    assert_eq!(publish().await, first);
    for actor in [GRANT_RECIPIENT_ACTOR, GRANT_ADMIN_ACTOR] {
        visible(actor, &[root]).await;
        assert_eq!(
            f.share_get(&folder_path, actor).await.1["items"][0]["id"],
            root
        );
        assert_eq!(f.share_get(&private_path, actor).await.0, 404);
        assert_eq!(f.share_get(&bookmark, actor).await.0, 200);
        assert_eq!(
            f.share_get("/published/search?q=unsafe", actor).await.1["items"][0]["entry"]["id"],
            SHARE_FILE
        );
        assert_eq!(
            f.share_get("/published/search?q=Private", actor).await.1["items"],
            json!([])
        );
        assert_eq!(
            f.share_request(
                "DELETE",
                &format!("/library-publications/{first}"),
                actor,
                Value::Null,
                None,
                None
            )
            .await
            .0,
            404
        );
    }
    for path in [
        "/published",
        "/published/search?q=unsafe",
        &folder_path,
        &bookmark,
        &content,
    ] {
        assert_eq!(f.request("GET", path, None, &[]).await.0, 401);
    }
    let auth = Some(auth(OTHER_SESSION, OTHER_SECRET));
    let (status, headers, bytes) = f.request("GET", &content, auth, &[]).await;
    assert_eq!(
        (status, bytes.as_slice()),
        (200, b"synthetic\0content\xff\x80".as_slice())
    );
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(
        f.request("GET", &content, auth, &[("Range", "bytes=0-2".into())])
            .await
            .0,
        206
    );

    let new_account =
        json!({"handle":"new-member","password":"disposable passphrase for new member"});
    assert_eq!(
        f.share_request(
            "POST",
            "/accounts",
            GRANT_ADMIN_ACTOR,
            new_account.clone(),
            None,
            None
        )
        .await
        .0,
        204
    );
    let (status, anonymous, body) = f.request("GET", "/auth/csrf", None, &[]).await;
    assert_eq!(status, 200);
    let csrf: Value = serde_json::from_slice(&body).unwrap();
    let (status, signed_in, _) = f
        .request_with_body(
            "POST",
            "/auth/local",
            None,
            &[
                ("Origin", SHARE_ORIGIN.to_owned()),
                (
                    "Cookie",
                    anonymous["set-cookie"]
                        .split(';')
                        .next()
                        .unwrap()
                        .to_owned(),
                ),
                (
                    "X-CSRF-Token",
                    csrf["csrf_token"].as_str().unwrap().to_owned(),
                ),
                ("Content-Type", "application/json".into()),
            ],
            new_account.to_string().as_bytes(),
        )
        .await;
    assert_eq!(status, 200);
    let late_cookie = (
        "Cookie",
        signed_in["set-cookie"]
            .split(';')
            .next()
            .unwrap()
            .to_owned(),
    );
    for path in [
        "/published",
        "/published/search?q=unsafe",
        &root_path,
        &content,
    ] {
        assert_eq!(
            f.request("GET", path, None, std::slice::from_ref(&late_cookie))
                .await
                .0,
            200
        );
    }

    let file_tag = f.share_get(&bookmark, GRANT_RECIPIENT_ACTOR).await.1["entry"]["etag"]
        .as_str()
        .unwrap()
        .to_owned();
    for (method, path, body) in [
        (
            "PATCH",
            format!("/entries/{SHARE_FILE}"),
            json!({"name":"changed"}),
        ),
        ("DELETE", format!("/entries/{SHARE_FILE}"), Value::Null),
        ("POST", move_url, json!({"parent_id":OTHER_ROOT})),
        (
            "POST",
            format!("/folders/{folder}/children"),
            json!({"name":"Foreign upload"}),
        ),
        (
            "POST",
            format!("/entries/{folder}/direct-grants"),
            json!({"handle":"unrelated-admin"}),
        ),
        ("POST", "/archives".into(), json!({"entry_ids":[root]})),
    ] {
        assert_eq!(
            f.share_request(
                method,
                &path,
                GRANT_RECIPIENT_ACTOR,
                body,
                Some(&file_tag),
                None
            )
            .await
            .0,
            404
        );
    }
    let (status, copy) = f
        .share_request(
            "POST",
            &format!("/entries/{SHARE_FILE}/copy-ticket"),
            GRANT_RECIPIENT_ACTOR,
            json!({"parent_id":OTHER_ROOT}),
            Some(&file_tag),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        f.share_request("POST", "/copies", GRANT_RECIPIENT_ACTOR, copy, None, None)
            .await
            .0,
        404
    );
    let (status, zip) = f
        .share_request(
            "POST",
            "/archives",
            GRANT_OWNER_ACTOR,
            json!({"entry_ids":[&folder]}),
            None,
            None,
        )
        .await;
    assert_eq!(status, 201);
    let zip = zip["url"].as_str().unwrap();
    assert_eq!(f.request("GET", zip, auth, &[]).await.0, 404);
    assert_eq!(
        f.request("GET", zip, Some((SESSION, SECRET)), &[]).await.0,
        200
    );

    f.change_grant_authority(OTHER, "disable", GRANT_RECIPIENT_ACTOR)
        .await;
    assert_eq!(
        f.share_get("/published", GRANT_RECIPIENT_ACTOR).await.0,
        401
    );
    assert_eq!(f.request("GET", &content, auth, &[]).await.0, 401);
    visible(GRANT_ADMIN_ACTOR, &[root]).await;
    f.change_grant_authority(OTHER, "enable", GRANT_RECIPIENT_ACTOR)
        .await;
    visible(GRANT_RECIPIENT_ACTOR, &[root]).await;
    assert_eq!(f.request("GET", &content, auth, &[]).await.0, 200);

    let (status, link) = f
        .share_request(
            "POST",
            "/share-links",
            GRANT_OWNER_ACTOR,
            json!({"entry_id":SHARE_FILE}),
            None,
            None,
        )
        .await;
    assert_eq!(status, 201);
    let anonymous_link = link["url"]
        .as_str()
        .unwrap()
        .strip_prefix(SHARE_ORIGIN)
        .unwrap();
    assert_eq!(
        f.request("GET", anonymous_link, None, &[]).await.2,
        BINARY_FILE.1
    );
    let direct = f.share_grant(&folder, "other").await;
    assert_eq!(
        f.share_get("/shared/search?q=unsafe", GRANT_RECIPIENT_ACTOR)
            .await
            .1["items"][0]["location"][0]["id"],
        folder
    );
    assert_eq!(withdraw(&first).await, 204);
    assert_eq!(
        f.request("GET", anonymous_link, None, &[]).await.2,
        BINARY_FILE.1
    );
    visible(GRANT_RECIPIENT_ACTOR, &[]).await;
    assert_eq!(
        f.share_get("/shared", GRANT_RECIPIENT_ACTOR).await.1["items"][0]["entry"]["id"],
        folder
    );
    assert_eq!(
        f.share_get("/shared/search?q=unsafe", GRANT_RECIPIENT_ACTOR)
            .await
            .1["items"][0]["location"][0]["id"],
        folder
    );
    assert_eq!(f.share_get(&root_path, GRANT_RECIPIENT_ACTOR).await.0, 404);
    assert_eq!(f.share_get(&bookmark, GRANT_RECIPIENT_ACTOR).await.0, 200);
    assert_eq!(f.share_get(&bookmark, GRANT_ADMIN_ACTOR).await.0, 404);
    f.share_revoke(&direct).await;
    assert_eq!(
        f.request(
            "GET",
            &content,
            auth,
            &[("If-None-Match", headers["etag"].clone())]
        )
        .await
        .0,
        404
    );

    let second = publish().await;
    let direct = f.share_grant(&folder, "other").await;
    f.share_revoke(&direct).await;
    assert_eq!(
        f.share_get("/shared", GRANT_RECIPIENT_ACTOR).await.1["items"],
        json!([])
    );
    assert_eq!(
        f.share_get("/shared/search?q=unsafe", GRANT_RECIPIENT_ACTOR)
            .await
            .1["items"],
        json!([])
    );
    assert_eq!(withdraw(&first).await, 404);
    visible(GRANT_RECIPIENT_ACTOR, &[root]).await;
    assert_eq!(f.share_get(&bookmark, GRANT_RECIPIENT_ACTOR).await.0, 200);
    assert_eq!(
        f.share_get("/published/search?q=unsafe", GRANT_RECIPIENT_ACTOR)
            .await
            .1["items"][0]["location"][0]["id"],
        root
    );
    assert_eq!(withdraw(&second).await, 204);
    visible(GRANT_RECIPIENT_ACTOR, &[]).await;
    assert_eq!(
        f.share_get("/published/search?q=unsafe", GRANT_RECIPIENT_ACTOR)
            .await
            .1["items"],
        json!([])
    );
    assert_eq!(f.share_get(&bookmark, GRANT_RECIPIENT_ACTOR).await.0, 404);

    let (publication, ()) = tokio::join!(
        f.share_request(
            "POST",
            &publish_url,
            GRANT_OWNER_ACTOR,
            Value::Null,
            None,
            None
        ),
        f.change_grant_authority(OWNER, "disable", GRANT_OWNER_ACTOR),
    );
    assert!(matches!(publication.0, 200 | 401));
    visible(GRANT_RECIPIENT_ACTOR, &[]).await;
    assert_eq!(f.request("GET", &content, auth, &[]).await.0, 404);
    assert_eq!(
        f.share_request(
            "POST",
            &publish_url,
            GRANT_OWNER_ACTOR,
            Value::Null,
            None,
            None
        )
        .await
        .0,
        401
    );
    f.change_grant_authority(OWNER, "enable", GRANT_OWNER_ACTOR)
        .await;
    visible(GRANT_RECIPIENT_ACTOR, &[]).await;
    assert_eq!(
        f.share_get("/published/search?q=unsafe", GRANT_RECIPIENT_ACTOR)
            .await
            .1["items"],
        json!([])
    );
    assert_eq!(f.share_get(&bookmark, GRANT_RECIPIENT_ACTOR).await.0, 404);
    let third = publish().await;
    assert_ne!(third, second);
    f.change_grant_authority(OWNER, "disable", GRANT_OWNER_ACTOR)
        .await;
    visible(GRANT_RECIPIENT_ACTOR, &[]).await;
    f.change_grant_authority(OWNER, "enable", GRANT_OWNER_ACTOR)
        .await;
    assert_eq!(withdraw(&third).await, 404);
    assert_eq!(f.request("GET", &content, auth, &[]).await.0, 404);
    f.close().await;
}

#[tokio::test]
async fn publication_live_tree_moves_and_restores_recheck_consent() {
    use serde_json::{Value, json};

    let f = Fixture::new("publication-tree").await;
    let (status, library) = f
        .share_request(
            "POST",
            "/libraries",
            GRANT_OWNER_ACTOR,
            json!({"name":"Published tree"}),
            None,
            None,
        )
        .await;
    assert_eq!(status, 201);
    let root = library["id"].as_str().unwrap();
    let destination = f.share_folder(root, "Published destination").await;
    let retained = f.share_folder(&destination, "Still shared").await;
    let folder = f.share_folder(ROOT, "Nested project").await;
    let publish = async || {
        let (status, reply) = f
            .share_request(
                "POST",
                &format!("/libraries/{root}/publication"),
                GRANT_OWNER_ACTOR,
                Value::Null,
                None,
                None,
            )
            .await;
        assert_eq!(status, 200);
        reply["id"].as_str().unwrap().to_owned()
    };
    let mut publication = publish().await;
    let bookmark = format!("/shared/entries/{SHARE_FILE}/content");
    let listing = format!("/shared/folders/{destination}/children");
    let scoped_search = format!("/published/search?folder_id={destination}&q=unsafe");
    let recipient = Some((OTHER_SESSION, OTHER_SECRET));
    let check_reachability = async |id, visible| {
        let (status, entries) = f.share_get(&listing, GRANT_RECIPIENT_ACTOR).await;
        assert_eq!(status, 200);
        let items = entries["items"].as_array().unwrap();
        assert!(items.iter().any(|item| item["id"] == retained));
        assert_eq!(items.iter().any(|item| item["id"] == id), visible);
        let (status, results) = f.share_get(&scoped_search, GRANT_RECIPIENT_ACTOR).await;
        assert_eq!(status, 200);
        assert_eq!(
            results["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|hit| hit["entry"]["id"] == SHARE_FILE),
            visible
        );
        let (status, _, bytes) = f.request("GET", &bookmark, recipient, &[]).await;
        if visible {
            assert_eq!(status, 200);
            assert_eq!(bytes, BINARY_FILE.1);
        } else {
            assert_eq!(status, 404);
        }
    };
    let move_to = async |id: &str, parent: &str, tag: &str, token: Option<&str>| {
        f.share_request(
            "POST",
            &format!("/entries/{id}/move"),
            GRANT_OWNER_ACTOR,
            json!({"parent_id":parent}),
            Some(tag),
            token,
        )
        .await
    };

    let carried = f.share_folder(&destination, "Carried grant").await;
    f.share_grant(&carried, "other").await;
    let (_, path) = f
        .share_get(&format!("/folders/{carried}/path"), GRANT_OWNER_ACTOR)
        .await;
    let tag = path["items"].as_array().unwrap().last().unwrap()["etag"]
        .as_str()
        .unwrap();
    let (status, confirmation) = move_to(&carried, ROOT, tag, None).await;
    assert_eq!(status, 409);
    assert_eq!(confirmation["confirmation"]["removes_access_for_all"], true);
    assert_eq!(
        confirmation["confirmation"]["carried_recipients"],
        json!(["other"])
    );
    assert_eq!(
        move_to(
            &carried,
            ROOT,
            tag,
            confirmation["confirmation"]["token"].as_str()
        )
        .await
        .0,
        200
    );

    let mut tag = sylux::domain::upload::folder_etag(SHARE_FILE, 1);
    for id in [SHARE_FILE, folder.as_str()] {
        let (status, preflight) = move_to(id, &destination, &tag, None).await;
        assert_eq!(status, 409);
        assert_eq!(preflight["confirmation"]["exposes_to_all"], true);
        check_reachability(id, false).await;
        assert_eq!(
            f.share_request(
                "DELETE",
                &format!("/library-publications/{publication}"),
                GRANT_OWNER_ACTOR,
                Value::Null,
                None,
                None
            )
            .await
            .0,
            204
        );
        assert_eq!(
            move_to(
                id,
                &destination,
                &tag,
                preflight["confirmation"]["token"].as_str()
            )
            .await
            .1["code"],
            "sharing_audience_changed"
        );
        publication = publish().await;
        let (status, renewed) = move_to(
            id,
            &destination,
            &tag,
            preflight["confirmation"]["token"].as_str(),
        )
        .await;
        assert_eq!(status, 409);
        assert_ne!(
            preflight["confirmation"]["token"],
            renewed["confirmation"]["token"]
        );
        assert_eq!(renewed["confirmation"]["exposes_to_all"], true);
        let (status, moved) = move_to(
            id,
            &destination,
            &tag,
            renewed["confirmation"]["token"].as_str(),
        )
        .await;
        assert_eq!(status, 200);
        check_reachability(id, true).await;

        let (status, moved_out) = move_to(id, ROOT, moved["etag"].as_str().unwrap(), None).await;
        assert_eq!(status, 200);
        check_reachability(id, false).await;
        let (status, preflight) =
            move_to(id, &destination, moved_out["etag"].as_str().unwrap(), None).await;
        assert_eq!(status, 409);
        let (status, returned) = move_to(
            id,
            &destination,
            moved_out["etag"].as_str().unwrap(),
            preflight["confirmation"]["token"].as_str(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            f.share_request(
                "DELETE",
                &format!("/entries/{id}"),
                GRANT_OWNER_ACTOR,
                Value::Null,
                returned["etag"].as_str(),
                None
            )
            .await
            .0,
            204
        );
        check_reachability(id, false).await;
        let (_, trash) = f.share_get("/trash", GRANT_OWNER_ACTOR).await;
        let deleted = trash["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap();
        let restore = async |token: Option<&str>| {
            f.share_request(
                "POST",
                &format!("/trash/{id}/restore"),
                GRANT_OWNER_ACTOR,
                Value::Null,
                deleted["etag"].as_str(),
                token,
            )
            .await
        };
        let (status, preflight) = restore(None).await;
        assert_eq!(status, 409);
        assert_eq!(preflight["confirmation"]["exposes_to_all"], true);
        check_reachability(id, false).await;
        assert_eq!(
            restore(preflight["confirmation"]["token"].as_str()).await.0,
            204
        );
        check_reachability(id, true).await;

        if id == SHARE_FILE {
            let (status, metadata) = f
                .share_get(&format!("/shared/entries/{id}"), GRANT_RECIPIENT_ACTOR)
                .await;
            assert_eq!(status, 200);
            assert_eq!(
                move_to(
                    id,
                    &folder,
                    metadata["entry"]["etag"].as_str().unwrap(),
                    None
                )
                .await
                .0,
                200
            );
            let (_, path) = f
                .share_get(&format!("/folders/{folder}/path"), GRANT_OWNER_ACTOR)
                .await;
            tag = path["items"][1]["etag"].as_str().unwrap().to_owned();
        }
    }

    // Existing library roots cannot be moved or trashed through Files.
    let (_, root_path) = f
        .share_get(&format!("/folders/{root}/path"), GRANT_OWNER_ACTOR)
        .await;
    let root_tag = root_path["items"][0]["etag"].as_str().unwrap();
    assert_eq!(move_to(root, ROOT, root_tag, None).await.0, 404);
    assert_eq!(
        f.share_request(
            "DELETE",
            &format!("/entries/{root}"),
            GRANT_OWNER_ACTOR,
            Value::Null,
            Some(root_tag),
            None
        )
        .await
        .0,
        404
    );
    check_reachability(folder.as_str(), true).await;
    f.close().await;
}

#[path = "content_folder_links.rs"]
mod folder_links;
