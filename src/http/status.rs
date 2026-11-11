use super::AuthState;
use crate::persistence::sqlite::ConnectionRole;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
};
use serde::Serialize;

pub async fn health() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

/// Liveness is process health only; it does not imply storage ownership.
pub async fn live() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

/// Readiness is served only after startup ownership/binding/recovery succeeded,
/// and rechecks the actual owned writer and reader policy on each probe.
pub async fn ready(State(state): State<AuthState>) -> impl IntoResponse {
    let writer = state.workers.policy_readback(ConnectionRole::Writer).await;
    let reader = state.workers.policy_readback(ConnectionRole::Reader).await;
    if writer.is_ok() && reader.is_ok() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

#[derive(Serialize)]
struct OperatorStatus {
    database_policy: &'static str,
    recent_events: Vec<AuditEvent>,
}

#[derive(Serialize)]
struct AuditEvent {
    actor_account_id: Option<String>,
    action: String,
    occurred_at: i64,
    outcome: String,
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn error(status: StatusCode, code: &'static str) -> Response {
    no_store((status, Json(serde_json::json!({ "code": code }))).into_response())
}

/// Bounded operational view for an authenticated application administrator.
/// It does not imply payload integrity, recovery completion or disk capacity.
pub async fn operator_status(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let Some(cookie) = super::middleware::session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "authentication_required");
    };
    let session_id = cookie.session_id;
    let secret = cookie.secret;
    let status = state.workers.read(move |db| {
        let Some((account_id, _handle, role)) = crate::domain::authz::validate_session(
            db,
            &session_id,
            &secret,
            super::auth::now(),
        ) else {
            return Ok((None, false, Vec::new()));
        };
        if role != "administrator" {
            return Ok((Some(account_id), false, Vec::new()));
        }
        let mut statement = db.prepare(
            "SELECT actor_account_id,action,occurred_at,outcome FROM event ORDER BY event_id DESC LIMIT 100",
        )?;
        let events = statement
            .query_map([], |row| {
                let action: String = row.get(1)?;
                Ok(AuditEvent {
                    actor_account_id: row.get(0)?,
                    action: if action.len() <= 128
                        && action
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                    {
                        action
                    } else {
                        "redacted".to_owned()
                    },
                    occurred_at: row.get(2)?,
                    outcome: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((Some(account_id), true, events))
    })
    .await;
    match status {
        Ok((None, _, _)) => error(StatusCode::UNAUTHORIZED, "authentication_required"),
        Ok((Some(_), false, _)) => error(StatusCode::FORBIDDEN, "administrator_required"),
        Ok((Some(_), true, events)) => {
            let writer = state.workers.policy_readback(ConnectionRole::Writer).await;
            let reader = state.workers.policy_readback(ConnectionRole::Reader).await;
            let database_policy = if writer.is_ok() && reader.is_ok() {
                "verified"
            } else {
                "unavailable"
            };
            no_store(
                Json(OperatorStatus {
                    database_policy,
                    recent_events: events,
                })
                .into_response(),
            )
        }
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "operator_status_unavailable",
        ),
    }
}
