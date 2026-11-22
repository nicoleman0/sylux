#![allow(clippy::result_large_err)]

use super::{
    AuthState,
    middleware::{decode_secret, origin_matches, session_cookie},
};
use crate::domain::{
    authz::csrf_matches,
    coordinator::AdmissionTicket,
    upload::{
        ActiveUpload, MAX_TEXT_EDIT_LENGTH, UploadAdmission, UploadStorage, VerifiedUploadTicket,
        WorkerError, admission_fingerprint, blob_id_from_staging, folder_etag,
        generation_from_etag, hex, is_editable_text, parse_digest, random_id, revision_etag,
        staging_name, validate_id,
    },
    upload::{UPLOAD_SCOPE, issue_upload_ticket, verify_upload_ticket},
};
use crate::persistence::payload::{PayloadError, PublishedPayload, StagingPayload};
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{
            CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH, LOCATION,
        },
    },
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;

#[cfg(feature = "test-harness")]
#[derive(Default)]
pub struct UploadCancelTestBarrier {
    cancel_paused: tokio::sync::Notify,
    cancel_release: tokio::sync::Notify,
    publish_armed: std::sync::atomic::AtomicBool,
    publish_paused: tokio::sync::Notify,
    publish_release: tokio::sync::Notify,
}

#[cfg(feature = "test-harness")]
impl UploadCancelTestBarrier {
    pub async fn wait_cancel_paused(&self) {
        self.cancel_paused.notified().await;
    }

    pub fn release_cancel(&self) {
        self.cancel_release.notify_one();
    }

    /// Pause the next PUT between payload publication and its cancel check.
    pub fn arm_publish_pause(&self) {
        self.publish_armed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub async fn wait_publish_paused(&self) {
        self.publish_paused.notified().await;
    }

    pub fn release_publish(&self) {
        self.publish_release.notify_one();
    }

    async fn pause_publish(&self) {
        if self
            .publish_armed
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.publish_paused.notify_one();
            self.publish_release.notified().await;
        }
    }

    async fn pause_cancel(&self) {
        self.cancel_paused.notify_one();
        self.cancel_release.notified().await;
    }
}

#[cfg(feature = "test-harness")]
#[derive(Default)]
pub struct UploadCommitTestBarrier {
    armed: std::sync::atomic::AtomicBool,
    paused: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[cfg(feature = "test-harness")]
impl UploadCommitTestBarrier {
    pub fn arm(&self) {
        self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub async fn wait_paused(&self) {
        self.paused.notified().await;
    }
    pub fn release(&self) {
        self.release.notify_one();
    }
    async fn pause(&self) {
        if self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.paused.notify_one();
            self.release.notified().await;
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TicketRequest {
    scope: String,
    #[serde(flatten)]
    admission: UploadAdmission,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    sha256: String,
    parent_etag: String,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn finish(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn error(status: StatusCode, code: &'static str) -> Response {
    finish(
        (
            status,
            Json(json!({
                "code": code,
                "message": code,
                "request_id": "poc"
            })),
        )
            .into_response(),
    )
}

fn storage(state: &AuthState) -> Result<Arc<UploadStorage>, Response> {
    state
        .storage
        .clone()
        .ok_or_else(|| error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"))
}

fn one_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let name = axum::http::HeaderName::from_bytes(name.as_bytes()).ok()?;
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}

fn require_create_precondition(headers: &HeaderMap) -> Result<(), Response> {
    match one_header(headers, IF_NONE_MATCH.as_str()) {
        Some("*") => Ok(()),
        Some(_) => Err(error(
            StatusCode::PRECONDITION_FAILED,
            "create_precondition_failed",
        )),
        None => Err(error(
            StatusCode::PRECONDITION_REQUIRED,
            "create_precondition_required",
        )),
    }
}

fn require_upload_precondition(
    headers: &HeaderMap,
    admission: &UploadAdmission,
) -> Result<(), Response> {
    if let Some(replacement) = &admission.replacement {
        match one_header(headers, IF_MATCH.as_str()) {
            Some(etag) if etag == replacement.etag && !headers.contains_key(IF_NONE_MATCH) => {
                Ok(())
            }
            Some(_) => Err(error(
                StatusCode::PRECONDITION_FAILED,
                "replacement_precondition_failed",
            )),
            None => Err(error(
                StatusCode::PRECONDITION_REQUIRED,
                "replacement_precondition_required",
            )),
        }
    } else {
        require_create_precondition(headers)
    }
}

fn check_parent(db: &Connection, ticket: &VerifiedUploadTicket) -> Result<(), Response> {
    let path =
        crate::domain::namespace::folder_path(db, &ticket.account_id, &ticket.admission.parent_id)
            .map_err(|failure| match failure {
                crate::domain::namespace::NamespaceError::NotFound => {
                    error(StatusCode::NOT_FOUND, "parent_not_found")
                }
                _ => error(StatusCode::INTERNAL_SERVER_ERROR, "parent_lookup_failed"),
            })?;
    let generation = path.last().expect("validated folder path").generation;
    let expected = generation_from_etag(&ticket.admission.parent_etag, &ticket.admission.parent_id)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_parent_etag"))?;
    if generation < 0 || generation as u64 != expected {
        return Err(error(StatusCode::PRECONDITION_FAILED, "stale_parent_etag"));
    }
    Ok(())
}

fn check_destination(
    db: &Connection,
    account: &str,
    admission: &UploadAdmission,
    name_key: &[u8],
) -> Result<(), Response> {
    let collision: Option<(String, String, String, i64, Option<String>)> = db.query_row(
        "SELECT entry_id,original_name,kind,generation,current_revision_id FROM entry WHERE owner_account_id=?1 AND parent_id=?2 AND name_key=?3 AND state='active'",
        params![account, admission.parent_id, name_key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).optional().map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "name_lookup_failed"))?;
    if let Some(replacement) = &admission.replacement {
        let Some((id, name, kind, generation, revision)) = collision else {
            return Err(error(
                StatusCode::PRECONDITION_FAILED,
                "replacement_changed",
            ));
        };
        if kind != "file"
            || id != replacement.entry_id
            || folder_etag(&id, generation as u64) != replacement.etag
            || replacement.text_revision.as_ref().is_some_and(|expected| {
                revision.as_ref() != Some(expected) || name != admission.name
            })
        {
            return Err(error(
                StatusCode::PRECONDITION_FAILED,
                "replacement_changed",
            ));
        }
    } else if let Some((id, name, kind, generation, _)) = collision {
        return Err(finish((StatusCode::CONFLICT, Json(json!({
            "code": "name_collision", "message": "An item already has this name.", "request_id": "poc",
            "existing": { "id": id, "name": name, "kind": kind, "etag": folder_etag(&id, generation as u64) }
        }))).into_response()));
    }
    Ok(())
}

async fn authorize_mutation(
    state: &AuthState,
    headers: &HeaderMap,
) -> Result<AdmissionTicket, Response> {
    if !origin_matches(headers, &state.origin) {
        return Err(error(StatusCode::FORBIDDEN, "origin_required"));
    }
    let cookie = session_cookie(headers)
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "authentication_required"))?;
    let csrf = one_header(headers, "x-csrf-token")
        .and_then(decode_secret)
        .ok_or_else(|| error(StatusCode::FORBIDDEN, "csrf_required"))?;
    let session_id = cookie.session_id;
    let secret = cookie.secret;
    let coordinator = state.coordinator.clone();
    let result = state
        .workers
        .read(move |db| {
            let ticket = match coordinator.admit_current(db, &session_id, &secret, now()) {
                Ok(ticket) => ticket,
                Err(_) => return Ok(Err(StatusCode::UNAUTHORIZED)),
            };
            if !csrf_matches(db, &session_id, &csrf) {
                return Ok(Err(StatusCode::FORBIDDEN));
            }
            Ok(Ok(ticket))
        })
        .await
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"))?;
    match result {
        Ok(ticket) => Ok(ticket),
        Err(status) => Err(error(
            status,
            if status == StatusCode::FORBIDDEN {
                "csrf_required"
            } else {
                "authentication_required"
            },
        )),
    }
}

fn verified_ticket(
    storage: &UploadStorage,
    headers: &HeaderMap,
    ticket: &AdmissionTicket,
) -> Result<VerifiedUploadTicket, Response> {
    let raw = one_header(headers, "x-mutation-id")
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "mutation_ticket_required"))?;
    verify_upload_ticket(storage, raw, ticket.account_id(), now())
        .map_err(|_| error(StatusCode::UNAUTHORIZED, "invalid_mutation_ticket"))
}

fn recovery_epoch(connection: &Connection) -> Result<i64, Response> {
    connection
        .query_row(
            "SELECT recovery_epoch FROM installation WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "installation_unavailable",
            )
        })
}

