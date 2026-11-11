//! H0 test primitives. Imported only by explicitly feature-gated integration targets.
#![cfg(feature = "test-harness")]
#![allow(dead_code)]

pub mod browser;
pub mod faults;
pub mod fixtures;
pub mod server;

use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, fstat, mkdirat, open, openat, statat, unlinkat,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}
fn identity(file: &File) -> std::io::Result<(u64, u64)> {
    let m = file.metadata()?;
    Ok((m.dev(), m.ino()))
}

/// An owned directory under the configured scratch directory. No implicit Drop deletion.
/// Open descriptors pin the parent, root and marker identity. Only registered, explicitly
/// joined workers can release cleanup leases; dropping a lease conservatively retains it.
/// Like the product failure model, this assumes no hostile same-UID host operator.
pub struct FixtureRoot {
    path: PathBuf,
    parent: File,
    root: File,
    marker_file: File,
    marker: String,
    state: Arc<Mutex<RootState>>,
}
#[derive(Default)]
struct RootState {
    next: u64,
    workers: BTreeSet<u64>,
    cleaned: bool,
}
pub struct WorkerLease {
    state: Arc<Mutex<RootState>>,
    id: u64,
}
impl WorkerLease {
    /// Call only after the associated thread/process/task has actually joined/reaped.
    pub fn joined(self) -> std::io::Result<()> {
        self.state
            .lock()
            .map_err(|_| std::io::Error::other("fixture state poisoned"))?
            .workers
            .remove(&self.id);
        Ok(())
    }
}
impl FixtureRoot {
    pub fn create(parent: &Path, run_id: &str, case_id: &str) -> std::io::Result<Self> {
        validate_component(run_id)?;
        validate_component(case_id)?;
        if !parent.is_absolute()
            || parent
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(std::io::Error::other(
                "fixture parent must be an absolute scratch path",
            ));
        }
        let scratch = fs::canonicalize(std::env::temp_dir())?;
        if !parent.starts_with(&scratch) || parent == scratch {
            return Err(std::io::Error::other(
                "fixture parent must be below scratch",
            ));
        }
        // Refuse symlink ancestors rather than canonicalizing an alias into personal data.
        let mut current = PathBuf::from("/");
        for part in parent.components().skip(1) {
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(m) if m.is_dir() && !m.file_type().is_symlink() => (),
                Ok(_) => {
                    return Err(std::io::Error::other(
                        "fixture parent contains non-directory or symlink",
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    fs::DirBuilder::new().mode(0o700).create(&current)?;
                }
                Err(e) => return Err(e),
            }
        }
        let parent_fd: File = open(parent, directory_flags(), Mode::empty())?.into();
        let name = format!("fixture-{run_id}-{case_id}-{}", unique_token());
        mkdirat(&parent_fd, &name, Mode::RWXU)?;
        let root: File = openat(&parent_fd, &name, directory_flags(), Mode::empty())?.into();
        let marker = unique_token();
        let mut marker_file: File = openat(
            &root,
            ".fixture-owner",
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?
        .into();
        marker_file.write_all(marker.as_bytes())?;
        marker_file.sync_all()?;
        Ok(Self {
            path: parent.join(name),
            parent: parent_fd,
            root,
            marker_file,
            marker,
            state: Arc::default(),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn register_worker(&self) -> std::io::Result<WorkerLease> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| std::io::Error::other("fixture state poisoned"))?;
        if s.cleaned {
            return Err(std::io::Error::other("fixture already cleaned"));
        }
        s.next = s
            .next
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("worker counter overflow"))?;
        let id = s.next;
        s.workers.insert(id);
        Ok(WorkerLease {
            state: self.state.clone(),
            id,
        })
    }
    pub fn cleanup_after_workers_exit(&self) -> std::io::Result<()> {
        // Hold across verification and removal: no new worker may race cleanup.
        let mut s = self
            .state
            .lock()
            .map_err(|_| std::io::Error::other("fixture state poisoned"))?;
        if s.cleaned || !s.workers.is_empty() {
            return Err(std::io::Error::other(
                "fixture cleaned or unjoined workers remain",
            ));
        }
        let current_parent: File = open(
            self.path
                .parent()
                .ok_or_else(|| std::io::Error::other("missing parent"))?,
            directory_flags(),
            Mode::empty(),
        )?
        .into();
        if identity(&current_parent)? != identity(&self.parent)? {
            return Err(std::io::Error::other("fixture parent identity changed"));
        }
        let name = self
            .path
            .file_name()
            .ok_or_else(|| std::io::Error::other("missing fixture name"))?;
        let current: File = openat(&self.parent, name, directory_flags(), Mode::empty())?.into();
        if identity(&current)? != identity(&self.root)? {
            return Err(std::io::Error::other("fixture root identity changed"));
        }
        let marker: File = openat(
            &current,
            ".fixture-owner",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
        if !marker.metadata()?.is_file()
            || marker.metadata()?.nlink() != 1
            || identity(&marker)? != identity(&self.marker_file)?
        {
            return Err(std::io::Error::other("fixture marker identity changed"));
        }
        let mut contents = String::new();
        marker.take(256).read_to_string(&mut contents)?;
        if contents != self.marker {
            return Err(std::io::Error::other("fixture marker mismatch"));
        }
        remove_children(&current, fstat(&current)?.st_dev as u64)?;
        // Never traverse a substituted pathname; recursion used only pinned descriptors.
        let final_stat = statat(&self.parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let held = fstat(&current)?;
        if (final_stat.st_dev, final_stat.st_ino) != (held.st_dev, held.st_ino) {
            return Err(std::io::Error::other("fixture replaced during cleanup"));
        }
        unlinkat(&self.parent, name, AtFlags::REMOVEDIR)?;
        s.cleaned = true;
        Ok(())
    }
}
fn remove_children(directory: &File, device: u64) -> std::io::Result<()> {
    for entry in Dir::read_from(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        let st = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
        if st.st_dev as u64 != device {
            return Err(std::io::Error::other("fixture cleanup cannot cross mounts"));
        }
        if FileType::from_raw_mode(st.st_mode) == FileType::Directory {
            let child: File = openat(directory, name, directory_flags(), Mode::empty())?.into();
            let held = fstat(&child)?;
            if (held.st_dev, held.st_ino) != (st.st_dev, st.st_ino) {
                return Err(std::io::Error::other("fixture child replaced"));
            }
            remove_children(&child, device)?;
            unlinkat(directory, name, AtFlags::REMOVEDIR)?;
        } else {
            // Unlink the entry, never follow a symlink (including outside-root targets).
            unlinkat(directory, name, AtFlags::empty())?;
        }
    }
    Ok(())
}
fn validate_component(value: &str) -> std::io::Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(std::io::Error::other("invalid fixture path component"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CaseStatus {
    Pass,
    Fail,
    Blocked,
    NotRun,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageRow {
    pub row_id: String,
    pub status: CaseStatus,
    pub blocker: Option<String>,
    pub evidence: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageReport {
    pub schema_version: u32,
    pub mode: String,
    pub rows: Vec<CoverageRow>,
}
impl CoverageReport {
    pub fn validate(&self, required_rows: &[String]) -> Result<(), String> {
        if self.schema_version != 1
            || !["harness-self", "qualification"].contains(&self.mode.as_str())
        {
            return Err("invalid coverage schema or mode".into());
        }
        let mut seen = BTreeSet::new();
        for row in &self.rows {
            if !seen.insert(&row.row_id) {
                return Err(format!("duplicate coverage row {}", row.row_id));
            }
            if row.status == CaseStatus::Blocked
                && row.blocker.as_deref().unwrap_or("").trim().is_empty()
            {
                return Err("blocked row has no blocker".into());
            }
            if row.status == CaseStatus::Pass
                && row.evidence.as_deref().unwrap_or("").trim().is_empty()
            {
                return Err("PASS row has no evidence".into());
            }
        }
        for required in required_rows {
            let row = self
                .rows
                .iter()
                .find(|r| &r.row_id == required)
                .ok_or_else(|| format!("required row {required} missing"))?;
            if row.status != CaseStatus::Pass {
                return Err(format!(
                    "required row {required} is {:?}, not PASS",
                    row.status
                ));
            }
        }
        Ok(())
    }
}

/// Shared logical UTC time. Real monotonic watchdogs remain independent.
#[derive(Debug, Clone)]
pub struct TestClock {
    now: Arc<AtomicI64>,
}
impl TestClock {
    pub fn at(unix_seconds: i64) -> Self {
        Self {
            now: Arc::new(AtomicI64::new(unix_seconds)),
        }
    }
    pub fn now(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
    pub fn advance(&self, seconds: i64) -> Result<i64, &'static str> {
        if seconds < 0 {
            return Err("clock cannot move backwards");
        }
        self.now
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                n.checked_add(seconds)
            })
            .map(|n| n + seconds)
            .map_err(|_| "test clock overflow")
    }
}

/// Single-use rendezvous: arrive -> observe -> release -> completed. Duplicate,
/// unused and timeout paths fail; no unbounded channel send or logical-clock sleep.
#[derive(Debug)]
pub struct TestBarrier {
    state: Mutex<BarrierState>,
    changed: Condvar,
}
#[derive(Debug, Default)]
struct BarrierState {
    arrived: bool,
    observed: bool,
    released: bool,
    completed: bool,
    failed: bool,
}
impl TestBarrier {
    pub fn new(_id: impl Into<String>) -> Self {
        Self {
            state: Mutex::default(),
            changed: Condvar::new(),
        }
    }
    pub fn worker_pause(&self, timeout: Duration) -> Result<(), String> {
        let mut s = self.state.lock().map_err(|_| "barrier poisoned")?;
        if s.arrived || s.failed {
            s.failed = true;
            self.changed.notify_all();
            return Err("duplicate or failed barrier arrival".into());
        }
        s.arrived = true;
        self.changed.notify_all();
        let (mut s, _) = self
            .changed
            .wait_timeout_while(s, timeout, |s| !s.released && !s.failed)
            .map_err(|_| "barrier poisoned")?;
        if !s.released || s.failed {
            s.failed = true;
            self.changed.notify_all();
            return Err("barrier release timeout or failed".into());
        }
        s.completed = true;
        Ok(())
    }
    pub fn wait_reached(&self, timeout: Duration) -> Result<(), String> {
        let s = self.state.lock().map_err(|_| "barrier poisoned")?;
        let (mut s, _) = self
            .changed
            .wait_timeout_while(s, timeout, |s| !s.arrived && !s.failed)
            .map_err(|_| "barrier poisoned")?;
        if !s.arrived || s.observed || s.failed {
            s.failed = true;
            self.changed.notify_all();
            return Err("barrier arrival timeout, duplicate or failed".into());
        }
        s.observed = true;
        Ok(())
    }
    pub fn release(&self) -> Result<(), String> {
        let mut s = self.state.lock().map_err(|_| "barrier poisoned")?;
        if !s.observed || s.released || s.failed {
            s.failed = true;
            self.changed.notify_all();
            return Err("barrier release before observation, duplicate or failed".into());
        }
        s.released = true;
        self.changed.notify_all();
        Ok(())
    }
    pub fn assert_drained(&self) -> Result<(), String> {
        let s = self.state.lock().map_err(|_| "barrier poisoned")?;
        if s.completed && !s.failed {
            Ok(())
        } else {
            Err("barrier unused, incomplete or failed".into())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExpectedFile {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub length: u64,
    pub sha256: String,
}
pub fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_bytes(&Sha256::digest(bytes))
}
pub fn create_private_file(root: &Path, relative: &str) -> std::io::Result<File> {
    validate_component(relative)?;
    let directory: File = open(root, directory_flags(), Mode::empty())?.into();
    Ok(openat(
        &directory,
        relative,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?
    .into())
}
pub fn unique_token() -> String {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes)
        .expect("OS randomness required for synthetic credentials and root identity");
    hex_bytes(&bytes)
}

/// Persist only allowlisted metadata. Entire headers/bodies/URLs/errors are discarded,
/// not regex-scrubbed after a secret-bearing raw trace has already been retained.
#[derive(Serialize)]
pub struct SanitizedExchange {
    status: u16,
    headers: &'static str,
    body: &'static str,
}
pub fn redact_exchange(status: u16, _headers: &[(&str, &str)], _body: &[u8]) -> SanitizedExchange {
    SanitizedExchange {
        status,
        headers: "[REDACTED]",
        body: "[REDACTED]",
    }
}
pub fn write_private_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()
}
