//! Optional instance branding, independent of the versioned file-storage schema.
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
    sync::Mutex,
};

const FILE_NAME: &str = "instance.json";
pub const DEFAULT_NAME: &str = "Sylux";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    name: String,
}

pub fn normalized_name(value: &str) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let name = value.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return None;
    }
    Some(name.to_owned())
}

pub struct InstanceSettings {
    directory: File,
    access: Mutex<()>,
}

impl InstanceSettings {
    pub fn open(root: &Path) -> io::Result<Self> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let root = fs::open(root, flags, Mode::empty())?;
        let directory = File::from(fs::openat(
            root,
            super::root::METADATA_DIR,
            flags,
            Mode::empty(),
        )?);
        let store = Self {
            directory,
            access: Mutex::new(()),
        };
        store.name()?;
        Ok(store)
    }

    pub fn name(&self) -> io::Result<String> {
        let _guard = self
            .access
            .lock()
            .map_err(|_| io::Error::other("settings lock poisoned"))?;
        let fd = match fs::openat(
            &self.directory,
            FILE_NAME,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(DEFAULT_NAME.to_owned()),
            Err(error) => return Err(error.into()),
        };
        let stat = fs::fstat(&fd)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_nlink != 1
            || stat.st_mode & 0o077 != 0
        {
            return Err(io::Error::other(
                "settings file must be private and regular",
            ));
        }
        let mut bytes = Vec::new();
        File::from(fd).take(1025).read_to_end(&mut bytes)?;
        if bytes.len() > 1024 {
            return Err(io::Error::other("settings file too large"));
        }
        let settings: Settings = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if normalized_name(&settings.name).as_deref() != Some(settings.name.as_str()) {
            return Err(io::Error::other("invalid stored instance name"));
        }
        Ok(settings.name)
    }

    pub fn save(&self, name: &str) -> io::Result<()> {
        if normalized_name(name).as_deref() != Some(name) {
            return Err(io::Error::other("invalid instance name"));
        }
        let _guard = self
            .access
            .lock()
            .map_err(|_| io::Error::other("settings lock poisoned"))?;
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let temporary = format!(
            ".instance-{}.tmp",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let fd = fs::openat(
            &self.directory,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?;
        let result = (|| {
            let mut file = File::from(fd);
            let bytes = serde_json::to_vec(&Settings {
                name: name.to_owned(),
            })
            .map_err(io::Error::other)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::renameat(
                &self.directory,
                temporary.as_str(),
                &self.directory,
                FILE_NAME,
            )?;
            self.directory.sync_all()
        })();
        if result.is_err() {
            let _ = fs::unlinkat(&self.directory, temporary.as_str(), AtFlags::empty());
        }
        result
    }
}