pub async fn mutation_ticket(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(request): Json<TicketRequest>,
) -> Response {
    if request.scope != UPLOAD_SCOPE {
        return error(StatusCode::BAD_REQUEST, "unsupported_mutation_scope");
    }
    let storage = match storage(&state) {
        Ok(storage) => storage,
        Err(response) => return response,
    };
    let principal = match authorize_mutation(&state, &headers).await {
        Ok(authorized) => authorized,
        Err(response) => return response,
    };
    let epoch = match state.workers.read(|db| Ok(recovery_epoch(db))).await {
        Ok(Ok(epoch)) => epoch,
        Ok(Err(response)) => return response,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"),
    };
    let issued = match issue_upload_ticket(
        &storage,
        principal.account_id(),
        epoch,
        &request.admission,
        now(),
    ) {
        Ok(issued) => issued,
        Err("upload_too_large") => {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "upload_too_large");
        }
        Err("text_edit_too_large") => {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "text_edit_too_large");
        }
        Err("text_edit_type") => {
            return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "text_edit_type");
        }
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_upload_admission"),
    };
    if let Some(replacement) = &request.admission.replacement
        && let Some(revision) = &replacement.text_revision
        && let Err(response) = super::content::validate_text_edit_source(
            &state,
            &headers,
            storage,
            &replacement.entry_id,
            revision,
            &request.admission.name,
        )
        .await
    {
        return response;
    }
    let mut response = finish(
        (
            StatusCode::CREATED,
            Json(json!({
                "mutation_id": issued.mutation_id,
                "ticket": issued.ticket,
                "expires_at": issued.expires_at,
                "max_chunk_length": crate::domain::upload::MAX_CHUNK_LENGTH,
                "max_upload_length": crate::domain::upload::MAX_UPLOAD_LENGTH,
            })),
        )
            .into_response(),
    );
    if let Ok(value) = HeaderValue::from_str(&issued.ticket) {
        response.headers_mut().insert("x-mutation-id", value);
    } else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "ticket_encoding_failed");
    }
    response
}

