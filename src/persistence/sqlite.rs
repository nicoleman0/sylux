//! Verified SQLite connection policy (s1-design-review §4).
//!
//! Every applicable connection — the single dedicated writer and every
//! bounded reader — is configured with WAL journaling, FULL synchronous
//! mode, enforced foreign keys and a finite busy timeout, and each setting is
//! then read back and compared before the connection is returned. SQLite
//! silently ignores unknown PRAGMAs, so setting alone is not a policy; the
//! read-back is the verification. A connection whose read-back disagrees is
//! discarded and no schema change can ever run through it.

use std::{
    any::Any,
    num::NonZeroUsize,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use rusqlite::{Connection, DropBehavior, OpenFlags, Transaction, TransactionBehavior};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

/// Required persistent journal mode.
pub const REQUIRED_JOURNAL_MODE: &str = "wal";
/// `PRAGMA synchronous` read-back value for FULL.
pub const REQUIRED_SYNCHRONOUS: i64 = 2;
/// `PRAGMA foreign_keys` read-back value for ON.
pub const REQUIRED_FOREIGN_KEYS: i64 = 1;
/// Default finite busy wait (design §7 recommendation: five seconds).
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// Which product role a connection plays. The writer is opened read-write
/// without `CREATE` (the database must already exist and be bound); readers
/// are opened read-only and cannot perform domain writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionRole {
    /// The single serialized writer under the mutation coordinator.
    Writer,
    /// A bounded read-only pool connection.
    Reader,
}

/// The per-connection policy as SQLite actually reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyReadback {
    pub journal_mode: String,
    pub synchronous: i64,
    pub foreign_keys: i64,
    pub busy_timeout_ms: i64,
}

/// Why a connection could not be opened under the required policy.
#[derive(Debug)]
pub enum ConnectionError {
    /// SQLite refused to open the file at all (missing, not a database, or
    /// unusable). The message is SQLite's own; it carries no secret values.
    Open(String),
    /// The read-back did not match the required policy. The connection is
    /// discarded; this is a fail-closed refusal, never a fallback.
    Policy(PolicyReadback),
}

impl ConnectionError {
    /// Redacted machine reason for contract events and diagnostics.
    pub fn reason(&self) -> String {
        match self {
            Self::Open(message) => format!("database_open_failed: {message}"),
            Self::Policy(actual) => format!(
                "policy_unverified: journal={:?} synchronous={} foreign_keys={} busy_timeout_ms={}",
                actual.journal_mode,
                actual.synchronous,
                actual.foreign_keys,
                actual.busy_timeout_ms
            ),
        }
    }
}

fn required_readback(busy_timeout_ms: i64) -> PolicyReadback {
    PolicyReadback {
        journal_mode: REQUIRED_JOURNAL_MODE.to_owned(),
        synchronous: REQUIRED_SYNCHRONOUS,
        foreign_keys: REQUIRED_FOREIGN_KEYS,
        busy_timeout_ms,
    }
}

/// Open a connection in the given role and verify its whole policy by
/// read-back. The database file must already exist: neither role uses
/// `SQLITE_OPEN_CREATE`.
pub fn open_connection(
    path: &Path,
    role: ConnectionRole,
    busy_timeout: Duration,
) -> Result<Connection, ConnectionError> {
    let flags = match role {
        ConnectionRole::Writer => {
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX
        }
        ConnectionRole::Reader => {
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
        }
    };
    let connection = Connection::open_with_flags(path, flags)
        .map_err(|error| ConnectionError::Open(error.to_string()))?;
    configure_and_verify(&connection, role, busy_timeout)?;
    Ok(connection)
}

/// Convenience wrapper for the dedicated writer.
pub fn open_writer(path: &Path, busy_timeout: Duration) -> Result<Connection, ConnectionError> {
    open_connection(path, ConnectionRole::Writer, busy_timeout)
}

