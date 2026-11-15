//! ZIP downloads of a folder or selection. `POST /archives` checks the selection and issues a
//! short-lived, single-use ticket; navigating to `GET /archives/{ticket}` lets the browser
//! stream the archive to disk. The member list is fixed when that GET resolves it.
#![allow(clippy::result_large_err)]

use super::{
    AuthState,
    content::attachment_filename,
    middleware::{decode_secret, origin_matches, session_cookie},
};
use crate::domain::{
    archive::{self, ArchiveContent, ArchiveEntry, ArchiveError, MAX_ARCHIVE_ENTRIES},
    authz,
    upload::{UploadStorage, validate_id},
    zip::{self, Member, MemberKind},
};
use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{
        HeaderMap, HeaderValue, Method, StatusCode,
        header::{
            CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE, REFERRER_POLICY,
            X_CONTENT_TYPE_OPTIONS,
        },
    },
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::HashMap,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::io::AsyncReadExt;

const TICKET_SECONDS: i64 = 60;
const TICKETS_PER_ACCOUNT: usize = 4;
const CHUNK_BYTES: usize = 64 * 1024;

pub struct ArchiveTicket {
    account_id: String,
    session_id: String,
    entry_ids: Vec<String>,
    expires_at: i64,
}

/// Outstanding tickets keyed by the SHA-256 of their token.
pub type ArchiveTickets = HashMap<[u8; 32], ArchiveTicket>;

