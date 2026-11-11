//! Lifetime exclusive ownership of a storage root (s1-design-review §3).
//!
//! `writer.lock` is opened without truncation and locked `LOCK_EX|LOCK_NB`;
//! the descriptor is retained in a non-cloneable guard for the whole process
//! lifetime and is released only when the guard is dropped (after workers and
//! connections have drained) or when the process dies. Flock is advisory and
//! bound to the open file description: every supported writer must cooperate,
//! and a second process therefore fails before opening the database.
//!
//! This is local exclusion only. Flock is not distributed fencing, and a
//! held lock never proves a remote lease will expire.

use std::{
    fs::File,
    io,
    os::fd::{AsFd, OwnedFd},
    path::Path,
};

use rustix::fs::{self, AtFlags, Dir, FileType, FlockOperation, Mode, OFlags, flock};

use super::root::LOCK_NAME;

/// Non-cloneable retained ownership of one storage root.
///
/// The lock descriptor never leaves this type. There is deliberately no
/// unlock method: shutdown must close admission, drain workers, and close the
/// database first — the guard is simply the last thing to drop.
#[derive(Debug)]
pub struct OwnershipGuard {
    lock: File,
}

impl OwnershipGuard {
    /// `(device, inode)` of the retained lock descriptor, for identity checks.
    pub fn lock_identity(&self) -> io::Result<(u64, u64)> {
        let stat = fs::fstat(self.lock.as_fd())?;
        #[allow(clippy::useless_conversion)] // st_dev differs across supported build hosts.
        let device = u64::try_from(stat.st_dev)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative device number"))?;
        Ok((device, stat.st_ino))
    }
}

// No `Clone`/`Copy` on purpose: duplicating the guard would duplicate the
// ownership claim itself, and exporting the descriptor would let another
// holder outlive this process's drain discipline.

/// Why exclusive ownership could not be established. Every variant carries a
/// redacted machine reason code, never a path or raw error.
#[derive(Debug)]
pub enum OwnershipError {
    /// The root path is not a usable private directory.
    RootInvalid(&'static str),
    /// The lock file is absent; a serving process never creates it.
    LockMissing,
    /// The lock file exists but is not a private regular file (symlink or
    /// hardlink alias, wrong type, or wrong permissions).
    LockInvalid(&'static str),
    /// Another process holds the exclusive writer lock.
    LockHeld,
    /// Unexpected I/O failure: unclassified and fail-closed.
    Io(&'static str),
}

impl OwnershipError {
    /// Redacted machine reason code for contract events.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::RootInvalid(code) | Self::LockInvalid(code) | Self::Io(code) => code,
            Self::LockMissing => "writer_lock_missing",
            Self::LockHeld => "writer_lock_held",
        }
    }
}

/// Acquire the writer lock for serving: the lock file must already exist and
/// be a private regular file. Nothing is ever created or written on this
/// path, so a refused process leaves the root byte-identical.
pub fn acquire_writer_lock(root: &Path) -> Result<OwnershipGuard, OwnershipError> {
    let root_fd = open_root_directory(root)?;
    let fd = open_existing_lock(&root_fd)?;
    lock_exclusive(fd)
}

/// Acquire the writer lock for explicit host initialization. The caller must
/// have verified the root is empty first; this creates `writer.lock` (0600,
/// exclusive, descriptor-relative) only when it does not yet exist and syncs
/// the root directory so the new entry is durable.
pub fn acquire_or_create_writer_lock(root: &Path) -> Result<OwnershipGuard, OwnershipError> {
    let root_fd = open_root_directory(root)?;
    let fd = match fs::statat(&root_fd, LOCK_NAME, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            validate_lock_stat(&stat)?;
            openat_lock(&root_fd)?
        }
        Err(error) if io::Error::from(error).kind() == io::ErrorKind::NotFound => {
            let fd = fs::openat(
                &root_fd,
                LOCK_NAME,
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|_| OwnershipError::Io("writer_lock_create_failed"))?;
            // Persist the new directory entry before anything else is created.
            fs::fsync(&root_fd).map_err(|_| OwnershipError::Io("root_sync_failed"))?;
            fd
        }
        Err(_) => return Err(OwnershipError::Io("writer_lock_stat_failed")),
    };
    lock_exclusive(fd)
}

/// The root contains only the stable lock entry (which may legitimately be
/// absent). Any other entry means this is not an explicitly empty dedicated
/// root, and initialization must refuse without touching anything.
pub fn root_is_empty_apart_from_lock(root_fd: &File) -> Result<bool, OwnershipError> {
    let directory = Dir::read_from(root_fd).map_err(|_| OwnershipError::Io("root_read_failed"))?;
    for entry in directory {
        let entry = entry.map_err(|_| OwnershipError::Io("root_read_failed"))?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." || name == LOCK_NAME.as_bytes() {
            continue;
        }
        return Ok(false);
    }
    Ok(true)
}

