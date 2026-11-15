//! Optional instance branding, independent of the versioned file-storage schema.
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageDecoder, ImageFormat, ImageReader, Limits};
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Cursor, Read, Write},
    path::Path,
    sync::{Arc, Mutex},
};

const FILE_NAME: &str = "instance.json";
const APPEARANCE_FILE: &str = "appearance.json";
pub const DEFAULT_NAME: &str = "Sylux";
pub const LOGO_BYTES: usize = 512 * 1024;
pub const LOGIN_IMAGE_BYTES: usize = 2 * 1024 * 1024;
pub const APPEARANCE_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;
const MAX_PIXELS: u64 = 8_000_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    name: String,
}

#[derive(Clone, Default)]
pub struct Appearance {
    pub accent_color: Option<String>,
    pub login_background_color: Option<String>,
    pub logo: Option<Asset>,
    pub login_image: Option<Asset>,
}

#[derive(Clone)]
pub struct Asset {
    media_type: &'static str,
    bytes: Arc<[u8]>,
    digest: String,
}

impl Asset {
    pub fn media_type(&self) -> &'static str {
        self.media_type
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetUpload {
    pub media_type: String,
    pub data: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredAppearance {
    accent_color: Option<String>,
    login_background_color: Option<String>,
    logo: Option<AssetUpload>,
    login_image: Option<AssetUpload>,
}

pub enum AssetChange {
    Keep,
    Replace(Option<Asset>),
}

pub struct AppearanceChange {
    pub accent_color: Option<String>,
    pub login_background_color: Option<String>,
    pub logo: AssetChange,
    pub login_image: AssetChange,
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

pub fn normalized_color(value: &str) -> Option<String> {
    (value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit))
    .then(|| value.to_ascii_uppercase())
}

impl AssetUpload {
    pub fn validate(self, maximum_bytes: usize) -> Result<Asset, &'static str> {
        if self.data.len() > maximum_bytes.div_ceil(3) * 4 {
            return Err("Image exceeds its upload size limit.");
        }
        let bytes = STANDARD
            .decode(&self.data)
            .map_err(|_| "Image data must be standard base64.")?;
        if bytes.is_empty() || bytes.len() > maximum_bytes {
            return Err("Image exceeds its upload size limit.");
        }
        let format = image::guess_format(&bytes).map_err(|_| "Image format is not supported.")?;
        let media_type = match format {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::WebP => "image/webp",
            _ => return Err("Use a PNG, JPEG or WebP image."),
        };
        if self.media_type != media_type {
            return Err("Image format does not match its media type.");
        }
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_DIMENSION);
        limits.max_image_height = Some(MAX_DIMENSION);
        limits.max_alloc = Some(128 * 1024 * 1024);
        let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
        reader.limits(limits.clone());
        let decoder = reader
            .into_decoder()
            .map_err(|_| "Image could not be decoded.")?;
        let (width, height) = decoder.dimensions();
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err("Image must have at most 8 million pixels and 4096 pixels per side.");
        }
        // A single-frame decode must not admit unchecked animation frames.
        match format {
            ImageFormat::Png => {
                let decoder =
                    image::codecs::png::PngDecoder::with_limits(Cursor::new(&bytes), limits)
                        .map_err(|_| "Image could not be decoded.")?;
                if decoder
                    .is_apng()
                    .map_err(|_| "Image could not be decoded.")?
                {
                    return Err("Animated images are not supported.");
                }
            }
            ImageFormat::WebP => {
                let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(&bytes))
                    .map_err(|_| "Image could not be decoded.")?;
                if decoder.has_animation() {
                    return Err("Animated images are not supported.");
                }
            }
            _ => {}
        }
        image::DynamicImage::from_decoder(decoder).map_err(|_| "Image could not be decoded.")?;
        let digest = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Asset {
            media_type,
            bytes: bytes.into(),
            digest,
        })
    }
}

impl Appearance {
    fn stored(&self) -> StoredAppearance {
        let asset = |value: &Asset| AssetUpload {
            media_type: value.media_type.to_owned(),
            data: STANDARD.encode(&value.bytes),
        };
        StoredAppearance {
            accent_color: self.accent_color.clone(),
            login_background_color: self.login_background_color.clone(),
            logo: self.logo.as_ref().map(asset),
            login_image: self.login_image.as_ref().map(asset),
        }
    }
}

impl StoredAppearance {
    fn validate(self) -> io::Result<Appearance> {
        let color = |value: Option<String>| {
            value
                .map(|value| {
                    normalized_color(&value)
                        .ok_or_else(|| io::Error::other("invalid stored appearance color"))
                })
                .transpose()
        };
        Ok(Appearance {
            accent_color: color(self.accent_color)?,
            login_background_color: color(self.login_background_color)?,
            logo: self
                .logo
                .map(|asset| asset.validate(LOGO_BYTES))
                .transpose()
                .map_err(io::Error::other)?,
            login_image: self
                .login_image
                .map(|asset| asset.validate(LOGIN_IMAGE_BYTES))
                .transpose()
                .map_err(io::Error::other)?,
        })
    }
}