pub async fn admit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(admission): Json<UploadAdmission>,
) -> Response {
    if let Err(response) = require_upload_precondition(&headers, &admission) {
        return response;
    }
    let storage = match storage(&state) {
        Ok(storage) => storage,
        Err(response) => return response,
    };
    let principal = match authorize_mutation(&state, &headers).await {
        Ok(authorized) => authorized,
        Err(response) => return response,
    };
    let ticket = match verified_ticket(&storage, &headers, &principal) {
        Ok(ticket) => ticket,
        Err(response) => return response,
    };
    if ticket.admission != admission {
        return error(StatusCode::CONFLICT, "admission_fingerprint_mismatch");
    }
    let name_key = match admission.name_key() {
        Ok(name_key) => name_key,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_name"),
    };
    let fingerprint = admission_fingerprint(&ticket);
    let attempt_id = match random_id() {
        Ok(id) => id,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "entropy_unavailable"),
    };
    let blob_id = match random_id() {
        Ok(id) => id,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "entropy_unavailable"),
    };
    let staging_name = match staging_name(&attempt_id, &blob_id) {
        Ok(name) => name,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "id_generation_failed"),
    };
    let attempt_id_for_insert = attempt_id.clone();
    let staging_name_for_insert = staging_name.clone();
    let ticket_for_insert = ticket.clone();
    let principal_for_insert = principal.clone();
    let coordinator = state.coordinator.clone();
    let result = state.workers.transact(move |transaction| -> rusqlite::Result<Result<Option<(String, String)>, Response>> {
            if coordinator.revalidate_current(transaction, &principal_for_insert, now()).is_err() {
                return Ok(Err(error(StatusCode::UNAUTHORIZED, "authentication_required")));
            }
            if recovery_epoch(transaction).map_err(|_| rusqlite::Error::InvalidQuery)? != ticket_for_insert.recovery_epoch {
                return Ok(Err(error(StatusCode::UNAUTHORIZED, "stale_mutation_ticket")));
            }
            // Resolve retries before mutable namespace checks to preserve idempotency.
            let existing: Option<(Vec<u8>, String, String)> = transaction.query_row(
                "SELECT admission_fingerprint,state,scope FROM mutation WHERE mutation_id=?1 AND principal_account_id=?2",
                params![ticket_for_insert.mutation_id,ticket_for_insert.account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional()?;
            if let Some((stored_fingerprint, state, scope)) = existing {
                if stored_fingerprint.as_slice() != fingerprint.as_slice() || scope != UPLOAD_SCOPE {
                    return Ok(Err(error(StatusCode::CONFLICT, "admission_fingerprint_mismatch")));
                }
                let attempt: Option<String> = transaction.query_row(
                    "SELECT attempt_id FROM upload_attempt WHERE mutation_id=?1 ORDER BY attempt_no LIMIT 1",
                    [ticket_for_insert.mutation_id.as_str()], |row| row.get(0),
                ).optional()?;
                if let Some(attempt_id) = attempt {
                    return Ok(Ok(Some((attempt_id, state))));
                }
                return Ok(Err(error(StatusCode::SERVICE_UNAVAILABLE, "upload_state_unavailable")));
            }
            if let Err(response) = check_parent(transaction, &ticket_for_insert) {
                return Ok(Err(response));
            }
            if let Err(response) = check_destination(transaction, &ticket_for_insert.account_id, &ticket_for_insert.admission, &name_key) {
                return Ok(Err(response));
            }
            transaction.execute(
                "INSERT INTO mutation(mutation_id,principal_account_id,scope,ticket_key_id,ticket_epoch,issued_at,expires_at,admission_fingerprint,state) VALUES(?1,?2,?3,'poc-v1',?4,?5,?6,?7,'admitted')",
                params![ticket_for_insert.mutation_id,ticket_for_insert.account_id,UPLOAD_SCOPE,ticket_for_insert.recovery_epoch,ticket_for_insert.issued_at,ticket_for_insert.expires_at,fingerprint.as_slice()],
            )?;
            transaction.execute(
                "INSERT INTO upload_attempt(attempt_id,mutation_id,attempt_no,staging_path,writer_active,bytes_received) VALUES(?1,?2,1,?3,0,0)",
                params![attempt_id_for_insert,ticket_for_insert.mutation_id,staging_name_for_insert],
            )?;
            Ok(Ok(None))
        }).await;
    let (response_attempt_id, response_state, response_status) = match result {
        Ok(Ok(Some((attempt_id, state)))) => (attempt_id, state, StatusCode::OK),
        Ok(Ok(None)) => (attempt_id, "admitted".to_owned(), StatusCode::CREATED),
        Ok(Err(response)) => return response,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"),
    };
    let mut response = finish(
        (
            response_status,
            Json(json!({
                "upload_id": ticket.mutation_id,
                "mutation_id": ticket.mutation_id,
                "attempt_id": response_attempt_id,
                "status": response_state,
                "length": admission.length,
                "sha256": admission.sha256,
            })),
        )
            .into_response(),
    );
    if let Ok(location) = HeaderValue::from_str(&format!("/uploads/{}", ticket.mutation_id)) {
        response.headers_mut().insert(LOCATION, location);
    }
    response
}

#[derive(Clone)]
struct Attempt {
    attempt_id: String,
    staging_name: String,
    attempt_no: i64,
    writer_active: i64,
    bytes_received: i64,
    outcome: Option<String>,
}

struct WriterRegistration {
    storage: Arc<UploadStorage>,
    upload_id: String,
    writer: Arc<ActiveUpload>,
}

impl Drop for WriterRegistration {
    fn drop(&mut self) {
        self.storage.finish_writer(&self.upload_id, &self.writer);
    }
}

enum WorkerMessage {
    Data(Vec<u8>),
    Finish,
    Abort(PayloadError),
}

#[derive(Clone)]
enum WorkerOutcome {
    Published(PublishedPayload),
    /// Canceled; carries the payload if it was already published to its final
    /// path, so its metadata can still be recorded for reclamation.
    Canceled(Option<PublishedPayload>),
    Failed(PayloadError),
}

fn run_worker(
    mut staging: StagingPayload,
    mut messages: mpsc::Receiver<WorkerMessage>,
    writer: Arc<ActiveUpload>,
    declared_length: u64,
    declared_digest: Option<[u8; 32]>,
) -> WorkerOutcome {
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    loop {
        if writer.is_cancelled() {
            return WorkerOutcome::Canceled(None);
        }
        match messages.blocking_recv() {
            Some(WorkerMessage::Data(bytes)) => {
                let length = bytes.len() as u64;
                if length > declared_length.saturating_sub(total) {
                    return WorkerOutcome::Failed(PayloadError::LengthExceeded);
                }
                if let Err(error) = staging.write_chunk(&bytes) {
                    return WorkerOutcome::Failed(error);
                }
                hasher.update(&bytes);
                total += length;
            }
            Some(WorkerMessage::Finish) => break,
            Some(WorkerMessage::Abort(error)) => return WorkerOutcome::Failed(error),
            None => return WorkerOutcome::Failed(PayloadError::LengthMismatch),
        }
    }
    if writer.is_cancelled() {
        return WorkerOutcome::Canceled(None);
    }
    if total != declared_length {
        return WorkerOutcome::Failed(PayloadError::LengthMismatch);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    if declared_digest.is_some_and(|expected| expected != digest) {
        return WorkerOutcome::Failed(PayloadError::DigestMismatch);
    }
    match staging.publish(digest) {
        Ok(published) => WorkerOutcome::Published(published),
        Err(error) => WorkerOutcome::Failed(error),
    }
}

fn activate_attempt(
    transaction: &rusqlite::Transaction<'_>,
    ticket: &VerifiedUploadTicket,
    principal: &AdmissionTicket,
    coordinator: &crate::domain::coordinator::MutationCoordinator,
) -> rusqlite::Result<Result<Attempt, Response>> {
    if coordinator
        .revalidate_current(transaction, principal, now())
        .is_err()
    {
        return Ok(Err(error(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
        )));
    }
    if recovery_epoch(transaction).map_err(|_| rusqlite::Error::InvalidQuery)?
        != ticket.recovery_epoch
    {
        return Ok(Err(error(
            StatusCode::UNAUTHORIZED,
            "stale_mutation_ticket",
        )));
    }
    let state: Option<String> = transaction
        .query_row(
            "SELECT state FROM mutation WHERE mutation_id=?1 AND principal_account_id=?2 AND completed_sha256 IS NULL",
            params![ticket.mutation_id,ticket.account_id],
            |row| row.get(0),
        )
        .optional()
        ?;
    if state.as_deref() != Some("admitted") {
        return Ok(Err(error(StatusCode::CONFLICT, "upload_not_staging")));
    }
    if let Err(response) = check_parent(transaction, ticket) {
        return Ok(Err(response));
    }
    let active: Option<i64> = transaction
        .query_row(
            "SELECT 1 FROM upload_attempt WHERE mutation_id=?1 AND writer_active=1",
            [ticket.mutation_id.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    if active.is_some() {
        return Ok(Err(error(StatusCode::CONFLICT, "writer_active")));
    }
    let previous = transaction
        .query_row(
            "SELECT attempt_id,staging_path,attempt_no,writer_active,bytes_received,outcome FROM upload_attempt WHERE mutation_id=?1 ORDER BY attempt_no DESC LIMIT 1",
            [ticket.mutation_id.as_str()],
            |row| Ok(Attempt {
                attempt_id: row.get(0)?,
                staging_name: row.get(1)?,
                attempt_no: row.get(2)?,
                writer_active: row.get(3)?,
                bytes_received: row.get(4)?,
                outcome: row.get(5)?,
            }),
        )
        .optional()
        ?;
    if previous
        .as_ref()
        .is_some_and(|attempt| attempt.bytes_received > 0)
    {
        return Ok(Err(error(StatusCode::CONFLICT, "upload_requires_resume")));
    }
    let mut attempt = match previous {
        Some(attempt)
            if attempt.writer_active == 0
                && attempt.bytes_received == 0
                && attempt.outcome.is_none() =>
        {
            attempt
        }
        Some(previous) => {
            let attempt_id = match random_id() {
                Ok(id) => id,
                Err(_) => {
                    return Ok(Err(error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "entropy_unavailable",
                    )));
                }
            };
            let blob_id = match random_id() {
                Ok(id) => id,
                Err(_) => {
                    return Ok(Err(error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "entropy_unavailable",
                    )));
                }
            };
            let staging_name = match staging_name(&attempt_id, &blob_id) {
                Ok(name) => name,
                Err(_) => {
                    return Ok(Err(error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "id_generation_failed",
                    )));
                }
            };
            let attempt = Attempt {
                attempt_id,
                staging_name,
                attempt_no: previous.attempt_no + 1,
                writer_active: 0,
                bytes_received: 0,
                outcome: None,
            };
            transaction.execute(
                "INSERT INTO upload_attempt(attempt_id,mutation_id,attempt_no,staging_path,writer_active,bytes_received) VALUES(?1,?2,?3,?4,0,0)",
                params![attempt.attempt_id,ticket.mutation_id,attempt.attempt_no,attempt.staging_name],
            )?;
            attempt
        }
        None => return Ok(Err(error(StatusCode::NOT_FOUND, "upload_not_found"))),
    };
    let changed = transaction
        .execute(
            "UPDATE upload_attempt SET writer_active=1,bytes_received=0 WHERE attempt_id=?1 AND writer_active=0",
            [attempt.attempt_id.as_str()],
        )
        ?;
    if changed != 1 {
        return Ok(Err(error(StatusCode::CONFLICT, "writer_active")));
    }
    attempt.writer_active = 1;
    Ok(Ok(attempt))
}

fn finish_attempt(
    transaction: &rusqlite::Transaction<'_>,
    ticket: &VerifiedUploadTicket,
    attempt: &Attempt,
    outcome: &WorkerOutcome,
    principal: &AdmissionTicket,
    coordinator: &crate::domain::coordinator::MutationCoordinator,
    csrf: &[u8; 32],
) -> rusqlite::Result<Result<(), Response>> {
    if coordinator
        .revalidate_current(transaction, principal, now())
        .is_err()
    {
        return Ok(Err(error(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
        )));
    }
    if !csrf_matches(transaction, principal.session_id(), csrf) {
        return Ok(Err(error(StatusCode::FORBIDDEN, "csrf_required")));
    }
    match outcome {
        WorkerOutcome::Published(published) => {
            let mutation_changed = transaction.execute(
                "UPDATE mutation SET state='publishing',completed_length=?2,completed_sha256=?3 WHERE mutation_id=?1 AND state IN ('admitted','publishing')",
                params![ticket.mutation_id,published.length as i64,published.sha256.as_slice()],
            )?;
            let attempt_changed = transaction.execute(
                "UPDATE upload_attempt SET writer_active=0,bytes_received=?2,completed_length=?2,completed_sha256=?3 WHERE attempt_id=?1 AND writer_active=1",
                params![attempt.attempt_id,published.length as i64,published.sha256.as_slice()],
            )?;
            if mutation_changed != 1 || attempt_changed != 1 {
                // A cancel that committed first leaves a published payload
                // that only this metadata lets the sweep reclaim.
                record_canceled_publication(
                    transaction,
                    &ticket.mutation_id,
                    &attempt.attempt_id,
                    published,
                )?;
                return Ok(Err(error(StatusCode::CONFLICT, "upload_state_changed")));
            }
        }
        WorkerOutcome::Canceled(published) => {
            transaction.execute(
                "UPDATE mutation SET state='rejected' WHERE mutation_id=?1 AND state='admitted'",
                [ticket.mutation_id.as_str()],
            )?;
            transaction.execute(
                "UPDATE upload_attempt SET writer_active=0,outcome='canceled' WHERE attempt_id=?1 AND writer_active=1",
                [attempt.attempt_id.as_str()],
            )?;
            if let Some(published) = published {
                record_canceled_publication(
                    transaction,
                    &ticket.mutation_id,
                    &attempt.attempt_id,
                    published,
                )?;
            }
        }
        WorkerOutcome::Failed(_) => {
            transaction.execute(
                "UPDATE upload_attempt SET writer_active=0,outcome='rejected' WHERE attempt_id=?1 AND writer_active=1",
                [attempt.attempt_id.as_str()],
            )?;
        }
    }
    Ok(Ok(()))
}

/// Record the length and digest of a payload published for an upload that was
/// already rejected, so `CANCELED_PAYLOAD_RULE` can match it. Does nothing
/// unless the mutation is rejected with no result and the attempt is canceled.
fn record_canceled_publication(
    transaction: &rusqlite::Transaction<'_>,
    mutation_id: &str,
    attempt_id: &str,
    published: &PublishedPayload,
) -> rusqlite::Result<()> {
    let length = published.length as i64;
    let digest = published.sha256.as_slice();
    transaction.execute(
        "UPDATE mutation SET completed_length=?2,completed_sha256=?3 WHERE mutation_id=?1 AND state='rejected' AND result_revision_id IS NULL AND completed_length IS NULL AND completed_sha256 IS NULL",
        params![mutation_id, length, digest],
    )?;
    transaction.execute(
        "UPDATE upload_attempt SET writer_active=0,bytes_received=?2,completed_length=?2,completed_sha256=?3 WHERE attempt_id=?1 AND outcome='canceled' AND completed_length IS NULL AND EXISTS(SELECT 1 FROM mutation m WHERE m.mutation_id=upload_attempt.mutation_id AND m.state='rejected' AND m.completed_length=?2 AND m.completed_sha256=?3)",
        params![attempt_id, length, digest],
    )?;
    Ok(())
}

async fn finish_attempt_owned(
    state: &AuthState,
    ticket: &VerifiedUploadTicket,
    attempt: &Attempt,
    outcome: &WorkerOutcome,
    principal: &AdmissionTicket,
    csrf: [u8; 32],
) -> Result<(), Response> {
    let ticket = ticket.clone();
    let attempt = attempt.clone();
    let outcome = outcome.clone();
    let principal = principal.clone();
    let coordinator = state.coordinator.clone();
    match state
        .workers
        .transact(move |tx| {
            finish_attempt(
                tx,
                &ticket,
                &attempt,
                &outcome,
                &principal,
                &coordinator,
                &csrf,
            )
        })
        .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(response)) => Err(response),
        Err(_) => Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_unavailable",
        )),
    }
}