/// Convenience wrapper for bounded read-only pool connections.
pub fn open_reader(path: &Path, busy_timeout: Duration) -> Result<Connection, ConnectionError> {
    open_connection(path, ConnectionRole::Reader, busy_timeout)
}

fn configure_and_verify(
    connection: &Connection,
    role: ConnectionRole,
    busy_timeout: Duration,
) -> Result<(), ConnectionError> {
    let busy_ms = i64::try_from(busy_timeout.as_millis())
        .map_err(|_| ConnectionError::Open("busy timeout out of range".to_owned()))?;
    if busy_ms <= 0 {
        return Err(ConnectionError::Open(
            "busy timeout must be finite and positive".to_owned(),
        ));
    }
    // Per-connection settings. `PRAGMA synchronous = FULL` and
    // `PRAGMA foreign_keys = ON` return no rows.
    connection
        .execute("PRAGMA synchronous = FULL", [])
        .map_err(|error| ConnectionError::Open(error.to_string()))?;
    connection
        .execute("PRAGMA foreign_keys = ON", [])
        .map_err(|error| ConnectionError::Open(error.to_string()))?;
    // `PRAGMA busy_timeout` returns the new value; the integer is formatted
    // from a validated i64, never user input.
    connection
        .query_row(&format!("PRAGMA busy_timeout = {busy_ms}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| ConnectionError::Open(error.to_string()))?;
    // Journal mode is persistent. The writer enforces WAL (the statement
    // returns the resulting mode); a reader can only verify that the database
    // already is in WAL mode.
    if role == ConnectionRole::Writer {
        connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| ConnectionError::Open(error.to_string()))?;
    }
    // One read-back covers the whole policy; any silent ignore or refused
    // setting shows up here and fails the connection closed.
    let actual =
        read_back_policy(connection).map_err(|error| ConnectionError::Open(error.to_string()))?;
    if actual != required_readback(busy_ms) {
        return Err(ConnectionError::Policy(actual));
    }
    Ok(())
}

/// Read the whole policy back from a connection exactly as SQLite reports it.
pub fn read_back_policy(connection: &Connection) -> Result<PolicyReadback, rusqlite::Error> {
    Ok(PolicyReadback {
        journal_mode: connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?,
        synchronous: connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?,
        foreign_keys: connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?,
        busy_timeout_ms: connection.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?,
    })
}

/// Verify the policy without knowing the configured busy timeout exactly.
/// The timeout must still be finite and positive. Used as the migration gate:
/// no schema change may run unless every required setting passes.
pub fn verify_static_policy(connection: &Connection) -> Result<(), PolicyReadback> {
    let actual = read_back_policy(connection).map_err(|_| PolicyReadback {
        journal_mode: String::new(),
        synchronous: -1,
        foreign_keys: -1,
        busy_timeout_ms: -1,
    })?;
    if actual.journal_mode == REQUIRED_JOURNAL_MODE
        && actual.synchronous == REQUIRED_SYNCHRONOUS
        && actual.foreign_keys == REQUIRED_FOREIGN_KEYS
        && actual.busy_timeout_ms > 0
    {
        Ok(())
    } else {
        Err(actual)
    }
}

/// Checkpoint the WAL into the main database and truncate it. Used at the end
/// of host initialization so the bound state is durable in the main database
/// file; this is a normal SQLite operation, never a manual WAL deletion.
pub fn wal_checkpoint_truncate(connection: &Connection) -> Result<(), ConnectionError> {
    let busy: i64 = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .map_err(|error| ConnectionError::Open(error.to_string()))?;
    if busy != 0 {
        return Err(ConnectionError::Open(
            "checkpoint reported a busy database".to_owned(),
        ));
    }
    Ok(())
}

/// Default number of waiting jobs admitted to each worker, excluding the job
/// currently executing. This is a bounded-resource setting, not a throughput
/// or product-limit promise.
pub const DEFAULT_WORKER_QUEUE_CAPACITY: NonZeroUsize = NonZeroUsize::new(32).unwrap();

/// Configuration for the two owned connection workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerConfig {
    pub busy_timeout: Duration,
    pub writer_queue_capacity: NonZeroUsize,
    pub reader_queue_capacity: NonZeroUsize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
            writer_queue_capacity: DEFAULT_WORKER_QUEUE_CAPACITY,
            reader_queue_capacity: DEFAULT_WORKER_QUEUE_CAPACITY,
        }
    }
}