fn error(status: StatusCode, code: &'static str) -> Response {
    let mut response = (
        status,
        Json(json!({"code":code,"message":code,"request_id":"poc"})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn plan_error(failure: ArchiveError) -> Response {
    match failure {
        ArchiveError::NotFound => error(StatusCode::NOT_FOUND, "entry_not_found"),
        ArchiveError::TooManyEntries => entry_limit(),
        ArchiveError::NameConflict => error(StatusCode::CONFLICT, "archive_name_conflict"),
        ArchiveError::Integrity => {
            error(StatusCode::INTERNAL_SERVER_ERROR, "content_integrity_error")
        }
        ArchiveError::Sql => error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"),
    }
}

fn entry_limit() -> Response {
    let code = "archive_entry_limit";
    let mut response = (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({"code":code,"message":code,"request_id":"poc","limit":MAX_ARCHIVE_ENTRIES})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveRequest {
    entry_ids: Vec<String>,
}

pub async fn issue(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<ArchiveRequest>,
) -> Response {
    if !origin_matches(&headers, &state.origin) {
        return error(StatusCode::FORBIDDEN, "forbidden");
    }
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "authentication_required");
    };
    let Some(csrf) = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .and_then(decode_secret)
    else {
        return error(StatusCode::FORBIDDEN, "forbidden");
    };
    let mut entry_ids = input.entry_ids;
    if entry_ids.is_empty() {
        return error(StatusCode::BAD_REQUEST, "invalid_selection");
    }
    if entry_ids.len() > MAX_ARCHIVE_ENTRIES {
        return entry_limit();
    }
    if entry_ids.iter().any(|id| validate_id(id).is_err()) {
        return error(StatusCode::NOT_FOUND, "entry_not_found");
    }
    entry_ids.sort();
    entry_ids.dedup();
    let session_id = cookie.session_id.clone();
    let ids = entry_ids.clone();
    let resolved = state
        .workers
        .read(move |connection| {
            let snapshot = connection.unchecked_transaction()?;
            let Some((account, _, _)) =
                authz::validate_session(&snapshot, &session_id, &cookie.secret, now())
            else {
                return Ok(Err(error(
                    StatusCode::UNAUTHORIZED,
                    "authentication_required",
                )));
            };
            if !authz::csrf_matches(&snapshot, &session_id, &csrf) {
                return Ok(Err(error(StatusCode::FORBIDDEN, "forbidden")));
            }
            let plan = archive::plan(&snapshot, &account, &ids).map_err(plan_error);
            snapshot.commit()?;
            Ok(plan.map(|plan| (account, plan)))
        })
        .await;
    let (account_id, plan) = match resolved {
        Ok(Ok(resolved)) => resolved,
        Ok(Err(response)) => return response,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"),
    };
    let mut token = [0u8; 32];
    if getrandom::fill(&mut token).is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "ticket_unavailable");
    }
    let Ok(mut tickets) = state.archive_tickets.lock() else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "ticket_unavailable");
    };
    let issued_at = now();
    tickets.retain(|_, ticket| ticket.expires_at > issued_at);
    if tickets
        .values()
        .filter(|ticket| ticket.account_id == account_id)
        .count()
        >= TICKETS_PER_ACCOUNT
    {
        return error(StatusCode::TOO_MANY_REQUESTS, "archive_busy");
    }
    tickets.insert(
        authz::verifier(&token),
        ArchiveTicket {
            account_id,
            session_id: cookie.session_id,
            entry_ids,
            expires_at: issued_at + TICKET_SECONDS,
        },
    );
    drop(tickets);
    let mut response = (
        StatusCode::CREATED,
        Json(json!({
            "url": format!("/archives/{}", URL_SAFE_NO_PAD.encode(token)),
            "name": plan.name,
            "entry_count": plan.entries.len(),
        })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub async fn download(
    State(state): State<AuthState>,
    Path(token): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if method != Method::GET {
        let mut response = error(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
        response
            .headers_mut()
            .insert(axum::http::header::ALLOW, HeaderValue::from_static("GET"));
        return response;
    }
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "authentication_required");
    };
    let Some(token) = decode_secret(&token) else {
        return error(StatusCode::NOT_FOUND, "archive_not_found");
    };
    let key = authz::verifier(&token);
    let ticket = match state.archive_tickets.lock() {
        // Only the issuing session can spend a ticket.
        Ok(mut tickets) => match tickets.get(&key) {
            Some(ticket) if ticket.session_id == cookie.session_id => tickets.remove(&key),
            _ => None,
        },
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "ticket_unavailable"),
    };
    let Some(ticket) = ticket.filter(|ticket| ticket.expires_at > now()) else {
        return error(StatusCode::NOT_FOUND, "archive_not_found");
    };
    let Some(storage) = state.storage.clone() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable");
    };
    let resolved = state
        .workers
        .read(move |connection| {
            let snapshot = connection.unchecked_transaction()?;
            let account =
                authz::validate_session(&snapshot, &ticket.session_id, &cookie.secret, now())
                    .map(|(account, _, _)| account);
            let plan = match account {
                Some(account) if account == ticket.account_id => {
                    archive::plan(&snapshot, &account, &ticket.entry_ids).map_err(plan_error)
                }
                Some(_) => Err(error(StatusCode::NOT_FOUND, "archive_not_found")),
                None => Err(error(StatusCode::UNAUTHORIZED, "authentication_required")),
            };
            snapshot.commit()?;
            Ok(plan)
        })
        .await;
    let plan = match resolved {
        Ok(Ok(plan)) => plan,
        Ok(Err(response)) => return response,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"),
    };
    let stream = ArchiveStream::new(
        plan.entries,
        storage,
        #[cfg(feature = "test-harness")]
        state.archive_activity.clone(),
    );
    let mut response = Response::new(Body::from_stream(futures_util::stream::unfold(
        stream,
        ArchiveStream::next,
    )));
    for (name, value) in [
        (CONTENT_TYPE, "application/zip".to_owned()),
        (X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        (CACHE_CONTROL, "no-store".to_owned()),
        (REFERRER_POLICY, "no-referrer".to_owned()),
        (CONTENT_DISPOSITION, attachment_filename(&plan.name)),
    ] {
        let Ok(value) = HeaderValue::from_str(&value) else {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "response_header_failed");
        };
        response.headers_mut().insert(name, value);
    }
    response
}

/// Signals a running payload verification to stop when the response body is dropped.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

struct OpenFile {
    member: Member,
    offset: u64,
    file: tokio::fs::File,
    remaining: u64,
    crc: crc32fast::Hasher,
}

/// Archive body state. It does work only when the server polls it for the next chunk, so
/// dropping the body (client gone) stops it; at most one chunk-sized read is in flight.
struct ArchiveStream {
    pending: std::vec::IntoIter<ArchiveEntry>,
    written: Vec<(Member, u32, u64)>,
    offset: u64,
    current: Option<OpenFile>,
    storage: Arc<UploadStorage>,
    folder_time: (u16, u16),
    cancelled: CancelOnDrop,
    finished: bool,
    #[cfg(feature = "test-harness")]
    activity: Arc<()>,
}

