use super::{
    AnonymousSession, AuthState,
    middleware::{
        anonymous_cookie, decode_secret, origin_absent_or_matches, origin_matches, secure_cookie,
        session_cookie,
    },
};
use axum::{
    Json,
    extract::State,
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, SET_COOKIE},
    },
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const ANONYMOUS_SECONDS: i64 = 10 * 60;
const MAX_ANONYMOUS_SESSIONS: usize = 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Login {
    handle: String,
    password: String,
}
#[derive(Serialize)]
pub struct Csrf {
    csrf_token: String,
}
#[derive(Serialize)]
pub struct User {
    account_id: String,
    handle: String,
    role: String,
}

struct AccountRow {
    account_id: String,
    handle: String,
    role: String,
    password_verifier: Option<String>,
    enabled: i64,
    credential_epoch: i64,
}

impl AccountRow {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            account_id: row.get(0)?,
            handle: row.get(1)?,
            role: row.get(2)?,
            password_verifier: row.get(3)?,
            enabled: row.get(4)?,
            credential_epoch: row.get(5)?,
        })
    }
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn random_secret() -> Option<([u8; 32], String)> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    let encoded = URL_SAFE_NO_PAD.encode(bytes);
    Some((bytes, encoded))
}

fn random_session_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn finish(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn fail(status: StatusCode) -> Response {
    finish((status, "authentication_failed").into_response())
}

fn with_cookie(response: Response, value: &str, max_age: i64) -> Response {
    let mut response = finish(response);
    let Ok(value) = HeaderValue::from_str(&secure_cookie(value, max_age)) else {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    };
    response.headers_mut().insert(SET_COOKIE, value);
    response
}

fn account_by_handle(
    connection: &Connection,
    handle: &str,
) -> rusqlite::Result<Option<AccountRow>> {
    connection
        .query_row(
            "SELECT account_id,handle,role,password_verifier,enabled,credential_epoch FROM account WHERE handle=?1",
            [handle],
            AccountRow::read,
        )
        .optional()
}

fn account_by_id(
    connection: &Connection,
    account_id: &str,
) -> rusqlite::Result<Option<AccountRow>> {
    connection
        .query_row(
            "SELECT account_id,handle,role,password_verifier,enabled,credential_epoch FROM account WHERE account_id=?1",
            [account_id],
            AccountRow::read,
        )
        .optional()
}

pub async fn csrf(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !origin_absent_or_matches(&headers, &state.origin) {
        return fail(StatusCode::FORBIDDEN);
    }
    let (Some((session_raw, session)), Some((csrf_raw, csrf_token))) =
        (random_secret(), random_secret())
    else {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let now = now();
    let Ok(mut anonymous) = state.anonymous.lock() else {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    };
    anonymous.retain(|_, session| session.expires_at > now);
    if anonymous.len() >= MAX_ANONYMOUS_SESSIONS {
        return fail(StatusCode::SERVICE_UNAVAILABLE);
    }
    anonymous.insert(
        crate::domain::authz::verifier(&session_raw),
        AnonymousSession {
            csrf_verifier: crate::domain::authz::verifier(&csrf_raw),
            expires_at: now + ANONYMOUS_SECONDS,
        },
    );
    drop(anonymous);
    with_cookie(
        Json(Csrf { csrf_token }).into_response(),
        &format!("anon.{session}"),
        ANONYMOUS_SECONDS,
    )
}

pub async fn login(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Response {
    if !origin_matches(&headers, &state.origin) {
        return fail(StatusCode::FORBIDDEN);
    }
    if input.handle.is_empty() || input.handle.len() > 64 {
        return fail(StatusCode::UNAUTHORIZED);
    }
    let Some(anonymous_raw) = anonymous_cookie(&headers) else {
        return fail(StatusCode::UNAUTHORIZED);
    };
    let Some(csrf) = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .and_then(decode_secret)
    else {
        return fail(StatusCode::FORBIDDEN);
    };

    // Consume the one-time pre-session before password work, preventing two
    // concurrent requests from promoting the same anonymous credential.
    let expected = {
        let Ok(mut anonymous) = state.anonymous.lock() else {
            return fail(StatusCode::INTERNAL_SERVER_ERROR);
        };
        anonymous.remove(&crate::domain::authz::verifier(&anonymous_raw))
    };
    let Some(expected) = expected else {
        return fail(StatusCode::UNAUTHORIZED);
    };

    if expected.expires_at <= now() {
        return fail(StatusCode::UNAUTHORIZED);
    }
    if !crate::domain::authz::verifier_matches(
        &expected.csrf_verifier,
        &crate::domain::authz::verifier(&csrf),
    ) {
        return fail(StatusCode::FORBIDDEN);
    }

    let handle = input.handle.clone();
    let account = match state
        .workers
        .read(move |db| account_by_handle(db, &handle))
        .await
    {
        Ok(account) => account,
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let encoded = account
        .as_ref()
        .and_then(|account| account.password_verifier.clone());
    let password_ok = state
        .limiter
        .verify_login_async(input.password.into_bytes(), encoded)
        .await;
    let Some(account) = account.filter(|account| password_ok && account.enabled == 1) else {
        return fail(StatusCode::UNAUTHORIZED);
    };

    let (Some(session_id), Some((secret_raw, secret)), Some((csrf_raw, csrf_token))) =
        (random_session_id(), random_secret(), random_secret())
    else {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let now = now();
    let (Some(idle_expires), Some(absolute_expires)) = (
        now.checked_add(crate::domain::authz::IDLE_SECONDS),
        now.checked_add(crate::domain::authz::ABSOLUTE_SECONDS),
    ) else {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let session_verifier = crate::domain::authz::verifier(&secret_raw);
    let csrf_verifier = crate::domain::authz::verifier(&csrf_raw);

    // Recheck and insert under one database lock: a disable/reset during or
    // after Argon2 cannot issue a session from the stale credential snapshot.
    let account_id = account.account_id.clone();
    let expected_epoch = account.credential_epoch;
    let expected_verifier = account.password_verifier.clone();
    let session_id_for_insert = session_id.clone();
    let insert_result = state.workers.transact(move |tx| {
        let current = account_by_id(tx, &account_id)?;
        let Some(current) = current.filter(|current| {
            current.enabled == 1
                && current.credential_epoch == expected_epoch
                && current.password_verifier == expected_verifier
        }) else {
            return Ok(None);
        };
        tx.execute(
            "INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,idle_expires_at,absolute_expires_at,revoked) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0)",
            params![session_id_for_insert, current.account_id, current.credential_epoch,
                session_verifier.as_slice(), csrf_verifier.as_slice(), now,
                idle_expires, absolute_expires],
        )?;
        Ok(Some(current))
    }).await;
    let account = match insert_result {
        Ok(Some(account)) => account,
        Ok(None) => return fail(StatusCode::UNAUTHORIZED),
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    };
    with_cookie(
        Json(serde_json::json!({
            "account_id": account.account_id,
            "handle": account.handle,
            "role": account.role,
            "csrf_token": csrf_token
        }))
        .into_response(),
        &format!("{session_id}.{secret}"),
        crate::domain::authz::ABSOLUTE_SECONDS,
    )
}

pub async fn me(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let Some(cookie) = session_cookie(&headers) else {
        return fail(StatusCode::UNAUTHORIZED);
    };
    let session_id = cookie.session_id.clone();
    let secret = cookie.secret;
    let principal = state
        .workers
        .read(move |db| {
            Ok(crate::domain::authz::validate_session(
                db,
                &session_id,
                &secret,
                now(),
            ))
        })
        .await;
    match principal {
        Ok(Some((account_id, handle, role))) => finish(
            Json(User {
                account_id,
                handle,
                role,
            })
            .into_response(),
        ),
        Ok(None) => fail(StatusCode::UNAUTHORIZED),
        Err(_) => fail(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn logout(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !origin_matches(&headers, &state.origin) {
        return fail(StatusCode::FORBIDDEN);
    }
    let Some(cookie) = session_cookie(&headers) else {
        return fail(StatusCode::UNAUTHORIZED);
    };
    let Some(csrf) = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .and_then(decode_secret)
    else {
        return fail(StatusCode::FORBIDDEN);
    };
    let Ok(_guard) = state.coordinator.serialize() else {
        return fail(StatusCode::SERVICE_UNAVAILABLE);
    };
    drop(_guard);
    let session_id = cookie.session_id.clone();
    let secret = cookie.secret;
    let revoked = state
        .workers
        .transact_result(move |tx| -> Result<(), StatusCode> {
            if crate::domain::authz::validate_session(tx, &session_id, &secret, now()).is_none() {
                return Err(StatusCode::UNAUTHORIZED);
            }
            if !crate::domain::authz::csrf_matches(tx, &session_id, &csrf) {
                return Err(StatusCode::FORBIDDEN);
            }
            let changed = tx.execute(
            "UPDATE session SET revoked=1 WHERE session_id=?1 AND verifier=?2 AND revoked=0",
            params![session_id, crate::domain::authz::verifier(&secret).as_slice()],
        ).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            if changed != 1 {
                return Err(StatusCode::UNAUTHORIZED);
            }
            Ok(())
        })
        .await;
    match revoked {
        Ok(Ok(())) => {}
        Ok(Err(status)) => return fail(status),
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    }
    with_cookie(StatusCode::NO_CONTENT.into_response(), "", 0)
}
