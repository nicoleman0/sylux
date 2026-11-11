use super::{AuthState, accounts};
use crate::persistence::instance::{DEFAULT_NAME, normalized_name};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    name: String,
}

fn name_response(name: String) -> Response {
    (
        [(CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"name": name})),
    )
        .into_response()
}

pub async fn get(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !super::middleware::origin_absent_or_matches(&headers, &state.origin) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(settings) = state.instance_settings else {
        return name_response(DEFAULT_NAME.to_owned());
    };
    match tokio::task::spawn_blocking(move || settings.name()).await {
        Ok(Ok(name)) => name_response(name),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn update(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<Update>,
) -> Response {
    let (cookie, csrf) = match accounts::credentials(&headers, &state, true) {
        Ok(value) => value,
        Err(status) => return status.into_response(),
    };
    let authorized = state
        .workers
        .read(move |db| Ok(accounts::authorize(db, &cookie, csrf.as_ref(), true)))
        .await;
    match authorized {
        Ok(Ok(_)) => (),
        Ok(Err(status)) => return status.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    let Some(name) = normalized_name(&input.name) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(settings) = state.instance_settings else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let saved_name = name.clone();
    match tokio::task::spawn_blocking(move || settings.save(&saved_name)).await {
        Ok(Ok(())) => name_response(name),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
