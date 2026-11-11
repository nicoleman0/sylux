//! Sylux entry point.
//!
//! `sylux init` / `sylux serve` implement the reviewed root-binding,
//! ownership and startup flow (s1-design-review §3):
//!
//! ```text
//! ownership.lock_acquire -> ownership.acquired -> (manifest)
//! -> database.open -> binding.verified -> migration.start
//! -> recovery.start -> listener.bind
//! ```
//!
//! Every refusal is fail-closed: a refused process exits nonzero, never
//! reaches migration/recovery/listener, and leaves the root byte-identical
//! (refused database connections are closed cleanly so SQLite's WAL siblings
//! are checkpointed away, not left behind or deleted manually).
//!
//! Running the binary with no subcommand retains the H0 static-asset
//! scaffold surface pinned by scripts/test-s1-runner.py (DEFAULT_SURFACE
//! row). That path binds no storage root, owns no product route, and is
//! replaced by the qualified product surface in later S1 tasks.

use std::{env, ffi::OsString, net::SocketAddr, process::ExitCode, time::Duration};

use axum::{Router, routing::get};
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};

use sylux::{
    application_name,
    config::{self, CliConfig, CommandKind},
    http::{self, AuthState},
    persistence::ownership,
    persistence::sqlite::{DEFAULT_BUSY_TIMEOUT, SqliteWorkers, WorkerConfig},
    persistence::{contract_event, contract_event_reason, reconcile, root, schema},
};

#[tokio::main]
async fn main() -> ExitCode {
    let arguments: Vec<OsString> = env::args_os().collect();
    if arguments.len() < 2 {
        return scaffold_only().await;
    }
    match config::parse(arguments.into_iter().skip(1)) {
        Ok(configuration) => match configuration.command {
            CommandKind::Init => run_init(&configuration),
            CommandKind::Serve => run_serve(configuration).await,
        },
        Err(error) => {
            eprintln!("sylux: {}", error.reason());
            ExitCode::from(2)
        }
    }
}

/// Explicit host initialization: creates the bound state on an empty
/// dedicated root and exits. It never binds a listener, and an existing
/// installation is refused rather than reset.
fn run_init(configuration: &CliConfig) -> ExitCode {
    match root::initialize_empty_root(&configuration.root, DEFAULT_BUSY_TIMEOUT) {
        Ok(_) => {
            contract_event("init.complete");
            ExitCode::SUCCESS
        }
        Err(error) => {
            // A held lock already emitted ownership.refused inside the
            // ownership flow; classify everything else as an init refusal.
            if !matches!(
                error,
                root::InitError::Ownership(ownership::OwnershipError::LockHeld)
            ) {
                contract_event_reason("init.refused", error.reason());
            }
            ExitCode::FAILURE
        }
    }
}

/// Serve the bound installation. Ownership first, then binding, then
/// migration, recovery, and only then the listener.
async fn run_serve(configuration: CliConfig) -> ExitCode {
    // 1. Lifetime exclusion before anything touches the root.
    contract_event("ownership.lock_acquire");
    let guard = match ownership::acquire_writer_lock(&configuration.root) {
        Ok(guard) => guard,
        Err(error) => {
            match error {
                ownership::OwnershipError::LockHeld => {
                    contract_event_reason("ownership.refused", error.reason())
                }
                // A root or lock file that is missing, aliased or not private
                // is not a bound installation: refuse like any other binding
                // failure, before the database is ever opened.
                _ => contract_event_reason("binding.refused", error.reason()),
            }
            return ExitCode::FAILURE;
        }
    };
    contract_event("ownership.acquired");

    // 2. Manifest.
    let manifest = match root::read_manifest(&configuration.root) {
        Ok(manifest) => manifest,
        Err(error) => {
            contract_event_reason("binding.refused", error.reason());
            return ExitCode::FAILURE;
        }
    };

    // 3. Protected material precedes SQLite: an early refusal must not create
    // WAL/SHM siblings in an otherwise unchanged rejected installation.
    if let Err(error) = root::verify_protected_material(&configuration.root) {
        contract_event_reason("binding.refused", error.reason());
        return ExitCode::FAILURE;
    }

    // 4. Bound database under the verified connection policy.
    contract_event("database.open");
    let bound = match root::open_bound_database(&configuration.root, DEFAULT_BUSY_TIMEOUT) {
        Ok(bound) => bound,
        Err(error) => {
            contract_event_reason("binding.refused", error.reason());
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = root::verify_binding(&bound, &configuration.root, &manifest) {
        // Close cleanly before announcing the refusal: the checkpointing
        // close leaves bound state byte-identical (WAL siblings are removed
        // by SQLite itself, never by us).
        drop(bound);
        contract_event_reason("binding.refused", error.reason());
        return ExitCode::FAILURE;
    }
    contract_event("binding.verified");

    // 5. Migration (no-op at the supported version, verified by inspection).
    contract_event("migration.start");
    if let Err(error) = schema::apply_migrations(&bound.connection) {
        drop(bound);
        contract_event_reason("startup.failed", &error.reason());
        return ExitCode::FAILURE;
    }

    // 6. Fail-closed recovery checks before readiness.
    contract_event("recovery.start");
    if let Err(error) = schema::recovery_checks(&bound.connection) {
        drop(bound);
        contract_event_reason("startup.failed", &error.reason());
        return ExitCode::FAILURE;
    }

    // 7. Reconciliation and listener setup retain ownership throughout.
    match serve_listener(configuration, guard, bound).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            contract_event_reason("startup.failed", reason);
            ExitCode::FAILURE
        }
    }
}

