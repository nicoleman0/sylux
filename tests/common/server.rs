//! Fixture-only HTTPS lifecycle, not product readiness/recovery or a product TLS route.
use super::{FixtureRoot, WorkerLease, create_private_file};
use axum::{Router, routing::get};
use axum_server::{Handle, tls_rustls::RustlsConfig};
use std::{
    fs,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use tokio_rustls::TlsConnector;
use tower_http::services::ServeDir;

pub struct LoopbackServer {
    address: SocketAddr,
    handle: Handle<SocketAddr>,
    task: Option<JoinHandle<io::Result<()>>>,
    worker: Option<WorkerLease>,
    shutdown_timeout: Duration,
}
impl LoopbackServer {
    pub async fn start(
        root: &FixtureRoot,
        assets: &Path,
        tls: &TlsFiles,
        shutdown_timeout: Duration,
    ) -> io::Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        if !assets.join("index.html").is_file()
            || fs::symlink_metadata(assets)?.file_type().is_symlink()
        {
            return Err(io::Error::other("compiled static assets required"));
        }
        if !cert_file_permissions_are_private(&tls.leaf_key)? {
            return Err(io::Error::other("synthetic key must be private"));
        }
        let config = RustlsConfig::from_pem_file(&tls.leaf_cert, &tls.leaf_key)
            .await
            .map_err(|_| io::Error::other("invalid synthetic TLS identity"))?;
        // Retain this exact :0 listener; never reserve, close and race a rebind.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let listener = listener.into_std()?;
        let app = Router::new()
            .route("/harness-status", get(|| async { "fixture-listener-only" }))
            .fallback_service(ServeDir::new(assets).append_index_html_on_directories(true));
        let handle = Handle::new();
        let server_handle = handle.clone();
        let worker = root.register_worker()?;
        let task = tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, config)
                .map_err(io::Error::other)?
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });
        let mut server = Self {
            address,
            handle,
            task: Some(task),
            worker: Some(worker),
            shutdown_timeout,
        };
        if !matches!(timeout(Duration::from_secs(5), server.handle.listening()).await, Ok(Some(addr)) if addr == address)
        {
            server.shutdown_and_join().await?;
            return Err(io::Error::other("fixture listener startup failed"));
        }
        Ok(server)
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn origin(&self) -> String {
        format!("https://127.0.0.1:{}", self.address.port())
    }
    /// Keep the JoinHandle and lease on timeout. A failed/drop/cancel path never
    /// represents successful worker exit and never permits root deletion.
    pub async fn shutdown_and_join(&mut self) -> io::Result<()> {
        self.handle.graceful_shutdown(Some(self.shutdown_timeout));
        let task = self
            .task
            .as_mut()
            .ok_or_else(|| io::Error::other("server already joined"))?;
        let result = timeout(self.shutdown_timeout + Duration::from_secs(1), task)
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "fixture server join timed out; retain root",
                )
            })?;
        self.task.take();
        self.worker
            .take()
            .ok_or_else(|| io::Error::other("missing server lease"))?
            .joined()?;
        result.map_err(|_| io::Error::other("fixture server task failed"))?
    }
}
impl Drop for LoopbackServer {
    fn drop(&mut self) {
        if self.task.is_some() {
            self.handle.graceful_shutdown(Some(self.shutdown_timeout));
        }
        // No asynchronous Drop claim: an unjoined lease stays registered forever.
    }
}

