use std::{env, ffi::OsString, net::SocketAddr, process::ExitCode, sync::Arc, time::Duration};

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
            CommandKind::AdminResetPassword => {
                match sylux::admin::reset_password_from_stdin(
                    &configuration.root,
                    configuration
                        .admin_handle
                        .as_deref()
                        .expect("parser requires handle"),
                )
                .await
                {
                    Ok(()) => {
                        eprintln!("sylux: administrator_password_reset");
                        ExitCode::SUCCESS
                    }
                    Err(reason) => {
                        eprintln!("sylux: {reason}");
                        ExitCode::FAILURE
                    }
                }
            }
            CommandKind::AdminVerifyPayloads => run_verify_payloads(&configuration),
            CommandKind::AdminBackup => {
                match sylux::backup::create(
                    &configuration.root,
                    configuration
                        .destination
                        .as_deref()
                        .expect("parser requires destination"),
                )
                .await
                {
                    Ok(count) => {
                        println!("backup_complete payloads={count}");
                        ExitCode::SUCCESS
                    }
                    Err(reason) => {
                        eprintln!("sylux: {reason}");
                        ExitCode::FAILURE
                    }
                }
            }
            CommandKind::AdminExport => {
                match sylux::export::export_account(
                    &configuration.root,
                    configuration
                        .admin_handle
                        .as_deref()
                        .expect("parser requires handle"),
                    configuration
                        .destination
                        .as_deref()
                        .expect("parser requires destination"),
                ) {
                    Ok(count) => {
                        println!("export_complete files={count}");
                        ExitCode::SUCCESS
                    }
                    Err(reason) => {
                        eprintln!(
                            "sylux: {reason}; export failed; any newly written destination is incomplete"
                        );
                        ExitCode::FAILURE
                    }
                }
            }
        },
        Err(error) => {
            eprintln!("sylux: {}", error.reason());
            ExitCode::from(2)
        }
    }
}

fn run_verify_payloads(configuration: &CliConfig) -> ExitCode {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let result = sylux::admin::verify_payloads(&configuration.root, |blob_id, fault| {
        writeln!(
            out,
            "damaged blob={blob_id} reason={}",
            sylux::admin::PayloadReport::fault_label(fault)
        )
    })
    .and_then(|report| {
        writeln!(
            out,
            "payload_verification checked={} verified={} damaged={}",
            report.checked,
            report.checked - report.damaged,
            report.damaged
        )
        .map_err(|_| "verify_output_failed")?;
        Ok(report)
    });
    match result {
        Ok(report) if report.damaged == 0 => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(reason) => {
            eprintln!("sylux: {reason}");
            ExitCode::FAILURE
        }
    }
}

