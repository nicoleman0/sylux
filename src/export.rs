//! Offline account export. Source ownership is retained until every reader closes.
use crate::persistence::{
    ownership,
    payload::{ExportCopyError, PayloadVerifier},
    root, schema,
};
use rusqlite::{Connection, OptionalExtension, params};
use rustix::fs::{self as rfs, Dir, Mode, OFlags};
use std::{
    collections::HashSet,
    fs::{self, File},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::DirBuilderExt,
    },
    path::{Path, PathBuf},
};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Writes plain files into a new or empty target. Failure leaves an incomplete target.
pub fn export_account(source: &Path, handle: &str, target: &Path) -> Result<u64, String> {
    if fs::symlink_metadata(source).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("export_source_symlink".into());
    }
    let source = fs::canonicalize(source).map_err(|_| "export_source_invalid")?;
    let uid = rustix::process::geteuid().as_raw();
    use std::os::unix::fs::MetadataExt;
    if fs::metadata(&source)
        .map_err(|_| "export_source_invalid")?
        .uid()
        != uid
    {
        return Err("export_service_identity_required".into());
    }
    let _guard = ownership::acquire_writer_lock(&source).map_err(|e| match e {
        ownership::OwnershipError::LockHeld => "service_running_stop_before_export",
        _ => "export_source_invalid",
    })?;
    let target = absolute_target(target)?;
    if target.starts_with(&source) || source.starts_with(&target) {
        return Err("export_destination_overlaps_source".into());
    }
    match fs::symlink_metadata(&target) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => (),
        Ok(_) => return Err("export_destination_not_directory".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::DirBuilder::new()
            .mode(0o700)
            .create(&target)
            .map_err(|_| "export_destination_create_failed")?,
        Err(_) => return Err("export_destination_unavailable".into()),
    }
    let out = open_dir(&target)?;
    for item in Dir::read_from(&out).map_err(|_| "export_destination_unreadable")? {
        let item = item.map_err(|_| "export_destination_unreadable")?;
        if !matches!(item.file_name().to_bytes(), b"." | b"..") {
            return Err("export_destination_not_empty".into());
        }
    }
    let manifest = root::read_manifest(&source).map_err(|_| "export_binding_invalid")?;
    root::verify_protected_material(&source).map_err(|_| "export_binding_invalid")?;
    let mut token = [0; 16];
    getrandom::fill(&mut token).map_err(|_| "export_random_failed")?;
    let suffix: String = token.iter().map(|b| format!("{b:02x}")).collect();
    let scratch = target.join(format!(".sylux-export-{suffix}"));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&scratch)
        .map_err(|_| "export_scratch_create_failed")?;
    let scratch = Scratch(scratch);
    let bound = root::snapshot_bound_database(&source, &scratch.0)
        .map_err(|_| "export_database_unavailable")?;
    root::verify_binding(&bound, &source, &manifest).map_err(|_| "export_binding_invalid")?;
    let db = &bound.connection;
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|_| "export_schema_invalid")?;
    if version != schema::SUPPORTED_SCHEMA_VERSION {
        return Err("export_schema_unsupported".into());
    }
    schema::verify_core_schema(db).map_err(|_| "export_schema_invalid")?;
    schema::recovery_checks(db).map_err(|_| "export_integrity_failed")?;
    let owner: String = db
        .query_row(
            "SELECT account_id FROM account WHERE handle=?1",
            [handle],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| "export_account_invalid")?
        .ok_or("export_account_not_found")?;
    let entry: String = db.query_row("SELECT entry_id FROM entry WHERE owner_account_id=?1 AND parent_id IS NULL AND kind='folder' AND state='active'", [&owner], |r| r.get(0))
        .map_err(|_| "export_account_root_invalid")?;
    let scratch_name = scratch.0.file_name().ok_or("export_destination_invalid")?;
    walk(
        db,
        &source,
        &owner,
        &entry,
        &out,
        0,
        Some(scratch_name.as_bytes()),
    )
}