pub struct TlsFiles {
    pub ca_cert: PathBuf,
    pub ca_der: PathBuf,
    pub leaf_cert: PathBuf,
    pub leaf_key: PathBuf,
}
impl TlsFiles {
    /// Generates fresh public CA + leaf and private keys only beneath the owned root.
    /// No secret is passed on argv, stdout, or persisted evidence. OpenSSL is existing
    /// tooling; this helper never provisions packages or changes host trust.
    pub fn generate(root: &FixtureRoot, directory: &str) -> io::Result<Self> {
        Self::generate_with_san(root, directory, "DNS:localhost,IP:127.0.0.1")
    }
    /// Reserved .test hostname resolved only by the isolated Chromium process to
    /// loopback. Omit the IP SAN so the real browser can exercise hostname denial.
    pub fn generate_browser(root: &FixtureRoot, directory: &str) -> io::Result<Self> {
        Self::generate_with_san(root, directory, "DNS:localhost,DNS:fixture.test")
    }
    fn generate_with_san(root: &FixtureRoot, directory: &str, san: &str) -> io::Result<Self> {
        if directory.is_empty()
            || !directory
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(io::Error::other("invalid TLS credential directory"));
        }
        let dir = root.path().join(directory);
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        for name in ["ca-key", "leaf-key"] {
            create_private_file(&dir, name)?;
        }
        let mut ext = create_private_file(&dir, "extensions")?;
        ext.write_all(format!("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName={san}\n").as_bytes())?;
        let commands: &[&[&str]] = &[
            &[
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "ca-key",
                "-out",
                "ca.pem",
                "-days",
                "1",
                "-subj",
                "/CN=Sylux disposable fixture CA",
                "-addext",
                "basicConstraints=critical,CA:TRUE",
                "-addext",
                "keyUsage=critical,keyCertSign,cRLSign",
            ],
            &[
                "req",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "leaf-key",
                "-out",
                "leaf.csr",
                "-subj",
                "/CN=localhost",
            ],
            &[
                "x509",
                "-req",
                "-in",
                "leaf.csr",
                "-CA",
                "ca.pem",
                "-CAkey",
                "ca-key",
                "-set_serial",
                "1",
                "-out",
                "leaf.pem",
                "-days",
                "1",
                "-extfile",
                "extensions",
            ],
            &["x509", "-in", "ca.pem", "-outform", "DER", "-out", "ca.der"],
        ];
        for args in commands {
            run_openssl(root, &dir, args)?;
        }
        Ok(Self {
            ca_cert: dir.join("ca.pem"),
            ca_der: dir.join("ca.der"),
            leaf_cert: dir.join("leaf.pem"),
            leaf_key: dir.join("leaf-key"),
        })
    }
}
fn run_openssl(root: &FixtureRoot, dir: &Path, args: &[&str]) -> io::Result<()> {
    let lease = root.register_worker()?;
    let spawned = Command::new("openssl")
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            lease.joined()?;
            return Err(error);
        }
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                lease.joined()?;
                return if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other("synthetic certificate generation failed"))
                };
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)), // watchdog, not race ordering
            Ok(None) => {
                let _ = child.kill();
                child.wait()?;
                lease.joined()?;
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "certificate generation timed out",
                ));
            }
            Err(e) => {
                let _ = child.kill();
                child.wait()?;
                lease.joined()?;
                return Err(e);
            }
        }
    }
}
pub fn openssl_sha256(root: &FixtureRoot, name: &str) -> io::Result<String> {
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(io::Error::other("invalid digest fixture name"));
    }
    let lease = root.register_worker()?;
    let spawned = Command::new("openssl")
        .current_dir(root.path())
        .args(["dgst", "-sha256", "-r", name])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            lease.joined()?;
            return Err(e);
        }
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                child.wait()?;
                lease.joined()?;
                return Err(io::Error::other("fixture digest failed or timed out"));
            }
        }
    };
    lease.joined()?;
    if !status.success() {
        return Err(io::Error::other("fixture digest failed"));
    }
    let mut output = String::new();
    child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing digest pipe"))?
        .take(4096)
        .read_to_string(&mut output)?;
    let digest = output
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::other("missing digest"))?;
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::other("invalid fixture digest"));
    }
    Ok(digest.to_ascii_lowercase())
}

pub fn client_config_with_ca(ca_der: &[u8]) -> Result<rustls::ClientConfig, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(ca_der.to_vec()))
        .map_err(|_| "invalid fixture CA")?;
    rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|_| "TLS versions unavailable".to_owned())
        .map(|b| b.with_root_certificates(roots).with_no_client_auth())
}
/// Small static-response probe; call on a blocking thread while async server runs.
/// Tests assert certificate error variants, not arbitrary connection failure.
pub fn tls_get(address: SocketAddr, ca: &[u8], hostname: &str) -> io::Result<Vec<u8>> {
    let config = client_config_with_ca(ca).map_err(io::Error::other)?;
    let name =
        rustls::pki_types::ServerName::try_from(hostname.to_owned()).map_err(io::Error::other)?;
    let connection =
        rustls::ClientConnection::new(Arc::new(config), name).map_err(io::Error::other)?;
    let socket = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut stream = rustls::StreamOwned::new(connection, socket);
    stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut bytes = Vec::new();
    stream.take(1024 * 1024).read_to_end(&mut bytes)?;
    Ok(bytes)
}
pub fn cert_file_permissions_are_private(path: &Path) -> io::Result<bool> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.nlink() == 1
        && metadata.permissions().mode() & 0o077 == 0)
}

