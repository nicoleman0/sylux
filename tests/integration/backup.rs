//! Online backup through the production CLI and disposable HTTP storage routes.
#[path = "../common/mod.rs"]
mod common;

use common::{FixtureRoot, TestBarrier, sha256_hex, unique_token};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use sylux::{
    http::{self, AuthState},
    persistence::{
        instance::{AppearanceChange, AssetChange, InstanceSettings},
        ownership,
        payload::{PayloadError, PublishFaultHook},
        reconcile, root,
        sqlite::{self, DEFAULT_BUSY_TIMEOUT, SqliteWorkers, WorkerConfig},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
    time::{sleep, timeout},
};

// Keep concurrent CLI launches out of fixture initialization and flock checks.
static CLI_FIXTURES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Fixture {
    owner: FixtureRoot,
    live: PathBuf,
    backup: PathBuf,
    restored: PathBuf,
}

impl Fixture {
    fn new(case: &str) -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("syl-73-{}", unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "syl-73", case).unwrap();
        let live = owner.path().join("live");
        fs::DirBuilder::new().mode(0o700).create(&live).unwrap();
        root::initialize_empty_root(&live, DEFAULT_BUSY_TIMEOUT).unwrap();
        Self {
            backup: owner.path().join("backup"),
            restored: owner.path().join("restored"),
            owner,
            live,
        }
    }

    fn close(self) {
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }

    async fn backup_to(&self, target: &Path) -> Output {
        let (source, target) = (self.live.clone(), target.to_owned());
        tokio::task::spawn_blocking(move || {
            Command::new(env!("CARGO_BIN_EXE_sylux"))
                .args(["admin", "backup", "--root"])
                .arg(source)
                .arg("--destination")
                .arg(target)
                .env("SYLUX_LISTEN", "invalid")
                .output()
                .unwrap()
        })
        .await
        .unwrap()
    }
}

struct Server {
    address: std::net::SocketAddr,
    origin: String,
    workers: SqliteWorkers,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
    _ownership: ownership::OwnershipGuard,
}

struct Auth {
    cookie: String,
    csrf: String,
}

struct Upload {
    id: String,
    ticket: String,
    digest: String,
    parent_etag: String,
}

impl Server {
    async fn start(storage: &Path) -> Self {
        Self::start_with_publish_hook(storage, None).await
    }

