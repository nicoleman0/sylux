//! F07 referenced-payload faults must block reconciliation and startup.
#[path = "../common/mod.rs"]
mod common;

use common::FixtureRoot;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
use sylux::persistence::{
    payload::PayloadStore,
    reconcile::{self, RecoveryDisposition},
    root,
    sqlite::{self, SqliteWorkers, WorkerConfig},
};

const OWNER: &str = "00000000000000000000000000000001";
const OWNER_ROOT: &str = "00000000000000000000000000000101";
const ENTRY: &str = "00000000000000000000000000000111";
const REVISION: &str = "00000000000000000000000000000211";
const BLOB: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ATTEMPT: &str = "cccccccccccccccccccccccccccccccc";
const BODY: &[u8] = b"synthetic referenced payload";
const BUSY: Duration = Duration::from_millis(300);

struct Fixture {
    owner: FixtureRoot,
    product: PathBuf,
    db_path: PathBuf,
    payload: PathBuf,
}

impl Fixture {
    fn new(case: &str) -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("sylux-f07-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "f07", case).unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).expect("initialize disposable bound root");
        let db_path = product.join("metadata/sylux.sqlite3");
        let connection = sqlite::open_writer(&db_path, BUSY).expect("open product SQLite");
        Self::seed(&connection);

        let store = PayloadStore::open(&product).expect("open product payload store");
        let mut staging = store
            .start(&format!("{ATTEMPT}-{BLOB}"), BODY.len() as u64)
            .expect("create staged fixture payload");
        staging.write_chunk(BODY).expect("write fixture payload");
        let published = staging
            .publish(Sha256::digest(BODY).into())
            .expect("publish fixture payload");
        let expected_digest: [u8; 32] = Sha256::digest(BODY).into();
        assert_eq!(published.blob_id, BLOB);
        assert_eq!(published.sha256, expected_digest);
        drop(store);
        drop(connection);

        let payload = product.join("payloads").join(&BLOB[..2]).join(BLOB);
        Self {
            owner,
            product,
            db_path,
            payload,
        }
    }

    fn seed(connection: &Connection) {
        let digest = Sha256::digest(BODY);
        connection.execute(
            "INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'owner','user',1,0)",
            [OWNER],
        ).unwrap();
        connection.execute(
            "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,NULL,'folder','',x'',1,1)",
            params![OWNER_ROOT, OWNER],
        ).unwrap();
        connection.execute(
            "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,generation) VALUES(?1,?2,?3,'file','file.bin',?4,1,1)",
            params![ENTRY, OWNER, OWNER_ROOT, b"file.bin".as_slice()],
        ).unwrap();
        connection.execute(
            "INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,'aa',?2,?3,'live')",
            params![BLOB, BODY.len() as i64, digest.as_slice()],
        ).unwrap();
        connection.execute(
            "INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?2,?3,?4,?5,1,?6)",
            params![REVISION, ENTRY, BLOB, BODY.len() as i64, digest.as_slice(), OWNER],
        ).unwrap();
        connection
            .execute(
                "UPDATE entry SET current_revision_id=?1 WHERE entry_id=?2",
                params![REVISION, ENTRY],
            )
            .unwrap();
    }

    async fn reconcile(&self) -> reconcile::ReconcileReport {
        let connection = sqlite::open_writer(&self.db_path, BUSY).expect("open faulted database");
        let workers = SqliteWorkers::from_writer(
            connection,
            &self.db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .expect("start SQLite workers");
        let result = reconcile::reconcile(&workers, &self.product)
            .await
            .expect("reconciliation adapter remains available");
        workers.shutdown().await.expect("drain SQLite workers");
        result
    }

    fn assert_startup_refuses_before_listener(&self) {
        let lease = self.owner.register_worker().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_sylux"))
            .args([
                "serve",
                "--root",
                self.product.to_str().unwrap(),
                "--listen",
                "127.0.0.1:0",
                "--tls-cert",
                "missing-cert-for-recovery-gate",
                "--tls-key",
                "missing-key-for-recovery-gate",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .expect("serve process starts");
        lease.joined().unwrap();
        assert!(!output.status.success(), "faulted root must refuse serve");
        let events: Vec<serde_json::Value> = String::from_utf8(output.stderr)
            .expect("contract event stream is UTF-8")
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        assert!(
            events.iter().any(|event| {
                event["contract_event"] == "startup.failed"
                    && event["reason"] == "recovery_required"
            }),
            "startup must report the generic recovery refusal"
        );
        assert!(
            !events
                .iter()
                .any(|event| event["contract_event"] == "listener.bind"),
            "unverified bytes must not reach readiness"
        );
    }

    fn assert_reference_retained(&self) {
        let connection = Connection::open(&self.db_path).unwrap();
        let counts: (i64, i64, Option<String>) = connection
            .query_row(
                "SELECT (SELECT count(*) FROM blob WHERE blob_id=?1), (SELECT count(*) FROM revision WHERE revision_id=?2), (SELECT current_revision_id FROM entry WHERE entry_id=?3)",
                params![BLOB, REVISION, ENTRY],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(counts, (1, 1, Some(REVISION.to_owned())));
    }

    fn close(self) {
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }
}

#[tokio::test]
async fn missing_referenced_payload_blocks_reconcile_and_startup() {
    let fixture = Fixture::new("missing-reference");
    fs::remove_file(&fixture.payload).unwrap();

    let report = fixture.reconcile().await;
    assert!(!report.ready);
    assert!(
        report
            .dispositions
            .contains(&RecoveryDisposition::RecoveryRequired)
    );
    fixture.assert_reference_retained();
    assert!(!fixture.payload.exists());
    fixture.assert_startup_refuses_before_listener();
    fixture.assert_reference_retained();
    fixture.close();
}

#[tokio::test]
async fn changed_referenced_payload_blocks_reconcile_and_startup_without_repair() {
    let fixture = Fixture::new("corrupt-reference");
    let mut corrupt = BODY.to_vec();
    corrupt[0] ^= 1;
    assert_eq!(corrupt.len(), BODY.len());
    assert_ne!(corrupt.as_slice(), BODY);
    fs::write(&fixture.payload, &corrupt).unwrap();

    let report = fixture.reconcile().await;
    assert!(!report.ready);
    assert!(
        report
            .dispositions
            .contains(&RecoveryDisposition::RecoveryRequired)
    );
    fixture.assert_reference_retained();
    assert_eq!(fs::read(&fixture.payload).unwrap(), corrupt);
    fixture.assert_startup_refuses_before_listener();
    fixture.assert_reference_retained();
    assert_eq!(fs::read(&fixture.payload).unwrap(), corrupt);
    fixture.close();
}