fn worker_error(payload_error: PayloadError) -> Response {
    match payload_error {
        PayloadError::LengthExceeded => error(StatusCode::PAYLOAD_TOO_LARGE, "length_exceeded"),
        PayloadError::LengthMismatch => error(StatusCode::UNPROCESSABLE_ENTITY, "length_mismatch"),
        PayloadError::DigestMismatch => error(StatusCode::UNPROCESSABLE_ENTITY, "digest_mismatch"),
        PayloadError::Collision => error(StatusCode::CONFLICT, "payload_collision"),
        PayloadError::Cancelled => error(StatusCode::CONFLICT, "upload_canceled"),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "payload_publication_failed",
        ),
    }
}

pub async fn content(
    State(state): State<AuthState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if validate_id(&upload_id).is_err() {
        return error(StatusCode::NOT_FOUND, "upload_not_found");
    }
    let storage = match storage(&state) {
        Ok(storage) => storage,
        Err(response) => return response,
    };
    let principal = match authorize_mutation(&state, &headers).await {
        Ok(authorized) => authorized,
        Err(response) => return response,
    };
    let ticket = match verified_ticket(&storage, &headers, &principal) {
        Ok(ticket) => ticket,
        Err(response) => return response,
    };
    if ticket.mutation_id != upload_id {
        return error(StatusCode::NOT_FOUND, "upload_not_found");
    }
    let Some(csrf_secret) = one_header(&headers, "x-csrf-token").and_then(decode_secret) else {
        return error(StatusCode::FORBIDDEN, "csrf_required");
    };
    if let Some(length) = one_header(&headers, CONTENT_LENGTH.as_str())
        && length.parse::<u64>().ok() != Some(ticket.admission.length)
    {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "length_mismatch");
    }
    if one_header(&headers, CONTENT_TYPE.as_str())
        .is_some_and(|value| value != "application/octet-stream")
    {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "octet_stream_required");
    }
    let body = if ticket.admission.text_revision().is_some() {
        match tokio::time::timeout(
            storage.chunk_idle_timeout(),
            to_bytes(body, MAX_TEXT_EDIT_LENGTH as usize),
        )
        .await
        {
            Ok(Ok(bytes)) if bytes.len() as u64 != ticket.admission.length => {
                return error(StatusCode::UNPROCESSABLE_ENTITY, "length_mismatch");
            }
            Ok(Ok(bytes)) if is_editable_text(&bytes) => Body::from(bytes),
            Ok(Ok(_)) => return error(StatusCode::UNPROCESSABLE_ENTITY, "text_edit_encoding"),
            Ok(Err(_)) => return error(StatusCode::BAD_REQUEST, "text_edit_body_invalid"),
            Err(_) => return error(StatusCode::REQUEST_TIMEOUT, "chunk_idle_timeout"),
        }
    } else {
        body
    };
    let declared_digest = match ticket.admission.declared_digest() {
        Ok(digest) => digest,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_sha256"),
    };
    let writer = match storage.register_writer(&upload_id) {
        Ok(writer) => writer,
        Err(WorkerError::AlreadyActive) => return error(StatusCode::CONFLICT, "writer_active"),
        Err(WorkerError::Unavailable) => return ChunkFailure::Stopping.response(),
    };
    let registration = WriterRegistration {
        storage: Arc::clone(&storage),
        upload_id: upload_id.clone(),
        writer: Arc::clone(&writer),
    };
    let ticket_for_activation = ticket.clone();
    let principal_for_activation = principal.clone();
    let coordinator_for_activation = state.coordinator.clone();
    let attempt = match state
        .workers
        .transact(move |tx| {
            activate_attempt(
                tx,
                &ticket_for_activation,
                &principal_for_activation,
                &coordinator_for_activation,
            )
        })
        .await
    {
        Ok(Ok(attempt)) => attempt,
        Ok(Err(response)) => return response,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"),
    };
    if writer.is_cancelled() {
        let result = finish_attempt_owned(
            &state,
            &ticket,
            &attempt,
            &WorkerOutcome::Canceled(None),
            &principal,
            csrf_secret,
        )
        .await;
        drop(registration);
        return match result {
            Ok(()) => error(StatusCode::CONFLICT, "upload_canceled"),
            Err(response) => response,
        };
    }
    let staging = match storage
        .payloads
        .start(&attempt.staging_name, ticket.admission.length)
    {
        Ok(staging) => staging,
        Err(payload_error) => {
            let outcome = WorkerOutcome::Failed(payload_error);
            let result =
                finish_attempt_owned(&state, &ticket, &attempt, &outcome, &principal, csrf_secret)
                    .await;
            drop(registration);
            return match result {
                Ok(()) => worker_error(payload_error),
                Err(response) => response,
            };
        }
    };
    let (sender, receiver) = mpsc::channel(2);
    let worker_writer = Arc::clone(&writer);
    let declared_length = ticket.admission.length;
    let worker = tokio::task::spawn_blocking(move || {
        run_worker(
            staging,
            receiver,
            worker_writer,
            declared_length,
            declared_digest,
        )
    });
    let mut body = body;
    let mut cancellation = writer.cancellation();
    let mut sent = 0u64;
    loop {
        let next = tokio::select! {
            frame = body.frame() => frame,
            _ = cancellation.changed() => None,
        };
        let Some(frame) = next else {
            if !writer.is_cancelled() {
                let message = if sent == declared_length {
                    WorkerMessage::Finish
                } else {
                    WorkerMessage::Abort(PayloadError::LengthMismatch)
                };
                let _ = sender.send(message).await;
            }
            break;
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(_) => {
                let _ = sender
                    .send(WorkerMessage::Abort(PayloadError::LengthMismatch))
                    .await;
                break;
            }
        };
        let Ok(data) = frame.into_data() else {
            continue;
        };
        for chunk in data.chunks(64 * 1024) {
            sent = sent.saturating_add(chunk.len() as u64);
            if sent > declared_length {
                let _ = sender
                    .send(WorkerMessage::Abort(PayloadError::LengthExceeded))
                    .await;
                break;
            }
            if sender
                .send(WorkerMessage::Data(chunk.to_vec()))
                .await
                .is_err()
            {
                break;
            }
        }
        if writer.is_cancelled() || sent > declared_length || sender.is_closed() {
            break;
        }
    }
    drop(sender);
    let outcome = match worker.await {
        Ok(outcome) => outcome,
        Err(_) => WorkerOutcome::Failed(PayloadError::Io("payload_worker_failed")),
    };
    #[cfg(feature = "test-harness")]
    if let Some(barrier) = &state.upload_cancel_test_barrier {
        barrier.pause_publish().await;
    }
    let outcome = if writer.is_cancelled() {
        match outcome {
            WorkerOutcome::Published(published) => WorkerOutcome::Canceled(Some(published)),
            _ => WorkerOutcome::Canceled(None),
        }
    } else {
        outcome
    };
    let result =
        finish_attempt_owned(&state, &ticket, &attempt, &outcome, &principal, csrf_secret).await;
    drop(registration);
    if let Err(response) = result {
        return response;
    }
    match outcome {
        WorkerOutcome::Published(published) => finish(
            (
                StatusCode::OK,
                Json(json!({
                    "upload_id": upload_id,
                    "mutation_id": upload_id,
                    "status": "staged",
                    "length": published.length,
                    "sha256": hex(&published.sha256),
                    "published": false,
                })),
            )
                .into_response(),
        ),
        WorkerOutcome::Canceled(_) => error(StatusCode::CONFLICT, "upload_canceled"),
        WorkerOutcome::Failed(payload_error) => worker_error(payload_error),
    }
}

