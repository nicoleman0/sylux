#[path = "../common/mod.rs"]
mod common;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{
    FixtureRoot, TestBarrier,
    server::{TlsApiServer, TlsFiles, tls_connect},
};
use rusqlite::{Connection, params};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use sylux::domain::{
    names::{EntryName, MAX_NAME_BYTES, NameError},
    namespace::{self, NamespaceError},
};

const OWNER: &str = "00000000000000000000000000000001";
const ROOT: &str = "00000000000000000000000000000002";
const SESSION: &str = "00000000000000000000000000000201";
const OTHER: &str = "00000000000000000000000000000003";
const OTHER_ROOT: &str = "00000000000000000000000000000004";
const OTHER_SESSION: &str = "00000000000000000000000000000202";
const OTHER_SECRET: &[u8; 32] = b"s1-other-secret-0123456789abcdef";
const OTHER_CSRF: &[u8; 32] = b"s1-other-csrf-0123456789abcdef!!";
const SECRET: &[u8; 32] = b"s1-owner-secret-0123456789abcdef";
const CSRF: &[u8; 32] = b"s1-owner-csrf-0123456789abcdef!!";
const BUSY: Duration = Duration::from_millis(375);

struct HttpFixture {
    owner: FixtureRoot,
    parent: PathBuf,
    workers: Option<sylux::persistence::sqlite::SqliteWorkers>,
    server: TlsApiServer,
    ca: Vec<u8>,
}
impl HttpFixture {
    async fn new(label: &str, count: usize) -> Self {
        use sylux::{
            domain::authz::verifier,
            http::{self, AuthState},
            persistence::{
                root,
                sqlite::{self, SqliteWorkers, WorkerConfig},
            },
        };
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-names-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "names", label).unwrap();
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
        db.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'names-owner','user',1,0)", [OWNER]).unwrap();
        db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)", params![ROOT, OWNER]).unwrap();
        for n in 0..count {
            let id = format!("{:032x}", n + 0x1000);
            let name = format!("item-{n:04}");
            db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,?3,'folder',?4,?5,1,1)", params![id, OWNER, ROOT, name, name.as_bytes()]).unwrap();
        }
        db.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?4,?5,?6,?7,0)", params![SESSION, OWNER, verifier(SECRET).as_slice(), verifier(CSRF).as_slice(), now, now+3600, now+7200]).unwrap();
        db.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'names-other','user',1,0)", [OTHER]).unwrap();
        db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)", params![OTHER_ROOT, OTHER]).unwrap();
        db.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,0,?3,?4,?5,?6,?7,0)", params![OTHER_SESSION, OTHER, verifier(OTHER_SECRET).as_slice(), verifier(OTHER_CSRF).as_slice(), now, now+3600, now+7200]).unwrap();
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
    async fn request_bytes(&self, path: &str, extra: &str, body: &[u8]) -> (u16, Vec<u8>) {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            time::timeout,
        };
        let mut stream = tls_connect(self.server.address(), &self.ca, "127.0.0.1")
            .await
            .unwrap();
        let cookie = format!(
            "__Host-sylux={}.{}",
            SESSION,
            URL_SAFE_NO_PAD.encode(SECRET)
        );
        let mut request = format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nCookie: {cookie}\r\nAccept: application/json\r\nOrigin: https://127.0.0.1\r\nX-CSRF-Token: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{extra}Content-Length: {}\r\n\r\n", URL_SAFE_NO_PAD.encode(CSRF), body.len()).into_bytes();
        request.extend_from_slice(body);
        stream.write_all(&request).await.unwrap();
        let mut response = Vec::new();
        timeout(Duration::from_secs(3), stream.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let status = std::str::from_utf8(&response[..split])
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (status, response[split + 4..].to_vec())
    }
    async fn list(&self, query: &str) -> (u16, Vec<u8>) {
        self.list_as(query, SESSION, SECRET).await
    }
    async fn list_as(&self, query: &str, sid: &str, secret: &[u8; 32]) -> (u16, Vec<u8>) {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            time::timeout,
        };
        let mut stream = tls_connect(self.server.address(), &self.ca, "127.0.0.1")
            .await
            .unwrap();
        let cookie = format!("__Host-sylux={}.{}", sid, URL_SAFE_NO_PAD.encode(secret));
        let req = format!(
            "GET /folders/children{query} HTTP/1.1\r\nHost: 127.0.0.1\r\nCookie: {cookie}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        timeout(Duration::from_secs(3), stream.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let status = std::str::from_utf8(&response[..split])
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (status, response[split + 4..].to_vec())
    }
    async fn close(mut self) {
        self.server.shutdown_and_join().await.unwrap();
        self.workers.take().unwrap().shutdown().await.unwrap();
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.parent).unwrap();
    }
}

