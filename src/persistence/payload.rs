//! Private resumable staging and create-only final payload publication.

use rusqlite::{Connection, params};
use rustix::fs::{self, FileType, Mode, OFlags, RenameFlags};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(feature = "test-harness")]
use std::sync::Arc;

/// Test-harness callback for deterministic faults at publication boundaries.
/// Returning an error aborts publication without changing the filesystem.
#[cfg(feature = "test-harness")]
pub type PublishFaultHook = Arc<dyn Fn(&'static str) -> Result<(), PayloadError> + Send + Sync>;

const PAYLOADS: &str = "payloads";
const STAGING: &str = "staging";
const ID_LENGTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadError {
    InvalidId,
    LengthExceeded,
    LengthMismatch,
    DigestMismatch,
    Collision,
    Missing,
    Integrity,
    Cancelled,
    Io(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPayload {
    pub blob_id: String,
    pub length: u64,
    pub sha256: [u8; 32],
}

pub struct VerifiedPayload {
    pub file: File,
    pub length: u64,
}

/// Validated payload directory descriptors.
pub struct PayloadStore {
    staging: File,
    payloads: File,
    #[cfg(feature = "test-harness")]
    publish_fault_hook: Option<PublishFaultHook>,
}

impl PayloadStore {
    /// Open or create the two private payload directories. Creation is synced
    /// through the root before any staged byte can be acknowledged.
    pub fn open(root: &Path) -> Result<Self, PayloadError> {
        let root_fd = fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| PayloadError::Io("payload_root_open_failed"))?;
        validate_directory(&root_fd, "payload_root_invalid")?;
        let root_fd = File::from(root_fd);
        let staging = open_or_create_directory(&root_fd, STAGING)?;
        let payloads = open_or_create_directory(&root_fd, PAYLOADS)?;
        Ok(Self {
            staging,
            payloads,
            #[cfg(feature = "test-harness")]
            publish_fault_hook: None,
        })
    }

    /// Set a per-store hook for test-harness publication seam faults.
    #[cfg(feature = "test-harness")]
    pub fn set_publish_fault_hook(&mut self, hook: Option<PublishFaultHook>) {
        self.publish_fault_hook = hook;
    }

    /// Exclusively create a server-named staging file.
    pub fn start(
        &self,
        staging_name: &str,
        declared_length: u64,
    ) -> Result<StagingPayload, PayloadError> {
        let (_, blob_id) = staging_ids(staging_name)?;
        let fd = fs::openat(
            &self.staging,
            staging_name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| match std::io::Error::from(error).kind() {
            std::io::ErrorKind::AlreadyExists => PayloadError::Collision,
            _ => PayloadError::Io("staging_create_failed"),
        })?;
        Ok(StagingPayload {
            file: File::from(fd),
            staging_name: staging_name.to_owned(),
            blob_id: blob_id.to_owned(),
            declared_length,
            written: 0,
            hasher: Sha256::new(),
            staging_directory: self
                .staging
                .try_clone()
                .map_err(|_| PayloadError::Io("staging_directory_clone_failed"))?,
            payloads_directory: self
                .payloads
                .try_clone()
                .map_err(|_| PayloadError::Io("payload_directory_clone_failed"))?,
            #[cfg(feature = "test-harness")]
            publish_fault_hook: self.publish_fault_hook.clone(),
        })
    }

    /// The caller must hold durable removal authorization and exclude retained references.
    pub(crate) fn unlink_authorized(&self, blob_id: &str) -> Result<(), PayloadError> {
        self.unlink_final(blob_id, None)
    }

    /// Unlink a canceled upload's final payload. A body still in staging never
    /// finished publication and is left in place.
    pub(crate) fn unlink_canceled(
        &self,
        payload: &CanceledPayload,
    ) -> Result<CanceledUnlink, PayloadError> {
        staging_ids(&payload.staging_name)?;
        match fs::statat(
            &self.staging,
            payload.staging_name.as_str(),
            fs::AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => return Ok(CanceledUnlink::Unpublished),
            Err(rustix::io::Errno::NOENT) => {}
            Err(_) => return Err(PayloadError::Io("staging_stat_failed")),
        }
        self.unlink_final(&payload.blob_id, Some(payload.length))?;
        Ok(CanceledUnlink::Removed)
    }

    /// Durably unlink one private final payload; an absent file is already done.
    fn unlink_final(&self, blob_id: &str, length: Option<u64>) -> Result<(), PayloadError> {
        #[cfg(feature = "test-harness")]
        self.cleanup_boundary("cleanup.before_unlink")?;
        validate_id(blob_id)?;
        let shard = match open_directory(&self.payloads, &blob_id[..2]) {
            Ok(shard) => shard,
            Err(PayloadError::Missing) => {
                return fs::fsync(&self.payloads)
                    .map_err(|_| PayloadError::Io("payload_directory_sync_failed"));
            }
            Err(error) => return Err(error),
        };
        match fs::statat(&shard, blob_id, fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => {
                if !FileType::from_raw_mode(stat.st_mode).is_file()
                    || stat.st_nlink != 1
                    || stat.st_mode & 0o777 != 0o600
                    || length.is_some_and(|length| u64::try_from(stat.st_size).ok() != Some(length))
                {
                    return Err(PayloadError::Integrity);
                }
                fs::unlinkat(&shard, blob_id, fs::AtFlags::empty())
                    .map_err(|_| PayloadError::Io("payload_unlink_failed"))?;
            }
            Err(error) if std::io::Error::from(error).kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(PayloadError::Io("payload_stat_failed")),
        }
        #[cfg(feature = "test-harness")]
        self.cleanup_boundary("cleanup.before_directory_sync")?;
        fs::fsync(&shard).map_err(|_| PayloadError::Io("payload_directory_sync_failed"))
    }

    #[cfg(feature = "test-harness")]
    #[inline(never)]
    pub(crate) fn cleanup_boundary(&self, boundary: &'static str) -> Result<(), PayloadError> {
        if let Some(hook) = &self.publish_fault_hook {
            hook(boundary)?;
        }
        Ok(())
    }

    /// Reopen only the acknowledged prefix. A lost request may have left a tail.
    pub fn open_append(
        &self,
        staging_name: &str,
        offset: u64,
        declared_length: u64,
        chunk_length: u64,
    ) -> Result<PartialAppend, PayloadError> {
        staging_ids(staging_name)?;
        if offset > declared_length || chunk_length > declared_length - offset {
            return Err(PayloadError::LengthExceeded);
        }
        if offset == 0 {
            match self.start(staging_name, declared_length) {
                Ok(staging) => drop(staging),
                Err(PayloadError::Collision) => {}
                Err(error) => return Err(error),
            }
        }
        let fd = fs::openat(
            &self.staging,
            staging_name,
            OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| PayloadError::Missing)?;
        let mut file = File::from(fd);
        let stat = fs::fstat(&file).map_err(|_| PayloadError::Integrity)?;
        if !FileType::from_raw_mode(stat.st_mode).is_file()
            || stat.st_nlink != 1
            || stat.st_mode & 0o777 != 0o600
            || stat.st_size < 0
            || (stat.st_size as u64) < offset
        {
            return Err(PayloadError::Integrity);
        }
        file.set_len(offset)
            .map_err(|_| PayloadError::Io("staging_truncate_failed"))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| PayloadError::Io("staging_seek_failed"))?;
        // Once a nonzero prefix is acknowledged, the entry was already synced.
        let directory = if offset == 0 {
            Some(
                self.staging
                    .try_clone()
                    .map_err(|_| PayloadError::Io("staging_directory_clone_failed"))?,
            )
        } else {
            None
        };
        Ok(PartialAppend {
            file,
            offset,
            remaining: chunk_length,
            written: 0,
            hasher: Sha256::new(),
            directory,
        })
    }

    pub fn append_at(
        &self,
        staging_name: &str,
        offset: u64,
        declared_length: u64,
        bytes: &[u8],
    ) -> Result<u64, PayloadError> {
        let mut append =
            self.open_append(staging_name, offset, declared_length, bytes.len() as u64)?;
        append.write(bytes)?;
        append.commit(Sha256::digest(bytes).into())
    }

    /// Hash the complete acknowledged file once, with a fixed buffer.
    pub fn prepare_partial(
        &self,
        staging_name: &str,
        length: u64,
        expected: Option<[u8; 32]>,
        cancelled: impl Fn() -> bool,
    ) -> Result<StagingPayload, PayloadError> {
        let (_, blob_id) = staging_ids(staging_name)?;
        let fd = fs::openat(
            &self.staging,
            staging_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| PayloadError::Missing)?;
        let mut file = File::from(fd);
        let stat = fs::fstat(&file).map_err(|_| PayloadError::Integrity)?;
        if !FileType::from_raw_mode(stat.st_mode).is_file()
            || stat.st_nlink != 1
            || stat.st_mode & 0o777 != 0o600
            || stat.st_size < 0
            || stat.st_size as u64 != length
        {
            return Err(PayloadError::Integrity);
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            if cancelled() {
                return Err(PayloadError::Cancelled);
            }
            let read = file
                .read(&mut buffer)
                .map_err(|_| PayloadError::Io("staging_read_failed"))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let digest: [u8; 32] = hasher.clone().finalize().into();
        if expected.is_some_and(|expected| expected != digest) {
            return Err(PayloadError::DigestMismatch);
        }
        Ok(StagingPayload {
            file,
            staging_name: staging_name.to_owned(),
            blob_id: blob_id.to_owned(),
            declared_length: length,
            written: length,
            hasher,
            staging_directory: self
                .staging
                .try_clone()
                .map_err(|_| PayloadError::Io("staging_directory_clone_failed"))?,
            payloads_directory: self
                .payloads
                .try_clone()
                .map_err(|_| PayloadError::Io("payload_directory_clone_failed"))?,
            #[cfg(feature = "test-harness")]
            publish_fault_hook: self.publish_fault_hook.clone(),
        })
    }

    /// Delete only a recorded expired partial, never a final payload.
    pub fn remove_partial(&self, staging_name: &str) -> Result<(), PayloadError> {
        staging_ids(staging_name)?;
        let fd = match fs::openat(
            &self.staging,
            staging_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            // A prior unlink may not have been synced before its marker was written.
            Err(rustix::io::Errno::NOENT) => {
                return fs::fsync(&self.staging)
                    .map_err(|_| PayloadError::Io("staging_directory_sync_failed"));
            }
            Err(_) => return Err(PayloadError::Integrity),
        };
        let stat = fs::fstat(&fd).map_err(|_| PayloadError::Integrity)?;
        if !FileType::from_raw_mode(stat.st_mode).is_file()
            || stat.st_nlink != 1
            || stat.st_mode & 0o777 != 0o600
        {
            return Err(PayloadError::Integrity);
        }
        match fs::unlinkat(&self.staging, staging_name, fs::AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => fs::fsync(&self.staging)
                .map_err(|_| PayloadError::Io("staging_directory_sync_failed")),
            Err(_) => Err(PayloadError::Io("staging_cleanup_failed")),
        }
    }

    /// Verify a referenced blob without following links before exposing it.
    pub fn open_verified(
        &self,
        blob_id: &str,
        expected_length: u64,
        expected_sha256: [u8; 32],
    ) -> Result<VerifiedPayload, PayloadError> {
        self.open_verified_unless(
            blob_id,
            expected_length,
            expected_sha256,
            &AtomicBool::new(false),
        )
    }

    /// `open_verified` that stops between reads once `cancelled` is set.
    pub fn open_verified_unless(
        &self,
        blob_id: &str,
        expected_length: u64,
        expected_sha256: [u8; 32],
        cancelled: &AtomicBool,
    ) -> Result<VerifiedPayload, PayloadError> {
        let mut file = check_payload(
            &self.payloads,
            blob_id,
            expected_length,
            expected_sha256,
            || {
                if cancelled.load(Ordering::Relaxed) {
                    Err(PayloadError::Cancelled)
                } else {
                    Ok(())
                }
            },
        )?;

        file.seek(SeekFrom::Start(0))
            .map_err(|_| PayloadError::Io("payload_seek_failed"))?;
        Ok(VerifiedPayload {
            file,
            length: expected_length,
        })
    }
}

/// Deletion rule for the final payload of an upload canceled before commit.
/// Cancel commits `rejected`, which is terminal, and only a `publishing`
/// mutation can insert a blob row, so a payload matching every clause has no
/// reference and can never gain one. Recovery-required attempts keep outcome
/// `rejected` and committed or uncertain mutations are not `rejected`.
const CANCELED_PAYLOAD_RULE: &str = "FROM upload_attempt a JOIN mutation m ON m.mutation_id=a.mutation_id \
    WHERE m.state='rejected' AND m.result_revision_id IS NULL \
    AND a.outcome='canceled' AND a.writer_active=0 \
    AND a.completed_length=m.completed_length AND a.completed_sha256=m.completed_sha256 \
    AND length(a.staging_path)=65 AND substr(a.staging_path,1,33)=a.attempt_id||'-' \
    AND NOT EXISTS(SELECT 1 FROM upload_attempt w WHERE w.mutation_id=m.mutation_id AND w.writer_active=1) \
    AND NOT EXISTS(SELECT 1 FROM blob b WHERE b.blob_id=substr(a.staging_path,34))";

/// Authority to unlink one canceled upload's final payload. Only
/// `CanceledPayload::page` creates it, from rows matching the deletion rule.
#[derive(Debug)]
pub(crate) struct CanceledPayload {
    attempt_id: String,
    mutation_id: String,
    staging_name: String,
    blob_id: String,
    length: u64,
}

#[derive(Debug, Default)]
pub(crate) struct CanceledPage {
    pub payloads: Vec<CanceledPayload>,
    pub malformed: usize,
    pub last: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanceledUnlink {
    Removed,
    /// The body is still in staging, so there is no final payload to remove.
    Unpublished,
}

impl CanceledPayload {
    /// One keyset page of attempts whose final payload the rule authorizes.
    /// A row whose path or length cannot be trusted is counted in `malformed`
    /// and never unlinked. `last` is the final row seen, so the cursor moves
    /// past it and later rows are not starved.
    pub fn page(db: &Connection, after: &str, limit: i64) -> rusqlite::Result<CanceledPage> {
        let mut statement = db.prepare(&format!(
            "SELECT a.attempt_id,a.mutation_id,a.staging_path,a.completed_length {CANCELED_PAYLOAD_RULE} AND a.attempt_id>?1 ORDER BY a.attempt_id LIMIT ?2"
        ))?;
        let rows = statement.query_map(params![after, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut page = CanceledPage::default();
        for row in rows {
            let (attempt_id, mutation_id, staging_name, length) = row?;
            page.last = Some(attempt_id.clone());
            // Reconciliation refuses these rows; never guess a path to unlink.
            let (Ok((_, blob_id)), Ok(length)) =
                (staging_ids(&staging_name), u64::try_from(length))
            else {
                page.malformed += 1;
                continue;
            };
            let blob_id = blob_id.to_owned();
            page.payloads.push(Self {
                attempt_id,
                mutation_id,
                staging_name,
                blob_id,
                length,
            });
        }
        Ok(page)
    }

    /// Recheck the rule for this exact attempt and path.
    pub fn still_authorized(&self, db: &Connection) -> rusqlite::Result<bool> {
        db.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 {CANCELED_PAYLOAD_RULE} AND a.attempt_id=?1 AND a.staging_path=?2)"
            ),
            params![self.attempt_id, self.staging_name],
            |row| row.get(0),
        )
    }

    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }

    pub fn mutation_id(&self) -> &str {
        &self.mutation_id
    }

    pub fn staging_name(&self) -> &str {
        &self.staging_name
    }
}

/// Why a stored payload failed verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadFault {
    InvalidId,
    Missing,
    /// Not a single-link regular file with mode 0600.
    NotPrivateFile,
    LengthMismatch,
    DigestMismatch,
    Io(&'static str),
}

impl From<PayloadFault> for PayloadError {
    fn from(fault: PayloadFault) -> Self {
        match fault {
            PayloadFault::InvalidId => Self::InvalidId,
            PayloadFault::Missing => Self::Missing,
            PayloadFault::Io(code) => Self::Io(code),
            PayloadFault::NotPrivateFile
            | PayloadFault::LengthMismatch
            | PayloadFault::DigestMismatch => Self::Integrity,
        }
    }
}

/// Read-only access to the payload directory for offline verification.
/// Unlike `PayloadStore`, it never creates a directory.
pub struct PayloadVerifier {
    payloads: File,
}

impl PayloadVerifier {
    pub fn open(root: &Path) -> Result<Self, PayloadError> {
        let root_fd = fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| PayloadError::Io("payload_root_open_failed"))?;
        validate_directory(&root_fd, "payload_root_invalid")?;
        let payloads = open_directory(&File::from(root_fd), PAYLOADS)?;
        Ok(Self { payloads })
    }

    pub fn verify(
        &self,
        blob_id: &str,
        expected_length: u64,
        expected_sha256: [u8; 32],
    ) -> Result<(), PayloadFault> {
        check_payload(
            &self.payloads,
            blob_id,
            expected_length,
            expected_sha256,
            || Ok(()),
        )
        .map(|_| ())
    }
}