/// The outcome of a transaction that was submitted to the dedicated writer.
#[derive(Debug)]
pub enum TransactionError {
    /// No commit occurred and rollback completed (or no transaction began).
    Definite(rusqlite::Error),
    /// SQLite still reports an active transaction. The writer is discarded.
    Active(rusqlite::Error),
    /// COMMIT returned an error and its outcome cannot be established locally.
    /// The writer is discarded; callers must reconcile by durable read-back.
    Unknown(rusqlite::Error),
}

impl TransactionError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Definite(_) => "transaction_definite_rejection",
            Self::Active(_) => "transaction_still_active",
            Self::Unknown(_) => "transaction_commit_unknown",
        }
    }
}

#[derive(Debug)]
pub enum WorkerError {
    /// The configured waiting-job capacity has been reached. No job was run or
    /// evicted, and the caller can apply its own admission policy.
    QueueFull,
    /// The worker has shut down or the connection was discarded.
    Closed,
    /// A blocking worker panicked. Its connection is not reused.
    Panicked,
    /// A bounded read or connection-inspection operation failed.
    Query(rusqlite::Error),
    /// A writer transaction has an explicit durability outcome.
    Transaction(TransactionError),
    /// Internal type erasure failed; no SQL outcome is inferred.
    InvalidResponse,
}

impl WorkerError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::QueueFull => "sqlite_worker_queue_full",
            Self::Closed => "sqlite_worker_closed",
            Self::Panicked => "sqlite_worker_panicked",
            Self::Query(_) => "sqlite_query_failed",
            Self::Transaction(outcome) => outcome.reason(),
            Self::InvalidResponse => "sqlite_worker_invalid_response",
        }
    }
}

type ErasedResult = Result<Box<dyn Any + Send>, WorkerError>;

struct Job {
    /// Returns true when the connection must be discarded after responding.
    run: Box<dyn FnOnce(&mut Connection) -> bool + Send>,
}

enum WorkerMessage {
    Run(Job),
    Shutdown,
}

struct WorkerHandle {
    sender: mpsc::Sender<WorkerMessage>,
    join: Mutex<Option<JoinHandle<()>>>,
    closing: AtomicBool,
}

impl WorkerHandle {
    fn start(connection: Connection, queue_capacity: NonZeroUsize) -> Arc<Self> {
        let (sender, mut receiver) = mpsc::channel(queue_capacity.get());
        let join = tokio::task::spawn_blocking(move || {
            let mut connection = connection;
            while let Some(message) = receiver.blocking_recv() {
                match message {
                    WorkerMessage::Run(job) => {
                        if (job.run)(&mut connection) {
                            break;
                        }
                    }
                    WorkerMessage::Shutdown => break,
                }
            }
        });
        Arc::new(Self {
            sender,
            join: Mutex::new(Some(join)),
            closing: AtomicBool::new(false),
        })
    }

