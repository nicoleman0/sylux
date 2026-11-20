//! Verified local backup from a single SQLite read snapshot.
use crate::persistence::{
    instance::InstanceSettings,
    ownership,
    payload::{
        ExportCopyError, PAYLOADS, PayloadFault, PayloadVerifier, STAGING, staging_ids, validate_id,
    },
    reconcile, root, schema,
    sqlite::{self, DEFAULT_BUSY_TIMEOUT, SqliteWorkers, WorkerConfig},
};
use rusqlite::{Connection, MAIN_DB, backup::Progress};
use rustix::fs::{CWD, RenameFlags, renameat_with};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// Publish a private, verified root at a new or empty destination.
pub async fn create(source: &Path, destination: &Path) -> Result<u64, String> {
    let source = fs::canonicalize(source).map_err(|e| io_error("source", e))?;
    let destination = Destination::validate(&source, destination)?;
    let manifest = root::read_manifest(&source).map_err(binding_error)?;
    let (key, authorization) = root::backup_secrets(&source).map_err(binding_error)?;

    let scratch = destination.stage()?;
    let output = &scratch.0;
    for name in [root::METADATA_DIR, root::SECRETS_DIR, PAYLOADS, STAGING] {
        private_dir(&output.join(name))?;
    }
    let db_path = output.join(root::METADATA_DIR).join(root::DB_NAME);
    snapshot(&source, &manifest, &db_path)?;

    let manifest_bytes =
        serde_json::to_vec(&manifest).map_err(|_| "backup_manifest_encode_failed")?;
    write_private(&output.join(root::MANIFEST_NAME), &manifest_bytes)?;
    let secrets = output.join(root::SECRETS_DIR);
    write_private(&secrets.join(root::KEY_NAME), &key)?;
    write_private(
        &secrets.join(root::SETUP_AUTHORIZATION_NAME),
        &authorization,
    )?;

    let settings = InstanceSettings::open(&source).map_err(|e| io_error("settings", e))?;
    for (name, bytes) in settings
        .backup_files()
        .map_err(|e| io_error("settings", e))?
    {
        write_private(&output.join(root::METADATA_DIR).join(name), &bytes)?;
    }
    InstanceSettings::open(output).map_err(|e| io_error("verify_settings", e))?;

    let bound = root::open_bound_database(output, DEFAULT_BUSY_TIMEOUT).map_err(binding_error)?;
    root::verify_binding(&bound, output, &manifest).map_err(binding_error)?;
    verify_metadata(&bound.connection, &authorization)?;
    let count = copy_payloads(&bound.connection, &source, output)?;

    // Acknowledged partial bodies can still change on the live root. Restored
    // admitted uploads can start again at zero using their existing tickets.
    bound
        .connection
        .execute(
            "UPDATE upload_attempt SET bytes_received=0 WHERE bytes_received!=0
         AND mutation_id IN (SELECT mutation_id FROM mutation WHERE state='admitted')",
            [],
        )
        .map_err(|e| sql_error("reset_partial_progress", e))?;

    let _ownership = ownership::acquire_or_create_writer_lock(output)
        .map_err(|e| format!("backup_output_lock_failed: {}", e.reason()))?;
    let workers = SqliteWorkers::from_writer(bound.connection, &db_path, WorkerConfig::default())
        .map_err(|e| format!("backup_verification_open_failed: {}", e.reason()))?;
    // This also reads back the copied payloads from the destination.
    let recovery = reconcile::reconcile(&workers, output).await;
    workers
        .shutdown()
        .await
        .map_err(|e| format!("backup_verification_close_failed: {}", e.reason()))?;
    if !recovery.is_ok_and(|report| report.ready) {
        return Err("backup_output_failed_startup_reconciliation".into());
    }

    let bound = root::open_bound_database(output, DEFAULT_BUSY_TIMEOUT).map_err(binding_error)?;
    sqlite::wal_checkpoint_truncate(&bound.connection)
        .map_err(|e| format!("backup_output_checkpoint_failed: {}", e.reason()))?;
    bound
        .connection
        .close()
        .map_err(|(_, e)| sql_error("close", e))?;
    destination.publish(scratch)?;
    Ok(count)
}

