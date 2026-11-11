//! Fail-closed restart reconciliation (s1-design-review §5).
//!
//! The caller must retain the root ownership guard and perform manifest/key/
//! database binding before calling this module. Reconciliation runs before
//! readiness; this module never adopts an orphan or deletes any payload.

use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{OptionalExtension, params};
use rustix::fs::{self as rfs, FileType, Mode, OFlags, RenameFlags};
use sha2::{Digest, Sha256};

use super::{payload::PayloadStore, root, schema, sqlite::SqliteWorkers};

const PAGE: i64 = 64;
const ID_LEN: usize = 32;
type InstallationIdentity = (String, String, String, i64, i64, i64, i64, i64);

/// Outcome assigned to each persisted or filesystem recovery candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryDisposition {
    /// Replay the recorded result for a committed result with matching payload.
    CommittedReplay,
    /// Mark admission-only work interrupted/restartable; bytes stay invisible.
    InterruptedAdmission,
    /// Preserve valid staging so the same-ticket commit may continue.
    StagingPreserved,
    /// Resync a valid final, then reauthorize/precondition-check before resume.
    FinalNeedsResync,
    /// Preserve candidates and fail readiness closed; do not clean up.
    UnknownPreserve,
    /// Require recovery for collision/corrupt/missing-reference cases.
    RecoveryRequired,
    /// Reject definitely-uncommitted expired tickets after reconciliation.
    ExpiredRejected,
}

/// Summary of restart reconciliation outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcileReport {
    pub dispositions: Vec<RecoveryDisposition>,
    pub ready: bool,
}

/// Failure to perform restart reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileError {
    Unavailable,
}

#[derive(Debug)]
struct MutationRow {
    id: String,
    principal: String,
    state: String,
    result_revision: Option<String>,
    completed_length: Option<i64>,
    completed_sha256: Option<Vec<u8>>,
    expires_at: i64,
}

#[derive(Debug)]
struct AttemptRow {
    id: String,
    path: String,
    writer_active: bool,
    completed_length: Option<i64>,
    completed_sha256: Option<Vec<u8>>,
}

#[derive(Debug)]
struct Candidate {
    attempt: AttemptRow,
    blob_id: String,
    staging: bool,
    final_file: bool,
}

