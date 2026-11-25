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
        self.request_using("POST", path, extra, body).await
    }

    async fn request_using(
        &self,
        method: &str,
        path: &str,
        extra: &str,
        body: &[u8],
    ) -> (u16, Vec<u8>) {
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
        let mut request = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nCookie: {cookie}\r\nAccept: application/json\r\nOrigin: https://127.0.0.1\r\nX-CSRF-Token: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{extra}Content-Length: {}\r\n\r\n", URL_SAFE_NO_PAD.encode(CSRF), body.len()).into_bytes();
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
        self.get_as(&format!("/folders/children{query}"), sid, secret)
            .await
    }
    async fn search(&self, query: &str) -> (u16, Vec<u8>) {
        self.get_as(&format!("/entries/search{query}"), SESSION, SECRET)
            .await
    }
    async fn search_as(&self, query: &str, sid: &str, secret: &[u8; 32]) -> (u16, Vec<u8>) {
        self.get_as(&format!("/entries/search{query}"), sid, secret)
            .await
    }
    async fn get_as(&self, path: &str, sid: &str, secret: &[u8; 32]) -> (u16, Vec<u8>) {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            time::timeout,
        };
        let mut stream = tls_connect(self.server.address(), &self.ca, "127.0.0.1")
            .await
            .unwrap();
        let cookie = format!("__Host-sylux={}.{}", sid, URL_SAFE_NO_PAD.encode(secret));
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nCookie: {cookie}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
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
    sylux::persistence::schema::migrate_v1(&db, None).unwrap();
    sylux::persistence::schema::migrate_v2(&db, None).unwrap();
    sylux::persistence::schema::migrate_v3(&db, None).unwrap();
    sylux::persistence::schema::migrate_v4(&db, None).unwrap();
    sylux::persistence::schema::migrate_direct_grants(&db, None).unwrap();
    sylux::persistence::schema::migrate_library_publication(&db, None).unwrap();
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
async fn search_reaches_nested_literal_matches_and_isolates_paths_and_pages() {
    let fixture = HttpFixture::new("account-search", 103).await;
    let nested = fixture
        .request_bytes(
            "/folders/children",
            "",
            format!(r#"{{"parent_id":"{ROOT}","name":"Projects"}}"#).as_bytes(),
        )
        .await;
    assert_eq!(nested.0, 201);
    let nested: serde_json::Value = serde_json::from_slice(&nested.1).unwrap();
    let parent = nested["id"].as_str().unwrap();
    let child = fixture
        .request_bytes(
            "/folders/children",
            "",
            format!(r#"{{"parent_id":"{parent}","name":"Notes%_Å"}}"#).as_bytes(),
        )
        .await;
    assert_eq!(child.0, 201);
    let child: serde_json::Value = serde_json::from_slice(&child.1).unwrap();
    let workers = fixture.workers.as_ref().unwrap().clone();
    workers.transact(|tx| {
        tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder','Notes%_private',?4,1)", params!["0000000000000000000000000000f001", OTHER, OTHER_ROOT, b"Notes%_private"]).unwrap();
        Ok(())
    }).await.unwrap();
    let (status, body) = fixture.search("?q=notes%25_%C3%A5").await;
    assert_eq!(status, 200);
    let found: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
    assert_eq!(found["items"][0]["entry"]["name"], "Notes%_Å");
    assert_eq!(found["items"][0]["location"][1]["name"], "Projects");
    assert_eq!(found["items"][0]["location"][0]["id"], ROOT);
    let (status, body) = fixture.search("?q=notes%25_A%CC%8A").await;
    assert_eq!(status, 200);
    let decomposed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(decomposed["items"][0]["entry"]["id"], child["id"]);
    let (_, body) = fixture.search("?q=%25_").await;
    let found: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        found["items"].as_array().unwrap().len(),
        1,
        "punctuation is literal and other account is private"
    );
    let (_, body) = fixture.search("?q=item&limit=100").await;
    let first: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 100);
    let cursor = first["next_cursor"].as_str().unwrap();
    let mut tampered = cursor.as_bytes().to_vec();
    let last_byte = tampered.len() - 1;
    tampered[last_byte] = if tampered[last_byte] == b'A' {
        b'B'
    } else {
        b'A'
    };
    assert_eq!(
        fixture
            .search(&format!(
                "?q=item&cursor={}",
                String::from_utf8(tampered).unwrap()
            ))
            .await
            .0,
        400
    );
    assert_eq!(
        fixture
            .search(&format!("?q=item&cursor={}", "A".repeat(4097)))
            .await
            .0,
        400
    );
    let (status, body) = fixture
        .search(&format!("?q=item&limit=100&cursor={cursor}"))
        .await;
    assert_eq!(status, 200);
    let last: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(last["items"].as_array().unwrap().len(), 3);
    assert_eq!(last["items"][0]["entry"]["name"], "item-0100");
    assert_eq!(last["has_more"], false);
    let (status, body) = fixture.search(&format!("?q=Items&cursor={cursor}")).await;
    assert_eq!(status, 400);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "invalid_cursor"
    );
    let (status, body) = fixture
        .search_as(
            &format!("?q=item&cursor={cursor}"),
            OTHER_SESSION,
            OTHER_SECRET,
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "invalid_cursor"
    );
    let (status, body) = fixture
        .search_as("?q=Notes", OTHER_SESSION, OTHER_SECRET)
        .await;
    assert_eq!(status, 200);
    let other: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(other["items"].as_array().unwrap().len(), 1);
    assert_eq!(other["items"][0]["location"][0]["id"], OTHER_ROOT);
    fixture.close().await;
}

#[tokio::test]
async fn search_scopes_descendants_before_pagination_and_binds_cursors_to_the_folder() {
    let fixture = HttpFixture::new("folder-search", 0).await;
    let (parent, scope, child, sibling, hidden, hidden_child, foreign, file) = fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            let folder = |parent, name| namespace::create_folder(tx, OWNER, parent, name).unwrap();
            let parent = folder(ROOT, "Projects");
            let scope = folder(&parent.id, "match-scope");
            let child = folder(&scope.id, "Drafts");
            let sibling = folder(&parent.id, "Sibling");
            folder(&sibling.id, "match-a-sibling");
            let hidden = folder(&scope.id, "match-hidden");
            let hidden_child = folder(&hidden.id, "match-hidden-child");
            let hidden_etag = namespace::folder_path(tx, OWNER, &hidden.id)
                .unwrap()
                .pop()
                .unwrap()
                .etag;
            namespace::trash(tx, OWNER, &hidden.id, &hidden_etag).unwrap();
            let foreign = namespace::create_folder(tx, OTHER, OTHER_ROOT, "match-private").unwrap();
            let file = "0000000000000000000000000000fa01";
            insert_sized_file_at(tx, file, "match-b-local", 12, 1);
            tx.execute(
                "UPDATE entry SET parent_id=?1 WHERE entry_id=?2",
                params![scope.id, file],
            )
            .unwrap();
            folder(&child.id, "match-c-descendant");
            Ok((
                parent.id,
                scope.id,
                child.id,
                sibling.id,
                hidden.id,
                hidden_child.id,
                foreign.id,
                file,
            ))
        })
        .await
        .unwrap();

    let query = "?q=match&limit=1";
    let scoped = format!("{query}&folder_id={scope}");
    let mut first_cursors = Vec::new();
    for (path, expected) in [
        (scoped.as_str(), vec!["match-b-local", "match-c-descendant"]),
        (
            query,
            vec![
                "match-a-sibling",
                "match-b-local",
                "match-c-descendant",
                "match-scope",
            ],
        ),
    ] {
        let mut cursor = None;
        for (index, name) in expected.iter().enumerate() {
            let suffix = cursor
                .as_ref()
                .map(|value| format!("&cursor={value}"))
                .unwrap_or_default();
            let (status, body) = fixture.search(&format!("{path}{suffix}")).await;
            assert_eq!(status, 200, "{path}: {}", String::from_utf8_lossy(&body));
            let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let items = page["items"].as_array().unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0]["entry"]["name"], *name);
            if *name == "match-c-descendant" {
                let location = items[0]["location"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| entry["id"].as_str().unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(location, [ROOT, &parent, &scope, &child]);
            }
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            assert_eq!(page["has_more"], index + 1 < expected.len());
            assert_eq!(cursor.is_some(), index + 1 < expected.len());
            if index == 0 {
                first_cursors.push(cursor.clone().unwrap());
            }
        }
    }
    let nested_cursor = &first_cursors[0];
    let root_cursor = &first_cursors[1];
    for (path, cursor) in [
        (query.to_owned(), nested_cursor),
        (format!("{query}&folder_id={sibling}"), nested_cursor),
        (scoped.clone(), root_cursor),
    ] {
        let (status, body) = fixture.search(&format!("{path}&cursor={cursor}")).await;
        assert_eq!(status, 400);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "invalid_cursor"
        );
    }
    assert_eq!(
        fixture
            .search(&format!("{query}&folder_id={ROOT}&cursor={root_cursor}"))
            .await
            .0,
        200
    );
    for invalid in ["bogus", file, &hidden, &hidden_child, &foreign] {
        let (status, body) = fixture
            .search(&format!("{query}&folder_id={invalid}"))
            .await;
        assert_eq!(status, 404, "{invalid}");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "not_found"
        );
    }

    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(move |tx| {
            let etag = namespace::folder_path(tx, OWNER, &parent).unwrap()[1]
                .etag
                .clone();
            namespace::rename(tx, OWNER, &parent, "Renamed", &etag).unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let (status, body) = fixture
        .search(&format!("{scoped}&cursor={nested_cursor}"))
        .await;
    assert_eq!(status, 409);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "listing_changed"
    );
    fixture.close().await;
}

#[tokio::test]
async fn scoped_search_reaches_match_at_maximum_absolute_depth() {
    let fixture = HttpFixture::new("deep-folder-search", 0).await;
    let (scope, deepest) = fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            let mut parent = ROOT.to_owned();
            let mut scope = String::new();
            for depth in 1..=128 {
                let name = if depth == 128 {
                    "boundary-match".to_owned()
                } else {
                    format!("depth-{depth}")
                };
                parent = namespace::create_folder(tx, OWNER, &parent, &name)
                    .unwrap()
                    .id;
                if depth == 120 {
                    scope = parent.clone();
                }
            }
            Ok((scope, parent))
        })
        .await
        .unwrap();
    let (status, body) = fixture
        .search(&format!("?q=boundary-match&folder_id={scope}"))
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["entry"]["id"], deepest);
    assert_eq!(items[0]["location"].as_array().unwrap().len(), 128);
    fixture.close().await;
}