/// Open the root as a private (0700) directory through a validated
/// descriptor; the final component must not be a symlink.
fn open_root_directory(root: &Path) -> Result<File, OwnershipError> {
    let fd = fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| OwnershipError::RootInvalid(root_open_code(io::Error::from(error))))?;
    let stat = fs::fstat(&fd).map_err(|_| OwnershipError::RootInvalid("root_stat_failed"))?;
    if !FileType::from_raw_mode(stat.st_mode).is_dir() {
        return Err(OwnershipError::RootInvalid("root_not_directory"));
    }
    if stat.st_mode & 0o777 != 0o700 {
        return Err(OwnershipError::RootInvalid("root_mode_not_0700"));
    }
    Ok(File::from(fd))
}

fn root_open_code(error: io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::NotFound => "root_missing",
        io::ErrorKind::NotADirectory => "root_not_directory",
        _ => "root_unusable",
    }
}

fn validate_lock_stat(stat: &fs::Stat) -> Result<(), OwnershipError> {
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(OwnershipError::LockInvalid("writer_lock_not_regular"));
    }
    if stat.st_nlink != 1 {
        return Err(OwnershipError::LockInvalid("writer_lock_aliased"));
    }
    if stat.st_mode & 0o777 != 0o600 {
        return Err(OwnershipError::LockInvalid("writer_lock_mode_not_0600"));
    }
    Ok(())
}

fn open_existing_lock(root_fd: &File) -> Result<OwnedFd, OwnershipError> {
    match fs::statat(root_fd, LOCK_NAME, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => validate_lock_stat(&stat)?,
        Err(error) if io::Error::from(error).kind() == io::ErrorKind::NotFound => {
            return Err(OwnershipError::LockMissing);
        }
        Err(_) => return Err(OwnershipError::Io("writer_lock_stat_failed")),
    }
    openat_lock(root_fd)
}

fn openat_lock(root_fd: &File) -> Result<OwnedFd, OwnershipError> {
    fs::openat(
        root_fd,
        LOCK_NAME,
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        OwnershipError::LockInvalid(match io::Error::from(error).kind() {
            io::ErrorKind::NotFound => "writer_lock_missing",
            // NOFOLLOW on a symlink fails here: an alias is never followed.
            _ => "writer_lock_aliased",
        })
    })
}

fn lock_exclusive(fd: OwnedFd) -> Result<OwnershipGuard, OwnershipError> {
    match flock(&fd, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(OwnershipGuard {
            lock: File::from(fd),
        }),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(OwnershipError::LockHeld),
        Err(_) => Err(OwnershipError::Io("writer_lock_flock_failed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::DirBuilderExt, os::unix::fs::PermissionsExt, path::PathBuf};

    fn temp_root(tag: &str) -> PathBuf {
        let mut token = [0u8; 16];
        getrandom::fill(&mut token).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "sylux-ownership-{tag}-{}",
            token.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        dir
    }

    #[test]
    fn second_acquire_is_refused_and_first_stays_held() {
        let root = temp_root("second");
        let lock = root.join(LOCK_NAME);
        fs::write(&lock, b"").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
        let first = acquire_writer_lock(&root).unwrap();
        match acquire_writer_lock(&root) {
            Err(OwnershipError::LockHeld) => (),
            other => panic!("second writer was not refused: {other:?}"),
        }
        drop(first);
        // Released by drop: a fresh acquirer can now take it.
        acquire_writer_lock(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_lock_is_refused_without_creating_anything() {
        let root = temp_root("missing");
        match acquire_writer_lock(&root) {
            Err(OwnershipError::LockMissing) => (),
            other => panic!("missing lock was not refused: {other:?}"),
        }
        assert!(fs::read_dir(&root).unwrap().next().is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn empty_apart_from_lock_detects_foreign_entries() {
        let root = temp_root("entries");
        let fd = fs::File::open(&root).unwrap();
        assert!(root_is_empty_apart_from_lock(&fd).unwrap());
        fs::write(root.join(LOCK_NAME), b"").unwrap();
        assert!(root_is_empty_apart_from_lock(&fd).unwrap());
        fs::write(root.join("installation.json"), b"{}").unwrap();
        assert!(!root_is_empty_apart_from_lock(&fd).unwrap());
        drop(fd);
        fs::remove_dir_all(&root).unwrap();
    }
}