/// Loopback-TLS product-router harness: serves the real product router
/// (`sylux::http::router`) over a synthetic-CA loopback TLS listener so contract
/// tests exercise the actual route stack with strict certificate validation.
/// Reuses the reviewed `TlsFiles` identity and `axum_server` TLS acceptor; no new
/// trust surface, no host trust change, no secrets on argv/stdout/evidence.
pub struct TlsApiServer {
    address: SocketAddr,
    handle: Handle<SocketAddr>,
    task: Option<JoinHandle<io::Result<()>>>,
    worker: Option<WorkerLease>,
    shutdown_timeout: Duration,
}
impl TlsApiServer {
    pub async fn start(
        root: &FixtureRoot,
        tls: &TlsFiles,
        app: Router,
        shutdown_timeout: Duration,
    ) -> io::Result<Self> {
        Self::start_with_origin(root, tls, shutdown_timeout, |_| app).await
    }

    /// Bind an ephemeral loopback TLS listener, then build a real application using
    /// its exact public origin. Browser fixtures use this to make the router's
    /// Origin policy and trusted certificate hostname agree with the bound port.
    pub async fn start_with_origin<F>(
        root: &FixtureRoot,
        tls: &TlsFiles,
        shutdown_timeout: Duration,
        make_app: F,
    ) -> io::Result<Self>
    where
        F: FnOnce(String) -> Router,
    {
        let _ = rustls::crypto::ring::default_provider().install_default();
        if !cert_file_permissions_are_private(&tls.leaf_key)? {
            return Err(io::Error::other("synthetic key must be private"));
        }
        let config = RustlsConfig::from_pem_file(&tls.leaf_cert, &tls.leaf_key)
            .await
            .map_err(|_| io::Error::other("invalid synthetic TLS identity"))?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let listener = listener.into_std()?;
        let origin = format!("https://fixture.test:{}", address.port());
        let app = make_app(origin);
        let handle = Handle::new();
        let server_handle = handle.clone();
        let worker = root.register_worker()?;
        let task = tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, config)
                .map_err(io::Error::other)?
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
        });
        let mut server = Self {
            address,
            handle,
            task: Some(task),
            worker: Some(worker),
            shutdown_timeout,
        };
        if !matches!(timeout(Duration::from_secs(5), server.handle.listening()).await, Ok(Some(addr)) if addr == address)
        {
            server.shutdown_and_join().await?;
            return Err(io::Error::other("fixture listener startup failed"));
        }
        Ok(server)
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn origin(&self) -> String {
        format!("https://127.0.0.1:{}", self.address.port())
    }
    pub async fn shutdown_and_join(&mut self) -> io::Result<()> {
        self.handle.graceful_shutdown(Some(self.shutdown_timeout));
        let task = self
            .task
            .as_mut()
            .ok_or_else(|| io::Error::other("server already joined"))?;
        let result = timeout(self.shutdown_timeout + Duration::from_secs(1), task)
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "fixture server join timed out; retain root",
                )
            })?;
        self.task.take();
        self.worker
            .take()
            .ok_or_else(|| io::Error::other("missing server lease"))?
            .joined()?;
        result.map_err(|_| io::Error::other("fixture server task failed"))?
    }
}
impl Drop for TlsApiServer {
    fn drop(&mut self) {
        if self.task.is_some() {
            self.handle.graceful_shutdown(Some(self.shutdown_timeout));
        }
    }
}

/// Establish an async client TLS connection validated against the synthetic CA,
/// returning an `AsyncRead + AsyncWrite` stream the caller drives with raw HTTP
/// (the same shape the plaintext `tokio::net::TcpStream` contract fixtures use).
/// `ca_der` is the DER CA bytes (e.g. `std::fs::read(&tls.ca_der)`).
pub async fn tls_connect(
    address: SocketAddr,
    ca_der: &[u8],
    hostname: &str,
) -> io::Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    let config = client_config_with_ca(ca_der).map_err(io::Error::other)?;
    let connector = TlsConnector::from(Arc::new(config));
    let name =
        rustls::pki_types::ServerName::try_from(hostname.to_owned()).map_err(io::Error::other)?;
    let tcp = tokio::net::TcpStream::connect(address).await?;
    connector.connect(name, tcp).await.map_err(io::Error::other)
}
