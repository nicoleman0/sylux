//! Coordinator-level current-session authorization and revoke ordering.
use rusqlite::{Transaction, params};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use sylux::{
    domain::{
        authz::validate_session,
        coordinator::{
            CoordinatorError, FinalCommitTestBarrier, MutationCoordinator, PublicationDecision,
        },
        coordinator_test_support::{Fixture, NOW, PRINCIPAL, SCOPE, SECRET, SESSION},
        ids::{MutationScope, MutationTicket, PrincipalId, TicketKeyId, TicketSigningKey},
        mutation::MutationResult,
    },
    persistence::sqlite::SqliteWorkers,
};

const WATCHDOG: Duration = Duration::from_secs(5);
const ROOT_ID: &str = "22222222222222222222222222222222";
const ENTRY_ID: &str = "33333333333333333333333333333333";
const BLOB_ID: &str = "44444444444444444444444444444444";
const REVISION_ID: &str = "55555555555555555555555555555555";

fn ticket(expires: i64) -> MutationTicket {
    MutationTicket::issue(
        PrincipalId::parse(PRINCIPAL).unwrap(),
        MutationScope::parse(SCOPE).unwrap(),
        1,
        expires,
        0,
        &TicketSigningKey::new(TicketKeyId::parse("synthetic-test-key").unwrap(), [7; 32]),
    )
    .unwrap()
}

async fn session_is_valid(workers: &SqliteWorkers) -> bool {
    let session = SESSION.to_owned();
    let secret = SECRET.to_vec();
    workers
        .read(move |db| Ok(validate_session(db, &session, &secret, NOW).is_some()))
        .await
        .unwrap()
}

async fn revoke_session(workers: &SqliteWorkers) {
    let changed = workers
        .transact(|tx| {
            tx.execute(
                "UPDATE session SET revoked=1 WHERE session_id=?1 AND revoked=0",
                [SESSION],
            )
        })
        .await
        .unwrap();
    assert_eq!(changed, 1, "synthetic session was not revoked");
}

async fn revoke_while_commit_is_paused(
    workers: &SqliteWorkers,
    barrier: &Arc<FinalCommitTestBarrier>,
) {
    let revoked = tokio::time::timeout(WATCHDOG, revoke_session(workers)).await;
    if revoked.is_err() {
        // Unwind an invalid seam that holds the sole writer across the pause.
        barrier.release();
    }
    revoked.expect(
        "revocation could not linearize while commit was paused; the writer lock is still held",
    );
}

async fn assert_revoked_credentials_cannot_read_or_replay(
    coordinator: &MutationCoordinator,
    ticket: &MutationTicket,
) {
    assert_eq!(
        coordinator
            .status(ticket.scoped_id(), ticket.scope(), SESSION, SECRET, NOW)
            .await,
        Err(CoordinatorError::InvalidSession),
        "revoked credentials read a mutation outcome"
    );
    assert_eq!(
        coordinator
            .admit_durable(ticket, [9; 32], SESSION, SECRET, NOW)
            .await,
        Err(CoordinatorError::InvalidSession),
        "revoked credentials replayed durable admission"
    );
    assert!(
        matches!(
            coordinator
                .begin_publication(ticket, SESSION, SECRET, NOW)
                .await,
            Err(CoordinatorError::InvalidSession)
        ),
        "revoked credentials replayed publication"
    );
}

fn insert_revision_effects(tx: &Transaction<'_>) -> rusqlite::Result<String> {
    tx.execute(
        "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
         VALUES(?1,?2,NULL,'folder','',x'',1)",
        params![ROOT_ID, PRINCIPAL],
    )?;
    tx.execute(
        "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
         VALUES(?1,?2,?3,'file','synthetic.txt',x'73796e7468657469632e747874',1)",
        params![ENTRY_ID, PRINCIPAL, ROOT_ID],
    )?;
    tx.execute(
        "INSERT INTO blob(blob_id,shard,length,sha256) VALUES(?1,'44',3,?2)",
        params![BLOB_ID, vec![5_u8; 32]],
    )?;
    tx.execute(
        "INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id)
         VALUES(?1,?2,?3,3,?4,10,?5)",
        params![REVISION_ID, ENTRY_ID, BLOB_ID, vec![5_u8; 32], PRINCIPAL],
    )?;
    tx.execute(
        "UPDATE entry SET current_revision_id=?1 WHERE entry_id=?2",
        params![REVISION_ID, ENTRY_ID],
    )?;
    Ok(REVISION_ID.to_owned())
}

