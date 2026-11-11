//! Test-only fault controls. Never compiled into the default production artifact.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FaultId {
    pub run_id: String,
    pub case_id: String,
    pub operation: String,
    pub boundary: String,
    pub occurrence: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FaultAction {
    ShortWrite { bytes: usize },
    IoError { error: InjectedIo },
    Pause,
    DefiniteReject,
    UncertainResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InjectedIo {
    Eio,
    Enospc,
    Exdev,
    Unsupported,
    Sync,
}

fn known_boundary(boundary: &str) -> bool {
    [
        "init.after_lock",
        "init.before_manifest_publish",
        "startup.before_db_open",
        "startup.before_recovery",
        "startup.before_listen",
        "stage.before_write",
        "stage.after_write",
        "stage.before_file_sync",
        "stage.before_dir_sync",
        "stage.before_completed_record",
        "stage.after_completed_record",
        "publish.before_write",
        "publish.before_file_sync",
        "publish.before_staging_dir_sync",
        "publish.before_noreplace",
        "publish.after_noreplace",
        "publish.before_final_dir_sync",
        "publish.before_ancestor_sync",
        "mutation.before_final_auth",
        "mutation.before_sql_commit",
        "mutation.after_sql_commit_before_result",
        "http.before_commit_response",
        "ownership.before_loss",
        "ownership.after_loss",
        "shutdown.before_worker_join",
        "shutdown.before_unlock",
        "setup.before_consume",
        "session.before_revoke",
        "listing.after_page",
    ]
    .contains(&boundary)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    pub sequence: u64,
    pub boundary: String,
    pub action: String,
}

/// Per-instance one-shot fault map; no process-global environment flag or shared queue.
#[derive(Clone, Default)]
pub struct FaultController {
    inner: Arc<Mutex<Inner>>,
}
#[derive(Default)]
struct Inner {
    armed: BTreeMap<FaultId, FaultAction>,
    sequence: u64,
    trace: Vec<TraceEvent>,
}
impl FaultController {
    pub fn arm(&self, id: FaultId, action: FaultAction) -> Result<(), String> {
        if !known_boundary(&id.boundary) || id.occurrence == 0 {
            return Err("unknown boundary or zero occurrence".into());
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "fault mutex poisoned".to_owned())?;
        if inner.armed.contains_key(&id) {
            return Err("fault already armed".into());
        }
        if inner.armed.len() >= 4096 {
            return Err("fault budget exhausted".into());
        }
        inner.armed.insert(id, action);
        Ok(())
    }
    pub fn trigger(&self, id: &FaultId) -> Result<Option<FaultAction>, String> {
        if !known_boundary(&id.boundary) || id.occurrence == 0 {
            return Err("unknown boundary or zero occurrence".into());
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "fault mutex poisoned".to_owned())?;
        if inner.trace.len() >= 4096 {
            return Err("trace budget exhausted".into());
        }
        inner.sequence += 1;
        let action = inner.armed.remove(id);
        let sequence = inner.sequence;
        inner.trace.push(TraceEvent {
            sequence,
            boundary: id.boundary.clone(),
            action: if action.is_some() {
                "injected"
            } else {
                "observed"
            }
            .into(),
        });
        Ok(action)
    }
    pub fn assert_drained(&self) -> Result<(), String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "fault mutex poisoned".to_owned())?;
        if inner.armed.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{} armed fault(s) were not reached",
                inner.armed.len()
            ))
        }
    }
    pub fn trace(&self) -> Result<Vec<TraceEvent>, String> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| "fault mutex poisoned".to_owned())?
            .trace
            .clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> FaultId {
        FaultId {
            run_id: "r".into(),
            case_id: "c".into(),
            operation: "op".into(),
            boundary: "stage.before_write".into(),
            occurrence: 1,
        }
    }
    #[test]
    fn faults_are_one_shot_and_scoped_to_controller() {
        let a = FaultController::default();
        let b = FaultController::default();
        a.arm(id(), FaultAction::ShortWrite { bytes: 2 }).unwrap();
        assert!(matches!(
            a.trigger(&id()).unwrap(),
            Some(FaultAction::ShortWrite { bytes: 2 })
        ));
        assert_eq!(a.trigger(&id()).unwrap(), None);
        assert_eq!(b.trigger(&id()).unwrap(), None);
        a.assert_drained().unwrap();
    }
    #[test]
    fn duplicate_arm_preserves_original_action() {
        let faults = FaultController::default();
        faults
            .arm(id(), FaultAction::ShortWrite { bytes: 2 })
            .unwrap();
        assert!(faults.arm(id(), FaultAction::Pause).is_err());
        assert_eq!(
            faults.trigger(&id()).unwrap(),
            Some(FaultAction::ShortWrite { bytes: 2 })
        );
    }
    #[test]
    fn missing_fault_reach_is_not_silent() {
        let faults = FaultController::default();
        faults.arm(id(), FaultAction::Pause).unwrap();
        assert!(faults.assert_drained().unwrap_err().contains("not reached"));
        let secret = crate::common::unique_token();
        let mut invalid = id();
        invalid.boundary = secret.clone();
        assert!(faults.arm(invalid.clone(), FaultAction::Pause).is_err());
        assert!(faults.trigger(&invalid).is_err());
        assert!(
            !serde_json::to_string(&faults.trace().unwrap())
                .unwrap()
                .contains(&secret)
        );
    }
}
