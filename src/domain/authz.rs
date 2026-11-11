//! Local password and opaque server-side session primitives.
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

/// Maximum accepted password length, measured in bytes. Reject rather than truncate.
pub const MAX_PASSWORD_BYTES: usize = 1024;
const MIN_PASSWORD_CHARS: usize = 15;
const MAX_PASSWORD_CHARS: usize = 128;

/// Enforce the local-password input bounds without trimming, normalizing, or
/// truncating the supplied Unicode string.
pub fn validate_password(password: &str) -> bool {
    let characters = password.chars().count();
    (MIN_PASSWORD_CHARS..=MAX_PASSWORD_CHARS).contains(&characters)
        && password.len() <= MAX_PASSWORD_BYTES
}

/// Browser session idle lifetime (30 minutes).
pub const IDLE_SECONDS: i64 = 30 * 60;
/// Browser session absolute lifetime (12 hours).
pub const ABSOLUTE_SECONDS: i64 = 12 * 60 * 60;
const MAX_PHC_BYTES: usize = 512;
const MIN_ARGON_MEMORY_KIB: u32 = 19 * 1024;
const MAX_ARGON_MEMORY_KIB: u32 = 64 * 1024;
const MAX_ARGON_ITERATIONS: u32 = 4;
const MAX_ARGON_LANES: u32 = 4;
const MAX_ACTIVE_PASSWORD_CHECKS: usize = 4;

/// Serializes expensive password verification to a small bounded number of workers.
#[derive(Clone)]
pub struct PasswordLimiter {
    active: Arc<Mutex<usize>>,
    dummy_verifier: Arc<String>,
}

impl Default for PasswordLimiter {
    fn default() -> Self {
        // This is a public, non-account verifier used only to make unknown-user
        // attempts execute the same bounded Argon2 path as wrong passwords.
        let dummy_verifier = Argon2::default()
            .hash_password(b"public dummy verifier input")
            .map(|hash| hash.to_string())
            .unwrap_or_default();
        Self {
            active: Arc::new(Mutex::new(0)),
            dummy_verifier: Arc::new(dummy_verifier),
        }
    }
}

struct ActiveCheck(Arc<Mutex<usize>>);
impl Drop for ActiveCheck {
    fn drop(&mut self) {
        if let Ok(mut active) = self.0.lock() {
            *active = active.saturating_sub(1);
        }
    }
}

impl PasswordLimiter {
    fn acquire(&self) -> Option<ActiveCheck> {
        let mut active = self.active.lock().ok()?;
        if *active >= MAX_ACTIVE_PASSWORD_CHECKS {
            return None;
        }
        *active += 1;
        Some(ActiveCheck(Arc::clone(&self.active)))
    }

    fn verify_with_permit(&self, password: &[u8], encoded: &str) -> bool {
        if password.len() > MAX_PASSWORD_BYTES || encoded.len() > MAX_PHC_BYTES {
            return false;
        }
        let Ok(hash) = PasswordHash::new(encoded) else {
            return false;
        };
        if hash.algorithm.as_str() != "argon2id"
            || hash.version != Some(19)
            || !hash
                .hash
                .is_some_and(|output| (16..=64).contains(&output.len()))
        {
            return false;
        }
        let parameter = |name| hash.params.get(name).and_then(|value| value.decimal().ok());
        let (Some(memory), Some(iterations), Some(lanes)) =
            (parameter("m"), parameter("t"), parameter("p"))
        else {
            return false;
        };
        if !(MIN_ARGON_MEMORY_KIB..=MAX_ARGON_MEMORY_KIB).contains(&memory)
            || !(1..=MAX_ARGON_ITERATIONS).contains(&iterations)
            || !(1..=MAX_ARGON_LANES).contains(&lanes)
        {
            return false;
        }
        Argon2::default().verify_password(password, &hash).is_ok()
    }

    /// Verifies the exact supplied bytes against a stored Argon2id PHC verifier.
    /// PHC resource parameters are rejected before Argon2 can allocate for them.
    pub fn verify(&self, password: &[u8], encoded: &str) -> bool {
        let Some(_permit) = self.acquire() else {
            return false;
        };
        self.verify_with_permit(password, encoded)
    }

    /// Runs a real verifier or a dummy verifier for an unknown account through
    /// the same bounded scheduler. The caller applies the enabled/epoch policy.
    pub fn verify_login(&self, password: &[u8], encoded: Option<&str>) -> bool {
        self.verify(password, encoded.unwrap_or(self.dummy_verifier.as_str()))
    }

