//! Core schema migration and verified startup checks (s1-design-review §4).
//!
//! The migration source of truth is `migrations/sqlite/0001_s1.sql`, embedded
//! here at compile time and applied as the single transaction the file
//! itself declares (`BEGIN IMMEDIATE` … `COMMIT`, with `PRAGMA user_version`
//! inside — SQLite rolls the version back with the transaction). No schema
//! change can run through a connection whose policy has not been verified.

use rusqlite::{Connection, params};

use super::{root::InstallationIdentity, sqlite};

/// Versions this build understands and serves. A bound database reporting
/// anything else refuses startup instead of migrating guesses.
pub const SUPPORTED_SCHEMA_VERSION: i64 = 1;
pub const SUPPORTED_LAYOUT_VERSION: i64 = 1;
pub const SUPPORTED_NAME_KEY_VERSION: i64 = 1;
pub const SUPPORTED_KEY_VERSION: i64 = 1;

/// The complete S1 core schema, applied as one transaction (see the file).
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
];

#[derive(Debug, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// The migration transaction ran and was committed.
    Applied,
    /// The database already reports the supported version and a complete
    /// core schema; nothing was changed.
    AlreadyCurrent,
}

/// Why a schema operation failed. Codes are redacted; SQL messages come from
/// SQLite itself and carry no user values.
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
    /// Redacted machine reason for contract events and diagnostics.
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

/// Bring the database to the supported schema version, or verify it is
/// already there. Runs only after the connection policy read-back has been
/// verified; a database whose tracker disagrees with its contents refuses
/// instead of being "repaired".
pub fn apply_migrations(connection: &Connection) -> Result<MigrationOutcome, SchemaError> {
    // Migration gate: no schema change may run through an unverified policy.
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
            verify_core_schema(connection)?;
            Ok(MigrationOutcome::Applied)
        }
        _ => Err(SchemaError::Inconsistent("unsupported user_version")),
    }
}

/// Inspect `sqlite_schema` and require every core object to exist exactly
/// once. Schema inspection is part of startup verification, not a format
/// guess from a table count.
pub fn verify_core_schema(connection: &Connection) -> Result<(), SchemaError> {
    for (kind, names) in [
        ("table", CORE_TABLES),
        ("index", CORE_INDEXES),
        ("trigger", CORE_TRIGGERS),
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

/// Fail-closed startup recovery checks: `PRAGMA integrity_check` must report
/// `ok` and `PRAGMA foreign_key_check` must report no rows. Results are
/// inspected, never inferred from a command exit code.
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

/// Insert the bound installation row during host initialization. Runs in its
/// own transaction; a crash before it leaves a database with no installation
/// row, which serving refuses — never a half-bound root.
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