/// Scan persisted upload state and payload candidates before serving traffic.
///
/// Reads are paginated and payloads are hashed one at a time using a fixed
/// buffer. SQLite owns WAL recovery: this function never unlinks or edits WAL
/// files. Result/status replay remains subject to current authentication in the
/// coordinator; a no-result final is merely re-synced here and cannot be
/// continued until the request path rechecks authorization and preconditions.
pub async fn reconcile(
    workers: &SqliteWorkers,
    root_path: &Path,
) -> Result<ReconcileReport, ReconcileError> {
    let mut report = ReconcileReport {
        dispositions: Vec::new(),
        ready: true,
    };

    // Recheck binding at this boundary as well as requiring the startup caller
    // to have acquired ownership before reaching it. No root/database repair.
    let manifest = match root::read_manifest(root_path) {
        Ok(identity) if root::verify_protected_material(root_path).is_ok() => identity,
        _ => {
            fail(&mut report, RecoveryDisposition::RecoveryRequired);
            return Ok(report);
        }
    };
    let identity_matches = workers
        .read(move |db| {
            let row: Option<InstallationIdentity> = db
                .query_row(
                    "SELECT installation_id,root_id,db_id,schema_version,layout_version,name_key_version,key_version,initialized FROM installation WHERE singleton=1",
                    [],
                    |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)),
                )
                .optional()?;
            let Some((installation_id, root_id, db_id, schema_v, layout_v, name_v, key_v, initialized)) = row else {
                return Ok(false);
            };
            Ok(initialized == 1
                && installation_id == manifest.installation_id
                && root_id == manifest.root_id
                && db_id == manifest.db_id
                && schema_v == manifest.schema_version
                && layout_v == manifest.layout_version
                && name_v == manifest.name_key_version
                && key_v == manifest.key_version
                && schema_v == schema::SUPPORTED_SCHEMA_VERSION
                && layout_v == schema::SUPPORTED_LAYOUT_VERSION
                && name_v == schema::SUPPORTED_NAME_KEY_VERSION
                && key_v == schema::SUPPORTED_KEY_VERSION)
        })
        .await
        .map_err(|_| ReconcileError::Unavailable)?;
    if !identity_matches {
        fail(&mut report, RecoveryDisposition::RecoveryRequired);
        return Ok(report);
    }

    // Do not infer integrity from SQL command success or inspect only the first
    // result row: walk every reported integrity issue and every FK violation.
    let checks_ok = workers
        .read(|db| {
            schema::verify_core_schema(db).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let mut integrity = db.prepare("PRAGMA integrity_check")?;
            let mut rows = integrity.query([])?;
            let mut saw_row = false;
            while let Some(row) = rows.next()? {
                saw_row = true;
                let value: String = row.get(0)?;
                if value != "ok" {
                    return Ok(false);
                }
            }
            if !saw_row {
                return Ok(false);
            }
            let mut foreign_keys = db.prepare("PRAGMA foreign_key_check")?;
            let mut rows = foreign_keys.query([])?;
            Ok(rows.next()?.is_none())
        })
        .await;
    match checks_ok {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            fail(&mut report, RecoveryDisposition::RecoveryRequired);
            return Ok(report);
        }
    }

    let store = match PayloadStore::open(root_path) {
        Ok(store) => store,
        Err(_) => {
            fail(&mut report, RecoveryDisposition::RecoveryRequired);
            return Ok(report);
        }
    };

    // Verify all authoritative blob references in keyset pages. Each page is
    // bounded; each referenced payload is opened no-follow and streamed once.
    let mut after = String::new();
    loop {
        let cursor = after.clone();
        let page = workers
            .read(move |db| {
                let mut statement = db.prepare(
                    "SELECT blob_id,shard,length,sha256 FROM blob WHERE blob_id>?1 ORDER BY blob_id LIMIT ?2",
                )?;
                let rows = statement.query_map(params![cursor, PAGE], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, Vec<u8>>(3)?))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .await
            .map_err(|_| ReconcileError::Unavailable)?;
        if page.is_empty() {
            break;
        }
        for (blob_id, shard, length, digest) in page {
            after = blob_id.clone();
            let valid_id = valid_id(&blob_id);
            let Ok(length) = u64::try_from(length) else {
                fail(&mut report, RecoveryDisposition::RecoveryRequired);
                continue;
            };
            let Ok(digest) = <[u8; 32]>::try_from(digest) else {
                fail(&mut report, RecoveryDisposition::RecoveryRequired);
                continue;
            };
            if !valid_id || shard != blob_id.get(..2).unwrap_or("") {
                fail(&mut report, RecoveryDisposition::RecoveryRequired);
                continue;
            }
            match store.open_verified(&blob_id, length, digest) {
                Ok(_) => {}
                Err(_) => fail(&mut report, RecoveryDisposition::RecoveryRequired),
            }
        }
        if after.is_empty() {
            break;
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(i64::MAX);

    // Keyset-paginate mutations. Attempt rows are paged separately so a ticket
    // with many retained retries cannot inflate startup memory.
    let mut mutation_after = String::new();
    loop {
        let cursor = mutation_after.clone();
        let page = workers
            .read(move |db| {
                let mut statement = db.prepare(
                    "SELECT mutation_id,principal_account_id,state,result_revision_id,completed_length,completed_sha256,expires_at FROM mutation WHERE mutation_id>?1 ORDER BY mutation_id LIMIT ?2",
                )?;
                let rows = statement.query_map(params![cursor, PAGE], |row| {
                    Ok(MutationRow {
                        id: row.get(0)?, principal: row.get(1)?, state: row.get(2)?,
                        result_revision: row.get(3)?, completed_length: row.get(4)?, completed_sha256: row.get(5)?, expires_at: row.get(6)?,
                    })
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .await
            .map_err(|_| ReconcileError::Unavailable)?;
        if page.is_empty() {
            break;
        }
        for mutation in page {
            mutation_after = mutation.id.clone();
            let attempts = read_attempts(workers, &mutation.id).await?;
            reconcile_mutation(
                workers,
                &store,
                root_path,
                &mutation,
                attempts,
                now,
                &mut report,
            )
            .await?;
        }
        if mutation_after.is_empty() {
            break;
        }
    }

    // Unknown directory entries are evidence, not cleanup targets. Walk one
    // filesystem entry at a time and ask SQLite whether it has an owner; no
    // whole-tree in-memory inventory is built.
    scan_unexpected_entries(workers, root_path, &mut report).await?;
    Ok(report)
}

async fn read_attempts(
    workers: &SqliteWorkers,
    mutation_id: &str,
) -> Result<Vec<AttemptRow>, ReconcileError> {
    // Bound total memory per mutation by retaining only a bounded page at once;
    // the caller processes this collection page-wise in later slices if the
    // attempt count grows. For S1, attempt rows are capped by the admission
    // limits and the page cap below fails closed instead of truncating evidence.
    let id = mutation_id.to_owned();
    workers
        .read(move |db| {
            let mut statement = db.prepare(
                "SELECT attempt_id,staging_path,writer_active,completed_length,completed_sha256 FROM upload_attempt WHERE mutation_id=?1 ORDER BY attempt_no LIMIT ?2",
            )?;
            let rows = statement.query_map(params![id, PAGE], |row| {
                Ok(AttemptRow { id: row.get(0)?, path: row.get(1)?, writer_active: row.get::<_, i64>(2)? != 0, completed_length: row.get(3)?, completed_sha256: row.get(4)? })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        .map_err(|_| ReconcileError::Unavailable)
}

async fn reconcile_mutation(
    workers: &SqliteWorkers,
    store: &PayloadStore,
    root_path: &Path,
    mutation: &MutationRow,
    attempts: Vec<AttemptRow>,
    now: i64,
    report: &mut ReconcileReport,
) -> Result<(), ReconcileError> {
    if !valid_id(&mutation.id) || attempts.len() as i64 >= PAGE {
        fail(report, RecoveryDisposition::RecoveryRequired);
        return Ok(());
    }
    if !matches!(
        mutation.state.as_str(),
        "admitted" | "publishing" | "committed" | "rejected"
    ) {
        fail(report, RecoveryDisposition::UnknownPreserve);
        return Ok(());
    }
    let mut candidates = Vec::with_capacity(attempts.len());
    let mut bad = false;
    for attempt in attempts {
        let Some((attempt_id, blob_id)) = split_staging_name(&attempt.path) else {
            bad = true;
            continue;
        };
        if attempt_id != attempt.id || !valid_id(&attempt.id) {
            bad = true;
            continue;
        }
        let blob_id = blob_id.to_owned();
        let staged = candidate_exists(root_path, "staging", &attempt.path);
        let final_path = root_path
            .join("payloads")
            .join(&blob_id[..2])
            .join(&blob_id);
        let final_file = fs::symlink_metadata(&final_path).is_ok();
        if staged && final_file {
            bad = true;
        }
        candidates.push(Candidate {
            attempt,
            blob_id,
            staging: staged,
            final_file,
        });
    }
    if bad {
        fail(report, RecoveryDisposition::RecoveryRequired);
        return Ok(());
    }

    let expired = now < 0 || now >= mutation.expires_at;
    match mutation.state.as_str() {
        "committed" => {
            let Some(result_revision) = mutation.result_revision.as_deref() else {
                fail(report, RecoveryDisposition::RecoveryRequired);
                return Ok(());
            };
            let id = mutation.id.clone();
            let principal = mutation.principal.clone();
            let revision = result_revision.to_owned();
            let result = workers.read(move |db| {
                db.query_row(
                    "SELECT r.revision_id,r.entry_id,r.blob_id,r.length,r.sha256,r.actor_account_id,m.result_revision_id FROM mutation m JOIN revision r ON r.revision_id=m.result_revision_id WHERE m.mutation_id=?1 AND m.principal_account_id=?2 AND m.state='committed' AND m.result_revision_id=?3",
                    params![id,principal,revision],
                    |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,i64>(3)?,row.get::<_,Vec<u8>>(4)?,row.get::<_,String>(5)?,row.get::<_,String>(6)?)),
                )
            }).await;
            let (revision_id, entry_id, blob_id, length, digest, actor, linked) = match result {
                Ok(row) => row,
                Err(_) => {
                    fail(report, RecoveryDisposition::RecoveryRequired);
                    return Ok(());
                }
            };
            let committed_attempt = candidates.iter().any(|candidate| {
                candidate.blob_id == blob_id
                    && candidate.attempt.completed_length == Some(length)
                    && candidate.attempt.completed_sha256.as_deref() == Some(digest.as_slice())
                    && candidate.final_file
            });
            if revision_id != linked
                || entry_id.is_empty()
                || actor != mutation.principal
                || !valid_id(&blob_id)
                || length < 0
                || !valid_digest(&digest)
                || !committed_attempt
                || mutation.completed_length != Some(length)
                || mutation.completed_sha256.as_deref() != Some(digest.as_slice())
            {
                fail(report, RecoveryDisposition::RecoveryRequired);
            } else {
                push_once(report, RecoveryDisposition::CommittedReplay);
            }
        }
        "admitted" => {
            if candidates.iter().any(|c| {
                c.final_file
                    || c.attempt.completed_length.is_some()
                    || c.attempt.completed_sha256.is_some()
            }) || mutation.completed_length.is_some()
                || mutation.completed_sha256.is_some()
                || candidates.is_empty()
            {
                fail(report, RecoveryDisposition::RecoveryRequired);
                return Ok(());
            }
            let mutation_id = mutation.id.clone();
            let expire = expired;
            workers.transact(move |tx| {
                tx.execute("UPDATE upload_attempt SET writer_active=0,outcome=CASE WHEN ?2 THEN 'rejected' ELSE outcome END WHERE mutation_id=?1 AND writer_active=1", params![mutation_id, expire])?;
                if expire {
                    tx.execute("UPDATE mutation SET state='rejected' WHERE mutation_id=?1 AND state='admitted' AND result_revision_id IS NULL", [mutation_id])?;
                }
                Ok(())
            }).await.map_err(|_| ReconcileError::Unavailable)?;
            push_once(
                report,
                if expired {
                    RecoveryDisposition::ExpiredRejected
                } else {
                    RecoveryDisposition::InterruptedAdmission
                },
            );
        }
        "publishing" => {
            let (Some(length), Some(digest_vec)) = (
                mutation.completed_length,
                mutation.completed_sha256.as_ref(),
            ) else {
                fail(report, RecoveryDisposition::RecoveryRequired);
                return Ok(());
            };
            let Ok(length) = u64::try_from(length) else {
                fail(report, RecoveryDisposition::RecoveryRequired);
                return Ok(());
            };
            let Ok(digest) = <[u8; 32]>::try_from(digest_vec.as_slice()) else {
                fail(report, RecoveryDisposition::RecoveryRequired);
                return Ok(());
            };
            let fixed: Vec<_> = candidates
                .iter()
                .filter(|c| {
                    c.attempt.completed_length == i64::try_from(length).ok()
                        && c.attempt.completed_sha256.as_deref() == Some(digest.as_slice())
                })
                .collect();
            if fixed.len() != 1 {
                fail(report, RecoveryDisposition::RecoveryRequired);
                return Ok(());
            }
            let candidate = fixed[0];
            let staging_name = candidate.attempt.path.clone();
            if candidate.staging {
                if publish_staging_candidate(
                    root_path,
                    &staging_name,
                    &candidate.blob_id,
                    length,
                    digest,
                )
                .is_err()
                {
                    fail(report, RecoveryDisposition::RecoveryRequired);
                    return Ok(());
                }
                match store.open_verified(&candidate.blob_id, length, digest) {
                    Ok(verified) => {
                        if verified.file.sync_all().is_err()
                            || sync_final_directory(root_path, &candidate.blob_id).is_err()
                        {
                            fail(report, RecoveryDisposition::RecoveryRequired);
                            return Ok(());
                        }
                    }
                    Err(_) => {
                        fail(report, RecoveryDisposition::RecoveryRequired);
                        return Ok(());
                    }
                }
                push_once(
                    report,
                    if expired {
                        RecoveryDisposition::ExpiredRejected
                    } else {
                        RecoveryDisposition::FinalNeedsResync
                    },
                );
            } else if candidate.final_file {
                match store.open_verified(&candidate.blob_id, length, digest) {
                    Ok(verified) => {
                        if verified.file.sync_all().is_err()
                            || sync_final_directory(root_path, &candidate.blob_id).is_err()
                        {
                            fail(report, RecoveryDisposition::RecoveryRequired);
                            return Ok(());
                        }
                        push_once(
                            report,
                            if expired {
                                RecoveryDisposition::ExpiredRejected
                            } else {
                                RecoveryDisposition::FinalNeedsResync
                            },
                        );
                    }
                    Err(_) => {
                        fail(report, RecoveryDisposition::RecoveryRequired);
                        return Ok(());
                    }
                }
            } else {
                fail(report, RecoveryDisposition::RecoveryRequired);
                return Ok(());
            }
            if expired {
                let mutation_id = mutation.id.clone();
                workers.transact(move |tx| {
                    tx.execute("UPDATE mutation SET state='rejected' WHERE mutation_id=?1 AND state='publishing' AND result_revision_id IS NULL", [mutation_id])?;
                    Ok(())
                }).await.map_err(|_| ReconcileError::Unavailable)?;
            }
        }
        "rejected" => {
            // A durable rejection never licenses deletion. Unexpected or
            // conflicting candidates are caught by the bounded inventory walk.
            if mutation.result_revision.is_some() {
                fail(report, RecoveryDisposition::RecoveryRequired);
            } else if candidates.iter().any(|c| c.attempt.writer_active) {
                let mutation_id = mutation.id.clone();
                workers.transact(move |tx| {
                    tx.execute("UPDATE upload_attempt SET writer_active=0 WHERE mutation_id=?1 AND writer_active=1", [mutation_id])?;
                    Ok(())
                }).await.map_err(|_| ReconcileError::Unavailable)?;
                push_once(report, RecoveryDisposition::InterruptedAdmission);
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

async fn scan_unexpected_entries(
    workers: &SqliteWorkers,
    root: &Path,
    report: &mut ReconcileReport,
) -> Result<(), ReconcileError> {
    let staging = root.join("staging");
    let entries = match fs::read_dir(&staging) {
        Ok(entries) => entries,
        Err(_) => {
            fail(report, RecoveryDisposition::RecoveryRequired);
            return Ok(());
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            fail(report, RecoveryDisposition::RecoveryRequired);
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => {
                fail(report, RecoveryDisposition::RecoveryRequired);
                continue;
            }
        };
        if metadata.nlink() != 1
            || metadata.permissions().mode() & 0o777 != 0o600
            || split_staging_name(&name).is_none()
        {
            fail(report, RecoveryDisposition::RecoveryRequired);
            continue;
        }
        let query = name.clone();
        let known = workers
            .read(move |db| {
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM upload_attempt WHERE staging_path=?1)",
                    [query],
                    |row| row.get::<_, i64>(0),
                )
            })
            .await
            .map_err(|_| ReconcileError::Unavailable)?;
        if known != 1 {
            fail(report, RecoveryDisposition::RecoveryRequired);
        }
    }

    let payloads = root.join("payloads");
    let shards = match fs::read_dir(&payloads) {
        Ok(shards) => shards,
        Err(_) => {
            fail(report, RecoveryDisposition::RecoveryRequired);
            return Ok(());
        }
    };
    for shard_entry in shards {
        let Ok(shard_entry) = shard_entry else {
            fail(report, RecoveryDisposition::RecoveryRequired);
            continue;
        };
        let shard_name = shard_entry.file_name().to_string_lossy().into_owned();
        if shard_name.len() != 2 || !shard_name.bytes().all(is_hex) {
            fail(report, RecoveryDisposition::RecoveryRequired);
            continue;
        }
        let shard_meta = match fs::symlink_metadata(shard_entry.path()) {
            Ok(metadata) if metadata.is_dir() => metadata,
            _ => {
                fail(report, RecoveryDisposition::RecoveryRequired);
                continue;
            }
        };
        if shard_meta.permissions().mode() & 0o777 != 0o700 {
            fail(report, RecoveryDisposition::RecoveryRequired);
            continue;
        }
        let files = match fs::read_dir(shard_entry.path()) {
            Ok(files) => files,
            Err(_) => {
                fail(report, RecoveryDisposition::RecoveryRequired);
                continue;
            }
        };
        for file_entry in files {
            let Ok(file_entry) = file_entry else {
                fail(report, RecoveryDisposition::RecoveryRequired);
                continue;
            };
            let blob_id = file_entry.file_name().to_string_lossy().into_owned();
            let file_meta = match fs::symlink_metadata(file_entry.path()) {
                Ok(metadata) if metadata.is_file() => metadata,
                _ => {
                    fail(report, RecoveryDisposition::RecoveryRequired);
                    continue;
                }
            };
            if !valid_id(&blob_id)
                || blob_id.get(..2) != Some(shard_name.as_str())
                || file_meta.nlink() != 1
                || file_meta.permissions().mode() & 0o777 != 0o600
            {
                fail(report, RecoveryDisposition::RecoveryRequired);
                continue;
            }
            let query = blob_id.clone();
            let known = workers.read(move |db| {
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM blob WHERE blob_id=?1) OR EXISTS(SELECT 1 FROM upload_attempt WHERE substr(staging_path,34,32)=?1)",
                    [query], |row| row.get::<_,i64>(0),
                )
            }).await.map_err(|_| ReconcileError::Unavailable)?;
            if known != 1 {
                fail(report, RecoveryDisposition::RecoveryRequired);
            }
        }
    }
    Ok(())
}

fn publish_staging_candidate(
    root: &Path,
    staging_name: &str,
    blob_id: &str,
    length: u64,
    digest: [u8; 32],
) -> std::io::Result<()> {
    let root_dir = rfs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    validate_recovery_directory(&root_dir)?;
    let staging_dir = rfs::openat(
        &root_dir,
        "staging",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    validate_recovery_directory(&staging_dir)?;
    let payloads_dir = rfs::openat(
        &root_dir,
        "payloads",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    validate_recovery_directory(&payloads_dir)?;

    let staged_fd = rfs::openat(
        &staging_dir,
        staging_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let stat = rfs::fstat(&staged_fd).map_err(std::io::Error::from)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_nlink != 1
        || stat.st_mode & 0o777 != 0o600
        || stat.st_size < 0
        || stat.st_size as u64 != length
    {
        return Err(std::io::Error::other("staging integrity"));
    }
    let mut staged_file = File::from(staged_fd);
    if hash_reader(&mut staged_file)? != digest {
        return Err(std::io::Error::other("staging digest"));
    }
    staged_file.sync_all()?;
    rfs::fsync(&staging_dir).map_err(std::io::Error::from)?;

    let shard_name = &blob_id[..2];
    let shard_created = match rfs::mkdirat(&payloads_dir, shard_name, Mode::RWXU) {
        Ok(()) => true,
        Err(error) if std::io::Error::from(error).kind() == std::io::ErrorKind::AlreadyExists => {
            false
        }
        Err(error) => return Err(std::io::Error::from(error)),
    };
    if shard_created {
        rfs::fsync(&payloads_dir).map_err(std::io::Error::from)?;
    }
    let shard = rfs::openat(
        &payloads_dir,
        shard_name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    validate_recovery_directory(&shard)?;
    rfs::renameat_with(
        &staging_dir,
        staging_name,
        &shard,
        blob_id,
        RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)?;
    rfs::fsync(&shard).map_err(std::io::Error::from)?;
    if shard_created {
        rfs::fsync(&payloads_dir).map_err(std::io::Error::from)?;
    }
    Ok(())
}

fn validate_recovery_directory(directory: &impl std::os::fd::AsFd) -> std::io::Result<()> {
    let stat = rfs::fstat(directory).map_err(std::io::Error::from)?;
    if !FileType::from_raw_mode(stat.st_mode).is_dir() || stat.st_mode & 0o777 != 0o700 {
        return Err(std::io::Error::other("recovery directory integrity"));
    }
    Ok(())
}

fn sync_final_directory(root: &Path, blob_id: &str) -> std::io::Result<()> {
    let payloads = rfs::open(
        root.join("payloads"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let shard_name = &blob_id[..2];
    let shard = rfs::openat(
        &payloads,
        shard_name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    rfs::fsync(&shard).map_err(std::io::Error::from)
}

fn candidate_exists(root: &Path, directory: &str, name: &str) -> bool {
    fs::symlink_metadata(root.join(directory).join(name)).is_ok()
}

fn split_staging_name(name: &str) -> Option<(&str, &str)> {
    if name.len() != ID_LEN * 2 + 1 || name.as_bytes().get(ID_LEN) != Some(&b'-') {
        return None;
    }
    let (attempt, blob) = name.split_at(ID_LEN);
    let blob = &blob[1..];
    (valid_id(attempt) && valid_id(blob)).then_some((attempt, blob))
}

fn valid_id(value: &str) -> bool {
    value.len() == ID_LEN && value.bytes().all(is_hex)
}
fn is_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}
fn valid_digest(digest: &[u8]) -> bool {
    digest.len() == 32
}

fn hash_reader(reader: &mut impl Read) -> std::io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().into())
}

fn fail(report: &mut ReconcileReport, disposition: RecoveryDisposition) {
    report.ready = false;
    push_once(report, disposition);
}

fn push_once(report: &mut ReconcileReport, disposition: RecoveryDisposition) {
    if !report.dispositions.contains(&disposition) {
        report.dispositions.push(disposition);
    }
}
