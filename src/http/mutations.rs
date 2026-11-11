//! Authenticated durable mutation admission and safe outcome reconciliation.
//!
//! The signed ticket and operation ID are never credentials. Both handlers pass
//! the cookie session to the coordinator, which validates the current account
//! against the requested principal before admitting work or revealing an
//! outcome. No handler retries a domain effect after an ambiguous result.

use super::middleware::session_cookie;
use crate::{
    domain::{
        coordinator::{CoordinatorError, MutationCoordinator},
        ids::{
            MutationId, MutationScope, MutationTicket, PrincipalId, ScopedMutationId,
            TicketExpectation, TicketSigningKey, TicketValidationError,
        },
        mutation::{CommittedResult, MutationResult},
        upload::hex,
    },
    error::MutationFailure,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, LOCATION},
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

const ADMISSION_DOMAIN: &[u8] = b"sylux-mutation-admission-v1";
const RECOVERY_HINT: &str = "Reconcile the same mutation ID and scope through the status Location with current authorization; do not create a new mutation.";

#[derive(Clone)]
pub struct MutationHttpState {
    pub coordinator: MutationCoordinator,
    pub keys: Arc<[TicketSigningKey]>,
}

impl MutationHttpState {
    pub fn new(coordinator: MutationCoordinator, keys: Arc<[TicketSigningKey]>) -> Self {
        Self { coordinator, keys }
    }
}

/// Register the self-contained mutation routes. The supplied coordinator must
/// be constructed with `MutationCoordinator::new(SqliteWorkers)`; the legacy
/// default coordinator deliberately fails durable operations closed.
pub fn register(router: Router, state: MutationHttpState) -> Router {
    let routes = Router::new()
        .route("/mutations", post(admit))
        .route("/mutations/{id}", get(status))
        .with_state(state);
    router.merge(routes)
}

#[derive(Deserialize)]
struct AdmissionRequest {
    scope: String,
    #[serde(alias = "principal_id")]
    principal: String,
    recovery_epoch: i64,
    /// Operation-specific fields are flattened into the request envelope. A
    /// sorted map makes their fingerprint order independent of JSON key order.
    #[serde(flatten)]
    operation: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct StatusQuery {
    scope: String,
    #[serde(alias = "principal_id")]
    principal: String,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn finish(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn error(
    status: StatusCode,
    code: &'static str,
    mutation_id: Option<&str>,
    recovery_hint: bool,
) -> Response {
    let mut body = Map::new();
    body.insert("code".to_owned(), Value::String(code.to_owned()));
    body.insert("message".to_owned(), Value::String(code.to_owned()));
    body.insert("request_id".to_owned(), Value::String("poc".to_owned()));
    if let Some(mutation_id) = mutation_id {
        body.insert(
            "mutation_id".to_owned(),
            Value::String(mutation_id.to_owned()),
        );
    }
    if recovery_hint {
        body.insert(
            "recovery_hint".to_owned(),
            Value::String(RECOVERY_HINT.to_owned()),
        );
    }
    finish((status, Json(Value::Object(body))).into_response())
}

fn with_location(mut response: Response, mutation_id: &str) -> Response {
    match HeaderValue::from_str(&format!("/mutations/{mutation_id}")) {
        Ok(location) => {
            response.headers_mut().insert(LOCATION, location);
            response
        }
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "response_encoding_failed",
            None,
            false,
        ),
    }
}

fn with_mutation_header(mut response: Response, mutation_id: &str) -> Response {
    match HeaderValue::from_str(mutation_id) {
        Ok(value) => {
            response.headers_mut().insert("mutation-id", value);
            response
        }
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "response_encoding_failed",
            None,
            false,
        ),
    }
}

fn recovery_error(status: StatusCode, code: &'static str, mutation_id: &str) -> Response {
    with_location(error(status, code, Some(mutation_id), true), mutation_id)
}

fn committed_response(committed: CommittedResult) -> Response {
    finish(
        (
            StatusCode::OK,
            Json(json!({
                "revision_id": committed.revision_id,
                "entry_id": committed.entry_id,
                "length": committed.length,
                "sha256": hex(&committed.sha256),
            })),
        )
            .into_response(),
    )
}

fn result_response(result: MutationResult, mutation_id: &str) -> Response {
    match result {
        MutationResult::Committed(committed) => committed_response(committed),
        MutationResult::Rejected => {
            finish((StatusCode::OK, Json(json!({"state": "rejected"}))).into_response())
        }
        MutationResult::Pending => with_location(
            finish((StatusCode::ACCEPTED, Json(json!({"state": "pending"}))).into_response()),
            mutation_id,
        ),
        MutationResult::Unknown => recovery_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "mutation_outcome_unknown",
            mutation_id,
        ),
    }
}

