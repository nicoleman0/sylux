use super::{AuthState, accounts};
use crate::domain::{dav_credentials, upload::validate_id};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct Credential {
    id: String,
    name: String,
    read_only: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NewCredential {
    name: String,
    read_only: bool,
}

fn response(result: impl IntoResponse) -> Response {
    let mut response = result.into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) async fn list(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let (cookie, _) = match accounts::credentials(&headers, &state, false) {
        Ok(credentials) => credentials,
        Err(status) => return response(status),
    };
    match state.workers.read(move |db| {
        let owner = match accounts::authorize(db, &cookie, None, false) {
            Ok(owner) => owner,
            Err(status) => return Ok(Err(status)),
        };
        let mut query = db.prepare(
            "SELECT credential_id, name, read_only FROM dav_credential JOIN account USING (account_id)
             WHERE account_id=?1 AND dav_credential.credential_epoch=account.credential_epoch
             ORDER BY name",
        )?;
        let credentials = query.query_map([owner], |row| {
            Ok(Credential { id: row.get(0)?, name: row.get(1)?, read_only: row.get(2)? })
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Ok(credentials))
    }).await {
        Ok(Ok(credentials)) => response(Json(credentials)),
        Ok(Err(status)) => response(status),
        Err(_) => response(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(super) async fn create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<NewCredential>,
) -> Response {
    let (cookie, csrf) = match accounts::credentials(&headers, &state, true) {
        Ok(credentials) => credentials,
        Err(status) => return response(status),
    };
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return response(StatusCode::BAD_REQUEST);
    }
    let Ok((id, verifier, password)) = dav_credentials::generate() else {
        return response(StatusCode::SERVICE_UNAVAILABLE);
    };
    let read_only = input.read_only;
    match state.workers.transact_result(move |tx| {
        let owner = accounts::authorize(tx, &cookie, csrf.as_ref(), false)?;
        let db_error = |_| StatusCode::SERVICE_UNAVAILABLE;
        tx.execute(
            "DELETE FROM dav_credential WHERE account_id=?1 AND credential_epoch <>
             (SELECT credential_epoch FROM account WHERE account_id=?1)",
            [&owner],
        ).map_err(db_error)?;
        let inserted = tx.execute(
            "INSERT INTO dav_credential(credential_id, account_id, name, verifier, credential_epoch, read_only)
             SELECT ?1, account_id, ?2, ?3, credential_epoch, ?4 FROM account
             WHERE account_id=?5 AND (SELECT count(*) FROM dav_credential WHERE account_id=?5)<32
             ON CONFLICT(account_id, name) DO NOTHING",
            params![id, name, verifier.as_slice(), read_only, owner],
        ).map_err(db_error)?;
        if inserted != 1 {
            return Err(StatusCode::CONFLICT);
        }
        Ok(Credential { id, name, read_only })
    }).await {
        Ok(Ok(credential)) => response((StatusCode::CREATED, Json(serde_json::json!({
            "credential": credential,
            "password": password,
        })))),
        Ok(Err(status)) => response(status),
        Err(_) => response(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(super) async fn revoke(
    State(state): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (cookie, csrf) = match accounts::credentials(&headers, &state, true) {
        Ok(credentials) => credentials,
        Err(status) => return response(status),
    };
    if validate_id(&id).is_err() {
        return response(StatusCode::NOT_FOUND);
    }
    match state
        .workers
        .transact_result(move |tx| {
            let owner = accounts::authorize(tx, &cookie, csrf.as_ref(), false)?;
            let removed = tx
                .execute(
                    "DELETE FROM dav_credential WHERE credential_id=?1 AND account_id=?2",
                    params![id, owner],
                )
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            if removed != 1 {
                return Err(StatusCode::NOT_FOUND);
            }
            Ok(())
        })
        .await
    {
        Ok(Ok(())) => response(StatusCode::NO_CONTENT),
        Ok(Err(status)) => response(status),
        Err(_) => response(StatusCode::SERVICE_UNAVAILABLE),
    }
}