/// The one rule for a valid stored payload: private regular file, recorded
/// length, recorded SHA-256. Hashing streams through a fixed buffer.
fn check_payload<E: From<PayloadFault>>(
    payloads: &File,
    blob_id: &str,
    expected_length: u64,
    expected_sha256: [u8; 32],
    mut check_cancelled: impl FnMut() -> Result<(), E>,
) -> Result<File, E> {
    validate_id(blob_id).map_err(|_| PayloadFault::InvalidId)?;
    let shard = open_directory(payloads, &blob_id[..2]).map_err(fault_from_error)?;
    let fd = fs::openat(
        &shard,
        blob_id,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| match std::io::Error::from(error).kind() {
        std::io::ErrorKind::NotFound => PayloadFault::Missing,
        _ => PayloadFault::Io("payload_open_failed"),
    })?;
    let mut file = File::from(fd);
    let stat = fs::fstat(&file).map_err(|_| PayloadFault::Io("payload_stat_failed"))?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_nlink != 1
        || stat.st_mode & 0o777 != 0o600
        || stat.st_size < 0
    {
        return Err(PayloadFault::NotPrivateFile.into());
    }
    if stat.st_size as u64 != expected_length {
        return Err(PayloadFault::LengthMismatch.into());
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check_cancelled()?;
        let read = file
            .read(&mut buffer)
            .map_err(|_| PayloadFault::Io("payload_read_failed"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual: [u8; 32] = hasher.finalize().into();
    if actual != expected_sha256 {
        return Err(PayloadFault::DigestMismatch.into());
    }
    Ok(file)
}

fn fault_from_error(error: PayloadError) -> PayloadFault {
    match error {
        PayloadError::InvalidId => PayloadFault::InvalidId,
        PayloadError::Missing => PayloadFault::Missing,
        PayloadError::Io(code) => PayloadFault::Io(code),
        _ => PayloadFault::Io("payload_directory_open_failed"),
    }
}

/// One chunk written at the acknowledged offset. Dropping it leaves an unsynced
/// tail that the next `open_append` truncates; only `commit` makes it durable.
pub struct PartialAppend {
    file: File,
    offset: u64,
    remaining: u64,
    written: u64,
    hasher: Sha256,
    directory: Option<File>,
}

impl PartialAppend {
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), PayloadError> {
        if bytes.len() as u64 > self.remaining {
            return Err(PayloadError::LengthExceeded);
        }
        self.file
            .write_all(bytes)
            .map_err(|_| PayloadError::Io("staging_write_failed"))?;
        self.hasher.update(bytes);
        self.remaining -= bytes.len() as u64;
        self.written += bytes.len() as u64;
        Ok(())
    }

    /// Sync only a complete chunk whose digest matches; returns the next offset.
    pub fn commit(self, expected: [u8; 32]) -> Result<u64, PayloadError> {
        if self.remaining != 0 {
            return Err(PayloadError::LengthMismatch);
        }
        if <[u8; 32]>::from(self.hasher.finalize()) != expected {
            return Err(PayloadError::DigestMismatch);
        }
        self.file
            .sync_all()
            .map_err(|_| PayloadError::Io("staging_sync_failed"))?;
        if let Some(directory) = self.directory {
            fs::fsync(&directory).map_err(|_| PayloadError::Io("staging_directory_sync_failed"))?;
        }
        Ok(self.offset + self.written)
    }
}

pub struct StagingPayload {
    file: File,
    staging_name: String,
    blob_id: String,
    declared_length: u64,
    written: u64,
    hasher: Sha256,
    staging_directory: File,
    payloads_directory: File,
    #[cfg(feature = "test-harness")]
    publish_fault_hook: Option<PublishFaultHook>,
}

impl StagingPayload {
    pub fn summary(&self) -> PublishedPayload {
        PublishedPayload {
            blob_id: self.blob_id.clone(),
            length: self.written,
            sha256: self.hasher.clone().finalize().into(),
        }
    }

    #[cfg(feature = "test-harness")]
    fn inject_publish_fault(&self, boundary: &'static str) -> Result<(), PayloadError> {
        if let Some(hook) = &self.publish_fault_hook {
            hook(boundary)?;
        }
        Ok(())
    }

    /// Write a whole bounded chunk or reject it before exceeding the declared
    /// length. `write_all` handles short writes; an I/O failure abandons the
    /// attempt without converting it into a completed payload.
    pub fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), PayloadError> {
        let chunk_length = chunk.len() as u64;
        if chunk_length > self.declared_length.saturating_sub(self.written) {
            return Err(PayloadError::LengthExceeded);
        }
        #[cfg(feature = "test-harness")]
        self.inject_publish_fault("publish.before_write")?;
        self.file
            .write_all(chunk)
            .map_err(|_| PayloadError::Io("payload_write_failed"))?;
        self.written += chunk_length;
        self.hasher.update(chunk);
        Ok(())
    }

    /// Publish the body after checking its length and independently recomputed digest.
    pub fn publish(self, expected_sha256: [u8; 32]) -> Result<PublishedPayload, PayloadError> {
        if self.written != self.declared_length {
            return Err(PayloadError::LengthMismatch);
        }
        let actual: [u8; 32] = self.hasher.clone().finalize().into();
        if actual != expected_sha256 {
            return Err(PayloadError::DigestMismatch);
        }
        #[cfg(feature = "test-harness")]
        self.inject_publish_fault("publish.before_file_sync")?;
        self.file
            .sync_all()
            .map_err(|_| PayloadError::Io("payload_file_sync_failed"))?;
        #[cfg(feature = "test-harness")]
        self.inject_publish_fault("publish.before_staging_dir_sync")?;
        fs::fsync(&self.staging_directory)
            .map_err(|_| PayloadError::Io("staging_directory_sync_failed"))?;

        let shard_name = &self.blob_id[..2];
        let shard_created = create_directory_if_missing(&self.payloads_directory, shard_name)?;
        if shard_created {
            #[cfg(feature = "test-harness")]
            self.inject_publish_fault("publish.before_ancestor_sync")?;
            fs::fsync(&self.payloads_directory)
                .map_err(|_| PayloadError::Io("payload_ancestor_sync_failed"))?;
        }
        let shard = open_directory(&self.payloads_directory, shard_name)?;
        #[cfg(feature = "test-harness")]
        self.inject_publish_fault("publish.before_noreplace")?;
        fs::renameat_with(
            &self.staging_directory,
            &self.staging_name,
            &shard,
            &self.blob_id,
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| match std::io::Error::from(error).kind() {
            std::io::ErrorKind::AlreadyExists => PayloadError::Collision,
            _ => PayloadError::Io("payload_rename_failed"),
        })?;
        #[cfg(feature = "test-harness")]
        self.inject_publish_fault("publish.before_final_dir_sync")?;
        fs::fsync(&shard).map_err(|_| PayloadError::Io("payload_directory_sync_failed"))?;
        if shard_created {
            #[cfg(feature = "test-harness")]
            self.inject_publish_fault("publish.before_ancestor_sync")?;
            fs::fsync(&self.payloads_directory)
                .map_err(|_| PayloadError::Io("payload_ancestor_sync_failed"))?;
        }
        Ok(PublishedPayload {
            blob_id: self.blob_id,
            length: self.written,
            sha256: actual,
        })
    }
}

fn staging_ids(staging_name: &str) -> Result<(&str, &str), PayloadError> {
    if staging_name.len() != ID_LENGTH * 2 + 1 || staging_name.as_bytes()[ID_LENGTH] != b'-' {
        return Err(PayloadError::InvalidId);
    }
    let (attempt, blob) = staging_name.split_at(ID_LENGTH);
    let blob = &blob[1..];
    validate_id(attempt)?;
    validate_id(blob)?;
    Ok((attempt, blob))
}

fn validate_id(id: &str) -> Result<(), PayloadError> {
    if id.len() != ID_LENGTH
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PayloadError::InvalidId);
    }
    Ok(())
}