pub async fn commit(
    State(state): State<AuthState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CommitRequest>,
) -> Response {
    if validate_id(&upload_id).is_err() {
        return error(StatusCode::NOT_FOUND, "upload_not_found");
    }
    let storage = match storage(&state) {
        Ok(storage) => storage,
        Err(response) => return response,
    };
    let principal = match authorize_mutation(&state, &headers).await {
        Ok(authorized) => authorized,
        Err(response) => return response,
    };
    let ticket = match verified_ticket(&storage, &headers, &principal) {
        Ok(ticket) => ticket,
        Err(response) => return response,
    };
    if let Err(response) = require_upload_precondition(&headers, &ticket.admission) {
        return response;
    }
    if ticket.mutation_id != upload_id || request.parent_etag != ticket.admission.parent_etag {
        return error(StatusCode::CONFLICT, "commit_fingerprint_mismatch");
    }
    let Some(csrf_secret) = one_header(&headers, "x-csrf-token").and_then(decode_secret) else {
        return error(StatusCode::FORBIDDEN, "csrf_required");
    };
    let completed_digest = match parse_digest(&request.sha256) {
        Ok(digest) => digest,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_sha256"),
    };
    let name_key = match ticket.admission.name_key() {
        Ok(name_key) => name_key,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_name"),
    };
    let entry_id = match random_id() {
        Ok(id) => id,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "entropy_unavailable"),
    };
    let revision_id = match random_id() {
        Ok(id) => id,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "entropy_unavailable"),
    };
    #[cfg(feature = "test-harness")]
    if let Some(barrier) = &state.upload_commit_test_barrier {
        barrier.pause().await;
    }
    let committed_at = now();
    let principal_for_tx = principal.clone();
    let coordinator = state.coordinator.clone();
    let ticket_for_tx = ticket.clone();
    let entry_id_for_tx = entry_id.clone();
    let revision_id_for_tx = revision_id.clone();
    let name_key_for_tx = name_key.clone();
    let result = state.workers.transact_result(move |transaction| -> Result<(i64, String), Response> {
            coordinator
                .revalidate_current(transaction, &principal_for_tx, committed_at)
                .map_err(|_| error(StatusCode::UNAUTHORIZED, "authentication_required"))?;
            if !csrf_matches(transaction, principal_for_tx.session_id(), &csrf_secret) {
                return Err(error(StatusCode::FORBIDDEN, "csrf_required"));
            }
            if recovery_epoch(transaction).map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "installation_unavailable"))? != ticket_for_tx.recovery_epoch {
                return Err(error(StatusCode::UNAUTHORIZED, "stale_mutation_ticket"));
            }
            let staged: Option<(Vec<u8>, i64)> = transaction.query_row(
                "SELECT completed_sha256,completed_length FROM mutation WHERE mutation_id=?1 AND principal_account_id=?2 AND state='publishing' AND NOT EXISTS(SELECT 1 FROM upload_attempt a WHERE a.mutation_id=mutation.mutation_id AND a.writer_active=1) AND EXISTS(SELECT 1 FROM upload_attempt a WHERE a.mutation_id=mutation.mutation_id AND a.completed_sha256=mutation.completed_sha256 AND a.completed_length=mutation.completed_length AND a.writer_active=0 AND a.outcome IS NULL)",
                params![ticket_for_tx.mutation_id,ticket_for_tx.account_id],
                |row| Ok((row.get(0)?,row.get(1)?)),
            ).optional().map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "upload_lookup_failed"))?;
            let Some((stored_digest, stored_length)) = staged else {
                return Err(error(StatusCode::CONFLICT, "upload_not_staged"));
            };
            if stored_digest.as_slice() != completed_digest
                || stored_length < 0
                || stored_length as u64 != ticket_for_tx.admission.length
            {
                return Err(error(StatusCode::CONFLICT, "commit_fingerprint_mismatch"));
            }
            check_parent(transaction, &ticket_for_tx)?;
            check_destination(transaction, &ticket_for_tx.account_id, &ticket_for_tx.admission, &name_key_for_tx)?;
            let staging_path: String = transaction.query_row(
                "SELECT staging_path FROM upload_attempt WHERE mutation_id=?1 AND completed_sha256=?2 AND completed_length=?3 ORDER BY attempt_no DESC LIMIT 1",
                params![ticket_for_tx.mutation_id,stored_digest,stored_length],
                |row| row.get(0),
            ).optional().map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "attempt_lookup_failed"))?
                .ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "published_attempt_missing"))?;
            let blob_id = blob_id_from_staging(&staging_path).map_err(|_| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "published_attempt_invalid",
                )
            })?;
            transaction.execute(
                "INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,substr(?1,1,2),?2,?3,'live')",
                params![blob_id,stored_length,stored_digest],
            ).map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "blob_commit_failed"))?;
            if let Some(replacement) = &ticket_for_tx.admission.replacement {
                let changed = transaction.execute(
                    "UPDATE entry SET state='deleted',generation=generation+1 WHERE entry_id=?1 AND owner_account_id=?2 AND state='active'",
                    params![replacement.entry_id,ticket_for_tx.account_id],
                ).map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "replacement_commit_failed"))?;
                if changed != 1 {
                    return Err(error(StatusCode::PRECONDITION_FAILED, "replacement_changed"));
                }
            }
            transaction.execute(
                "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,current_revision_id,generation) VALUES(?1,?2,?3,'file',?4,?5,1,NULL,1)",
                params![entry_id_for_tx,ticket_for_tx.account_id,ticket_for_tx.admission.parent_id,ticket_for_tx.admission.name,name_key_for_tx],
            ).map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "entry_commit_failed"))?;
            transaction.execute(
                "INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![revision_id_for_tx,entry_id_for_tx,blob_id,stored_length,stored_digest,committed_at,ticket_for_tx.account_id],
            ).map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "revision_commit_failed"))?;
            let updated = transaction
                .execute(
                    "UPDATE entry SET current_revision_id=?2 WHERE entry_id=?1",
                    params![entry_id_for_tx, revision_id_for_tx],
                )
                .map_err(|_| {
                    error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "entry_revision_commit_failed",
                    )
                })?;
            if updated != 1 {
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "entry_revision_commit_failed",
                ));
            }
            transaction
                .execute(
                    "UPDATE entry SET generation=generation+1 WHERE entry_id=?1",
                    [ticket_for_tx.admission.parent_id.as_str()],
                )
                .map_err(|_| {
                    error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "parent_generation_commit_failed",
                    )
                })?;
            transaction.execute(
                "UPDATE mutation SET state='committed',result_revision_id=?2 WHERE mutation_id=?1 AND state='publishing'",
                params![ticket_for_tx.mutation_id,revision_id_for_tx],
            ).map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "mutation_result_commit_failed"))?;
            Ok((stored_length, blob_id.to_owned()))
        }).await;
    let (length, _blob_id) = match result {
        Ok(Ok(result)) => result,
        Ok(Err(response)) => return response,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"),
    };
    let etag = revision_etag(&revision_id);
    let entry = json!({
        "id": entry_id,
        "entry_id": entry_id,
        "parent_id": ticket.admission.parent_id,
        "name": ticket.admission.name,
        "kind": "file",
        "owner_id": ticket.account_id,
        "size": length,
        "length": length,
        "revision_id": revision_id,
        "etag": etag,
        "sha256": request.sha256,
        "upload_id": upload_id,
        "mutation_id": upload_id,
        "status": "committed",
        "replaced_entry_id": ticket.admission.replacement.as_ref().map(|replacement| &replacement.entry_id),
    });
    let mut response = finish((StatusCode::CREATED, Json(entry)).into_response());
    if let (Ok(location), Ok(etag_header)) = (
        HeaderValue::from_str(&format!("/entries/{entry_id}")),
        HeaderValue::from_str(&etag),
    ) {
        response.headers_mut().insert(LOCATION, location);
        response.headers_mut().insert(ETAG, etag_header);
    }
    response
}

