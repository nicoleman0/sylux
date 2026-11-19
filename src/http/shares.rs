#![allow(clippy::result_large_err)]

use super::{
    AuthState,
    auth::now,
    content::attachment_filename,
    middleware::{SessionCookie, decode_secret, origin_matches, session_cookie},
};
use crate::domain::{
    authz,
    ids::MutationId,
    namespace::{self, NamespaceError},
    upload::validate_id,
};
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::io::ReaderStream;

pub(super) const DOWNLOAD_SLOTS: usize = 8;
const OWNER_LINK_LIMIT: i64 = 50;
const SECONDS_PER_DAY: i64 = 86_400;

#[derive(Clone, Copy)]
enum Failure {
    Unauthorized,
    Forbidden,
    EntryMissing,
    InvalidExpiry,
    Missing,
    Limit,
    Unavailable,
    Integrity,
}

impl From<rusqlite::Error> for Failure {
    fn from(_: rusqlite::Error) -> Self {
        Self::Unavailable
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "authentication_required"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::EntryMissing => (StatusCode::NOT_FOUND, "entry_not_found"),
            Self::InvalidExpiry => (StatusCode::BAD_REQUEST, "invalid_share_expiry"),
            Self::Missing => (StatusCode::NOT_FOUND, "share_not_found"),
            Self::Limit => (StatusCode::CONFLICT, "share_link_limit"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "share_unavailable"),
            Self::Integrity => (StatusCode::INTERNAL_SERVER_ERROR, "content_integrity_error"),
        };
        secured((status, Json(serde_json::json!({ "code": code }))).into_response())
    }
}

