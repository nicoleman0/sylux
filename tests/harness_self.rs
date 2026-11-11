#[path = "common/mod.rs"]
mod common;

use std::{fs, io::Read, path::PathBuf, sync::mpsc, thread, time::Duration};

use common::fixtures::{FixtureKind, FixturePlan};
use common::{CaseStatus, CoverageReport, CoverageRow, FixtureRoot, TestBarrier, TestClock};

fn temp_parent() -> PathBuf {
    fs::canonicalize(std::env::temp_dir())
        .expect("canonical system temp directory")
        .join(format!("sylux-harness-{}", common::unique_token()))
}

#[test]
fn fixture_root_is_unique_private_and_explicitly_cleanup_owned() {
    let parent = temp_parent();
    let a = FixtureRoot::create(&parent, "run", "case-a").unwrap();
    let b = FixtureRoot::create(&parent, "run", "case-b").unwrap();
    assert_ne!(a.path(), b.path());
    assert!(a.path().is_dir() && b.path().is_dir());
    let a_path = a.path().to_path_buf();
    a.cleanup_after_workers_exit().unwrap();
    assert!(!a_path.exists());
    b.cleanup_after_workers_exit().unwrap();
    fs::remove_dir(parent).unwrap();
}

#[test]
fn cleanup_rejects_replaced_directory_even_with_original_marker() {
    let parent = temp_parent();
    let root = FixtureRoot::create(&parent, "run", "replaced").unwrap();
    let path = root.path().to_owned();
    let moved = parent.join("original");
    fs::rename(&path, &moved).unwrap();
    fs::create_dir(&path).unwrap();
    fs::copy(moved.join(".fixture-owner"), path.join(".fixture-owner")).unwrap();
    let denied = root.cleanup_after_workers_exit().is_err();
    // Only test-created files; explicitly dismantle the replacement to restore the owned root.
    if path.exists() {
        fs::remove_file(path.join(".fixture-owner")).unwrap();
        fs::remove_dir(&path).unwrap();
    }
    fs::rename(&moved, &path).unwrap();
    assert!(denied, "cleanup accepted a same-marker replacement inode");
    root.cleanup_after_workers_exit().unwrap();
    fs::remove_dir(parent).unwrap();
}

#[test]
fn barrier_rejects_release_before_observed_arrival() {
    assert!(TestBarrier::new("unused").release().is_err());
}

#[test]
fn private_file_is_created_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    let parent = temp_parent();
    let root = FixtureRoot::create(&parent, "run", "private").unwrap();
    let file = common::create_private_file(root.path(), "credentials").unwrap();
    let mode = file.metadata().unwrap().permissions().mode() & 0o777;
    root.cleanup_after_workers_exit().unwrap();
    fs::remove_dir(parent).unwrap();
    assert_eq!(mode, 0o600);
}

#[test]
fn deterministic_fixture_is_generated_in_bounded_chunks() {
    let plan = FixturePlan::new(1234).with_chunk_size(257).unwrap();
    let expected = plan
        .expected_file(
            "id".into(),
            "owner".into(),
            "payload.bin".into(),
            FixtureKind::Generated,
            2_000_123,
        )
        .unwrap();
    let mut stream = plan.stream(FixtureKind::Generated, 2_000_123);
    let mut hash = sha2::Sha256::new();
    use sha2::Digest;
    let mut buffer = vec![0; 1024 * 1024];
    let mut total = 0;
    loop {
        let n = stream.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        assert!(n <= 257);
        hash.update(&buffer[..n]);
        total += n as u64;
    }
    assert_eq!(total, expected.length);
    assert_eq!(common::hex_bytes(&hash.finalize()), expected.sha256);
}

#[test]
fn injected_clock_and_barrier_have_deterministic_boundary() {
    let clock = TestClock::at(100);
    assert_eq!(clock.advance(5).unwrap(), 105);
    assert!(clock.advance(i64::MAX).is_err());
    let barrier = std::sync::Arc::new(TestBarrier::new("pause-before-write"));
    let worker = barrier.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let thread = thread::spawn(move || {
        let result = worker.worker_pause(Duration::from_secs(2));
        done_tx.send(result).unwrap();
    });
    barrier.wait_reached(Duration::from_secs(2)).unwrap();
    assert!(done_rx.try_recv().is_err());
    barrier.release().unwrap();
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    thread.join().unwrap();
}