    /// Perform expensive password verification on a blocking worker. Admission
    /// is acquired before enqueueing, so the hashing queue is strictly bounded.
    pub async fn verify_login_async(&self, password: Vec<u8>, encoded: Option<String>) -> bool {
        let Some(permit) = self.acquire() else {
            return false;
        };
        let limiter = self.clone();
        let dummy = self.dummy_verifier.as_str().to_owned();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            limiter.verify_with_permit(&password, encoded.as_deref().unwrap_or(&dummy))
        })
        .await
        .unwrap_or(false)
    }

    /// Hash a supplied password off the async executor under the same global
    /// bounded Argon2 admission limit used by login verification.
    pub async fn hash_password(&self, password: Vec<u8>) -> Option<String> {
        if password.len() > MAX_PASSWORD_BYTES {
            return None;
        }
        let permit = self.acquire()?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            Argon2::default()
                .hash_password(&password)
                .map(|hash| hash.to_string())
                .ok()
        })
        .await
        .ok()
        .flatten()
    }
}

/// Validate all live-session invariants from the database. Mutation commit paths
/// must call this under their serialized commit guard, not rely on an earlier check.
pub fn validate_session(
    connection: &Connection,
    session_id: &str,
    secret: &[u8],
    now: i64,
) -> Option<(String, String, String)> {
    let digest = verifier(secret);
    connection
        .query_row(
            "SELECT a.account_id,a.handle,a.role FROM session s JOIN account a ON a.account_id=s.account_id WHERE s.session_id=?1 AND s.verifier=?2 AND s.revoked=0 AND a.enabled=1 AND s.credential_epoch=a.credential_epoch AND s.idle_expires_at>?3 AND s.absolute_expires_at>?3",
            rusqlite::params![session_id, digest.as_slice(), now],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok()
}

/// Validate session-bound CSRF material using a constant-time verifier comparison.
/// Call only after `validate_session` has accepted the same session in this transaction.
pub fn csrf_matches(connection: &Connection, session_id: &str, csrf: &[u8]) -> bool {
    let stored = connection
        .query_row(
            "SELECT csrf_verifier FROM session WHERE session_id=?1 AND revoked=0",
            [session_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .ok()
        .flatten();
    stored.is_some_and(|stored| verifier_matches(&stored, &verifier(csrf)))
}

/// SHA-256 verifier for a random bearer token. Raw token is never persisted.
pub fn verifier(token: &[u8]) -> [u8; 32] {
    Sha256::digest(token).into()
}

/// Constant-time equality for fixed-size secret verifiers.
pub fn verifier_matches(left: &[u8], right: &[u8]) -> bool {
    if left.len() != 32 || right.len() != 32 {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::{Argon2, Params, password_hash::PasswordHasher};

    #[test]
    fn password_validation_enforces_character_and_byte_bounds_without_normalizing() {
        assert!(validate_password("123456789012345"));
        assert!(!validate_password("12345678901234"));
        assert!(!validate_password(&"x".repeat(129)));
        assert!(validate_password("é".repeat(15).as_str()));
        assert!(!validate_password(&"é".repeat(513)));
        assert!(validate_password("  spaced password  "));
    }

    #[test]
    fn verifier_is_fixed_size_and_deterministic() {
        let digest = verifier(b"opaque-token");
        assert_eq!(digest.len(), 32);
        assert!(verifier_matches(&digest, &verifier(b"opaque-token")));
        assert!(!verifier_matches(&digest, &verifier(b"other")));
    }

    #[test]
    fn exact_password_bytes_are_verified_and_overlong_is_rejected() {
        let hash = Argon2::default()
            .hash_password(b" spaced ")
            .unwrap()
            .to_string();
        let limiter = PasswordLimiter::default();
        assert!(limiter.verify(b" spaced ", &hash));
        assert!(!limiter.verify(b"spaced", &hash));
        assert!(!limiter.verify(&vec![b'x'; MAX_PASSWORD_BYTES + 1], &hash));
    }

    #[test]
    fn argon2id_costs_outside_the_policy_are_rejected_before_verification() {
        let limiter = PasswordLimiter::default();
        for (memory_kib, iterations, lanes) in [
            (8 * 1024, 2, 1),
            (1024, 2, 1),
            (65_537, 2, 1),
            (8 * 1024, 5, 1),
            (8 * 1024, 2, 5),
        ] {
            let params = Params::new(memory_kib, iterations, lanes, None).unwrap();
            let encoded = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
                .hash_password(b"policy test password")
                .unwrap()
                .to_string();
            assert!(
                !limiter.verify(b"policy test password", &encoded),
                "accepted m={memory_kib} t={iterations} p={lanes}"
            );
        }
    }
}
