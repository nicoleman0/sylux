//! Stable mutation failure classes. Internal driver details stay behind this boundary.
//!
//! No class is a retry signal. In particular, an active or unknown transaction
//! requires durable read-back/recovery, never blind repetition of domain effects.

use crate::persistence::sqlite::{TransactionError, WorkerError};
use axum::http::StatusCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationFailure {
    /// No COMMIT occurred and the adapter established rejection/rollback.
    DefiniteRejection,
    /// SQLite still reported an active transaction; its outcome needs recovery.
    TransactionActive,
    /// COMMIT was requested, but its outcome could not be established.
    CommitOutcomeUnknown,
    /// Work was definitely not admitted, or a read-only worker operation failed.
    AdmissionUnavailable,
    /// A worker result was lost or invalid after execution may have occurred.
    OperationOutcomeUnknown,
}

impl MutationFailure {
    pub fn from_worker(error: &WorkerError) -> Self {
        match error {
            WorkerError::Transaction(TransactionError::Definite(_)) => Self::DefiniteRejection,
            WorkerError::Transaction(TransactionError::Active(_)) => Self::TransactionActive,
            WorkerError::Transaction(TransactionError::Unknown(_)) => Self::CommitOutcomeUnknown,
            WorkerError::QueueFull | WorkerError::Query(_) => Self::AdmissionUnavailable,
            // These variants do not prove whether an accepted job ran. Keep the
            // conservative unknown classification even when closure may have
            // preceded submission; the worker API does not carry that phase.
            WorkerError::Closed | WorkerError::Panicked | WorkerError::InvalidResponse => {
                Self::OperationOutcomeUnknown
            }
        }
    }

    pub fn status(self) -> StatusCode {
        match self {
            Self::DefiniteRejection => StatusCode::CONFLICT,
            Self::TransactionActive
            | Self::CommitOutcomeUnknown
            | Self::AdmissionUnavailable
            | Self::OperationOutcomeUnknown => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::DefiniteRejection => "mutation_rejected",
            Self::TransactionActive => "mutation_transaction_active",
            Self::CommitOutcomeUnknown | Self::OperationOutcomeUnknown => {
                "mutation_outcome_unknown"
            }
            Self::AdmissionUnavailable => "mutation_unavailable",
        }
    }

    pub fn requires_durable_reconciliation(self) -> bool {
        matches!(
            self,
            Self::TransactionActive | Self::CommitOutcomeUnknown | Self::OperationOutcomeUnknown
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Error;

    fn transaction(error: TransactionError) -> WorkerError {
        WorkerError::Transaction(error)
    }

    #[test]
    fn transaction_classes_preserve_definite_active_and_unknown_commit() {
        let definite = MutationFailure::from_worker(&transaction(TransactionError::Definite(
            Error::QueryReturnedNoRows,
        )));
        assert_eq!(definite, MutationFailure::DefiniteRejection);
        assert_eq!(definite.status(), StatusCode::CONFLICT);
        assert_eq!(definite.code(), "mutation_rejected");
        assert!(!definite.requires_durable_reconciliation());

        let active = MutationFailure::from_worker(&transaction(TransactionError::Active(
            Error::QueryReturnedNoRows,
        )));
        assert_eq!(active, MutationFailure::TransactionActive);
        assert_eq!(active.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(active.code(), "mutation_transaction_active");
        assert!(active.requires_durable_reconciliation());

        let unknown = MutationFailure::from_worker(&transaction(TransactionError::Unknown(
            Error::QueryReturnedNoRows,
        )));
        assert_eq!(unknown, MutationFailure::CommitOutcomeUnknown);
        assert_eq!(unknown.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(unknown.code(), "mutation_outcome_unknown");
        assert!(unknown.requires_durable_reconciliation());
    }

    #[test]
    fn worker_response_loss_is_unknown_not_a_safe_admission_rejection() {
        for error in [
            WorkerError::Closed,
            WorkerError::Panicked,
            WorkerError::InvalidResponse,
        ] {
            let class = MutationFailure::from_worker(&error);
            assert_eq!(class, MutationFailure::OperationOutcomeUnknown);
            assert_eq!(class.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(class.code(), "mutation_outcome_unknown");
            assert!(class.requires_durable_reconciliation());
        }
    }

    #[test]
    fn queue_and_read_failures_do_not_claim_a_transaction_outcome() {
        for error in [
            WorkerError::QueueFull,
            WorkerError::Query(Error::QueryReturnedNoRows),
        ] {
            let class = MutationFailure::from_worker(&error);
            assert_eq!(class, MutationFailure::AdmissionUnavailable);
            assert_eq!(class.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(class.code(), "mutation_unavailable");
            assert!(!class.requires_durable_reconciliation());
        }
    }
}
