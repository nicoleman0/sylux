//! Bind a private storage root to its manifest and database identity.

use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    time::Duration,
};

use rusqlite::Connection;
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, RenameFlags, renameat_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    ownership::{self, OwnershipError},
    schema,
    sqlite::{self, ConnectionError},
};

pub const LOCK_NAME: &str = "writer.lock";
pub const MANIFEST_NAME: &str = "installation.json";
pub const MANIFEST_TMP_NAME: &str = "installation.json.tmp";
pub const METADATA_DIR: &str = "metadata";
pub const SECRETS_DIR: &str = "secrets";
pub const DB_NAME: &str = "sylux.sqlite3";
pub const KEY_NAME: &str = "mutation.key";
pub const SETUP_AUTHORIZATION_NAME: &str = "setup.authorization";

/// Mutation signing key length in bytes.
pub const KEY_LENGTH: usize = 32;
/// Hex-encoded application ID length.
pub const ID_LENGTH: usize = 32;
/// Maximum manifest JSON size.
const MANIFEST_MAX_BYTES: usize = 4096;

/// Installation identity shared by the manifest and database row.
/// Field names are part of the on-disk format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationIdentity {
    pub installation_id: String,
    pub root_id: String,
    pub db_id: String,
    pub schema_version: i64,
    pub layout_version: i64,
    pub name_key_version: i64,
    pub key_version: i64,
    pub initialized: bool,
}

impl InstallationIdentity {
    /// Structural format validation: 32 lowercase-hex IDs and positive
    /// versions. Values are never compared to user input here.
    pub fn validate_format(&self) -> Result<(), &'static str> {
        for id in [&self.installation_id, &self.root_id, &self.db_id] {
            if id.len() != ID_LENGTH
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err("identity_field_format");
            }
        }
        if self.schema_version <= 0
            || self.layout_version <= 0
            || self.name_key_version <= 0
            || self.key_version <= 0
        {
            return Err("identity_version_format");
        }
        Ok(())
    }
}