fn namespace_db() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(include_str!("../../migrations/sqlite/0001_s1.sql"))
        .unwrap();
    db.execute(
        "INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'fixture-owner','user',1,0)",
        [OWNER],
    ).unwrap();
    db.execute(
        "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)",
        params![ROOT, OWNER],
    ).unwrap();
    db
}

#[test]
fn original_and_normalized_utf8_byte_boundaries_are_enforced_without_trim() {
    let original_255 = "é".repeat(127) + "a";
    assert_eq!(original_255.len(), MAX_NAME_BYTES);
    let accepted = EntryName::parse(&original_255).unwrap();
    assert_eq!(accepted.display, original_255);
    assert_eq!(
        EntryName::parse(&(original_255.clone() + "a")).unwrap_err(),
        NameError::TooLong
    );

    // U+0344 is two UTF-8 bytes but NFC expands it to U+0308 U+0301 (four bytes).
    let normalized_overflow = "\u{0344}".repeat(85);
    assert_eq!(normalized_overflow.len(), 170);
    assert_eq!(
        EntryName::parse(&normalized_overflow).unwrap_err(),
        NameError::TooLong
    );
    // 63 NFC-expanding U+0344 scalars normalize to 252 bytes; three ASCII
    // bytes bring the key to exactly 255 while submitted spelling is 129.
    let normalized_255 = "\u{0344}".repeat(63) + "abc";
    let parsed = EntryName::parse(&normalized_255).unwrap();
    assert_eq!(normalized_255.len(), 129);
    assert_eq!(parsed.key.len(), MAX_NAME_BYTES);
    assert_eq!(EntryName::parse(" name ").unwrap().display, " name ");
}

#[test]
fn canonical_sibling_keys_conflict_but_case_variants_coexist() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    let composed = namespace::create_folder(&tx, OWNER, ROOT, "Å").unwrap();
    let lowercase = namespace::create_folder(&tx, OWNER, ROOT, "å").unwrap();
    assert_eq!(composed.name, "Å");
    assert_eq!(lowercase.name, "å");
    assert!(matches!(
        namespace::create_folder(&tx, OWNER, ROOT, "A\u{30a}").unwrap_err(),
        NamespaceError::Conflict
    ));
    tx.commit().unwrap();
    let listed = namespace::list(&db, OWNER, ROOT, None, 10).unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Å", "å"]
    );
}

#[test]
fn invalid_names_are_rejected_without_normalizing_or_trimming_display() {
    for name in ["", ".", "..", "a/b", "a\\b", "a\0b", "a\u{7f}b", "a\nb"] {
        assert!(
            EntryName::parse(name).is_err(),
            "accepted forbidden name {name:?}"
        );
    }
    let spaces = EntryName::parse("  padded  ").unwrap();
    assert_eq!(spaces.display, "  padded  ");
}

#[test]
fn depth_128_is_valid_and_129th_child_is_rejected() {
    let db = namespace_db();
    let mut parent = ROOT.to_owned();
    for depth in 1..=128 {
        let tx = db.unchecked_transaction().unwrap();
        let entry = namespace::create_folder(&tx, OWNER, &parent, &format!("d{depth}")).unwrap();
        tx.commit().unwrap();
        parent = entry.id;
    }
    let tx = db.unchecked_transaction().unwrap();
    let error = namespace::create_folder(&tx, OWNER, &parent, "too-deep").unwrap_err();
    assert!(matches!(error, NamespaceError::InvalidName));
}

#[test]
fn keyset_order_is_exact_and_limit_reads_only_one_extra_row() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    for (id, name) in [
        ("00000000000000000000000000001004", "delta"),
        ("00000000000000000000000000001001", "alpha"),
        ("00000000000000000000000000001002", "bravo"),
        ("00000000000000000000000000001003", "charlie"),
    ] {
        tx.execute(
            "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,?3,'folder',?4,?5,1,1)",
            params![id, OWNER, ROOT, name, name.as_bytes()],
        ).unwrap();
    }
    tx.commit().unwrap();
    let first = namespace::list(&db, OWNER, ROOT, None, 2).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|entry| (entry.name.as_str(), entry.id.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("alpha", "00000000000000000000000000001001"),
            ("bravo", "00000000000000000000000000001002"),
            ("charlie", "00000000000000000000000000001003")
        ]
    );
    let next = namespace::list(
        &db,
        OWNER,
        ROOT,
        Some((b"bravo", "00000000000000000000000000001002")),
        2,
    )
    .unwrap();
    assert_eq!(
        next.iter()
            .map(|entry| (entry.name.as_str(), entry.id.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("charlie", "00000000000000000000000000001003"),
            ("delta", "00000000000000000000000000001004")
        ]
    );
}

