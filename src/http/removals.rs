#![allow(clippy::result_large_err)]
use crate::{
    domain::{
        authz,
        removal::{self, RemovalError},
    },
    http::{
        AuthState,
        middleware::{decode_secret, origin_matches, session_cookie},
    },
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    selection: removal::Selection,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteInput {
    operation_id: String,
    confirmation: String,
}
fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(serde_json::json!({"code":code}))).into_response()
}
fn failure(e: RemovalError) -> Response {
    match e {
        RemovalError::Invalid => error(StatusCode::BAD_REQUEST, "invalid_confirmation"),
        RemovalError::Changed => error(StatusCode::CONFLICT, "listing_changed"),
        RemovalError::NotFound => error(StatusCode::NOT_FOUND, "not_found"),
        RemovalError::Conflict => error(StatusCode::CONFLICT, "operation_conflict"),
        RemovalError::Sql => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}
fn key(s: &AuthState) -> Result<[u8; 32], Response> {
    s.storage
        .as_ref()
        .map(|storage| storage.namespace_signing_key())
        .ok_or_else(|| error(StatusCode::SERVICE_UNAVAILABLE, "removal_unavailable"))
}
fn csrf(s: &AuthState, headers: &HeaderMap) -> Result<[u8; 32], Response> {
    if !origin_matches(headers, &s.origin) {
        return Err(error(StatusCode::FORBIDDEN, "forbidden"));
    }
    headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .and_then(decode_secret)
        .ok_or_else(|| error(StatusCode::FORBIDDEN, "forbidden"))
}
pub async fn preview(
    State(s): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<PreviewInput>,
) -> Response {
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let token = match csrf(&s, &headers) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let key = match key(&s) {
        Ok(key) => key,
        Err(response) => return response,
    };
    let result = s
        .workers
        .read(move |db| {
            let tx = db.unchecked_transaction()?;
            let Some((owner, _, _)) = authz::validate_session(
                &tx,
                &cookie.session_id,
                &cookie.secret,
                crate::http::auth::now(),
            ) else {
                return Ok(Err(error(StatusCode::UNAUTHORIZED, "unauthorized")));
            };
            if !authz::csrf_matches(&tx, &cookie.session_id, &token) {
                return Ok(Err(error(StatusCode::FORBIDDEN, "forbidden")));
            }
            Ok(
                removal::preview(&tx, &owner, input.selection, &key, crate::http::auth::now())
                    .map_err(failure),
            )
        })
        .await;
    match result {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(r)) => r,
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "removal_unavailable"),
    }
}
pub async fn execute(
    State(s): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<ExecuteInput>,
) -> Response {
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let token = match csrf(&s, &headers) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let key = match key(&s) {
        Ok(key) => key,
        Err(_) => return error(StatusCode::CONFLICT, "removal_not_committed"),
    };
    let Ok(guard) = s.coordinator.serialize() else {
        return error(StatusCode::CONFLICT, "removal_not_committed");
    };
    drop(guard);
    #[cfg(feature = "test-harness")]
    let commit_hook = s.removal_commit_hook.clone();
    let result = s
        .workers
        .transact_result(move |tx| {
            let Some((owner, _, _)) = authz::validate_session(
                tx,
                &cookie.session_id,
                &cookie.secret,
                crate::http::auth::now(),
            ) else {
                return Err(error(StatusCode::UNAUTHORIZED, "unauthorized"));
            };
            if !authz::csrf_matches(tx, &cookie.session_id, &token) {
                return Err(error(StatusCode::FORBIDDEN, "forbidden"));
            }
            match removal::execute(
                tx,
                &owner,
                &input.operation_id,
                &input.confirmation,
                &key,
                crate::http::auth::now(),
            ) {
                Ok(value) => {
                    #[cfg(feature = "test-harness")]
                    if let Some(hook) = &commit_hook {
                        hook("removal.before_commit")
                            .map_err(|_| error(StatusCode::CONFLICT, "removal_not_committed"))?;
                    }
                    Ok(value)
                }
                Err(RemovalError::Sql) => Err(error(StatusCode::CONFLICT, "removal_not_committed")),
                Err(e) => Err(failure(e)),
            }
        })
        .await;
    match result {
        Ok(Ok(value)) => {
            s.cleanup_wake.notify_one();
            Json(value).into_response()
        }
        Ok(Err(r)) => r,
        Err(
            crate::persistence::sqlite::WorkerError::Transaction(
                crate::persistence::sqlite::TransactionError::Definite(_),
            )
            | crate::persistence::sqlite::WorkerError::QueueFull,
        ) => error(StatusCode::CONFLICT, "removal_not_committed"),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "removal_unavailable"),
    }
}
pub async fn status(
    State(s): State<AuthState>,
    headers: HeaderMap,
    Path(operation): Path<String>,
) -> Response {
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let result = s
        .workers
        .read(move |db| {
            let tx = db.unchecked_transaction()?;
            let Some((owner, _, _)) = authz::validate_session(
                &tx,
                &cookie.session_id,
                &cookie.secret,
                crate::http::auth::now(),
            ) else {
                return Ok(Err(error(StatusCode::UNAUTHORIZED, "unauthorized")));
            };
            Ok(removal::status(&tx, &owner, &operation).map_err(failure))
        })
        .await;
    match result {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(r)) => r,
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "removal_unavailable"),
    }
}
