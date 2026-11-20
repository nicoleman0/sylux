//! Copy through authenticated product routes, real TLS, SQLite and payload storage.
#[path = "../common/mod.rs"]
mod common;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{
    FixtureRoot,
    server::{TlsApiServer, TlsFiles, tls_connect},
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use sylux::{
    domain::{authz::verifier, upload::folder_etag},
    http::{self, AuthState},
    persistence::{
        cleanup::Runner,
        payload::PayloadStore,
        reconcile, root,
        sqlite::{self, SqliteWorkers, WorkerConfig},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

const BUSY: Duration = Duration::from_millis(375);
const DEADLINE: Duration = Duration::from_secs(5);
const OWNER: &str = "00000000000000000000000000000001";
const OTHER: &str = "00000000000000000000000000000002";
const OWNER_ROOT: &str = "00000000000000000000000000000101";
const OTHER_ROOT: &str = "00000000000000000000000000000102";
const BYTES: &[u8] = b"SYL-81 current bytes\x00\xff";

struct Actor {
    account: &'static str,
    root: &'static str,
    session: &'static str,
    handle: &'static str,
    secret: [u8; 32],
    csrf: [u8; 32],
}

const ALICE: Actor = Actor {
    account: OWNER,
    root: OWNER_ROOT,
    session: "00000000000000000000000000000201",
    handle: "syl-81-owner",
    secret: [1; 32],
    csrf: [2; 32],
};
const BOB: Actor = Actor {
    account: OTHER,
    root: OTHER_ROOT,
    session: "00000000000000000000000000000202",
    handle: "syl-81-other",
    secret: [3; 32],
    csrf: [4; 32],
};

struct Fixture {
    root: FixtureRoot,
    product: PathBuf,
    workers: SqliteWorkers,
    server: TlsApiServer,
    cleanup: Runner,
    ca: Vec<u8>,
}

type CommitHook = Arc<dyn Fn() -> rusqlite::Result<()> + Send + Sync>;

impl Fixture {
    async fn new(case: &str, hook: Option<CommitHook>) -> Self {
        let parent = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("SYL-81-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let root = FixtureRoot::create(&parent, "SYL-81", case).unwrap();
        let tls = TlsFiles::generate(&root, "tls").unwrap();
        let ca = fs::read(&tls.ca_der).unwrap();
        let product = root.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).unwrap();
        let path = product.join("metadata/sylux.sqlite3");
        let db = sqlite::open_writer(&path, BUSY).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        for actor in [ALICE, BOB] {
            db.execute(
                "INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,?2,'user',1,0)",
                params![actor.account, actor.handle],
            )
            .unwrap();
            db.execute("INSERT INTO entry(entry_id,owner_account_id,kind,original_name,name_key,name_key_version)
                VALUES(?1,?2,'folder','',x'',1)", params![actor.root, actor.account]).unwrap();
            db.execute("INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at)
                VALUES(?1,?2,0,?3,?4,?5,?6,?7)",
                params![actor.session,actor.account,verifier(&actor.secret).as_slice(),verifier(&actor.csrf).as_slice(),now,now+3600,now+7200]).unwrap();
        }
        let workers = SqliteWorkers::from_writer(
            db,
            &path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let mut state = AuthState::new(workers.clone(), "https://127.0.0.1".into())
            .with_storage_root(&product)
            .unwrap();
        state.copy_commit_hook = hook;
        let cleanup = state.start_cleanup().unwrap();
        let server = TlsApiServer::start(&root, &tls, http::router(state), DEADLINE)
            .await
            .unwrap();
        Self {
            root,
            product,
            workers,
            server,
            cleanup,
            ca,
        }
    }

    fn db(&self) -> Connection {
        sqlite::open_writer(&self.product.join("metadata/sylux.sqlite3"), BUSY).unwrap()
    }

    fn seed_file(&self, parent: &str, history: bool) -> (String, PathBuf) {
        let mut db = self.db();
        let tx = db.transaction().unwrap();
        let file = format!("{:032x}", 301);
        let name = "SYL-81 résumé.bin";
        let key = sylux::domain::names::EntryName::parse(name).unwrap();
        tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'file',?4,?5,1)",
            params![file, OWNER,parent,key.display,key.key]).unwrap();
        let store = PayloadStore::open(&self.product).unwrap();
        let mut payload = PathBuf::new();
        for (index, bytes) in [(1, b"SYL-81 historical".as_slice()), (2, BYTES)] {
            if index == 1 && !history {
                continue;
            }
            let blob = format!("{:032x}", 401 + index);
            let attempt = format!("{:032x}", 501 + index);
            let revision = format!("{:032x}", 601 + index);
            let digest: [u8; 32] = Sha256::digest(bytes).into();
            let mut staging = store
                .start(&format!("{attempt}-{blob}"), bytes.len() as u64)
                .unwrap();
            staging.write_chunk(bytes).unwrap();
            assert_eq!(staging.publish(digest).unwrap().blob_id, blob);
            tx.execute(
                "INSERT INTO blob(blob_id,shard,length,sha256) VALUES(?1,?2,?3,?4)",
                params![blob, &blob[..2], bytes.len() as i64, digest.as_slice()],
            )
            .unwrap();
            tx.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id)
                VALUES(?1,?2,?3,?4,?5,0,?6)", params![revision,file,blob,bytes.len() as i64,digest.as_slice(),OWNER]).unwrap();
            tx.execute(
                "UPDATE entry SET current_revision_id=?2 WHERE entry_id=?1",
                params![file, revision],
            )
            .unwrap();
            payload = self.product.join("payloads").join(&blob[..2]).join(blob);
        }
        tx.commit().unwrap();
        (file, payload)
    }

    fn request_text(method: &str, path: &str, actor: &Actor, etag: &str, body: &str) -> String {
        format!(
            "{} {} HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://127.0.0.1\r\nCookie: __Host-sylux={}.{}\r\nX-CSRF-Token: {}\r\nIf-Match: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            method,
            path,
            actor.session,
            URL_SAFE_NO_PAD.encode(actor.secret),
            URL_SAFE_NO_PAD.encode(actor.csrf),
            etag,
            body.len(),
            body
        )
    }

    async fn request(&self, request: String) -> (u16, Vec<u8>) {
        let mut stream = tls_connect(self.server.address(), &self.ca, "127.0.0.1")
            .await
            .unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        timeout(DEADLINE, stream.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        let end = response
            .windows(4)
            .position(|chunk| chunk == b"\r\n\r\n")
            .unwrap();
        let status = std::str::from_utf8(&response[..end])
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (status, response[end + 4..].to_vec())
    }

    async fn get(&self, actor: &Actor, path: &str) -> (u16, Vec<u8>) {
        self.request(Self::request_text("GET", path, actor, "", ""))
            .await
    }

    async fn json(
        &self,
        method: &str,
        path: &str,
        actor: &Actor,
        etag: &str,
        body: Value,
        status: u16,
    ) -> Value {
        let body = if body.is_null() {
            String::new()
        } else {
            body.to_string()
        };
        let (actual, response) = self
            .request(Self::request_text(method, path, actor, etag, &body))
            .await;
        assert_eq!(
            actual,
            status,
            "{path}: {}",
            String::from_utf8_lossy(&response)
        );
        if response.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&response).unwrap()
        }
    }

    async fn folder(&self, parent: &str, name: &str) -> Value {
        self.json(
            "POST",
            "/folders/children",
            &ALICE,
            "",
            json!({"parent_id":parent,"name":name}),
            201,
        )
        .await
    }

    async fn items(&self, folder: &str) -> Vec<Value> {
        let (status, response) = self
            .get(&ALICE, &format!("/folders/{folder}/children"))
            .await;
        assert_eq!(status, 200);
        serde_json::from_slice::<Value>(&response).unwrap()["items"]
            .as_array()
            .unwrap()
            .clone()
    }

    async fn ticket(&self, actor: &Actor, source: &str, parent: &str, tag: &str) -> Value {
        self.json(
            "POST",
            &format!("/entries/{source}/copy-ticket"),
            actor,
            tag,
            json!({"parent_id":parent}),
            200,
        )
        .await
    }

    async fn commit(&self, ticket: &Value) {
        assert_eq!(
            self.json("POST", "/copies", &ALICE, "", ticket.clone(), 200)
                .await["state"],
            "committed"
        );
    }

    async fn assert_content(&self, file: &str, status: u16, bytes: &[u8]) {
        let (actual, response) = self.get(&ALICE, &format!("/entries/{file}/content")).await;
        assert_eq!(actual, status);
        if status == 200 {
            assert_eq!(response, bytes);
        }
    }

    async fn trash(&self, value: &Value) {
        self.json(
            "DELETE",
            &format!("/entries/{}", id(value)),
            &ALICE,
            etag(value),
            Value::Null,
            204,
        )
        .await;
    }

    async fn permanently_remove(&self, file: &str, operation: &str) {
        let (status, body) = self.get(&ALICE, "/trash").await;
        assert_eq!(status, 200);
        let listing: Value = serde_json::from_slice(&body).unwrap();
        let item = listing["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == file)
            .unwrap();
        let preview = self
            .json(
                "POST",
                "/trash/removals/preview",
                &ALICE,
                "",
                json!({"selection":{"kind":"selected","items":[{"id":file,"etag":etag(item)}]}}),
                200,
            )
            .await;
        let result = self
            .json(
                "POST",
                "/trash/removals",
                &ALICE,
                "",
                json!({"operation_id":operation,"confirmation":preview["confirmation"]}),
                200,
            )
            .await;
        assert_eq!(result["state"], "committed");
        timeout(DEADLINE, async {
            loop {
                let (status, body) = self
                    .get(&ALICE, &format!("/trash/removals/{operation}"))
                    .await;
                assert_eq!(status, 200);
                if serde_json::from_slice::<Value>(&body).unwrap()["cleanup"] == "complete" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("reclamation should finish in this serving session");
    }

    async fn close(mut self) {
        self.server.shutdown_and_join().await.unwrap();
        self.cleanup.shutdown().await.unwrap();
        self.workers.shutdown().await.unwrap();
        self.root.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.root.path().parent().unwrap()).unwrap();
    }
}

fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}
fn etag(value: &Value) -> &str {
    value["etag"].as_str().unwrap()
}

#[tokio::test]
async fn oversized_copy_returns_422_without_changing_destination() {
    let fixture = Fixture::new("too-large", None).await;
    let source = fixture.folder(OWNER_ROOT, "Large source").await;
    let destination = fixture.folder(OWNER_ROOT, "Destination").await;
    {
        let mut db = fixture.db();
        let tx = db.transaction().unwrap();
        for i in 0..10_000 {
            let child_id = format!("{:032x}", 0x100000 + i);
            let name = format!("child-{i:05}");
            let key = sylux::domain::names::EntryName::parse(&name).unwrap();
            tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder',?4,?5,1)", params![child_id, OWNER, id(&source), key.display, key.key]).unwrap();
        }
        tx.commit().unwrap();
    }
    let ticket = fixture
        .ticket(&ALICE, id(&source), id(&destination), etag(&source))
        .await;
    let response = fixture
        .json("POST", "/copies", &ALICE, "", ticket, 422)
        .await;
    assert_eq!(response["code"], "copy_too_large");
    assert!(fixture.items(id(&destination)).await.is_empty());
    let current = fixture
        .items(OWNER_ROOT)
        .await
        .into_iter()
        .find(|entry| id(entry) == id(&destination))
        .unwrap();
    assert_eq!(etag(&destination), etag(&current));
    fixture.close().await;
}

#[tokio::test]
async fn tree_copy_preserves_live_structure_names_bytes_and_only_the_current_revision() {
    let fixture = Fixture::new("live-tree", None).await;
    let source = fixture.folder(OWNER_ROOT, "SYL-81 résumé").await;
    let nested = fixture.folder(id(&source), "Nested Ω").await;
    let empty = fixture.folder(id(&source), "Empty").await;
    let hidden = fixture.folder(id(&nested), "Already trashed").await;
    fixture.trash(&hidden).await;
    let (file, _) = fixture.seed_file(id(&nested), true);
    let destination = fixture.folder(OWNER_ROOT, "Destination").await;
    let source_tag = folder_etag(id(&source), 3);
    let ticket = fixture
        .ticket(&ALICE, id(&source), id(&destination), &source_tag)
        .await;
    fixture.commit(&ticket).await;

    let updated_destination = fixture
        .items(OWNER_ROOT)
        .await
        .into_iter()
        .find(|entry| id(entry) == id(&destination))
        .unwrap();
    assert_ne!(etag(&destination), etag(&updated_destination));
    let copy = fixture.items(id(&destination)).await.pop().unwrap();
    assert_eq!(copy["name"], source["name"]);
    assert_ne!(id(&copy), id(&source));
    let children = fixture.items(id(&copy)).await;
    assert_eq!(
        children
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Empty", "Nested Ω"]
    );
    assert_ne!(id(&children[0]), id(&empty));
    assert!(fixture.items(id(&children[0])).await.is_empty());
    assert_ne!(id(&children[1]), id(&nested));
    let nested_files = fixture.items(id(&children[1])).await;
    assert_eq!(nested_files.len(), 1);
    let copied_file = &nested_files[0];
    assert_eq!(copied_file["name"], "SYL-81 résumé.bin");
    assert_ne!(id(copied_file), file);
    fixture.assert_content(id(copied_file), 200, BYTES).await;
    let revisions: Vec<String> = {
        let db = fixture.db();
        db.prepare("SELECT blob_id FROM revision WHERE entry_id=?1")
            .unwrap()
            .query_map([id(copied_file)], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert_eq!(revisions, [format!("{:032x}", 403)]);
    let renamed = fixture
        .json(
            "PATCH",
            &format!("/entries/{}", id(copied_file)),
            &ALICE,
            etag(copied_file),
            json!({"name":"Renamed copy.bin"}),
            200,
        )
        .await;
    assert_eq!(
        fixture.items(id(&nested)).await[0]["name"],
        "SYL-81 résumé.bin"
    );
    fixture.trash(&renamed).await;
    fixture.assert_content(id(copied_file), 404, b"").await;
    fixture.assert_content(&file, 200, BYTES).await;
    fixture.commit(&ticket).await;
    assert_eq!(fixture.items(id(&destination)).await.len(), 1);
    assert!(
        reconcile::reconcile(&fixture.workers, &fixture.product)
            .await
            .unwrap()
            .ready
    );
    fixture.close().await;
}

#[tokio::test]
async fn copy_rejects_conflicts_cycles_stale_and_private_or_trashed_entries() {
    let fixture = Fixture::new("invalid-destinations", None).await;
    let source = fixture.folder(OWNER_ROOT, "SYL-81 café").await;
    let nested = fixture.folder(id(&source), "Child").await;
    let destination = fixture.folder(OWNER_ROOT, "Destination").await;
    let existing = fixture.folder(id(&destination), "SYL-81 cafe\u{301}").await;
    let tag = folder_etag(id(&source), 2);
    let before: i64 = fixture
        .db()
        .query_row("SELECT count(*) FROM entry", [], |row| row.get(0))
        .unwrap();
    for (actor, source_id, parent, tag, status, code) in [
        (
            &ALICE,
            id(&source),
            id(&destination),
            tag.as_str(),
            409,
            "name_conflict",
        ),
        (
            &ALICE,
            id(&source),
            OWNER_ROOT,
            tag.as_str(),
            409,
            "name_conflict",
        ),
        (
            &ALICE,
            id(&source),
            id(&source),
            tag.as_str(),
            409,
            "invalid_destination",
        ),
        (
            &ALICE,
            id(&source),
            id(&nested),
            tag.as_str(),
            409,
            "invalid_destination",
        ),
        (
            &ALICE,
            id(&source),
            id(&destination),
            etag(&source),
            412,
            "stale_etag",
        ),
        (
            &ALICE,
            id(&source),
            OTHER_ROOT,
            tag.as_str(),
            404,
            "not_found",
        ),
        (
            &BOB,
            id(&source),
            OTHER_ROOT,
            tag.as_str(),
            404,
            "not_found",
        ),
    ] {
        let ticket = fixture.ticket(actor, source_id, parent, tag).await;
        assert_eq!(
            fixture
                .json("POST", "/copies", actor, "", ticket.clone(), status)
                .await["code"],
            code
        );
        assert_eq!(
            fixture
                .json("POST", "/copies/status", actor, "", ticket, 200)
                .await["state"],
            "unknown"
        );
    }
    let owner_ticket = fixture
        .ticket(&ALICE, id(&source), id(&destination), &tag)
        .await;
    let tampered = json!({"ticket":format!("A{}", owner_ticket["ticket"].as_str().unwrap())});
    for path in ["/copies", "/copies/status"] {
        assert_eq!(
            fixture
                .json("POST", path, &BOB, "", owner_ticket.clone(), 403)
                .await["code"],
            "copy_other_account"
        );
        assert_eq!(
            fixture
                .json("POST", path, &ALICE, "", tampered.clone(), 400)
                .await["code"],
            "invalid_copy_ticket"
        );
    }
    assert_eq!(
        fixture.items(id(&destination)).await[0]["id"],
        id(&existing)
    );
    assert!(fixture.items(id(&existing)).await.is_empty());
    assert_eq!(
        fixture
            .db()
            .query_row("SELECT count(*) FROM entry", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        before
    );
    let current_source = fixture
        .items(OWNER_ROOT)
        .await
        .into_iter()
        .find(|entry| id(entry) == id(&source))
        .unwrap();
    fixture.trash(&current_source).await;
    for (source, dest) in [
        (id(&source), OWNER_ROOT),
        (id(&nested), OWNER_ROOT),
        (id(&destination), id(&nested)),
    ] {
        let ticket = fixture
            .ticket(&ALICE, source, dest, &folder_etag(source, 2))
            .await;
        assert_eq!(
            fixture
                .json("POST", "/copies", &ALICE, "", ticket, 404)
                .await["code"],
            "not_found"
        );
    }
    fixture.close().await;
}

#[tokio::test]
async fn copy_rolls_back_and_a_lost_tls_reply_reconciles_the_original_even_after_source_changes() {
    let (reached, reached_receiver) = tokio::sync::oneshot::channel();
    let reached = Mutex::new(Some(reached));
    let (release, pause) = std::sync::mpsc::channel();
    let pause = Mutex::new(pause);
    let hook = {
        let first = std::sync::atomic::AtomicBool::new(true);
        Arc::new(move || {
            if first.swap(false, std::sync::atomic::Ordering::Relaxed) {
                return Err(rusqlite::Error::InvalidQuery);
            }
            if let Some(reached) = reached.lock().unwrap().take() {
                reached.send(()).unwrap();
                pause.lock().unwrap().recv_timeout(DEADLINE).unwrap();
            }
            Ok(())
        })
    };
    let fixture = Fixture::new("lost-reply", Some(hook)).await;
    let (file, _) = fixture.seed_file(OWNER_ROOT, false);
    let dest = fixture.folder(OWNER_ROOT, "Destination").await;
    let ticket = fixture
        .ticket(&ALICE, &file, id(&dest), &folder_etag(&file, 1))
        .await;
    assert_eq!(
        fixture
            .json("POST", "/copies", &ALICE, "", ticket.clone(), 409)
            .await["code"],
        "copy_not_committed"
    );
    assert!(fixture.items(id(&dest)).await.is_empty());
    assert_eq!(
        fixture
            .db()
            .query_row("SELECT count(*) FROM revision", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        fixture
            .json("POST", "/copies/status", &ALICE, "", ticket.clone(), 200)
            .await["state"],
        "unknown"
    );

    let mut stream = tls_connect(fixture.server.address(), &fixture.ca, "127.0.0.1")
        .await
        .unwrap();
    stream
        .write_all(
            Fixture::request_text("POST", "/copies", &ALICE, "", &ticket.to_string()).as_bytes(),
        )
        .await
        .unwrap();
    timeout(DEADLINE, reached_receiver).await.unwrap().unwrap();
    assert_eq!(
        fixture
            .json("POST", "/copies/status", &ALICE, "", ticket.clone(), 200)
            .await["state"],
        "unknown"
    );
    drop(stream);
    release.send(()).unwrap();
    timeout(DEADLINE, async {
        loop {
            if fixture
                .json("POST", "/copies/status", &ALICE, "", ticket.clone(), 200)
                .await["state"]
                == "committed"
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the initial upload-free copy should commit after the client disconnects");
    let file_entry = fixture
        .items(OWNER_ROOT)
        .await
        .into_iter()
        .find(|entry| id(entry) == file)
        .unwrap();
    fixture.trash(&file_entry).await;
    fixture.commit(&ticket).await;
    let copies = fixture.items(id(&dest)).await;
    assert_eq!(copies.len(), 1);
    fixture.assert_content(id(&copies[0]), 200, BYTES).await;
    fixture.close().await;
}

async fn check_reclamation(copy_first: bool) {
    let fixture = Fixture::new(
        if copy_first {
            "copy-first"
        } else {
            "source-first"
        },
        None,
    )
    .await;
    let (source, payload) = fixture.seed_file(OWNER_ROOT, false);
    let dest = fixture.folder(OWNER_ROOT, "Destination").await;
    let ticket = fixture
        .ticket(&ALICE, &source, id(&dest), &folder_etag(&source, 1))
        .await;
    fixture.commit(&ticket).await;
    let original = fixture
        .items(OWNER_ROOT)
        .await
        .into_iter()
        .find(|item| id(item) == source)
        .unwrap();
    let copy = fixture.items(id(&dest)).await.pop().unwrap();
    let (first, second) = if copy_first {
        (&copy, &original)
    } else {
        (&original, &copy)
    };
    assert_eq!(
        fixture
            .get(&BOB, &format!("/entries/{}/content", id(&copy)))
            .await
            .0,
        404
    );

    fixture.trash(first).await;
    fixture
        .permanently_remove(id(first), "81000000000000000000000000000001")
        .await;
    assert!(payload.exists());
    fixture.assert_content(id(first), 404, b"").await;
    fixture.assert_content(id(second), 200, BYTES).await;

    fixture.trash(second).await;
    fixture
        .permanently_remove(id(second), "81000000000000000000000000000002")
        .await;
    assert!(!payload.exists());
    fixture.assert_content(id(second), 404, b"").await;
    fixture.commit(&ticket).await;
    assert!(fixture.items(id(&dest)).await.is_empty());
    assert!(
        reconcile::reconcile(&fixture.workers, &fixture.product)
            .await
            .unwrap()
            .ready
    );
    fixture.close().await;
}

#[tokio::test]
async fn permanent_removal_keeps_a_live_copy_then_reclaims_its_final_reference() {
    check_reclamation(false).await;
}

#[tokio::test]
async fn permanent_removal_keeps_the_original_when_the_copy_is_removed_first() {
    check_reclamation(true).await;
}
