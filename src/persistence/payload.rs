//! Durable, create-only payload publication.
//!
//! Staging and final shards live beneath the same validated storage root.
//! Every staging file is exclusive and server-named. Publication performs a
//! checked write, SHA-256 verification, file sync, staging-directory sync,
//! `RENAME_NOREPLACE`, and final-directory/created-ancestry sync. No failure
//! path deletes candidate bytes or falls back to an overwriting rename/copy.

use rustix::fs::{self, FileType, Mode, OFlags, RenameFlags};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
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

/// Validated directory descriptors for one installation's payload storage.
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

    /// Exclusively create a staging file named `<attempt-id>-<blob-id>`.
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

    /// Open a referenced blob without following links and verify its complete
    /// length and SHA-256 before exposing a stream to an authorized reader.
    pub fn open_verified(
        &self,
        blob_id: &str,
        expected_length: u64,
        expected_sha256: [u8; 32],
    ) -> Result<VerifiedPayload, PayloadError> {
        validate_id(blob_id)?;
        let shard = open_directory(&self.payloads, &blob_id[..2])?;
        let fd = fs::openat(
            &shard,
            blob_id,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| match std::io::Error::from(error).kind() {
            std::io::ErrorKind::NotFound => PayloadError::Missing,
            _ => PayloadError::Io("payload_open_failed"),
        })?;
        let mut file = File::from(fd);
        let stat = fs::fstat(&file).map_err(|_| PayloadError::Io("payload_stat_failed"))?;
        if !FileType::from_raw_mode(stat.st_mode).is_file()
            || stat.st_nlink != 1
            || stat.st_mode & 0o777 != 0o600
            || stat.st_size < 0
            || stat.st_size as u64 != expected_length
        {
            return Err(PayloadError::Integrity);
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|_| PayloadError::Io("payload_read_failed"))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let actual: [u8; 32] = hasher.finalize().into();
        if actual != expected_sha256 {
            return Err(PayloadError::Integrity);
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| PayloadError::Io("payload_seek_failed"))?;
        Ok(VerifiedPayload {
            file,
            length: expected_length,
        })
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

    /// Durably publish this completed body. The expected digest is supplied by
    /// the admission layer only after its own streaming hash has been checked;
    /// the independently computed value here must still match exactly.
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
    use super::{PayloadStore, PublishedPayload};
    use sha2::{Digest, Sha256};
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
