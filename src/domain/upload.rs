//! Upload admission, signed ticket and active-writer primitives.

use crate::persistence::payload::{PayloadError, PayloadStore};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use rustix::fs::{self, FileType, Mode, OFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;
use unicode_normalization::UnicodeNormalization;

pub const UPLOAD_SCOPE: &str = "upload_create";
pub const MAX_UPLOAD_LENGTH: u64 = 64 * 1024 * 1024 * 1024;
pub const MAX_CHUNK_LENGTH: u64 = 8 * 1024 * 1024;
/// A chunk body that sends nothing for this long releases its writer.
pub const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Absolute lifetime from issuance; acknowledged progress does not extend it.
pub const TICKET_SECONDS: i64 = 24 * 60 * 60;
const MAX_TICKET_LENGTH: usize = 4096;

#[derive(Debug)]
pub enum StorageOpenError {
    Payload(PayloadError),
    MutationKey,
    InstanceSettings,
}

impl From<PayloadError> for StorageOpenError {
    fn from(error: PayloadError) -> Self {
        Self::Payload(error)
    }
}

/// Shared storage plus in-process worker rendezvous. Durable writer exclusion
/// is still backed by `upload_attempt_one_active`; this map only propagates
/// cancellation and lets DELETE join an active worker.
pub struct UploadStorage {
    pub payloads: Arc<PayloadStore>,
    mutation_key: [u8; 32],
    active: Mutex<HashMap<String, Arc<ActiveUpload>>>,
    stop: watch::Sender<bool>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    chunk_idle_timeout: Duration,
}

pub struct ActiveUpload {
    cancelled: AtomicBool,
    cancel_notice: watch::Sender<()>,
    done: watch::Sender<bool>,
}

impl ActiveUpload {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.done.subscribe()
    }

    pub fn cancellation(&self) -> watch::Receiver<()> {
        self.cancel_notice.subscribe()
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.cancel_notice.send_replace(());
    }

    fn finish(&self) {
        self.done.send_replace(true);
    }
}

#[derive(Debug)]
pub enum WorkerError {
    AlreadyActive,
    Unavailable,
}

impl UploadStorage {
    pub fn open(root: &Path) -> Result<Self, StorageOpenError> {
        let mutation_key = read_mutation_key(root)?;
        let payloads = PayloadStore::open(root)?;
        Ok(Self {
            payloads: Arc::new(payloads),
            mutation_key,
            active: Mutex::new(HashMap::new()),
            stop: watch::channel(false).0,
            tasks: Mutex::new(Vec::new()),
            chunk_idle_timeout: CHUNK_IDLE_TIMEOUT,
        })
    }

    pub(crate) fn removal_signing_key(&self) -> [u8; 32] {
        self.mutation_key
    }

    pub fn register_writer(&self, upload_id: &str) -> Result<Arc<ActiveUpload>, WorkerError> {
        let mut active = self.active.lock().map_err(|_| WorkerError::Unavailable)?;
        if self.is_stopping() {
            return Err(WorkerError::Unavailable);
        }
        if active.contains_key(upload_id) {
            return Err(WorkerError::AlreadyActive);
        }
        let (done, _) = watch::channel(false);
        let (cancel_notice, _) = watch::channel(());
        let writer = Arc::new(ActiveUpload {
            cancelled: AtomicBool::new(false),
            cancel_notice,
            done,
        });
        active.insert(upload_id.to_owned(), Arc::clone(&writer));
        Ok(writer)
    }

    pub fn chunk_idle_timeout(&self) -> Duration {
        self.chunk_idle_timeout
    }

    #[cfg(feature = "test-harness")]
    pub fn set_chunk_idle_timeout(&mut self, timeout: Duration) {
        self.chunk_idle_timeout = timeout;
    }

    pub fn is_stopping(&self) -> bool {
        *self.stop.borrow()
    }

    /// Resolves once shutdown starts. Stopping releases a writer; it is not a user cancel.
    pub fn stopping(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    /// Run a request's storage work to completion even if its HTTP future is
    /// dropped; `shutdown` joins it before the database stops.
    pub fn spawn_owned<T: Send + 'static>(
        &self,
        future: impl std::future::Future<Output = T> + Send + 'static,
    ) -> Result<tokio::sync::oneshot::Receiver<T>, WorkerError> {
        let mut tasks = self.tasks.lock().map_err(|_| WorkerError::Unavailable)?;
        if self.is_stopping() {
            return Err(WorkerError::Unavailable);
        }
        tasks.retain(|task| !task.is_finished());
        let (send, receive) = tokio::sync::oneshot::channel();
        tasks.push(tokio::spawn(async move {
            let _ = send.send(future.await);
        }));
        Ok(receive)
    }