#[tokio::test]
async fn search_page_moves_fifty_files_through_http_with_ownership_etags_and_conflicts() {
    let fixture = HttpFixture::new("search-bulk-move", 0).await;
    let (destination, blocker) = fixture.workers.as_ref().unwrap().transact(|tx| {
        let alpha = namespace::create_folder(tx, OWNER, ROOT, "Alpha").unwrap();
        let beta = namespace::create_folder(tx, OWNER, ROOT, "Beta").unwrap();
        let destination = namespace::create_folder(tx, OWNER, ROOT, "Destination").unwrap();
        for index in 0..50 {
            let id = format!("{:032x}", 0x3000 + index);
            insert_sized_file(tx, &id, &format!("batch-{index:02}.bin"), index);
            let parent = if index % 2 == 0 { &alpha.id } else { &beta.id };
            tx.execute("UPDATE entry SET parent_id=?1 WHERE entry_id=?2", params![parent, id])?;
        }
        let blocker = namespace::create_folder(tx, OWNER, &destination.id, "batch-00.bin").unwrap();
        tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES('0000000000000000000000000000ffff',?1,?2,'file','batch-private.bin',?3,1)", params![OTHER, OTHER_ROOT, b"batch-private.bin"])?;
        Ok((destination, blocker))
    }).await.unwrap();
    let (status, body) = fixture.search("?q=batch-&limit=100").await;
    assert_eq!(status, 200);
    let found: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let selected: Vec<_> = found["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|hit| hit["entry"]["kind"] == "file")
        .cloned()
        .collect();
    assert_eq!(selected.len(), 50);
    assert!(
        selected
            .iter()
            .all(|hit| hit["entry"]["name"] != "batch-private.bin")
    );
    let (status, body) = fixture
        .search_as("?q=batch-", OTHER_SESSION, OTHER_SECRET)
        .await;
    assert_eq!(status, 200);
    let other: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(other["items"].as_array().unwrap().len(), 1);
    assert_eq!(other["items"][0]["entry"]["name"], "batch-private.bin");
    let first = selected
        .iter()
        .find(|hit| hit["entry"]["name"] == "batch-00.bin")
        .unwrap();
    let path = format!("/entries/{}/move", first["entry"]["id"].as_str().unwrap());
    let body = serde_json::to_vec(&serde_json::json!({ "parent_id": destination.id })).unwrap();
    let (status, response) = fixture
        .request_bytes(
            &path,
            &format!("If-Match: {}\r\n", first["entry"]["etag"].as_str().unwrap()),
            &body,
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&response).unwrap()["code"],
        "name_conflict"
    );
    assert_eq!(
        fixture
            .request_bytes(&path, "If-Match: \"stale\"\r\n", &body)
            .await
            .0,
        412
    );
    assert_eq!(
        fixture
            .request_bytes(
                "/entries/0000000000000000000000000000ffff/move",
                "If-Match: \"g1\"\r\n",
                &body
            )
            .await
            .0,
        404
    );
    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(move |tx| {
            namespace::trash(tx, OWNER, &blocker.id, &blocker.etag).unwrap();
            Ok(())
        })
        .await
        .unwrap();
    for hit in &selected {
        let entry = &hit["entry"];
        let (status, response) = fixture
            .request_bytes(
                &format!("/entries/{}/move", entry["id"].as_str().unwrap()),
                &format!("If-Match: {}\r\n", entry["etag"].as_str().unwrap()),
                &body,
            )
            .await;
        assert_eq!(
            status,
            200,
            "{}: {}",
            entry["name"],
            String::from_utf8_lossy(&response)
        );
    }
    let (status, body) = fixture.search("?q=batch-&limit=100").await;
    assert_eq!(status, 200);
    let refreshed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let hits = refreshed["items"].as_array().unwrap();
    assert_eq!(hits.len(), 50);
    for hit in hits {
        assert_eq!(hit["entry"]["parent_id"], destination.id);
        assert_eq!(hit["location"][1]["name"], "Destination");
        let original = selected
            .iter()
            .find(|original| original["entry"]["id"] == hit["entry"]["id"])
            .unwrap();
        assert_ne!(hit["entry"]["etag"], original["entry"]["etag"]);
    }
    let (status, body) = fixture
        .get_as(
            &format!("/folders/{}/children?limit=100", destination.id),
            SESSION,
            SECRET,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        50
    );
    fixture.close().await;
}