/// Binding failure. Reasons are redacted codes.
#[derive(Debug)]
pub enum BindingError {
    ManifestMissing,
    ManifestInvalid(&'static str),
    KeyMissing,
    KeyInvalid(&'static str),
    /// The metadata or secrets subdirectory is not a private directory.
    LayoutInvalid(&'static str),
    DatabaseMissing,
    DatabaseInvalid(&'static str),
    /// The installation table or its singleton row is absent.
    RowMissing,
    /// Manifest and database disagree on an identity field.
    IdentityMismatch(&'static str),
    /// A version field is not one this build supports.
    UnsupportedVersion(&'static str),
    /// The installation is not marked initialized.
    NotInitialized,
    /// The database path stopped referencing the validated inode.
    DatabaseReplaced,
    Io(&'static str),
    SnapshotDestination(std::io::Error),
    SnapshotOpen(String),
}

impl BindingError {
    /// Redacted machine reason code for contract events.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::ManifestMissing => "manifest_missing",
            Self::ManifestInvalid(code)
            | Self::KeyInvalid(code)
            | Self::LayoutInvalid(code)
            | Self::DatabaseInvalid(code)
            | Self::Io(code) => code,
            Self::SnapshotDestination(_) => "snapshot_destination_error",
            Self::SnapshotOpen(_) => "snapshot_open_error",
            Self::KeyMissing => "mutation_key_missing",
            Self::DatabaseMissing => "database_missing",
            Self::RowMissing => "installation_row_missing",
            Self::IdentityMismatch(field) => match *field {
                "installation_id" => "installation_id_mismatch",
                "root_id" => "root_id_mismatch",
                "db_id" => "db_id_mismatch",
                "schema_version" => "schema_version_mismatch",
                "layout_version" => "layout_version_mismatch",
                "name_key_version" => "name_key_version_mismatch",
                "key_version" => "key_version_mismatch",
                _ => "identity_mismatch",
            },
            Self::UnsupportedVersion(field) => match *field {
                "schema_version" => "schema_version_unsupported",
                "layout_version" => "layout_version_unsupported",
                "name_key_version" => "name_key_version_unsupported",
                "key_version" => "key_version_unsupported",
                _ => "version_unsupported",
            },
            Self::NotInitialized => "installation_not_initialized",
            Self::DatabaseReplaced => "database_replaced",
        }
    }
}

/// Verified writer connection and pinned metadata directory descriptor.
pub struct BoundDatabase {
    pub connection: Connection,
    _metadata: File,
}

/// The generated identity produced by a successful initialization.
#[derive(Debug)]
pub struct InitializedRoot {
    pub identity: InstallationIdentity,
}

/// Why host initialization refused or failed.
#[derive(Debug)]
pub enum InitError {
    /// The root is not an explicitly empty private directory.
    RootInvalid(&'static str),
    /// The root contains entries beyond the stable writer lock; an existing
    /// installation is never reset.
    NotExplicitlyEmpty,
    Ownership(OwnershipError),
    Io(&'static str),
}

impl InitError {
    /// Redacted machine reason code for contract events.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::RootInvalid(code) | Self::Io(code) => code,
            Self::NotExplicitlyEmpty => "root_not_explicitly_empty",
            Self::Ownership(error) => error.reason(),
        }
    }
}

/// Read a bounded private manifest without following aliases.
pub fn read_manifest(root: &Path) -> Result<InstallationIdentity, BindingError> {
    let root_fd = open_root_directory(root)?;
    let stat = match fs::statat(&root_fd, MANIFEST_NAME, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(error) if std::io::Error::from(error).kind() == std::io::ErrorKind::NotFound => {
            return Err(BindingError::ManifestMissing);
        }
        Err(_) => return Err(BindingError::Io("manifest_stat_failed")),
    };
    validate_private_regular(
        &stat,
        "manifest_not_regular",
        "manifest_aliased",
        "manifest_mode_not_0600",
    )
    .map_err(BindingError::ManifestInvalid)?;
    if stat.st_size < 0 || stat.st_size as usize > MANIFEST_MAX_BYTES {
        return Err(BindingError::ManifestInvalid("manifest_too_large"));
    }
    let fd = fs::openat(
        &root_fd,
        MANIFEST_NAME,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| BindingError::ManifestInvalid("manifest_open_failed"))?;
    let mut bytes = Vec::new();
    File::from(fd)
        .take(MANIFEST_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| BindingError::ManifestInvalid("manifest_read_failed"))?;
    if bytes.len() > MANIFEST_MAX_BYTES {
        return Err(BindingError::ManifestInvalid("manifest_too_large"));
    }
    let identity: InstallationIdentity = serde_json::from_slice(&bytes)
        .map_err(|_| BindingError::ManifestInvalid("manifest_not_canonical_json"))?;
    identity
        .validate_format()
        .map_err(|_| BindingError::ManifestInvalid("manifest_identity_format"))?;
    Ok(identity)
}

/// Open the bound database through validated descriptors and verify its
/// connection policy. Re-stat after open to detect path replacement.
pub fn open_bound_database(
    root: &Path,
    busy_timeout: Duration,
) -> Result<BoundDatabase, BindingError> {
    open_bound_database_as(root, busy_timeout, sqlite::ConnectionRole::Writer)
}

/// Same validation as `open_bound_database`, but the connection cannot write.
pub fn open_bound_database_read_only(
    root: &Path,
    busy_timeout: Duration,
) -> Result<BoundDatabase, BindingError> {
    open_bound_database_as(root, busy_timeout, sqlite::ConnectionRole::Reader)
}

/// Copy metadata under the caller-held writer lock, so SQLite never touches source sidecars.
/// `scratch` must be a private, empty directory outside the source root.
pub fn snapshot_bound_database(root: &Path, scratch: &Path) -> Result<BoundDatabase, BindingError> {
    use std::os::unix::fs::OpenOptionsExt;
    let root_fd = open_root_directory(root)?;
    let metadata = open_subdirectory(&root_fd, METADATA_DIR)
        .map_err(|_| BindingError::Io("metadata_open_failed"))?;
    for name in [DB_NAME.to_owned(), format!("{DB_NAME}-wal")] {
        let fd = match fs::openat(
            &metadata,
            name.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(e) if e == rustix::io::Errno::NOENT && name != DB_NAME => continue,
            Err(_) => return Err(BindingError::Io("snapshot_source_unavailable")),
        };
        let stat = fs::fstat(&fd).map_err(|_| BindingError::Io("snapshot_stat_failed"))?;
        validate_private_regular(
            &stat,
            "database_not_regular",
            "database_aliased",
            "database_mode_not_0600",
        )
        .map_err(BindingError::DatabaseInvalid)?;
        let mut input = File::from(fd);
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(scratch.join(&name))
            .map_err(BindingError::SnapshotDestination)?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|_| BindingError::Io("snapshot_source_read_failed"))?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .map_err(BindingError::SnapshotDestination)?;
        }
    }
    let connection = sqlite::open_reader(&scratch.join(DB_NAME), sqlite::DEFAULT_BUSY_TIMEOUT)
        .map_err(|error| BindingError::SnapshotOpen(error.reason()))?;
    Ok(BoundDatabase {
        connection,
        _metadata: metadata,
    })
}

fn open_bound_database_as(
    root: &Path,
    busy_timeout: Duration,
    role: sqlite::ConnectionRole,
) -> Result<BoundDatabase, BindingError> {
    let root_fd = open_root_directory(root)?;
    let metadata_fd = open_subdirectory(&root_fd, METADATA_DIR).map_err(|error| match error {
        SubdirectoryError::Missing => BindingError::DatabaseMissing,
        other => BindingError::LayoutInvalid(other.reason()),
    })?;
    let stat = match fs::statat(&metadata_fd, DB_NAME, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(error) if std::io::Error::from(error).kind() == std::io::ErrorKind::NotFound => {
            return Err(BindingError::DatabaseMissing);
        }
        Err(_) => return Err(BindingError::Io("database_stat_failed")),
    };
    validate_private_regular(
        &stat,
        "database_not_regular",
        "database_aliased",
        "database_mode_not_0600",
    )
    .map_err(BindingError::DatabaseInvalid)?;
    let expected = (stat.st_dev, stat.st_ino);
    let path = root.join(METADATA_DIR).join(DB_NAME);
    let connection = sqlite::open_connection(&path, role, busy_timeout)
        .map_err(|error| BindingError::DatabaseInvalid(connection_open_code(error)))?;
    let after = fs::statat(&metadata_fd, DB_NAME, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| BindingError::Io("database_stat_failed"))?;
    if (after.st_dev, after.st_ino) != expected {
        // The path was replaced between validation and open: fail closed
        // rather than serve an unbound database.
        drop(connection);
        return Err(BindingError::DatabaseReplaced);
    }
    Ok(BoundDatabase {
        connection,
        _metadata: metadata_fd,
    })
}

fn connection_open_code(error: ConnectionError) -> &'static str {
    match error {
        ConnectionError::Policy(_) => "database_policy_unverified",
        ConnectionError::Open(_) => "database_open_failed",
    }
}

/// Verify the protected key before opening SQLite, avoiding WAL/SHM side effects.
pub fn verify_protected_material(root: &Path) -> Result<(), BindingError> {
    let root_fd = open_root_directory(root)?;
    read_mutation_key(&root_fd).map(drop)
}

/// Verify the protected key and agreement between manifest, database row and
/// supported versions.
pub fn verify_binding(
    bound: &BoundDatabase,
    root: &Path,
    manifest: &InstallationIdentity,
) -> Result<(), BindingError> {
    verify_protected_material(root)?;
    let row = read_installation_row(&bound.connection)?;
    if !manifest.initialized || !row.initialized {
        return Err(BindingError::NotInitialized);
    }
    if row.installation_id != manifest.installation_id {
        return Err(BindingError::IdentityMismatch("installation_id"));
    }
    if row.root_id != manifest.root_id {
        return Err(BindingError::IdentityMismatch("root_id"));
    }
    if row.db_id != manifest.db_id {
        return Err(BindingError::IdentityMismatch("db_id"));
    }
    for (field, manifest_version, row_version, supported) in [
        (
            "schema_version",
            manifest.schema_version,
            row.schema_version,
            schema::SUPPORTED_SCHEMA_VERSION,
        ),
        (
            "layout_version",
            manifest.layout_version,
            row.layout_version,
            schema::SUPPORTED_LAYOUT_VERSION,
        ),
        (
            "name_key_version",
            manifest.name_key_version,
            row.name_key_version,
            schema::SUPPORTED_NAME_KEY_VERSION,
        ),
        (
            "key_version",
            manifest.key_version,
            row.key_version,
            schema::SUPPORTED_KEY_VERSION,
        ),
    ] {
        if manifest_version != row_version {
            return Err(BindingError::IdentityMismatch(field));
        }
        if row_version != supported {
            return Err(BindingError::UnsupportedVersion(field));
        }
    }
    Ok(())
}

/// Initialize an explicitly empty root. Existing installations are never reset.
pub fn initialize_empty_root(
    root: &Path,
    busy_timeout: Duration,
) -> Result<InitializedRoot, InitError> {
    let root_fd =
        open_root_directory(root).map_err(|error| InitError::RootInvalid(error.reason()))?;
    if !ownership::root_is_empty_apart_from_lock(&root_fd).map_err(InitError::Ownership)? {
        return Err(InitError::NotExplicitlyEmpty);
    }

    super::contract_event("ownership.lock_acquire");
    let guard = ownership::acquire_or_create_writer_lock(root).map_err(|error| {
        if matches!(error, OwnershipError::LockHeld) {
            super::contract_event_reason("ownership.refused", error.reason());
        }
        InitError::Ownership(error)
    })?;
    super::contract_event("ownership.acquired");

    // Persist only the setup verifier in SQLite; the raw authorization stays protected.
    let installation_id = random_id()?;
    let root_id = random_id()?;
    let db_id = random_id()?;
    let mut mutation_key = [0u8; KEY_LENGTH];
    getrandom::fill(&mut mutation_key).map_err(|_| InitError::Io("entropy_unavailable"))?;
    let setup_authorization = random_hex_64()?;
    let setup_verifier: [u8; 32] = Sha256::digest(setup_authorization.as_bytes()).into();
    let identity = InstallationIdentity {
        installation_id,
        root_id,
        db_id,
        schema_version: schema::SUPPORTED_SCHEMA_VERSION,
        layout_version: schema::SUPPORTED_LAYOUT_VERSION,
        name_key_version: schema::SUPPORTED_NAME_KEY_VERSION,
        key_version: schema::SUPPORTED_KEY_VERSION,
        initialized: true,
    };

    create_private_directory(&root_fd, SECRETS_DIR)?;
    let secrets_fd =
        open_subdirectory(&root_fd, SECRETS_DIR).map_err(|error| InitError::Io(error.reason()))?;
    write_exclusive(&secrets_fd, KEY_NAME, &mutation_key)?;
    write_exclusive(
        &secrets_fd,
        SETUP_AUTHORIZATION_NAME,
        setup_authorization.as_bytes(),
    )?;
    sync_fd(&secrets_fd, "secrets_sync_failed")?;

    let manifest_bytes =
        serde_json::to_vec(&identity).map_err(|_| InitError::Io("manifest_encode_failed"))?;
    write_exclusive(&root_fd, MANIFEST_TMP_NAME, &manifest_bytes)?;
    renameat_with(
        &root_fd,
        MANIFEST_TMP_NAME,
        &root_fd,
        MANIFEST_NAME,
        RenameFlags::NOREPLACE,
    )
    .map_err(|_| InitError::Io("manifest_publish_failed"))?;
    sync_fd(&root_fd, "root_sync_failed")?;

    create_private_directory(&root_fd, METADATA_DIR)?;
    let metadata_fd =
        open_subdirectory(&root_fd, METADATA_DIR).map_err(|error| InitError::Io(error.reason()))?;
    sync_fd(&metadata_fd, "metadata_sync_failed")?;
    let db_fd = fs::openat(
        &metadata_fd,
        DB_NAME,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| InitError::Io("database_create_failed"))?;
    let db_file = File::from(db_fd);
    sync_fd(&db_file, "database_sync_failed")?;
    sync_fd(&metadata_fd, "metadata_sync_failed")?;

    let db_path = root.join(METADATA_DIR).join(DB_NAME);
    let connection = sqlite::open_writer(&db_path, busy_timeout)
        .map_err(|_| InitError::Io("database_open_failed"))?;
    let migrated =
        schema::apply_migrations(&connection).map_err(|_| InitError::Io("migration_failed"))?;
    if migrated != schema::MigrationOutcome::Applied {
        return Err(InitError::Io("migration_not_applied"));
    }
    schema::insert_installation_row(&connection, &identity, setup_verifier)
        .map_err(|_| InitError::Io("installation_row_failed"))?;
    sqlite::wal_checkpoint_truncate(&connection).map_err(|_| InitError::Io("checkpoint_failed"))?;
    sync_fd(&db_file, "database_sync_failed")?;
    drop(connection);
    sync_fd(&metadata_fd, "metadata_sync_failed")?;

    drop(guard);
    Ok(InitializedRoot { identity })
}

fn read_installation_row(connection: &Connection) -> Result<InstallationIdentity, BindingError> {
    connection
        .query_row(
            "SELECT installation_id, root_id, db_id, schema_version, layout_version,
                    name_key_version, key_version, initialized
             FROM installation WHERE singleton = 1",
            [],
            |row| {
                Ok(InstallationIdentity {
                    installation_id: row.get(0)?,
                    root_id: row.get(1)?,
                    db_id: row.get(2)?,
                    schema_version: row.get(3)?,
                    layout_version: row.get(4)?,
                    name_key_version: row.get(5)?,
                    key_version: row.get(6)?,
                    initialized: row.get::<_, i64>(7)? != 0,
                })
            },
        )
        .map_err(|_| BindingError::RowMissing)
}

fn read_mutation_key(root_fd: &File) -> Result<Vec<u8>, BindingError> {
    let secrets_fd = open_subdirectory(root_fd, SECRETS_DIR).map_err(|error| match error {
        SubdirectoryError::Missing => BindingError::KeyMissing,
        other => BindingError::LayoutInvalid(other.reason()),
    })?;
    let stat = match fs::statat(&secrets_fd, KEY_NAME, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(error) if std::io::Error::from(error).kind() == std::io::ErrorKind::NotFound => {
            return Err(BindingError::KeyMissing);
        }
        Err(_) => return Err(BindingError::Io("mutation_key_stat_failed")),
    };
    validate_private_regular(
        &stat,
        "mutation_key_not_regular",
        "mutation_key_aliased",
        "mutation_key_mode_not_0600",
    )
    .map_err(BindingError::KeyInvalid)?;
    if stat.st_size != KEY_LENGTH as i64 {
        return Err(BindingError::KeyInvalid("mutation_key_length"));
    }
    let fd = fs::openat(
        &secrets_fd,
        KEY_NAME,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| BindingError::KeyInvalid("mutation_key_open_failed"))?;
    let mut bytes = Vec::new();
    File::from(fd)
        .take(KEY_LENGTH as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| BindingError::KeyInvalid("mutation_key_read_failed"))?;
    if bytes.len() != KEY_LENGTH {
        return Err(BindingError::KeyInvalid("mutation_key_length"));
    }
    Ok(bytes)
}

/// Only immutable installation secrets belong in a backup.
pub(crate) fn backup_secrets(root: &Path) -> Result<(Vec<u8>, Vec<u8>), BindingError> {
    let root_fd = open_root_directory(root)?;
    let key = read_mutation_key(&root_fd)?;
    let secrets = open_subdirectory(&root_fd, SECRETS_DIR)
        .map_err(|error| BindingError::LayoutInvalid(error.reason()))?;
    let invalid = BindingError::Io("setup_authorization_invalid");
    let fd = fs::openat(
        &secrets,
        SETUP_AUTHORIZATION_NAME,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| BindingError::Io("setup_authorization_unavailable"))?;
    let stat = fs::fstat(&fd).map_err(|_| BindingError::Io("setup_authorization_stat_failed"))?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_nlink != 1
        || stat.st_mode & 0o777 != 0o600
    {
        return Err(invalid);
    }
    let mut authorization = Vec::new();
    File::from(fd)
        .take(65)
        .read_to_end(&mut authorization)
        .map_err(|_| BindingError::Io("setup_authorization_read_failed"))?;
    if authorization.len() != 64 || !authorization.iter().all(u8::is_ascii_hexdigit) {
        return Err(invalid);
    }
    Ok((key, authorization))
}

/// Open the root as a private (0700) directory through a validated
/// descriptor; the final component must not be a symlink.
fn open_root_directory(root: &Path) -> Result<File, BindingError> {
    let fd = fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(rustix_io_error)
    .map_err(|error| {
        BindingError::Io(match error.kind() {
            std::io::ErrorKind::NotFound => "root_missing",
            std::io::ErrorKind::NotADirectory => "root_not_directory",
            _ => "root_unusable",
        })
    })?;
    let stat = fs::fstat(&fd).map_err(|_| BindingError::Io("root_stat_failed"))?;
    if !FileType::from_raw_mode(stat.st_mode).is_dir() {
        return Err(BindingError::Io("root_not_directory"));
    }
    if stat.st_mode & 0o777 != 0o700 {
        return Err(BindingError::Io("root_mode_not_0700"));
    }
    Ok(File::from(fd))
}

/// Why a root subdirectory could not be opened as a private directory.
#[derive(Debug)]
enum SubdirectoryError {
    Missing,
    NotDirectory,
    Aliased,
    Mode,
    Stat,
}

impl SubdirectoryError {
    fn reason(&self) -> &'static str {
        match self {
            Self::Missing => "subdirectory_missing",
            Self::NotDirectory => "subdirectory_not_directory",
            Self::Aliased => "subdirectory_aliased",
            Self::Mode => "subdirectory_mode_not_0700",
            Self::Stat => "subdirectory_stat_failed",
        }
    }
}

/// Open a named private (0700) subdirectory of the root through a validated
/// descriptor that never follows a symlink.
fn open_subdirectory(root_fd: &File, name: &str) -> Result<File, SubdirectoryError> {
    let fd = fs::openat(
        root_fd,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(rustix_io_error)
    .map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => SubdirectoryError::Missing,
        std::io::ErrorKind::NotADirectory => SubdirectoryError::NotDirectory,
        // NOFOLLOW on a symlink fails here.
        _ => SubdirectoryError::Aliased,
    })?;
    let stat = fs::fstat(&fd).map_err(|_| SubdirectoryError::Stat)?;
    if !FileType::from_raw_mode(stat.st_mode).is_dir() {
        return Err(SubdirectoryError::NotDirectory);
    }
    if stat.st_mode & 0o777 != 0o700 {
        return Err(SubdirectoryError::Mode);
    }
    Ok(File::from(fd))
}