fn spawn_commit_flow(
    coordinator: MutationCoordinator,
    ticket: MutationTicket,
    effects: Arc<AtomicUsize>,
) -> tokio::task::JoinHandle<(MutationTicket, Result<MutationResult, CoordinatorError>)> {
    tokio::spawn(async move {
        assert_eq!(
            coordinator
                .admit_durable(&ticket, [9; 32], SESSION, SECRET, NOW)
                .await
                .unwrap(),
            MutationResult::Pending
        );
        let permit = match coordinator
            .begin_publication(&ticket, SESSION, SECRET, NOW)
            .await
            .unwrap()
        {
            PublicationDecision::Ready(permit) => permit,
            PublicationDecision::Existing(result) => {
                panic!("unexpected admission state: {result:?}")
            }
        };
        let result = coordinator
            .commit(permit, SESSION, SECRET, NOW, move |tx| {
                effects.fetch_add(1, Ordering::SeqCst);
                insert_revision_effects(tx)
            })
            .await;
        (ticket, result)
    })
}

async fn await_commit(
    commit: tokio::task::JoinHandle<(MutationTicket, Result<MutationResult, CoordinatorError>)>,
) -> (MutationTicket, Result<MutationResult, CoordinatorError>) {
    tokio::time::timeout(WATCHDOG, commit)
        .await
        .expect("commit did not finish after the barrier released")
        .expect("commit task panicked")
}

