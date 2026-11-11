//! Canonical, byte-bounded sibling names.
use unicode_normalization::UnicodeNormalization;

pub const MAX_NAME_BYTES: usize = 255;
pub const NAME_KEY_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryName {
    pub display: String,
    pub key: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    Dot,
    Separator,
    Control,
    TooLong,
}

impl EntryName {
    /// Preserve submitted spelling while comparing by NFC UTF-8 bytes.
    pub fn parse(value: &str) -> Result<Self, NameError> {
        if value.is_empty() {
            return Err(NameError::Empty);
        }
        if value == "." || value == ".." {
            return Err(NameError::Dot);
        }
        if value.chars().any(|c| c == '/' || c == '\\') {
            return Err(NameError::Separator);
        }
        if value.chars().any(char::is_control) {
            return Err(NameError::Control);
        }
        if value.len() > MAX_NAME_BYTES {
            return Err(NameError::TooLong);
        }
        let key = value.nfc().collect::<String>().into_bytes();
        if key.len() > MAX_NAME_BYTES {
            return Err(NameError::TooLong);
        }
        Ok(Self {
            display: value.to_owned(),
            key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_spelling_and_normalizes_sibling_key() {
        let composed = EntryName::parse("é").unwrap();
        let decomposed = EntryName::parse("e\u{301}").unwrap();
        assert_eq!(composed.display, "é");
        assert_eq!(composed.key, decomposed.key);
    }
    #[test]
    fn rejects_forbidden_names_without_trimming() {
        for value in ["", ".", "..", "a/b", "a\\b", "a\0b", "a\nb"] {
            assert!(EntryName::parse(value).is_err(), "{value:?}");
        }
        assert_eq!(EntryName::parse(" name ").unwrap().display, " name ");
        assert!(EntryName::parse(&"x".repeat(256)).is_err());
    }
}