#[test]
fn listings_above_500_are_bounded_and_ordered() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    for n in (0..503).rev() {
        let id = format!("{:032x}", n + 1000);
        let name = format!("name-{n:04}");
        tx.execute(
            "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,?3,'folder',?4,?5,1,1)",
            params![id, OWNER, ROOT, name, name.as_bytes()],
        ).unwrap();
    }
    tx.commit().unwrap();
    let first = namespace::list(&db, OWNER, ROOT, None, 500).unwrap();
    assert_eq!(
        first.len(),
        501,
        "listing fetches at most the requested 500 plus one sentinel"
    );
    assert_eq!(first[0].name, "name-0000");
    assert_eq!(first[499].name, "name-0499");
    assert_eq!(first[500].name, "name-0500");
}

#[test]
fn deep_folder_boundary_and_child_listing_have_no_implicit_full_sort() {
    let db = namespace_db();
    let plan: String = db.query_row(
        "EXPLAIN QUERY PLAN SELECT entry_id FROM entry WHERE owner_account_id=?1 AND parent_id=?2 AND state='active' AND (?3 IS NULL OR name_key>?3 OR (name_key=?3 AND entry_id>?4)) ORDER BY name_key,entry_id LIMIT ?5",
        params![OWNER, ROOT, Option::<Vec<u8>>::None, "", 501],
        |row| row.get(3),
    ).unwrap();
    assert!(
        plan.contains("entry_active_listing"),
        "expected indexed sibling keyset scan, got {plan}"
    );
}

#[tokio::test]
async fn http_listing_over_500_entries_has_exact_ordered_pages() {
    let fixture = HttpFixture::new("large-listing", 503).await;
    let (status, body) = fixture.list("?limit=500").await;
    assert_eq!(status, 200);
    let first: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = first["items"].as_array().unwrap();
    assert_eq!(items.len(), 500);
    assert_eq!(first["has_more"], true);
    for (i, item) in items.iter().enumerate() {
        assert_eq!(item["name"], format!("item-{i:04}"), "first page item {i}");
    }
    let cursor = first["next_cursor"].as_str().unwrap();
    let (status, body) = fixture.list(&format!("?limit=500&cursor={cursor}")).await;
    assert_eq!(status, 200);
    let last: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(last["items"].as_array().unwrap().len(), 3);
    assert_eq!(last["items"][0]["name"], "item-0500");
    assert_eq!(last["items"][1]["name"], "item-0501");
    assert_eq!(last["items"][2]["name"], "item-0502");
    assert_eq!(last["has_more"], false);
    fixture.close().await;
}

#[tokio::test]
async fn raw_http_rejects_tampered_oversized_and_cross_actor_cursors() {
    let fixture = HttpFixture::new("cursor-errors", 4).await;
    let (status, body) = fixture.list("?limit=2").await;
    assert_eq!(status, 200);
    let cursor = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["next_cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut tampered = cursor.clone().into_bytes();
    let i = tampered.len() - 1;
    tampered[i] = if tampered[i] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).unwrap();
    let (status, _) = fixture.list(&format!("?limit=2&cursor={tampered}")).await;
    assert_eq!(status, 400, "modified cursor tag is rejected");
    let oversized = "A".repeat(2049);
    let (status, _) = fixture.list(&format!("?limit=2&cursor={oversized}")).await;
    assert_eq!(status, 400, "oversized cursor is rejected before decode");
    let (status, body) = fixture
        .list_as(
            &format!("?folder_id={OTHER_ROOT}&limit=2&cursor={cursor}"),
            OTHER_SESSION,
            OTHER_SECRET,
        )
        .await;
    assert_eq!(
        status, 400,
        "cursor bound to another actor/folder is rejected"
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "invalid_cursor"
    );
    fixture.close().await;
}

#[tokio::test]
async fn raw_invalid_utf8_name_body_is_rejected() {
    let fixture = HttpFixture::new("invalid-utf8", 0).await;
    let mut body = br#"{"name":"bad"}"#.to_vec();
    let at = body.iter().position(|b| *b == b'd').unwrap();
    body[at] = 0xff;
    let (status, _) = fixture.request_bytes("/folders/children", "", &body).await;
    assert_eq!(
        status, 400,
        "invalid UTF-8 JSON must not be accepted or trimmed"
    );
    fixture.close().await;
}

