//! Deterministic commit-boundary fault observations using real SQLite workers.
//! Product recovery assertions intentionally remain RED against the T021 stub.

#[path = "../common/mod.rs"]
mod common;
use common::{
    FixtureRoot,
    faults::{FaultAction, FaultController, FaultId},
};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf, sync::Arc, time::Duration};
use sylux::persistence::{
    payload::{PayloadError, PayloadStore, PublishFaultHook},
    reconcile, root,
    sqlite::{self, ConnectionError, SqliteWorkers, TransactionError, WorkerConfig, WorkerError},
};

const BUSY: Duration = Duration::from_millis(300);

struct DbFixture {
    owner: FixtureRoot,
    product: PathBuf,
    db_path: PathBuf,
    workers: Option<SqliteWorkers>,
}
impl DbFixture {
    async fn new(case: &str) -> Self {
        let parent = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("fault-commit-{}", common::unique_token()));
        fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
        let owner = FixtureRoot::create(&parent, "fault-commit", case).unwrap();
        let product = owner.path().join("product");
        fs::DirBuilder::new().mode(0o700).create(&product).unwrap();
        root::initialize_empty_root(&product, BUSY).unwrap();
        drop(PayloadStore::open(&product).unwrap());
        let db_path = product.join("metadata/sylux.sqlite3");
        let writer = sqlite::open_writer(&db_path, BUSY).unwrap();
        let workers = SqliteWorkers::from_writer(
            writer,
            &db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        Self {
            owner,
            product,
            db_path,
            workers: Some(workers),
        }
    }
    fn workers(&self) -> &SqliteWorkers {
        self.workers.as_ref().unwrap()
    }
    async fn restart_and_reconcile(&mut self) -> reconcile::ReconcileReport {
        if let Some(workers) = self.workers.take() {
            workers.shutdown().await.unwrap();
        }
        let writer = sqlite::open_writer(&self.db_path, BUSY).unwrap();
        let workers = SqliteWorkers::from_writer(
            writer,
            &self.db_path,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        match reconcile::reconcile(&workers, &self.product).await {
            Ok(report) => {
                self.workers = Some(workers);
                report
            }
            Err(error) => {
                workers.shutdown().await.unwrap();
                self.cleanup_root();
                panic!("reconcile returned error: {error:?}");
            }
        }
    }
    fn cleanup_root(&self) {
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }
    async fn close(mut self) {
        if let Some(workers) = self.workers.take() {
            workers.shutdown().await.unwrap();
        }
        self.owner.cleanup_after_workers_exit().unwrap();
        fs::remove_dir(self.owner.path().parent().unwrap()).unwrap();
    }
}

fn fault_id(boundary: &str, case: &str) -> FaultId {
    FaultId {
        run_id: "t020-commit".into(),
        case_id: case.into(),
        operation: "durable-test-result".into(),
        boundary: boundary.into(),
        occurrence: 1,
    }
}

#[tokio::test]
async fn definite_pre_commit_error_is_the_real_worker_observation() {
    let fixture = DbFixture::new("before-result").await;
    let controller = FaultController::default();
    let id = fault_id("mutation.before_sql_commit", "before-result");
    controller
        .arm(id.clone(), FaultAction::DefiniteReject)
        .unwrap();
    let injected = controller.trigger(&id).unwrap();
    let trace = controller.trace().unwrap();
    assert_eq!(trace.len(), 1);
    assert_eq!(trace[0].boundary, "mutation.before_sql_commit");
    assert_eq!(trace[0].action, "injected");
    let observed = fixture
        .workers()
        .transact(move |tx| {
            if injected.is_some() {
                return Err(rusqlite::Error::InvalidQuery);
            }
            tx.execute(
                "UPDATE installation SET setup_consumed=1 WHERE singleton=1",
                [],
            )?;
            Ok(())
        })
        .await;
    assert!(
        matches!(
            observed,
            Err(WorkerError::Transaction(TransactionError::Definite(_)))
        ),
        "actual adapter result: {observed:?}"
    );
    let consumed: i64 = fixture
        .workers()
        .read(|db| {
            db.query_row(
                "SELECT setup_consumed FROM installation WHERE singleton=1",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(
        consumed, 0,
        "pre-commit rejection cannot create a durable side effect"
    );
    controller.assert_drained().unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn lost_caller_result_does_not_undo_durable_sql_commit() {
    let mut fixture = DbFixture::new("after-result").await;
    let id = fault_id("mutation.after_sql_commit_before_result", "after-result");
    let controller = FaultController::default();
    controller
        .arm(id.clone(), FaultAction::UncertainResult)
        .unwrap();
    let committed = fixture
        .workers()
        .transact(|tx| {
            tx.execute(
                "UPDATE installation SET setup_consumed=1 WHERE singleton=1",
                [],
            )?;
            Ok("durable-result".to_owned())
        })
        .await
        .unwrap();
    assert_eq!(committed, "durable-result");
    assert_eq!(
        controller.trigger(&id).unwrap(),
        Some(FaultAction::UncertainResult)
    );
    let trace = controller.trace().unwrap();
    assert_eq!(trace.len(), 1);
    assert_eq!(trace[0].boundary, "mutation.after_sql_commit_before_result");
    assert_eq!(trace[0].action, "injected");
    controller.assert_drained().unwrap();
    let durable: i64 = fixture
        .workers()
        .read(|db| {
            db.query_row(
                "SELECT setup_consumed FROM installation WHERE singleton=1",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(
        durable, 1,
        "discarding the caller result must not roll back committed state"
    );
    let report = fixture.restart_and_reconcile().await;
    assert!(report.ready);
    fixture.close().await;
}

#[tokio::test]
async fn unavailable_metadata_db_is_observed_without_fallback_or_replacement() {
    let mut fixture = DbFixture::new("db-unavailable").await;
    let original = fs::read(&fixture.db_path).unwrap();
    if let Some(workers) = fixture.workers.take() {
        workers.shutdown().await.unwrap();
    }
    fs::remove_file(&fixture.db_path).unwrap();
    let actual = SqliteWorkers::open(
        &fixture.db_path,
        WorkerConfig {
            busy_timeout: BUSY,
            ..WorkerConfig::default()
        },
    );
    assert!(
        matches!(actual, Err(ConnectionError::Open(_))),
        "missing authoritative DB must fail with open error, not create a replacement"
    );
    assert!(!fixture.db_path.exists());
    assert!(!original.is_empty());
    fixture.owner.cleanup_after_workers_exit().unwrap();
    fs::remove_dir(fixture.owner.path().parent().unwrap()).unwrap();
}

#[tokio::test]
async fn reconciliation_does_not_remove_unknown_durably_staged_candidate() {
    let mut fixture = DbFixture::new("preserve-inventory").await;
    let bytes = b"durable candidate retained at injected boundary";
    let attempt = "00000000000000000000000000000001";
    let blob = "00000000000000000000000000000002";
    let staging_name = format!("{attempt}-{blob}");
    let staging_path = fixture.product.join("staging").join(&staging_name);
    let controller = FaultController::default();
    let id = fault_id("publish.before_noreplace", "preserve-inventory");
    controller
        .arm(
            id.clone(),
            FaultAction::IoError {
                error: common::faults::InjectedIo::Sync,
            },
        )
        .unwrap();
    let faults = controller.clone();
    let hook: PublishFaultHook = Arc::new(move |boundary| {
        if boundary == "publish.before_noreplace" && faults.trigger(&id).unwrap().is_some() {
            return Err(PayloadError::Io("injected_publish_boundary"));
        }
        Ok(())
    });
    let mut store = PayloadStore::open(&fixture.product).unwrap();
    store.set_publish_fault_hook(Some(hook));
    let mut candidate = store.start(&staging_name, bytes.len() as u64).unwrap();
    candidate.write_chunk(bytes).unwrap();
    assert!(matches!(
        candidate.publish(Sha256::digest(bytes).into()),
        Err(PayloadError::Io("injected_publish_boundary"))
    ));
    controller.assert_drained().unwrap();
    let trace = controller.trace().unwrap();
    assert_eq!(trace.len(), 1);
    assert_eq!(trace[0].boundary, "publish.before_noreplace");
    assert_eq!(trace[0].action, "injected");
    assert_eq!(fs::read(&staging_path).unwrap(), bytes);
    let expected_inventory = vec![(staging_name, bytes.len(), hex(&Sha256::digest(bytes)))];
    drop(store);
    let report = fixture.restart_and_reconcile().await;
    let actual_inventory: Vec<(String, usize, String)> =
        fs::read_dir(fixture.product.join("staging"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let bytes = fs::read(entry.path()).unwrap();
                let digest = hex(&Sha256::digest(&bytes));
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    bytes.len(),
                    digest,
                )
            })
            .collect();
    assert_eq!(actual_inventory, expected_inventory);
    assert!(
        report
            .dispositions
            .contains(&reconcile::RecoveryDisposition::RecoveryRequired)
    );
    fixture.close().await;
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
