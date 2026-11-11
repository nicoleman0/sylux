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
};
use tokio::sync::watch;
use unicode_normalization::UnicodeNormalization;

pub const UPLOAD_SCOPE: &str = "upload_create";
pub const MAX_UPLOAD_LENGTH: u64 = 1024 * 1024 * 1024;
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
    pub payloads: PayloadStore,
    mutation_key: [u8; 32],
    active: Mutex<HashMap<String, Arc<ActiveUpload>>>,
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
            payloads,
            mutation_key,
            active: Mutex::new(HashMap::new()),
        })
    }

    pub fn register_writer(&self, upload_id: &str) -> Result<Arc<ActiveUpload>, WorkerError> {
        let mut active = self.active.lock().map_err(|_| WorkerError::Unavailable)?;
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

    pub fn cancel_writer(&self, upload_id: &str) -> Result<Option<Arc<ActiveUpload>>, WorkerError> {
        let active = self.active.lock().map_err(|_| WorkerError::Unavailable)?;
        let writer = active.get(upload_id).cloned();
        if let Some(writer) = &writer {
            writer.cancel();
        }
        Ok(writer)
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