fn absolute_target(target: &Path) -> Result<PathBuf, String> {
    let bytes = target.as_os_str().as_bytes();
    let end = bytes.iter().rposition(|b| *b != b'/').map_or(1, |i| i + 1);
    let trimmed = std::ffi::OsString::from_vec(bytes[..end.min(bytes.len())].to_vec());
    let target = Path::new(&trimmed);
    if fs::symlink_metadata(target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("export_destination_symlink".into());
    }
    if target.exists() {
        return fs::canonicalize(target).map_err(|_| "export_destination_invalid".into());
    }
    let name = target.file_name().ok_or("export_destination_invalid")?;
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(fs::canonicalize(parent)
        .map_err(|_| "export_destination_parent_missing")?
        .join(name))
}
fn open_dir(path: &Path) -> Result<File, String> {
    rfs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| "export_destination_open_failed".into())
}
fn walk(
    db: &Connection,
    source: &Path,
    owner: &str,
    parent: &str,
    out: &File,
    depth: u32,
    scratch: Option<&[u8]>,
) -> Result<u64, String> {
    if depth > 128 {
        return Err("export_namespace_depth_invalid".into());
    }
    let mut stmt = db.prepare("SELECT e.entry_id,e.original_name,e.kind,r.blob_id,r.length,r.sha256,b.length,b.sha256,b.lifecycle
        FROM entry e LEFT JOIN revision r ON r.revision_id=e.current_revision_id AND r.entry_id=e.entry_id
        LEFT JOIN blob b ON b.blob_id=r.blob_id
        WHERE e.owner_account_id=?1 AND e.parent_id=?2 AND e.state='active' ORDER BY e.entry_id")
        .map_err(|_| "export_metadata_unreadable")?;
    let mut rows = stmt
        .query(params![owner, parent])
        .map_err(|_| "export_metadata_unreadable")?;
    let mut count = 0;
    let mut names = HashSet::new();
    while let Some(row) = rows.next().map_err(|_| "export_metadata_unreadable")? {
        let id: String = row.get(0).map_err(|_| "export_metadata_invalid")?;
        let name: String = row.get(1).map_err(|_| "export_metadata_invalid")?;
        // Only path traversal is forbidden here; actual target creation decides representability.
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
            return Err("export_name_invalid".into());
        }
        names.insert(name.clone());
        let kind: String = row.get(2).map_err(|_| "export_metadata_invalid")?;
        let name_error = |e| format!("export_name_unrepresentable_or_collision name={name:?}: {e}");
        if kind == "folder" {
            rfs::mkdirat(out, name.as_str(), Mode::RUSR | Mode::WUSR | Mode::XUSR)
                .map_err(name_error)?;
            let child = rfs::openat(
                out,
                name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| "export_folder_open_failed")?;
            count += walk(db, source, owner, &id, &File::from(child), depth + 1, None)?;
        } else if kind == "file" {
            let blob: String = row.get(3).map_err(|_| "export_revision_missing")?;
            let length: i64 = row.get(4).map_err(|_| "export_metadata_invalid")?;
            let digest: Vec<u8> = row.get(5).map_err(|_| "export_metadata_invalid")?;
            let blob_length: i64 = row.get(6).map_err(|_| "export_blob_missing")?;
            let blob_digest: Vec<u8> = row.get(7).map_err(|_| "export_metadata_invalid")?;
            let lifecycle: String = row.get(8).map_err(|_| "export_metadata_invalid")?;
            if length != blob_length || digest != blob_digest || lifecycle != "live" {
                return Err("export_blob_metadata_mismatch".into());
            }
            let length = u64::try_from(length).map_err(|_| "export_metadata_invalid")?;
            let digest = <[u8; 32]>::try_from(digest).map_err(|_| "export_metadata_invalid")?;
            let fd = rfs::openat(
                out,
                name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(name_error)?;
            let verifier =
                PayloadVerifier::open(source).map_err(|_| "export_payload_store_invalid")?;
            let mut file = File::from(fd);
            verifier
                .copy_verified(&blob, length, digest, &mut file)
                .map_err(|error| match error {
                    ExportCopyError::Payload(fault) => format!(
                        "export_payload_invalid reason={}",
                        crate::admin::PayloadReport::fault_label(fault)
                    ),
                    ExportCopyError::Destination(error) => format!("export_write_failed: {error}"),
                })?;
            file.sync_all()
                .map_err(|error| format!("export_write_failed: {error}"))?;
            count += 1;
        } else {
            return Err("export_metadata_invalid".into());
        }
    }
    for item in Dir::read_from(out).map_err(|_| "export_destination_unreadable")? {
        let item = item.map_err(|_| "export_destination_unreadable")?;
        let actual = item.file_name().to_bytes();
        if matches!(actual, b"." | b"..") || scratch == Some(actual) {
            continue;
        }
        if let Ok(name) = std::str::from_utf8(actual) {
            names.remove(name);
        }
    }
    if let Some(name) = names.iter().next() {
        return Err(format!("export_name_spelling_changed name={name:?}"));
    }
    Ok(count)
}