fn one_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let name = axum::http::HeaderName::from_bytes(name.as_bytes()).ok()?;
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}

fn ticket_error(validation: TicketValidationError) -> Response {
    match validation {
        TicketValidationError::Expired => {
            error(StatusCode::GONE, "mutation_ticket_expired", None, false)
        }
        TicketValidationError::InvalidClock => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "mutation_unavailable",
            None,
            false,
        ),
        _ => error(
            StatusCode::UNAUTHORIZED,
            "invalid_mutation_ticket",
            None,
            false,
        ),
    }
}

fn admission_error(coordinator_error: CoordinatorError, mutation_id: &str) -> Response {
    let response = match coordinator_error {
        CoordinatorError::InvalidSession => error(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            None,
            false,
        ),
        CoordinatorError::Conflict => error(
            StatusCode::CONFLICT,
            "admission_fingerprint_mismatch",
            Some(mutation_id),
            false,
        ),
        CoordinatorError::Expired => error(
            StatusCode::GONE,
            "mutation_ticket_expired",
            Some(mutation_id),
            false,
        ),
        CoordinatorError::InvalidState => recovery_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "mutation_state_unavailable",
            mutation_id,
        ),
        CoordinatorError::Failure(MutationFailure::DefiniteRejection) => error(
            StatusCode::CONFLICT,
            MutationFailure::DefiniteRejection.code(),
            Some(mutation_id),
            false,
        ),
        CoordinatorError::Failure(failure) => {
            recovery_error(StatusCode::SERVICE_UNAVAILABLE, failure.code(), mutation_id)
        }
    };
    with_mutation_header(response, mutation_id)
}

fn status_error(coordinator_error: CoordinatorError, mutation_id: &str) -> Response {
    match coordinator_error {
        CoordinatorError::InvalidSession => {
            error(StatusCode::NOT_FOUND, "mutation_not_found", None, false)
        }
        CoordinatorError::Failure(failure) => {
            recovery_error(StatusCode::SERVICE_UNAVAILABLE, failure.code(), mutation_id)
        }
        _ => recovery_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "mutation_unavailable",
            mutation_id,
        ),
    }
}

async fn admit(
    State(state): State<MutationHttpState>,
    headers: HeaderMap,
    Json(request): Json<AdmissionRequest>,
) -> Response {
    let Some(cookie) = session_cookie(&headers) else {
        return error(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            None,
            false,
        );
    };
    let Ok(principal) = PrincipalId::parse(&request.principal) else {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_mutation_principal",
            None,
            false,
        );
    };
    let Ok(scope) = MutationScope::parse(&request.scope) else {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_mutation_scope",
            None,
            false,
        );
    };
    if request.recovery_epoch < 0 {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_recovery_epoch",
            None,
            false,
        );
    }
    let Some(raw_ticket) = one_header(&headers, "x-mutation-id") else {
        return error(
            StatusCode::BAD_REQUEST,
            "mutation_ticket_required",
            None,
            false,
        );
    };
    let now = now();
    let expectation = TicketExpectation::new(&principal, &scope, request.recovery_epoch, now);
    let ticket = match MutationTicket::validate(raw_ticket, &expectation, &state.keys) {
        Ok(ticket) => ticket,
        Err(error) => return ticket_error(error),
    };
    let fingerprint = match admission_fingerprint(&ticket, &request.operation) {
        Ok(fingerprint) => fingerprint,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_mutation_admission",
                None,
                false,
            );
        }
    };
    let mutation_id = ticket.mutation_id().as_str().to_owned();

    // The principal above is only a validation selector, not authority. This
    // durable call revalidates the cookie session and requires its current
    // account to equal the ticket principal before any mutation is admitted.
    let result = state
        .coordinator
        .admit_durable(
            &ticket,
            fingerprint,
            &cookie.session_id,
            &cookie.secret,
            now,
        )
        .await;
    match result {
        Ok(result) => with_mutation_header(result_response(result, &mutation_id), &mutation_id),
        Err(error) => admission_error(error, &mutation_id),
    }
}