    async fn submit<T, F>(&self, operation: F) -> Result<T, WorkerError>
    where
        F: FnOnce(&mut Connection) -> (Result<T, WorkerError>, bool) + Send + 'static,
        T: Send + 'static,
    {
        if self.closing.load(Ordering::Acquire) {
            return Err(WorkerError::Closed);
        }
        let (response, receive) = oneshot::channel::<ErasedResult>();
        let job = Job {
            run: Box::new(move |connection| {
                let (result, discard) = operation(connection);
                let result = result.map(|value| Box::new(value) as Box<dyn Any + Send>);
                let _ = response.send(result);
                discard
            }),
        };
        match self.sender.try_send(WorkerMessage::Run(job)) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => return Err(WorkerError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(WorkerError::Closed),
        }
        let result = receive.await.map_err(|_| WorkerError::Closed)??;
        result
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| WorkerError::InvalidResponse)
    }

    fn close_admission(&self) {
        self.closing.store(true, Ordering::Release);
    }

    async fn shutdown(&self) -> Result<(), WorkerError> {
        self.close_admission();
        // This bounded send drains work admitted before the shutdown marker.
        // If the worker already stopped, joining below still reports its state.
        let _ = self.sender.send(WorkerMessage::Shutdown).await;
        let join = self.join.lock().map_err(|_| WorkerError::Closed)?.take();
        if let Some(join) = join {
            join.await.map_err(|_| WorkerError::Panicked)?;
        }
        Ok(())
    }
}

/// One dedicated owned writer and one separate reusable read-only reader
/// (the reviewed size-one bounded reader pool). Connections never escape their
/// blocking workers. Every result is owned and sent only after its SQL closure
/// and any transaction have ended, so callers cannot hold a SQL snapshot over
/// network streaming. Read closures must materialize bounded DTOs/pages.
#[derive(Clone)]
pub struct SqliteWorkers {
    writer: Arc<WorkerHandle>,
    reader: Arc<WorkerHandle>,
}

impl SqliteWorkers {
    /// Open writer first (establishing WAL), then the read-only reader. Neither
    /// open uses CREATE; the database must already exist and be bound.
    pub fn open(path: &Path, config: WorkerConfig) -> Result<Self, ConnectionError> {
        let writer = open_writer(path, config.busy_timeout)?;
        Self::from_writer(writer, path, config)
    }

    /// Adopt the verified startup writer without opening a second writer, then
    /// open the sole reader. This is the serving handoff for `BoundDatabase`.
    pub fn from_writer(
        writer: Connection,
        path: &Path,
        config: WorkerConfig,
    ) -> Result<Self, ConnectionError> {
        verify_existing_policy(&writer, config.busy_timeout)?;
        let reader = open_reader(path, config.busy_timeout)?;
        Ok(Self {
            writer: WorkerHandle::start(writer, config.writer_queue_capacity),
            reader: WorkerHandle::start(reader, config.reader_queue_capacity),
        })
    }

    /// Submit a short `BEGIN IMMEDIATE` transaction. Final authorization and
    /// relationship preconditions belong inside this closure, not in a stale
    /// reader snapshot. No filesystem/network operation may run inside it.
    pub async fn transact<T, F>(&self, operation: F) -> Result<T, WorkerError>
    where
        F: for<'connection> FnOnce(&Transaction<'connection>) -> rusqlite::Result<T>
            + Send
            + 'static,
        T: Send + 'static,
    {
        self.writer
            .submit(move |connection| run_transaction(connection, operation))
            .await
    }

    /// Run a writer transaction whose domain rejection must be rolled back
    /// while remaining distinguishable from SQLite/commit uncertainty.
    pub async fn transact_result<T, E, F>(&self, operation: F) -> Result<Result<T, E>, WorkerError>
    where
        F: for<'connection> FnOnce(&Transaction<'connection>) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: Send + 'static,
    {
        self.writer
            .submit(move |connection| run_result_transaction(connection, operation))
            .await
    }

    /// Materialize one bounded owned result on the read-only connection. The
    /// SQL slot and any snapshot end before the returned future resolves.
    pub async fn read<T, F>(&self, operation: F) -> Result<T, WorkerError>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.reader
            .submit(move |connection| (operation(connection).map_err(WorkerError::Query), false))
            .await
    }

    /// Read the policy from the actual live connection selected by `role`, not
    /// from cached startup configuration.
    pub async fn policy_readback(
        &self,
        role: ConnectionRole,
    ) -> Result<PolicyReadback, WorkerError> {
        let worker = match role {
            ConnectionRole::Writer => &self.writer,
            ConnectionRole::Reader => &self.reader,
        };
        worker
            .submit(|connection| {
                (
                    read_back_policy(connection).map_err(WorkerError::Query),
                    false,
                )
            })
            .await
    }

    /// Stop admission, drain work ordered before each shutdown marker and join
    /// both blocking workers. The root ownership guard must remain held until
    /// this completes. There is no implicit reopen or reacquisition.
    pub async fn shutdown(self) -> Result<(), WorkerError> {
        self.writer.close_admission();
        self.reader.close_admission();
        let writer = self.writer.shutdown().await;
        let reader = self.reader.shutdown().await;
        writer.and(reader)
    }
}

