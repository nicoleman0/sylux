use super::{
    AuthState,
    auth::now,
    middleware::{
        SessionCookie, decode_secret, origin_absent_or_matches, origin_matches, session_cookie,
    },
};
use crate::domain::{authz, names::NAME_KEY_VERSION};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    handle: String,
    password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reset {
    password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    current_password: String,
    new_password: String,
}

fn valid_handle(handle: &str) -> bool {
    !handle.is_empty()
        && handle.len() <= 64
        && handle.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || (i > 0 && matches!(b, b'.' | b'_' | b'-'))
        })
}
fn credentials(
    headers: &HeaderMap,
    state: &AuthState,
    mutation: bool,
) -> Result<(SessionCookie, Option<[u8; 32]>), StatusCode> {
    if if mutation {
        !origin_matches(headers, &state.origin)
    } else {
        !origin_absent_or_matches(headers, &state.origin)
    } {
        return Err(StatusCode::FORBIDDEN);
    }
    let cookie = session_cookie(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let csrf = if mutation {
        Some(
            headers
                .get("x-csrf-token")
                .and_then(|v| v.to_str().ok())
                .and_then(decode_secret)
                .ok_or(StatusCode::FORBIDDEN)?,
        )
    } else {
        None
    };
    Ok((cookie, csrf))
}
fn authorize(
    db: &Connection,
    cookie: &SessionCookie,
    csrf: Option<&[u8; 32]>,
    admin: bool,
) -> Result<String, StatusCode> {
    let (id, _, role) = authz::validate_session(db, &cookie.session_id, &cookie.secret, now())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if csrf.is_some_and(|token| !authz::csrf_matches(db, &cookie.session_id, token))
        || (admin && role != "administrator")
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(id)
}
fn response(
    result: Result<Result<(), StatusCode>, crate::persistence::sqlite::WorkerError>,
) -> Response {
    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT,
        Ok(Err(status)) => status,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
    .into_response()
}
async fn preflight(
    state: &AuthState,
    cookie: &SessionCookie,
    csrf: Option<&[u8; 32]>,
    admin: bool,
) -> Result<(), StatusCode> {
    let cookie = SessionCookie {
        session_id: cookie.session_id.clone(),
        secret: cookie.secret,
    };
    let csrf = csrf.copied();
    state
        .workers
        .read(move |db| Ok(authorize(db, &cookie, csrf.as_ref(), admin)))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    Ok(())
}
fn random_id() -> Result<String, StatusCode> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub async fn list(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let (cookie, _) = match credentials(&headers, &state, false) {
        Ok(v) => v,
        Err(s) => return s.into_response(),
    };
    let result = state.workers.read(move |db| {
        if let Err(status) = authorize(db, &cookie, None, true) { return Ok(Err(status)); }
        let mut statement = db.prepare("SELECT account_id,handle,role,enabled FROM account ORDER BY handle")?;
        let rows = statement.query_map([], |r| Ok(serde_json::json!({"account_id":r.get::<_,String>(0)?,"handle":r.get::<_,String>(1)?,"role":r.get::<_,String>(2)?,"enabled":r.get::<_,i64>(3)? == 1})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Ok(rows))
    }).await;
    match result {
        Ok(Ok(rows)) => {
            let mut response = Json(rows).into_response();
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
            response
        }
        Ok(Err(s)) => s.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
pub async fn create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<Create>,
) -> Response {
    let (cookie, csrf) = match credentials(&headers, &state, true) {
        Ok(v) => v,
        Err(s) => return s.into_response(),
    };
    if let Err(s) = preflight(&state, &cookie, csrf.as_ref(), true).await {
        return s.into_response();
    }
    if !valid_handle(&input.handle) || !authz::validate_password(&input.password) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(verifier) = state
        .limiter
        .hash_password(input.password.into_bytes())
        .await
    else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let (Ok(account), Ok(root)) = (random_id(), random_id()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    response(state.workers.transact_result(move |tx| {
        authorize(tx, &cookie, csrf.as_ref(), true)?;
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM account WHERE handle=?1)", [&input.handle], |r| r.get(0)).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if exists { return Err(StatusCode::CONFLICT); }
        tx.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch,password_verifier) VALUES(?1,?2,'user',1,0,?3)", params![account,input.handle,verifier]).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,NULL,'folder','','',?3)", params![root,account,NAME_KEY_VERSION]).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(())
    }).await)
}
pub async fn disable(
    State(state): State<AuthState>,
    Path(target): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (cookie, csrf) = match credentials(&headers, &state, true) {
        Ok(v) => v,
        Err(s) => return s.into_response(),
    };
    response(state.workers.transact_result(move |tx| {
        let actor = authorize(tx, &cookie, csrf.as_ref(), true)?;
        if actor == target { return Err(StatusCode::CONFLICT); }
        let changed = tx.execute("UPDATE account SET enabled=0,credential_epoch=credential_epoch+1 WHERE account_id=?1 AND enabled=1 AND role='user'", [&target]).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if changed != 1 { return Err(StatusCode::NOT_FOUND); }
        tx.execute("UPDATE session SET revoked=1 WHERE account_id=?1", [&target]).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(())
    }).await)
}
pub async fn reset(
    State(state): State<AuthState>,
    Path(target): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Reset>,
) -> Response {
    let (cookie, csrf) = match credentials(&headers, &state, true) {
        Ok(v) => v,
        Err(s) => return s.into_response(),
    };
    if let Err(s) = preflight(&state, &cookie, csrf.as_ref(), true).await {
        return s.into_response();
    }
    if !authz::validate_password(&input.password) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(verifier) = state
        .limiter
        .hash_password(input.password.into_bytes())
        .await
    else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    response(state.workers.transact_result(move |tx| {
        let actor = authorize(tx, &cookie, csrf.as_ref(), true)?;
        if actor == target { return Err(StatusCode::CONFLICT); }
        let changed = tx.execute("UPDATE account SET password_verifier=?1,credential_epoch=credential_epoch+1 WHERE account_id=?2 AND role='user' AND enabled=1", params![verifier,target]).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if changed != 1 { return Err(StatusCode::NOT_FOUND); }
        tx.execute("UPDATE session SET revoked=1 WHERE account_id=?1", [&target]).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(())
    }).await)
}
pub async fn change_password(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<Change>,
) -> Response {
    let (cookie, csrf) = match credentials(&headers, &state, true) {
        Ok(v) => v,
        Err(s) => return s.into_response(),
    };
    if !authz::validate_password(&input.new_password) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let copy = SessionCookie {
        session_id: cookie.session_id.clone(),
        secret: cookie.secret,
    };
    let snapshot = state
        .workers
        .read(move |db| {
            let id = match authorize(db, &copy, csrf.as_ref(), false) {
                Ok(id) => id,
                Err(s) => return Ok(Err(s)),
            };
            let verifier: Option<String> = db
                .query_row(
                    "SELECT password_verifier FROM account WHERE account_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            Ok(Ok((id, verifier)))
        })
        .await;
    let (id, old) = match snapshot {
        Ok(Ok(v)) => v,
        Ok(Err(s)) => return s.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if !state
        .limiter
        .verify_login_async(input.current_password.into_bytes(), old.clone())
        .await
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(verifier) = state
        .limiter
        .hash_password(input.new_password.into_bytes())
        .await
    else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    response(state.workers.transact_result(move |tx| {
        let actor = authorize(tx, &cookie, csrf.as_ref(), false)?;
        if actor != id { return Err(StatusCode::UNAUTHORIZED); }
        let changed = tx.execute("UPDATE account SET password_verifier=?1,credential_epoch=credential_epoch+1 WHERE account_id=?2 AND password_verifier=?3", params![verifier,id,old]).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if changed != 1 { return Err(StatusCode::CONFLICT); }
        tx.execute("UPDATE session SET revoked=1 WHERE account_id=?1", [&id]).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(())
    }).await)
}
