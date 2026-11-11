//! Deterministic synthetic fixtures. Seeds govern only public test data, never secrets.
use std::io::{self, Read};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ExpectedFile;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FixtureKind {
    Empty,
    AllBytes,
    UnicodeNfc,
    UnicodeNfd,
    HtmlAttachment,
    SvgAttachment,
    Generated,
}

#[derive(Debug, Clone)]
pub struct FixturePlan {
    seed: u64,
    chunk_size: usize,
}
impl FixturePlan {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            chunk_size: 16 * 1024,
        }
    }
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Result<Self, &'static str> {
        if chunk_size == 0 || chunk_size > 64 * 1024 {
            return Err("chunk size must be 1..=65536");
        }
        self.chunk_size = chunk_size;
        Ok(self)
    }
    pub fn stream(&self, kind: FixtureKind, length: u64) -> FixtureStream {
        let length = if kind == FixtureKind::Empty {
            0
        } else {
            length
        };
        FixtureStream::new(self.seed, kind, length, self.chunk_size)
    }
    pub fn expected_file(
        &self,
        id: String,
        owner_id: String,
        name: String,
        kind: FixtureKind,
        length: u64,
    ) -> io::Result<ExpectedFile> {
        let mut reader = self.stream(kind, length);
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = vec![0; self.chunk_size];
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            total += n as u64;
        }
        Ok(ExpectedFile {
            id,
            owner_id,
            name,
            length: total,
            sha256: super::hex_bytes(&hasher.finalize()),
        })
    }
}

/// A bounded-memory deterministic byte stream. Re-reading the same plan yields identical bytes.
pub struct FixtureStream {
    seed: u64,
    kind: FixtureKind,
    remaining: u64,
    offset: u64,
    chunk: usize,
}
impl FixtureStream {
    fn new(seed: u64, kind: FixtureKind, length: u64, chunk: usize) -> Self {
        Self {
            seed,
            kind,
            remaining: length,
            offset: 0,
            chunk,
        }
    }
    pub fn max_chunk_size(&self) -> usize {
        self.chunk
    }
    fn byte_at(&self, at: u64) -> u8 {
        match self.kind {
            FixtureKind::Empty => 0,
            FixtureKind::AllBytes => at as u8,
            FixtureKind::UnicodeNfc => "café".as_bytes()[at as usize % "café".len()],
            FixtureKind::UnicodeNfd => "cafe\u{301}".as_bytes()[at as usize % "cafe\u{301}".len()],
            FixtureKind::HtmlAttachment => {
                let pattern = b"<!doctype html><script>window.fixtureExecuted=true</script>";
                pattern[at as usize % pattern.len()]
            }
            FixtureKind::SvgAttachment => {
                let pattern = b"<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"window.fixtureExecuted=true\"/>";
                pattern[at as usize % pattern.len()]
            }
            FixtureKind::Generated => {
                let mut x = self.seed ^ at.wrapping_mul(0x9E3779B97F4A7C15);
                x ^= x >> 30;
                x = x.wrapping_mul(0xBF58476D1CE4E5B9);
                x ^= x >> 27;
                x = x.wrapping_mul(0x94D049BB133111EB);
                x ^= x >> 31;
                x as u8
            }
        }
    }
}
impl Read for FixtureStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = out
            .len()
            .min(self.chunk)
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        for (i, byte) in out[..n].iter_mut().enumerate() {
            *byte = self.byte_at(self.offset + i as u64);
        }
        self.offset += n as u64;
        self.remaining -= n as u64;
        Ok(n)
    }
}

/// Declarative identities only, not an alternate DB writer or production seed route.
/// H1 must later adapt these through reviewed constraints while stopped/owned.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeedAccount {
    pub account_id: String,
    pub root_id: String,
    pub handle: String,
    pub role: String,
}
pub fn synthetic_accounts(root: &super::FixtureRoot) -> io::Result<Vec<SeedAccount>> {
    use std::io::Write;
    let mut accounts = Vec::new();
    for (index, handle, role) in [
        (1_u32, "owner-a", "user"),
        (2, "owner-b", "user"),
        (3, "unrelated-admin", "administrator"),
    ] {
        let mut credentials = super::create_private_file(root.path(), handle)?;
        credentials.write_all(super::unique_token().as_bytes())?;
        credentials.sync_all()?;
        accounts.push(SeedAccount {
            account_id: format!("{index:032x}"),
            root_id: format!("{:032x}", index + 100),
            handle: handle.into(),
            role: role.into(),
        });
    }
    Ok(accounts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn same_seed_has_same_stream_and_digest() {
        let plan = FixturePlan::new(42).with_chunk_size(31).unwrap();
        let mut a = plan.stream(FixtureKind::Generated, 100_003);
        let mut b = plan.stream(FixtureKind::Generated, 100_003);
        let mut av = Vec::new();
        let mut bv = Vec::new();
        a.read_to_end(&mut av).unwrap();
        b.read_to_end(&mut bv).unwrap();
        assert_eq!(av, bv);
        assert_eq!(Sha256::digest(av), Sha256::digest(bv));
    }

    #[test]
    fn stream_never_exceeds_configured_buffer() {
        let plan = FixturePlan::new(5).with_chunk_size(7).unwrap();
        let mut stream = plan.stream(FixtureKind::AllBytes, 100);
        let mut buffer = [0; 128];
        assert_eq!(stream.read(&mut buffer).unwrap(), 7);
        assert_eq!(&buffer[..7], &[0, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn active_content_and_empty_fixture_semantics() {
        let mut data = Vec::new();
        FixturePlan::new(0)
            .stream(FixtureKind::HtmlAttachment, 15)
            .read_to_end(&mut data)
            .unwrap();
        assert!(
            data.starts_with(b"<!doctype html>"),
            "HTML fixture must contain real active-content syntax"
        );
        assert_eq!(
            FixturePlan::new(0)
                .stream(FixtureKind::Empty, 99)
                .read(&mut [0; 1])
                .unwrap(),
            0
        );
    }

    #[test]
    fn empty_stream_is_empty() {
        assert_eq!(
            FixturePlan::new(0)
                .stream(FixtureKind::Empty, 0)
                .read(&mut [0; 1])
                .unwrap(),
            0
        );
    }
}