/// Convert a rustix `Errno` into a `std::io::Error` so classification uses
/// standard error kinds.
fn rustix_io_error(errno: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from(errno)
}

/// Validate a stat as a private regular file with no hardlink alias. The
/// call site supplies redacted reason codes naming the file.
fn validate_private_regular(
    stat: &fs::Stat,
    not_regular: &'static str,
    aliased: &'static str,
    bad_mode: &'static str,
) -> Result<(), &'static str> {
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(not_regular);
    }
    if stat.st_nlink != 1 {
        return Err(aliased);
    }
    if stat.st_mode & 0o777 != 0o600 {
        return Err(bad_mode);
    }
    Ok(())
}

/// Create a private subdirectory and sync the parent so the entry is durable.
fn create_private_directory(root_fd: &File, name: &str) -> Result<(), InitError> {
    rustix::fs::mkdirat(root_fd, name, Mode::RWXU)
        .map_err(|_| InitError::Io("subdirectory_create_failed"))?;
    sync_fd(root_fd, "root_sync_failed")
}

/// Create a private file exclusively beneath a validated directory, write the
/// whole payload, and sync it. No file is ever truncated or replaced.
fn write_exclusive(directory: &File, name: &str, payload: &[u8]) -> Result<(), InitError> {
    let fd = fs::openat(
        directory,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| InitError::Io("protected_file_create_failed"))?;
    let mut file = File::from(fd);
    file.write_all(payload)
        .and_then(|()| file.sync_all())
        .map_err(|_| InitError::Io("protected_file_write_failed"))
}