fn snapshot(
    source: &Path,
    manifest: &root::InstallationIdentity,
    output: &Path,
) -> Result<(), String> {
    let bound =
        root::open_bound_database_read_only(source, DEFAULT_BUSY_TIMEOUT).map_err(binding_error)?;
    bound
        .connection
        .execute_batch("BEGIN DEFERRED")
        .map_err(|e| sql_error("snapshot", e))?;
    // The installation read pins the WAL snapshot until the metadata copy ends.
    root::verify_binding(&bound, source, manifest).map_err(binding_error)?;
    private_file(output)?;
    bound
        .connection
        .backup(MAIN_DB, output, None::<fn(Progress)>)
        .map_err(|e| sql_error("snapshot", e))
}

fn verify_metadata(db: &Connection, authorization: &[u8]) -> Result<(), String> {
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| sql_error("schema_version", e))?;
    if version != schema::SUPPORTED_SCHEMA_VERSION {
        return Err("backup_schema_unsupported".into());
    }
    schema::verify_core_schema(db).map_err(|e| format!("backup_schema_invalid: {}", e.reason()))?;
    let (consumed, verifier): (bool, Vec<u8>) = db
        .query_row(
            "SELECT setup_consumed, setup_verifier FROM installation WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| sql_error("setup_verifier", e))?;
    let expected: [u8; 32] = Sha256::digest(authorization).into();
    if !consumed && verifier != expected {
        return Err("backup_setup_authorization_does_not_match_snapshot".into());
    }
    Ok(())
}

fn copy_payloads(db: &Connection, source: &Path, output: &Path) -> Result<u64, String> {
    // A blob in removal cleanup is already logically gone at this snapshot.
    let mut statement = db
        .prepare(
            "SELECT blob_id, shard, length, sha256, 0 FROM blob WHERE lifecycle='live'
         UNION ALL
         SELECT a.attempt_id, a.staging_path, a.completed_length, a.completed_sha256, 1
         FROM mutation m JOIN upload_attempt a ON a.mutation_id=m.mutation_id
         WHERE m.state='publishing' AND a.completed_length=m.completed_length
           AND a.completed_sha256=m.completed_sha256",
        )
        .map_err(|e| sql_error("payload_inventory", e))?;
    let mut rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })
        .map_err(|e| sql_error("payload_inventory", e))?;
    let Some(first) = rows.next() else {
        return Ok(0);
    };
    let input = PayloadVerifier::open(source)
        .map_err(|e| format!("backup_payload_store_invalid: {e:?}"))?;
    let mut copied = 0;
    for row in std::iter::once(first).chain(rows) {
        let (id, location, length, digest, publishing) =
            row.map_err(|e| sql_error("payload_inventory", e))?;
        let (blob, staging) = if publishing {
            let (attempt, blob) =
                staging_ids(&location).map_err(|_| "backup_invalid_staging_name")?;
            if attempt != id {
                return Err("backup_invalid_staging_attempt".into());
            }
            (blob, Some(location.as_str()))
        } else {
            if validate_id(&id).is_err() || id.get(..2) != Some(&location) {
                return Err("backup_invalid_blob_identity".into());
            }
            (id.as_str(), None)
        };
        let length = u64::try_from(length).map_err(|_| "backup_invalid_payload_length")?;
        let digest = <[u8; 32]>::try_from(digest).map_err(|_| "backup_invalid_payload_digest")?;
        let shard = output.join(PAYLOADS).join(&blob[..2]);
        match fs::DirBuilder::new().mode(0o700).create(&shard) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(io_error("mkdir", error)),
        }
        let mut file = private_file(&shard.join(blob))?;
        let mut result = input.copy_verified_final(blob, length, digest, &mut file);
        if let (Err(ExportCopyError::Payload(PayloadFault::Missing)), Some(name)) =
            (&result, staging)
        {
            result =
                PayloadVerifier::copy_verified_staging(source, name, length, digest, &mut file);
            if matches!(result, Err(ExportCopyError::Payload(PayloadFault::Missing))) {
                // The finalized bytes may have crossed the atomic rename.
                result = input.copy_verified_final(blob, length, digest, &mut file);
            }
        }
        result.map_err(|e| match e {
            ExportCopyError::Payload(fault) => format!(
                "backup_payload_unavailable blob={blob} reason={}",
                crate::admin::PayloadReport::fault_label(fault)
            ),
            ExportCopyError::Destination(error) => io_error("write", error),
        })?;
        file.sync_all().map_err(|e| io_error("sync", e))?;
        copied += 1;
    }
    Ok(copied)
}

