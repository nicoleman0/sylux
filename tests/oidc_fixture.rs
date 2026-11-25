//! Disposable TLS product server for the Keycloak browser contract.
use serde::Deserialize;
use std::{fs, io, path::PathBuf, time::Duration};
use sylux::{
    http::{self, AuthState, TestOidcConfig},
    persistence::{
        root,
        sqlite::{self, SqliteWorkers, WorkerConfig},
    },
};
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};

#[derive(Deserialize)]
struct Config {
    issuer: String,
    client_id: String,
    secret: String,
    attempt_seconds: u64,
    session_seconds: u64,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let Some(fixture) = args.next().filter(|arg| arg != "--list") else {
        return Ok(());
    };
    let fixture = PathBuf::from(fixture);
    let mode = args
        .next()
        .ok_or_else(|| io::Error::other("fixture mode required"))?;
    let port: u16 = args
        .next()
        .and_then(|value| value.to_str()?.parse().ok())
        .unwrap_or(0);
    let product = fixture.join("sylux");
    let config: Config = serde_json::from_slice(&fs::read(fixture.join("sylux-oidc.json"))?)?;
    let busy = Duration::from_millis(500);
    if !product.join("metadata/sylux.sqlite3").exists() {
        root::initialize_empty_root(&product, busy)
            .map_err(|_| io::Error::other("fixture initialization failed"))?;
    }
    let db = product.join("metadata/sylux.sqlite3");
    let writer = sqlite::open_writer(&db, busy)
        .map_err(|_| io::Error::other("fixture database unavailable"))?;
    let workers = SqliteWorkers::from_writer(
        writer,
        &db,
        WorkerConfig {
            busy_timeout: busy,
            ..WorkerConfig::default()
        },
    )
    .map_err(|_| io::Error::other("fixture database workers unavailable"))?;

    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| io::Error::other("TLS provider unavailable"))?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        fixture.join("tls/leaf.pem"),
        fixture.join("tls/leaf.key"),
    )
    .await?;
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let address = listener.local_addr()?;
    let origin = format!("https://localhost:{}", address.port());
    let setup = fs::read_to_string(product.join("secrets").join(root::SETUP_AUTHORIZATION_NAME))?;
    let mut state = AuthState::new_with_setup(workers.clone(), origin.clone(), setup)
        .with_storage_root(&product)
        .map_err(|_| io::Error::other("fixture storage unavailable"))?
        .with_webdav(true);
    match mode.to_str() {
        Some("enabled") => {
            state = state
                .with_test_oidc(TestOidcConfig {
                    issuer: config.issuer,
                    client_id: config.client_id,
                    secret: config.secret,
                    ca_pem: fs::read(fixture.join("ca.pem"))?,
                    attempt: Duration::from_secs(config.attempt_seconds),
                    session: Duration::from_secs(config.session_seconds),
                })
                .await
                .map_err(|_| io::Error::other("fixture OIDC configuration rejected"))?;
        }
        Some("disabled") => state
            .disable_oidc()
            .await
            .map_err(|_| io::Error::other("fixture OIDC removal failed"))?,
        _ => return Err(io::Error::other("invalid fixture mode")),
    }
    let mut terminate = signal(SignalKind::terminate())?;
    let handle = axum_server::Handle::new();
    let server_handle = handle.clone();
    let app = http::router(state).fallback(http::assets::serve);
    let listener = listener.into_std()?;
    let task = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)?
            .handle(server_handle)
            .serve(app.into_make_service())
            .await
    });
    if tokio::time::timeout(Duration::from_secs(5), handle.listening())
        .await
        .ok()
        .flatten()
        != Some(address)
    {
        return Err(io::Error::other("fixture listener failed"));
    }
    println!("{origin}");
    terminate.recv().await;
    handle.graceful_shutdown(Some(Duration::from_secs(2)));
    task.await.map_err(io::Error::other)??;
    workers
        .shutdown()
        .await
        .map_err(|_| io::Error::other("fixture workers failed to stop"))?;
    Ok(())
}