fn run_transaction<T, F>(
    connection: &mut Connection,
    operation: F,
) -> (Result<T, WorkerError>, bool)
where
    F: for<'connection> FnOnce(&Transaction<'connection>) -> rusqlite::Result<T>,
{
    let was_autocommit = connection.is_autocommit();
    let mut transaction = match Transaction::new(connection, TransactionBehavior::Immediate) {
        Ok(transaction) => transaction,
        Err(error) => {
            if was_autocommit {
                return (
                    Err(WorkerError::Transaction(TransactionError::Definite(error))),
                    false,
                );
            }
            return (
                Err(WorkerError::Transaction(TransactionError::Active(error))),
                true,
            );
        }
    };

    match operation(&transaction) {
        Ok(value) => {
            // rusqlite otherwise attempts an unobserved rollback when a failed
            // COMMIT is dropped, erasing the active/unknown distinction.
            transaction.set_drop_behavior(DropBehavior::Ignore);
            match transaction.commit() {
                Ok(()) => {
                    debug_assert!(connection.is_autocommit());
                    (Ok(value), false)
                }
                Err(error) => {
                    let active = !connection.is_autocommit();
                    let outcome = if active {
                        TransactionError::Active(error)
                    } else {
                        // Autocommit alone cannot distinguish an acknowledged
                        // COMMIT from an error-path rollback. Never guess.
                        TransactionError::Unknown(error)
                    };
                    (Err(WorkerError::Transaction(outcome)), true)
                }
            }
        }
        Err(error) => {
            // No COMMIT was requested. Make rollback explicit and observed;
            // disable Drop's hidden second attempt so the result is truthful.
            transaction.set_drop_behavior(DropBehavior::Ignore);
            match transaction.rollback() {
                Ok(()) => (
                    Err(WorkerError::Transaction(TransactionError::Definite(error))),
                    false,
                ),
                Err(rollback_error) => {
                    if connection.is_autocommit() {
                        // The work still never reached COMMIT, so rejection is
                        // definite; discard after the abnormal rollback path.
                        (
                            Err(WorkerError::Transaction(TransactionError::Definite(error))),
                            true,
                        )
                    } else {
                        (
                            Err(WorkerError::Transaction(TransactionError::Active(
                                rollback_error,
                            ))),
                            true,
                        )
                    }
                }
            }
        }
    }
}

fn run_result_transaction<T, E, F>(
    connection: &mut Connection,
    operation: F,
) -> (Result<Result<T, E>, WorkerError>, bool)
where
    F: for<'connection> FnOnce(&Transaction<'connection>) -> Result<T, E>,
{
    let was_autocommit = connection.is_autocommit();
    let mut transaction = match Transaction::new(connection, TransactionBehavior::Immediate) {
        Ok(transaction) => transaction,
        Err(error) => {
            let outcome = if was_autocommit {
                TransactionError::Definite(error)
            } else {
                TransactionError::Active(error)
            };
            return (Err(WorkerError::Transaction(outcome)), !was_autocommit);
        }
    };
    match operation(&transaction) {
        Ok(value) => {
            transaction.set_drop_behavior(DropBehavior::Ignore);
            match transaction.commit() {
                Ok(()) => (Ok(Ok(value)), false),
                Err(error) => {
                    let active = !connection.is_autocommit();
                    let outcome = if active {
                        TransactionError::Active(error)
                    } else {
                        TransactionError::Unknown(error)
                    };
                    (Err(WorkerError::Transaction(outcome)), true)
                }
            }
        }
        Err(domain_error) => {
            transaction.set_drop_behavior(DropBehavior::Ignore);
            match transaction.rollback() {
                Ok(()) => (Ok(Err(domain_error)), false),
                Err(_rollback_error) if connection.is_autocommit() => (Ok(Err(domain_error)), true),
                Err(rollback_error) => (
                    Err(WorkerError::Transaction(TransactionError::Active(
                        rollback_error,
                    ))),
                    true,
                ),
            }
        }
    }
}

