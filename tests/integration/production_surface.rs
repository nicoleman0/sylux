//! Compare production and test-harness release binaries.
use std::{fs, path::PathBuf, process::Command};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn build(test_harness: bool) {
    let root = project_root();
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&root).args([
        "build",
        "--locked",
        "--release",
        "--no-default-features",
        "--bin",
        "sylux",
    ]);
    if test_harness {
        cmd.args(["--features", "test-harness"]);
    }
    let status = cmd.status().expect("cargo build must start");
    assert!(status.success(), "release binary build failed");
}

fn function_symbols(binary: &PathBuf) -> Vec<String> {
    let output = Command::new("nm")
        .args(["-C", "--defined-only"])
        .arg(binary)
        .output()
        .expect("nm must run");
    assert!(
        output.status.success(),
        "nm failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("nm output is UTF-8")
        .lines()
        .filter(|line| matches!(line.split_whitespace().nth(1), Some("T" | "t" | "W" | "w")))
        .map(str::to_owned)
        .collect()
}

fn contains_marker(binary: &[u8], marker: &str) -> bool {
    binary
        .windows(marker.len())
        .any(|bytes| bytes == marker.as_bytes())
}

#[test]
fn default_release_excludes_test_only_seed_and_fault_controls() {
    let root = project_root();
    let target = root.join("target/release");
    build(false);
    let production = target.join("sylux");
    let production_symbols = function_symbols(&production);
    let production_bytes = fs::read(&production).expect("read default release binary");

    build(true);
    let test_harness = target.join("sylux");
    let test_symbols = function_symbols(&test_harness);
    let test_bytes = fs::read(&test_harness).expect("read test-harness release binary");

    // Named test seams survive optimization without relying on an Arc drop instantiation.
    let expected_test_only = [
        "sylux::http::setup::pause_at_setup_commit",
        "coordinator_test_support::Fixture>::create",
        "Arc<sylux::domain::coordinator::FinalCommitTestBarrier>>::drop_slow",
        "Arc<sylux::http::setup::SetupCommitTestBarrier>>::drop_slow",
        "Arc<sylux::http::uploads::UploadCancelTestBarrier>>::drop_slow",
        "sylux::persistence::payload::PayloadStore>::cleanup_boundary",
    ];
    for symbol in expected_test_only {
        assert!(
            test_symbols.iter().any(|line| line.contains(symbol)),
            "test-harness release is missing FUNCTION symbol fragment {symbol:?}"
        );
        assert!(
            !production_symbols.iter().any(|line| line.contains(symbol)),
            "default release contains test-only FUNCTION symbol fragment {symbol:?}"
        );
    }

    // The hooks receive these literals at runtime even when their calls inline.
    for marker in [
        "publish.before_write",
        "publish.before_file_sync",
        "publish.before_staging_dir_sync",
        "publish.before_ancestor_sync",
        "publish.before_noreplace",
        "publish.before_final_dir_sync",
        "removal.before_commit",
        "cleanup.before_unlink",
        "cleanup.before_directory_sync",
        "cleanup.before_finalize",
    ] {
        assert!(
            contains_marker(&test_bytes, marker),
            "test-harness release is missing fault boundary {marker:?}"
        );
        assert!(
            !contains_marker(&production_bytes, marker),
            "default release contains fault boundary {marker:?}"
        );
    }
}