#[tokio::test]
async fn search_rejects_invalid_inputs_and_restarts_after_nested_changes() {
    let fixture = HttpFixture::new("search-changes", 2).await;
    for query in ["?q=", "?q=%0A", "?q=a&limit=0", "?q=a&limit=101"] {
        assert_eq!(fixture.search(query).await.0, 400, "{query}");
    }
    assert_eq!(
        fixture.search(&format!("?q={}", "a".repeat(256))).await.0,
        400
    );
    assert_eq!(
        fixture
            .search_as("?q=item", "00000000000000000000000000000fff", SECRET)
            .await
            .0,
        401
    );
    let (status, body) = fixture.search("?q=item&limit=1").await;
    assert_eq!(status, 200);
    let first: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap();
    let workers = fixture.workers.as_ref().unwrap().clone();
    workers
        .transact(|tx| {
            let entry = namespace::list(tx, OWNER, ROOT, None, 1).unwrap().remove(0);
            namespace::rename(tx, OWNER, &entry.id, "renamed", &entry.etag).unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let (status, body) = fixture
        .search(&format!("?q=item&limit=1&cursor={cursor}"))
        .await;
    assert_eq!(
        status, 409,
        "changed search restarts even when fewer matches remain"
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "listing_changed"
    );
    let (status, body) = fixture.search("?q=renamed").await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["items"][0]["entry"]["name"],
        "renamed"
    );
    fixture.close().await;
}

#[tokio::test]
async fn search_orders_duplicate_files_by_location_and_tracks_move_and_trash() {
    let fixture = HttpFixture::new("search-mutations", 0).await;
    let workers = fixture.workers.as_ref().unwrap().clone();
    let (alpha, beta, gamma) = workers.transact(|tx| {
        let alpha = namespace::create_folder(tx, OWNER, ROOT, "Alpha").unwrap();
        let beta = namespace::create_folder(tx, OWNER, ROOT, "Beta").unwrap();
        let gamma = namespace::create_folder(tx, OWNER, ROOT, "Gamma").unwrap();
        for (id, parent) in [("0000000000000000000000000000f101", alpha.id.as_str()), ("0000000000000000000000000000f102", beta.id.as_str())] {
            tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'file','Report.txt',?4,1)", params![id, OWNER, parent, b"Report.txt"]).unwrap();
        }
        Ok((alpha, beta, gamma))
    }).await.unwrap();
    let (status, body) = fixture.search("?q=report").await;
    assert_eq!(status, 200);
    let found: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = found["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["entry"]["kind"], "file");
    assert_eq!(items[0]["location"][1]["name"], "Alpha");
    assert_eq!(items[1]["location"][1]["name"], "Beta");
    let file_id = items[0]["entry"]["id"].as_str().unwrap().to_owned();
    let etag = items[0]["entry"]["etag"].as_str().unwrap().to_owned();
    workers
        .transact(move |tx| {
            namespace::move_entry(tx, OWNER, &file_id, &gamma.id, &etag).unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let (_, body) = fixture.search("?q=report").await;
    let moved: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(moved["items"].as_array().unwrap().len(), 2);
    assert_eq!(moved["items"][0]["location"][1]["name"], "Beta");
    assert_eq!(moved["items"][1]["location"][1]["name"], "Gamma");
    let moved_hit = moved["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hit| hit["entry"]["id"] == "0000000000000000000000000000f101")
        .unwrap();
    let new_etag = moved_hit["entry"]["etag"].as_str().unwrap().to_owned();
    workers
        .transact(move |tx| {
            namespace::trash(tx, OWNER, "0000000000000000000000000000f101", &new_etag).unwrap();
            tx.execute(
                "UPDATE entry SET state='deleted' WHERE entry_id=?1",
                [&alpha.id],
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let (_, body) = fixture.search("?q=report").await;
    let after: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(after["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        after["items"][0]["entry"]["id"],
        "0000000000000000000000000000f102"
    );
    workers
        .transact(move |tx| {
            tx.execute(
                "UPDATE entry SET state='deleted' WHERE entry_id=?1",
                [&beta.id],
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let (_, body) = fixture.search("?q=report").await;
    assert!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty(),
        "active children of a non-live parent are outside the live tree"
    );
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
fn folder_recovery_preserves_earlier_deleted_child() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    let parent = namespace::create_folder(&tx, OWNER, ROOT, "parent").unwrap();
    let child = namespace::create_folder(&tx, OWNER, &parent.id, "child").unwrap();
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

fn insert_sized_file(db: &Connection, id: &str, name: &str, size: i64) {
    insert_sized_file_at(db, id, name, size, 1);
}

fn insert_sized_file_at(db: &Connection, id: &str, name: &str, size: i64, time: i64) {
    let key = EntryName::parse(name).unwrap().key;
    db.execute(
        "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'file',?4,?5,1)",
        params![id, OWNER, ROOT, name, key],
    ).unwrap();
    db.execute(
        "INSERT INTO blob(blob_id,shard,length,sha256) VALUES(?1,substr(?1,1,2),?2,zeroblob(32))",
        params![id, size],
    )
    .unwrap();
    db.execute(
        "INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?1,?1,?2,zeroblob(32),?4,?3)",
        params![id, size, OWNER, time],
    ).unwrap();
    db.execute(
        "UPDATE entry SET current_revision_id=?1 WHERE entry_id=?1",
        [id],
    )
    .unwrap();
}

#[tokio::test]
async fn folder_sorting_orders_complete_listing_before_paging_and_returns_file_sizes() {
    let fixture = HttpFixture::new("sorted-pages", 0).await;
    fixture.workers.as_ref().unwrap().transact(|tx| {
        for (id, name) in [
            ("00000000000000000000000000001003", "Å folder"),
            ("00000000000000000000000000001001", "B folder"),
            ("00000000000000000000000000001002", "Z folder"),
        ] {
            let key = EntryName::parse(name).unwrap().key;
            tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder',?4,?5,1)", params![id, OWNER, ROOT, name, key]).unwrap();
        }
        for (id, name, size) in [
            ("00000000000000000000000000002001", "zero.txt", 0),
            ("00000000000000000000000000002002", "a.txt", 5),
            ("00000000000000000000000000002003", "Z.txt", 5),
            ("00000000000000000000000000002004", "A\u{30a}.txt", 5),
            ("00000000000000000000000000002005", "big.txt", 90),
            ("00000000000000000000000000002006", "small.txt", 1),
        ] {
            insert_sized_file(tx, id, name, size);
        }
        Ok(())
    }).await.unwrap();
    let folders = ["B folder", "Z folder", "Å folder"];
    for (sort, files) in [
        (
            "name_asc",
            [
                "Z.txt",
                "a.txt",
                "big.txt",
                "small.txt",
                "zero.txt",
                "A\u{30a}.txt",
            ],
        ),
        (
            "name_desc",
            [
                "A\u{30a}.txt",
                "zero.txt",
                "small.txt",
                "big.txt",
                "a.txt",
                "Z.txt",
            ],
        ),
        (
            "size_asc",
            [
                "zero.txt",
                "small.txt",
                "Z.txt",
                "a.txt",
                "A\u{30a}.txt",
                "big.txt",
            ],
        ),
        (
            "size_desc",
            [
                "big.txt",
                "Z.txt",
                "a.txt",
                "A\u{30a}.txt",
                "small.txt",
                "zero.txt",
            ],
        ),
    ] {
        let expected: Vec<_> = folders.into_iter().chain(files).collect();
        for limit in [1, 2, 4, 500] {
            let mut cursor = None;
            let mut names = Vec::new();
            let mut ids = std::collections::HashSet::new();
            loop {
                let query = format!(
                    "?sort={sort}&limit={limit}{}",
                    cursor
                        .as_ref()
                        .map(|value| format!("&cursor={value}"))
                        .unwrap_or_default()
                );
                let (status, body) = fixture.list(&query).await;
                assert_eq!(status, 200, "{sort}: {}", String::from_utf8_lossy(&body));
                let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let items = page["items"].as_array().unwrap();
                assert!(!items.is_empty());
                for item in items {
                    assert!(
                        ids.insert(item["id"].as_str().unwrap().to_owned()),
                        "duplicate entry in {sort}"
                    );
                    let name = item["name"].as_str().unwrap();
                    names.push(name.to_owned());
                    if item["kind"] == "file" {
                        let expected_size = match name {
                            "zero.txt" => 0,
                            "small.txt" => 1,
                            "big.txt" => 90,
                            _ => 5,
                        };
                        assert_eq!(item["size"].as_i64(), Some(expected_size));
                    } else {
                        assert_eq!(item["size"].as_i64(), Some(0));
                    }
                }
                cursor = page["next_cursor"].as_str().map(str::to_owned);
                assert_eq!(page["has_more"], cursor.is_some());
                if cursor.is_none() {
                    break;
                }
                assert!(names.len() < expected.len(), "paging must terminate");
            }
            assert_eq!(names, expected, "{sort} with page size {limit}");
        }
    }
    let (status, body) = fixture
        .get_as(
            &format!("/folders/{ROOT}/children?sort=size_desc"),
            SESSION,
            SECRET,
        )
        .await;
    assert_eq!(status, 200);
    let explicit: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        explicit["items"][3]["name"], "big.txt",
        "explicit folder route forwards sorting"
    );
    let (_, default) = fixture.list("").await;
    let (_, name_asc) = fixture.list("?sort=name_asc").await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&default).unwrap(),
        serde_json::from_slice::<serde_json::Value>(&name_asc).unwrap()
    );
    fixture.close().await;
}

#[tokio::test]
async fn folder_sorting_rejects_invalid_modes_and_cross_sort_cursors() {
    let fixture = HttpFixture::new("sort-cursors", 3).await;
    for invalid in ["", "NAME_ASC", "modified", "size"] {
        let (status, body) = fixture.list(&format!("?sort={invalid}")).await;
        assert_eq!(status, 400);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "invalid_sort"
        );
    }
    let sorts = [
        "name_asc",
        "name_desc",
        "size_asc",
        "size_desc",
        "modified_asc",
        "modified_desc",
    ];
    for sort in sorts {
        let (status, body) = fixture.list(&format!("?sort={sort}&limit=1")).await;
        assert_eq!(status, 200);
        let first: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let cursor = first["next_cursor"].as_str().unwrap();
        for other in sorts.into_iter().filter(|other| *other != sort) {
            let (status, body) = fixture
                .list(&format!("?sort={other}&cursor={cursor}"))
                .await;
            assert_eq!(status, 400, "{sort} cursor reused for {other}");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
                "invalid_cursor"
            );
        }
        if sort == "name_asc" {
            assert_eq!(fixture.list(&format!("?cursor={cursor}")).await.0, 200);
        }
        fixture
            .workers
            .as_ref()
            .unwrap()
            .transact(|tx| {
                tx.execute(
                    "UPDATE entry SET generation=generation+1 WHERE entry_id=?1",
                    [ROOT],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let (status, body) = fixture.list(&format!("?sort={sort}&cursor={cursor}")).await;
        assert_eq!(status, 409);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "listing_changed"
        );
    }
    fixture.close().await;
}

#[test]
fn folder_sorting_breaks_equal_name_keys_by_id_on_every_page() {
    let db = namespace_db();
    // Exercise the final tie-breaker despite active sibling uniqueness normally preventing ties.
    db.execute("DROP INDEX entry_active_sibling", []).unwrap();
    for id in [
        "00000000000000000000000000002003",
        "00000000000000000000000000002001",
        "00000000000000000000000000002002",
    ] {
        insert_sized_file(&db, id, "same.txt", 5);
    }
    for sort in [
        namespace::ListingSort::NameAsc,
        namespace::ListingSort::NameDesc,
        namespace::ListingSort::SizeAsc,
        namespace::ListingSort::SizeDesc,
        namespace::ListingSort::ModifiedAsc,
        namespace::ListingSort::ModifiedDesc,
    ] {
        let mut cursor = None;
        let mut ids = Vec::new();
        loop {
            let mut rows =
                namespace::list_sorted(&db, OWNER, ROOT, sort, cursor.as_ref(), 1).unwrap();
            let more = rows.len() > 1;
            let row = rows.remove(0);
            ids.push(row.entry.id);
            cursor = Some(row.position);
            if !more {
                break;
            }
            assert!(ids.len() < 3);
        }
        assert_eq!(
            ids,
            [
                "00000000000000000000000000002001",
                "00000000000000000000000000002002",
                "00000000000000000000000000002003"
            ]
        );
    }
}

#[tokio::test]
async fn folder_sorting_traverses_over_500_mixed_entries_in_every_mode() {
    const FOLDERS: usize = 103;
    const FILES: usize = 503;
    let fixture = HttpFixture::new("large-sorted-pages", 0).await;
    let expected = fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            let mut expected = Vec::new();
            for n in (0..FOLDERS).rev() {
                let id = format!("{:032x}", n + 0x1000);
                let name = format!("folder-{n:04}");
                let key = EntryName::parse(&name).unwrap().key;
                tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder',?4,?5,1)", params![id, OWNER, ROOT, name, key])?;
                expected.push((id, name, true, 0));
            }
            for n in (0..FILES).rev() {
                let id = format!("{:032x}", n + 0x2000);
                let name = format!("file-{n:04}");
                let size = (n % 17) as i64 * 13;
                insert_sized_file(tx, &id, &name, size);
                expected.push((id, name, false, size));
            }
            Ok(expected)
        })
        .await
        .unwrap();
    for sort in ["name_asc", "name_desc", "size_asc", "size_desc"] {
        let mut expected = expected.clone();
        expected.sort_by(|left, right| {
            right
                .2
                .cmp(&left.2)
                .then_with(|| {
                    if left.2 {
                        left.1.cmp(&right.1)
                    } else {
                        match sort {
                            "name_asc" => left.1.cmp(&right.1),
                            "name_desc" => right.1.cmp(&left.1),
                            "size_asc" => left.3.cmp(&right.3).then(left.1.cmp(&right.1)),
                            "size_desc" => right.3.cmp(&left.3).then(left.1.cmp(&right.1)),
                            _ => unreachable!(),
                        }
                    }
                })
                .then(left.0.cmp(&right.0))
        });
        let mut cursor = None;
        let mut actual = Vec::new();
        let mut ids = std::collections::HashSet::new();
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= (FOLDERS + FILES).div_ceil(100),
                "{sort} must terminate"
            );
            let query = format!(
                "?sort={sort}&limit=100{}",
                cursor
                    .as_ref()
                    .map(|value| format!("&cursor={value}"))
                    .unwrap_or_default()
            );
            let (status, body) = fixture.list(&query).await;
            assert_eq!(status, 200, "{sort}: {}", String::from_utf8_lossy(&body));
            let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let items = page["items"].as_array().unwrap();
            assert_eq!(items.len(), 100.min(FOLDERS + FILES - actual.len()));
            for item in items {
                let id = item["id"].as_str().unwrap().to_owned();
                assert!(ids.insert(id.clone()), "{sort}: duplicate entry {id}");
                actual.push((
                    id,
                    item["name"].as_str().unwrap().to_owned(),
                    item["kind"] == "folder",
                    item["size"].as_i64().unwrap_or(0),
                ));
            }
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            assert_eq!(page["has_more"], cursor.is_some());
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(actual, expected, "complete order for {sort}");
        assert_eq!(ids.len(), FOLDERS + FILES);
        assert_eq!(pages, (FOLDERS + FILES).div_ceil(100));
    }
    fixture.close().await;
}

fn sized_file_in(db: &Connection, id: &str, parent: &str, size: i64) {
    insert_sized_file(db, id, id, size);
    db.execute(
        "UPDATE entry SET parent_id=?1 WHERE entry_id=?2",
        params![parent, id],
    )
    .unwrap();
}

#[tokio::test]
async fn recursive_folder_sizes_scope_active_trees_and_keep_folder_cursors_stable() {
    let fixture = HttpFixture::new("recursive-folder-sizes", 0).await;
    let (a, b, empty, unknown, nested, inactive) = fixture.workers.as_ref().unwrap().transact(|tx| {
        let a = namespace::create_folder(tx, OWNER, ROOT, "A").unwrap();
        let b = namespace::create_folder(tx, OWNER, ROOT, "B").unwrap();
        let empty = namespace::create_folder(tx, OWNER, ROOT, "Empty").unwrap();
        let unknown = namespace::create_folder(tx, OWNER, ROOT, "Unknown").unwrap();
        let nested = namespace::create_folder(tx, OWNER, &a.id, "Nested").unwrap();
        let inactive = namespace::create_folder(tx, OWNER, &a.id, "Inactive").unwrap();
        sized_file_in(tx, "00000000000000000000000000009001", &a.id, 11);
        sized_file_in(tx, "00000000000000000000000000009002", &nested.id, 23);
        sized_file_in(tx, "00000000000000000000000000009003", &nested.id, 999);
        tx.execute("UPDATE entry SET state='deleted' WHERE entry_id='00000000000000000000000000009003'", []).unwrap();
        sized_file_in(tx, "00000000000000000000000000009004", &inactive.id, 888);
        tx.execute("UPDATE entry SET state='deleted' WHERE entry_id=?1", [&inactive.id]).unwrap();
        let unknown_nested = namespace::create_folder(tx, OWNER, &unknown.id, "Nested unknown").unwrap();
        sized_file_in(tx, "00000000000000000000000000009006", &unknown.id, 17);
        sized_file_in(tx, "00000000000000000000000000009007", &unknown_nested.id, 19);
        tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES('00000000000000000000000000009005',?1,?2,'file','missing',x'6d697373696e67',1)", params![OWNER, unknown_nested.id]).unwrap();
        let unknown_children = namespace::list_sorted(tx, OWNER, &unknown.id, namespace::ListingSort::NameAsc, None, 100).unwrap();
        assert_eq!(unknown_children.iter().find(|row| row.entry.id == unknown_nested.id).unwrap().size, None);
        namespace::create_folder(tx, OTHER, OTHER_ROOT, "Private").unwrap();
        Ok((a, b, empty, unknown, nested, inactive))
    }).await.unwrap();
    for sort in ["name_asc", "name_desc", "size_asc", "size_desc"] {
        let (status, body) = fixture.list(&format!("?sort={sort}")).await;
        assert_eq!(status, 200);
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let rows = page["items"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .map(|v| v["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["A", "B", "Empty", "Unknown"]
        );
        assert_eq!(rows[0]["size"], 34);
        assert_eq!(rows[1]["size"], 0);
        assert_eq!(rows[2]["size"], 0);
        assert!(rows[3].get("size").is_none_or(serde_json::Value::is_null));
        let (status, body) = fixture.list(&format!("?sort={sort}&limit=1")).await;
        assert_eq!(status, 200);
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let cursor = page["next_cursor"].as_str().unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(cursor.split('.').next().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded["last_size"], 0);
        let a_id = a.id.clone();
        fixture
            .workers
            .as_ref()
            .unwrap()
            .transact(move |tx| {
                let folder = namespace::root_entry(tx, OWNER).unwrap();
                let id = format!(
                    "{:032x}",
                    match sort {
                        "name_asc" => 0xa001,
                        "name_desc" => 0xa002,
                        "size_asc" => 0xa003,
                        _ => 0xa004,
                    }
                );
                sized_file_in(tx, &id, &a_id, 1);
                assert_eq!(
                    namespace::root_entry(tx, OWNER).unwrap().generation,
                    folder.generation
                );
                Ok(())
            })
            .await
            .unwrap();
        let (status, body) = fixture
            .list(&format!("?sort={sort}&limit=1&cursor={cursor}"))
            .await;
        assert_eq!(
            status, 200,
            "nested totals do not invalidate the sibling cursor"
        );
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(page["items"][0]["name"], "B");
        // Reset only the synthetic additions for the next sort.
        fixture.workers.as_ref().unwrap().transact(|tx| {
            tx.execute("UPDATE entry SET state='deleted' WHERE entry_id IN ('0000000000000000000000000000a001','0000000000000000000000000000a002','0000000000000000000000000000a003','0000000000000000000000000000a004')", [])?;
            Ok(())
        }).await.unwrap();
    }
    let (status, _) = fixture
        .get_as(
            &format!("/folders/{}/children", a.id),
            OTHER_SESSION,
            OTHER_SECRET,
        )
        .await;
    assert_eq!(status, 404);
    let (status, body) = fixture
        .get_as(
            &format!("/folders/{}/children", inactive.id),
            SESSION,
            SECRET,
        )
        .await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&body));
    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(move |tx| {
            let current = namespace::list(tx, OWNER, &a.id, None, 100)
                .unwrap()
                .into_iter()
                .find(|entry| entry.id == nested.id)
                .unwrap();
            let assert_totals = |a_bytes, b_bytes| {
                let rows = namespace::list_sorted(
                    tx,
                    OWNER,
                    ROOT,
                    namespace::ListingSort::NameAsc,
                    None,
                    100,
                )
                .unwrap();
                assert_eq!(
                    rows.iter().find(|row| row.entry.id == a.id).unwrap().size,
                    Some(a_bytes)
                );
                assert_eq!(
                    rows.iter().find(|row| row.entry.id == b.id).unwrap().size,
                    Some(b_bytes)
                );
            };
            namespace::move_entry(tx, OWNER, &nested.id, &b.id, &current.etag).unwrap();
            assert_totals(11, 23);
            let file = namespace::list(tx, OWNER, &a.id, None, 100)
                .unwrap()
                .into_iter()
                .find(|entry| entry.kind == "file")
                .unwrap();
            namespace::trash(tx, OWNER, &file.id, &file.etag).unwrap();
            assert_totals(0, 23);
            let deleted = namespace::trashed(tx, OWNER)
                .unwrap()
                .into_iter()
                .find(|entry| entry.entry.id == file.id)
                .unwrap();
            namespace::restore(tx, OWNER, &file.id, &deleted.entry.etag).unwrap();
            assert_totals(11, 23);
            let file = namespace::list(tx, OWNER, &a.id, None, 100)
                .unwrap()
                .into_iter()
                .find(|entry| entry.kind == "file")
                .unwrap();
            namespace::move_entry(tx, OWNER, &file.id, &b.id, &file.etag).unwrap();
            let rows = namespace::list_sorted(
                tx,
                OWNER,
                ROOT,
                namespace::ListingSort::SizeDesc,
                None,
                100,
            )
            .unwrap();
            assert_eq!(
                rows.iter().find(|row| row.entry.id == a.id).unwrap().size,
                Some(0)
            );
            assert_eq!(
                rows.iter().find(|row| row.entry.id == b.id).unwrap().size,
                Some(34)
            );
            assert_eq!(
                rows.iter()
                    .find(|row| row.entry.id == empty.id)
                    .unwrap()
                    .size,
                Some(0)
            );
            assert_eq!(
                rows.iter()
                    .find(|row| row.entry.id == unknown.id)
                    .unwrap()
                    .size,
                None
            );
            Ok(())
        })
        .await
        .unwrap();
    fixture.close().await;
}

#[test]
fn recursive_folder_sizes_exclude_populated_lookahead_until_next_page() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    let a = namespace::create_folder(&tx, OWNER, ROOT, "A").unwrap();
    let b = namespace::create_folder(&tx, OWNER, ROOT, "B").unwrap();
    sized_file_in(&tx, "0000000000000000000000000000b001", &a.id, 11);
    sized_file_in(&tx, "0000000000000000000000000000b002", &b.id, 29);
    tx.commit().unwrap();
    for sort in [
        namespace::ListingSort::NameAsc,
        namespace::ListingSort::NameDesc,
        namespace::ListingSort::SizeAsc,
        namespace::ListingSort::SizeDesc,
        namespace::ListingSort::ModifiedAsc,
        namespace::ListingSort::ModifiedDesc,
    ] {
        let first = namespace::list_sorted(&db, OWNER, ROOT, sort, None, 1).unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].entry.id, a.id);
        assert_eq!(first[0].size, Some(11));
        assert_eq!(first[1].entry.id, b.id);
        assert_eq!(
            first[1].size, None,
            "lookahead descendants must not be aggregated"
        );
        let next =
            namespace::list_sorted(&db, OWNER, ROOT, sort, Some(&first[0].position), 1).unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].entry.id, b.id);
        assert_eq!(next[0].size, Some(29));
    }
}