    /// Refuse new owned work, signal running work to release, then join it.
    pub async fn shutdown(&self) -> Result<(), WorkerError> {
        let tasks = {
            let mut tasks = self.tasks.lock().map_err(|_| WorkerError::Unavailable)?;
            self.stop.send_replace(true);
            std::mem::take(&mut *tasks)
        };
        let mut failed = false;
        for task in tasks {
            failed |= task.await.is_err();
        }
        if failed {
            Err(WorkerError::Unavailable)
        } else {
            Ok(())
        }
    }

    pub fn cancel_writer(&self, upload_id: &str) -> Result<Option<Arc<ActiveUpload>>, WorkerError> {
        let active = self.active.lock().map_err(|_| WorkerError::Unavailable)?;
        let writer = active.get(upload_id).cloned();
        if let Some(writer) = &writer {
            writer.cancel();
        }
        Ok(writer)
    }

    /// SQL releases the claim before the registration drops; status must cover both.
    pub fn writer_registered(&self, upload_id: &str) -> bool {
        self.active
            .lock()
            .map_or(true, |active| active.contains_key(upload_id))
    }

    pub fn finish_writer(&self, upload_id: &str, writer: &Arc<ActiveUpload>) {
        writer.finish();
        if let Ok(mut active) = self.active.lock()
            && active
                .get(upload_id)
                .is_some_and(|current| Arc::ptr_eq(current, writer))
        {
            active.remove(upload_id);
        }
    }
}

impl Drop for UploadStorage {
    fn drop(&mut self) {
        self.mutation_key.fill(0);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    pub entry_id: String,
    pub etag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UploadAdmission {
    pub parent_id: String,
    pub parent_etag: String,
    pub name: String,
    pub length: u64,
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<Replacement>,
}

impl UploadAdmission {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_id(&self.parent_id)?;
        if self.parent_etag
            != folder_etag(
                &self.parent_id,
                generation_from_etag(&self.parent_etag, &self.parent_id)?,
            )
        {
            return Err("invalid_parent_etag");
        }
        if let Some(replacement) = &self.replacement {
            validate_id(&replacement.entry_id)?;
            generation_from_etag(&replacement.etag, &replacement.entry_id)?;
        }
        normalized_name(&self.name)?;
        if self.length > MAX_UPLOAD_LENGTH {
            return Err("upload_too_large");
        }
        if let Some(digest) = &self.sha256 {
            parse_digest(digest)?;
        }
        Ok(())
    }

    pub fn name_key(&self) -> Result<Vec<u8>, &'static str> {
        normalized_name(&self.name)
    }

    pub fn declared_digest(&self) -> Result<Option<[u8; 32]>, &'static str> {
        self.sha256.as_deref().map(parse_digest).transpose()
    }
}

#[derive(Debug, Clone)]
pub struct IssuedTicket {
    pub mutation_id: String,
    pub ticket: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct VerifiedUploadTicket {
    pub mutation_id: String,
    pub account_id: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub recovery_epoch: i64,
    pub admission: UploadAdmission,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketPayload {
    version: u8,
    mutation_id: String,
    account_id: String,
    scope: String,
    issued_at: i64,
    expires_at: i64,
    recovery_epoch: i64,
    admission: UploadAdmission,
}

pub fn issue_upload_ticket(
    storage: &UploadStorage,
    account_id: &str,
    recovery_epoch: i64,
    admission: &UploadAdmission,
    now: i64,
) -> Result<IssuedTicket, &'static str> {
    validate_id(account_id)?;
    admission.validate()?;
    if recovery_epoch < 0 || now < 0 {
        return Err("invalid_ticket_time");
    }
    let expires_at = now
        .checked_add(TICKET_SECONDS)
        .ok_or("invalid_ticket_time")?;
    let mutation_id = random_id()?;
    let payload = TicketPayload {
        version: 1,
        mutation_id: mutation_id.clone(),
        account_id: account_id.to_owned(),
        scope: UPLOAD_SCOPE.to_owned(),
        issued_at: now,
        expires_at,
        recovery_epoch,
        admission: admission.clone(),
    };
    let encoded =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).map_err(|_| "ticket_encoding_failed")?);
    let signature = sign(&storage.mutation_key, encoded.as_bytes())?;
    let ticket = format!("v1.{encoded}.{}", URL_SAFE_NO_PAD.encode(signature));
    if ticket.len() > MAX_TICKET_LENGTH {
        return Err("ticket_too_large");
    }
    Ok(IssuedTicket {
        mutation_id,
        ticket,
        expires_at,
    })
}

