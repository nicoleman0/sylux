use std::{
    io::{self, IsTerminal, Read},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    domain::authz::{MAX_PASSWORD_BYTES, PasswordLimiter, validate_password},
    persistence::{
        ownership,
        payload::{PayloadError, PayloadFault, PayloadVerifier},
        root, schema,
        sqlite::DEFAULT_BUSY_TIMEOUT,
    },
};

const VERIFY_PAGE: i64 = 64;

/// Counts from `verify_payloads`. Damaged payloads are reported through the
/// callback as found, so a killed run keeps its findings.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PayloadReport {
    pub checked: u64,
    pub damaged: u64,
}

impl PayloadReport {
    pub fn fault_label(fault: PayloadFault) -> &'static str {
        match fault {
            PayloadFault::Missing => "missing",
            PayloadFault::LengthMismatch => "length_mismatch",
            PayloadFault::DigestMismatch => "digest_mismatch",
            PayloadFault::NotPrivateFile => "not_private_file",
            PayloadFault::InvalidId | PayloadFault::Io(_) => "unreadable",
        }
    }
}

/// Check every payload against the length and SHA-256 recorded in metadata.
/// Takes the writer lock like recovery, but opens SQLite read-only and
/// creates nothing. Damage goes to `on_damaged` (blob IDs are random hex, safe
/// to log); a failing callback aborts the run.
pub fn verify_payloads(
    root_path: &Path,
    mut on_damaged: impl FnMut(&str, PayloadFault) -> io::Result<()>,
) -> Result<PayloadReport, &'static str> {
    verify_recovery_owner(root_path, rustix::process::geteuid().as_raw()).map_err(
        |code| match code {
            "recovery_service_identity_required" => "verify_service_identity_required",
            _ => "verify_root_invalid",
        },
    )?;
    let _guard = ownership::acquire_writer_lock(root_path).map_err(|error| match error {
        ownership::OwnershipError::LockHeld => "service_running_stop_before_verify",
        _ => "verify_root_invalid",
    })?;
    let manifest = root::read_manifest(root_path).map_err(|_| "verify_binding_invalid")?;
    if manifest.schema_version == 1 {
        return Err("verify_schema_upgrade_required_run_serve");
    }
    root::verify_protected_material(root_path).map_err(|_| "verify_binding_invalid")?;
    // Declared after ownership so every return closes SQLite before unlocking.
    let bound = root::open_bound_database_read_only(root_path, DEFAULT_BUSY_TIMEOUT)
        .map_err(|_| "verify_database_unavailable")?;
    root::verify_binding(&bound, root_path, &manifest).map_err(|_| "verify_binding_invalid")?;
    let version: i64 = bound
        .connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| "verify_schema_invalid")?;
    if version != schema::SUPPORTED_SCHEMA_VERSION {
        return Err("verify_schema_unsupported");
    }
    schema::verify_core_schema(&bound.connection).map_err(|_| "verify_schema_invalid")?;
    schema::recovery_checks(&bound.connection).map_err(|_| "verify_integrity_failed")?;

    // A root that never served has no payload directory; its blobs are missing.
    let verifier = match PayloadVerifier::open(root_path) {
        Ok(verifier) => Some(verifier),
        Err(PayloadError::Missing) => None,
        Err(_) => return Err("verify_payload_store_invalid"),
    };
    let mut statement = bound
        .connection
        .prepare(
            "SELECT blob_id,length,sha256 FROM blob WHERE blob_id>?1 AND lifecycle='live' ORDER BY blob_id LIMIT ?2",
        )
        .map_err(|_| "verify_metadata_unreadable")?;
    let mut report = PayloadReport::default();
    let mut after = String::new();
    loop {
        let page = statement
            .query_map(params![after, VERIFY_PAGE], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
            .map_err(|_| "verify_metadata_unreadable")?;
        let Some((last, _, _)) = page.last() else {
            break;
        };
        after = last.clone();
        for (blob_id, length, digest) in page {
            // Schema CHECK constraints guarantee these; a violation means the database was altered.
            let length = u64::try_from(length).map_err(|_| "verify_metadata_invalid")?;
            let digest = <[u8; 32]>::try_from(digest).map_err(|_| "verify_metadata_invalid")?;
            report.checked += 1;
            let result = match &verifier {
                Some(verifier) => verifier.verify(&blob_id, length, digest),
                None => Err(PayloadFault::Missing),
            };
            if let Err(fault) = result {
                if fault == PayloadFault::InvalidId {
                    return Err("verify_metadata_invalid");
                }
                report.damaged += 1;
                on_damaged(&blob_id, fault).map_err(|_| "verify_output_failed")?;
            }
        }
    }
    Ok(report)
}