async fn status(
    State(state): State<MutationHttpState>,
    Path(mutation_id): Path<String>,
    Query(query): Query<StatusQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(cookie) = session_cookie(&headers) else {
        return error(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            None,
            false,
        );
    };
    let (Ok(mutation_id), Ok(principal)) = (
        MutationId::parse(&mutation_id),
        PrincipalId::parse(&query.principal),
    ) else {
        return error(StatusCode::NOT_FOUND, "mutation_not_found", None, false);
    };
    let Ok(scope) = MutationScope::parse(&query.scope) else {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_mutation_scope",
            None,
            false,
        );
    };
    let mutation_id_text = mutation_id.as_str().to_owned();
    let operation = ScopedMutationId::new(principal, mutation_id);

    // `status` validates the current session and principal on one SQLite read
    // snapshot. Possession of the operation ID alone therefore reveals nothing.
    let result = state
        .coordinator
        .status(
            &operation,
            &scope,
            &cookie.session_id,
            &cookie.secret,
            now(),
        )
        .await;
    match result {
        Ok(result) => result_response(result, &mutation_id_text),
        Err(error) => status_error(error, &mutation_id_text),
    }
}

fn admission_fingerprint(
    ticket: &MutationTicket,
    operation: &BTreeMap<String, Value>,
) -> Result<[u8; 32], serde_json::Error> {
    fingerprint_parts(
        ticket.scope().as_str(),
        ticket.principal().as_str(),
        ticket.mutation_id().as_str(),
        ticket.key_id().as_str(),
        ticket.recovery_epoch(),
        operation,
    )
}

fn fingerprint_parts(
    scope: &str,
    principal: &str,
    mutation_id: &str,
    key_id: &str,
    recovery_epoch: i64,
    operation: &BTreeMap<String, Value>,
) -> Result<[u8; 32], serde_json::Error> {
    let mut fields = vec![
        ADMISSION_DOMAIN.to_vec(),
        scope.as_bytes().to_vec(),
        principal.as_bytes().to_vec(),
        mutation_id.as_bytes().to_vec(),
        key_id.as_bytes().to_vec(),
        recovery_epoch.to_be_bytes().to_vec(),
    ];
    for (name, value) in operation {
        fields.push(name.as_bytes().to_vec());
        fields.push(serde_json::to_vec(value)?);
    }

    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_errors_distinguish_conflict_expiry_and_rejection_from_uncertainty() {
        let conflict = admission_error(CoordinatorError::Conflict, "mutation-1");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(conflict.headers()[CACHE_CONTROL], "no-store");

        assert_eq!(
            admission_error(CoordinatorError::Expired, "mutation-1").status(),
            StatusCode::GONE
        );
        let rejected = admission_error(
            CoordinatorError::Failure(MutationFailure::DefiniteRejection),
            "mutation-1",
        );
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
        let active = admission_error(
            CoordinatorError::Failure(MutationFailure::TransactionActive),
            "mutation-1",
        );
        assert_eq!(active.status(), StatusCode::SERVICE_UNAVAILABLE);
        let unknown = admission_error(
            CoordinatorError::Failure(MutationFailure::CommitOutcomeUnknown),
            "mutation-1",
        );
        assert_eq!(unknown.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            admission_error(CoordinatorError::InvalidState, "mutation-1").status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn outcome_states_map_pending_and_unknown_to_reconciliation_statuses() {
        let pending = result_response(MutationResult::Pending, "mutation-1");
        assert_eq!(pending.status(), StatusCode::ACCEPTED);
        assert_eq!(pending.headers()[LOCATION], "/mutations/mutation-1");
        let unknown = result_response(MutationResult::Unknown, "mutation-1");
        assert_eq!(unknown.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            status_error(CoordinatorError::InvalidState, "mutation-1").status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_error(CoordinatorError::InvalidSession, "mutation-1").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn mutation_ticket_without_current_session_cannot_read_result() {
        let response = status(
            State(MutationHttpState::new(
                MutationCoordinator::default(),
                Arc::from([]),
            )),
            Path("22222222222222222222222222222222".to_owned()),
            Query(StatusQuery {
                scope: "upload_create".to_owned(),
                principal: "11111111111111111111111111111111".to_owned(),
            }),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn admission_fingerprint_has_a_stable_canonical_encoding() {
        let operation = BTreeMap::from([
            ("length".to_owned(), json!(42)),
            ("name".to_owned(), json!("alpha")),
            (
                "parent_id".to_owned(),
                json!("33333333333333333333333333333333"),
            ),
        ]);
        let fingerprint = fingerprint_parts(
            "upload_create",
            "11111111111111111111111111111111",
            "22222222222222222222222222222222",
            "key-2026-01",
            7,
            &operation,
        )
        .unwrap();
        assert_eq!(
            hex(&fingerprint),
            "245cfb3d082f477d394a3d305e7043d67a53f030948d8ab67c81636c0ca36845"
        );
    }
}