#[tokio::test]
async fn modified_dates_sort_complete_listing_and_preserve_epoch_zero() {
    let fixture = HttpFixture::new("modified-pages", 0).await;
    let expected = fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            namespace::create_folder(tx, OWNER, ROOT, "Z folder").unwrap();
            namespace::create_folder(tx, OWNER, ROOT, "A folder").unwrap();
            let mut files = Vec::new();
            for n in (0..113).rev() {
                let id = format!("{:032x}", n + 0xc000);
                let name = format!("file-{n:03}");
                let time = (n % 7) as i64 * 100;
                insert_sized_file_at(tx, &id, &name, n as i64, time);
                files.push((id, name, time));
            }
            Ok(files)
        })
        .await
        .unwrap();
    for sort in ["modified_asc", "modified_desc"] {
        let mut files = expected.clone();
        files.sort_by(|a, b| {
            (if sort == "modified_asc" {
                a.2.cmp(&b.2)
            } else {
                b.2.cmp(&a.2)
            })
            .then(a.1.cmp(&b.1))
            .then(a.0.cmp(&b.0))
        });
        for limit in [1, 3, 100] {
            let mut cursor = None;
            let mut names = Vec::new();
            let mut ids = std::collections::HashSet::new();
            loop {
                let query = format!(
                    "?sort={sort}&limit={limit}{}",
                    cursor
                        .as_ref()
                        .map(|c| format!("&cursor={c}"))
                        .unwrap_or_default()
                );
                let (status, body) = fixture.list(&query).await;
                assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
                let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
                for item in page["items"].as_array().unwrap() {
                    assert!(ids.insert(item["id"].as_str().unwrap().to_owned()));
                    names.push(item["name"].as_str().unwrap().to_owned());
                    if item["kind"] == "folder" {
                        assert!(item["modified_at"].is_null());
                    } else {
                        let expected_time = expected
                            .iter()
                            .find(|entry| entry.0 == item["id"])
                            .unwrap()
                            .2;
                        assert_eq!(item["modified_at"], expected_time);
                    }
                }
                cursor = page["next_cursor"].as_str().map(str::to_owned);
                if cursor.is_none() {
                    break;
                }
                assert!(names.len() < 115);
            }
            let expected_names = ["A folder".to_owned(), "Z folder".to_owned()]
                .into_iter()
                .chain(files.iter().map(|entry| entry.1.clone()))
                .collect::<Vec<_>>();
            assert_eq!(names, expected_names, "{sort}, page size {limit}");
        }
    }
    fixture.close().await;
}

#[test]
fn modified_dates_survive_namespace_mutations() {
    let mut db = namespace_db();
    let tx = db.transaction().unwrap();
    let id = "0000000000000000000000000000d001";
    insert_sized_file(&tx, id, "original.txt", 42);
    let destination = namespace::create_folder(&tx, OWNER, ROOT, "Destination").unwrap();
    let assert_time = |folder: &str| {
        let rows = namespace::list_sorted(
            &tx,
            OWNER,
            folder,
            namespace::ListingSort::ModifiedDesc,
            None,
            100,
        )
        .unwrap();
        let row = rows.into_iter().find(|row| row.entry.id == id).unwrap();
        assert_eq!(row.modified_at, Some(1));
        row.entry
    };
    let original = assert_time(ROOT);
    namespace::rename(&tx, OWNER, id, "renamed.txt", &original.etag).unwrap();
    let renamed = assert_time(ROOT);
    namespace::move_entry(&tx, OWNER, id, &destination.id, &renamed.etag).unwrap();
    let moved = assert_time(&destination.id);
    namespace::trash(&tx, OWNER, id, &moved.etag).unwrap();
    let deleted = namespace::trashed(&tx, OWNER)
        .unwrap()
        .into_iter()
        .find(|row| row.entry.id == id)
        .unwrap();
    namespace::restore(&tx, OWNER, id, &deleted.entry.etag).unwrap();
    assert_time(&destination.id);
    tx.commit().unwrap();
}

