#![allow(clippy::result_large_err)]
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
    modified_at: Option<i64>,
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
    folder_id: Option<String>,
    sort: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

fn result_sort(value: Option<&str>) -> Result<namespace::ResultSort, Response> {
    use namespace::ResultSort;
    match value.unwrap_or("name_asc") {
        "name_asc" => Ok(ResultSort::NameAsc),
        "name_desc" => Ok(ResultSort::NameDesc),
        "size_asc" => Ok(ResultSort::SizeAsc),
        "size_desc" => Ok(ResultSort::SizeDesc),
        "modified_asc" => Ok(ResultSort::ModifiedAsc),
        "modified_desc" => Ok(ResultSort::ModifiedDesc),
        "location_asc" => Ok(ResultSort::LocationAsc),
        "location_desc" => Ok(ResultSort::LocationDesc),
        _ => Err(error(StatusCode::BAD_REQUEST, "invalid_sort")),
    }
}

#[derive(Serialize, Deserialize)]
struct SearchCursor {
    version: u8,
    actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    folder_id: Option<String>,
    query: String,
    sort: namespace::ResultSort,
    fingerprint: String,
    offset: usize,
}

fn search_cursor_encode(key: &[u8; 32], cursor: &SearchCursor) -> Option<String> {
    let body = serde_json::to_vec(cursor).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(b"sylux:account-search-cursor:v2\0");
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
    mac.update(b"sylux:account-search-cursor:v2\0");
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
        .read(move |db| {
            let snapshot = db.unchecked_transaction()?;
            let reply = build_search(&snapshot, cookie, account, q, limit, key);
            snapshot.commit()?;
            Ok(reply)
        })
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
    let sort = result_sort(q.sort.as_deref())?;
    let found = domain_search::find_sorted(db, &account, q.folder_id.as_deref(), &q.q, sort)
        .map_err(|failure| match failure {
            NamespaceError::NotFound => error(StatusCode::NOT_FOUND, "not_found"),
            _ => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        })?;
    // Keep existing root cursors valid and make older servers reject folder cursors.
    let version = if found.folder_id.is_some() { 3 } else { 2 };
    let offset = if let Some(token) = q.cursor {
        let Some(cursor) = search_cursor_decode(&key, &token) else {
            return Err(error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        };
        if cursor.version != version
            || cursor.sort != sort
            || cursor.actor != account
            || cursor.folder_id != found.folder_id
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
                version,
                sort,
                actor: account,
                folder_id: found.folder_id,
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
    last_modified_at: Option<i64>,
    last_key: String,
    last_id: String,
}
fn encode_cursor(key: &[u8; 32], cursor: &Cursor) -> Option<String> {
    let body = serde_json::to_vec(cursor).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(b"sylux:entry-list-cursor:v3\0");
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
    mac.update(b"sylux:entry-list-cursor:v3\0");
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
pub async fn folder_path(
    State(s): State<AuthState>,
    headers: HeaderMap,
    Path(folder): Path<String>,
) -> Response {
    let (account, _) = match principal(&s, &headers).await {
        Ok(value) => value,
        Err(value) => return error(value.0, value.1),
    };
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let result = s
        .workers
        .read(move |db| {
            let snapshot = db.unchecked_transaction()?;
            let db = &snapshot;
            let Some((current, _, _)) = authz::validate_session(
                db,
                &cookie.session_id,
                &cookie.secret,
                crate::http::auth::now(),
            ) else {
                return Ok(Err(error(StatusCode::UNAUTHORIZED, "unauthorized")));
            };
            if current != account {
                return Ok(Err(error(StatusCode::UNAUTHORIZED, "unauthorized")));
            }
            Ok(
                namespace::folder_path(db, &account, &folder).map_err(|failure| match failure {
                    NamespaceError::NotFound => error(StatusCode::NOT_FOUND, "not_found"),
                    _ => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
                }),
            )
        })
        .await;
    match result {
        Ok(Ok(items)) => Json(serde_json::json!({ "items": items })).into_response(),
        Ok(Err(response)) => response,
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
        "modified_asc" => namespace::ListingSort::ModifiedAsc,
        "modified_desc" => namespace::ListingSort::ModifiedDesc,
        _ => return error(StatusCode::BAD_REQUEST, "invalid_sort"),
    };
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let key = cursor_key(&s);
    let result = s
        .workers
        .read(move |db| {
            let snapshot = db.unchecked_transaction()?;
            let Some((current, _, _)) = authz::validate_session(
                &snapshot,
                &cookie.session_id,
                &cookie.secret,
                crate::http::auth::now(),
            ) else {
                return Ok(Err(error(StatusCode::UNAUTHORIZED, "unauthorized")));
            };
            if current != account {
                return Ok(Err(error(StatusCode::UNAUTHORIZED, "unauthorized")));
            }
            let result =
                build_listing(&snapshot, &account, q.folder_id, q.cursor, sort, limit, key);
            snapshot.commit()?;
            Ok(result)
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
    let path = namespace::folder_path(db, account, &folder).map_err(|failure| match failure {
        NamespaceError::NotFound => error(StatusCode::NOT_FOUND, "not_found"),
        _ => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    })?;
    let generation = path.last().expect("validated folder path").generation;
    let after = if let Some(value) = cursor {
        let Some(decoded) = decode_cursor(&key, &value) else {
            return Err(error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        };
        if decoded.version != 3
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
            || decoded.last_modified_at.is_some_and(|time| time < 0)
            || decoded.last_size < 0
            || (decoded.last_kind_rank == 0 && decoded.last_size != 0)
            || (decoded.last_kind_rank == 0 && decoded.last_modified_at.is_some())
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
            modified_at: decoded.last_modified_at,
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
                    version: 3,
                    actor: account.to_owned(),
                    root: root_id,
                    folder: folder.clone(),
                    generation,
                    filter: "all".to_owned(),
                    sort,
                    key_version: crate::domain::names::NAME_KEY_VERSION,
                    last_kind_rank: last.position.kind_rank,
                    last_size: last.position.size,
                    last_modified_at: last.position.modified_at,
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
                size: row.size,
                modified_at: row.modified_at,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrashQuery {
    sort: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct TrashCursor {
    version: u8,
    actor: String,
    sort: namespace::ResultSort,
    fingerprint: String,
    last_name: String,
    last_id: String,
}

fn trash_cursor_encode(key: &[u8; 32], cursor: &TrashCursor) -> Option<String> {
    let body = serde_json::to_vec(cursor).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(b"sylux:trash-list-cursor:v2\0");
    mac.update(&body);
    let token = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(body),
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    );
    (token.len() <= MAX_CURSOR_BYTES).then_some(token)
}

fn trash_cursor_decode(key: &[u8; 32], value: &str) -> Option<TrashCursor> {
    if value.len() > MAX_CURSOR_BYTES {
        return None;
    }
    let (body, signature) = value.split_once('.')?;
    let body = URL_SAFE_NO_PAD.decode(body).ok()?;
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(b"sylux:trash-list-cursor:v2\0");
    mac.update(&body);
    mac.verify_slice(&signature).ok()?;
    serde_json::from_slice(&body).ok()
}

pub async fn trash_listing(
    State(s): State<AuthState>,
    headers: HeaderMap,
    Query(q): Query<TrashQuery>,
) -> Response {
    let sort = match result_sort(q.sort.as_deref()) {
        Ok(sort) => sort,
        Err(response) => return response,
    };
    let limit = q.limit.unwrap_or(100);
    if limit == 0 || limit > namespace::MAX_TRASH_PAGE_SIZE {
        return error(StatusCode::BAD_REQUEST, "invalid_limit");
    }
    let (account, _) = match principal(&s, &headers).await {
        Ok(x) => x,
        Err(r) => return error(r.0, r.1),
    };
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let key = cursor_key(&s);
    let result = s
        .workers
        .read(move |db| {
            let snapshot = db.unchecked_transaction()?;
            let Some((current, _, _)) = authz::validate_session(
                &snapshot,
                &cookie.session_id,
                &cookie.secret,
                crate::http::auth::now(),
            ) else {
                return Ok(Err(error(StatusCode::UNAUTHORIZED, "unauthorized")));
            };
            if current != account {
                return Ok(Err(error(StatusCode::UNAUTHORIZED, "unauthorized")));
            }
            let reply = build_trash_listing(&snapshot, &account, q.cursor, sort, limit, key);
            snapshot.commit()?;
            Ok(reply)
        })
        .await;
    match result {
        Ok(Ok(reply)) => (StatusCode::OK, Json(reply)).into_response(),
        Ok(Err(response)) => response,
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

fn build_trash_listing(
    db: &rusqlite::Connection,
    account: &str,
    cursor: Option<String>,
    sort: namespace::ResultSort,
    limit: usize,
    key: [u8; 32],
) -> Result<serde_json::Value, Response> {
    let decoded = if let Some(token) = cursor {
        let cursor = trash_cursor_decode(&key, &token)
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid_cursor"))?;
        if cursor.version != 2
            || cursor.sort != sort
            || cursor.actor != account
            || crate::domain::names::EntryName::parse(&cursor.last_name).is_err()
            || cursor.last_id.len() != 32
            || !cursor
                .last_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        }
        Some(cursor)
    } else {
        None
    };
    let fingerprint = namespace::trash_fingerprint(db, account)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"))?;
    if decoded
        .as_ref()
        .is_some_and(|cursor| cursor.fingerprint != fingerprint)
    {
        return Err(error(StatusCode::CONFLICT, "listing_changed"));
    }
    let after = decoded.map(|cursor| namespace::TrashPosition {
        name: cursor.last_name,
        id: cursor.last_id,
    });
    let page = namespace::trashed_page_sorted(db, account, sort, after.as_ref(), limit)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"))?;
    let next_cursor = if page.has_more {
        let last = page.items.last().expect("nonempty bounded Trash page");
        Some(
            trash_cursor_encode(
                &key,
                &TrashCursor {
                    version: 2,
                    sort,
                    actor: account.to_owned(),
                    fingerprint,
                    last_name: last.entry.name.clone(),
                    last_id: last.entry.id.clone(),
                },
            )
            .ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"))?,
        )
    } else {
        None
    };
    Ok(
        serde_json::json!({"items": page.items, "next_cursor": next_cursor, "has_more": page.has_more}),
    )
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
        Ok(Err(WriteFailure::Namespace(NamespaceError::ParentUnavailable))) => {
            error(StatusCode::CONFLICT, "parent_unavailable")
        }
        Ok(Err(WriteFailure::Namespace(NamespaceError::Conflict))) => {
            error(StatusCode::CONFLICT, "name_conflict")
        }
        _ => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    #[test]
    fn listing_cursor_rejects_correctly_signed_v2_domain() {
        let key = [7; 32];
        let body = serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "actor": "owner",
            "root": "root",
            "folder": "root",
            "generation": 1,
            "filter": "all",
            "sort": "name_asc",
            "key_version": 1,
            "last_kind_rank": 0,
            "last_size": 0,
            "last_key": "QQ",
            "last_id": "00000000000000000000000000000001"
        }))
        .unwrap();
        let cursor: Cursor = serde_json::from_slice(&body).unwrap();
        let mut mac = HmacSha256::new_from_slice(&key).unwrap();
        mac.update(b"sylux:entry-list-cursor:v2\0");
        mac.update(&body);
        let signature = mac.finalize().into_bytes();
        let mut verifier = HmacSha256::new_from_slice(&key).unwrap();
        verifier.update(b"sylux:entry-list-cursor:v2\0");
        verifier.update(&body);
        verifier.verify_slice(&signature).unwrap();
        let obsolete = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&body),
            URL_SAFE_NO_PAD.encode(signature)
        );
        assert!(decode_cursor(&key, &obsolete).is_none());
        // The old payload is readable; the signing domain causes the rejection.
        assert!(decode_cursor(&key, &encode_cursor(&key, &cursor).unwrap()).is_some());
    }
}
