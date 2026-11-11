#[path = "../common/mod.rs"]
mod common;

use common::faults::{FaultAction, FaultController, FaultId, InjectedIo};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::DirBuilderExt, sync::Arc};
use sylux::persistence::payload::{PayloadError, PayloadStore};

fn id(boundary: &str) -> FaultId {
    FaultId {
        run_id: "payload-seam".into(),
        case_id: "publish".into(),
        operation: "publish".into(),
        boundary: boundary.into(),
        occurrence: 1,
    }
}

#[test]
fn payload_publish_fault_controller_arms_triggers_and_drains_both_seams() {
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).unwrap();
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let root = std::env::temp_dir().join(format!("sylux-payload-seam-{suffix}"));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();

    let faults = FaultController::default();
    let boundaries = ["publish.before_noreplace", "publish.before_final_dir_sync"];
    for boundary in boundaries {
        faults
            .arm(
                id(boundary),
                FaultAction::IoError {
                    error: InjectedIo::Eio,
                },
            )
            .unwrap();
    }

    let controller = faults.clone();
    let mut store = PayloadStore::open(&root).unwrap();
    store.set_publish_fault_hook(Some(Arc::new(move |boundary| {
        match controller
            .trigger(&id(boundary))
            .map_err(|_| PayloadError::Io("fault_controller_failed"))?
        {
            Some(FaultAction::IoError { .. }) => Err(PayloadError::Io("injected_publish_fault")),
            Some(_) => Err(PayloadError::Io("unexpected_fault_action")),
            None => Ok(()),
        }
    })));

    for (index, boundary) in boundaries.into_iter().enumerate() {
        let attempt = format!("{:032x}", index + 1);
        let blob = format!("{:032x}", index + 101);
        let staging = store.start(&format!("{attempt}-{blob}"), 0).unwrap();
        let error = staging.publish(Sha256::digest([]).into()).unwrap_err();
        assert_eq!(
            error,
            PayloadError::Io("injected_publish_fault"),
            "{boundary}"
        );
    }

    let trace = faults.trace().unwrap();
    // Only the two armed seams are injected; every other boundary the richer
    // publish() flow passes (write/file-sync/staging-dir-sync/ancestor-sync) is
    // merely observed, returning None without faulting.
    let injected: Vec<&crate::common::faults::TraceEvent> = trace
        .iter()
        .filter(|event| event.action == "injected")
        .collect();
    assert_eq!(injected.len(), 2);
    assert_eq!(injected[0].boundary, "publish.before_noreplace");
    assert_eq!(injected[1].boundary, "publish.before_final_dir_sync");
    faults.assert_drained().unwrap();
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