pub async fn reset_password_from_stdin(root_path: &Path, handle: &str) -> Result<(), &'static str> {
    verify_recovery_owner(root_path, rustix::process::geteuid().as_raw())?;
    let _guard = ownership::acquire_writer_lock(root_path).map_err(|error| match error {
        ownership::OwnershipError::LockHeld => "service_running_stop_before_recovery",
        _ => "recovery_root_invalid",
    })?;
    let manifest = root::read_manifest(root_path).map_err(|_| "recovery_binding_invalid")?;
    if manifest.schema_version == 1 {
        return Err("recovery_schema_upgrade_required_run_serve");
    }
    root::verify_protected_material(root_path).map_err(|_| "recovery_binding_invalid")?;
    // Declared after ownership so every return closes SQLite before unlocking.
    let mut bound = root::open_bound_database(root_path, DEFAULT_BUSY_TIMEOUT)
        .map_err(|_| "recovery_database_unavailable")?;
    root::verify_binding(&bound, root_path, &manifest).map_err(|_| "recovery_binding_invalid")?;
    let version: i64 = bound
        .connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| "recovery_schema_invalid")?;
    if version != schema::SUPPORTED_SCHEMA_VERSION {
        return Err("recovery_schema_unsupported");
    }
    schema::verify_core_schema(&bound.connection).map_err(|_| "recovery_schema_invalid")?;
    schema::recovery_checks(&bound.connection).map_err(|_| "recovery_integrity_failed")?;
    let setup_consumed: i64 = bound
        .connection
        .query_row(
            "SELECT setup_consumed FROM installation WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| "recovery_setup_invalid")?;
    if setup_consumed != 1 {
        return Err("recovery_setup_incomplete");
    }
    let account_id: Option<String> = bound
        .connection
        .query_row(
            "SELECT account_id FROM account WHERE handle=?1 AND role='administrator'",
            [handle],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "recovery_target_invalid")?;
    let account_id = account_id.ok_or("recovery_target_invalid")?;
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Err("password_requires_nonterminal_stdin");
    }
    let password = read_password(stdin.lock())?;
    let verifier = PasswordLimiter::default()
        .hash_password(password)
        .await
        .ok_or("password_hash_failed")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .ok_or("recovery_clock_invalid")?;
    let tx = bound
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "recovery_transaction_failed")?;
    let changed = tx
        .execute(
            "UPDATE account SET password_verifier=?1,enabled=1,credential_epoch=credential_epoch+1
         WHERE account_id=?2 AND role='administrator' AND credential_epoch<9223372036854775807",
            params![verifier, account_id],
        )
        .map_err(|_| "recovery_transaction_failed")?;
    if changed != 1 {
        return Err("recovery_target_invalid");
    }
    tx.execute(
        "UPDATE session SET revoked=1 WHERE account_id=?1",
        [&account_id],
    )
    .map_err(|_| "recovery_transaction_failed")?;
    tx.execute(
        "INSERT INTO event(actor_account_id,action,occurred_at,outcome)
                VALUES(NULL,'admin.password_reset',?1,'ok')",
        [now],
    )
    .map_err(|_| "recovery_transaction_failed")?;
    // A failed COMMIT can have an uncertain outcome. Never replay this effect.
    tx.commit()
        .map_err(|_| "recovery_commit_uncertain_restart_and_test_credentials_before_retry")?;
    Ok(())
}

fn verify_recovery_owner(root_path: &Path, effective_uid: u32) -> Result<(), &'static str> {
    let directory = rustix::fs::open(
        root_path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| "recovery_root_invalid")?;
    let stat = rustix::fs::fstat(&directory).map_err(|_| "recovery_root_invalid")?;
    if stat.st_uid != effective_uid {
        return Err("recovery_service_identity_required");
    }
    Ok(())
}

fn read_password(reader: impl Read) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_PASSWORD_BYTES + 3) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "password_stdin_read_failed")?;
    if bytes.ends_with(b"\n") {
        bytes.pop();
        if bytes.ends_with(b"\r") {
            bytes.pop();
        }
    }
    let password = std::str::from_utf8(&bytes).map_err(|_| "password_input_invalid")?;
    if password.contains(['\n', '\r']) || !validate_password(password) {
        return Err("password_input_invalid");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_requires_root_owner_identity_including_for_root() {
        use std::{fs, os::unix::fs::DirBuilderExt};
        let mut token = [0u8; 16];
        getrandom::fill(&mut token).unwrap();
        let suffix: String = token.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("sylux-recovery-owner-{suffix}"));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let owner = rustix::process::geteuid().as_raw();
        assert_eq!(verify_recovery_owner(&path, owner), Ok(()));
        let wrong_uid = if owner == 0 { 1 } else { 0 };
        assert_eq!(
            verify_recovery_owner(&path, wrong_uid),
            Err("recovery_service_identity_required")
        );
        assert!(fs::read_dir(&path).unwrap().next().is_none());
        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn stdin_preserves_spaces_and_strips_only_one_line_ending() {
        for suffix in ["", "\n", "\r\n"] {
            assert_eq!(
                read_password(format!("  recovery password  {suffix}").as_bytes()).unwrap(),
                b"  recovery password  "
            );
        }
        for invalid in [
            b"".as_slice(),
            b"short",
            b"recovery password\n\n",
            b"recovery\npassword",
            b"recovery password\r",
            b"recovery password\xff",
        ] {
            assert!(read_password(invalid).is_err());
        }
        assert!(read_password("x".repeat(MAX_PASSWORD_BYTES + 3).as_bytes()).is_err());
    }
}