#[tokio::test]
async fn folder_paths_require_owned_active_ancestry_and_return_root_first() {
    let fixture = HttpFixture::new("folder-path", 0).await;
    let (parent, nested) = fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            let parent = namespace::create_folder(tx, OWNER, ROOT, "Parent").unwrap();
            let nested = namespace::create_folder(tx, OWNER, &parent.id, "Nested").unwrap();
            Ok((parent, nested))
        })
        .await
        .unwrap();
    let path = format!("/folders/{}/path", nested.id);
    let (status, body) = fixture.get_as(&path, SESSION, SECRET).await;
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = value["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [ROOT, &parent.id, &nested.id]
    );
    assert_eq!(
        items
            .iter()
            .map(|item| item["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["", "Parent", "Nested"]
    );
    assert_eq!(items[1]["etag"], format!("\"{}:2\"", parent.id));
    assert_eq!(
        fixture.get_as(&path, OTHER_SESSION, OTHER_SECRET).await.0,
        404
    );
    assert_eq!(
        fixture
            .get_as("/folders/missing/path", SESSION, SECRET)
            .await
            .0,
        404
    );
    let renamed_id = parent.id.clone();
    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(move |tx| {
            namespace::rename(
                tx,
                OWNER,
                &renamed_id,
                "Renamed parent",
                &format!("\"{renamed_id}:2\""),
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    let (status, body) = fixture.get_as(&path, SESSION, SECRET).await;
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["items"][1]["name"], "Renamed parent");
    assert_eq!(value["items"][2]["name"], "Nested");
    assert_eq!(value["items"][1]["etag"], format!("\"{}:3\"", parent.id));
    let id = parent.id.clone();
    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(move |tx| {
            tx.execute("UPDATE entry SET state='deleted' WHERE entry_id=?1", [id])?;
            Ok(())
        })
        .await
        .unwrap();
    let (status, body) = fixture.get_as(&path, SESSION, SECRET).await;
    assert_eq!(status, 404);
    assert!(!String::from_utf8_lossy(&body).contains("Parent"));
    fixture.close().await;
}

#[test]
fn folder_paths_reject_corrupt_cycles_missing_and_foreign_ancestors() {
    let mut db = namespace_db();
    db.pragma_update(None, "foreign_keys", false).unwrap();
    let tx = db.transaction().unwrap();
    let parent = namespace::create_folder(&tx, OWNER, ROOT, "Parent").unwrap();
    let nested = namespace::create_folder(&tx, OWNER, &parent.id, "Nested").unwrap();
    tx.execute_batch("DROP TRIGGER entry_update_no_parent_cycle; DROP TRIGGER entry_update_parent_active_folder;").unwrap();
    tx.execute(
        "UPDATE entry SET parent_id=?1 WHERE entry_id=?2",
        params![nested.id, parent.id],
    )
    .unwrap();
    assert!(matches!(
        namespace::folder_path(&tx, OWNER, &nested.id),
        Err(NamespaceError::NotFound)
    ));
    tx.execute(
        "UPDATE entry SET parent_id=?1 WHERE entry_id=?2",
        params![OTHER_ROOT, parent.id],
    )
    .unwrap();
    assert!(matches!(
        namespace::folder_path(&tx, OWNER, &nested.id),
        Err(NamespaceError::NotFound)
    ));
    tx.execute(
        "UPDATE entry SET parent_id='missing' WHERE entry_id=?1",
        [&parent.id],
    )
    .unwrap();
    assert!(matches!(
        namespace::folder_path(&tx, OWNER, &nested.id),
        Err(NamespaceError::NotFound)
    ));
}

#[test]
fn folder_recovery_marks_only_root_and_invalidates_active_descendants() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    let top = namespace::create_folder(&tx, OWNER, ROOT, "tree").unwrap();
    let nested = namespace::create_folder(&tx, OWNER, &top.id, "nested").unwrap();
    let live = namespace::create_folder(&tx, OWNER, &nested.id, "live").unwrap();
    let earlier = namespace::create_folder(&tx, OWNER, &nested.id, "earlier").unwrap();
    let old_child = namespace::create_folder(&tx, OWNER, &earlier.id, "old child").unwrap();
    let current = |id: &str| -> (String, i64) {
        tx.query_row(
            "SELECT state,generation FROM entry WHERE entry_id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    };
    let tag = |id: &str| sylux::domain::upload::folder_etag(id, current(id).1 as u64);
    namespace::trash(&tx, OWNER, &earlier.id, &tag(&earlier.id)).unwrap();
    let before = [&top.id, &nested.id, &live.id, &earlier.id, &old_child.id].map(|id| current(id));
    namespace::trash(&tx, OWNER, &top.id, &tag(&top.id)).unwrap();
    assert_eq!(namespace::trashed(&tx, OWNER).unwrap().len(), 2);
    for (index, id) in [&top.id, &nested.id, &live.id].iter().enumerate() {
        assert_eq!(current(id).1, before[index].1 + 1);
        assert_eq!(current(id).0, if index == 0 { "deleted" } else { "active" });
    }
    assert_eq!(current(&earlier.id), before[3]);
    assert_eq!(current(&old_child.id), before[4]);
    for id in [&nested.id, &live.id] {
        assert!(matches!(
            namespace::folder_path(&tx, OWNER, id),
            Err(NamespaceError::NotFound)
        ));
        assert!(matches!(
            namespace::list(&tx, OWNER, id, None, 10),
            Err(NamespaceError::NotFound)
        ));
        assert!(matches!(
            namespace::create_folder(&tx, OWNER, id, "hidden"),
            Err(NamespaceError::NotFound)
        ));
        assert!(matches!(
            namespace::rename(&tx, OWNER, id, "hidden", &tag(id)),
            Err(NamespaceError::NotFound)
        ));
        assert!(matches!(
            namespace::move_entry(&tx, OWNER, id, ROOT, &tag(id)),
            Err(NamespaceError::NotFound)
        ));
        assert!(matches!(
            namespace::trash(&tx, OWNER, id, &tag(id)),
            Err(NamespaceError::NotFound)
        ));
    }
    assert!(matches!(
        namespace::restore(&tx, OWNER, &earlier.id, &tag(&earlier.id)),
        Err(NamespaceError::ParentUnavailable)
    ));
    assert!(matches!(
        namespace::trash(&tx, OWNER, ROOT, &tag(ROOT)),
        Err(NamespaceError::NotFound)
    ));
    namespace::restore(&tx, OWNER, &top.id, &tag(&top.id)).unwrap();
    assert!(namespace::folder_path(&tx, OWNER, &live.id).is_ok());
    assert!(matches!(
        namespace::rename(&tx, OWNER, &live.id, "stale", &live.etag),
        Err(NamespaceError::Stale)
    ));
    assert_eq!(current(&earlier.id), before[3]);
    assert_eq!(current(&old_child.id), before[4]);
    tx.commit().unwrap();
}

#[test]
fn folder_recovery_rolls_back_generation_updates_and_conflicts_without_changes() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    let top = namespace::create_folder(&tx, OWNER, ROOT, "tree").unwrap();
    let child = namespace::create_folder(&tx, OWNER, &top.id, "child").unwrap();
    tx.commit().unwrap();
    let inventory = || {
        let mut statement = db
            .prepare("SELECT entry_id,state,generation FROM entry ORDER BY entry_id")
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let before = inventory();
    db.execute_batch("CREATE TRIGGER abort_tree_marker BEFORE UPDATE OF state ON entry WHEN NEW.state='deleted' BEGIN SELECT RAISE(ABORT,'synthetic marker failure'); END").unwrap();
    let tx = db.unchecked_transaction().unwrap();
    let top_tag = sylux::domain::upload::folder_etag(&top.id, 2);
    assert!(matches!(
        namespace::trash(&tx, OWNER, &top.id, &top_tag),
        Err(NamespaceError::Sql)
    ));
    tx.rollback().unwrap();
    assert_eq!(inventory(), before);
    db.execute_batch("DROP TRIGGER abort_tree_marker").unwrap();
    let tx = db.unchecked_transaction().unwrap();
    namespace::trash(&tx, OWNER, &top.id, &top_tag).unwrap();
    namespace::create_folder(&tx, OWNER, ROOT, "tree").unwrap();
    tx.commit().unwrap();
    let before = inventory();
    let tx = db.unchecked_transaction().unwrap();
    let deleted = namespace::trashed(&tx, OWNER).unwrap().remove(0);
    assert!(matches!(
        namespace::restore(&tx, OWNER, &top.id, &deleted.entry.etag),
        Err(NamespaceError::Conflict)
    ));
    tx.commit().unwrap();
    assert_eq!(inventory(), before);
    assert!(matches!(
        namespace::folder_path(&db, OWNER, &child.id),
        Err(NamespaceError::NotFound)
    ));
}

#[tokio::test]
async fn folder_recovery_serializes_moves_with_tree_membership() {
    for (trash_first, move_out) in [(false, false), (true, false), (false, true), (true, true)] {
        let fixture = HttpFixture::new("tree-move-order", 0).await;
        let workers = fixture.workers.as_ref().unwrap().clone();
        let (top, nested, source, item) = workers
            .transact(move |tx| {
                let top = namespace::create_folder(tx, OWNER, ROOT, "tree").unwrap();
                let nested = namespace::create_folder(tx, OWNER, &top.id, "nested").unwrap();
                let source = namespace::create_folder(tx, OWNER, ROOT, "source").unwrap();
                let item = namespace::create_folder(
                    tx,
                    OWNER,
                    if move_out { &nested.id } else { &source.id },
                    "item",
                )
                .unwrap();
                let top = namespace::folder_path(tx, OWNER, &top.id)
                    .unwrap()
                    .pop()
                    .unwrap();
                Ok((top, nested, source, item))
            })
            .await
            .unwrap();
        let destination = if move_out {
            source.id.clone()
        } else {
            nested.id.clone()
        };
        let first_destination = destination.clone();
        let nested_id = nested.id.clone();
        let source_id = source.id.clone();
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_workers = workers.clone();
        let (first_top, first_item) = (top.clone(), item.clone());
        let first = tokio::spawn(async move {
            first_workers
                .transact(move |tx| {
                    if trash_first {
                        namespace::trash(tx, OWNER, &first_top.id, &first_top.etag).unwrap();
                    } else {
                        namespace::move_entry(
                            tx,
                            OWNER,
                            &first_item.id,
                            &first_destination,
                            &first_item.etag,
                        )
                        .unwrap();
                    }
                    let _ = reached_tx.send(());
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                })
                .await
                .unwrap();
        });
        reached_rx.await.unwrap();
        let second_top = top.clone();
        let second_item = item.clone();
        let mut second = Box::pin(workers.transact(move |tx| {
            if trash_first {
                Ok(namespace::move_entry(
                    tx,
                    OWNER,
                    &second_item.id,
                    &destination,
                    &second_item.etag,
                )
                .map(|_| ()))
            } else {
                Ok(namespace::trash(
                    tx,
                    OWNER,
                    &second_top.id,
                    &second_top.etag,
                ))
            }
        }));
        tokio::select! { biased; result = &mut second => panic!("queued mutation completed: {result:?}"), () = tokio::task::yield_now() => {} }
        release_tx.send(()).unwrap();
        first.await.unwrap();
        let second = second.await.unwrap();
        if trash_first {
            assert!(matches!(second, Err(NamespaceError::NotFound)));
        } else {
            second.unwrap();
        }
        workers
            .transact(move |tx| {
                let outside = namespace::list(tx, OWNER, &source_id, None, 10).unwrap();
                assert_eq!(
                    outside.len(),
                    usize::from(if move_out { !trash_first } else { trash_first })
                );
                assert!(matches!(
                    namespace::folder_path(tx, OWNER, &top.id),
                    Err(NamespaceError::NotFound)
                ));
                let deleted = namespace::trashed(tx, OWNER)
                    .unwrap()
                    .into_iter()
                    .find(|entry| entry.entry.id == top.id)
                    .unwrap();
                namespace::restore(tx, OWNER, &top.id, &deleted.entry.etag).unwrap();
                let nested_items = namespace::list(tx, OWNER, &nested_id, None, 10).unwrap();
                assert_eq!(
                    nested_items.len(),
                    usize::from(if move_out { trash_first } else { !trash_first })
                );
                let generation: i64 = tx.query_row(
                    "SELECT generation FROM entry WHERE entry_id=?1",
                    [&item.id],
                    |row| row.get(0),
                )?;
                let expected = match (trash_first, move_out) {
                    (false, false) => 3,
                    (true, false) => 1,
                    _ => 2,
                };
                assert_eq!(generation, expected);
                assert_eq!(
                    namespace::folder_path(tx, OWNER, &top.id)
                        .unwrap()
                        .last()
                        .unwrap()
                        .generation,
                    top.generation + 2
                );
                Ok(())
            })
            .await
            .unwrap();
        fixture.close().await;
    }
}

#[tokio::test]
async fn folder_recovery_measures_1000_entry_tree_and_invalidates_descendant_cursor() {
    let fixture = HttpFixture::new("tree-1000", 0).await;
    let workers = fixture.workers.as_ref().unwrap();
    let (top, branch) = workers
        .transact(|tx| {
            let top = namespace::create_folder(tx, OWNER, ROOT, "Tree").unwrap();
            let mut branches = Vec::new();
            for index in 0..9 {
                branches.push(
                    namespace::create_folder(tx, OWNER, &top.id, &format!("branch-{index}"))
                        .unwrap(),
                );
            }
            for index in 0..990 {
                namespace::create_folder(
                    tx,
                    OWNER,
                    &branches[index % 9].id,
                    &format!("leaf-{index:04}"),
                )
                .unwrap();
            }
            Ok((
                namespace::folder_path(tx, OWNER, &top.id)
                    .unwrap()
                    .pop()
                    .unwrap(),
                branches.remove(0),
            ))
        })
        .await
        .unwrap();
    let top_id = top.id.clone();
    let (sql_trash, sql_restore) = workers
        .transact(move |tx| {
            let start = std::time::Instant::now();
            namespace::trash(tx, OWNER, &top.id, &top.etag).unwrap();
            let trash = start.elapsed();
            let deleted = namespace::trashed(tx, OWNER).unwrap().remove(0);
            let start = std::time::Instant::now();
            namespace::restore(tx, OWNER, &top.id, &deleted.entry.etag).unwrap();
            Ok((trash, start.elapsed()))
        })
        .await
        .unwrap();
    let (_, page) = fixture
        .list(&format!("?folder_id={}&limit=1", branch.id))
        .await;
    let page: serde_json::Value = serde_json::from_slice(&page).unwrap();
    let cursor = page["next_cursor"].as_str().unwrap();
    let (_, root) = fixture.list("").await;
    let root: serde_json::Value = serde_json::from_slice(&root).unwrap();
    let top = &root["items"][0];
    let start = std::time::Instant::now();
    let (status, _) = fixture
        .request_using(
            "DELETE",
            &format!("/entries/{top_id}"),
            &format!("If-Match: {}\r\n", top["etag"].as_str().unwrap()),
            &[],
        )
        .await;
    let api_trash = start.elapsed();
    assert_eq!(status, 204);
    assert_eq!(
        fixture.list(&format!("?folder_id={}", branch.id)).await.0,
        404
    );
    let (status, trash) = fixture.get_as("/trash", SESSION, SECRET).await;
    assert_eq!(status, 200);
    let trash: serde_json::Value = serde_json::from_slice(&trash).unwrap();
    assert_eq!(trash["items"].as_array().unwrap().len(), 1);
    let start = std::time::Instant::now();
    let (status, _) = fixture
        .request_using(
            "POST",
            &format!("/trash/{top_id}/restore"),
            &format!(
                "If-Match: {}\r\n",
                trash["items"][0]["etag"].as_str().unwrap()
            ),
            &[],
        )
        .await;
    let api_restore = start.elapsed();
    assert_eq!(status, 204);
    assert_eq!(
        fixture
            .list(&format!("?folder_id={}&limit=1&cursor={cursor}", branch.id))
            .await
            .0,
        409
    );
    let count = workers.read(|db| db.query_row("SELECT count(*) FROM entry WHERE owner_account_id=?1 AND parent_id IS NOT NULL AND state='active'", [OWNER], |row| row.get::<_, i64>(0))).await.unwrap();
    assert_eq!(count, 1000);
    eprintln!(
        "FOLDER_RECOVERY_COST entries=1000 layout=1-root,9-branches,990-leaves sql_trash_us={} sql_restore_us={} tls_api_trash_us={} tls_api_restore_us={} sql_excludes_commit=true api_includes_tls_handshake=true",
        sql_trash.as_micros(),
        sql_restore.as_micros(),
        api_trash.as_micros(),
        api_restore.as_micros()
    );
    fixture.close().await;
}

#[test]
fn folder_recovery_missing_original_parent_rejects_without_generation_changes() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    let parent = namespace::create_folder(&tx, OWNER, ROOT, "parent").unwrap();
    let tree = namespace::create_folder(&tx, OWNER, &parent.id, "tree").unwrap();
    let child = namespace::create_folder(&tx, OWNER, &tree.id, "child").unwrap();
    namespace::trash(
        &tx,
        OWNER,
        &tree.id,
        &sylux::domain::upload::folder_etag(&tree.id, 2),
    )
    .unwrap();
    let deleted = namespace::trashed(&tx, OWNER).unwrap().remove(0);
    tx.commit().unwrap();
    // Model missing legacy inventory; no product route permanently removes parents.
    db.pragma_update(None, "foreign_keys", "OFF").unwrap();
    db.execute("DELETE FROM entry WHERE entry_id=?1", [&parent.id])
        .unwrap();
    db.pragma_update(None, "foreign_keys", "ON").unwrap();
    let tx = db.unchecked_transaction().unwrap();
    assert!(matches!(
        namespace::restore(&tx, OWNER, &tree.id, &deleted.entry.etag),
        Err(NamespaceError::ParentUnavailable)
    ));
    let state: (String, i64) = tx
        .query_row(
            "SELECT state,generation FROM entry WHERE entry_id=?1",
            [&tree.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, ("deleted".into(), 3));
    let generation: i64 = tx
        .query_row(
            "SELECT generation FROM entry WHERE entry_id=?1",
            [&child.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(generation, 2);
    tx.commit().unwrap();
}

#[tokio::test]
async fn folder_recovery_http_failure_rolls_back_the_marker_and_all_generations() {
    let fixture = HttpFixture::new("tree-http-rollback", 0).await;
    let workers = fixture.workers.as_ref().unwrap();
    let top = workers.transact(|tx| {
        let top = namespace::create_folder(tx, OWNER, ROOT, "tree").unwrap();
        namespace::create_folder(tx, OWNER, &top.id, "child").unwrap();
        let top = namespace::folder_path(tx, OWNER, &top.id).unwrap().pop().unwrap();
        tx.execute_batch("CREATE TRIGGER fail_tree_marker BEFORE UPDATE OF state ON entry WHEN NEW.state='deleted' BEGIN SELECT RAISE(ABORT,'synthetic marker failure'); END")?;
        Ok(top)
    }).await.unwrap();
    let inventory = |db: &Connection| {
        let mut statement =
            db.prepare("SELECT entry_id,state,generation FROM entry ORDER BY entry_id")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
    };
    let before = workers.read(inventory).await.unwrap();
    let (status, _) = fixture
        .request_using(
            "DELETE",
            &format!("/entries/{}", top.id),
            &format!("If-Match: {}\r\n", top.etag),
            &[],
        )
        .await;
    assert_eq!(status, 500);
    assert_eq!(workers.read(inventory).await.unwrap(), before);
    fixture.close().await;
}

#[test]
fn folder_recovery_preserves_the_maximum_depth_boundary() {
    let db = namespace_db();
    let tx = db.unchecked_transaction().unwrap();
    let top = namespace::create_folder(&tx, OWNER, ROOT, "depth-1").unwrap();
    let mut deepest = top.clone();
    for depth in 2..=128 {
        deepest =
            namespace::create_folder(&tx, OWNER, &deepest.id, &format!("depth-{depth}")).unwrap();
    }
    let path = namespace::folder_path(&tx, OWNER, &deepest.id).unwrap();
    assert_eq!(path.len(), 129);
    assert!(matches!(
        namespace::create_folder(&tx, OWNER, &deepest.id, "too-deep"),
        Err(NamespaceError::InvalidName)
    ));
    let top = namespace::folder_path(&tx, OWNER, &top.id)
        .unwrap()
        .remove(1);
    namespace::trash(&tx, OWNER, &top.id, &top.etag).unwrap();
    assert!(matches!(
        namespace::folder_path(&tx, OWNER, &deepest.id),
        Err(NamespaceError::NotFound)
    ));
    let deleted = namespace::trashed(&tx, OWNER).unwrap().remove(0);
    namespace::restore(&tx, OWNER, &top.id, &deleted.entry.etag).unwrap();
    assert_eq!(
        namespace::folder_path(&tx, OWNER, &deepest.id)
            .unwrap()
            .len(),
        129
    );
    assert!(
        namespace::list(&tx, OWNER, &deepest.id, None, 10)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        namespace::create_folder(&tx, OWNER, &deepest.id, "too-deep"),
        Err(NamespaceError::InvalidName)
    ));
    assert_eq!(
        tx.query_row(
            "SELECT count(*) FROM entry WHERE owner_account_id=?1 AND parent_id IS NOT NULL",
            [OWNER],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        128
    );
    tx.commit().unwrap();
}

#[tokio::test]
async fn trash_pages_are_bounded_ordered_and_mark_duplicates_across_pages() {
    let fixture = HttpFixture::new("trash-pages", 205).await;
    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            for entry in namespace::list(tx, OWNER, ROOT, None, 500).unwrap() {
                namespace::trash(tx, OWNER, &entry.id, &entry.etag).unwrap();
            }
            for name in ["same", "same", "Å", "A\u{30a}"] {
                let entry = namespace::create_folder(tx, OWNER, ROOT, name).unwrap();
                namespace::trash(tx, OWNER, &entry.id, &entry.etag).unwrap();
            }
            let foreign = namespace::create_folder(tx, OTHER, OTHER_ROOT, "foreign").unwrap();
            namespace::trash(tx, OTHER, &foreign.id, &foreign.etag).unwrap();
            Ok(())
        })
        .await
        .unwrap();
    for (query, expected, more) in [("", 100, true), ("?limit=200", 200, true)] {
        let (status, body) = fixture
            .get_as(&format!("/trash{query}"), SESSION, SECRET)
            .await;
        assert_eq!(status, 200);
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(page["items"].as_array().unwrap().len(), expected);
        assert_eq!(page["has_more"], more);
    }
    let expected = fixture
        .workers
        .as_ref()
        .unwrap()
        .read(|db| {
            Ok(namespace::trashed(db, OWNER)
                .unwrap()
                .into_iter()
                .map(|row| row.entry.id)
                .collect::<Vec<_>>())
        })
        .await
        .unwrap();
    let mut ids = Vec::new();
    let mut cursor = None;
    loop {
        let query = cursor
            .as_ref()
            .map(|token| format!("&cursor={token}"))
            .unwrap_or_default();
        let (status, body) = fixture
            .get_as(&format!("/trash?limit=1{query}"), SESSION, SECRET)
            .await;
        assert_eq!(status, 200);
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let rows = page["items"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        ids.push(row["id"].as_str().unwrap().to_owned());
        if row["name"] == "same" {
            assert_eq!(
                row["needs_reference"], true,
                "duplicate exists on another page"
            );
        } else {
            assert!(row.get("needs_reference").is_none());
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        assert_eq!(page["has_more"], cursor.is_some());
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids, expected);
    assert_eq!(ids.len(), 209);
    fixture.close().await;
}

#[tokio::test]
async fn trash_cursors_reject_tampering_cross_account_and_invalid_limits() {
    let fixture = HttpFixture::new("trash-cursor-errors", 2).await;
    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            for entry in namespace::list(tx, OWNER, ROOT, None, 10).unwrap() {
                namespace::trash(tx, OWNER, &entry.id, &entry.etag).unwrap();
            }
            Ok(())
        })
        .await
        .unwrap();
    let (_, body) = fixture.get_as("/trash?limit=1", SESSION, SECRET).await;
    let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let cursor = page["next_cursor"].as_str().unwrap();
    for limit in [0, 201, usize::MAX] {
        let (status, body) = fixture
            .get_as(&format!("/trash?limit={limit}"), SESSION, SECRET)
            .await;
        assert_eq!(status, 400);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "invalid_limit"
        );
    }
    let mut tampered = cursor.as_bytes().to_vec();
    tampered[0] = if tampered[0] == b'A' { b'B' } else { b'A' };
    for invalid in [String::from_utf8(tampered).unwrap(), "A".repeat(4097)] {
        let (status, body) = fixture
            .get_as(&format!("/trash?cursor={invalid}"), SESSION, SECRET)
            .await;
        assert_eq!(status, 400);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "invalid_cursor"
        );
    }
    let (status, body) = fixture
        .get_as(
            &format!("/trash?cursor={cursor}"),
            OTHER_SESSION,
            OTHER_SECRET,
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
        "invalid_cursor"
    );
    fixture.close().await;
}

#[tokio::test]
async fn trash_cursor_tracks_owner_tree_changes_and_ignores_other_accounts() {
    let fixture = HttpFixture::new("trash-stale", 0).await;
    let (parent, destination, spare) = fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            let parent = namespace::create_folder(tx, OWNER, ROOT, "parent").unwrap();
            let destination = namespace::create_folder(tx, OWNER, ROOT, "destination").unwrap();
            let spare = namespace::create_folder(tx, OWNER, ROOT, "spare").unwrap();
            for name in ["a", "b", "c"] {
                let child = namespace::create_folder(tx, OWNER, &parent.id, name).unwrap();
                namespace::trash(tx, OWNER, &child.id, &child.etag).unwrap();
            }
            Ok((parent.id, destination.id, spare.id))
        })
        .await
        .unwrap();
    for operation in 0..6 {
        let (_, body) = fixture.get_as("/trash?limit=1", SESSION, SECRET).await;
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let cursor = page["next_cursor"].as_str().unwrap();
        let parent = parent.clone();
        let destination = destination.clone();
        let spare = spare.clone();
        fixture.workers.as_ref().unwrap().transact(move |tx| {
            let tag = |id: &str| {
                let generation: i64 = tx.query_row("SELECT generation FROM entry WHERE entry_id=?1", [id], |r| r.get(0)).unwrap();
                sylux::domain::upload::folder_etag(id, generation as u64)
            };
            match operation {
                0 => { namespace::create_folder(tx, OTHER, OTHER_ROOT, "other change").unwrap(); }
                1 => namespace::rename(tx, OWNER, &parent, "renamed", &tag(&parent)).map(|_| ()).unwrap(),
                2 => namespace::move_entry(tx, OWNER, &parent, &destination, &tag(&parent)).map(|_| ()).unwrap(),
                3 => namespace::trash(tx, OWNER, &spare, &tag(&spare)).unwrap(),
                4 => namespace::restore(tx, OWNER, &spare, &tag(&spare)).unwrap(),
                5 => {
                    // Upload replacement preserves the previous entry as a deleted row.
                    insert_sized_file(tx, "00000000000000000000000000007001", "replaced", 1);
                    tx.execute("UPDATE entry SET state='deleted',generation=generation+1 WHERE entry_id='00000000000000000000000000007001'", [])?;
                    insert_sized_file(tx, "00000000000000000000000000007002", "replaced", 2);
                }
                _ => unreachable!(),
            }
            Ok(())
        }).await.unwrap();
        let (status, body) = fixture
            .get_as(&format!("/trash?limit=1&cursor={cursor}"), SESSION, SECRET)
            .await;
        if operation == 0 {
            assert_eq!(status, 200, "foreign namespace change leaves cursor valid");
        } else {
            assert_eq!(status, 409, "operation {operation}");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
                "listing_changed"
            );
        }
    }
    fixture.close().await;
}