/// Bind the TLS listener and serve until terminated. On shutdown, admission
/// closes first, the outer server task is joined, the local database handle is
/// closed, and the ownership descriptor remains locked until process teardown
/// has dropped all per-connection workers and their database handles.
async fn serve_listener(
    configuration: CliConfig,
    guard: ownership::OwnershipGuard,
    mut bound: root::BoundDatabase,
) -> Result<(), &'static str> {
    let connection = std::mem::replace(
        &mut bound.connection,
        rusqlite::Connection::open_in_memory().map_err(|_| "database_detach_failed")?,
    );
    let database_path = configuration
        .root
        .join(root::METADATA_DIR)
        .join(root::DB_NAME);
    let workers = SqliteWorkers::from_writer(connection, &database_path, WorkerConfig::default())
        .map_err(|_| "database_workers_unavailable")?;
    let recovery = reconcile::reconcile(&workers, &configuration.root).await;
    if !matches!(recovery, Ok(report) if report.ready) {
        let _ = workers.shutdown().await;
        return Err("recovery_required");
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls_cert = configuration
        .tls_cert
        .as_deref()
        .ok_or("tls_required_for_serve")?;
    let tls_key = configuration
        .tls_key
        .as_deref()
        .ok_or("tls_required_for_serve")?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(tls_cert, tls_key)
        .await
        .map_err(|_| "tls_identity_unavailable")?;
    let listener = TcpListener::bind(configuration.listen)
        .await
        .map_err(|_| "listen_bind_failed")?;
    let std_listener = listener.into_std().map_err(|_| "listener_convert_failed")?;
    let setup_authorization = std::fs::read_to_string(
        configuration
            .root
            .join("secrets")
            .join(root::SETUP_AUTHORIZATION_NAME),
    )
    .map_err(|_| "setup_authorization_unavailable")?;
    if setup_authorization.len() != 64
        || !setup_authorization.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("setup_authorization_invalid");
    }
    let auth = AuthState::new_with_setup(
        workers.clone(),
        configuration.public_origin.unwrap_or_else(|| {
            format!(
                "https://{}:{}",
                configuration.listen.ip(),
                configuration.listen.port()
            )
        }),
        setup_authorization,
    )
    .with_storage_root(&configuration.root)
    .map_err(|_| "payload_storage_unavailable")?;
    let app = http::router(auth)
        .route("/healthz", get(|| async { "ok" }))
        .fallback(http::assets::serve);
    // Install signal handling before spawning anything that owns root/DB
    // state. An early error can then drop everything without detaching work.
    let mut terminate = signal(SignalKind::terminate()).map_err(|_| "signal_setup_failed")?;
    let handle = axum_server::Handle::new();
    let server_handle = handle.clone();
    let mut task = tokio::spawn(async move {
        axum_server::from_tcp_rustls(std_listener, tls)
            .map_err(std::io::Error::other)?
            .handle(server_handle)
            .serve(app.into_make_service())
            .await
    });
    // Confirm the listener is actually serving before announcing readiness.
    let listening = tokio::time::timeout(Duration::from_secs(5), handle.listening()).await;
    if !matches!(listening, Ok(Some(_))) {
        let _ = task.await;
        drop(bound);
        retain_guard_until_process_exit(guard);
        return Err("listener_start_failed");
    }
    contract_event("listener.bind");

    // Wait for termination (or the server ending on its own), then drain in
    // order: graceful shutdown, join, close database, process-lifetime lock.
    let server_joined = tokio::select! {
        _ = tokio::signal::ctrl_c() => false,
        _ = terminate.recv() => false,
        joined = &mut task => {
            let _ = joined;
            true
        }
    };
    if server_joined {
        // The select already consumed the JoinHandle output; do not poll the
        // completed handle again. Connection workers are dropped by runtime
        // teardown while the forgotten descriptor still owns the lock.
        let _ = workers.shutdown().await;
        drop(bound);
        retain_guard_until_process_exit(guard);
        return Ok(());
    }
    handle.graceful_shutdown(Some(Duration::from_secs(5)));
    drain_server_task(task, bound, guard, Some(workers), Duration::from_secs(6)).await;
    Ok(())
}

