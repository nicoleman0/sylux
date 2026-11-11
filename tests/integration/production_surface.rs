//! T027 compares actual release function symbols for default and test-harness builds.
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

#[test]
fn default_release_excludes_test_only_seed_and_fault_control_symbols() {
    let root = project_root();
    let target = root.join("target/release");
    build(false);
    let production = target.join("sylux");
    let saved = target.join("t027-production-surface.bin");
    fs::copy(&production, &saved).expect("save default release binary before feature build");
    let production_symbols = function_symbols(&saved);

    build(true);
    let test_harness = target.join("sylux");
    let test_symbols = function_symbols(&test_harness);

    // These fragments are taken from actual `nm -C` FUNCTION records in the
    // optimized feature build: the setup pause function, each test barrier's
    // monomorphized drop function, and the payload fault-hook closure type.
    // PublishFaultHook is a type alias (not a symbol), so assert its emitted
    // closure/drop function rather than searching stripped string literals.
    let expected_test_only = [
        "sylux::http::setup::pause_at_setup_commit",
        "coordinator_test_support::Fixture>::create",
        "Arc<sylux::domain::coordinator::FinalCommitTestBarrier>>::drop_slow",
        "Arc<sylux::http::setup::SetupCommitTestBarrier>>::drop_slow",
        "Arc<sylux::http::uploads::UploadCancelTestBarrier>>::drop_slow",
        "Weak<dyn core::ops::function::Fn<(&str,), Output = core::result::Result<(), sylux::persistence::payload::PayloadError>>",
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

    let evidence = format!(
        "scope=local-synthetic-release-binary-symbols\nproduction=tested-no-default-features\ntest_harness=tested-no-default-features-plus-test-harness\nproduction_test_only_symbols=none\ntest_harness_test_only_symbols={}\n",
        expected_test_only.join("\n")
    );
    fs::write(target.join("t027-surface-diff.txt"), evidence)
        .expect("write local ignored symbol-surface evidence");
}
