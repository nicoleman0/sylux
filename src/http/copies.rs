#![allow(clippy::result_large_err)]
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::Connection;
use serde::Deserialize;

use crate::{
    domain::{
        authz,
        copy::{CopyError, Ticket},
        namespace::NamespaceError,
    },
    http::{
        AuthState,
        auth::now,
        middleware::{SessionCookie, decode_secret, origin_matches, session_cookie},
    },
    persistence::sqlite::{TransactionError, WorkerError},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareInput {
    parent_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyInput {
    ticket: String,
}

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(serde_json::json!({"code": code}))).into_response()
}

fn unavailable() -> Response {
    error(StatusCode::SERVICE_UNAVAILABLE, "copy_unavailable")
}

fn not_committed() -> Response {
    error(StatusCode::CONFLICT, "copy_not_committed")
}

fn failure(failure: CopyError) -> Response {
    match failure {
        CopyError::InvalidTicket => error(StatusCode::BAD_REQUEST, "invalid_copy_ticket"),
        CopyError::OtherAccount => error(StatusCode::FORBIDDEN, "copy_other_account"),
        CopyError::Expired => error(StatusCode::CONFLICT, "copy_ticket_expired"),
        CopyError::Namespace(NamespaceError::NotFound) => error(StatusCode::NOT_FOUND, "not_found"),
        CopyError::Namespace(NamespaceError::Stale) => {
            error(StatusCode::PRECONDITION_FAILED, "stale_etag")
        }
        CopyError::Namespace(NamespaceError::Conflict) => {
            error(StatusCode::CONFLICT, "name_conflict")
        }
        CopyError::Namespace(NamespaceError::InvalidDestination) => {
            error(StatusCode::CONFLICT, "invalid_destination")
        }
        CopyError::Namespace(NamespaceError::DepthLimit) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, "depth_limit")
        }
        CopyError::Namespace(NamespaceError::CopyTooLarge) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, "copy_too_large")
        }
        _ => not_committed(),
    }
}

fn credentials(
    s: &AuthState,
    headers: &HeaderMap,
) -> Result<(SessionCookie, [u8; 32], [u8; 32]), Response> {
    let cookie =
        session_cookie(headers).ok_or_else(|| error(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .and_then(decode_secret)
        .filter(|_| origin_matches(headers, &s.origin))
        .ok_or_else(|| error(StatusCode::FORBIDDEN, "forbidden"))?;
    let key = s
        .storage
        .as_ref()
        .ok_or_else(unavailable)?
        .namespace_signing_key();
    Ok((cookie, csrf, key))
}

fn owner(
    db: &Connection,
    cookie: &SessionCookie,
    csrf: &[u8; 32],
    now: i64,
) -> Result<String, Response> {
    let Some((owner, _, _)) = authz::validate_session(db, &cookie.session_id, &cookie.secret, now)
    else {
        return Err(error(StatusCode::UNAUTHORIZED, "unauthorized"));
    };
    if !authz::csrf_matches(db, &cookie.session_id, csrf) {
        return Err(error(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(owner)
}

pub async fn prepare(
    State(s): State<AuthState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    Json(input): Json<PrepareInput>,
) -> Result<Json<serde_json::Value>, Response> {
    let (cookie, csrf, key) = credentials(&s, &headers)?;
    let etag = headers
        .get("if-match")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| error(StatusCode::PRECONDITION_REQUIRED, "if_match_required"))?
        .to_owned();
    s.workers
        .read(move |db| {
            let tx = db.unchecked_transaction()?;
            let now = now();
            Ok(owner(&tx, &cookie, &csrf, now).and_then(|owner| {
                Ticket::issue(&key, owner, source, input.parent_id, etag, now)
                    .map(|ticket| Json(serde_json::json!({"ticket": ticket})))
                    .map_err(failure)
            }))
        })
        .await
        .map_err(|_| unavailable())?
}

pub async fn execute(
    State(s): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<CopyInput>,
) -> Result<Json<serde_json::Value>, Response> {
    let (cookie, csrf, key) = credentials(&s, &headers)?;
    let Ok(guard) = s.coordinator.serialize() else {
        return Err(unavailable());
    };
    drop(guard);
    #[cfg(feature = "test-harness")]
    let commit_hook = s.copy_commit_hook.clone();
    let result = s
        .workers
        .transact_result(move |tx| {
            let now = now();
            let owner = owner(tx, &cookie, &csrf, now)?;
            let ticket = Ticket::verify(&key, &owner, &input.ticket).map_err(failure)?;
            let state = ticket.execute(tx, now).map_err(failure)?;
            #[cfg(feature = "test-harness")]
            if let Some(hook) = commit_hook {
                hook().map_err(|_| not_committed())?;
            }
            Ok(Json(serde_json::json!({"state": state})))
        })
        .await;
    match result {
        Ok(response) => response,
        Err(WorkerError::Transaction(TransactionError::Definite(_)) | WorkerError::QueueFull) => {
            Err(not_committed())
        }
        Err(_) => Err(unavailable()),
    }
}

pub async fn status(
    State(s): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<CopyInput>,
) -> Result<Json<serde_json::Value>, Response> {
    let (cookie, csrf, key) = credentials(&s, &headers)?;
    s.workers
        .read(move |db| {
            let tx = db.unchecked_transaction()?;
            Ok(owner(&tx, &cookie, &csrf, now()).and_then(|owner| {
                let ticket = Ticket::verify(&key, &owner, &input.ticket).map_err(failure)?;
                let state = ticket.status(&tx).map_err(|_| unavailable())?;
                Ok(Json(serde_json::json!({"state": state})))
            }))
        })
        .await
        .map_err(|_| unavailable())?
}