/// Join the outer server before closing the local database handle. Retain
/// ownership until process teardown because axum-server's independently
/// spawned connection workers can outlive its outer serve future.
async fn drain_server_task(
    mut task: tokio::task::JoinHandle<std::io::Result<()>>,
    bound: root::BoundDatabase,
    guard: ownership::OwnershipGuard,
    workers: Option<SqliteWorkers>,
    join_timeout: Duration,
) {
    if tokio::time::timeout(join_timeout, &mut task).await.is_err() {
        // Dropping a timed-out JoinHandle would detach the server and could
        // unlock the root while its database/root users were still running.
        task.abort();
        let _ = task.await;
    }
    if let Some(workers) = workers {
        let _ = workers.shutdown().await;
    }
    drop(bound);
    retain_guard_until_process_exit(guard);
}

/// Keep the lock descriptor open until the process runtime has dropped every
/// spawned connection worker and its AuthState/database handle. Process exit
/// then closes the descriptor; there is no manual early-unlock or reacquire.
fn retain_guard_until_process_exit(guard: ownership::OwnershipGuard) {
    std::mem::forget(guard);
}

/// The retained H0 scaffold surface (see the module documentation). Not a
/// product serve path: no root is bound and no product route exists.
async fn scaffold_only() -> ExitCode {
    let address: SocketAddr = match env::var("SYLUX_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:3000".to_owned())
        .parse()
    {
        Ok(address) => address,
        Err(_) => {
            eprintln!("SYLUX_LISTEN is not a valid address");
            return ExitCode::FAILURE;
        }
    };
    if !address.ip().is_loopback() {
        eprintln!("SYLUX_LISTEN must be a loopback address, got {address}");
        return ExitCode::FAILURE;
    }
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .fallback(sylux::http::assets::serve);
    let listener = match TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("bind failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "{} static assets listening on {}",
        application_name(),
        listener
            .local_addr()
            .map(|address| address.to_string())
            .unwrap_or_default()
    );
    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("serve failed: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf, sync::mpsc};

    fn temporary_root() -> PathBuf {
        let mut token = [0u8; 16];
        getrandom::fill(&mut token).unwrap();
        let suffix = token
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let root = std::env::temp_dir().join(format!("sylux-t006-drain-{suffix}"));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        root
    }

    struct ServerTaskDropped(mpsc::Sender<()>);

    impl Drop for ServerTaskDropped {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    #[tokio::test]
    async fn a10_shutdown_timeout_joins_server_task_before_unlock() {
        let root = temporary_root();
        root::initialize_empty_root(&root, DEFAULT_BUSY_TIMEOUT).unwrap();
        let guard = ownership::acquire_writer_lock(&root).unwrap();
        let bound = root::open_bound_database(&root, DEFAULT_BUSY_TIMEOUT).unwrap();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let task = tokio::spawn(async move {
            let _dropped = ServerTaskDropped(dropped_tx);
            std::future::pending::<std::io::Result<()>>().await
        });

        drain_server_task(task, bound, guard, None, Duration::from_millis(20)).await;

        assert!(
            dropped_rx.try_recv().is_ok(),
            "ownership was released while the server task was still alive"
        );
        assert!(
            matches!(
                ownership::acquire_writer_lock(&root),
                Err(ownership::OwnershipError::LockHeld)
            ),
            "the process-lifetime guard must remain locked until runtime teardown"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
