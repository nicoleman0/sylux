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
const ROOT: &str = "00000000000000000000000000000101";
const OTHER_ROOT: &str = "00000000000000000000000000000102";
const SESSION: &str = "00000000000000000000000000000201";
const OTHER_SESSION: &str = "00000000000000000000000000000202";
const SECRET: &[u8; 32] = b"s1-owner-secret-0123456789abcdef";
const OTHER_SECRET: &[u8; 32] = b"s1-other-secret-0123456789abcdef";
const CSRF: &[u8; 32] = b"s1-owner-csrf-0123456789abcdef!!";
const OTHER_CSRF: &[u8; 32] = b"s1-other-csrf-0123456789abcdef!!";
const BUSY: Duration = Duration::from_millis(375);

struct Fixture {
    root: FixtureRoot,
    server: Option<TlsApiServer>,
    workers: Option<SqliteWorkers>,
    ca: Vec<u8>,
}
impl Fixture {
    async fn new(case: &str) -> Self {
        let parent = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("sylux-content-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let root = FixtureRoot::create(&parent, "content", case).unwrap();
        let product = root.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).unwrap();
        let tls = TlsFiles::generate(&root, "tls").unwrap();
        let ca = fs::read(&tls.ca_der).unwrap();
        let db_path = product.join("metadata/sylux.sqlite3");
        let connection = sqlite::open_writer(&db_path, BUSY).unwrap();
        seed(&connection);
        seed_file(&connection, &product);
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
fn seed_file(c: &Connection, product: &std::path::Path) {
    let bytes = b"synthetic\0content\xff\x80";
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
    tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,current_revision_id,generation) VALUES('00000000000000000000000000000301',?1,?2,'file','unsafe <name>.bin',x'756e73616665203c6e616d653e2e62696e',1,NULL,1)", params![OWNER, ROOT]).unwrap();
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

fn mutation_headers(csrf: &[u8; 32]) -> [(&'static str, String); 3] {
    [
        ("Origin", SHARE_ORIGIN.to_owned()),
        ("X-CSRF-Token", URL_SAFE_NO_PAD.encode(csrf)),
        ("Content-Type", "application/json".to_owned()),
    ]
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
        (ROOT, auth(SESSION, SECRET), CSRF),
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
