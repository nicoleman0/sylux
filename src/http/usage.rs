//! Storage use for the signed-in account and, for administrators, every account.

use super::{
    AuthState,
    accounts::{authorize, credentials},
};
use crate::{
    domain::usage::{self, AccountUsage},
    persistence::payload::VolumeSpace,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Serialize)]
struct AllUsage {
    /// `None` when the filesystem could not be queried.
    volume: Option<VolumeSpace>,
    accounts: Vec<AccountUsage>,
}

fn no_store(body: impl Serialize) -> Response {
    let mut response = Json(body).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub async fn mine(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let (cookie, _) = match credentials(&headers, &state, false) {
        Ok(value) => value,
        Err(status) => return status.into_response(),
    };
    let result = state
        .workers
        .read(move |db| match authorize(db, &cookie, None, false) {
            Ok(account) => usage::account(db, &account).map(Ok),
            Err(status) => Ok(Err(status)),
        })
        .await;
    match result {
        Ok(Ok(usage)) => no_store(usage),
        Ok(Err(status)) => status.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn all(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let (cookie, _) = match credentials(&headers, &state, false) {
        Ok(value) => value,
        Err(status) => return status.into_response(),
    };
    let result = state
        .workers
        .read(move |db| match authorize(db, &cookie, None, true) {
            Ok(_) => usage::all(db).map(Ok),
            Err(status) => Ok(Err(status)),
        })
        .await;
    let accounts = match result {
        Ok(Ok(accounts)) => accounts,
        Ok(Err(status)) => return status.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    // Queried only after administrator authorization, outside the SQLite worker.
    let volume = match state
        .storage
        .as_ref()
        .map(|storage| storage.payloads.clone())
    {
        Some(payloads) => tokio::task::spawn_blocking(move || payloads.volume_space().ok())
            .await
            .ok()
            .flatten(),
        None => None,
    };
    no_store(AllUsage { volume, accounts })
}