pub fn verify_upload_ticket(
    storage: &UploadStorage,
    ticket: &str,
    account_id: &str,
    now: i64,
) -> Result<VerifiedUploadTicket, &'static str> {
    if ticket.len() > MAX_TICKET_LENGTH {
        return Err("invalid_mutation_ticket");
    }
    let mut parts = ticket.split('.');
    let (Some("v1"), Some(encoded), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("invalid_mutation_ticket");
    };
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| "invalid_mutation_ticket")?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&storage.mutation_key)
        .map_err(|_| "ticket_key_unavailable")?;
    mac.update(encoded.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| "invalid_mutation_ticket")?;
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "invalid_mutation_ticket")?;
    if decoded.len() > 3072 {
        return Err("invalid_mutation_ticket");
    }
    let payload: TicketPayload =
        serde_json::from_slice(&decoded).map_err(|_| "invalid_mutation_ticket")?;
    if payload.version != 1
        || payload.scope != UPLOAD_SCOPE
        || payload.account_id != account_id
        || payload.expires_at <= now
        || payload.issued_at < 0
        || payload.expires_at < payload.issued_at
        || payload.recovery_epoch < 0
    {
        return Err("invalid_mutation_ticket");
    }
    validate_id(&payload.mutation_id)?;
    payload.admission.validate()?;
    Ok(VerifiedUploadTicket {
        mutation_id: payload.mutation_id,
        account_id: payload.account_id,
        issued_at: payload.issued_at,
        expires_at: payload.expires_at,
        recovery_epoch: payload.recovery_epoch,
        admission: payload.admission,
    })
}

pub fn admission_fingerprint(ticket: &VerifiedUploadTicket) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for field in [
        b"poc-upload-admission-v1".as_slice(),
        ticket.account_id.as_bytes(),
        ticket.mutation_id.as_bytes(),
        ticket.admission.parent_id.as_bytes(),
        ticket.admission.parent_etag.as_bytes(),
        ticket.admission.name.as_bytes(),
        &ticket.admission.length.to_be_bytes(),
        ticket.admission.sha256.as_deref().unwrap_or("").as_bytes(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    if let Some(replacement) = &ticket.admission.replacement {
        for field in [replacement.entry_id.as_bytes(), replacement.etag.as_bytes()] {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field);
        }
    }
    hasher.finalize().into()
}

pub fn staging_name(attempt_id: &str, blob_id: &str) -> Result<String, &'static str> {
    validate_id(attempt_id)?;
    validate_id(blob_id)?;
    Ok(format!("{attempt_id}-{blob_id}"))
}

pub fn blob_id_from_staging(staging_name: &str) -> Result<&str, &'static str> {
    if staging_name.len() != 65 || staging_name.as_bytes()[32] != b'-' {
        return Err("invalid_staging_name");
    }
    let blob_id = &staging_name[33..];
    validate_id(blob_id)?;
    Ok(blob_id)
}

pub fn random_id() -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "entropy_unavailable")?;
    Ok(hex(&bytes))
}

pub fn validate_id(id: &str) -> Result<(), &'static str> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid_id");
    }
    Ok(())
}

pub fn folder_etag(id: &str, generation: u64) -> String {
    format!("\"{id}:{generation}\"")
}

pub fn revision_etag(revision_id: &str) -> String {
    format!("\"{revision_id}\"")
}

pub fn generation_from_etag(etag: &str, id: &str) -> Result<u64, &'static str> {
    let prefix = format!("\"{id}:");
    let generation = etag
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix('"'))
        .ok_or("invalid_parent_etag")?;
    if generation.is_empty() || (generation.len() > 1 && generation.starts_with('0')) {
        return Err("invalid_parent_etag");
    }
    generation.parse().map_err(|_| "invalid_parent_etag")
}