fn run_init(configuration: &CliConfig) -> ExitCode {
    match root::initialize_empty_root(&configuration.root, DEFAULT_BUSY_TIMEOUT) {
        Ok(_) => {
            contract_event("init.complete");
            ExitCode::SUCCESS
        }
        Err(error) => {
            // Lock contention already emitted ownership.refused.
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

async fn run_serve(configuration: CliConfig) -> ExitCode {
    contract_event("ownership.lock_acquire");
    let guard = match ownership::acquire_writer_lock(&configuration.root) {
        Ok(guard) => guard,
        Err(error) => {
            match error {
                ownership::OwnershipError::LockHeld => {
                    contract_event_reason("ownership.refused", error.reason())
                }
                _ => contract_event_reason("binding.refused", error.reason()),
            }
            return ExitCode::FAILURE;
        }
    };
    contract_event("ownership.acquired");

    let manifest = match root::read_manifest(&configuration.root) {
        Ok(manifest) => manifest,
        Err(error) => {
            contract_event_reason("binding.refused", error.reason());
            return ExitCode::FAILURE;
        }
    };

    // Validate before SQLite can create WAL/SHM files in a rejected installation.
    if let Err(error) = root::verify_protected_material(&configuration.root) {
        contract_event_reason("binding.refused", error.reason());
        return ExitCode::FAILURE;
    }

    contract_event("database.open");
    let bound = match root::open_bound_database(&configuration.root, DEFAULT_BUSY_TIMEOUT) {
        Ok(bound) => bound,
        Err(error) => {
            contract_event_reason("binding.refused", error.reason());
            return ExitCode::FAILURE;
        }
    };
    let _manifest = match root::upgrade_binding(&bound, &configuration.root, &manifest) {
        Ok(manifest) => manifest,
        Err(error) => {
            drop(bound);
            contract_event_reason("binding.refused", error.reason());
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = schema::apply_migrations(&bound.connection) {
        drop(bound);
        contract_event_reason("startup.failed", &error.reason());
        return ExitCode::FAILURE;
    }
    contract_event("migration.complete");
    contract_event("recovery.start");
    if let Err(error) = schema::recovery_checks(&bound.connection) {
        drop(bound);
        contract_event_reason("startup.failed", &error.reason());
        return ExitCode::FAILURE;
    }

    match serve_listener(configuration, guard, bound).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            contract_event_reason("startup.failed", reason);
            ExitCode::FAILURE
        }
    }
}

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
    let mut terminate = signal(SignalKind::terminate()).map_err(|_| "signal_setup_failed")?;
    let storage = auth.storage.clone().ok_or("payload_storage_unavailable")?;
    let cleanup = auth.start_cleanup();
    let app = http::router(auth)
        .route("/healthz", get(|| async { "ok" }))
        .fallback(http::assets::serve);
    let handle = axum_server::Handle::new();
    let server_handle = handle.clone();
    let mut task = tokio::spawn(async move {
        axum_server::from_tcp_rustls(std_listener, tls)
            .map_err(std::io::Error::other)?
            .handle(server_handle)
            .serve(app.into_make_service())
            .await
    });
    let listening = tokio::time::timeout(Duration::from_secs(5), handle.listening()).await;
    if !matches!(listening, Ok(Some(_))) {
        return finish_server_task(
            ServerExit::ListenerStartFailed(task),
            bound,
            guard,
            workers,
            storage,
            cleanup,
        )
        .await;
    }
    contract_event("listener.bind");
    let partial_cleanup = sylux::domain::upload::PartialCleanup::start(
        workers.clone(),
        Arc::clone(&storage),
        Duration::from_secs(60),
    );

    let server_joined = tokio::select! {
        _ = tokio::signal::ctrl_c() => false,
        _ = terminate.recv() => false,
        joined = &mut task => {
            let _ = joined;
            true
        }
    };
    if partial_cleanup.shutdown().await.is_err() {
        eprintln!("partial upload cleanup task failed");
    }
    let exit = if server_joined {
        ServerExit::Joined
    } else {
        handle.graceful_shutdown(Some(Duration::from_secs(5)));
        ServerExit::Graceful(task, Duration::from_secs(6))
    };
    finish_server_task(exit, bound, guard, workers, storage, cleanup).await
}

enum ServerExit {
    ListenerStartFailed(tokio::task::JoinHandle<std::io::Result<()>>),
    Joined,
    Graceful(tokio::task::JoinHandle<std::io::Result<()>>, Duration),
}

async fn finish_server_task(
    exit: ServerExit,
    bound: root::BoundDatabase,
    guard: ownership::OwnershipGuard,
    workers: SqliteWorkers,
    storage: Arc<sylux::domain::upload::UploadStorage>,
    cleanup: Option<sylux::persistence::cleanup::Runner>,
) -> Result<(), &'static str> {
    let (result, graceful) = match exit {
        ServerExit::ListenerStartFailed(task) => {
            let _ = task.await;
            (Err("listener_start_failed"), false)
        }
        ServerExit::Joined => (Ok(()), false),
        ServerExit::Graceful(mut task, join_timeout) => {
            if tokio::time::timeout(join_timeout, &mut task).await.is_err() {
                // Dropping the handle would detach a task that still uses the root.
                task.abort();
                let _ = task.await;
            }
            (Ok(()), true)
        }
    };
    // Upload requests outlive their connections; join them while SQLite runs.
    if storage.shutdown().await.is_err() {
        eprintln!("upload writer shutdown failed");
    }
    let cleanup_failed = match cleanup {
        Some(cleanup) => cleanup.shutdown().await.is_err(),
        None => false,
    };
    let _ = workers.shutdown().await;
    drop(bound);
    retain_guard_until_process_exit(guard);
    if cleanup_failed {
        if !graceful {
            return Err("cleanup_shutdown_failed");
        }
        contract_event_reason("removal.cleanup_pending", "cleanup_shutdown_failed");
    }
    result
}

// Connection workers can outlive the server future; keep ownership until process exit.
fn retain_guard_until_process_exit(guard: ownership::OwnershipGuard) {
    std::mem::forget(guard);
}

/// Static-only loopback server; no storage root or product routes.
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
        let workers = SqliteWorkers::open(
            &root.join(root::METADATA_DIR).join(root::DB_NAME),
            WorkerConfig::default(),
        )
        .unwrap();
        let storage = Arc::new(sylux::domain::upload::UploadStorage::open(&root).unwrap());
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let task = tokio::spawn(async move {
            let _dropped = ServerTaskDropped(dropped_tx);
            std::future::pending::<std::io::Result<()>>().await
        });

        finish_server_task(
            ServerExit::Graceful(task, Duration::from_millis(20)),
            bound,
            guard,
            workers,
            storage,
            None,
        )
        .await
        .unwrap();

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

    #[cfg(feature = "test-harness")]
    #[tokio::test]
    async fn cleanup_panic_during_exceptional_shutdown_joins_sqlite_and_retains_guard() {
        use sylux::persistence::{cleanup::Runner, sqlite::WorkerError};

        for exit in [
            ServerExit::ListenerStartFailed(tokio::spawn(async {
                Err(std::io::Error::other("synthetic listener startup failure"))
            })),
            ServerExit::Joined,
            ServerExit::Graceful(
                tokio::spawn(std::future::pending()),
                Duration::from_millis(20),
            ),
        ] {
            let root = temporary_root();
            root::initialize_empty_root(&root, DEFAULT_BUSY_TIMEOUT).unwrap();
            let guard = ownership::acquire_writer_lock(&root).unwrap();
            let bound = root::open_bound_database(&root, DEFAULT_BUSY_TIMEOUT).unwrap();
            let workers = SqliteWorkers::open(
                &root.join(root::METADATA_DIR).join(root::DB_NAME),
                WorkerConfig::default(),
            )
            .unwrap();
            let retained_workers = workers.clone();
            assert!(retained_workers.transact(|_| Ok(())).await.is_ok());
            assert!(retained_workers.read(|_| Ok(())).await.is_ok());
            let storage = Arc::new(sylux::domain::upload::UploadStorage::open(&root).unwrap());

            let graceful = matches!(&exit, ServerExit::Graceful(..));
            let result = finish_server_task(
                exit,
                bound,
                guard,
                workers,
                storage,
                Some(Runner::panicking_for_test()),
            )
            .await;

            assert_eq!(
                result,
                if graceful {
                    Ok(())
                } else {
                    Err("cleanup_shutdown_failed")
                }
            );
            assert!(matches!(
                retained_workers.transact(|_| Ok(())).await,
                Err(WorkerError::Closed)
            ));
            assert!(matches!(
                retained_workers.read(|_| Ok(())).await,
                Err(WorkerError::Closed)
            ));
            assert!(matches!(
                ownership::acquire_writer_lock(&root),
                Err(ownership::OwnershipError::LockHeld)
            ));
            fs::remove_dir_all(root).unwrap();
        }
    }
}