#[test]
fn trash_page_resolves_only_returned_paths_and_preserves_hidden_ancestors() {
    let db = namespace_db();
    db.pragma_update(None, "foreign_keys", false).unwrap();
    for limit in [0, 201, usize::MAX] {
        assert!(matches!(
            namespace::trashed_page(&db, OWNER, None, limit),
            Err(NamespaceError::InvalidLimit)
        ));
    }
    let tx = db.unchecked_transaction().unwrap();
    let parent = namespace::create_folder(&tx, OWNER, ROOT, "parent").unwrap();
    let first = namespace::create_folder(&tx, OWNER, &parent.id, "a").unwrap();
    let second = namespace::create_folder(&tx, OWNER, ROOT, "z").unwrap();
    namespace::trash(&tx, OWNER, &first.id, &first.etag).unwrap();
    namespace::trash(&tx, OWNER, &second.id, &second.etag).unwrap();
    let generation: i64 = tx
        .query_row(
            "SELECT generation FROM entry WHERE entry_id=?1",
            [&parent.id],
            |r| r.get(0),
        )
        .unwrap();
    namespace::trash(
        &tx,
        OWNER,
        &parent.id,
        &sylux::domain::upload::folder_etag(&parent.id, generation as u64),
    )
    .unwrap();
    let page = namespace::trashed_page(&tx, OWNER, None, 1).unwrap();
    assert!(page.has_more);
    assert_eq!(page.items[0].entry.id, first.id);
    assert_eq!(page.items[0].original_path, vec!["parent"]);
    tx.execute_batch("DROP TRIGGER entry_update_parent_active_folder;")
        .unwrap();
    tx.execute(
        "UPDATE entry SET parent_id='missing' WHERE entry_id=?1",
        [&second.id],
    )
    .unwrap();
    let page = namespace::trashed_page(
        &tx,
        OWNER,
        Some(&namespace::TrashPosition {
            name: "a".into(),
            id: first.id,
        }),
        1,
    )
    .unwrap();
    assert_eq!(page.items[0].entry.id, parent.id);
    assert!(page.has_more, "sentinel path is not resolved");
    assert!(namespace::trashed(&tx, OWNER).is_err());
}