pub async fn status(
    State(state): State<AuthState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if validate_id(&upload_id).is_err() {
        return error(StatusCode::NOT_FOUND, "upload_not_found");
    }
    let Some(cookie) = session_cookie(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "authentication_required");
    };
    let session_id = cookie.session_id;
    let secret = cookie.secret;
    let upload_id_for_query = upload_id.clone();
    let result = state.workers.read(move |db| {
        let Some((account_id, _, _)) = crate::domain::authz::validate_session(db, &session_id, &secret, now()) else {
            return Ok(Err(StatusCode::UNAUTHORIZED));
        };
        let row = db.query_row(
            "SELECT mutation.state,mutation.completed_length,mutation.completed_sha256,mutation.result_revision_id,upload_attempt.bytes_received,upload_attempt.writer_active,upload_attempt.outcome FROM mutation LEFT JOIN upload_attempt ON upload_attempt.attempt_id=(SELECT attempt_id FROM upload_attempt WHERE mutation_id=?1 ORDER BY attempt_no DESC LIMIT 1) WHERE mutation.mutation_id=?1 AND mutation.principal_account_id=?2",
            params![upload_id_for_query,account_id],
            |row| Ok((
                row.get::<_,String>(0)?,
                row.get::<_,Option<i64>>(1)?,
                row.get::<_,Option<Vec<u8>>>(2)?,
                row.get::<_,Option<String>>(3)?,
                row.get::<_,Option<i64>>(4)?,
                row.get::<_,Option<i64>>(5)?,
                row.get::<_,Option<String>>(6)?,
            )),
        ).optional()?;
        Ok(Ok(row))
    }).await;
    let row = match result {
        Ok(Ok(row)) => row,
        Ok(Err(status)) => return error(status, "authentication_required"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "upload_lookup_failed"),
    };
    // Read after SQL: a writer still registered in memory would refuse the next PATCH.
    let registered = storage(&state).is_ok_and(|storage| storage.writer_registered(&upload_id));
    let Some((state, completed_length, digest, revision_id, bytes, active, outcome)) = row else {
        return error(StatusCode::NOT_FOUND, "upload_not_found");
    };
    let active = if registered { Some(1) } else { active };
    let status = match (state.as_str(), outcome.as_deref(), active) {
        ("committed", _, _) => "committed",
        ("publishing", Some("rejected"), _) => "recovery_required",
        ("publishing", _, Some(1)) => "uploading",
        ("publishing", _, _) => "staged",
        ("rejected", Some("canceled"), _) => "canceled",
        ("rejected", _, _) => "rejected",
        ("admitted", _, Some(1)) => "uploading",
        _ => "admitted",
    };
    let length = completed_length.or(bytes).unwrap_or(0);
    finish(
        Json(json!({
            "upload_id": upload_id,
            "mutation_id": upload_id,
            "status": status,
            "state": state,
            "bytes_received": bytes.unwrap_or(0),
            "length": length,
            "sha256": digest.map(|digest| hex(&digest)),
            "revision_id": revision_id,
            "writer_active": active == Some(1),
        }))
        .into_response(),
    )
}

pub async fn cancel(
    State(state): State<AuthState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if validate_id(&upload_id).is_err() {
        return error(StatusCode::NOT_FOUND, "upload_not_found");
    }
    let storage = match storage(&state) {
        Ok(storage) => storage,
        Err(response) => return response,
    };
    let principal = match authorize_mutation(&state, &headers).await {
        Ok(authorized) => authorized,
        Err(response) => return response,
    };
    let Some(csrf_secret) = one_header(&headers, "x-csrf-token").and_then(decode_secret) else {
        return error(StatusCode::FORBIDDEN, "csrf_required");
    };
    let account_id = principal.account_id().to_owned();
    let upload_id_for_lookup = upload_id.clone();
    let owned = state
        .workers
        .read(move |db| {
            db.query_row(
                "SELECT 1 FROM mutation WHERE mutation_id=?1 AND principal_account_id=?2",
                params![upload_id_for_lookup, account_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
        })
        .await;
    match owned {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "upload_not_found"),
        _ => return error(StatusCode::INTERNAL_SERVER_ERROR, "upload_lookup_failed"),
    }
    let active = match storage.cancel_writer(&upload_id) {
        Ok(active) => active,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cancellation_unavailable",
            );
        }
    };
    let done = active.as_ref().map(|writer| writer.subscribe());
    let principal_for_tx = principal.clone();
    let coordinator = state.coordinator.clone();
    let upload_id_for_tx = upload_id.clone();
    let result = state.workers.transact_result(move |transaction| -> Result<bool, Response> {
        coordinator.revalidate_current(transaction, &principal_for_tx, now())
            .map_err(|_| error(StatusCode::UNAUTHORIZED, "authentication_required"))?;
        if !csrf_matches(transaction, principal_for_tx.session_id(), &csrf_secret) {
            return Err(error(StatusCode::FORBIDDEN, "csrf_required"));
        }
        let state: Option<String> = transaction.query_row(
            "SELECT state FROM mutation WHERE mutation_id=?1 AND principal_account_id=?2",
            params![upload_id_for_tx, principal_for_tx.account_id()], |row| row.get(0)
        ).optional().map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "upload_lookup_failed"))?;
        let Some(state) = state else { return Err(error(StatusCode::NOT_FOUND, "upload_not_found")); };
        if state == "committed" { return Ok(true); }
        transaction.execute(
            "UPDATE mutation SET state='rejected' WHERE mutation_id=?1 AND principal_account_id=?2 AND state IN ('admitted','publishing')",
            params![upload_id_for_tx,principal_for_tx.account_id()],
        ).map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "cancellation_commit_failed"))?;
        transaction.execute(
            "UPDATE upload_attempt SET writer_active=0,outcome='canceled' WHERE mutation_id=?1 AND outcome IS NULL",
            [upload_id_for_tx.as_str()],
        ).map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "cancellation_commit_failed"))?;
        Ok(false)
    }).await;
    #[cfg(not(feature = "test-harness"))]
    let _ = &active;
    #[cfg(feature = "test-harness")]
    if active.is_none()
        && let Some(barrier) = &state.upload_cancel_test_barrier
    {
        barrier.pause_cancel().await;
    }
    if let Some(mut done) = done
        && done.wait_for(|finished| *finished).await.is_err()
    {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "worker_join_failed");
    }
    match result {
        Ok(Ok(true)) => {
            let mut response = error(StatusCode::CONFLICT, "upload_already_committed");
            if let Ok(location) = HeaderValue::from_str(&format!("/uploads/{upload_id}")) {
                response.headers_mut().insert(LOCATION, location);
            }
            response
        }
        Ok(Ok(false)) => finish(StatusCode::NO_CONTENT.into_response()),
        Ok(Err(response)) => response,
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"),
    }
}