/// Sync a descriptor (file or directory), mapping failure to a redacted code.
fn sync_fd(descriptor: &impl std::os::fd::AsFd, code: &'static str) -> Result<(), InitError> {
    fs::fsync(descriptor).map_err(|_| InitError::Io(code))
}

/// A fresh 128-bit server-random ID as 32 lowercase hex characters.
fn random_id() -> Result<String, InitError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| InitError::Io("entropy_unavailable"))?;
    Ok(hex(&bytes))
}

/// A fresh 256-bit value as 64 lowercase hex characters (used for the raw
/// one-use setup authorization).
fn random_hex_64() -> Result<String, InitError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| InitError::Io("entropy_unavailable"))?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Complete supported schema/binding transitions under the startup writer lock.
pub fn upgrade_binding(
    bound: &BoundDatabase,
    root: &Path,
    manifest: &InstallationIdentity,
) -> Result<InstallationIdentity, BindingError> {
    if !schema::is_older_schema(manifest.schema_version) {
        verify_binding(bound, root, manifest)?;
        super::contract_event("binding.verified");
        super::contract_event("migration.start");
        return Ok(manifest.clone());
    }
    upgrade_binding_step(bound, root, manifest, true)
}

fn upgrade_binding_step(
    bound: &BoundDatabase,
    root: &Path,
    manifest: &InstallationIdentity,
    announce: bool,
) -> Result<InstallationIdentity, BindingError> {
    verify_protected_material(root)?;
    let row = read_installation_row(&bound.connection)?;
    let mut expected = manifest.clone();
    expected.schema_version = row.schema_version;
    if expected != row {
        return Err(BindingError::IdentityMismatch("schema_version"));
    }
    if !row.initialized
        || row.layout_version != schema::SUPPORTED_LAYOUT_VERSION
        || row.name_key_version != schema::SUPPORTED_NAME_KEY_VERSION
        || row.key_version != schema::SUPPORTED_KEY_VERSION
    {
        return Err(BindingError::UnsupportedVersion("schema_version"));
    }
    let version: i64 = bound
        .connection
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|_| BindingError::DatabaseInvalid("schema_version_unreadable"))?;
    let bytes =
        serde_json::to_vec(manifest).map_err(|_| BindingError::Io("manifest_encode_failed"))?;
    let hash = Sha256::digest(&bytes);
    let previous = manifest.schema_version;
    let next = previous + 1;
    let migrate = match previous {
        1 => schema::migrate_v1,
        2 => schema::migrate_v2,
        3 => schema::migrate_v3,
        4 => schema::migrate_v4,
        5 => schema::migrate_direct_grants,
        6 => schema::migrate_library_publication,
        7 => schema::migrate_external_identity,
        _ => return Err(BindingError::UnsupportedVersion("schema_version")),
    };
    if row.schema_version == previous && version == previous {
        if announce {
            super::contract_event("migration.start");
        }
        migrate(&bound.connection, Some(hash.as_slice()))
            .map_err(|_| BindingError::DatabaseInvalid("schema_migration_failed"))?;
    } else if row.schema_version == next && version == next {
        let recorded: Vec<u8> = bound
            .connection
            .query_row(
                "SELECT legacy_manifest_hash FROM binding_transition WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| BindingError::IdentityMismatch("schema_version"))?;
        if recorded != hash.as_slice() {
            return Err(BindingError::IdentityMismatch("schema_version"));
        }
        schema::verify_schema_at(&bound.connection, next)
            .map_err(|_| BindingError::DatabaseInvalid("schema_invalid"))?;
        if announce {
            super::contract_event("migration.start");
        }
    } else {
        return Err(BindingError::UnsupportedVersion("schema_version"));
    }
    let mut upgraded = manifest.clone();
    upgraded.schema_version = next;
    let bytes =
        serde_json::to_vec(&upgraded).map_err(|_| BindingError::Io("manifest_encode_failed"))?;
    let root_fd = open_root_directory(root)?;
    // An interrupted exclusive temporary publication may leave only our fixed temp name.
    match fs::statat(&root_fd, MANIFEST_TMP_NAME, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            validate_private_regular(
                &stat,
                "manifest_not_regular",
                "manifest_aliased",
                "manifest_mode_not_0600",
            )
            .map_err(BindingError::ManifestInvalid)?;
            fs::unlinkat(&root_fd, MANIFEST_TMP_NAME, AtFlags::empty())
                .map_err(|_| BindingError::Io("manifest_temp_remove_failed"))?;
        }
        Err(error) if std::io::Error::from(error).kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(BindingError::Io("manifest_stat_failed")),
    }
    write_exclusive(&root_fd, MANIFEST_TMP_NAME, &bytes)
        .map_err(|_| BindingError::Io("manifest_write_failed"))?;
    fs::renameat(&root_fd, MANIFEST_TMP_NAME, &root_fd, MANIFEST_NAME)
        .map_err(|_| BindingError::Io("manifest_publish_failed"))?;
    fs::fsync(&root_fd).map_err(|_| BindingError::Io("manifest_sync_failed"))?;
    if next < schema::SUPPORTED_SCHEMA_VERSION {
        return upgrade_binding_step(bound, root, &upgraded, false);
    }
    verify_binding(bound, root, &upgraded)?;
    super::contract_event("binding.verified");
    Ok(upgraded)
}