    async fn start_with_publish_hook(storage: &Path, hook: Option<PublishFaultHook>) -> Self {
        let ownership = ownership::acquire_writer_lock(storage).unwrap();
        let db = root::open_bound_database(storage, DEFAULT_BUSY_TIMEOUT).unwrap();
        let manifest = root::read_manifest(storage).unwrap();
        root::verify_binding(&db, storage, &manifest).unwrap();
        let workers = SqliteWorkers::from_writer(
            db.connection,
            &storage.join(root::METADATA_DIR).join(root::DB_NAME),
            WorkerConfig::default(),
        )
        .unwrap();
        let report = reconcile::reconcile(&workers, storage).await.unwrap();
        assert!(report.ready, "{report:?}");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        let token = fs::read_to_string(storage.join("secrets/setup.authorization")).unwrap();
        let mut state = AuthState::new_with_setup(workers.clone(), origin.clone(), token)
            .with_storage_root(storage)
            .unwrap();
        if let Some(hook) = hook {
            let storage = Arc::get_mut(state.storage.as_mut().unwrap()).unwrap();
            Arc::get_mut(&mut storage.payloads)
                .unwrap()
                .set_publish_fault_hook(Some(hook));
        }
        let app = http::router(state);
        let (stop, signal) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        });
        Self {
            address,
            origin,
            workers,
            stop,
            task,
            _ownership: ownership,
        }
    }

    async fn close(self) {
        self.stop.send(()).unwrap();
        timeout(Duration::from_secs(5), self.task)
            .await
            .unwrap()
            .unwrap();
        self.workers.shutdown().await.unwrap();
    }

    async fn send(
        &self,
        method: &str,
        route: &str,
        auth: Option<&Auth>,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Response {
        let mut stream = self.begin(method, route, auth, headers, body.len()).await;
        stream.write_all(body).await.unwrap();
        response(stream).await
    }

    async fn begin(
        &self,
        method: &str,
        route: &str,
        auth: Option<&Auth>,
        headers: &[(&str, &str)],
        length: usize,
    ) -> TcpStream {
        use std::fmt::Write as _;
        let mut head = format!(
            "{method} {route} HTTP/1.1\r\nHost: {}\r\nOrigin: {}\r\nConnection: close\r\nContent-Length: {length}\r\n",
            self.address, self.origin
        );
        if let Some(auth) = auth {
            write!(
                head,
                "Cookie: {}\r\nX-CSRF-Token: {}\r\n",
                auth.cookie, auth.csrf
            )
            .unwrap();
        }
        for (name, value) in headers {
            write!(head, "{name}: {value}\r\n").unwrap();
        }
        let mut stream = TcpStream::connect(self.address).await.unwrap();
        stream
            .write_all(format!("{head}\r\n").as_bytes())
            .await
            .unwrap();
        stream
    }

    async fn setup(&self, root: &Path) {
        let token = fs::read_to_string(root.join("secrets/setup.authorization")).unwrap();
        self.send(
            "POST",
            "/setup/admin",
            None,
            &[
                ("Content-Type", "application/json"),
                ("X-Setup-Authorization", &token),
            ],
            br#"{"handle":"owner","password":"synthetic account passphrase"}"#,
        )
        .await
        .expect(201);
    }

    async fn login(&self) -> Auth {
        let response = self
            .send("GET", "/auth/csrf", None, &[], &[])
            .await
            .expect(200);
        let anonymous = Auth {
            cookie: response.cookie(),
            csrf: response.json()["csrf_token"].as_str().unwrap().into(),
        };
        let response = self
            .send(
                "POST",
                "/auth/local",
                Some(&anonymous),
                &[("Content-Type", "application/json")],
                br#"{"handle":"owner","password":"synthetic account passphrase"}"#,
            )
            .await
            .expect(200);
        Auth {
            cookie: response.cookie(),
            csrf: response.json()["csrf_token"].as_str().unwrap().into(),
        }
    }

    async fn listing(&self, auth: &Auth) -> Value {
        self.send("GET", "/folders/children", Some(auth), &[], &[])
            .await
            .expect(200)
            .json()
    }

    async fn admit(&self, auth: &Auth, name: &str, bytes: &[u8]) -> Upload {
        let root = &self.listing(auth).await["root"];
        let digest = sha256_hex(bytes);
        let mut input = json!({
            "scope":"upload_create", "parent_id":root["id"], "parent_etag":root["etag"],
            "name":name, "length":bytes.len(), "sha256":digest,
        });
        let ticket = self
            .send(
                "POST",
                "/mutation-tickets",
                Some(auth),
                &[("Content-Type", "application/json")],
                &serde_json::to_vec(&input).unwrap(),
            )
            .await
            .expect(201)
            .header("x-mutation-id")
            .to_owned();
        input.as_object_mut().unwrap().remove("scope");
        let admitted = self
            .send(
                "POST",
                "/uploads",
                Some(auth),
                &[
                    ("Content-Type", "application/json"),
                    ("X-Mutation-ID", &ticket),
                    ("If-None-Match", "*"),
                ],
                &serde_json::to_vec(&input).unwrap(),
            )
            .await
            .expect(201)
            .json();
        Upload {
            id: admitted["upload_id"].as_str().unwrap().into(),
            ticket,
            digest,
            parent_etag: root["etag"].as_str().unwrap().into(),
        }
    }

    async fn upload(&self, auth: &Auth, name: &str, bytes: &[u8]) -> String {
        let upload = self.admit(auth, name, bytes).await;
        self.put(auth, &upload, bytes).await;
        self.commit(auth, &upload).await
    }

    async fn put(&self, auth: &Auth, upload: &Upload, bytes: &[u8]) {
        self.send(
            "PUT",
            &format!("/uploads/{}/content", upload.id),
            Some(auth),
            &[
                ("Content-Type", "application/octet-stream"),
                ("X-Mutation-ID", &upload.ticket),
            ],
            bytes,
        )
        .await
        .expect(200);
    }

    async fn commit(&self, auth: &Auth, upload: &Upload) -> String {
        let result = self
            .send(
                "POST",
                &format!("/uploads/{}/commit", upload.id),
                Some(auth),
                &[
                    ("Content-Type", "application/json"),
                    ("X-Mutation-ID", &upload.ticket),
                    ("If-None-Match", "*"),
                ],
                &serde_json::to_vec(
                    &json!({"sha256":upload.digest,"parent_etag":upload.parent_etag}),
                )
                .unwrap(),
            )
            .await
            .expect(201)
            .json();
        result["id"].as_str().unwrap().into()
    }

    async fn download(&self, auth: &Auth, id: &str) -> Vec<u8> {
        self.send(
            "GET",
            &format!("/entries/{id}/content"),
            Some(auth),
            &[],
            &[],
        )
        .await
        .expect(200)
        .body
    }

    async fn status(&self, auth: &Auth, upload: &Upload) -> Value {
        self.send(
            "GET",
            &format!("/uploads/{}", upload.id),
            Some(auth),
            &[],
            &[],
        )
        .await
        .expect(200)
        .json()
    }

    async fn begin_patch(
        &self,
        auth: &Auth,
        upload: &Upload,
        offset: usize,
        bytes: &[u8],
    ) -> TcpStream {
        self.begin(
            "PATCH",
            &format!("/uploads/{}/content", upload.id),
            Some(auth),
            &[
                ("Content-Type", "application/octet-stream"),
                ("X-Mutation-ID", &upload.ticket),
                ("Upload-Offset", &offset.to_string()),
                ("Upload-Checksum", &sha256_hex(bytes)),
            ],
            bytes.len(),
        )
        .await
    }
}