#[tokio::test]
async fn trash_pages_disambiguate_historical_displayed_paths_across_parent_ids() {
    let fixture = HttpFixture::new("trash-historical-paths", 0).await;
    let expected = fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            let mut children = Vec::new();
            for parent_name in ["history", "history", "elsewhere"] {
                let parent = namespace::create_folder(tx, OWNER, ROOT, parent_name).unwrap();
                let nested = namespace::create_folder(tx, OWNER, &parent.id, "nested").unwrap();
                let child = namespace::create_folder(tx, OWNER, &nested.id, "same").unwrap();
                namespace::trash(tx, OWNER, &child.id, &child.etag).unwrap();
                let generation: i64 = tx.query_row(
                    "SELECT generation FROM entry WHERE entry_id=?1",
                    [&parent.id],
                    |row| row.get(0),
                )?;
                namespace::trash(
                    tx,
                    OWNER,
                    &parent.id,
                    &sylux::domain::upload::folder_etag(&parent.id, generation as u64),
                )
                .unwrap();
                children.push((child.id, nested.id, parent_name.to_owned()));
            }
            let foreign_parent =
                namespace::create_folder(tx, OTHER, OTHER_ROOT, "elsewhere").unwrap();
            let foreign_nested =
                namespace::create_folder(tx, OTHER, &foreign_parent.id, "nested").unwrap();
            let foreign_child =
                namespace::create_folder(tx, OTHER, &foreign_nested.id, "same").unwrap();
            namespace::trash(tx, OTHER, &foreign_child.id, &foreign_child.etag).unwrap();
            Ok(children)
        })
        .await
        .unwrap();
    assert_ne!(
        expected[0].1, expected[1].1,
        "historical parents have distinct IDs"
    );
    let mut cursor = None;
    let mut checked = 0;
    loop {
        let query = cursor
            .as_ref()
            .map(|token| format!("&cursor={token}"))
            .unwrap_or_default();
        let (status, body) = fixture
            .get_as(&format!("/trash?limit=1{query}"), SESSION, SECRET)
            .await;
        assert_eq!(status, 200);
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        let row = &page["items"][0];
        if let Some((_, parent_id, path)) =
            expected.iter().find(|(id, _, _)| row["id"] == id.as_str())
        {
            assert_eq!(row["parent_id"], parent_id.as_str());
            assert_eq!(row["original_path"], serde_json::json!([path, "nested"]));
            if path == "history" {
                assert_eq!(
                    row["needs_reference"], true,
                    "equal displayed paths on another page need a reference"
                );
            } else {
                assert!(
                    row.get("needs_reference").is_none(),
                    "distinct owner paths and foreign peers need no reference"
                );
            }
            checked += 1;
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(checked, 3);
    fixture.close().await;
}

#[derive(Clone, Debug)]
struct ExpectedResult {
    id: String,
    name: String,
    folder: bool,
    location: String,
    size: Option<i64>,
    modified: Option<i64>,
}

fn expected_result_order(rows: &[ExpectedResult], sort: &str, search: bool) -> Vec<String> {
    let mut rows = rows.to_vec();
    if search {
        for row in &mut rows {
            row.name = row.name.to_lowercase();
            row.location = row.location.to_lowercase();
        }
    }
    rows.sort_by(|a, b| {
        let names = || a.name.cmp(&b.name);
        let known_metric = |a: Option<i64>, b: Option<i64>, descending: bool| match (a, b) {
            (Some(a), Some(b)) if descending => b.cmp(&a),
            (Some(a), Some(b)) => a.cmp(&b),
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        };
        let order = match sort {
            "name_asc" => names(),
            "name_desc" => b.name.cmp(&a.name),
            "location_asc" => a.location.cmp(&b.location).then_with(names),
            "location_desc" => b.location.cmp(&a.location).then_with(names),
            _ => b.folder.cmp(&a.folder).then_with(|| {
                if a.folder {
                    return names();
                }
                match sort {
                    "size_asc" => known_metric(a.size, b.size, false),
                    "size_desc" => known_metric(a.size, b.size, true),
                    "modified_asc" => known_metric(a.modified, b.modified, false),
                    "modified_desc" => known_metric(a.modified, b.modified, true),
                    _ => unreachable!(),
                }
                .then_with(names)
            }),
        };
        order
            .then_with(|| {
                if search {
                    a.location.cmp(&b.location)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    rows.into_iter().map(|row| row.id).collect()
}

#[tokio::test]
async fn result_sorting_orders_complete_search_and_trash_before_bounded_pages() {
    let fixture = HttpFixture::new("result-sort-pages", 0).await;
    let expected = fixture.workers.as_ref().unwrap().transact(|tx| {
        let mut expected = Vec::new();
        let mut parents = Vec::new();
        for name in ["result-A", "result-Z"] {
            let entry = namespace::create_folder(tx, OWNER, ROOT, name).unwrap();
            expected.push(ExpectedResult { id: entry.id.clone(), name: name.to_owned(), folder: true, location: String::new(), size: None, modified: None });
            parents.push(entry.id);
        }
        for n in (0..113).rev() {
            let id = format!("{:032x}", 0xd000+n);
            let name = format!("result-{n:03}");
            let size = (n % 5) as i64;
            let modified = (n % 7) as i64;
            insert_sized_file_at(tx, &id, &name, size, modified);
            let location = if n % 3 == 0 {
                tx.execute("UPDATE entry SET parent_id=?1 WHERE entry_id=?2", params![parents[n % 2],id]).unwrap();
                if n % 2 == 0 { "result-A" } else { "result-Z" }
            } else { "" };
            expected.push(ExpectedResult { id, name, folder: false, location: location.to_owned(), size: Some(size), modified: Some(modified) });
        }
        for (n, parent) in parents.iter().enumerate() {
            let id = format!("{:032x}", 0xe000+n);
            tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'file','result-unknown',?4,1)", params![id,OWNER,parent,b"result-unknown"]).unwrap();
            expected.push(ExpectedResult { id, name: "result-unknown".to_owned(), folder: false, location: if n == 0 { "result-A" } else { "result-Z" }.to_owned(), size: None, modified: None });
        }
        namespace::create_folder(tx, OTHER, OTHER_ROOT, "result-private").unwrap();
        Ok(expected)
    }).await.unwrap();
    let sorts = [
        "name_asc",
        "name_desc",
        "size_asc",
        "size_desc",
        "modified_asc",
        "modified_desc",
        "location_asc",
        "location_desc",
    ];
    for search in [true, false] {
        if !search {
            let to_trash = expected.clone();
            fixture
                .workers
                .as_ref()
                .unwrap()
                .transact(move |tx| {
                    for row in to_trash
                        .iter()
                        .filter(|row| !row.folder)
                        .chain(to_trash.iter().filter(|row| row.folder))
                    {
                        let generation: i64 = tx
                            .query_row(
                                "SELECT generation FROM entry WHERE entry_id=?1",
                                [&row.id],
                                |r| r.get(0),
                            )
                            .unwrap();
                        namespace::trash(
                            tx,
                            OWNER,
                            &row.id,
                            &sylux::domain::upload::folder_etag(&row.id, generation as u64),
                        )
                        .unwrap();
                    }
                    Ok(())
                })
                .await
                .unwrap();
        }
        for sort in sorts {
            for limit in [7, 100] {
                let mut cursor = None;
                let mut ids = Vec::new();
                loop {
                    let suffix = cursor
                        .as_ref()
                        .map(|value| format!("&cursor={value}"))
                        .unwrap_or_default();
                    let path = if search {
                        format!("/entries/search?q=result&sort={sort}&limit={limit}{suffix}")
                    } else {
                        format!("/trash?sort={sort}&limit={limit}{suffix}")
                    };
                    let (status, body) = fixture.get_as(&path, SESSION, SECRET).await;
                    assert_eq!(status, 200, "{path}: {}", String::from_utf8_lossy(&body));
                    let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let items = page["items"].as_array().unwrap();
                    assert!(!items.is_empty() && items.len() <= limit);
                    for item in items {
                        let entry = if search { &item["entry"] } else { item };
                        let id = entry["id"].as_str().unwrap();
                        let row = expected
                            .iter()
                            .find(|row| row.id == id)
                            .expect("owned result");
                        assert_eq!(entry["size"].as_i64(), row.size);
                        assert_eq!(entry["modified_at"].as_i64(), row.modified);
                        ids.push(id.to_owned());
                    }
                    cursor = page["next_cursor"].as_str().map(str::to_owned);
                    assert_eq!(page["has_more"], cursor.is_some());
                    if cursor.is_none() {
                        break;
                    }
                    assert!(ids.len() < expected.len(), "paging terminates");
                }
                assert_eq!(
                    ids,
                    expected_result_order(&expected, sort, search),
                    "search={search} sort={sort} limit={limit}"
                );
            }
        }
    }
    fixture.close().await;
}

#[tokio::test]
async fn result_sort_cursors_bind_order_and_revision_metadata() {
    let fixture = HttpFixture::new("result-sort-cursors", 0).await;
    fixture
        .workers
        .as_ref()
        .unwrap()
        .transact(|tx| {
            for n in 0..3 {
                insert_sized_file_at(
                    tx,
                    &format!("{:032x}", 0xf000 + n),
                    &format!("result-{n}"),
                    n as i64,
                    n as i64,
                );
            }
            Ok(())
        })
        .await
        .unwrap();
    let sorts = [
        "name_asc",
        "name_desc",
        "size_asc",
        "size_desc",
        "modified_asc",
        "modified_desc",
        "location_asc",
        "location_desc",
    ];
    for search in [true, false] {
        if !search {
            fixture
                .workers
                .as_ref()
                .unwrap()
                .transact(|tx| {
                    for entry in namespace::list(tx, OWNER, ROOT, None, 10).unwrap() {
                        namespace::trash(tx, OWNER, &entry.id, &entry.etag).unwrap();
                    }
                    Ok(())
                })
                .await
                .unwrap();
        }
        let base = if search {
            "/entries/search?q=result&"
        } else {
            "/trash?"
        };
        for invalid in ["", "NAME_ASC", "modified", "random"] {
            let (status, body) = fixture
                .get_as(&format!("{base}sort={invalid}"), SESSION, SECRET)
                .await;
            assert_eq!(status, 400);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
                "invalid_sort"
            );
        }
        for sort in sorts {
            let (_, body) = fixture
                .get_as(&format!("{base}sort={sort}&limit=1"), SESSION, SECRET)
                .await;
            let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let cursor = page["next_cursor"].as_str().unwrap();
            for other in sorts.into_iter().filter(|other| *other != sort) {
                let (status, body) = fixture
                    .get_as(
                        &format!("{base}sort={other}&cursor={cursor}"),
                        SESSION,
                        SECRET,
                    )
                    .await;
                assert_eq!(status, 400, "search={search}: {sort} -> {other}");
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
                    "invalid_cursor"
                );
            }
        }
        for (index, column) in ["length", "committed_at"].into_iter().enumerate() {
            let (_, body) = fixture
                .get_as(
                    &format!("{base}sort=modified_desc&limit=1"),
                    SESSION,
                    SECRET,
                )
                .await;
            let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let cursor = page["next_cursor"].as_str().unwrap();
            fixture.workers.as_ref().unwrap().transact(move |tx| {
                let revision = format!("{:032x}", 0xf100 + index + if search { 0 } else { 2 });
                let length = if column == "length" { "length+1" } else { "length" };
                let committed = if column == "committed_at" { "committed_at+1" } else { "committed_at" };
                tx.execute(&format!("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) SELECT ?1,entry_id,blob_id,{length},sha256,{committed},actor_account_id FROM revision WHERE revision_id=(SELECT current_revision_id FROM entry WHERE entry_id='0000000000000000000000000000f000')"), [&revision]).unwrap();
                tx.execute("UPDATE entry SET current_revision_id=?1 WHERE entry_id='0000000000000000000000000000f000'", [&revision]).unwrap();
                Ok(())
            }).await.unwrap();
            let (status, body) = fixture
                .get_as(
                    &format!("{base}sort=modified_desc&cursor={cursor}"),
                    SESSION,
                    SECRET,
                )
                .await;
            assert_eq!(status, 409, "search={search}: {column}");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
                "listing_changed"
            );
        }
    }
    fixture.close().await;
}