struct Destination {
    path: PathBuf,
    existing: Option<(u64, u64)>,
}

impl Destination {
    fn validate(source: &Path, path: &Path) -> Result<Self, String> {
        let (path, existing) = match fs::canonicalize(path) {
            Ok(path) => {
                let identity = directory_identity(&path)?;
                (path, Some(identity))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let name = path.file_name().ok_or("backup_destination_invalid")?;
                let parent = path
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                let parent =
                    fs::canonicalize(parent).map_err(|e| io_error("destination_parent", e))?;
                (parent.join(name), None)
            }
            Err(e) => return Err(io_error("destination", e)),
        };
        if path.starts_with(source) || source.starts_with(&path) {
            return Err("backup_destination_overlaps_source".into());
        }
        let source_id = directory_identity(source)?;
        if existing == Some(source_id) {
            return Err("backup_destination_overlaps_source".into());
        }
        for ancestor in path
            .parent()
            .ok_or("backup_destination_invalid")?
            .ancestors()
        {
            if directory_identity(ancestor)? == source_id {
                return Err("backup_destination_overlaps_source".into());
            }
        }
        if existing.is_some() {
            verify_empty(&path)?;
        } else if fs::symlink_metadata(&path).is_ok() {
            return Err("backup_destination_invalid".into());
        }
        Ok(Self { path, existing })
    }

    fn stage(&self) -> Result<Scratch, String> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|_| "backup_entropy_unavailable")?;
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = self
            .path
            .with_file_name(format!(".sylux-backup-incomplete-{suffix}"));
        private_dir(&path)?;
        Ok(Scratch(path))
    }

    fn publish(self, scratch: Scratch) -> Result<(), String> {
        if let Some(identity) = self.existing {
            if directory_identity(&self.path)? != identity {
                return Err("backup_destination_changed".into());
            }
            verify_empty(&self.path)?;
            // A directory rename cannot replace a non-empty directory.
            fs::rename(&scratch.0, &self.path).map_err(|e| io_error("publish", e))
        } else {
            renameat_with(CWD, &scratch.0, CWD, &self.path, RenameFlags::NOREPLACE)
                .map_err(|e| io_error("publish", e.into()))
        }
    }
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn directory_identity(path: &Path) -> Result<(u64, u64), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io_error("directory", e))?;
    if !metadata.is_dir() {
        return Err("backup_destination_not_directory".into());
    }
    Ok((metadata.dev(), metadata.ino()))
}

fn verify_empty(path: &Path) -> Result<(), String> {
    if fs::read_dir(path)
        .map_err(|e| io_error("destination_read", e))?
        .next()
        .is_some()
    {
        return Err("backup_destination_not_empty".into());
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|e| io_error("mkdir", e))
}

fn private_file(path: &Path) -> Result<File, String> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| io_error("create", e))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = private_file(path)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| io_error("write", e))
}

fn io_error(action: &str, error: io::Error) -> String {
    format!("backup_{action}_failed: {error}")
}

fn sql_error(action: &str, error: rusqlite::Error) -> String {
    format!("backup_{action}_failed: {error}")
}

