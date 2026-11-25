//! Core schema migration and startup checks.

use rusqlite::{Connection, params};

use super::{root::InstallationIdentity, sqlite};

/// Versions this build understands and serves. A bound database reporting
/// anything else refuses startup instead of migrating guesses.
pub const SUPPORTED_SCHEMA_VERSION: i64 = 4;

pub fn is_older_schema(version: i64) -> bool {
    (1..SUPPORTED_SCHEMA_VERSION).contains(&version)
}
pub const SUPPORTED_LAYOUT_VERSION: i64 = 1;
pub const SUPPORTED_NAME_KEY_VERSION: i64 = 1;
pub const SUPPORTED_KEY_VERSION: i64 = 1;

/// Core schema SQL embedded from `migrations/sqlite/0001_s1.sql`.
pub const MIGRATION_0001_S1: &str = include_str!("../../migrations/sqlite/0001_s1.sql");

const CORE_TABLES: &[&str] = &[
    "installation",
    "account",
    "session",
    "entry",
    "blob",
    "revision",
    "mutation",
    "upload_attempt",
    "event",
    "removal",
    "removal_member",
    "removal_cleanup",
    "binding_transition",
];
const CORE_INDEXES: &[&str] = &[
    "entry_active_sibling",
    "entry_one_root",
    "entry_active_listing",
    "revision_entry_time",
    "revision_blob",
    "session_account",
    "mutation_principal",
    "upload_attempt_one_active",
    "removal_cleanup_pending",
    "removal_member_operation",
    "removal_cleanup_operation",
];
const CORE_TRIGGERS: &[&str] = &[
    "entry_insert_parent_active_folder",
    "entry_update_parent_active_folder",
    "entry_insert_no_parent_cycle",
    "entry_update_no_parent_cycle",
    "entry_insert_current_same_entry",
    "entry_update_current_same_entry",
    "revision_rows_are_immutable",
    "revision_rows_cannot_be_deleted",
    "removal_immutable_update",
    "removal_immutable_delete",
    "removal_member_immutable_update",
    "removal_member_immutable_delete",
    "removal_member_owner",
    "revision_requires_live_blob",
    "removed_entry_stays_removed",
    "blob_delete_authorization",
    "blob_no_resurrection",
    "removed_parent_rejects_insert",
    "removed_parent_rejects_move",
    "removal_cleanup_immutable_delete",
    "removal_cleanup_only_completion",
    "removal_cleanup_requires_members",
];

#[derive(Debug, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// The migration transaction ran and was committed.
    Applied,
    /// The database already reports the supported version and a complete
    /// core schema; nothing was changed.
    AlreadyCurrent,
}

/// Why a schema operation failed.
#[derive(Debug)]
pub enum SchemaError {
    /// The migration tracker disagrees with the schema actually present.
    Inconsistent(&'static str),
    /// A core table, index or trigger is missing.
    MissingObject(String),
    /// SQL failure while migrating or checking.
    Sql(String),
    /// `PRAGMA integrity_check` did not report `ok`.
    Integrity,
    /// `PRAGMA foreign_key_check` reported violations.
    ForeignKeyViolation,
}

impl SchemaError {
    pub fn reason(&self) -> String {
        match self {
            Self::Inconsistent(code) => format!("schema_inconsistent: {code}"),
            Self::MissingObject(object) => format!("schema_object_missing: {object}"),
            Self::Sql(message) => format!("schema_sql_failed: {message}"),
            Self::Integrity => "integrity_check_failed".to_owned(),
            Self::ForeignKeyViolation => "foreign_key_check_failed".to_owned(),
        }
    }
}

fn sql_error(error: rusqlite::Error) -> SchemaError {
    SchemaError::Sql(error.to_string())
}

/// Apply or verify the supported schema version.
pub fn apply_migrations(connection: &Connection) -> Result<MigrationOutcome, SchemaError> {
    sqlite::verify_static_policy(connection)
        .map_err(|policy| SchemaError::Sql(format!("policy unverified: {policy:?}")))?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(sql_error)?;
    match version {
        SUPPORTED_SCHEMA_VERSION => {
            verify_core_schema(connection)?;
            Ok(MigrationOutcome::AlreadyCurrent)
        }
        version if is_older_schema(version) => Err(SchemaError::Inconsistent(
            "older schema requires verified binding upgrade through serve",
        )),
        0 => {
            let tables: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                    [],
                    |row| row.get(0),
                )
                .map_err(sql_error)?;
            if tables != 0 {
                return Err(SchemaError::Inconsistent(
                    "tables present without a migration version",
                ));
            }
            connection
                .execute_batch(MIGRATION_0001_S1)
                .map_err(sql_error)?;
            migrate_v1(connection, None)?;
            migrate_v2(connection, None)?;
            migrate_v3(connection, None)?;
            Ok(MigrationOutcome::Applied)
        }
        _ => Err(SchemaError::Inconsistent("unsupported user_version")),
    }
}

