//! Durable, serialized mutation admission and result reconciliation.
//!
//! All durable transitions use the single bounded SQLite writer. An unresolved
//! `admitted`/`publishing` row is never expired, deleted, or evicted. An
//! uncertain COMMIT is surfaced to the caller; this coordinator never retries
//! the transaction or domain-effect closure. Call `status` to reconcile after
//! recovery before deciding whether further work is safe.

use crate::{
    domain::{
        authz::validate_session,
        ids::{MutationScope, MutationTicket, ScopedMutationId},
        mutation::{CommittedResult, MutationResult},
    },
    error::MutationFailure,
    persistence::sqlite::{SqliteWorkers, WorkerError},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::sync::{Arc, Mutex, MutexGuard};

#[cfg(feature = "test-harness")]
#[derive(Default)]
pub struct FinalCommitTestBarrier {
    state: Mutex<FinalCommitBarrierState>,
    changed: std::sync::Condvar,
}

#[cfg(feature = "test-harness")]
#[derive(Default)]
struct FinalCommitBarrierState {
    paused: bool,
    reached: bool,
    released: bool,
}

#[cfg(feature = "test-harness")]
impl FinalCommitTestBarrier {
    pub fn pause(&self) {
        let mut state = self.state.lock().expect("final commit barrier poisoned");
        state.paused = true;
        state.reached = false;
        state.released = false;
    }

    pub fn wait_reached(&self) {
        let mut state = self.state.lock().expect("final commit barrier poisoned");
        while !state.reached {
            state = self
                .changed
                .wait(state)
                .expect("final commit barrier poisoned");
        }
    }

    pub fn release(&self) {
        let mut state = self.state.lock().expect("final commit barrier poisoned");
        state.released = true;
        self.changed.notify_all();
    }

    fn pause_at_final_commit(&self) {
        let mut state = self.state.lock().expect("final commit barrier poisoned");
        if !state.paused {
            return;
        }
        state.reached = true;
        self.changed.notify_all();
        while !state.released {
            state = self
                .changed
                .wait(state)
                .expect("final commit barrier poisoned");
        }
        state.paused = false;
    }
}

#[derive(Clone, Default)]
pub struct MutationCoordinator {
    gate: Arc<Mutex<()>>,
    workers: Option<SqliteWorkers>,
    #[cfg(feature = "test-harness")]
    final_commit_barrier: Arc<Mutex<Option<Arc<FinalCommitTestBarrier>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    Unavailable,
    InvalidSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatorError {
    InvalidSession,
    Conflict,
    Expired,
    InvalidState,
    Failure(MutationFailure),
}

/// Begin-publication result: only a new admitted-to-publishing transition
/// grants a one-use final-commit permit. Existing unresolved work stays pending
/// until recovery establishes whether it is safe to resume.
pub enum PublicationDecision {
    Ready(PublicationPermit),
    Existing(MutationResult),
}

/// Non-cloneable authority consumed by the final result transaction.
pub struct PublicationPermit {
    operation: ScopedMutationId,
    scope: MutationScope,
}

/// Guard retained by existing synchronous request boundaries for serialization
/// with logout. Durable record transitions are additionally serialized by the
/// SQLite writer's `BEGIN IMMEDIATE` transaction.
pub struct AdmissionGuard<'a> {
    _guard: MutexGuard<'a, ()>,
}

/// Session-bound authorization proof used by existing request handlers.
#[derive(Clone)]
pub struct AdmissionTicket {
    session_id: String,
    secret: [u8; 32],
    account_id: String,
}

impl AdmissionTicket {
    pub fn account_id(&self) -> &str {
        &self.account_id
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

impl MutationCoordinator {
    /// Construct the durable coordinator over the integrated bounded SQLite
    /// workers. A default coordinator remains available for legacy in-memory
    /// session serialization; durable methods fail closed until workers are set.
    pub fn new(workers: SqliteWorkers) -> Self {
        Self {
            gate: Arc::default(),
            workers: Some(workers),
            #[cfg(feature = "test-harness")]
            final_commit_barrier: Arc::default(),
        }
    }

    #[cfg(feature = "test-harness")]
    pub fn set_final_commit_test_barrier(&self, barrier: Option<Arc<FinalCommitTestBarrier>>) {
        *self
            .final_commit_barrier
            .lock()
            .expect("final commit barrier poisoned") = barrier;
    }

    fn workers(&self) -> Result<&SqliteWorkers, CoordinatorError> {
        self.workers.as_ref().ok_or(CoordinatorError::Failure(
            MutationFailure::AdmissionUnavailable,
        ))
    }

    pub fn serialize(&self) -> Result<AdmissionGuard<'_>, AdmissionError> {
        self.gate
            .lock()
            .map(|guard| AdmissionGuard { _guard: guard })
            .map_err(|_| AdmissionError::Unavailable)
    }

    /// Preserve the synchronous session admission seam used by existing HTTP
    /// callers. Durable mutation-row admission is `admit_durable` below.
    pub fn admit(
        &self,
        _guard: &AdmissionGuard<'_>,
        connection: &Connection,
        session_id: &str,
        secret: &[u8],
        now: i64,
    ) -> Result<AdmissionTicket, AdmissionError> {
        self.admit_current(connection, session_id, secret, now)
    }

    /// Authenticate using an owned-worker read closure. The returned proof is
    /// rechecked inside each final writer transaction before its effect.
    pub fn admit_current(
        &self,
        connection: &Connection,
        session_id: &str,
        secret: &[u8],
        now: i64,
    ) -> Result<AdmissionTicket, AdmissionError> {
        let Some((account_id, _, _)) = validate_session(connection, session_id, secret, now) else {
            return Err(AdmissionError::InvalidSession);
        };
        let secret: [u8; 32] = secret
            .try_into()
            .map_err(|_| AdmissionError::InvalidSession)?;
        Ok(AdmissionTicket {
            session_id: session_id.to_owned(),
            secret,
            account_id,
        })
    }

    pub fn revalidate(
        &self,
        _guard: &AdmissionGuard<'_>,
        connection: &Connection,
        ticket: &AdmissionTicket,
        now: i64,
    ) -> Result<(String, String, String), AdmissionError> {
        self.revalidate_current(connection, ticket, now)
    }

    pub fn revalidate_current(
        &self,
        connection: &Connection,
        ticket: &AdmissionTicket,
        now: i64,
    ) -> Result<(String, String, String), AdmissionError> {
        validate_session(connection, &ticket.session_id, &ticket.secret, now)
            .filter(|(account_id, _, _)| account_id == &ticket.account_id)
            .ok_or(AdmissionError::InvalidSession)
    }

    /// Atomically authenticate and create a durable scoped admission. The
    /// caller must first validate the signed ticket against current principal,
    /// scope, epoch and clock. It computes the canonical versioned request
    /// fingerprint; an exact replay returns stored state, a changed fingerprint
    /// conflicts. The coordinator does not persist ticket secrets.
    pub async fn admit_durable(
        &self,
        ticket: &MutationTicket,
        admission_fingerprint: [u8; 32],
        session_id: &str,
        secret: &[u8],
        now: i64,
    ) -> Result<MutationResult, CoordinatorError> {
        let workers = self.workers()?;
        let principal = ticket.principal().as_str().to_owned();
        let mutation_id = ticket.mutation_id().as_str().to_owned();
        let scope = ticket.scope().as_str().to_owned();
        let key_id = ticket.key_id().as_str().to_owned();
        let epoch = ticket.recovery_epoch();
        let issued_at = ticket.issued_at();
        let expires_at = ticket.expires_at();
        let session_id = session_id.to_owned();
        let secret = secret.to_vec();
        let result = workers.transact(move |tx| {
            let Some((account_id, _, _)) = validate_session(tx, &session_id, &secret, now) else {
                return Ok(DurableDecision::InvalidSession);
            };
            if account_id != principal { return Ok(DurableDecision::InvalidSession); }
            let existing: Option<(String, Vec<u8>)> = tx.query_row(
                "SELECT scope,admission_fingerprint FROM mutation WHERE principal_account_id=?1 AND mutation_id=?2",
                params![principal, mutation_id], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            if let Some((stored_scope, existing)) = existing {
                if stored_scope != scope || existing.as_slice() != admission_fingerprint.as_slice() { return Ok(DurableDecision::Conflict); }
                return Ok(match mutation_result(tx, &principal, &mutation_id, &scope)? {
                    Some(result) => DurableDecision::Result(result),
                    None => DurableDecision::Result(MutationResult::Unknown),
                });
            }
            if now < 0 || issued_at > now || expires_at <= now {
                return Ok(DurableDecision::Expired);
            }
            tx.execute(
                "INSERT INTO mutation(mutation_id,principal_account_id,scope,ticket_key_id,ticket_epoch,issued_at,expires_at,admission_fingerprint,state)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'admitted')",
                params![mutation_id, principal, scope, key_id, epoch, issued_at, expires_at, admission_fingerprint.as_slice()],
            )?;
            Ok(DurableDecision::Result(MutationResult::Pending))
        }).await.map_err(worker_error)?;
        decision_result(result)
    }

    /// Move an admission to `publishing` once. Only the winning transition gets
    /// a one-shot permit; seeing an existing `publishing` row returns Pending
    /// without authorizing another effect. Expired, still-admitted work is
    /// durably rejected; expiry never touches an unresolved publishing row.
    pub async fn begin_publication(
        &self,
        ticket: &MutationTicket,
        session_id: &str,
        secret: &[u8],
        now: i64,
    ) -> Result<PublicationDecision, CoordinatorError> {
        let workers = self.workers()?;
        let principal = ticket.principal().as_str().to_owned();
        let mutation_id = ticket.mutation_id().as_str().to_owned();
        let scope = ticket.scope().as_str().to_owned();
        let expires_at = ticket.expires_at();
        let operation = ticket.scoped_id().clone();
        let permit_scope = ticket.scope().clone();
        let session_id = session_id.to_owned();
        let secret = secret.to_vec();
        let result = workers.transact(move |tx| {
            if !session_matches(tx, &session_id, &secret, now, &principal) {
                return Ok(DurableDecision::InvalidSession);
            }
            let state: Option<String> = tx.query_row(
                "SELECT state FROM mutation WHERE principal_account_id=?1 AND mutation_id=?2 AND scope=?3",
                params![principal, mutation_id, scope], |row| row.get(0),
            ).optional()?;
            match state.as_deref() {
                Some("admitted") => {
                    if now < 0 || now >= expires_at {
                        tx.execute("UPDATE mutation SET state='rejected' WHERE principal_account_id=?1 AND mutation_id=?2 AND state='admitted'", params![principal, mutation_id])?;
                        return Ok(DurableDecision::Result(MutationResult::Rejected));
                    }
                    tx.execute("UPDATE mutation SET state='publishing' WHERE principal_account_id=?1 AND mutation_id=?2 AND state='admitted'", params![principal, mutation_id])?;
                    Ok(DurableDecision::Permit(PublicationPermit { operation, scope: permit_scope }))
                }
                Some("publishing") => Ok(DurableDecision::Result(MutationResult::Pending)),
                Some("committed" | "rejected") => Ok(DurableDecision::Result(mutation_result(tx, &principal, &mutation_id, &scope)?.unwrap_or(MutationResult::Unknown))),
                Some(_) => Ok(DurableDecision::InvalidState),
                None => Ok(DurableDecision::Result(MutationResult::Unknown)),
            }
        }).await.map_err(worker_error)?;
        match result {
            DurableDecision::Permit(permit) => Ok(PublicationDecision::Ready(permit)),
            other => decision_result(other).map(PublicationDecision::Existing),
        }
    }

    /// Commit caller-supplied SQL effects and the immutable revision result in
    /// one transaction. The one-use permit is consumed by this call; the closure
    /// must contain SQL effects only (never filesystem/network side effects).
    /// An uncertain COMMIT is surfaced and never retried by this coordinator.
    pub async fn commit<F>(
        &self,
        permit: PublicationPermit,
        session_id: &str,
        secret: &[u8],
        now: i64,
        operation: F,
    ) -> Result<MutationResult, CoordinatorError>
    where
        F: for<'connection> FnOnce(&Transaction<'connection>) -> rusqlite::Result<String>
            + Send
            + 'static,
    {
        let workers = self.workers()?;
        let principal = permit.operation.principal().as_str().to_owned();
        let mutation_id = permit.operation.mutation_id().as_str().to_owned();
        let scope = permit.scope.as_str().to_owned();
        let session_id = session_id.to_owned();
        let secret = secret.to_vec();
        #[cfg(feature = "test-harness")]
        let final_commit_barrier = self
            .final_commit_barrier
            .lock()
            .map_err(|_| CoordinatorError::Failure(MutationFailure::AdmissionUnavailable))?
            .clone();
        #[cfg(feature = "test-harness")]
        if let Some(barrier) = &final_commit_barrier {
            // Do not hold the sole writer's transaction across this test-only pause.
            barrier.pause_at_final_commit();
        }
        let decision = workers.transact(move |tx| {
            if !session_matches(tx, &session_id, &secret, now, &principal) {
                return Ok(DurableDecision::InvalidSession);
            }
            let state: Option<String> = tx.query_row(
                "SELECT state FROM mutation WHERE principal_account_id=?1 AND mutation_id=?2 AND scope=?3",
                params![principal, mutation_id, scope], |row| row.get(0),
            ).optional()?;
            match state.as_deref() {
                Some("committed" | "rejected") => return Ok(DurableDecision::Result(mutation_result(tx, &principal, &mutation_id, &scope)?.unwrap_or(MutationResult::Unknown))),
                Some("publishing") => {},
                Some("admitted") => return Ok(DurableDecision::InvalidState),
                Some(_) => return Ok(DurableDecision::InvalidState),
                None => return Ok(DurableDecision::Result(MutationResult::Unknown)),
            }
            let revision_id = operation(tx)?;
            let committed: Option<CommittedResult> = tx.query_row(
                "SELECT r.revision_id,r.entry_id,r.length,r.sha256 FROM revision r JOIN mutation m ON m.result_revision_id IS NULL AND m.principal_account_id=?1 AND m.mutation_id=?2 AND m.state='publishing' WHERE r.revision_id=?3 AND r.actor_account_id=m.principal_account_id",
                params![principal, mutation_id, revision_id], |row| {
                    let length: i64 = row.get(2)?;
                    let digest: Vec<u8> = row.get(3)?;
                    let Ok(sha256) = <[u8; 32]>::try_from(digest) else { return Ok(None); };
                    let Ok(length) = u64::try_from(length) else { return Ok(None); };
                    Ok(Some(CommittedResult { revision_id: row.get(0)?, entry_id: row.get(1)?, length, sha256 }))
                },
            ).optional()?.flatten();
            let Some(committed) = committed else { return Err(rusqlite::Error::QueryReturnedNoRows); };
            let changed = tx.execute(
                "UPDATE mutation SET result_revision_id=?3,state='committed' WHERE principal_account_id=?1 AND mutation_id=?2 AND state='publishing' AND result_revision_id IS NULL",
                params![principal, mutation_id, committed.revision_id],
            )?;
            if changed != 1 { return Err(rusqlite::Error::QueryReturnedNoRows); }
            Ok(DurableDecision::Result(MutationResult::Committed(committed)))
        }).await.map_err(worker_error)?;
        decision_result(decision)
    }

    /// Authenticated read-only reconciliation. Missing/corrupt rows are
    /// `Unknown`; an admitted or publishing row is `Pending`. The method never
    /// changes state or triggers domain effects, and deliberately ignores
    /// ticket expiry because expiry is an execution rule, not record retention.
    pub async fn status(
        &self,
        operation: &ScopedMutationId,
        scope: &MutationScope,
        session_id: &str,
        secret: &[u8],
        now: i64,
    ) -> Result<MutationResult, CoordinatorError> {
        let workers = self.workers()?;
        let principal = operation.principal().as_str().to_owned();
        let mutation_id = operation.mutation_id().as_str().to_owned();
        let scope = scope.as_str().to_owned();
        let session_id = session_id.to_owned();
        let secret = secret.to_vec();
        workers
            .read(move |connection| {
                // Keep current-session validation and result lookup on the same
                // SQLite snapshot so a concurrent revocation cannot race the
                // outcome disclosure.
                connection.execute_batch("BEGIN DEFERRED")?;
                let decision = (|| {
                    if !session_matches(connection, &session_id, &secret, now, &principal) {
                        return Ok(DurableDecision::InvalidSession);
                    }
                    Ok(DurableDecision::Result(
                        mutation_result(connection, &principal, &mutation_id, &scope)?
                            .unwrap_or(MutationResult::Unknown),
                    ))
                })();
                match decision {
                    Ok(decision) => match connection.execute_batch("COMMIT") {
                        Ok(()) => Ok(decision),
                        Err(error) => {
                            let _ = connection.execute_batch("ROLLBACK");
                            Err(error)
                        }
                    },
                    Err(error) => {
                        let _ = connection.execute_batch("ROLLBACK");
                        Err(error)
                    }
                }
            })
            .await
            .map_err(worker_error)
            .and_then(decision_result)
    }

    /// Definitively reject only work that has not entered publication. A
    /// publishing record requires recovery and cannot be overwritten by a
    /// convenient rejection.
    pub async fn reject_admission(
        &self,
        ticket: &MutationTicket,
        session_id: &str,
        secret: &[u8],
        now: i64,
    ) -> Result<MutationResult, CoordinatorError> {
        let workers = self.workers()?;
        let principal = ticket.principal().as_str().to_owned();
        let mutation_id = ticket.mutation_id().as_str().to_owned();
        let scope = ticket.scope().as_str().to_owned();
        let session_id = session_id.to_owned();
        let secret = secret.to_vec();
        let result = workers.transact(move |tx| {
            if !session_matches(tx, &session_id, &secret, now, &principal) { return Ok(DurableDecision::InvalidSession); }
            let state: Option<String> = tx.query_row("SELECT state FROM mutation WHERE principal_account_id=?1 AND mutation_id=?2 AND scope=?3", params![principal, mutation_id, scope], |row| row.get(0)).optional()?;
            match state.as_deref() {
                Some("admitted") => {
                    tx.execute("UPDATE mutation SET state='rejected' WHERE principal_account_id=?1 AND mutation_id=?2 AND state='admitted'", params![principal, mutation_id])?;
                    Ok(DurableDecision::Result(MutationResult::Rejected))
                }
                Some("rejected" | "committed") => Ok(DurableDecision::Result(mutation_result(tx, &principal, &mutation_id, &scope)?.unwrap_or(MutationResult::Unknown))),
                Some("publishing") => Ok(DurableDecision::InvalidState),
                Some(_) => Ok(DurableDecision::InvalidState),
                None => Ok(DurableDecision::Result(MutationResult::Unknown)),
            }
        }).await.map_err(worker_error)?;
        decision_result(result)
    }
}

enum DurableDecision {
    Result(MutationResult),
    Permit(PublicationPermit),
    InvalidSession,
    Conflict,
    Expired,
    InvalidState,
}

fn decision_result(decision: DurableDecision) -> Result<MutationResult, CoordinatorError> {
    match decision {
        DurableDecision::Result(result) => Ok(result),
        DurableDecision::Permit(_) => Err(CoordinatorError::InvalidState),
        DurableDecision::InvalidSession => Err(CoordinatorError::InvalidSession),
        DurableDecision::Conflict => Err(CoordinatorError::Conflict),
        DurableDecision::Expired => Err(CoordinatorError::Expired),
        DurableDecision::InvalidState => Err(CoordinatorError::InvalidState),
    }
}

fn worker_error(error: WorkerError) -> CoordinatorError {
    CoordinatorError::Failure(MutationFailure::from_worker(&error))
}

fn session_matches(
    connection: &Connection,
    session_id: &str,
    secret: &[u8],
    now: i64,
    principal: &str,
) -> bool {
    validate_session(connection, session_id, secret, now)
        .is_some_and(|(account_id, _, _)| account_id == principal)
}

type MutationResultRow = (
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<Vec<u8>>,
);

fn mutation_result(
    connection: &Connection,
    principal: &str,
    mutation_id: &str,
    scope: &str,
) -> rusqlite::Result<Option<MutationResult>> {
    let row: Option<MutationResultRow> = connection.query_row(
        "SELECT m.state,m.result_revision_id,r.entry_id,r.length,r.sha256 FROM mutation m LEFT JOIN revision r ON r.revision_id=m.result_revision_id AND r.actor_account_id=m.principal_account_id WHERE m.principal_account_id=?1 AND m.mutation_id=?2 AND m.scope=?3",
        params![principal, mutation_id, scope], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).optional()?;
    Ok(row.map(
        |(state, revision_id, entry_id, length, digest)| match state.as_str() {
            "admitted" | "publishing" => MutationResult::Pending,
            "rejected" => MutationResult::Rejected,
            "committed" => match (revision_id, entry_id, length, digest) {
                (Some(revision_id), Some(entry_id), Some(length), Some(digest)) => {
                    match (<[u8; 32]>::try_from(digest), u64::try_from(length)) {
                        (Ok(sha256), Ok(length)) => MutationResult::Committed(CommittedResult {
                            revision_id,
                            entry_id,
                            length,
                            sha256,
                        }),
                        _ => MutationResult::Unknown,
                    }
                }
                _ => MutationResult::Unknown,
            },
            _ => MutationResult::Unknown,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{
            ids::{MutationScope, MutationTicket, PrincipalId, TicketKeyId, TicketSigningKey},
            mutation::{CommittedResult, MutationResult},
        },
        persistence::{
            schema,
            sqlite::{self, SqliteWorkers, WorkerConfig},
        },
    };
    use rusqlite::{Connection, params};
    use std::{
        fs::{self, OpenOptions},
        num::NonZeroUsize,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    const PRINCIPAL: &str = "11111111111111111111111111111111";
    const SESSION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SECRET: &[u8; 32] = b"0123456789abcdef0123456789abcdef";
    const SCOPE: &str = "upload_create";
    const NOW: i64 = 10;

    struct TestDatabase {
        directory: PathBuf,
        workers: SqliteWorkers,
        coordinator: MutationCoordinator,
    }

    impl TestDatabase {
        fn create(writer_queue_capacity: usize) -> Self {
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random).unwrap();
            let token = random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let directory = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!("sylux-coordinator-{token}"));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            let database = directory.join("metadata.sqlite3");
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&database)
                .unwrap();
            let connection = sqlite::open_writer(&database, Duration::from_millis(500)).unwrap();
            schema::apply_migrations(&connection).unwrap();
            seed_account_and_session(&connection);
            drop(connection);
            let workers = SqliteWorkers::open(
                &database,
                WorkerConfig {
                    busy_timeout: Duration::from_millis(500),
                    writer_queue_capacity: NonZeroUsize::new(writer_queue_capacity).unwrap(),
                    reader_queue_capacity: NonZeroUsize::new(2).unwrap(),
                },
            )
            .unwrap();
            let coordinator = MutationCoordinator::new(workers.clone());
            Self {
                directory,
                workers,
                coordinator,
            }
        }

        async fn cleanup(self) {
            self.workers.shutdown().await.unwrap();
            for name in [
                "metadata.sqlite3-wal",
                "metadata.sqlite3-shm",
                "metadata.sqlite3",
            ] {
                match fs::remove_file(self.directory.join(name)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => panic!("test database cleanup failed: {error}"),
                }
            }
            fs::remove_dir(self.directory).unwrap();
        }
    }

    fn seed_account_and_session(connection: &Connection) {
        connection
            .execute(
                "INSERT INTO account(account_id,handle,role,enabled,credential_epoch)
             VALUES(?1,'alice','user',1,0)",
                [PRINCIPAL],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,
             created_at,idle_expires_at,absolute_expires_at,revoked)
             VALUES(?3,?1,0,?2,?2,0,100,200,0)",
                params![
                    PRINCIPAL,
                    crate::domain::authz::verifier(SECRET).as_slice(),
                    SESSION
                ],
            )
            .unwrap();
    }

    fn ticket(expires_at: i64) -> MutationTicket {
        MutationTicket::issue(
            PrincipalId::parse(PRINCIPAL).unwrap(),
            MutationScope::parse(SCOPE).unwrap(),
            1,
            expires_at,
            0,
            &TicketSigningKey::new(TicketKeyId::parse("test-key").unwrap(), [7; 32]),
        )
        .unwrap()
    }

    async fn admit(coordinator: &MutationCoordinator, ticket: &MutationTicket) -> MutationResult {
        coordinator
            .admit_durable(ticket, [9; 32], SESSION, SECRET, NOW)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn scoped_admission_replays_exact_fingerprint_and_rejects_changed_fingerprint() {
        let db = TestDatabase::create(8);
        let ticket = ticket(20);
        assert_eq!(
            admit(&db.coordinator, &ticket).await,
            MutationResult::Pending
        );
        assert_eq!(
            admit(&db.coordinator, &ticket).await,
            MutationResult::Pending
        );
        assert_eq!(
            db.coordinator
                .admit_durable(&ticket, [8; 32], SESSION, SECRET, NOW)
                .await,
            Err(CoordinatorError::Conflict),
        );
        let mutation_id = ticket.mutation_id().as_str().to_owned();
        let count: i64 = db
            .workers
            .read(move |connection| {
                connection.query_row(
                    "SELECT count(*) FROM mutation WHERE principal_account_id=?1 AND mutation_id=?2",
                    params![PRINCIPAL, mutation_id], |row| row.get(0),
        )
            })
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "replay/mismatch must not create or evict scoped records"
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn mutation_and_revision_result_commit_atomically_and_replay_is_read_only() {
        let db = TestDatabase::create(8);
        let ticket = ticket(20);
        assert_eq!(
            admit(&db.coordinator, &ticket).await,
            MutationResult::Pending
        );
        let permit = match db
            .coordinator
            .begin_publication(&ticket, SESSION, SECRET, NOW)
            .await
            .unwrap()
        {
            PublicationDecision::Ready(permit) => permit,
            PublicationDecision::Existing(result) => {
                panic!("new admission was not ready: {result:?}")
            }
        };

        let call_count = Arc::new(AtomicUsize::new(0));
        let observed = call_count.clone();
        let operation = ticket.scoped_id().clone();
        let committed = db.coordinator.commit(permit, SESSION, SECRET, NOW, move |tx| {
            observed.fetch_add(1, Ordering::SeqCst);
            tx.execute(
                "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
                 VALUES('22222222222222222222222222222222',?1,NULL,'folder','','',1)", [PRINCIPAL])?;
            tx.execute(
                "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
                 VALUES('33333333333333333333333333333333',?1,'22222222222222222222222222222222','file','file',x'66696c65',1)", [PRINCIPAL])?;
            tx.execute(
                "INSERT INTO blob(blob_id,shard,length,sha256) VALUES('44444444444444444444444444444444','44',3,?1)",
                [vec![5_u8; 32]])?;
            tx.execute(
                "INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id)
                 VALUES('55555555555555555555555555555555','33333333333333333333333333333333',
                 '44444444444444444444444444444444',3,?1,10,?2)", params![vec![5_u8; 32], PRINCIPAL])?;
            tx.execute("UPDATE entry SET current_revision_id='55555555555555555555555555555555' WHERE entry_id='33333333333333333333333333333333'", [])?;
            tx.execute("UPDATE entry SET generation=generation+1 WHERE entry_id='22222222222222222222222222222222'", [])?;
            Ok("55555555555555555555555555555555".to_owned())
        }).await.unwrap();
        let expected = MutationResult::Committed(CommittedResult {
            revision_id: "55555555555555555555555555555555".to_owned(),
            entry_id: "33333333333333333333333333333333".to_owned(),
            length: 3,
            sha256: [5; 32],
        });
        assert_eq!(committed, expected);
        assert_eq!(
            db.coordinator
                .status(
                    &operation,
                    &MutationScope::parse(SCOPE).unwrap(),
                    SESSION,
                    SECRET,
                    11
                )
                .await
                .unwrap(),
            expected
        );

        assert!(matches!(
            db.coordinator
                .begin_publication(&ticket, SESSION, SECRET, 11)
                .await
                .unwrap(),
            PublicationDecision::Existing(MutationResult::Committed(_)),
        ));
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "resolved status replay must not issue another publication permit"
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn missing_result_revision_rolls_back_effects_and_keeps_unresolved_record() {
        let db = TestDatabase::create(8);
        let ticket = ticket(20);
        admit(&db.coordinator, &ticket).await;
        let permit = match db
            .coordinator
            .begin_publication(&ticket, SESSION, SECRET, NOW)
            .await
            .unwrap()
        {
            PublicationDecision::Ready(permit) => permit,
            PublicationDecision::Existing(result) => {
                panic!("new admission was not ready: {result:?}")
            }
        };
        let result = db.coordinator.commit(permit, SESSION, SECRET, NOW, |tx| {
            tx.execute("INSERT INTO event(actor_account_id,action,occurred_at,outcome) VALUES(?1,'test.effect',10,'ok')", [PRINCIPAL])?;
            Ok("66666666666666666666666666666666".to_owned())
        }).await;
        assert!(matches!(
            result,
            Err(CoordinatorError::Failure(
                MutationFailure::DefiniteRejection
            ))
        ));
        let operation = ticket.scoped_id().clone();
        let mutation_id = operation.mutation_id().as_str().to_owned();
        let (events, state, result_revision): (i64, String, Option<String>) = db
            .workers
            .read(move |connection| {
                Ok((
                    connection.query_row(
                        "SELECT count(*) FROM event WHERE action='test.effect'",
                        [],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT state FROM mutation WHERE mutation_id=?1",
                        [&mutation_id],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT result_revision_id FROM mutation WHERE mutation_id=?1",
                        [&mutation_id],
                        |row| row.get(0),
                    )?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(
            events, 0,
            "domain-side SQL must roll back with a missing result"
        );
        assert_eq!(state, "publishing");
        assert_eq!(result_revision, None);
        assert_eq!(
            db.coordinator
                .status(
                    &operation,
                    &MutationScope::parse(SCOPE).unwrap(),
                    SESSION,
                    SECRET,
                    11
                )
                .await
                .unwrap(),
            MutationResult::Pending
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn result_lookup_rechecks_current_session_authority() {
        let db = TestDatabase::create(8);
        let ticket = ticket(20);
        admit(&db.coordinator, &ticket).await;
        db.workers
            .transact(|tx| {
                tx.execute(
                    "UPDATE session SET revoked=1 WHERE session_id=?1",
                    [SESSION],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            db.coordinator
                .status(
                    ticket.scoped_id(),
                    &MutationScope::parse(SCOPE).unwrap(),
                    SESSION,
                    SECRET,
                    NOW + 1,
                )
                .await,
            Err(CoordinatorError::InvalidSession),
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn full_writer_queue_rejects_admission_without_evicting_existing_work() {
        let db = TestDatabase::create(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let workers = db.workers.clone();
        let running = tokio::spawn(async move {
            workers
                .transact(move |_| {
                    let _ = started_tx.send(());
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        started_rx.await.unwrap();
        let queued_workers = db.workers.clone();
        let mut queued = Box::pin(queued_workers.transact(|_| Ok(())));
        tokio::select! {
            biased;
            result = &mut queued => panic!("queued transaction completed unexpectedly: {result:?}"),
            () = tokio::task::yield_now() => {}
        }
        let denied = db
            .coordinator
            .admit_durable(&ticket(20), [9; 32], SESSION, SECRET, NOW)
            .await;
        assert!(matches!(
            denied,
            Err(CoordinatorError::Failure(
                MutationFailure::AdmissionUnavailable
            ))
        ));
        release_tx.send(()).unwrap();
        running.await.unwrap().unwrap();
        queued.await.unwrap();
        let count: i64 = db
            .workers
            .read(|connection| {
                connection.query_row("SELECT count(*) FROM mutation", [], |row| row.get(0))
            })
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "saturation rejects the new admission and leaves queued work intact"
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn committed_result_linked_to_foreign_revision_is_unknown() {
        let db = TestDatabase::create(8);
        let ticket = ticket(20);
        admit(&db.coordinator, &ticket).await;

        let mutation_id = ticket.mutation_id().as_str().to_owned();
        db.workers
            .transact(move |tx| {
                tx.execute(
                    "INSERT INTO account(account_id,handle,role,enabled,credential_epoch)
                     VALUES('eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee','bob','user',1,0)",
                    [],
                )?;
                tx.execute(
                    "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
                     VALUES('66666666666666666666666666666666','eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',NULL,'folder','','',1)",
                    [],
                )?;
                tx.execute(
                    "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
                     VALUES('77777777777777777777777777777777','eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee','66666666666666666666666666666666','file','secret',x'736563726574',1)",
                    [],
                )?;
                tx.execute(
                    "INSERT INTO blob(blob_id,shard,length,sha256)
                     VALUES('88888888888888888888888888888888','88',6,?1)",
                    [vec![6_u8; 32]],
                )?;
                tx.execute(
                    "INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id)
                     VALUES('99999999999999999999999999999999','77777777777777777777777777777777',
                     '88888888888888888888888888888888',6,?1,10,'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee')",
                    [vec![6_u8; 32]],
                )?;
                tx.execute(
                    "UPDATE entry SET current_revision_id='99999999999999999999999999999999'
                     WHERE entry_id='77777777777777777777777777777777'",
                    [],
                )?;
                tx.execute(
                    "UPDATE mutation SET state='committed',result_revision_id='99999999999999999999999999999999'
                     WHERE mutation_id=?1",
                    [mutation_id],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        assert_eq!(
            db.coordinator
                .status(
                    ticket.scoped_id(),
                    &MutationScope::parse(SCOPE).unwrap(),
                    SESSION,
                    SECRET,
                    NOW + 1,
                )
                .await
                .unwrap(),
            MutationResult::Unknown,
            "result lookup must not disclose another principal's revision"
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn unresolved_records_remain_pending_after_ticket_expiry() {
        let db = TestDatabase::create(8);
        let ticket = ticket(20);
        let operation = ticket.scoped_id().clone();
        assert_eq!(
            db.coordinator
                .status(
                    &operation,
                    &MutationScope::parse(SCOPE).unwrap(),
                    SESSION,
                    SECRET,
                    NOW
                )
                .await
                .unwrap(),
            MutationResult::Unknown,
        );
        assert_eq!(
            admit(&db.coordinator, &ticket).await,
            MutationResult::Pending
        );
        let permit = match db
            .coordinator
            .begin_publication(&ticket, SESSION, SECRET, NOW)
            .await
            .unwrap()
        {
            PublicationDecision::Ready(permit) => permit,
            PublicationDecision::Existing(result) => {
                panic!("new admission was not ready: {result:?}")
            }
        };
        assert!(matches!(
            db.coordinator
                .begin_publication(&ticket, SESSION, SECRET, NOW)
                .await
                .unwrap(),
            PublicationDecision::Existing(MutationResult::Pending),
        ));
        assert_eq!(
            db.coordinator
                .status(
                    &operation,
                    &MutationScope::parse(SCOPE).unwrap(),
                    SESSION,
                    SECRET,
                    30
                )
                .await
                .unwrap(),
            MutationResult::Pending
        );
        let count: i64 = db
            .workers
            .read(|connection| {
                connection.query_row("SELECT count(*) FROM mutation", [], |row| row.get(0))
            })
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "pending records are not removed or evicted when the ticket expires"
        );
        drop(permit);
        db.cleanup().await;
    }
}