#[tokio::test]
async fn stale_cursor_and_barrier_ordered_listing_are_deterministic() {
    let fixture = HttpFixture::new("stale-cursor", 3).await;
    let (status, body) = fixture.list("?limit=1").await;
    assert_eq!(status, 200);
    let cursor = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["next_cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let create = fixture
        .request_bytes(
            "/folders/children",
            "",
            format!(r#"{{"parent_id":"{ROOT}","name":"new"}}"#).as_bytes(),
        )
        .await;
    assert_eq!(create.0, 201);
    let (status, body) = fixture.list(&format!("?limit=1&cursor={cursor}")).await;
    assert_eq!(
        status, 409,
        "generation-stale cursor asks the client to restart"
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "listing_changed"
    );

    let db = std::sync::Arc::new(std::sync::Mutex::new(namespace_db()));
    let barrier = std::sync::Arc::new(TestBarrier::new("names-listing-order"));
    let worker_barrier = barrier.clone();
    let worker_db = db.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let join = std::thread::spawn(move || {
        worker_barrier.worker_pause(Duration::from_secs(2)).unwrap();
        let guard = worker_db.lock().unwrap();
        let page = namespace::list(&guard, OWNER, ROOT, None, 10).unwrap();
        tx.send(page.into_iter().map(|entry| entry.name).collect::<Vec<_>>())
            .unwrap();
    });
    barrier.wait_reached(Duration::from_secs(2)).unwrap();
    {
        let guard = db.lock().unwrap();
        let tx = guard.unchecked_transaction().unwrap();
        namespace::create_folder(&tx, OWNER, ROOT, "middle").unwrap();
        tx.commit().unwrap();
    }
    barrier.release().unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        vec!["middle"]
    );
    join.join().unwrap();
    barrier.assert_drained().unwrap();
    fixture.close().await;
}

#[test]
fn trash_requires_empty_folders_and_restores_parent_before_child() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    let parent = namespace::create_folder(&tx, OWNER, ROOT, "parent").unwrap();
    let child = namespace::create_folder(&tx, OWNER, &parent.id, "child").unwrap();
    let parent = namespace::list(&tx, OWNER, ROOT, None, 10)
        .unwrap()
        .remove(0);
    assert!(matches!(
        namespace::trash(&tx, OWNER, &parent.id, &parent.etag),
        Err(NamespaceError::NotEmpty)
    ));
    assert!(matches!(
        namespace::trash(&tx, OTHER, &child.id, &child.etag),
        Err(NamespaceError::NotFound)
    ));
    namespace::trash(&tx, OWNER, &child.id, &child.etag).unwrap();
    let parent = namespace::list(&tx, OWNER, ROOT, None, 10)
        .unwrap()
        .remove(0);
    namespace::trash(&tx, OWNER, &parent.id, &parent.etag).unwrap();
    let deleted = namespace::trashed(&tx, OWNER).unwrap();
    let child_deleted = deleted.iter().find(|e| e.entry.id == child.id).unwrap();
    let parent_deleted = deleted.iter().find(|e| e.entry.id == parent.id).unwrap();
    assert!(matches!(
        namespace::restore(&tx, OWNER, &child.id, &child_deleted.entry.etag),
        Err(NamespaceError::ParentUnavailable)
    ));
    namespace::restore(&tx, OWNER, &parent.id, &parent_deleted.entry.etag).unwrap();
    namespace::restore(&tx, OWNER, &child.id, &child_deleted.entry.etag).unwrap();
    assert_eq!(
        namespace::list(&tx, OWNER, &parent.id, None, 10)
            .unwrap()
            .len(),
        1
    );
    tx.commit().unwrap();
}

#[test]
fn deleted_entries_include_owner_folder_paths_even_when_parent_is_deleted() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    let work = namespace::create_folder(&tx, OWNER, ROOT, "Work").unwrap();
    let home = namespace::create_folder(&tx, OWNER, ROOT, "Home").unwrap();
    let first = namespace::create_folder(&tx, OWNER, &work.id, "notes").unwrap();
    let second = namespace::create_folder(&tx, OWNER, &home.id, "notes").unwrap();
    namespace::trash(&tx, OWNER, &first.id, &first.etag).unwrap();
    namespace::trash(&tx, OWNER, &second.id, &second.etag).unwrap();
    let work = namespace::list(&tx, OWNER, ROOT, None, 10)
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == work.id)
        .unwrap();
    namespace::trash(&tx, OWNER, &work.id, &work.etag).unwrap();
    let deleted = namespace::trashed(&tx, OWNER).unwrap();
    assert_eq!(
        deleted
            .iter()
            .find(|item| item.entry.id == first.id)
            .unwrap()
            .original_path,
        vec!["Work"]
    );
    assert_eq!(
        deleted
            .iter()
            .find(|item| item.entry.id == second.id)
            .unwrap()
            .original_path,
        vec!["Home"]
    );
    tx.commit().unwrap();
}