fn verify_existing_policy(
    connection: &Connection,
    busy_timeout: Duration,
) -> Result<(), ConnectionError> {
    let busy_ms = i64::try_from(busy_timeout.as_millis())
        .map_err(|_| ConnectionError::Open("busy timeout out of range".to_owned()))?;
    if busy_ms <= 0 {
        return Err(ConnectionError::Open(
            "busy timeout must be finite and positive".to_owned(),
        ));
    }
    let actual =
        read_back_policy(connection).map_err(|error| ConnectionError::Open(error.to_string()))?;
    if actual == required_readback(busy_ms) {
        Ok(())
    } else {
        Err(ConnectionError::Policy(actual))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::schema;
    use rusqlite::{ErrorCode, params};
    use std::{
        fs::{self, OpenOptions},
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        path::PathBuf,
    };

    const BUSY: Duration = Duration::from_millis(375);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn create(case: &str) -> (Self, PathBuf) {
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random).unwrap();
            let token = random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let path = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!("sylux-sqlite-worker-{case}-{token}"));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            let database = path.join("metadata.sqlite3");
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&database)
                .unwrap();
            let connection = open_writer(&database, BUSY).unwrap();
            assert_eq!(
                schema::apply_migrations(&connection).unwrap(),
                schema::MigrationOutcome::Applied
            );
            schema::recovery_checks(&connection).unwrap();
            drop(connection);
            (Self(path), database)
        }

        fn cleanup(self) {
            for name in [
                "metadata.sqlite3-wal",
                "metadata.sqlite3-shm",
                "metadata.sqlite3",
            ] {
                match fs::remove_file(self.0.join(name)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => panic!("test database cleanup failed: {error}"),
                }
            }
            fs::remove_dir(&self.0).unwrap();
        }
    }

    fn expected_policy() -> PolicyReadback {
        PolicyReadback {
            journal_mode: REQUIRED_JOURNAL_MODE.to_owned(),
            synchronous: REQUIRED_SYNCHRONOUS,
            foreign_keys: REQUIRED_FOREIGN_KEYS,
            busy_timeout_ms: 375,
        }
    }

    #[test]
    fn migration_gate_rejects_zero_busy_timeout() {
        let (directory, database) = TestDirectory::create("migration-busy-gate");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "PRAGMA synchronous = FULL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 0;",
            )
            .unwrap();
        let actual = read_back_policy(&connection).unwrap();
        assert_eq!(actual.journal_mode, REQUIRED_JOURNAL_MODE);
        assert_eq!(actual.synchronous, REQUIRED_SYNCHRONOUS);
        assert_eq!(actual.foreign_keys, REQUIRED_FOREIGN_KEYS);
        assert_eq!(actual.busy_timeout_ms, 0);
        assert!(
            schema::apply_migrations(&connection).is_err(),
            "migration must reject a connection without finite busy handling"
        );
        drop(connection);
        directory.cleanup();
    }

    #[tokio::test]
    async fn workers_apply_transactions_and_verify_both_connection_policies() {
        let (directory, database) = TestDirectory::create("core");
        let workers = SqliteWorkers::open(
            &database,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();

        for role in [ConnectionRole::Writer, ConnectionRole::Reader] {
            assert_eq!(
                workers.policy_readback(role).await.unwrap(),
                expected_policy()
            );
        }

        workers
            .transact(|transaction| {
                transaction.execute(
                    "INSERT INTO account (account_id, handle, role, enabled, credential_epoch)
                     VALUES (?1, ?2, 'user', 1, 0)",
                    params!["00000000000000000000000000000001", "owner"],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let count = workers
            .read(|connection| {
                connection.query_row("SELECT count(*) FROM account", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(count, 1);

        let error = workers
            .read(|connection| {
                connection.execute(
                    "INSERT INTO account (account_id, handle, role, enabled, credential_epoch)
                     VALUES ('00000000000000000000000000000002', 'other', 'user', 1, 0)",
                    [],
                )
            })
            .await
            .unwrap_err();
        match error {
            WorkerError::Query(error) => {
                assert_eq!(error.sqlite_error_code(), Some(ErrorCode::ReadOnly));
            }
            other => panic!("unexpected reader write error: {other:?}"),
        }

        workers.shutdown().await.unwrap();
        let reopened = SqliteWorkers::open(
            &database,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        assert_eq!(
            reopened
                .policy_readback(ConnectionRole::Reader)
                .await
                .unwrap(),
            expected_policy()
        );
        let count = reopened
            .read(|connection| {
                connection.query_row("SELECT count(*) FROM account", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
        reopened.shutdown().await.unwrap();
        directory.cleanup();
    }

    #[tokio::test]
    async fn domain_rejection_rolls_back_and_keeps_the_writer_usable() {
        let (directory, database) = TestDirectory::create("domain-rejection-rollback");
        let workers = SqliteWorkers::open(
            &database,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        let rejected =
            workers
                .transact_result(|transaction| -> Result<(), &'static str> {
                    transaction.execute(
                "INSERT INTO account (account_id, handle, role, enabled, credential_epoch)
                 VALUES ('00000000000000000000000000000001', 'owner', 'user', 1, 0)",
                [],
            ).map_err(|_| "sql")?;
                    Err("domain rejection")
                })
                .await
                .unwrap();
        assert_eq!(rejected, Err("domain rejection"));
        let count = workers
            .read(|db| {
                db.query_row("SELECT count(*) FROM account", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(count, 0, "domain error must roll back earlier writes");
        workers
            .transact(|transaction| {
                transaction.execute(
                    "INSERT INTO account (account_id, handle, role, enabled, credential_epoch)
                 VALUES ('00000000000000000000000000000002', 'other', 'user', 1, 0)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        workers.shutdown().await.unwrap();
        directory.cleanup();
    }

    #[tokio::test]
    async fn transaction_rolls_back_relationship_violation_as_definite() {
        let (directory, database) = TestDirectory::create("rollback");
        let workers = SqliteWorkers::open(
            &database,
            WorkerConfig {
                busy_timeout: BUSY,
                ..WorkerConfig::default()
            },
        )
        .unwrap();
        workers
            .transact(|transaction| {
                transaction.execute(
                    "INSERT INTO account (account_id, handle, role, enabled, credential_epoch)
                     VALUES ('00000000000000000000000000000001', 'owner', 'user', 1, 0)",
                    [],
                )?;
                transaction.execute(
                    "INSERT INTO entry (
                        entry_id, owner_account_id, parent_id, kind, original_name,
                        name_key, name_key_version
                     ) VALUES (
                        '00000000000000000000000000000003',
                        '00000000000000000000000000000001', NULL, 'folder', '', ?1, 1
                     )",
                    params![Vec::<u8>::new()],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let error = workers
            .transact(|transaction| {
                transaction.execute(
                    "INSERT INTO account (account_id, handle, role, enabled, credential_epoch)
                     VALUES ('00000000000000000000000000000002', 'other', 'user', 1, 0)",
                    [],
                )?;
                transaction.execute(
                    "INSERT INTO entry (
                        entry_id, owner_account_id, parent_id, kind, original_name,
                        name_key, name_key_version
                     ) VALUES (
                        '00000000000000000000000000000004',
                        '00000000000000000000000000000002',
                        '00000000000000000000000000000003',
                        'folder', 'child', ?1, 1
                     )",
                    params![b"child".as_slice()],
                )?;
                Ok(())
            })
            .await
            .unwrap_err();

        let other_accounts = workers
            .read(|connection| {
                connection.query_row(
                    "SELECT count(*) FROM account WHERE handle = 'other'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(other_accounts, 0, "failed transaction must roll back");
        assert!(matches!(
            error,
            WorkerError::Transaction(TransactionError::Definite(_))
        ));
        workers.shutdown().await.unwrap();
        directory.cleanup();
    }

    #[tokio::test]
    async fn reader_admission_is_bounded_without_evicting_waiters() {
        let (directory, database) = TestDirectory::create("bounded-reader");
        let workers = SqliteWorkers::open(
            &database,
            WorkerConfig {
                busy_timeout: BUSY,
                writer_queue_capacity: NonZeroUsize::new(1).unwrap(),
                reader_queue_capacity: NonZeroUsize::new(1).unwrap(),
            },
        )
        .unwrap();

        let (started_sender, started_receiver) = oneshot::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let first_workers = workers.clone();
        let first = tokio::spawn(async move {
            first_workers
                .read(move |connection| {
                    let _ = started_sender.send(());
                    release_receiver.recv().unwrap();
                    connection.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                })
                .await
        });
        started_receiver.await.unwrap();

        let mut queued =
            Box::pin(workers.read(|connection| {
                connection.query_row("SELECT 2", [], |row| row.get::<_, i64>(0))
            }));
        tokio::select! {
            biased;
            result = &mut queued => panic!("queued read finished before its turn: {result:?}"),
            () = tokio::task::yield_now() => {}
        }
        let third = tokio::time::timeout(
            Duration::from_millis(50),
            workers.read(|connection| {
                connection.query_row("SELECT 3", [], |row| row.get::<_, i64>(0))
            }),
        )
        .await;

        release_sender.send(()).unwrap();
        assert_eq!(first.await.unwrap().unwrap(), 1);
        assert_eq!(queued.await.unwrap(), 2);
        workers.shutdown().await.unwrap();
        directory.cleanup();

        match third {
            Ok(Err(WorkerError::QueueFull)) => {}
            other => panic!("full reader queue did not reject cleanly: {other:?}"),
        }
    }

    #[tokio::test]
    async fn shutdown_closes_admission_before_draining() {
        let (directory, database) = TestDirectory::create("shutdown-admission");
        let workers = SqliteWorkers::open(
            &database,
            WorkerConfig {
                busy_timeout: BUSY,
                writer_queue_capacity: NonZeroUsize::new(1).unwrap(),
                reader_queue_capacity: NonZeroUsize::new(1).unwrap(),
            },
        )
        .unwrap();

        let (started_sender, started_receiver) = oneshot::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let first_workers = workers.clone();
        let first = tokio::spawn(async move {
            first_workers
                .read(move |connection| {
                    let _ = started_sender.send(());
                    release_receiver.recv().unwrap();
                    connection.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                })
                .await
        });
        started_receiver.await.unwrap();

        let shutdown_workers = workers.clone();
        let shutdown = tokio::spawn(async move { shutdown_workers.shutdown().await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while workers.reader.sender.capacity() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let rejected = workers
            .read(|connection| connection.query_row("SELECT 2", [], |row| row.get::<_, i64>(0)))
            .await;

        release_sender.send(()).unwrap();
        assert_eq!(first.await.unwrap().unwrap(), 1);
        shutdown.await.unwrap().unwrap();
        directory.cleanup();
        assert!(matches!(rejected, Err(WorkerError::Closed)));
    }
}