pub struct InstanceSettings {
    directory: File,
    access: Mutex<Appearance>,
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
            access: Mutex::new(Appearance::default()),
        };
        store.name()?;
        if let Some(bytes) = store.read_private(APPEARANCE_FILE, APPEARANCE_BODY_BYTES)? {
            let stored: StoredAppearance =
                serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            *store
                .access
                .lock()
                .map_err(|_| io::Error::other("settings lock poisoned"))? = stored.validate()?;
        }
        Ok(store)
    }

    fn read_private(&self, name: &str, maximum_bytes: usize) -> io::Result<Option<Vec<u8>>> {
        let fd = match fs::openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
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
        File::from(fd)
            .take(maximum_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > maximum_bytes {
            return Err(io::Error::other("settings file too large"));
        }
        Ok(Some(bytes))
    }

    pub fn name(&self) -> io::Result<String> {
        let _guard = self
            .access
            .lock()
            .map_err(|_| io::Error::other("settings lock poisoned"))?;
        let Some(bytes) = self.read_private(FILE_NAME, 1024)? else {
            return Ok(DEFAULT_NAME.to_owned());
        };
        let settings: Settings = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if normalized_name(&settings.name).as_deref() != Some(settings.name.as_str()) {
            return Err(io::Error::other("invalid stored instance name"));
        }
        Ok(settings.name)
    }

    pub fn appearance(&self) -> io::Result<Appearance> {
        self.access
            .lock()
            .map(|appearance| appearance.clone())
            .map_err(|_| io::Error::other("settings lock poisoned"))
    }

    pub fn save(&self, name: &str) -> io::Result<()> {
        if normalized_name(name).as_deref() != Some(name) {
            return Err(io::Error::other("invalid instance name"));
        }
        let _guard = self
            .access
            .lock()
            .map_err(|_| io::Error::other("settings lock poisoned"))?;
        let bytes = serde_json::to_vec(&Settings {
            name: name.to_owned(),
        })
        .map_err(io::Error::other)?;
        self.replace_private(FILE_NAME, &bytes, || {})
    }

    pub fn save_appearance(&self, change: AppearanceChange) -> io::Result<Appearance> {
        let mut current = self
            .access
            .lock()
            .map_err(|_| io::Error::other("settings lock poisoned"))?;
        let replace = |change, old: &Option<Asset>| match change {
            AssetChange::Keep => old.clone(),
            AssetChange::Replace(asset) => asset,
        };
        let next = Appearance {
            accent_color: change.accent_color,
            login_background_color: change.login_background_color,
            logo: replace(change.logo, &current.logo),
            login_image: replace(change.login_image, &current.login_image),
        };
        // Revalidate colors at the persistence boundary; Asset construction is private.
        if next
            .accent_color
            .as_ref()
            .is_some_and(|value| normalized_color(value).as_ref() != Some(value))
            || next
                .login_background_color
                .as_ref()
                .is_some_and(|value| normalized_color(value).as_ref() != Some(value))
        {
            return Err(io::Error::other("invalid appearance color"));
        }
        if next
            .logo
            .as_ref()
            .is_some_and(|asset| asset.bytes.len() > LOGO_BYTES)
            || next
                .login_image
                .as_ref()
                .is_some_and(|asset| asset.bytes.len() > LOGIN_IMAGE_BYTES)
        {
            return Err(io::Error::other("appearance image exceeds its size limit"));
        }
        let bytes = serde_json::to_vec(&next.stored()).map_err(io::Error::other)?;
        self.replace_private(APPEARANCE_FILE, &bytes, || *current = next.clone())?;
        Ok(next)
    }

    fn replace_private(
        &self,
        name: &str,
        bytes: &[u8],
        published: impl FnOnce(),
    ) -> io::Result<()> {
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
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::renameat(&self.directory, temporary.as_str(), &self.directory, name)?;
            published();
            self.directory.sync_all()
        })();
        if result.is_err() {
            let _ = fs::unlinkat(&self.directory, temporary.as_str(), AtFlags::empty());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(width: u32, height: u32, format: ImageFormat) -> AssetUpload {
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(width, height)
            .write_to(&mut encoded, format)
            .unwrap();
        AssetUpload {
            media_type: format.to_mime_type().to_owned(),
            data: STANDARD.encode(encoded.into_inner()),
        }
    }

    #[test]
    fn appearance_assets_require_full_decoding_and_bounded_dimensions() {
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
            let asset = upload(2, 2, format).validate(LOGO_BYTES).unwrap();
            assert_eq!(asset.media_type(), format.to_mime_type());
            let mut incomplete = asset.bytes().to_vec();
            incomplete.truncate(incomplete.len() / 2);
            assert!(
                AssetUpload {
                    media_type: asset.media_type().to_owned(),
                    data: STANDARD.encode(incomplete)
                }
                .validate(LOGO_BYTES)
                .is_err()
            );
        }
        assert!(
            upload(MAX_DIMENSION + 1, 1, ImageFormat::Png)
                .validate(LOGO_BYTES)
                .is_err()
        );
        assert!(
            upload(3000, 3000, ImageFormat::Png)
                .validate(LOGO_BYTES)
                .is_err()
        );
    }

    #[test]
    fn appearance_assets_reject_animated_png() {
        let still = upload(2, 2, ImageFormat::Png);
        let mut bytes = STANDARD.decode(&still.data).unwrap();
        let mut control = Vec::from(&b"acTL"[..]);
        control.extend_from_slice(&1_u32.to_be_bytes());
        control.extend_from_slice(&0_u32.to_be_bytes());
        let mut crc = !0_u32;
        for byte in &control {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xEDB8_8320 & 0_u32.wrapping_sub(crc & 1));
            }
        }
        let mut chunk = Vec::from(&8_u32.to_be_bytes()[..]);
        chunk.extend_from_slice(&control);
        chunk.extend_from_slice(&(!crc).to_be_bytes());
        bytes.splice(33..33, chunk);
        let decoder = image::codecs::png::PngDecoder::new(Cursor::new(&bytes)).unwrap();
        assert!(
            decoder.is_apng().unwrap(),
            "fixture must reach animation detection"
        );
        assert!(
            AssetUpload {
                media_type: "image/png".to_owned(),
                data: STANDARD.encode(bytes)
            }
            .validate(LOGO_BYTES)
            .is_err()
        );
    }
}