pub fn parse_digest(value: &str) -> Result<[u8; 32], &'static str> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid_sha256");
    }
    let mut digest = [0u8; 32];
    for (index, output) in digest.iter_mut().enumerate() {
        *output = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid_sha256")?;
    }
    Ok(digest)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalized_name(name: &str) -> Result<Vec<u8>, &'static str> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name
            .bytes()
            .any(|byte| byte == 0 || byte == b'/' || byte == b'\\')
        || name.chars().any(char::is_control)
    {
        return Err("invalid_name");
    }
    let normalized = name.nfc().collect::<String>();
    if normalized.is_empty() || normalized.len() > 255 {
        return Err("invalid_name");
    }
    Ok(normalized.into_bytes())
}

fn sign(key: &[u8; 32], payload: &[u8]) -> Result<[u8; 32], &'static str> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| "ticket_key_unavailable")?;
    mac.update(payload);
    Ok(mac.finalize().into_bytes().into())
}

fn read_mutation_key(root: &Path) -> Result<[u8; 32], StorageOpenError> {
    let root_fd = fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| StorageOpenError::MutationKey)?;
    let secrets = fs::openat(
        &root_fd,
        "secrets",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| StorageOpenError::MutationKey)?;
    let fd = fs::openat(
        &secrets,
        "mutation.key",
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| StorageOpenError::MutationKey)?;
    let stat = fs::fstat(&fd).map_err(|_| StorageOpenError::MutationKey)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_nlink != 1
        || stat.st_mode & 0o777 != 0o600
        || stat.st_size != 32
    {
        return Err(StorageOpenError::MutationKey);
    }
    let mut key = [0u8; 32];
    File::from(fd)
        .read_exact(&mut key)
        .map_err(|_| StorageOpenError::MutationKey)?;
    Ok(key)
}

/// Staging path recorded once an expired partial's unlink is durable, so later
/// sweeps skip it. It is not a valid staging name and never names a file.
pub const CLEANED_PARTIAL: &str = "partial-cleaned";