/// Require every core object to exist exactly once.
pub fn verify_core_schema(connection: &Connection) -> Result<(), SchemaError> {
    verify_schema_at(connection, SUPPORTED_SCHEMA_VERSION)
}

pub(super) fn verify_schema_at(connection: &Connection, version: i64) -> Result<(), SchemaError> {
    for (kind, names) in [
        ("table", CORE_TABLES),
        ("index", CORE_INDEXES),
        ("trigger", CORE_TRIGGERS),
        (
            "index",
            if version >= 4 {
                &["entry_library_name"]
            } else {
                &[]
            },
        ),
        ("table", if version >= 3 { &["share_link"] } else { &[] }),
        (
            "index",
            if version >= 3 {
                &["share_link_owner", "share_link_entry", "share_link_expiry"]
            } else {
                &[]
            },
        ),
        (
            "trigger",
            if version >= 3 {
                &[
                    "share_link_disable",
                    "share_link_replace",
                    "share_link_trash",
                ]
            } else {
                &[]
            },
        ),
    ] {
        for name in names {
            let found: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE type = ?1 AND name = ?2",
                    params![kind, name],
                    |row| row.get(0),
                )
                .map_err(sql_error)?;
            if found != 1 {
                return Err(SchemaError::MissingObject(format!("{kind} {name}")));
            }
        }
    }
    Ok(())
}

/// Require a clean integrity and foreign-key check.
pub fn recovery_checks(connection: &Connection) -> Result<(), SchemaError> {
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(sql_error)?;
    if integrity != "ok" {
        return Err(SchemaError::Integrity);
    }
    let violations: i64 = connection
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .map_err(sql_error)?;
    if violations != 0 {
        return Err(SchemaError::ForeignKeyViolation);
    }
    Ok(())
}

/// Insert the installation identity in its own transaction.
pub fn insert_installation_row(
    connection: &Connection,
    identity: &InstallationIdentity,
    setup_verifier: [u8; 32],
) -> Result<(), SchemaError> {
    let transaction = connection.unchecked_transaction().map_err(sql_error)?;
    transaction
        .execute(
            "INSERT INTO installation (
                singleton, installation_id, root_id, db_id,
                schema_version, layout_version, name_key_version, key_version,
                initialized, recovery_epoch, setup_verifier, setup_consumed
            ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 0, ?8, 0)",
            params![
                identity.installation_id,
                identity.root_id,
                identity.db_id,
                identity.schema_version,
                identity.layout_version,
                identity.name_key_version,
                identity.key_version,
                setup_verifier,
            ],
        )
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)
}

/// Upgrade metadata atomically, optionally recording the verified prior manifest.
pub fn migrate_v1(
    connection: &Connection,
    manifest_hash: Option<&[u8]>,
) -> Result<(), SchemaError> {
    migrate(
        connection,
        2,
        include_str!("../../migrations/sqlite/0002_removal.sql"),
        manifest_hash,
    )
}

pub fn migrate_v2(
    connection: &Connection,
    manifest_hash: Option<&[u8]>,
) -> Result<(), SchemaError> {
    verify_schema_at(connection, 2)?;
    migrate(
        connection,
        3,
        include_str!("../../migrations/sqlite/0003_share_links.sql"),
        manifest_hash,
    )
}

pub fn migrate_v3(
    connection: &Connection,
    manifest_hash: Option<&[u8]>,
) -> Result<(), SchemaError> {
    verify_schema_at(connection, 3)?;
    migrate(
        connection,
        4,
        include_str!("../../migrations/sqlite/0004_private_libraries.sql"),
        manifest_hash,
    )
}

fn migrate(
    connection: &Connection,
    next: i64,
    sql: &str,
    manifest_hash: Option<&[u8]>,
) -> Result<(), SchemaError> {
    let tx = connection.unchecked_transaction().map_err(sql_error)?;
    tx.execute_batch(sql).map_err(sql_error)?;
    if let Some(hash) = manifest_hash {
        tx.execute(
            "INSERT INTO binding_transition(singleton,legacy_manifest_hash) VALUES(1,?1)
             ON CONFLICT(singleton) DO UPDATE SET legacy_manifest_hash=excluded.legacy_manifest_hash",
            [hash],
        )
        .map_err(sql_error)?;
    }
    verify_schema_at(&tx, next)?;
    tx.commit().map_err(sql_error)
}