fn binding_error(error: root::BindingError) -> String {
    format!("backup_binding_invalid: {}", error.reason())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::payload::PayloadStore;
    use rusqlite::params;

    #[test]
    fn payload_inventory_honors_both_sides_of_the_snapshot_removal_boundary() {
        let mut token = [0; 8];
        getrandom::fill(&mut token).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "syl-73-snapshot-{:016x}",
            u64::from_ne_bytes(token)
        ));
        private_dir(&dir).unwrap();
        let scratch = Scratch(dir);
        let source = scratch.0.join("live");
        private_dir(&source).unwrap();
        let initialized = root::initialize_empty_root(&source, DEFAULT_BUSY_TIMEOUT).unwrap();
        let db = root::open_bound_database(&source, DEFAULT_BUSY_TIMEOUT).unwrap();
        let store = PayloadStore::open(&source).unwrap();
        let before_cut = "11223344556677889900112233445566";
        let after_cut = "22334455667788990011223344556677";
        let attempt = "33112233445566778899001122334455";
        let body = b"verified payload bytes";
        let digest: [u8; 32] = Sha256::digest(body).into();
        db.connection.execute(
            "INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,'fixture','administrator',1,0)",
            [attempt],
        ).unwrap();
        db.connection.execute(
            "INSERT INTO entry(entry_id,owner_account_id,kind,original_name,name_key,name_key_version)
             VALUES(?1,?1,'folder','',x'',1)", [attempt],
        ).unwrap();
        for blob in [before_cut, after_cut] {
            let mut staging = store
                .start(&format!("{attempt}-{blob}"), body.len() as u64)
                .unwrap();
            staging.write_chunk(body).unwrap();
            staging.publish(digest).unwrap();
            db.connection
                .execute(
                    "INSERT INTO blob(blob_id,shard,length,sha256) VALUES(?1,substr(?1,1,2),?2,?3)",
                    params![blob, body.len() as i64, digest.as_slice()],
                )
                .unwrap();
            db.connection.execute(
                "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version,state)
                 VALUES(?1,?2,?2,'file',?1,CAST(?1 AS BLOB),1,'deleted')", params![blob,attempt],
            ).unwrap();
            db.connection.execute(
                "INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id)
                 VALUES(?1,?1,?1,?2,?3,0,?4)", params![blob,body.len() as i64,digest.as_slice(),attempt],
            ).unwrap();
        }

        let remove = |blob: &str| {
            let tx = db.connection.unchecked_transaction().unwrap();
            tx.execute(
                "INSERT INTO removal(operation_id,owner_account_id,confirmation_hash,removed_count,committed_at)
                 VALUES(?1,?2,zeroblob(32),1,0)", params![blob,attempt],
            ).unwrap();
            tx.execute(
                "INSERT INTO removal_member(entry_id,operation_id) VALUES(?1,?1)",
                [blob],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO removal_cleanup(blob_id,operation_id,state) VALUES(?1,?1,'pending')",
                [blob],
            )
            .unwrap();
            tx.execute(
                "UPDATE blob SET lifecycle='deleting' WHERE blob_id=?1",
                [blob],
            )
            .unwrap();
            tx.commit().unwrap();
            fs::remove_file(source.join(PAYLOADS).join(&blob[..2]).join(blob)).unwrap();
        };
        remove(before_cut);

        let db_path = scratch.0.join(root::DB_NAME);
        snapshot(&source, &initialized.identity, &db_path).unwrap();
        let snapshot = Connection::open(&db_path).unwrap();
        let output = scratch.0.join("out");
        private_dir(&output).unwrap();
        private_dir(&output.join(PAYLOADS)).unwrap();
        assert_eq!(copy_payloads(&snapshot, &source, &output).unwrap(), 1);
        let copied = output.join(PAYLOADS).join(&after_cut[..2]).join(after_cut);
        assert_eq!(fs::read(&copied).unwrap(), body);

        remove(after_cut);
        assert_eq!(fs::read(copied).unwrap(), body);
        let output = scratch.0.join("retry");
        private_dir(&output).unwrap();
        private_dir(&output.join(PAYLOADS)).unwrap();
        let error = copy_payloads(&snapshot, &source, &output).unwrap_err();
        assert_eq!(
            error,
            format!("backup_payload_unavailable blob={after_cut} reason=missing")
        );
    }
}