/// Storage owns the request task through SQL checkpointing even if its client
/// disconnects, and joins it at shutdown.
pub async fn append(
    State(state): State<AuthState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let storage = match storage(&state) {
        Ok(storage) => storage,
        Err(response) => return response,
    };
    let Ok(task) = storage.spawn_owned(append_owned(state, upload_id, headers, body)) else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "server_stopping");
    };
    match task.await {
        Ok(response) => response,
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "upload_worker_failed"),
    }
}

enum AppendMessage {
    Data(axum::body::Bytes),
    Finish,
}

enum ChunkFailure {
    Canceled,
    Stopping,
    Idle,
    Invalid,
    WriteFailed,
}

impl ChunkFailure {
    fn response(self) -> Response {
        match self {
            Self::Canceled => error(StatusCode::CONFLICT, "upload_canceled"),
            Self::Stopping => error(StatusCode::SERVICE_UNAVAILABLE, "server_stopping"),
            Self::Idle => error(StatusCode::REQUEST_TIMEOUT, "chunk_idle_timeout"),
            Self::Invalid => error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "chunk_incomplete_or_checksum_mismatch",
            ),
            Self::WriteFailed => error(StatusCode::INTERNAL_SERVER_ERROR, "chunk_write_failed"),
        }
    }
}

/// Stream one chunk into staging with fixed buffering. The worker is joined
/// before returning, so no write outlives the claimed writer.
#[allow(clippy::too_many_arguments)]
async fn receive_chunk(
    storage: &Arc<UploadStorage>,
    writer: &ActiveUpload,
    staging_name: String,
    offset: u64,
    length: u64,
    total: u64,
    digest: [u8; 32],
    mut body: Body,
) -> Result<u64, ChunkFailure> {
    let (sender, mut receiver) = mpsc::channel(2);
    let store = Arc::clone(storage);
    let worker = tokio::task::spawn_blocking(move || -> Result<u64, PayloadError> {
        let mut append = store
            .payloads
            .open_append(&staging_name, offset, total, length)?;
        loop {
            match receiver.blocking_recv() {
                Some(AppendMessage::Data(bytes)) => append.write(&bytes)?,
                Some(AppendMessage::Finish) => return append.commit(digest),
                None => return Err(PayloadError::LengthMismatch),
            }
        }
    });
    let idle = storage.chunk_idle_timeout();
    let mut cancellation = writer.cancellation();
    let mut stopping = storage.stopping();
    let mut received = 0u64;
    let ended = loop {
        if writer.is_cancelled() {
            break Err(ChunkFailure::Canceled);
        }
        if storage.is_stopping() {
            break Err(ChunkFailure::Stopping);
        }
        let frame = tokio::select! {
            frame = tokio::time::timeout(idle, body.frame()) => frame,
            _ = cancellation.changed() => break Err(ChunkFailure::Canceled),
            _ = stopping.changed() => break Err(ChunkFailure::Stopping),
        };
        let data = match frame {
            Err(_) => break Err(ChunkFailure::Idle),
            Ok(None) if received == length => break Ok(()),
            Ok(None) | Ok(Some(Err(_))) => break Err(ChunkFailure::Invalid),
            Ok(Some(Ok(frame))) => match frame.into_data() {
                Ok(data) => data,
                Err(_) => continue,
            },
        };
        if data.len() as u64 > length - received {
            break Err(ChunkFailure::Invalid);
        }
        received += data.len() as u64;
        let mut sent = true;
        for start in (0..data.len()).step_by(64 * 1024) {
            let piece = data.slice(start..data.len().min(start + 64 * 1024));
            if sender.send(AppendMessage::Data(piece)).await.is_err() {
                sent = false;
                break;
            }
        }
        if !sent {
            break Err(ChunkFailure::WriteFailed);
        }
    };
    if ended.is_ok() {
        let _ = sender.send(AppendMessage::Finish).await;
    }
    drop(sender);
    let written = worker.await;
    ended?;
    match written {
        Ok(Ok(next)) => Ok(next),
        Ok(Err(PayloadError::DigestMismatch | PayloadError::LengthMismatch)) => {
            Err(ChunkFailure::Invalid)
        }
        _ => Err(ChunkFailure::WriteFailed),
    }
}