/// One bounded sweep; rejection commits before private bytes are unlinked.
pub async fn cleanup_expired_partials(
    workers: &crate::persistence::sqlite::SqliteWorkers,
    storage: Arc<UploadStorage>,
    after: &mut String,
    at: i64,
) -> Result<usize, &'static str> {
    use rusqlite::params;
    let cursor = after.clone();
    let rows = workers.transact(move |tx| {
        let rows = {
            let mut statement = tx.prepare(
                "SELECT a.attempt_id,a.staging_path,a.mutation_id FROM upload_attempt a JOIN mutation m USING(mutation_id) WHERE a.attempt_id>?1 AND m.scope=?3 AND m.completed_length IS NULL AND m.completed_sha256 IS NULL AND NOT EXISTS(SELECT 1 FROM upload_attempt active WHERE active.mutation_id=m.mutation_id AND active.writer_active=1) AND a.staging_path<>?4 AND a.writer_active=0 AND a.completed_length IS NULL AND a.completed_sha256 IS NULL AND m.result_revision_id IS NULL AND m.state IN ('admitted','rejected') AND m.expires_at<=?2 ORDER BY a.attempt_id LIMIT 32"
            )?;
            statement.query_map(params![cursor,at,UPLOAD_SCOPE,CLEANED_PARTIAL], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (_,_,mutation) in &rows {
            tx.execute("UPDATE mutation SET state='rejected' WHERE mutation_id=?1 AND state='admitted'", [mutation])?;
        }
        Ok(rows)
    }).await.map_err(|_| "partial_cleanup_database_unavailable")?;
    let count = rows.len();
    let next = rows.last().map(|row| row.0.clone()).unwrap_or_default();
    let (cleaned, failed) = tokio::task::spawn_blocking(move || {
        let mut cleaned = Vec::new();
        let mut failed = false;
        for (id, name, _) in rows {
            if storage.payloads.remove_partial(&name).is_ok() {
                cleaned.push((id, name));
            } else {
                failed = true;
            }
        }
        (cleaned, failed)
    })
    .await
    .map_err(|_| "partial_cleanup_worker_failed")?;
    if !cleaned.is_empty() {
        workers.transact(move |tx| {
            for (id, name) in cleaned {
                tx.execute("UPDATE upload_attempt SET staging_path=?3 WHERE attempt_id=?1 AND staging_path=?2 AND writer_active=0 AND completed_length IS NULL AND completed_sha256 IS NULL AND EXISTS(SELECT 1 FROM mutation m WHERE m.mutation_id=upload_attempt.mutation_id AND m.state='rejected' AND m.result_revision_id IS NULL AND m.completed_length IS NULL AND m.completed_sha256 IS NULL)", params![id,name,CLEANED_PARTIAL])?;
            }
            Ok(())
        }).await.map_err(|_| "partial_cleanup_database_unavailable")?;
    }
    *after = next;
    if failed {
        return Err("partial_cleanup_failed");
    }
    Ok(count)
}

/// Staging path recorded once a canceled upload's final payload unlink is
/// durable. Like `CLEANED_PARTIAL`, it never names a file.
pub const RECLAIMED_PAYLOAD: &str = "payload-reclaimed";

/// One bounded pass that unlinks final payloads authorized by
/// `CanceledPayload`, after their rejection committed. Returns how many were
/// reclaimed.
pub async fn reclaim_canceled_payloads(
    workers: &crate::persistence::sqlite::SqliteWorkers,
    storage: Arc<UploadStorage>,
    after: &mut String,
) -> Result<usize, &'static str> {
    use crate::persistence::payload::{CanceledPayload, CanceledUnlink};
    let cursor = after.clone();
    let page = workers
        .read(move |db| CanceledPayload::page(db, &cursor, 32))
        .await
        .map_err(|_| "payload_reclaim_database_unavailable")?;
    let next = page
        .last()
        .map(|payload| payload.attempt_id().to_owned())
        .unwrap_or_default();
    let (removed, failed) = tokio::task::spawn_blocking(move || {
        let mut removed = Vec::new();
        let mut failed = false;
        for payload in page {
            // A writer registered before the rejection may still be publishing.
            // One registered after it cannot activate, so this check is stable.
            if storage.writer_registered(payload.mutation_id()) {
                continue;
            }
            let unlinked = storage.payloads.unlink_canceled(&payload);
            #[cfg(feature = "test-harness")]
            let unlinked = unlinked.and_then(|outcome| {
                storage
                    .payloads
                    .cleanup_boundary("cleanup.before_finalize")
                    .map(|()| outcome)
            });
            match unlinked {
                Ok(CanceledUnlink::Removed) => removed.push(payload),
                Ok(CanceledUnlink::Unpublished) => {}
                Err(_) => failed = true,
            }
        }
        (removed, failed)
    })
    .await
    .map_err(|_| "payload_reclaim_worker_failed")?;
    let count = removed.len();
    if count != 0 {
        workers
            .transact(move |tx| {
                for payload in removed {
                    // The rule cannot stop holding after rejection; fail loudly if it did.
                    if !payload.still_authorized(tx)? {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    tx.execute(
                        "UPDATE upload_attempt SET staging_path=?3 WHERE attempt_id=?1 AND staging_path=?2",
                        rusqlite::params![payload.attempt_id(), payload.staging_name(), RECLAIMED_PAYLOAD],
                    )?;
                }
                Ok(())
            })
            .await
            .map_err(|_| "payload_reclaim_database_unavailable")?;
    }
    *after = next;
    if failed {
        return Err("payload_reclaim_failed");
    }
    Ok(count)
}

/// The periodic sweep of expired partials and canceled final payloads. Its
/// first pass runs at start.
pub struct PartialCleanup {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl PartialCleanup {
    pub fn start(
        workers: crate::persistence::sqlite::SqliteWorkers,
        storage: Arc<UploadStorage>,
        period: Duration,
    ) -> Self {
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut cursor = String::new();
            let mut reclaim_cursor = String::new();
            let mut interval = tokio::time::interval(period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = stopped.changed() => break,
                    _ = interval.tick() => {}
                }
                // A started pass is never dropped: its blocking unlinks must be joined.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .min(i64::MAX as u64) as i64;
                if cleanup_expired_partials(&workers, Arc::clone(&storage), &mut cursor, now)
                    .await
                    .is_err()
                {
                    eprintln!("partial upload cleanup failed; retrying next pass");
                }
                if reclaim_canceled_payloads(&workers, Arc::clone(&storage), &mut reclaim_cursor)
                    .await
                    .is_err()
                {
                    eprintln!("canceled upload payload cleanup failed; retrying next pass");
                }
            }
        });
        Self { stop, task }
    }

    pub async fn shutdown(self) -> Result<(), WorkerError> {
        self.stop.send_replace(true);
        self.task.await.map_err(|_| WorkerError::Unavailable)
    }
}
