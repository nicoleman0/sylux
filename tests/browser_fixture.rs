//! Executable test-only fixture owner, not the Sylux product service/CLI.
// harness=false still sets cfg(test), but libtest-only functions are omitted;
// their module imports otherwise trigger unused-import warnings in this driver.
#[allow(unused_imports)]
#[path = "common/mod.rs"]
mod common;

use common::{FixtureRoot, create_private_file, sha256_hex, unique_token, write_private_json};
use common::{
    browser::{BROWSER_PASSWORD, SeededBrowserAccount, seed_browser_account},
    server::{LoopbackServer, TlsApiServer, TlsFiles},
};
use serde_json::{Value, json};
use std::{
    fs, io,
    io::Read,
    os::unix::{fs::DirBuilderExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use sylux::{
    http::{self, AuthState},
    persistence::{
        root,
        sqlite::{self, SqliteWorkers, WorkerConfig},
    },
};
use tower_http::services::ServeDir;

const BUSY: Duration = Duration::from_millis(375);

fn safe_reporter_events(log: &str) -> Vec<Value> {
    const CASES: &[&str] = &[
        "trusted_assets",
        "trusted_api_routes",
        "wrong_san",
        "unrelated_ca",
        "plaintext_and_cookie_policy",
        "accessibility_keyboard_sign_in",
        "accessibility_control_names",
        "accessibility_rename_escape_focus",
        "accessibility_live_failure_retry",
        "storage_responsive_controls",
        "storage_listing_states",
        "storage_upload_download_roundtrip",
        "storage_delayed_upload_browsing",
        "storage_upload_cancel",
        "storage_lost_commit_response",
        "unregistered",
    ];
    const NETWORK_ERRORS: &[&str] = &[
        "ERR_EMPTY_RESPONSE",
        "ERR_CONNECTION_RESET",
        "ERR_CONNECTION_CLOSED",
        "ERR_SSL_PROTOCOL_ERROR",
        "ERR_CERT_AUTHORITY_INVALID",
        "ERR_CERT_COMMON_NAME_INVALID",
        "ERR_HTTP_RESPONSE_CODE_FAILURE",
        "ERR_BLOCKED_BY_CLIENT",
        "ERR_INVALID_HTTP_RESPONSE",
        "ERR_UNSAFE_PORT",
        "ERR_TIMED_OUT",
    ];
    log.lines()
        .filter_map(|line| {
            let event: Value = serde_json::from_str(line).ok()?;
            match event["event"].as_str()? {
                "case" => {
                    let case = event["case_id"]
                        .as_str()
                        .filter(|case| CASES.contains(case))?;
                    let status = event["status"].as_str().filter(|status| {
                        [
                            "passed",
                            "failed",
                            "timedOut",
                            "skipped",
                            "interrupted",
                            "unknown",
                        ]
                        .contains(status)
                    })?;
                    let mut safe = json!({"event": "case", "case_id": case, "status": status});
                    if let (Some(file), Some(line)) = (
                        event["failure_file"].as_str(),
                        event["failure_line"].as_u64(),
                    ) && [
                        "harness.spec.ts",
                        "storage.spec.ts",
                        "accessibility.spec.ts",
                    ]
                    .contains(&file)
                        && (1..=1_000_000).contains(&line)
                    {
                        safe["failure_file"] = json!(file);
                        safe["failure_line"] = json!(line);
                    }
                    if let Some(network) = event["network_error"]
                        .as_str()
                        .filter(|network| NETWORK_ERRORS.contains(network))
                    {
                        safe["network_error"] = json!(network);
                    }
                    if event["unexpected_resolution"] == true {
                        safe["unexpected_resolution"] = json!(true);
                    }
                    Some(safe)
                }
                "runner_error" => Some(json!({"event": "runner_error"})),
                "end" => {
                    let status = event["status"].as_str().filter(|status| {
                        ["passed", "failed", "timedout", "interrupted", "unknown"].contains(status)
                    })?;
                    Some(json!({"event": "end", "status": status}))
                }
                _ => None,
            }
        })
        .collect()
}

/// Locate the `certutil` binary for the isolated Chromium NSS trust store.
/// Prefers an explicit `S1_CERTUTIL` override, then the repo-local test-tools
/// provision (gitignored `.local/`), reaching the main checkout from a worktree.
fn certutil_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("S1_CERTUTIL") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    // Walk up from the manifest dir so a worktree checkout (`.worktrees/<id>`)
    // still reaches the main checkout's gitignored `.local/test-tools` provision.
    let mut dir = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    for _ in 0..4 {
        let candidate = dir.join(".local/test-tools/nss/usr/bin/certutil");
        if candidate.is_file() {
            return Ok(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    Err(io::Error::other(
        "certutil not found; set S1_CERTUTIL or provision .local/test-tools/nss",
    ))
}

#[allow(clippy::too_many_arguments)]
fn browser_phase(
    root: &FixtureRoot,
    tls: &TlsFiles,
    output: &Path,
    phase: &str,
    origin: &str,
    unrelated: &str,
    assets: &Path,
    seeded: &SeededBrowserAccount,
) -> io::Result<Value> {
    let folder = output.join(format!("browser-{phase}"));
    let log_name = format!("browser-{phase}-log");
    let log = create_private_file(output, &log_name)?;
    let lease = root.register_worker()?;
    let spawned = Command::new("npm")
        .args(["--silent", "--prefix", "web", "run", "test:browser"])
        .env("S1_HTTPS_ORIGIN", origin)
        .env("S1_UNRELATED_ORIGIN", unrelated)
        .env("S1_CA_CERT", &tls.ca_cert)
        .env("S1_CERTUTIL", certutil_path()?)
        .env("S1_BROWSER_FIXTURE", root.path())
        .env("S1_BROWSER_PHASE", phase)
        .env("S1_BROWSER_HANDLE", &seeded.handle)
        .env("S1_BROWSER_PASSWORD", BROWSER_PASSWORD)
        .env(
            "S1_BROWSER_FIXTURES",
            seeded.fixtures_json().map_err(io::Error::other)?,
        )
        .env("S1_BROWSER_OUTPUT", &folder)
        .env(
            "S1_ASSET_SHA256",
            sha256_hex(&fs::read(assets.join("index.html"))?),
        )
        .env_remove("S1_BROWSER_OWNER")
        .env_remove("DEBUG")
        .env_remove("PWDEBUG")
        .env_remove("NODE_OPTIONS")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(_) => {
            lease.joined()?;
            return Err(io::Error::other("browser launcher unavailable"));
        }
    };
    let deadline = Instant::now() + Duration::from_secs(330);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                // Terminate the entire browser/Node/npm process group; direct child
                // wait alone cannot prove descendant exit, so keep the lease/root.
                if let Ok(mut killer) = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    let reap_deadline = Instant::now() + Duration::from_secs(2);
                    while matches!(killer.try_wait(), Ok(None)) && Instant::now() < reap_deadline {
                        thread::sleep(Duration::from_millis(10));
                    }
                    let _ = killer.kill();
                    let _ = killer.try_wait();
                }
                let _ = child.kill();
                let reap_deadline = Instant::now() + Duration::from_secs(5);
                while matches!(child.try_wait(), Ok(None)) && Instant::now() < reap_deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                return Err(io::Error::other(
                    "browser watchdog failed; retain fixture root",
                ));
            }
        }
    };
    // Only the normal successfully closed browser lifecycle proves descendants
    // were joined. Failure is conservative: keep the lease and disposable root.
    if !status.success() {
        if let Ok(log) = fs::read_to_string(output.join(&log_name)) {
            // npm stderr shares this file; never forward raw lines or arbitrary JSON fields.
            for event in safe_reporter_events(&log) {
                eprintln!("{event}");
            }
        }
        return Err(io::Error::other(
            "browser assertions failed; retain fixture root",
        ));
    }
    let engine_path = folder.join("engine.json");
    let engine_bytes = fs::read(&engine_path)?;
    let engine: Value = serde_json::from_slice(&engine_bytes)?;
    if engine["closed"] != true
        || engine["normal_tls_verification"] != true
        || engine["isolated_nss"] != true
        || engine["playwright"] != "1.65.0"
        || engine["engine"] != "158.0.8127.18"
    {
        return Err(io::Error::other("browser lifecycle observation incomplete"));
    }
    lease.joined()?;
    let log = fs::read_to_string(output.join(&log_name))?;
    let events: Vec<Value> = log
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    if events
        .last()
        .is_none_or(|e| e["event"] != "end" || e["status"] != "passed")
    {
        return Err(io::Error::other("browser execution summary missing"));
    }
    let cases: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "case")
        .map(|e| json!({"case_id": e["case_id"], "status": e["status"]}))
        .collect();
    Ok(json!({"phase": phase, "engine": engine["engine"],
        "executable_sha256": engine["executable_sha256"], "playwright": engine["playwright"],
        "engine_file": format!("browser-{phase}/engine.json"),
        "engine_sha256": sha256_hex(&engine_bytes),
        "raw": log_name, "raw_sha256": sha256_hex(log.as_bytes()), "cases": cases}))
}

