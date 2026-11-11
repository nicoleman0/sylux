//! Ordered S1 payload publication fault contracts.
//!
//! Hook-injected failures below are deterministic seam faults, not kernel or
//! filesystem failures. The collision test is exercised against the real local
//! filesystem. Short writes and actual EXDEV/EIO/ENOSPC/unsupported-filesystem
//! behavior are deferred to T031's bounded disposable quota/VFS qualification:
//! this hook cannot wrap Write and this test must not consume host disk.
#[path = "../common/mod.rs"]
mod common;

use sha2::{Digest, Sha256};
use std::{fs, io::Read, os::unix::fs::DirBuilderExt, path::PathBuf, sync::Arc};
use sylux::persistence::payload::{PayloadError, PayloadStore};

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sylux-fault-publication-{}",
        common::unique_token()
    ));
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

fn payload_id(n: u8) -> String {
    format!("{:02x}{}", n, "0".repeat(30))
}

fn publish(store: &PayloadStore, attempt: u8, blob: u8, bytes: &[u8]) {
    let mut stage = store
        .start(
            &format!("{}-{}", payload_id(attempt), payload_id(blob)),
            bytes.len() as u64,
        )
        .unwrap();
    stage.write_chunk(bytes).unwrap();
    stage.publish(Sha256::digest(bytes).into()).unwrap();
}

fn assert_payload(store: &PayloadStore, blob: u8, bytes: &[u8]) {
    let mut verified = store
        .open_verified(
            &payload_id(blob),
            bytes.len() as u64,
            Sha256::digest(bytes).into(),
        )
        .unwrap();
    let mut actual = Vec::new();
    verified.file.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, bytes);
}

#[test]
fn actual_noreplace_collision_preserves_committed_sentinel() {
    let root = root();
    let store = PayloadStore::open(&root).unwrap();
    let sentinel = b"committed sentinel: exact synthetic bytes";
    publish(&store, 1, 0x10, sentinel);

    let replacement = b"candidate must never overwrite sentinel";
    let mut stage = store
        .start(
            &format!("{}-{}", payload_id(2), payload_id(0x10)),
            replacement.len() as u64,
        )
        .unwrap();
    stage.write_chunk(replacement).unwrap();
    assert_eq!(
        stage.publish(Sha256::digest(replacement).into()),
        Err(PayloadError::Collision)
    );
    assert_payload(&store, 0x10, sentinel);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn seam_injected_publish_failures_preserve_sentinel_and_stop_at_boundary() {
    // These are hook errors before operations, not actual I/O failures.
    let cases: &[(&str, &[&str])] = &[
        (
            "publish.before_file_sync",
            &["publish.before_write", "publish.before_file_sync"],
        ),
        (
            "publish.before_staging_dir_sync",
            &[
                "publish.before_write",
                "publish.before_file_sync",
                "publish.before_staging_dir_sync",
            ],
        ),
        (
            "publish.before_ancestor_sync",
            &[
                "publish.before_write",
                "publish.before_file_sync",
                "publish.before_staging_dir_sync",
                "publish.before_ancestor_sync",
            ],
        ),
        (
            "publish.before_noreplace",
            &[
                "publish.before_write",
                "publish.before_file_sync",
                "publish.before_staging_dir_sync",
                "publish.before_ancestor_sync",
                "publish.before_noreplace",
            ],
        ),
        (
            "publish.before_final_dir_sync",
            &[
                "publish.before_write",
                "publish.before_file_sync",
                "publish.before_staging_dir_sync",
                "publish.before_ancestor_sync",
                "publish.before_noreplace",
                "publish.before_final_dir_sync",
            ],
        ),
    ];
    for (index, (failure_boundary, expected_trace)) in cases.iter().enumerate() {
        let root = root();
        let mut store = PayloadStore::open(&root).unwrap();
        let sentinel = b"sentinel stays byte-for-byte exact";
        publish(&store, 3, 0x20, sentinel);
        let trace = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = Arc::clone(&trace);
        let failure_boundary = *failure_boundary;
        store.set_publish_fault_hook(Some(Arc::new(move |boundary| {
            observed.lock().unwrap().push(boundary);
            if boundary == failure_boundary {
                Err(PayloadError::Io("injected_publication_boundary"))
            } else {
                Ok(())
            }
        })));

        let candidate = b"synthetic candidate";
        let mut stage = store
            .start(
                &format!(
                    "{}-{}",
                    payload_id(4 + index as u8),
                    payload_id(0x30 + index as u8)
                ),
                candidate.len() as u64,
            )
            .unwrap();
        stage.write_chunk(candidate).unwrap();
        assert_eq!(
            stage.publish(Sha256::digest(candidate).into()),
            Err(PayloadError::Io("injected_publication_boundary")),
            "{failure_boundary}"
        );
        assert_eq!(
            *trace.lock().unwrap(),
            *expected_trace,
            "{failure_boundary}"
        );
        assert_payload(&store, 0x20, sentinel);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn controller_drives_publish_boundaries_in_order_and_records_only_declared_injection() {
    use common::faults::{FaultAction, FaultController, FaultId, InjectedIo};
    let root = root();
    let mut store = PayloadStore::open(&root).unwrap();
    let sentinel = b"ordered sentinel exact bytes";
    publish(&store, 8, 0x40, sentinel);
    let faults = FaultController::default();
    let boundaries = [
        "publish.before_write",
        "publish.before_file_sync",
        "publish.before_staging_dir_sync",
        "publish.before_ancestor_sync",
        "publish.before_noreplace",
        "publish.before_final_dir_sync",
    ];
    let failed = boundaries[5];
    for boundary in boundaries {
        faults
            .arm(
                FaultId {
                    run_id: "synthetic".into(),
                    case_id: "publication-order".into(),
                    operation: "publish".into(),
                    boundary: boundary.into(),
                    occurrence: 1,
                },
                if boundary == failed {
                    FaultAction::IoError {
                        error: InjectedIo::Eio,
                    }
                } else {
                    FaultAction::Pause
                },
            )
            .unwrap();
    }
    let controller = faults.clone();
    store.set_publish_fault_hook(Some(Arc::new(move |boundary| {
        let id = FaultId {
            run_id: "synthetic".into(),
            case_id: "publication-order".into(),
            operation: "publish".into(),
            boundary: boundary.into(),
            occurrence: 1,
        };
        match controller
            .trigger(&id)
            .map_err(|_| PayloadError::Io("controller_failed"))?
        {
            Some(FaultAction::IoError { .. }) => {
                Err(PayloadError::Io("injected_publication_boundary"))
            }
            Some(FaultAction::Pause) | None => Ok(()),
            _ => Err(PayloadError::Io("unexpected_fault_action")),
        }
    })));
    let candidate = b"not committed before final directory sync";
    let mut stage = store
        .start(
            &format!("{}-{}", payload_id(9), payload_id(0x41)),
            candidate.len() as u64,
        )
        .unwrap();
    stage.write_chunk(candidate).unwrap();
    assert_eq!(
        stage.publish(Sha256::digest(candidate).into()),
        Err(PayloadError::Io("injected_publication_boundary"))
    );
    let events = faults.trace().unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.boundary.as_str())
            .collect::<Vec<_>>(),
        boundaries
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.action == "injected")
            .count(),
        boundaries.len()
    );
    assert_eq!(
        events.last().unwrap().boundary,
        "publish.before_final_dir_sync"
    );
    faults.assert_drained().unwrap();
    // The only injected failure precedes final-directory fsync; publish cannot return success.
    assert_payload(&store, 0x40, sentinel);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
