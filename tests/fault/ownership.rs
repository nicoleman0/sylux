//! T005/A10/F06 scenario specifications + explicitly labeled H0/kernel controls.
//! Product coordinator behavior is BLOCKED, not simulated by a test-owned model.
#[path = "../common/mod.rs"]
mod common;

use common::{
    FixtureRoot, TestBarrier,
    faults::{FaultAction, FaultController, FaultId},
};
use rustix::fs::{FlockOperation, flock};
use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::Path,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};
const WATCHDOG: Duration = Duration::from_secs(10);
const FINAL_BOUNDARIES: &[&str] = &[
    "publish.before_noreplace",
    "publish.before_final_dir_sync",
    "publish.before_ancestor_sync",
    "mutation.before_final_auth",
    "mutation.before_sql_commit",
];
fn fixture(case: &str) -> FixtureRoot {
    let parent = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("sylux-t005-{}", common::unique_token()));
    fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
    FixtureRoot::create(&parent, "ownership", case).unwrap()
}
fn cleanup(fixture: FixtureRoot) {
    fixture.cleanup_after_workers_exit().unwrap();
    fs::remove_dir(fixture.path().parent().unwrap()).unwrap();
}
fn create_lock(parent: &Path) -> File {
    use rustix::fs::{Mode, OFlags, open, openat};
    let directory = open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .unwrap();
    openat(
        &directory,
        "writer.lock",
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .unwrap()
    .into()
}
fn id(boundary: &str) -> FaultId {
    FaultId {
        run_id: "T005".into(),
        case_id: "ownership".into(),
        operation: "mutation-a".into(),
        boundary: boundary.into(),
        occurrence: 1,
    }
}
fn same_identity(held: &File, path: &Path) -> bool {
    let a = held.metadata().unwrap();
    let b = fs::symlink_metadata(path).unwrap();
    (a.dev(), a.ino()) == (b.dev(), b.ino()) && a.nlink() == 1 && b.nlink() == 1
}

#[derive(Debug, Clone, Copy)]
enum Scenario {
    LossBefore(&'static str),
    LossAfterSubmitted(&'static str),
    LockPathReplaced,
    RootPathReplaced,
    DescriptorFailed,
    ShutdownWithBlockedWorker,
}
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    RejectedKnownLoss,
    UnknownRequiresReconciliation,
    Drained,
}
#[derive(Debug)]
struct ProductReport {
    events: Vec<String>,
    ready: bool,
    admitting: bool,
    outcome: Outcome,
    old_generation: u64,
    terminal_generation: u64,
    automatic_reacquisitions: u32,
}
#[derive(Debug)]
struct UnsupportedH1H2;
struct ProductAdapter;
impl ProductAdapter {
    /// H1/H2 handoff: replace this with the real coordinator/guard driving H0
    /// hooks. Unsupported does not trigger faults, return invented events or PASS.
    fn run(
        scenario: Scenario,
        _faults: &FaultController,
    ) -> Result<ProductReport, UnsupportedH1H2> {
        match scenario {
            Scenario::LossBefore(boundary) | Scenario::LossAfterSubmitted(boundary) => {
                assert!(
                    FINAL_BOUNDARIES.contains(&boundary)
                        || boundary == "publish.after_noreplace"
                        || boundary == "mutation.after_sql_commit_before_result"
                );
            }
            _ => (),
        }
        Err(UnsupportedH1H2)
    }
}
fn assert_terminal(report: &ProductReport) {
    assert!(
        !report.ready && !report.admitting,
        "loss must close both readiness and admission"
    );
    assert!(
        report.terminal_generation > report.old_generation,
        "old permits must become invalid"
    );
    assert_eq!(
        report.automatic_reacquisitions, 0,
        "loss is terminal; never transparently reacquire"
    );
    let lost = report
        .events
        .iter()
        .position(|e| e == "ownership.lost")
        .expect("known loss event");
    assert!(
        !report.events[lost + 1..]
            .iter()
            .any(|e| FINAL_BOUNDARIES.contains(&e.as_str())),
        "new final boundary after known loss: {:?}",
        report.events
    );
}
macro_rules! known_loss {
    ($name:ident, $boundary:literal) => {
        #[test]
        #[ignore = "BLOCKED T008/H2: requires real coordinator, permits and final-boundary hooks"]
        fn $name() {
            let faults = FaultController::default();
            faults
                .arm(id("ownership.after_loss"), FaultAction::DefiniteReject)
                .unwrap();
            let report = ProductAdapter::run(Scenario::LossBefore($boundary), &faults)
                .expect("BLOCKED: Unsupported is not product ownership-loss RED");
            assert_terminal(&report);
            assert_eq!(report.outcome, Outcome::RejectedKnownLoss);
            assert_eq!(
                report.events,
                [
                    "permit.issued",
                    "ownership.lost",
                    "readiness.closed",
                    "admission.closed",
                    "final.rejected"
                ]
            );
            faults.assert_drained().unwrap();
        }
    };
}
known_loss!(f06_loss_before_noreplace, "publish.before_noreplace");
known_loss!(
    f06_loss_before_final_directory_sync,
    "publish.before_final_dir_sync"
);
known_loss!(
    f06_loss_before_ancestor_sync,
    "publish.before_ancestor_sync"
);
known_loss!(
    f06_loss_before_final_authorization,
    "mutation.before_final_auth"
);
known_loss!(f06_loss_before_sql_commit, "mutation.before_sql_commit");

macro_rules! submitted_loss {
    ($name:ident, $boundary:literal) => {
        #[test]
        #[ignore = "BLOCKED T008/H2: requires real submitted-I/O result classification"]
        fn $name() {
            let faults = FaultController::default();
            faults
                .arm(id($boundary), FaultAction::UncertainResult)
                .unwrap();
            let report = ProductAdapter::run(Scenario::LossAfterSubmitted($boundary), &faults)
                .expect("BLOCKED: no product submitted-I/O adapter");
            assert_terminal(&report);
            assert_eq!(report.outcome, Outcome::UnknownRequiresReconciliation);
            assert_eq!(
                report.events,
                [
                    "permit.issued",
                    "io.submitted",
                    "ownership.lost",
                    "readiness.closed",
                    "admission.closed",
                    "io.result.uncertain",
                    "reconciliation.required"
                ]
            );
            assert!(
                !report
                    .events
                    .iter()
                    .any(|e| e == "io.canceled" || e == "io.rolled_back"),
                "already-submitted kernel I/O cannot be retroactively canceled"
            );
            faults.assert_drained().unwrap();
        }
    };
}
submitted_loss!(
    f06_submitted_rename_result_loss_requires_reconciliation,
    "publish.after_noreplace"
);
submitted_loss!(
    f06_submitted_commit_result_loss_requires_reconciliation,
    "mutation.after_sql_commit_before_result"
);

macro_rules! identity_loss {
    ($name:ident, $scenario:expr) => {
        #[test]
        #[ignore = "BLOCKED T006/T008: inject actual retained-descriptor/root identity failure"]
        fn $name() {
            let report = ProductAdapter::run($scenario, &FaultController::default())
                .expect("BLOCKED: no product identity-loss adapter");
            assert_terminal(&report);
            assert_eq!(report.outcome, Outcome::RejectedKnownLoss);
        }
    };
}
identity_loss!(
    f06_lock_path_replacement_is_terminal,
    Scenario::LockPathReplaced
);
identity_loss!(
    f06_root_path_replacement_is_terminal,
    Scenario::RootPathReplaced
);
identity_loss!(
    f06_descriptor_failure_is_terminal,
    Scenario::DescriptorFailed
);

#[test]
#[ignore = "BLOCKED T006/T008: real shutdown must join a barrier-blocked worker before unlocking"]
fn a10_product_shutdown_retains_guard_until_worker_and_database_drain() {
    let report = ProductAdapter::run(
        Scenario::ShutdownWithBlockedWorker,
        &FaultController::default(),
    )
    .expect("BLOCKED: no product shutdown/drain adapter");
    assert_eq!(report.outcome, Outcome::Drained);
    assert!(!report.ready && !report.admitting);
    assert_eq!(
        report.events,
        [
            "readiness.closed",
            "admission.closed",
            "permits.invalidated",
            "worker.joined",
            "database.closed",
            "ownership.unlocked"
        ]
    );
}

#[test]
fn harness_replaced_path_can_lock_new_inode_but_held_identity_is_lost() {
    let owner = fixture("inode-replacement");
    let path = owner.path().join("writer.lock");
    let held = create_lock(owner.path());
    flock(&held, FlockOperation::NonBlockingLockExclusive).unwrap();
    assert!(same_identity(&held, &path));
    fs::rename(&path, owner.path().join("original.lock")).unwrap();
    let replacement = create_lock(owner.path());
    assert!(
        !same_identity(&held, &path),
        "identity mismatch, not mythical kernel fencing"
    );
    // Different inode really CAN be locked. Never assert the opposite.
    flock(&replacement, FlockOperation::NonBlockingLockExclusive).unwrap();
    let contender = File::open(owner.path().join("original.lock")).unwrap();
    assert!(flock(&contender, FlockOperation::NonBlockingLockExclusive).is_err());
    drop(held);
    flock(&contender, FlockOperation::NonBlockingLockExclusive).unwrap();
    drop((contender, replacement));
    cleanup(owner);
}

#[test]
fn harness_blocked_worker_is_joined_before_fixture_cleanup_and_descriptor_close() {
    let owner = fixture("joined-worker");
    let held = create_lock(owner.path());
    flock(&held, FlockOperation::NonBlockingLockExclusive).unwrap();
    let contender = File::open(owner.path().join("writer.lock")).unwrap();
    let barrier = Arc::new(TestBarrier::new("blocked-staging"));
    let worker_barrier = barrier.clone();
    let lease = owner.register_worker().unwrap();
    let mut staging = common::create_private_file(owner.path(), "staging-part").unwrap();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        worker_barrier.worker_pause(WATCHDOG).unwrap();
        staging.write_all(&[0, 1, 2, 255]).unwrap();
        tx.send("worker.finished").unwrap();
    });
    barrier.wait_reached(WATCHDOG).unwrap();
    assert!(rx.try_recv().is_err());
    assert!(owner.cleanup_after_workers_exit().is_err());
    assert!(flock(&contender, FlockOperation::NonBlockingLockExclusive).is_err());
    barrier.release().unwrap();
    worker.join().unwrap(); // join BEFORE collecting, avoiding inherited try_iter race
    lease.joined().unwrap();
    assert_eq!(rx.iter().collect::<Vec<_>>(), ["worker.finished"]);
    barrier.assert_drained().unwrap();
    assert!(flock(&contender, FlockOperation::NonBlockingLockExclusive).is_err());
    drop(held);
    flock(&contender, FlockOperation::NonBlockingLockExclusive).unwrap();
    drop(contender);
    cleanup(owner);
}

#[test]
fn harness_uncertain_fault_is_one_shot_and_traced_not_a_product_cancellation() {
    let faults = FaultController::default();
    let boundary = id("mutation.after_sql_commit_before_result");
    faults
        .arm(boundary.clone(), FaultAction::UncertainResult)
        .unwrap();
    assert_eq!(
        faults.trigger(&boundary).unwrap(),
        Some(FaultAction::UncertainResult)
    );
    assert_eq!(faults.trigger(&boundary).unwrap(), None);
    faults.assert_drained().unwrap();
    let trace = faults.trace().unwrap();
    assert_eq!(
        trace
            .iter()
            .map(|e| (e.sequence, e.boundary.as_str(), e.action.as_str()))
            .collect::<Vec<_>>(),
        [
            (1, "mutation.after_sql_commit_before_result", "injected"),
            (2, "mutation.after_sql_commit_before_result", "observed")
        ]
    );
}