#[test]
fn coverage_rejects_missing_and_blocked_rows() {
    let required = vec!["HARNESS_SELF".to_owned()];
    let missing = CoverageReport {
        schema_version: 1,
        mode: "harness-self".into(),
        rows: vec![],
    };
    assert!(missing.validate(&required).unwrap_err().contains("missing"));
    let blocked = CoverageReport {
        schema_version: 1,
        mode: "harness-self".into(),
        rows: vec![CoverageRow {
            row_id: "HARNESS_SELF".into(),
            status: CaseStatus::Blocked,
            blocker: Some("browser missing".into()),
            evidence: None,
        }],
    };
    assert!(
        blocked
            .validate(&required)
            .unwrap_err()
            .contains("not PASS")
    );
    let pass = CoverageReport {
        schema_version: 1,
        mode: "harness-self".into(),
        rows: vec![CoverageRow {
            row_id: "HARNESS_SELF".into(),
            status: CaseStatus::Pass,
            blocker: None,
            evidence: Some("selftest".into()),
        }],
    };
    pass.validate(&required).unwrap();
}

#[test]
fn cleanup_tracks_workers_and_rejects_symlink_and_hardlink_marker() {
    use std::os::unix::fs::symlink;
    let parent = temp_parent();
    let root = FixtureRoot::create(&parent, "run", "worker").unwrap();
    let lease = root.register_worker().unwrap();
    assert!(root.cleanup_after_workers_exit().is_err());
    lease.joined().unwrap();
    let marker = root.path().join(".fixture-owner");
    let alias = root.path().join("marker-alias");
    fs::hard_link(&marker, &alias).unwrap();
    assert!(root.cleanup_after_workers_exit().is_err());
    fs::remove_file(alias).unwrap();
    let outside = FixtureRoot::create(&parent, "run", "outside").unwrap();
    fs::write(outside.path().join("sentinel"), b"must survive").unwrap();
    symlink(outside.path(), root.path().join("outside-link")).unwrap();
    root.cleanup_after_workers_exit().unwrap();
    assert_eq!(
        fs::read(outside.path().join("sentinel")).unwrap(),
        b"must survive"
    );
    outside.cleanup_after_workers_exit().unwrap();
    fs::remove_dir(parent).unwrap();
}

#[test]
fn barrier_duplicate_timeout_unused_and_clock_clone() {
    let unused = TestBarrier::new("unused");
    assert!(unused.assert_drained().is_err());
    assert!(unused.wait_reached(Duration::ZERO).is_err());
    assert!(unused.worker_pause(Duration::ZERO).is_err());
    let b = std::sync::Arc::new(TestBarrier::new("one"));
    let worker = b.clone();
    let t = thread::spawn(move || worker.worker_pause(Duration::from_secs(2)));
    b.wait_reached(Duration::from_secs(2)).unwrap();
    b.release().unwrap();
    assert!(b.release().is_err());
    t.join().unwrap().ok();
    assert!(
        b.assert_drained().is_err(),
        "duplicate release must latch failure"
    );
    assert!(b.worker_pause(Duration::ZERO).is_err());
    let clock = TestClock::at(100);
    clock.clone().advance(1).unwrap();
    assert_eq!(clock.now(), 101);
    assert!(clock.advance(-1).is_err());
}

#[test]
fn sanitizer_drops_all_headers_and_bodies_including_unknown_keys() {
    let secret = common::unique_token();
    let exchange = common::redact_exchange(
        401,
        &[
            ("Authorization", &secret),
            ("X-Unknown", &secret),
            ("Set-Cookie", &secret),
        ],
        secret.as_bytes(),
    );
    let serialized = serde_json::to_string(&exchange).unwrap();
    assert!(!serialized.contains(&secret));
    assert_eq!(serialized.matches("[REDACTED]").count(), 2);
}

