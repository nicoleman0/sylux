//! Durable mutation outcome DTOs. Results are the immutable revision identity;
//! the completed body digest is deliberately not used as result identity.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedResult {
    pub revision_id: String,
    pub entry_id: String,
    pub length: u64,
    pub sha256: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationResult {
    Pending,
    Committed(CommittedResult),
    Rejected,
    /// Durable state could not establish whether COMMIT completed.
    Unknown,
}

use std::sync::{Arc, Mutex, Weak};

/// Process-local admission gate. Durable mutation state remains authoritative;
/// this gate only serializes the current execution owner and shutdown/loss.
#[derive(Clone, Default)]
pub struct OwnershipGate(Arc<Mutex<GateState>>);

#[derive(Default)]
struct GateState {
    generation: u64,
    owner: Option<String>,
    stopped: bool,
}

/// Non-cloneable ownership authority; loss or shutdown invalidates it.
pub struct OwnershipPermit {
    gate: Weak<Mutex<GateState>>,
    generation: u64,
    owner: String,
}

/// One-use authority to cross the final execution boundary.
pub struct ExecutionPermit {
    _ownership: OwnershipPermit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnershipError {
    Unavailable,
    CompetingOwner,
    StaleOwner,
}

impl OwnershipGate {
    /// Acquire exclusive ownership. A repeated acquisition by the same owner
    /// returns the existing valid authority only after its previous permit drops.
    pub fn acquire(&self, owner: impl Into<String>) -> Result<OwnershipPermit, OwnershipError> {
        let owner = owner.into();
        let mut state = self.0.lock().map_err(|_| OwnershipError::Unavailable)?;
        if state.stopped {
            return Err(OwnershipError::Unavailable);
        }
        if state.owner.is_some() {
            return Err(OwnershipError::CompetingOwner);
        }
        state.generation = state.generation.wrapping_add(1);
        state.owner = Some(owner.clone());
        Ok(OwnershipPermit {
            gate: Arc::downgrade(&self.0),
            generation: state.generation,
            owner,
        })
    }

    /// Irrevocably stop new admission and final execution boundaries.
    pub fn shutdown(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.stopped = true;
            state.owner = None;
            state.generation = state.generation.wrapping_add(1);
        }
    }
}

impl OwnershipPermit {
    /// Signal lease/owner loss; outstanding permits become stale immediately.
    pub fn lose(&self) {
        if let Some(gate) = self.gate.upgrade()
            && let Ok(mut state) = gate.lock()
            && state.generation == self.generation
            && state.owner.as_deref() == Some(&self.owner)
        {
            state.owner = None;
            state.generation = state.generation.wrapping_add(1);
        }
    }

    /// Create a final boundary only while this remains the current owner.
    pub fn begin_execution(self) -> Result<ExecutionPermit, OwnershipError> {
        let Some(gate) = self.gate.upgrade() else {
            return Err(OwnershipError::StaleOwner);
        };
        let state = gate.lock().map_err(|_| OwnershipError::Unavailable)?;
        if state.stopped {
            return Err(OwnershipError::Unavailable);
        }
        if state.generation != self.generation || state.owner.as_deref() != Some(&self.owner) {
            return Err(OwnershipError::StaleOwner);
        }
        drop(state);
        Ok(ExecutionPermit { _ownership: self })
    }
}

impl Drop for OwnershipPermit {
    fn drop(&mut self) {
        if let Some(gate) = self.gate.upgrade()
            && let Ok(mut state) = gate.lock()
            && state.generation == self.generation
            && state.owner.as_deref() == Some(&self.owner)
        {
            state.owner = None;
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    #[test]
    fn competing_owner_cannot_cross_boundary_until_current_owner_releases() {
        let gate = OwnershipGate::default();
        let owner = gate.acquire("worker-a").unwrap();
        assert_eq!(
            gate.acquire("worker-b").err(),
            Some(OwnershipError::CompetingOwner)
        );
        let _execution = owner.begin_execution().unwrap();
        let replacement = gate.acquire("worker-b");
        assert_eq!(replacement.err(), Some(OwnershipError::CompetingOwner));
    }

    #[test]
    fn ownership_loss_invalidates_stale_permit_and_allows_new_owner() {
        let gate = OwnershipGate::default();
        let stale = gate.acquire("worker-a").unwrap();
        stale.lose();
        assert_eq!(
            stale.begin_execution().err(),
            Some(OwnershipError::StaleOwner)
        );
        gate.acquire("worker-b").unwrap().begin_execution().unwrap();
    }

    #[test]
    fn shutdown_blocks_admission_and_final_boundaries() {
        let gate = OwnershipGate::default();
        let permit = gate.acquire("worker-a").unwrap();
        gate.shutdown();
        assert_eq!(
            permit.begin_execution().err(),
            Some(OwnershipError::Unavailable)
        );
        assert_eq!(
            gate.acquire("worker-b").err(),
            Some(OwnershipError::Unavailable)
        );
    }
}
