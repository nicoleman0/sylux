//! Authenticated owner namespace operations.
#![allow(clippy::result_large_err)]
// Axum responses are the request-local control-flow error channel here.
use crate::{
    domain::{
        authz,
        namespace::{self, NamespaceError},
        search as domain_search,
    },
    http::{
        AuthState,
        middleware::{decode_secret, origin_matches, session_cookie},
    },
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
type HmacSha256 = Hmac<Sha256>;
enum WriteFailure {
    Unauthorized,
    Forbidden,
    Namespace(NamespaceError),
}
fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(serde_json::json!({"code":code}))).into_response()
}
fn entry_response(status: StatusCode, entry: namespace::Entry) -> Response {
    let mut response = (status, Json(&entry)).into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(&entry.etag) {
        response
            .headers_mut()
            .insert(axum::http::header::ETAG, value);
    }
    response
}
async fn principal(
    state: &AuthState,
    headers: &HeaderMap,
) -> Result<(String, String), (StatusCode, &'static str)> {
    let Some(c) = session_cookie(headers) else {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized"));
    };
    let session_id = c.session_id.clone();
    let secret = c.secret;
    let principal = state
        .workers
        .read(move |db| {
            Ok(authz::validate_session(
                db,
                &session_id,
                &secret,
                crate::http::auth::now(),
            ))
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"))?;
    principal
        .map(|(id, _, _)| (id, c.session_id))
        .ok_or((StatusCode::UNAUTHORIZED, "unauthorized"))
}
async fn csrf(
    state: &AuthState,
    headers: &HeaderMap,
    account: &str,
    sid: &str,
) -> Result<(), (StatusCode, &'static str)> {
    if !origin_matches(headers, &state.origin) {
        return Err((StatusCode::FORBIDDEN, "forbidden"));
    }
    let Some(token) = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .and_then(decode_secret)
    else {
        return Err((StatusCode::FORBIDDEN, "forbidden"));
    };
    let sid = sid.to_owned();
    let matched = state
        .workers
        .read(move |db| Ok(authz::csrf_matches(db, &sid, &token)))
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"))?;
    if matched {
        return Ok(());
    }
    let _ = account;
    Err((StatusCode::FORBIDDEN, "forbidden"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateFolder {
    parent_id: String,
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rename {
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    folder_id: Option<String>,
    sort: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}
#[derive(Serialize)]
struct ListingItem {
    #[serde(flatten)]
    entry: namespace::Entry,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<i64>,
}

#[derive(Serialize)]
struct ListingReply {
    items: Vec<ListingItem>,
    root: Option<namespace::Entry>,
    next_cursor: Option<String>,
    has_more: bool,
}
fn cursor_key(state: &AuthState) -> [u8; 32] {
    use sha2::Digest;
    Sha256::digest(state.setup_authorization.as_bytes()).into()
}
const MAX_CURSOR_BYTES: usize = 4096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchQuery {
    q: String,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SearchCursor {
    version: u8,
    actor: String,
    query: String,
    fingerprint: String,
    offset: usize,
}

fn search_cursor_encode(key: &[u8; 32], cursor: &SearchCursor) -> Option<String> {
    let body = serde_json::to_vec(cursor).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(b"sylux:account-search-cursor:v1\0");
    mac.update(&body);
    let token = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(body),
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    );
    (token.len() <= MAX_CURSOR_BYTES).then_some(token)
}

fn search_cursor_decode(key: &[u8; 32], value: &str) -> Option<SearchCursor> {
    if value.len() > MAX_CURSOR_BYTES {
        return None;
    }
    let (body, signature) = value.split_once('.')?;
    let body = URL_SAFE_NO_PAD.decode(body).ok()?;
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(b"sylux:account-search-cursor:v1\0");
    mac.update(&body);
    mac.verify_slice(&signature).ok()?;
    serde_json::from_slice(&body).ok()
}

pub async fn search(
    State(s): State<AuthState>,
    headers: HeaderMap,
    Query(q): Query<SearchQuery>,
) -> Response {
    if q.q.is_empty() || q.q.len() > 255 || q.q.chars().any(char::is_control) {
        return error(StatusCode::BAD_REQUEST, "invalid_query");
    }
    let limit = q.limit.unwrap_or(100);
    if limit == 0 || limit > 100 {
        return error(StatusCode::BAD_REQUEST, "invalid_limit");
    }
    let (account, _) = match principal(&s, &headers).await {
        Ok(value) => value,
        Err(value) => return error(value.0, value.1),
    };
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let key = cursor_key(&s);
    let result = s
        .workers
        .read(move |db| Ok(build_search(db, cookie, account, q, limit, key)))
        .await;
    match result {
        Ok(Ok(reply)) => (StatusCode::OK, Json(reply)).into_response(),
        Ok(Err(response)) => response,
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

fn build_search(
    db: &rusqlite::Connection,
    cookie: crate::http::middleware::SessionCookie,
    account: String,
    q: SearchQuery,
    limit: usize,
    key: [u8; 32],
) -> Result<serde_json::Value, Response> {
    let Some((current, _, _)) = authz::validate_session(
        db,
        &cookie.session_id,
        &cookie.secret,
        crate::http::auth::now(),
    ) else {
        return Err(error(StatusCode::UNAUTHORIZED, "unauthorized"));
    };
    if current != account {
        return Err(error(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let found = domain_search::find(db, &account, &q.q)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"))?;
    let offset = if let Some(token) = q.cursor {
        let Some(cursor) = search_cursor_decode(&key, &token) else {
            return Err(error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        };
        if cursor.version != 1
            || cursor.actor != account
            || cursor.query != q.q
            || cursor.offset == 0
        {
            return Err(error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        }
        if cursor.fingerprint != found.fingerprint {
            return Err(error(StatusCode::CONFLICT, "listing_changed"));
        }
        if cursor.offset >= found.hits.len() {
            return Err(error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        }
        cursor.offset
    } else {
        0
    };
    let end = offset.saturating_add(limit).min(found.hits.len());
    let next_cursor = if end < found.hits.len() {
        search_cursor_encode(
            &key,
            &SearchCursor {
                version: 1,
                actor: account,
                query: q.q,
                fingerprint: found.fingerprint,
                offset: end,
            },
        )
        .ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"))?
        .into()
    } else {
        None
    };
    Ok(
        serde_json::json!({ "items": &found.hits[offset..end], "next_cursor": next_cursor, "has_more": end < found.hits.len() }),
    )
}

#[derive(Debug, Serialize, Deserialize)]
struct Cursor {
    version: u8,
    actor: String,
    root: String,
    folder: String,
    generation: i64,
    filter: String,
    sort: namespace::ListingSort,
    key_version: i64,
    last_kind_rank: u8,
    last_size: i64,
    last_key: String,
    last_id: String,
}
fn encode_cursor(key: &[u8; 32], cursor: &Cursor) -> Option<String> {
    let body = serde_json::to_vec(cursor).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(b"sylux:entry-list-cursor:v2\0");
    mac.update(&body);
    let token = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(body),
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    );
    (token.len() <= MAX_CURSOR_BYTES).then_some(token)
}
fn decode_cursor(key: &[u8; 32], value: &str) -> Option<Cursor> {
    if value.len() > MAX_CURSOR_BYTES {
        return None;
    }
    let (a, b) = value.split_once('.')?;
    let body = URL_SAFE_NO_PAD.decode(a).ok()?;
    let sig = URL_SAFE_NO_PAD.decode(b).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(b"sylux:entry-list-cursor:v2\0");
    mac.update(&body);
    mac.verify_slice(&sig).ok()?;
    serde_json::from_slice(&body).ok()
}
pub async fn create_in_folder(
    State(state): State<AuthState>,
    Path(parent): Path<String>,
    headers: HeaderMap,
    Json(input): Json<NewFolder>,
) -> Response {
    create(
        State(state),
        headers,
        Json(CreateFolder {
            parent_id: parent,
            name: input.name,
        }),
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewFolder {
    name: String,
}
pub async fn listing_folder(
    State(state): State<AuthState>,
    Path(folder_id): Path<String>,
    headers: HeaderMap,
    Query(mut query): Query<Listing>,
) -> Response {
    query.folder_id = Some(folder_id);
    listing(State(state), headers, Query(query)).await
}
pub async fn create(
    State(s): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<CreateFolder>,
) -> Response {
    let Ok(_guard) = s.coordinator.serialize() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "mutation_unavailable");
    };
    drop(_guard);
    let (account, sid) = match principal(&s, &headers).await {
        Ok(x) => x,
        Err(r) => return error(r.0, r.1),
    };
    if let Err(r) = csrf(&s, &headers, &account, &sid).await {
        return error(r.0, r.1);
    }
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let Some(csrf_token) = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .and_then(decode_secret)
    else {
        return error(StatusCode::FORBIDDEN, "forbidden");
    };
    let sid_for_write = cookie.session_id;
    let secret = cookie.secret;
    let account_for_write = account.clone();
    let parent_id = input.parent_id;
    let name = input.name;
    let result = s
        .workers
        .transact(move |tx| {
            let Some((current, _, _)) =
                authz::validate_session(tx, &sid_for_write, &secret, crate::http::auth::now())
            else {
                return Ok(Err(WriteFailure::Unauthorized));
            };
            if current != account_for_write {
                return Ok(Err(WriteFailure::Unauthorized));
            }
            if !authz::csrf_matches(tx, &sid_for_write, &csrf_token) {
                return Ok(Err(WriteFailure::Forbidden));
            }
            match namespace::create_folder(tx, &account_for_write, &parent_id, &name) {
                Ok(entry) => Ok(Ok(entry)),
                Err(NamespaceError::Sql) => Err(rusqlite::Error::InvalidQuery),
                Err(error) => Ok(Err(WriteFailure::Namespace(error))),
            }
        })
        .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    };
    let result = match result {
        Ok(entry) => Ok(entry),
        Err(WriteFailure::Unauthorized) => return error(StatusCode::UNAUTHORIZED, "unauthorized"),
        Err(WriteFailure::Forbidden) => return error(StatusCode::FORBIDDEN, "forbidden"),
        Err(WriteFailure::Namespace(error)) => Err(error),
    };
    match result {
        Ok(entry) => entry_response(StatusCode::CREATED, entry),
        Err(NamespaceError::InvalidName) => error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_name"),
        Err(NamespaceError::Conflict) => error(StatusCode::CONFLICT, "name_conflict"),
        Err(NamespaceError::NotFound) => error(StatusCode::NOT_FOUND, "not_found"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}
pub async fn listing(
    State(s): State<AuthState>,
    headers: HeaderMap,
    Query(q): Query<Listing>,
) -> Response {
    let (account, _) = match principal(&s, &headers).await {
        Ok(x) => x,
        Err(r) => return error(r.0, r.1),
    };
    let limit = q.limit.unwrap_or(100);
    if limit == 0 || limit > 500 {
        return error(StatusCode::BAD_REQUEST, "invalid_limit");
    }
    let sort = match q.sort.as_deref().unwrap_or("name_asc") {
        "name_asc" => namespace::ListingSort::NameAsc,
        "name_desc" => namespace::ListingSort::NameDesc,
        "size_asc" => namespace::ListingSort::SizeAsc,
        "size_desc" => namespace::ListingSort::SizeDesc,
        _ => return error(StatusCode::BAD_REQUEST, "invalid_sort"),
    };
    let key = cursor_key(&s);
    let result = s
        .workers
        .read(move |db| {
            Ok(build_listing(
                db,
                &account,
                q.folder_id,
                q.cursor,
                sort,
                limit,
                key,
            ))
        })
        .await;
    match result {
        Ok(Ok(listing)) => (StatusCode::OK, Json(listing)).into_response(),
        Ok(Err(response)) => response,
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

fn build_listing(
    db: &rusqlite::Connection,
    account: &str,
    folder_id: Option<String>,
    cursor: Option<String>,
    sort: namespace::ListingSort,
    limit: usize,
    key: [u8; 32],
) -> Result<ListingReply, Response> {
    let root = namespace::root_entry(db, account)
        .map_err(|_| error(StatusCode::NOT_FOUND, "not_found"))?;
    let root_id = root.id.clone();
    let is_root = folder_id.is_none();
    let folder = folder_id.unwrap_or_else(|| root_id.clone());
    let generation: Option<i64> = db
        .query_row(
            "SELECT generation FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND kind='folder' AND state='active'",
            params![folder, account],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let Some(generation) = generation else {
        return Err(error(StatusCode::NOT_FOUND, "not_found"));
    };
    let after = if let Some(value) = cursor {
        let Some(decoded) = decode_cursor(&key, &value) else {
            return Err(error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        };
        if decoded.version != 2
            || decoded.actor != account
            || decoded.root != root_id
            || decoded.folder != folder
            || decoded.filter != "all"
            || decoded.sort != sort
            || decoded.key_version != crate::domain::names::NAME_KEY_VERSION
        {
            return Err(error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        }
        if decoded.generation != generation {
            return Err(error(StatusCode::CONFLICT, "listing_changed"));
        }
        let Some(last_key) = URL_SAFE_NO_PAD.decode(decoded.last_key).ok() else {
            return Err(error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        };
        if decoded.last_kind_rank > 1
            || decoded.last_size < 0
            || (decoded.last_kind_rank == 0 && decoded.last_size != 0)
            || last_key.is_empty()
            || last_key.len() > crate::domain::names::MAX_NAME_BYTES
            || decoded.last_id.len() != 32
            || !decoded
                .last_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        }
        Some(namespace::ListingPosition {
            kind_rank: decoded.last_kind_rank,
            name_key: last_key,
            size: decoded.last_size,
            id: decoded.last_id,
        })
    } else {
        None
    };
    let mut rows = namespace::list_sorted(db, account, &folder, sort, after.as_ref(), limit)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"))?;
    let more = rows.len() > limit;
    if more {
        rows.truncate(limit);
    }
    let next_cursor = if more {
        let last = rows.last().unwrap();
        Some(
            encode_cursor(
                &key,
                &Cursor {
                    version: 2,
                    actor: account.to_owned(),
                    root: root_id,
                    folder: folder.clone(),
                    generation,
                    filter: "all".to_owned(),
                    sort,
                    key_version: crate::domain::names::NAME_KEY_VERSION,
                    last_kind_rank: last.position.kind_rank,
                    last_size: last.position.size,
                    last_key: URL_SAFE_NO_PAD.encode(&last.position.name_key),
                    last_id: last.position.id.clone(),
                },
            )
            .ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"))?,
        )
    } else {
        None
    };
    Ok(ListingReply {
        items: rows
            .into_iter()
            .map(|row| ListingItem {
                size: (row.entry.kind == "file").then_some(row.position.size),
                entry: row.entry,
            })
            .collect(),
        root: is_root.then_some(root),
        next_cursor,
        has_more: more,
    })
}
pub async fn rename(
    State(s): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Rename>,
) -> Response {
    let Ok(_guard) = s.coordinator.serialize() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "mutation_unavailable");
    };
    drop(_guard);
    let (account, sid) = match principal(&s, &headers).await {
        Ok(x) => x,
        Err(r) => return error(r.0, r.1),
    };
    if let Err(r) = csrf(&s, &headers, &account, &sid).await {
        return error(r.0, r.1);
    }
    let Some(tag) = headers.get("if-match").and_then(|v| v.to_str().ok()) else {
        return error(StatusCode::PRECONDITION_REQUIRED, "if_match_required");
    };
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let Some(csrf_token) = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .and_then(decode_secret)
    else {
        return error(StatusCode::FORBIDDEN, "forbidden");
    };
    let sid_for_write = cookie.session_id;
    let secret = cookie.secret;
    let account_for_write = account.clone();
    let name = input.name;
    let tag = tag.to_owned();
    let result = s
        .workers
        .transact(move |tx| {
            let Some((current, _, _)) =
                authz::validate_session(tx, &sid_for_write, &secret, crate::http::auth::now())
            else {
                return Ok(Err(WriteFailure::Unauthorized));
            };
            if current != account_for_write {
                return Ok(Err(WriteFailure::Unauthorized));
            }
            if !authz::csrf_matches(tx, &sid_for_write, &csrf_token) {
                return Ok(Err(WriteFailure::Forbidden));
            }
            match namespace::rename(tx, &account_for_write, &id, &name, &tag) {
                Ok(entry) => Ok(Ok(entry)),
                Err(NamespaceError::Sql) => Err(rusqlite::Error::InvalidQuery),
                Err(error) => Ok(Err(WriteFailure::Namespace(error))),
            }
        })
        .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    };
    let result = match result {
        Ok(entry) => Ok(entry),
        Err(WriteFailure::Unauthorized) => return error(StatusCode::UNAUTHORIZED, "unauthorized"),
        Err(WriteFailure::Forbidden) => return error(StatusCode::FORBIDDEN, "forbidden"),
        Err(WriteFailure::Namespace(error)) => Err(error),
    };
    match result {
        Ok(entry) => entry_response(StatusCode::OK, entry),
        Err(NamespaceError::InvalidName) => error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_name"),
        Err(NamespaceError::Conflict) => error(StatusCode::CONFLICT, "name_conflict"),
        Err(NamespaceError::Stale) => error(StatusCode::PRECONDITION_FAILED, "stale_etag"),
        Err(NamespaceError::NotFound) => error(StatusCode::NOT_FOUND, "not_found"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveEntry {
    parent_id: String,
}

pub async fn move_entry(
    State(s): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<MoveEntry>,
) -> Response {
    let Ok(guard) = s.coordinator.serialize() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "mutation_unavailable");
    };
    drop(guard);
    if !origin_matches(&headers, &s.origin) {
        return error(StatusCode::FORBIDDEN, "forbidden");
    }
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let Some(csrf_token) = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .and_then(decode_secret)
    else {
        return error(StatusCode::FORBIDDEN, "forbidden");
    };
    let Some(tag) = headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        return error(StatusCode::PRECONDITION_REQUIRED, "if_match_required");
    };
    let result = s
        .workers
        .transact(move |tx| {
            let Some((account, _, _)) = authz::validate_session(
                tx,
                &cookie.session_id,
                &cookie.secret,
                crate::http::auth::now(),
            ) else {
                return Ok(Err(WriteFailure::Unauthorized));
            };
            if !authz::csrf_matches(tx, &cookie.session_id, &csrf_token) {
                return Ok(Err(WriteFailure::Forbidden));
            }
            match namespace::move_entry(tx, &account, &id, &input.parent_id, &tag) {
                Ok(entry) => Ok(Ok(entry)),
                Err(NamespaceError::Sql) => Err(rusqlite::Error::InvalidQuery),
                Err(error) => Ok(Err(WriteFailure::Namespace(error))),
            }
        })
        .await;
    match result {
        Ok(Ok(entry)) => entry_response(StatusCode::OK, entry),
        Ok(Err(WriteFailure::Unauthorized)) => error(StatusCode::UNAUTHORIZED, "unauthorized"),
        Ok(Err(WriteFailure::Forbidden)) => error(StatusCode::FORBIDDEN, "forbidden"),
        Ok(Err(WriteFailure::Namespace(NamespaceError::NotFound))) => {
            error(StatusCode::NOT_FOUND, "not_found")
        }
        Ok(Err(WriteFailure::Namespace(NamespaceError::Stale))) => {
            error(StatusCode::PRECONDITION_FAILED, "stale_etag")
        }
        Ok(Err(WriteFailure::Namespace(NamespaceError::Conflict))) => {
            error(StatusCode::CONFLICT, "name_conflict")
        }
        Ok(Err(WriteFailure::Namespace(NamespaceError::InvalidDestination))) => {
            error(StatusCode::CONFLICT, "invalid_destination")
        }
        Ok(Err(WriteFailure::Namespace(NamespaceError::DepthLimit))) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, "depth_limit")
        }
        _ => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

pub async fn trash_listing(State(s): State<AuthState>, headers: HeaderMap) -> Response {
    let (account, _) = match principal(&s, &headers).await {
        Ok(x) => x,
        Err(r) => return error(r.0, r.1),
    };
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    match s
        .workers
        .read(move |db| {
            let Some((current, _, _)) = authz::validate_session(
                db,
                &cookie.session_id,
                &cookie.secret,
                crate::http::auth::now(),
            ) else {
                return Ok(None);
            };
            if current != account {
                return Ok(None);
            }
            namespace::trashed(db, &account)
                .map(Some)
                .map_err(|_| rusqlite::Error::InvalidQuery)
        })
        .await
    {
        Ok(Some(items)) => {
            (StatusCode::OK, Json(serde_json::json!({"items":items}))).into_response()
        }
        Ok(None) => error(StatusCode::UNAUTHORIZED, "unauthorized"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

pub async fn trash(
    State(s): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    change_trash(s, id, headers, false).await
}
pub async fn restore(
    State(s): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    change_trash(s, id, headers, true).await
}
async fn change_trash(s: AuthState, id: String, headers: HeaderMap, restore: bool) -> Response {
    let Ok(_guard) = s.coordinator.serialize() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "mutation_unavailable");
    };
    drop(_guard);
    if !origin_matches(&headers, &s.origin) {
        return error(StatusCode::FORBIDDEN, "forbidden");
    }
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let Some(csrf_token) = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .and_then(decode_secret)
    else {
        return error(StatusCode::FORBIDDEN, "forbidden");
    };
    let Some(tag) = headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        return error(StatusCode::PRECONDITION_REQUIRED, "if_match_required");
    };
    let result = s
        .workers
        .transact(move |tx| {
            let Some((account, _, _)) = authz::validate_session(
                tx,
                &cookie.session_id,
                &cookie.secret,
                crate::http::auth::now(),
            ) else {
                return Ok(Err(WriteFailure::Unauthorized));
            };
            if !authz::csrf_matches(tx, &cookie.session_id, &csrf_token) {
                return Ok(Err(WriteFailure::Forbidden));
            }
            let result = if restore {
                namespace::restore(tx, &account, &id, &tag)
            } else {
                namespace::trash(tx, &account, &id, &tag)
            };
            match result {
                Ok(()) => Ok(Ok(())),
                Err(NamespaceError::Sql) => Err(rusqlite::Error::InvalidQuery),
                Err(e) => Ok(Err(WriteFailure::Namespace(e))),
            }
        })
        .await;
    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(WriteFailure::Unauthorized)) => error(StatusCode::UNAUTHORIZED, "unauthorized"),
        Ok(Err(WriteFailure::Forbidden)) => error(StatusCode::FORBIDDEN, "forbidden"),
        Ok(Err(WriteFailure::Namespace(NamespaceError::NotFound))) => {
            error(StatusCode::NOT_FOUND, "not_found")
        }
        Ok(Err(WriteFailure::Namespace(NamespaceError::Stale))) => {
            error(StatusCode::PRECONDITION_FAILED, "stale_etag")
        }
        Ok(Err(WriteFailure::Namespace(NamespaceError::NotEmpty))) => {
            error(StatusCode::CONFLICT, "folder_not_empty")
        }
        Ok(Err(WriteFailure::Namespace(NamespaceError::ParentUnavailable))) => {
            error(StatusCode::CONFLICT, "parent_unavailable")
        }
        Ok(Err(WriteFailure::Namespace(NamespaceError::Conflict))) => {
            error(StatusCode::CONFLICT, "name_conflict")
        }
        _ => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}