fn open_or_create_directory(parent: &File, name: &str) -> Result<File, PayloadError> {
    let created = create_directory_if_missing(parent, name)?;
    if created {
        fs::fsync(parent).map_err(|_| PayloadError::Io("payload_ancestor_sync_failed"))?;
    }
    open_directory(parent, name)
}

fn create_directory_if_missing(parent: &File, name: &str) -> Result<bool, PayloadError> {
    match fs::mkdirat(parent, name, Mode::RWXU) {
        Ok(()) => Ok(true),
        Err(error) if std::io::Error::from(error).kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(false)
        }
        Err(_) => Err(PayloadError::Io("payload_directory_create_failed")),
    }
}

fn open_directory(parent: &File, name: &str) -> Result<File, PayloadError> {
    let fd = fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| match std::io::Error::from(error).kind() {
        std::io::ErrorKind::NotFound => PayloadError::Missing,
        _ => PayloadError::Io("payload_directory_open_failed"),
    })?;
    validate_directory(&fd, "payload_directory_invalid")?;
    Ok(File::from(fd))
}

fn validate_directory(
    directory: &impl std::os::fd::AsFd,
    code: &'static str,
) -> Result<(), PayloadError> {
    let stat = fs::fstat(directory).map_err(|_| PayloadError::Io(code))?;
    if !FileType::from_raw_mode(stat.st_mode).is_dir() || stat.st_mode & 0o777 != 0o700 {
        return Err(PayloadError::Io(code));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PayloadError, PayloadStore, PublishedPayload};
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::{fs, io::Read, os::unix::fs::DirBuilderExt, path::PathBuf};

    fn root() -> PathBuf {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let root = std::env::temp_dir().join(format!("sylux-payload-{suffix}"));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        root
    }

    #[test]
    fn publishes_exact_bytes_and_never_replaces_a_blob() {
        let root = root();
        let store = PayloadStore::open(&root).unwrap();
        let attempt = "00000000000000000000000000000001";
        let blob = "10000000000000000000000000000001";
        let staging_name = format!("{attempt}-{blob}");
        let bytes = b"\0binary\xffpayload".repeat(4097);
        let digest: [u8; 32] = Sha256::digest(&bytes).into();

        let mut staging = store.start(&staging_name, bytes.len() as u64).unwrap();
        for chunk in bytes.chunks(113) {
            staging.write_chunk(chunk).unwrap();
        }
        let published: PublishedPayload = staging.publish(digest).unwrap();
        assert_eq!(published.blob_id, blob);
        assert_eq!(published.length, bytes.len() as u64);
        assert_eq!(published.sha256, digest);

        let mut opened = store
            .open_verified(blob, bytes.len() as u64, digest)
            .unwrap();
        let mut downloaded = Vec::new();
        opened.file.read_to_end(&mut downloaded).unwrap();
        assert_eq!(downloaded, bytes);

        let other = b"prior bytes must not be replaced";
        let second_attempt = "00000000000000000000000000000002";
        let mut second = store
            .start(&format!("{second_attempt}-{blob}"), other.len() as u64)
            .unwrap();
        second.write_chunk(other).unwrap();
        assert!(second.publish(Sha256::digest(other).into()).is_err());
        let mut opened = store
            .open_verified(blob, bytes.len() as u64, digest)
            .unwrap();
        let mut after = Vec::new();
        opened.file.read_to_end(&mut after).unwrap();
        assert_eq!(after, bytes);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_payload_and_checked_length_and_digest_are_supported() {
        let root = root();
        let store = PayloadStore::open(&root).unwrap();
        let attempt = "00000000000000000000000000000003";
        let blob = "10000000000000000000000000000003";
        let staging_name = format!("{attempt}-{blob}");
        let empty_digest: [u8; 32] = Sha256::digest([]).into();
        let staging = store.start(&staging_name, 0).unwrap();
        let published = staging.publish(empty_digest).unwrap();
        assert_eq!(published.length, 0);
        assert_eq!(published.sha256, empty_digest);

        let retry_name = format!("{attempt}-10000000000000000000000000000004");
        assert!(
            store
                .start(&retry_name, 1)
                .unwrap()
                .publish(empty_digest)
                .is_err()
        );
        let retry_name = format!("{attempt}-10000000000000000000000000000005");
        let mut wrong = store.start(&retry_name, 1).unwrap();
        wrong.write_chunk(b"x").unwrap();
        assert!(wrong.publish(empty_digest).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verification_checks_cancellation_before_and_between_reads() {
        let root = root();
        let store = PayloadStore::open(&root).unwrap();
        let blob = "10000000000000000000000000000006";
        let bytes = b"payload".repeat(100_000);
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let mut staging = store
            .start(
                &format!("00000000000000000000000000000006-{blob}"),
                bytes.len() as u64,
            )
            .unwrap();
        staging.write_chunk(&bytes).unwrap();
        staging.publish(digest).unwrap();
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            store.open_verified_unless(blob, bytes.len() as u64, digest, &cancelled),
            Err(PayloadError::Cancelled)
        ));
        let mut checks = 0;
        assert!(matches!(
            super::check_payload(&store.payloads, blob, bytes.len() as u64, digest, || {
                checks += 1;
                if checks == 2 {
                    Err(PayloadError::Cancelled)
                } else {
                    Ok(())
                }
            }),
            Err(PayloadError::Cancelled)
        ));
        assert_eq!(checks, 2);
        cancelled.store(false, Ordering::Relaxed);
        assert!(
            store
                .open_verified_unless(blob, bytes.len() as u64, digest, &cancelled)
                .is_ok()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn canceled_unlink_skips_unpublished_bodies_and_checks_the_recorded_length() {
        use super::{CanceledPayload, CanceledUnlink};
        let root = root();
        let store = PayloadStore::open(&root).unwrap();
        let attempt = "0000000000000000000000000000000a";
        let blob = "1000000000000000000000000000000a";
        let bytes = b"canceled final payload";
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let canceled = |length| CanceledPayload {
            attempt_id: attempt.to_owned(),
            mutation_id: "2000000000000000000000000000000a".to_owned(),
            staging_name: format!("{attempt}-{blob}"),
            blob_id: blob.to_owned(),
            length,
        };
        let length = bytes.len() as u64;
        let mut staging = store.start(&format!("{attempt}-{blob}"), length).unwrap();
        staging.write_chunk(bytes).unwrap();
        assert_eq!(
            store.unlink_canceled(&canceled(length)),
            Ok(CanceledUnlink::Unpublished)
        );
        staging.publish(digest).unwrap();
        assert_eq!(
            store.unlink_canceled(&canceled(length + 1)),
            Err(PayloadError::Integrity)
        );
        assert!(store.open_verified(blob, length, digest).is_ok());
        assert_eq!(
            store.unlink_canceled(&canceled(length)),
            Ok(CanceledUnlink::Removed)
        );
        assert!(matches!(
            store.open_verified(blob, length, digest),
            Err(PayloadError::Missing)
        ));
        // A retry after an unrecorded unlink finds nothing and still succeeds.
        assert_eq!(
            store.unlink_canceled(&canceled(length)),
            Ok(CanceledUnlink::Removed)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "test-harness")]
    #[test]
    fn publish_fault_hook_is_called_at_the_noreplace_boundary() {
        use super::PayloadError;
        use std::sync::{Arc, Mutex};

        let root = root();
        let mut store = PayloadStore::open(&root).unwrap();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let hook_observed = Arc::clone(&observed);
        store.set_publish_fault_hook(Some(Arc::new(move |boundary| {
            hook_observed.lock().unwrap().push(boundary);
            if boundary == "publish.before_noreplace" {
                Err(PayloadError::Io("injected_publish_fault"))
            } else {
                Ok(())
            }
        })));

        let attempt = "00000000000000000000000000000009";
        let blob = "10000000000000000000000000000009";
        let staging = store.start(&format!("{attempt}-{blob}"), 0).unwrap();
        let error = staging.publish(Sha256::digest([]).into()).unwrap_err();
        assert_eq!(error, PayloadError::Io("injected_publish_fault"));
        // A zero-length body never calls `write_chunk`, but still passes the
        // file/staging-directory/ancestor sync boundaries before the rename.
        assert_eq!(
            *observed.lock().unwrap(),
            vec![
                "publish.before_file_sync",
                "publish.before_staging_dir_sync",
                "publish.before_ancestor_sync",
                "publish.before_noreplace",
            ]
        );

        fs::remove_dir_all(root).unwrap();
    }
}