fn secured(mut response: Response) -> Response {
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

fn owner(db: &Connection, cookie: &SessionCookie, time: i64) -> Result<String, Failure> {
    authz::validate_session(db, &cookie.session_id, &cookie.secret, time)
        .map(|(id, _, _)| id)
        .ok_or(Failure::Unauthorized)
}

fn write_credentials(
    state: &AuthState,
    headers: &HeaderMap,
) -> Result<(SessionCookie, [u8; 32]), Failure> {
    let cookie = session_cookie(headers).ok_or(Failure::Unauthorized)?;
    if !origin_matches(headers, &state.origin) {
        return Err(Failure::Forbidden);
    }
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .and_then(decode_secret)
        .ok_or(Failure::Forbidden)?;
    Ok((cookie, csrf))
}

fn authorize_write(
    db: &Connection,
    cookie: &SessionCookie,
    csrf: &[u8; 32],
    time: i64,
) -> Result<String, Failure> {
    let account = owner(db, cookie, time)?;
    if !authz::csrf_matches(db, &cookie.session_id, csrf) {
        return Err(Failure::Forbidden);
    }
    Ok(account)
}

fn require_live_parent(
    db: &Connection,
    account: &str,
    parent: &str,
    missing: Failure,
) -> Result<(), Failure> {
    namespace::folder_path(db, account, parent)
        .map(|_| ())
        .map_err(|error| match error {
            NamespaceError::NotFound => missing,
            _ => Failure::Unavailable,
        })
}

fn default_expiry_days() -> u8 {
    7
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateLink {
    entry_id: String,
    #[serde(default = "default_expiry_days")]
    expiry_days: u8,
}

#[derive(Serialize)]
struct Link {
    id: String,
    entry_id: String,
    name: String,
    created_at: i64,
    expires_at: i64,
}

#[derive(Serialize)]
struct CreatedLink {
    #[serde(flatten)]
    link: Link,
    url: String,
}

pub async fn create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<CreateLink>,
) -> Response {
    match create_link(state, headers, input).await {
        Ok(link) => secured((StatusCode::CREATED, Json(link)).into_response()),
        Err(failure) => failure.into_response(),
    }
}

async fn create_link(
    state: AuthState,
    headers: HeaderMap,
    input: CreateLink,
) -> Result<CreatedLink, Failure> {
    let (cookie, csrf) = write_credentials(&state, &headers)?;
    let CreateLink {
        entry_id,
        expiry_days,
    } = input;
    if !matches!(expiry_days, 1 | 7 | 30) {
        return Err(Failure::InvalidExpiry);
    }
    validate_id(&entry_id).map_err(|_| Failure::EntryMissing)?;
    let mut token = [0u8; 32];
    getrandom::fill(&mut token).map_err(|_| Failure::Unavailable)?;
    let verifier = authz::verifier(&token);
    let id = MutationId::generate().map_err(|_| Failure::Unavailable)?;
    let link = state.workers.transact_result(move |db| {
        let time = now();
        let account = authorize_write(db, &cookie, &csrf, time)?;
        let (name, revision, parent): (String, String, String) = db.query_row(
            "SELECT e.original_name,r.revision_id,e.parent_id FROM entry e
             JOIN revision r ON r.revision_id=e.current_revision_id AND r.entry_id=e.entry_id
             JOIN blob b ON b.blob_id=r.blob_id AND b.lifecycle='live'
             WHERE e.entry_id=?1 AND e.owner_account_id=?2 AND e.state='active' AND e.kind='file'",
            params![entry_id, account],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?.ok_or(Failure::EntryMissing)?;
        require_live_parent(db, &account, &parent, Failure::EntryMissing)?;
        // Use short, indexed cleanup batches so old links cannot stall storage writes.
        db.execute(
            "DELETE FROM share_link WHERE share_id IN (
                SELECT share_id FROM share_link WHERE expires_at<=?1 ORDER BY expires_at LIMIT 50
            )",
            [time],
        )?;
        let active: i64 = db.query_row(
            "SELECT count(*) FROM share_link WHERE owner_account_id=?1 AND expires_at>?2",
            params![account, time],
            |row| row.get(0),
        )?;
        if active >= OWNER_LINK_LIMIT {
            return Err(Failure::Limit);
        }
        let expires_at = time + i64::from(expiry_days) * SECONDS_PER_DAY;
        db.execute(
            "INSERT INTO share_link(share_id,verifier,owner_account_id,entry_id,revision_id,created_at,expires_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![id.as_str(), verifier.as_slice(), account, entry_id, revision, time, expires_at],
        )?;
        Ok(Link { id: id.as_str().to_owned(), entry_id, name, created_at: time, expires_at })
    }).await.map_err(|_| Failure::Unavailable)??;
    Ok(CreatedLink {
        link,
        url: format!("{}/share/{}", state.origin, URL_SAFE_NO_PAD.encode(token)),
    })
}

pub async fn list(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let Some(cookie) = session_cookie(&headers) else {
        return Failure::Unauthorized.into_response();
    };
    let result = state
        .workers
        .read(move |db| {
            let snapshot = db.unchecked_transaction()?;
            let time = now();
            let account = match owner(&snapshot, &cookie, time) {
                Ok(account) => account,
                Err(failure) => return Ok(Err(failure)),
            };
            let rows = {
                let mut statement = snapshot.prepare(
                    "SELECT s.share_id,s.entry_id,e.original_name,s.created_at,s.expires_at
                 FROM share_link s JOIN entry e ON e.entry_id=s.entry_id
                 AND e.owner_account_id=s.owner_account_id AND e.current_revision_id=s.revision_id
                 WHERE s.owner_account_id=?1 AND s.expires_at>?2 AND e.state='active'
                 ORDER BY s.created_at DESC,s.share_id",
                )?;
                statement
                    .query_map(params![account, time], |row| {
                        Ok(Link {
                            id: row.get(0)?,
                            entry_id: row.get(1)?,
                            name: row.get(2)?,
                            created_at: row.get(3)?,
                            expires_at: row.get(4)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            snapshot.commit()?;
            Ok(Ok(rows))
        })
        .await;
    match result {
        Ok(Ok(links)) => secured(Json(links).into_response()),
        Ok(Err(failure)) => failure.into_response(),
        Err(_) => Failure::Unavailable.into_response(),
    }
}

pub async fn revoke(
    State(state): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (cookie, csrf) = match write_credentials(&state, &headers) {
        Ok(credentials) => credentials,
        Err(failure) => return failure.into_response(),
    };
    if validate_id(&id).is_err() {
        return Failure::Missing.into_response();
    }
    let result = state
        .workers
        .transact_result(move |db| {
            let account = authorize_write(db, &cookie, &csrf, now())?;
            // A repeated revoke has the same outcome and reveals no other owner's records.
            db.execute(
                "DELETE FROM share_link WHERE share_id=?1 AND owner_account_id=?2",
                params![id, account],
            )?;
            Ok(())
        })
        .await;
    match result {
        Ok(Ok(())) => secured(StatusCode::NO_CONTENT.into_response()),
        Ok(Err(failure)) => Failure::into_response(failure),
        Err(_) => Failure::Unavailable.into_response(),
    }
}

struct Download {
    name: String,
    blob_id: String,
    length: u64,
    digest: [u8; 32],
}

fn resolve(db: &Connection, verifier: [u8; 32]) -> Result<Download, Failure> {
    let snapshot = db.unchecked_transaction()?;
    let (download, account, parent) = snapshot
        .query_row(
            "SELECT e.original_name,b.blob_id,b.length,b.sha256,s.owner_account_id,e.parent_id
         FROM share_link s
         JOIN account a ON a.account_id=s.owner_account_id AND a.enabled=1
         JOIN entry e ON e.entry_id=s.entry_id AND e.owner_account_id=s.owner_account_id
         JOIN revision r ON r.revision_id=s.revision_id AND r.entry_id=e.entry_id
         JOIN blob b ON b.blob_id=r.blob_id AND b.lifecycle='live'
         WHERE s.verifier=?1 AND s.expires_at>?2 AND e.kind='file'
         AND e.state='active' AND e.current_revision_id=s.revision_id",
            params![verifier.as_slice(), now()],
            |row| {
                let length = u64::try_from(row.get::<_, i64>(2)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let digest = row
                    .get::<_, Vec<u8>>(3)?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok((
                    Download {
                        name: row.get(0)?,
                        blob_id: row.get(1)?,
                        length,
                        digest,
                    },
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or(Failure::Missing)?;
    require_live_parent(&snapshot, &account, &parent, Failure::Missing)?;
    snapshot.commit()?;
    Ok(download)
}

pub async fn missing() -> Response {
    Failure::Missing.into_response()
}

pub async fn download(State(state): State<AuthState>, method: Method, uri: Uri) -> Response {
    if method != Method::GET {
        return secured((StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "GET")]).into_response());
    }
    let Some(token) = uri.path().strip_prefix("/share/").and_then(decode_secret) else {
        return missing().await;
    };
    let Ok(permit) = Arc::clone(&state.share_downloads).try_acquire_owned() else {
        return secured(
            (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "1")]).into_response(),
        );
    };
    let verifier = authz::verifier(&token);
    let (reference, permit) = match state
        .workers
        .read(move |db| Ok(resolve(db, verifier).map(|reference| (reference, permit))))
        .await
    {
        Ok(Ok(resolved)) => resolved,
        Ok(Err(failure)) => return failure.into_response(),
        Err(_) => return Failure::Unavailable.into_response(),
    };
    let Some(storage) = state.storage else {
        return Failure::Unavailable.into_response();
    };
    let Download {
        name,
        blob_id,
        length,
        digest,
    } = reference;
    let opened = tokio::task::spawn_blocking(move || {
        let file = storage.payloads.open_verified(&blob_id, length, digest)?;
        Ok::<_, crate::persistence::payload::PayloadError>((file, permit))
    })
    .await;
    let (file, permit) = match opened {
        Ok(Ok(opened)) => opened,
        Ok(Err(crate::persistence::payload::PayloadError::Missing)) => return missing().await,
        _ => return Failure::Integrity.into_response(),
    };
    let body = Body::from_stream(ReaderStream::new(tokio::fs::File::from_std(file.file)).map(
        move |chunk| {
            let _hold = &permit;
            chunk
        },
    ));
    secured(
        (
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
                (header::CONTENT_DISPOSITION, attachment_filename(&name)),
                (header::CONTENT_LENGTH, length.to_string()),
            ],
            body,
        )
            .into_response(),
    )
}