#[test]
fn fixture_inventory_has_independent_digest_and_fresh_credentials() {
    let parent = temp_parent();
    let a = FixtureRoot::create(&parent, "inventory", "a").unwrap();
    let b = FixtureRoot::create(&parent, "inventory", "b").unwrap();
    let accounts_a = common::fixtures::synthetic_accounts(&a).unwrap();
    let accounts_b = common::fixtures::synthetic_accounts(&b).unwrap();
    assert_eq!(accounts_a, accounts_b);
    for account in &accounts_a {
        assert!(
            fs::read(a.path().join(&account.handle)).unwrap()
                != fs::read(b.path().join(&account.handle)).unwrap(),
            "generated credentials must differ"
        );
    }
    let plan = FixturePlan::new(42);
    let cases = [
        ("sentinel.bin", FixtureKind::AllBytes, 256_u64),
        ("empty.bin", FixtureKind::Empty, 0),
        ("café.txt", FixtureKind::UnicodeNfc, 5),
        ("cafe\u{301}.txt", FixtureKind::UnicodeNfd, 6),
        ("attachment.html", FixtureKind::HtmlAttachment, 57),
        ("attachment.svg", FixtureKind::SvgAttachment, 75),
        ("one-mib.bin", FixtureKind::Generated, 1024 * 1024),
        ("long.bin", FixtureKind::Generated, 4 * 1024 * 1024 + 7),
    ];
    let mut inventory = Vec::new();
    for (index, (name, kind, length)) in cases.into_iter().enumerate() {
        let expected = plan
            .expected_file(
                format!("{:032x}", index + 200),
                accounts_a[0].account_id.clone(),
                name.into(),
                kind.clone(),
                length,
            )
            .unwrap();
        let physical_name = format!("payload-{index}");
        let mut file = common::create_private_file(a.path(), &physical_name).unwrap();
        assert_eq!(
            std::io::copy(&mut plan.stream(kind, length), &mut file).unwrap(),
            expected.length
        );
        file.sync_all().unwrap();
        let digest = common::server::openssl_sha256(&a, &physical_name).unwrap();
        assert_eq!(digest, expected.sha256);
        inventory.push(expected);
    }
    inventory.sort_by(|a, b| a.id.cmp(&b.id));
    if let Some(output) = std::env::var_os("S1_HARNESS_OUTPUT") {
        common::write_private_json(&PathBuf::from(output).join("fixture-inventory.json"), &serde_json::json!({"schema_version": 1, "scope": "synthetic-fixtures-only", "seed": 42, "accounts": accounts_a, "files": inventory})).unwrap();
    }
    a.cleanup_after_workers_exit().unwrap();
    b.cleanup_after_workers_exit().unwrap();
    fs::remove_dir(parent).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_loopback_tls_trust_san_isolation_restart_and_join() {
    use common::server::{LoopbackServer, TlsFiles, tls_get};
    let parent = temp_parent();
    let a = FixtureRoot::create(&parent, "tls", "a").unwrap();
    let b = FixtureRoot::create(&parent, "tls", "b").unwrap();
    let tls_a = TlsFiles::generate(&a, "credentials").unwrap();
    let tls_b = TlsFiles::generate(&b, "credentials").unwrap();
    assert!(common::server::cert_file_permissions_are_private(&tls_a.leaf_key).unwrap());
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web/dist");
    let mut sa = LoopbackServer::start(&a, &assets, &tls_a, Duration::from_secs(2))
        .await
        .unwrap();
    let mut sb = LoopbackServer::start(&b, &assets, &tls_b, Duration::from_secs(2))
        .await
        .unwrap();
    assert_ne!(sa.address(), sb.address());
    assert!(sa.address().ip().is_loopback());
    assert!(a.cleanup_after_workers_exit().is_err());
    let address = sa.address();
    let ca_a = fs::read(&tls_a.ca_der).unwrap();
    let ca_b = fs::read(&tls_b.ca_der).unwrap();
    let (good, wrong_san, unrelated) = tokio::task::spawn_blocking(move || {
        (
            tls_get(address, &ca_a, "localhost"),
            tls_get(address, &ca_a, "unrelated.invalid"),
            tls_get(address, &ca_b, "localhost"),
        )
    })
    .await
    .unwrap();
    let good = good.unwrap();
    assert!(good.starts_with(b"HTTP/1.1 200"));
    let html = fs::read(assets.join("index.html")).unwrap();
    assert!(good.windows(html.len()).any(|w| w == html));
    for error in [wrong_san.unwrap_err(), unrelated.unwrap_err()] {
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<rustls::Error>()
                .is_some_and(|e| matches!(e, rustls::Error::InvalidCertificate(_))),
            "must be certificate validation failure, not socket failure"
        );
    }
    sa.shutdown_and_join().await.unwrap();
    sb.shutdown_and_join().await.unwrap();
    let mut restarted = LoopbackServer::start(&a, &assets, &tls_a, Duration::from_secs(2))
        .await
        .unwrap();
    restarted.shutdown_and_join().await.unwrap();
    a.cleanup_after_workers_exit().unwrap();
    b.cleanup_after_workers_exit().unwrap();
    fs::remove_dir(parent).unwrap();
}