impl ArchiveStream {
    fn new(
        entries: Vec<ArchiveEntry>,
        storage: Arc<UploadStorage>,
        #[cfg(feature = "test-harness")] activity: Arc<()>,
    ) -> Self {
        Self {
            written: Vec::with_capacity(entries.len()),
            pending: entries.into_iter(),
            offset: 0,
            current: None,
            storage,
            folder_time: zip::dos_timestamp(now()),
            cancelled: CancelOnDrop(Arc::new(AtomicBool::new(false))),
            finished: false,
            #[cfg(feature = "test-harness")]
            activity,
        }
    }

    async fn next(mut self) -> Option<(Result<Bytes, io::Error>, Self)> {
        if self.finished {
            return None;
        }
        match self.step().await {
            Ok(bytes) => Some((Ok(bytes), self)),
            // An error ends the body before the central directory, so the client
            // sees a failed transfer rather than a shorter valid archive.
            Err(failure) => {
                self.finished = true;
                Some((Err(failure), self))
            }
        }
    }

    async fn step(&mut self) -> Result<Bytes, io::Error> {
        if let Some(open) = &mut self.current
            && open.remaining > 0
        {
            let mut buffer = vec![0u8; CHUNK_BYTES.min(open.remaining as usize)];
            let read = open.file.read(&mut buffer).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "payload_truncated",
                ));
            }
            buffer.truncate(read);
            open.crc.update(&buffer);
            open.remaining -= read as u64;
            self.offset += read as u64;
            return Ok(Bytes::from(buffer));
        }
        if let Some(open) = self.current.take() {
            let crc = open.crc.finalize();
            let descriptor = zip::data_descriptor(&open.member, crc);
            self.offset += descriptor.len() as u64;
            self.written.push((open.member, crc, open.offset));
            return Ok(Bytes::from(descriptor));
        }
        if let Some(entry) = self.pending.next() {
            return self.begin(entry).await;
        }
        let start = self.offset;
        let mut directory = Vec::new();
        for (member, crc, offset) in &self.written {
            directory.extend_from_slice(&zip::central_header(member, *crc, *offset));
        }
        let size = directory.len() as u64;
        directory.extend_from_slice(&zip::end_records(self.written.len() as u64, start, size));
        self.finished = true;
        Ok(Bytes::from(directory))
    }

    async fn begin(&mut self, entry: ArchiveEntry) -> Result<Bytes, io::Error> {
        let offset = self.offset;
        match entry.content {
            ArchiveContent::Folder => {
                let member = Member {
                    name: format!("{}/", entry.path),
                    kind: MemberKind::Directory,
                    size: 0,
                    dos_time: self.folder_time.0,
                    dos_date: self.folder_time.1,
                };
                let header = zip::local_header(&member);
                self.offset += header.len() as u64;
                self.written.push((member, 0, offset));
                Ok(Bytes::from(header))
            }
            ArchiveContent::File {
                blob_id,
                length,
                sha256,
                modified_at,
            } => {
                let storage = Arc::clone(&self.storage);
                let cancelled = Arc::clone(&self.cancelled.0);
                #[cfg(feature = "test-harness")]
                let activity = Arc::clone(&self.activity);
                let verified = tokio::task::spawn_blocking(move || {
                    #[cfg(feature = "test-harness")]
                    let _activity = activity;
                    storage
                        .payloads
                        .open_verified_unless(&blob_id, length, sha256, &cancelled)
                })
                .await
                .map_err(|_| io::Error::other("payload_verification_failed"))?
                .map_err(|_| io::Error::other("content_integrity_error"))?;
                let (dos_time, dos_date) = zip::dos_timestamp(modified_at);
                let member = Member {
                    name: entry.path,
                    kind: MemberKind::File,
                    size: length,
                    dos_time,
                    dos_date,
                };
                let header = zip::local_header(&member);
                self.offset += header.len() as u64;
                self.current = Some(OpenFile {
                    member,
                    offset,
                    file: tokio::fs::File::from_std(verified.file),
                    remaining: length,
                    crc: crc32fast::Hasher::new(),
                });
                Ok(Bytes::from(header))
            }
        }
    }
}

fn now() -> i64 {
    super::auth::now()
}