async fn append_owned(
    state: AuthState,
    upload_id: String,
    headers: HeaderMap,
    body: Body,
) -> Response {
    use crate::domain::upload::MAX_CHUNK_LENGTH;
    if validate_id(&upload_id).is_err() {
        return error(StatusCode::NOT_FOUND, "upload_not_found");
    }
    let storage = match storage(&state) {
        Ok(storage) => storage,
        Err(response) => return response,
    };
    let principal = match authorize_mutation(&state, &headers).await {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let ticket = match verified_ticket(&storage, &headers, &principal) {
        Ok(ticket) => ticket,
        Err(response) => return response,
    };
    if ticket.mutation_id != upload_id {
        return error(StatusCode::NOT_FOUND, "upload_not_found");
    }
    if ticket.admission.text_revision().is_some() {
        return error(StatusCode::BAD_REQUEST, "text_edit_requires_full_content");
    }
    let Some(csrf) = one_header(&headers, "x-csrf-token").and_then(decode_secret) else {
        return error(StatusCode::FORBIDDEN, "csrf_required");
    };
    let Some(offset) =
        one_header(&headers, "upload-offset").and_then(|value| value.parse::<u64>().ok())
    else {
        return error(StatusCode::BAD_REQUEST, "upload_offset_required");
    };
    let Some(length) =
        one_header(&headers, "content-length").and_then(|value| value.parse::<u64>().ok())
    else {
        return error(StatusCode::LENGTH_REQUIRED, "chunk_length_required");
    };
    if length > MAX_CHUNK_LENGTH {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "chunk_too_large");
    }
    if offset > ticket.admission.length
        || length > ticket.admission.length - offset
        || (length == 0 && offset != ticket.admission.length)
    {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "length_mismatch");
    }
    if one_header(&headers, "content-type") != Some("application/octet-stream") {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "octet_stream_required");
    }
    let Some(chunk_digest) =
        one_header(&headers, "upload-checksum").and_then(|value| parse_digest(value).ok())
    else {
        return error(StatusCode::BAD_REQUEST, "chunk_checksum_required");
    };
    let writer = match storage.register_writer(&upload_id) {
        Ok(writer) => writer,
        Err(WorkerError::AlreadyActive) => return error(StatusCode::CONFLICT, "writer_active"),
        Err(WorkerError::Unavailable) => return ChunkFailure::Stopping.response(),
    };
    let _registration = WriterRegistration {
        storage: Arc::clone(&storage),
        upload_id: upload_id.clone(),
        writer: Arc::clone(&writer),
    };
    let t = ticket.clone();
    let p = principal.clone();
    let coordinator = state.coordinator.clone();
    let attempt = match state.workers.transact(move |tx| -> rusqlite::Result<Result<Attempt, Response>> {
        if coordinator.revalidate_current(tx, &p, now()).is_err() { return Ok(Err(error(StatusCode::UNAUTHORIZED, "authentication_required"))); }
        if recovery_epoch(tx).map_err(|_| rusqlite::Error::InvalidQuery)? != t.recovery_epoch { return Ok(Err(error(StatusCode::UNAUTHORIZED, "stale_mutation_ticket"))); }
        let row: Option<Attempt> = tx.query_row(
            "SELECT a.attempt_id,a.staging_path,a.attempt_no,a.writer_active,a.bytes_received,a.outcome FROM upload_attempt a JOIN mutation m USING(mutation_id) WHERE m.mutation_id=?1 AND m.principal_account_id=?2 AND m.state='admitted' AND m.expires_at>?3 ORDER BY a.attempt_no DESC LIMIT 1",
            params![t.mutation_id,t.account_id,now()], |row| Ok(Attempt { attempt_id: row.get(0)?, staging_name: row.get(1)?, attempt_no: row.get(2)?, writer_active: row.get(3)?, bytes_received: row.get(4)?, outcome: row.get(5)? })
        ).optional()?;
        let Some(attempt) = row else { return Ok(Err(error(StatusCode::CONFLICT, "upload_not_staging"))); };
        if attempt.writer_active != 0 { return Ok(Err(error(StatusCode::CONFLICT, "writer_active"))); }
        if attempt.bytes_received < 0 || attempt.bytes_received as u64 != offset || attempt.outcome.is_some() {
            return Ok(Err(error(StatusCode::CONFLICT, "upload_offset_mismatch")));
        }
        tx.execute("UPDATE upload_attempt SET writer_active=1 WHERE attempt_id=?1 AND writer_active=0", [&attempt.attempt_id])?;
        Ok(Ok(attempt))
    }).await {
        Ok(Ok(attempt)) => attempt, Ok(Err(response)) => return response,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"),
    };
    // Memory is bounded per request regardless of chunk size; no partial chunk is acknowledged.
    let total = ticket.admission.length;
    let next_offset = match receive_chunk(
        &storage,
        &writer,
        attempt.staging_name.clone(),
        offset,
        length,
        total,
        chunk_digest,
        body,
    )
    .await
    {
        Ok(next) => next,
        Err(failure) => {
            if let Err(response) = release_append(&state, &attempt.attempt_id).await {
                return response;
            }
            return failure.response();
        }
    };
    let id = attempt.attempt_id.clone();
    let t = ticket.clone();
    let p = principal.clone();
    let coordinator = state.coordinator.clone();
    let checkpoint = state.workers.transact(move |tx| {
        if coordinator.revalidate_current(tx, &p, now()).is_err() || !csrf_matches(tx, p.session_id(), &csrf) { return Ok(false); }
        Ok(tx.execute("UPDATE upload_attempt SET bytes_received=?2,writer_active=?3 WHERE attempt_id=?1 AND writer_active=1 AND outcome IS NULL AND EXISTS(SELECT 1 FROM mutation WHERE mutation_id=?4 AND state='admitted')",
            params![id,next_offset as i64,i64::from(next_offset == total),t.mutation_id])? == 1)
    }).await;
    if !matches!(checkpoint, Ok(true)) {
        if let Err(response) = release_append(&state, &attempt.attempt_id).await {
            return response;
        }
        if writer.is_cancelled() {
            return ChunkFailure::Canceled.response();
        }
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "chunk_checkpoint_unconfirmed",
        );
    }
    if next_offset != total {
        return finish(Json(json!({"upload_id":upload_id,"mutation_id":upload_id,"status":"admitted","bytes_received":next_offset,"length":total,"published":false})).into_response());
    }
    let store = Arc::clone(&storage);
    let name = attempt.staging_name.clone();
    let expected = ticket.admission.declared_digest().ok().flatten();
    let hashing_writer = writer.clone();
    let staging = match tokio::task::spawn_blocking(move || {
        store.payloads.prepare_partial(&name, total, expected, || {
            hashing_writer.is_cancelled() || store.is_stopping()
        })
    })
    .await
    {
        Ok(Ok(staging)) => staging,
        Ok(Err(payload_error)) => {
            if payload_error == PayloadError::DigestMismatch {
                let mutation = upload_id.clone();
                match state.workers.transact(move |tx| {
                    tx.execute("UPDATE mutation SET state='rejected' WHERE mutation_id=?1 AND state='admitted'", [&mutation])?;
                    tx.execute("UPDATE upload_attempt SET writer_active=0,outcome='rejected' WHERE mutation_id=?1", [mutation])?;
                    Ok(())
                }).await {
                    Ok(()) => {},
                    Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "rejection_unconfirmed"),
                }
            } else {
                if let Err(response) = release_append(&state, &attempt.attempt_id).await {
                    return response;
                }
                if payload_error == PayloadError::Cancelled && storage.is_stopping() {
                    return ChunkFailure::Stopping.response();
                }
            }
            return worker_error(payload_error);
        }
        Err(_) => {
            if let Err(response) = release_append(&state, &attempt.attempt_id).await {
                return response;
            }
            return error(StatusCode::INTERNAL_SERVER_ERROR, "payload_worker_failed");
        }
    };
    let metadata = staging.summary();
    let id = attempt.attempt_id.clone();
    let mutation = upload_id.clone();
    let p = principal.clone();
    let coordinator = state.coordinator.clone();
    let prepared = state.workers.transact(move |tx| {
        if coordinator.revalidate_current(tx, &p, now()).is_err() || !csrf_matches(tx, p.session_id(), &csrf) { return Ok(false); }
        let changed = tx.execute("UPDATE mutation SET state='publishing',completed_length=?2,completed_sha256=?3 WHERE mutation_id=?1 AND state='admitted'", params![mutation, metadata.length as i64, metadata.sha256.as_slice()])?;
        if changed != 1 { return Ok(false); }
        tx.execute("UPDATE upload_attempt SET completed_length=?2,completed_sha256=?3 WHERE attempt_id=?1 AND writer_active=1", params![id, metadata.length as i64, metadata.sha256.as_slice()])?;
        Ok(true)
    }).await;
    if !matches!(prepared, Ok(true)) {
        if let Err(response) = release_append(&state, &attempt.attempt_id).await {
            return response;
        }
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "publication_preparation_unconfirmed",
        );
    }
    let digest = staging.summary().sha256;
    let outcome = match tokio::task::spawn_blocking(move || staging.publish(digest)).await {
        Ok(Ok(published)) => WorkerOutcome::Published(published),
        Ok(Err(error)) => WorkerOutcome::Failed(error),
        Err(_) => WorkerOutcome::Failed(PayloadError::Io("payload_worker_failed")),
    };
    if let Err(response) =
        finish_attempt_owned(&state, &ticket, &attempt, &outcome, &principal, csrf).await
    {
        if let Err(release_error) = release_append(&state, &attempt.attempt_id).await {
            return release_error;
        }
        return response;
    }
    match outcome {
        WorkerOutcome::Published(published) => finish(Json(json!({"upload_id":upload_id,"mutation_id":upload_id,"status":"staged","bytes_received":total,"length":total,"sha256":hex(&published.sha256),"published":false})).into_response()),
        WorkerOutcome::Failed(error) => worker_error(error),
        WorkerOutcome::Canceled(_) => error(StatusCode::CONFLICT, "upload_canceled"),
    }
}

async fn release_append(state: &AuthState, attempt_id: &str) -> Result<(), Response> {
    let id = attempt_id.to_owned();
    state
        .workers
        .transact(move |tx| {
            tx.execute(
                "UPDATE upload_attempt SET writer_active=0,outcome=CASE WHEN EXISTS(SELECT 1 FROM mutation m WHERE m.mutation_id=upload_attempt.mutation_id AND m.state='publishing') THEN 'rejected' ELSE outcome END WHERE attempt_id=?1",
                [id],
            )?;
            Ok(())
        })
        .await
        .map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "upload_checkpoint_unavailable",
            )
        })
}