#[tokio::test]
async fn current_session_authority_rejects_disabled_stale_epoch_idle_absolute_and_revoked_states() {
    let fixture = Fixture::create();
    assert!(session_is_valid(&fixture.workers).await);

    for (case, statement) in [
        ("disabled", "UPDATE account SET enabled=0"),
        (
            "stale epoch",
            "UPDATE account SET credential_epoch=credential_epoch+1",
        ),
        ("idle timeout", "UPDATE session SET idle_expires_at=10"),
        (
            "absolute timeout",
            "UPDATE session SET absolute_expires_at=10",
        ),
        ("revoked", "UPDATE session SET revoked=1"),
    ] {
        fixture
            .workers
            .transact(move |tx| {
                tx.execute_batch(statement)?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            !session_is_valid(&fixture.workers).await,
            "authoritative validation accepted {case}"
        );

        fixture
            .workers
            .transact(|tx| {
                tx.execute_batch(
                    "UPDATE account SET enabled=1,credential_epoch=0;
                     UPDATE session SET idle_expires_at=100,absolute_expires_at=200,revoked=0",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            session_is_valid(&fixture.workers).await,
            "reset after {case} did not restore the valid control session"
        );
    }
    fixture.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn barrier_revoke_first_rejects_commit_and_blocks_result_reads_and_retries() {
    let fixture = Fixture::create();
    let ticket = ticket(20);
    let barrier = Arc::new(FinalCommitTestBarrier::default());
    barrier.pause();
    fixture
        .coordinator
        .set_final_commit_test_barrier(Some(barrier.clone()));
    let effects = Arc::new(AtomicUsize::new(0));
    let commit = spawn_commit_flow(fixture.coordinator.clone(), ticket, effects.clone());

    barrier.wait_reached();
    revoke_while_commit_is_paused(&fixture.workers, &barrier).await;
    barrier.release();

    let (ticket, result) = await_commit(commit).await;
    assert_eq!(
        result,
        Err(CoordinatorError::InvalidSession),
        "revocation did not linearize before the final authoritative recheck"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let operation = ticket.mutation_id().as_str().to_owned();
    let (state, revisions): (String, i64) = fixture
        .workers
        .read(move |db| {
            db.query_row(
                "SELECT state,(SELECT count(*) FROM revision) FROM mutation
                 WHERE principal_account_id=?1 AND mutation_id=?2",
                params![PRINCIPAL, operation],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .await
        .unwrap();
    assert_eq!(state, "publishing");
    assert_eq!(revisions, 0, "revoke-first commit produced a domain effect");

    assert_revoked_credentials_cannot_read_or_replay(&fixture.coordinator, &ticket).await;
    assert_eq!(
        effects.load(Ordering::SeqCst),
        0,
        "a retry replayed effects through revoked credentials"
    );
    fixture.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn barrier_commit_first_preserves_result_then_blocks_reads_and_retries_after_revocation() {
    let fixture = Fixture::create();
    let ticket = ticket(20);
    let barrier = Arc::new(FinalCommitTestBarrier::default());
    barrier.pause();
    fixture
        .coordinator
        .set_final_commit_test_barrier(Some(barrier.clone()));
    let effects = Arc::new(AtomicUsize::new(0));
    let commit = spawn_commit_flow(fixture.coordinator.clone(), ticket, effects.clone());

    barrier.wait_reached();
    barrier.release();
    let (ticket, result) = await_commit(commit).await;
    let committed = match result.unwrap() {
        MutationResult::Committed(committed) => committed,
        other => panic!("commit-first did not commit: {other:?}"),
    };
    assert_eq!(committed.revision_id, REVISION_ID);
    assert_eq!(committed.entry_id, ENTRY_ID);
    assert_eq!(committed.length, 3);
    assert_eq!(committed.sha256, [5; 32]);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture
            .coordinator
            .status(ticket.scoped_id(), ticket.scope(), SESSION, SECRET, NOW)
            .await
            .unwrap(),
        MutationResult::Committed(committed),
        "committed result was not truthfully readable before revocation"
    );

    revoke_session(&fixture.workers).await;
    assert_revoked_credentials_cannot_read_or_replay(&fixture.coordinator, &ticket).await;
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "a retry replayed effects through revoked credentials"
    );
    fixture.cleanup().await;
}

#[test]
fn final_commit_barrier_is_injectable_for_deterministic_ordering_tests() {
    let coordinator = MutationCoordinator::default();
    let barrier = Arc::new(FinalCommitTestBarrier::default());
    coordinator.set_final_commit_test_barrier(Some(barrier));
    coordinator.set_final_commit_test_barrier(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_change_before_final_commit_rejects_prior_session_without_effects() {
    for (before, after) in [("user", "administrator"), ("administrator", "user")] {
        let fixture = Fixture::create();
        fixture
            .workers
            .transact(move |tx| tx.execute("UPDATE account SET role=?1", [before]))
            .await
            .unwrap();
        let barrier = Arc::new(FinalCommitTestBarrier::default());
        barrier.pause();
        fixture
            .coordinator
            .set_final_commit_test_barrier(Some(barrier.clone()));
        let effects = Arc::new(AtomicUsize::new(0));
        let commit = spawn_commit_flow(fixture.coordinator.clone(), ticket(20), effects.clone());
        barrier.wait_reached();
        let changed = tokio::time::timeout(
            WATCHDOG,
            fixture.workers.transact(move |tx| {
                // Leave revoked unset to prove the changed authority epoch rejects the session.
                tx.execute(
                    "UPDATE account SET role=?1,credential_epoch=credential_epoch+1",
                    [after],
                )
            }),
        )
        .await;
        if changed.is_err() {
            barrier.release();
        }
        assert_eq!(
            changed
                .expect("authority change could not acquire writer")
                .unwrap(),
            1
        );
        barrier.release();
        let (ticket, result) = await_commit(commit).await;
        assert_eq!(result, Err(CoordinatorError::InvalidSession));
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        assert_revoked_credentials_cannot_read_or_replay(&fixture.coordinator, &ticket).await;
        let revisions: i64 = fixture
            .workers
            .read(|db| db.query_row("SELECT count(*) FROM revision", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(revisions, 0);
        fixture.cleanup().await;
    }
}