struct Response {
    head: String,
    body: Vec<u8>,
}

impl Response {
    fn expect(self, status: u16) -> Self {
        let actual: u16 = self
            .head
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(actual, status, "{}", String::from_utf8_lossy(&self.body));
        self
    }

    fn header(&self, key: &str) -> &str {
        self.head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| name.eq_ignore_ascii_case(key).then_some(value.trim()))
            .unwrap()
    }

    fn cookie(&self) -> String {
        self.header("set-cookie").split(';').next().unwrap().into()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

async fn response(mut stream: TcpStream) -> Response {
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(10), stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let cut = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    Response {
        head: String::from_utf8(bytes[..cut].to_vec()).unwrap(),
        body: bytes[cut + 4..].to_vec(),
    }
}

fn backup_succeeded(out: Output, count: u64) {
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("backup_complete payloads={count}\n")
    );
}

fn backup_failed(out: Output, reason: &str) {
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(String::from_utf8(out.stderr).unwrap().contains(reason));
}

#[tokio::test]
async fn cli_restores_committed_bytes_and_recovers_a_real_upload_batch() {
    let _serial = CLI_FIXTURES.lock().await;
    let fixture = Fixture::new("active-batch");
    let armed = Arc::new(AtomicBool::new(false));
    let gate = Arc::new(TestBarrier::new("before publication rename"));
    let hook: PublishFaultHook = {
        let (armed, gate) = (Arc::clone(&armed), Arc::clone(&gate));
        Arc::new(move |step| {
            if step == "publish.before_noreplace" && armed.swap(false, Ordering::Relaxed) {
                gate.worker_pause(Duration::from_secs(20))
                    .map_err(|_| PayloadError::Io("backup_fixture_publish_barrier_failed"))?;
            }
            Ok(())
        })
    };
    let live = Server::start_with_publish_hook(&fixture.live, Some(hook)).await;
    live.setup(&fixture.live).await;
    let auth = live.login().await;

    let settings = InstanceSettings::open(&fixture.live).unwrap();
    settings.save("Backup fixture").unwrap();
    settings
        .save_appearance(AppearanceChange {
            accent_color: Some("#112233".into()),
            login_background_color: None,
            logo: AssetChange::Keep,
            login_image: AssetChange::Keep,
        })
        .unwrap();
    let binary: Vec<u8> = (0..1_000_005).map(|i| (i % 251) as u8).collect();
    let committed = [
        (
            live.upload(&auth, "before.bin", &binary).await,
            binary.as_slice(),
        ),
        (live.upload(&auth, "empty.txt", b"").await, &[]),
    ];

    let body = b"publish now, commit after backup";
    let publishing = live.admit(&auth, "publishing.bin", body).await;
    live.put(&auth, &publishing, body).await;
    assert_eq!(live.status(&auth, &publishing).await["status"], "staged");

    let partial = live.admit(&auth, "partial.bin", b"first second").await;
    let mut first = live.begin_patch(&auth, &partial, 0, b"first ").await;
    first.write_all(b"first ").await.unwrap();
    assert_eq!(
        response(first).await.expect(200).json()["bytes_received"],
        6
    );
    let mut held = live.begin_patch(&auth, &partial, 6, b"second").await;
    held.write_all(b"se").await.unwrap();
    timeout(Duration::from_secs(5), async {
        while live.status(&auth, &partial).await["writer_active"] != true {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let ready_body = b"fixed while still in staging";
    let ready = live.admit(&auth, "ready.bin", ready_body).await;
    let mut staged = live.begin_patch(&auth, &ready, 0, ready_body).await;
    armed.store(true, Ordering::Relaxed);
    staged.write_all(ready_body).await.unwrap();
    let waiter = Arc::clone(&gate);
    tokio::task::spawn_blocking(move || waiter.wait_reached(Duration::from_secs(8)))
        .await
        .unwrap()
        .unwrap();
    let ready_state = live.status(&auth, &ready).await;
    assert_eq!(ready_state["writer_active"], true);
    assert_eq!(ready_state["bytes_received"], ready_body.len());

    backup_succeeded(fixture.backup_to(&fixture.backup).await, 4);
    gate.release().unwrap();
    response(staged).await.expect(200);
    gate.assert_drained().unwrap();
    let active = live.status(&auth, &partial).await;
    assert_eq!(active["writer_active"], true);
    assert_eq!(active["bytes_received"], 6);
    held.write_all(b"cond").await.unwrap();
    response(held).await.expect(200);
    let post_cut = live.commit(&auth, &publishing).await;
    assert_eq!(live.download(&auth, &post_cut).await, body);
    live.close().await;

    fs::create_dir(&fixture.restored).unwrap();
    assert!(
        Command::new("cp")
            .arg("-pR")
            .arg(fixture.backup.join("."))
            .arg(&fixture.restored)
            .status()
            .unwrap()
            .success()
    );
    let restored = Server::start(&fixture.restored).await;
    let listing = restored.listing(&auth).await;
    assert_eq!(listing["items"].as_array().unwrap().len(), committed.len());
    for (id, bytes) in committed {
        assert_eq!(restored.download(&auth, &id).await, bytes);
    }
    let state = restored.status(&auth, &partial).await;
    assert_eq!(state["writer_active"], false);
    assert_eq!(state["bytes_received"], 0);
    let mut retry = restored
        .begin_patch(&auth, &partial, 0, b"first second")
        .await;
    retry.write_all(b"first second").await.unwrap();
    response(retry).await.expect(200);
    assert_eq!(
        restored.status(&auth, &publishing).await["status"],
        "staged"
    );
    let recovered_ready = restored.commit(&auth, &ready).await;
    assert_eq!(restored.download(&auth, &recovered_ready).await, ready_body);
    let settings = InstanceSettings::open(&fixture.restored).unwrap();
    assert_eq!(settings.name().unwrap(), "Backup fixture");
    assert_eq!(
        settings.appearance().unwrap().accent_color.as_deref(),
        Some("#112233")
    );
    restored.close().await;
    fixture.close();
}

#[tokio::test]
async fn cli_refuses_overlap_aliases_and_nonempty_destinations() {
    let _serial = CLI_FIXTURES.lock().await;
    let fixture = Fixture::new("destination");
    let alias = fixture.owner.path().join("alias");
    symlink(&fixture.live, &alias).unwrap();
    backup_failed(
        fixture.backup_to(&fixture.live.join("out")).await,
        "overlaps_source",
    );
    backup_failed(
        fixture.backup_to(&alias.join("metadata/../out")).await,
        "overlaps_source",
    );
    backup_failed(fixture.backup_to(&alias).await, "overlaps_source");
    backup_failed(
        fixture.backup_to(fixture.owner.path()).await,
        "overlaps_source",
    );
    assert!(!fixture.live.join("out").exists());

    fs::create_dir(&fixture.backup).unwrap();
    let sentinel = fixture.backup.join(".keep");
    fs::write(&sentinel, b"keep").unwrap();
    backup_failed(
        fixture.backup_to(&fixture.backup).await,
        "destination_not_empty",
    );
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    fs::remove_file(sentinel).unwrap();

    backup_succeeded(fixture.backup_to(&fixture.backup).await, 0);
    assert!(fixture.backup.join(root::MANIFEST_NAME).is_file());
    assert!(!fixture.backup.join("metadata/sylux.sqlite3-wal").exists());
    assert!(!fixture.backup.join("metadata/sylux.sqlite3-shm").exists());
    fixture.close();
}

#[tokio::test]
async fn cli_rejects_invalid_setup_authorization_without_publishing() {
    let _serial = CLI_FIXTURES.lock().await;
    let fixture = Fixture::new("setup-authorization");
    let authorization = fixture.live.join("secrets/setup.authorization");
    let original = fs::read(&authorization).unwrap();
    let wrong = if original[0] == b'a' { b"b" } else { b"a" };
    let mut mismatch = original.clone();
    mismatch[0] = wrong[0];
    fs::write(&authorization, &mismatch).unwrap();
    backup_failed(
        fixture.backup_to(&fixture.backup).await,
        "backup_setup_authorization_does_not_match_snapshot",
    );
    assert!(!fixture.backup.exists());
    fs::write(&authorization, b"malformed").unwrap();
    backup_failed(
        fixture.backup_to(&fixture.backup).await,
        "setup_authorization_invalid",
    );
    assert!(!fixture.backup.exists());
    fixture.close();
}

#[tokio::test]
async fn cli_discards_failed_payload_copy_and_does_not_write_live_data() {
    let _serial = CLI_FIXTURES.lock().await;
    let fixture = Fixture::new("bad-payload");
    let live = Server::start(&fixture.live).await;
    live.setup(&fixture.live).await;
    let auth = live.login().await;
    let valid = live.upload(&auth, "good.bin", b"retained").await;
    let bad = live.upload(&auth, "corrupt.bin", b"correct").await;
    let db_path = fixture.live.join("metadata/sylux.sqlite3");
    let db = sqlite::open_reader(&db_path, DEFAULT_BUSY_TIMEOUT).unwrap();
    let blob: String = db.query_row(
        "SELECT r.blob_id FROM entry e JOIN revision r ON r.revision_id=e.current_revision_id WHERE e.entry_id=?1",
        [bad], |r| r.get(0),
    ).unwrap();
    drop(db);
    let payload = fixture.live.join("payloads").join(&blob[..2]).join(&blob);
    fs::write(&payload, b"damaged").unwrap();
    let before = [
        fs::read(&db_path).unwrap(),
        fs::read(db_path.with_extension("sqlite3-wal")).unwrap(),
    ];

    backup_failed(
        fixture.backup_to(&fixture.backup).await,
        "reason=digest_mismatch",
    );
    assert!(!fixture.backup.exists());
    let leftovers = fs::read_dir(fixture.owner.path())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".sylux-backup-incomplete")
        });
    assert!(!leftovers);
    assert_eq!(fs::read(payload).unwrap(), b"damaged");
    assert_eq!(fs::read(&db_path).unwrap(), before[0]);
    assert_eq!(
        fs::read(db_path.with_extension("sqlite3-wal")).unwrap(),
        before[1]
    );
    assert_eq!(live.download(&auth, &valid).await, b"retained");
    live.close().await;
    fixture.close();
}