async fn start_api_server(
    fixture: &FixtureRoot,
    tls: &TlsFiles,
    product: &Path,
    assets: &Path,
    setup_authorization: &str,
    workers: &SqliteWorkers,
) -> io::Result<TlsApiServer> {
    TlsApiServer::start_with_origin(fixture, tls, Duration::from_secs(2), |origin| {
        let state =
            AuthState::new_with_setup(workers.clone(), origin, setup_authorization.to_owned())
                .with_storage_root(product)
                .expect("fixture payload storage");
        http::router(state)
            .fallback_service(ServeDir::new(assets).append_index_html_on_directories(true))
    })
    .await
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--safe-reporter-events") {
        let mut log = String::new();
        io::stdin().read_to_string(&mut log)?;
        for event in safe_reporter_events(&log) {
            println!("{event}");
        }
        return Ok(());
    }
    let output = PathBuf::from(
        std::env::var_os("S1_HARNESS_OUTPUT")
            .ok_or_else(|| io::Error::other("fresh private evidence output required"))?,
    )
    // The browser subprocess (npm `--prefix web`) runs with cwd=web/, so a relative
    // evidence path would resolve under web/ and fail the below-evidence ownership
    // check. Canonicalize to an absolute path so S1_BROWSER_OUTPUT stays under the
    // repo's evidence root regardless of the child's cwd.
    .canonicalize()
    .map_err(|_| io::Error::other("S1_HARNESS_OUTPUT must be an existing directory"))?;
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web/dist");
    let parent =
        fs::canonicalize(std::env::temp_dir())?.join(format!("sylux-browser-{}", unique_token()));
    let a = FixtureRoot::create(&parent, "browser", "trusted")?;
    let b = FixtureRoot::create(&parent, "browser", "unrelated")?;
    let tls_a = TlsFiles::generate_browser(&a, "credentials")?;
    let tls_b = TlsFiles::generate_browser(&b, "credentials")?;
    let sentinel = unique_token();
    fs::write(a.path().join("sentinel"), sentinel.as_bytes())?;
    let product = a.path().join("product");
    fs::DirBuilder::new().mode(0o700).create(&product)?;
    root::initialize_empty_root(&product, BUSY)
        .map_err(|_| io::Error::other("fixture root initialization failed"))?;
    let setup_authorization = fs::read_to_string(
        product
            .join(root::SECRETS_DIR)
            .join(root::SETUP_AUTHORIZATION_NAME),
    )?;
    let db_path = product.join(root::METADATA_DIR).join("sylux.sqlite3");
    let mut workers = SqliteWorkers::from_writer(
        sqlite::open_writer(&db_path, BUSY)
            .map_err(|_| io::Error::other("fixture SQLite writer open failed"))?,
        &db_path,
        WorkerConfig {
            busy_timeout: BUSY,
            ..WorkerConfig::default()
        },
    )
    .map_err(|_| io::Error::other("fixture SQLite workers failed"))?;
    let mut api_server = start_api_server(
        &a,
        &tls_a,
        &product,
        &assets,
        &setup_authorization,
        &workers,
    )
    .await?;
    let ca_der = fs::read(&tls_a.ca_der)?;
    let seeded = seed_browser_account(
        api_server.address(),
        &ca_der,
        &format!("https://fixture.test:{}", api_server.address().port()),
        &setup_authorization,
    )
    .await?;
    let mut sb = LoopbackServer::start(&b, &assets, &tls_b, Duration::from_secs(2)).await?;
    if api_server.address() == sb.address()
        || !api_server.address().ip().is_loopback()
        || !sb.address().ip().is_loopback()
    {
        return Err(io::Error::other("fixture listeners not isolated"));
    }
    let mut phases = Vec::new();
    let mut failure = None;
    for phase in ["initial", "restart"] {
        let result = browser_phase(
            &a,
            &tls_a,
            &output,
            phase,
            &format!("https://fixture.test:{}", api_server.address().port()),
            &format!("https://fixture.test:{}", sb.address().port()),
            &assets,
            &seeded,
        );
        if let Ok(observation) = result {
            phases.push(observation);
        } else {
            failure = result.err();
            break;
        }
        api_server.shutdown_and_join().await?;
        if phase == "initial" {
            if fs::read(a.path().join("sentinel"))? != sentinel.as_bytes() {
                return Err(io::Error::other("restart fixture inventory changed"));
            }
            workers
                .shutdown()
                .await
                .map_err(|_| io::Error::other("fixture SQLite workers failed"))?;
            workers = SqliteWorkers::from_writer(
                sqlite::open_writer(&db_path, BUSY)
                    .map_err(|_| io::Error::other("fixture SQLite writer open failed"))?,
                &db_path,
                WorkerConfig {
                    busy_timeout: BUSY,
                    ..WorkerConfig::default()
                },
            )
            .map_err(|_| io::Error::other("fixture SQLite workers failed"))?;
            api_server = start_api_server(
                &a,
                &tls_a,
                &product,
                &assets,
                &setup_authorization,
                &workers,
            )
            .await?;
        }
    }
    if failure.is_some() {
        api_server.shutdown_and_join().await?;
    }
    sb.shutdown_and_join().await?;
    if let Some(error) = failure {
        workers
            .shutdown()
            .await
            .map_err(|_| io::Error::other("fixture SQLite workers failed"))?;
        return Err(error);
    }
    workers
        .shutdown()
        .await
        .map_err(|_| io::Error::other("fixture SQLite workers failed"))?;
    a.cleanup_after_workers_exit()?;
    b.cleanup_after_workers_exit()?;
    fs::remove_dir(parent)?;
    write_private_json(
        &output.join("browser-observation.json"),
        &json!({
            "schema_version": 1, "scope": "fixture-only", "joined": true,
            "restarted": true, "cleaned": true, "phases": phases,
        }),
    )?;
    println!("fixture browser lifecycle joined and cleaned; no product acceptance");
    Ok(())
}
