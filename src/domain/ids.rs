//! Validated opaque identifiers used by mutation records and signed tickets.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use getrandom::fill;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MutationId(String);

impl MutationId {
    pub fn generate() -> Result<Self, IdError> {
        let mut bytes = [0_u8; 16];
        fill(&mut bytes).map_err(|_| IdError::EntropyUnavailable)?;
        Ok(Self::from_bytes(bytes))
    }

    pub fn parse(value: &str) -> Result<Self, IdError> {
        parse_opaque_id(value).map(Self)
    }

    fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(encode_lower_hex(&bytes))
    }

    fn to_bytes(&self) -> [u8; 16] {
        decode_opaque_id(&self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrincipalId(String);

impl PrincipalId {
    pub fn parse(value: &str) -> Result<Self, IdError> {
        parse_opaque_id(value).map(Self)
    }

    fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(encode_lower_hex(&bytes))
    }

    fn to_bytes(&self) -> [u8; 16] {
        decode_opaque_id(&self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A principal plus its server-random operation ID, matching the durable
/// mutation uniqueness boundary rather than treating the operation ID as global.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopedMutationId {
    principal: PrincipalId,
    mutation: MutationId,
}

impl ScopedMutationId {
    pub fn new(principal: PrincipalId, mutation: MutationId) -> Self {
        Self {
            principal,
            mutation,
        }
    }

    pub fn principal(&self) -> &PrincipalId {
        &self.principal
    }

    pub fn mutation_id(&self) -> &MutationId {
        &self.mutation
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MutationScope(String);

impl MutationScope {
    pub fn parse(value: &str) -> Result<Self, IdError> {
        if valid_bounded_label(value, true) {
            Ok(Self(value.to_owned()))
        } else {
            Err(IdError::Invalid)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TicketKeyId(String);

impl TicketKeyId {
    pub fn parse(value: &str) -> Result<Self, IdError> {
        if valid_bounded_label(value, false) {
            Ok(Self(value.to_owned()))
        } else {
            Err(IdError::Invalid)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn parse_opaque_id(value: &str) -> Result<String, IdError> {
    if value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(value.to_owned())
    } else {
        Err(IdError::Invalid)
    }
}

fn encode_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

fn decode_opaque_id(value: &str) -> [u8; 16] {
    debug_assert!(parse_opaque_id(value).is_ok());
    let mut decoded = [0_u8; 16];
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    debug_assert!(remainder.is_empty());
    for (output, pair) in decoded.iter_mut().zip(pairs) {
        *output = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    decoded
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => unreachable!("validated opaque ID contains only lowercase hex"),
    }
}

fn valid_bounded_label(value: &str, lowercase_only: bool) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    let valid_alpha = |byte: u8| {
        byte.is_ascii_digit()
            || if lowercase_only {
                byte.is_ascii_lowercase()
            } else {
                byte.is_ascii_alphanumeric()
            }
    };
    value.len() <= 64
        && valid_alpha(first)
        && bytes.all(|byte| valid_alpha(byte) || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdError {
    Invalid,
    EntropyUnavailable,
}

pub const TICKET_VERSION: u8 = 1;
pub const MAX_TICKET_LENGTH: usize = 512;
const MAX_PAYLOAD_LENGTH: usize = 187;
const TICKET_DOMAIN: &[u8] = b"sylux\x00mutation-ticket\x00v1\x00";

/// A protected 256-bit HMAC key. Its ID is public key-routing metadata; the
/// secret has no `Debug`, Clone or accessor implementation.
pub struct TicketSigningKey {
    id: TicketKeyId,
    secret: [u8; 32],
}

impl TicketSigningKey {
    pub fn new(id: TicketKeyId, secret: [u8; 32]) -> Self {
        Self { id, secret }
    }

    pub fn id(&self) -> &TicketKeyId {
        &self.id
    }
}

/// Expected current authority and execution context. Callers still compare the
/// returned `ScopedMutationId` with any operation ID present in route/storage.
pub struct TicketExpectation<'a> {
    principal: &'a PrincipalId,
    scope: &'a MutationScope,
    recovery_epoch: i64,
    now: i64,
}

impl<'a> TicketExpectation<'a> {
    pub fn new(
        principal: &'a PrincipalId,
        scope: &'a MutationScope,
        recovery_epoch: i64,
        now: i64,
    ) -> Self {
        Self {
            principal,
            scope,
            recovery_epoch,
            now,
        }
    }
}

/// An authenticated, versioned server-issued mutation ticket.
///
/// Expiry rejects new execution only. It is not a retention deadline and never
/// authorizes expiring, evicting or sweeping an unresolved mutation record.
#[derive(PartialEq, Eq)]
pub struct MutationTicket {
    scoped_id: ScopedMutationId,
    scope: MutationScope,
    issued_at: i64,
    expires_at: i64,
    recovery_epoch: i64,
    key_id: TicketKeyId,
    encoded: String,
}

impl std::fmt::Debug for MutationTicket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MutationTicket")
            .field("scoped_id", &self.scoped_id)
            .field("scope", &self.scope)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .field("recovery_epoch", &self.recovery_epoch)
            .field("key_id", &self.key_id)
            .field("encoded", &"[redacted]")
            .finish()
    }
}

impl MutationTicket {
    /// Issue a fresh server-random operation ID and sign its complete scope.
    /// `expires_at` is exclusive; no clock skew is accepted at validation.
    pub fn issue(
        principal: PrincipalId,
        scope: MutationScope,
        issued_at: i64,
        expires_at: i64,
        recovery_epoch: i64,
        key: &TicketSigningKey,
    ) -> Result<Self, TicketIssueError> {
        if issued_at < 0 || expires_at <= issued_at || recovery_epoch < 0 {
            return Err(TicketIssueError::InvalidLifetime);
        }
        let mutation = MutationId::generate().map_err(|error| match error {
            IdError::EntropyUnavailable => TicketIssueError::EntropyUnavailable,
            IdError::Invalid => TicketIssueError::SigningUnavailable,
        })?;
        let mut ticket = Self {
            scoped_id: ScopedMutationId::new(principal, mutation),
            scope,
            issued_at,
            expires_at,
            recovery_epoch,
            key_id: key.id.clone(),
            encoded: String::new(),
        };
        let payload = URL_SAFE_NO_PAD.encode(ticket.payload_bytes());
        let mut mac = Hmac::<Sha256>::new_from_slice(&key.secret)
            .map_err(|_| TicketIssueError::SigningUnavailable)?;
        mac.update(TICKET_DOMAIN);
        mac.update(payload.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        ticket.encoded = format!("v{TICKET_VERSION}.{payload}.{signature}");
        if ticket.encoded.len() > MAX_TICKET_LENGTH {
            return Err(TicketIssueError::SigningUnavailable);
        }
        Ok(ticket)
    }

    /// Authenticate the envelope before applying caller scope, epoch or clock
    /// checks. Unknown keys and every time uncertainty fail closed.
    pub fn validate(
        encoded: &str,
        expectation: &TicketExpectation<'_>,
        keys: &[TicketSigningKey],
    ) -> Result<Self, TicketValidationError> {
        if encoded.is_empty() || encoded.len() > MAX_TICKET_LENGTH {
            return Err(TicketValidationError::Malformed);
        }
        let mut parts = encoded.split('.');
        let (Some(version), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(TicketValidationError::Malformed);
        };
        if version != format!("v{TICKET_VERSION}") {
            return Err(TicketValidationError::UnsupportedVersion);
        }
        let payload_bytes = decode_canonical_base64(payload)?;
        let signature = decode_canonical_base64(signature)?;
        let ticket = Self::parse_payload(&payload_bytes)?;
        let key = keys
            .iter()
            .find(|candidate| candidate.id == ticket.key_id)
            .ok_or(TicketValidationError::UnknownKey)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&key.secret)
            .map_err(|_| TicketValidationError::UnknownKey)?;
        mac.update(TICKET_DOMAIN);
        mac.update(payload.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| TicketValidationError::Tampered)?;

        if expectation.now < 0 {
            return Err(TicketValidationError::InvalidClock);
        }
        if ticket.issued_at < 0
            || ticket.expires_at <= ticket.issued_at
            || ticket.recovery_epoch < 0
        {
            return Err(TicketValidationError::InvalidLifetime);
        }
        if ticket.principal() != expectation.principal {
            return Err(TicketValidationError::WrongPrincipal);
        }
        if ticket.scope() != expectation.scope {
            return Err(TicketValidationError::WrongScope);
        }
        if ticket.recovery_epoch != expectation.recovery_epoch {
            return Err(TicketValidationError::EpochMismatch);
        }
        if ticket.issued_at > expectation.now {
            return Err(TicketValidationError::IssuedInFuture);
        }
        if ticket.expires_at <= expectation.now {
            return Err(TicketValidationError::Expired);
        }
        Ok(Self {
            encoded: encoded.to_owned(),
            ..ticket
        })
    }

    fn payload_bytes(&self) -> Vec<u8> {
        let mut payload =
            Vec::with_capacity(59 + self.scope.as_str().len() + self.key_id.as_str().len());
        payload.push(TICKET_VERSION);
        payload.extend_from_slice(&self.mutation_id().to_bytes());
        payload.extend_from_slice(&self.principal().to_bytes());
        payload.push(self.scope.as_str().len() as u8);
        payload.extend_from_slice(self.scope.as_str().as_bytes());
        payload.extend_from_slice(&self.issued_at.to_be_bytes());
        payload.extend_from_slice(&self.expires_at.to_be_bytes());
        payload.extend_from_slice(&self.recovery_epoch.to_be_bytes());
        payload.push(self.key_id.as_str().len() as u8);
        payload.extend_from_slice(self.key_id.as_str().as_bytes());
        payload
    }

    fn parse_payload(payload: &[u8]) -> Result<Self, TicketValidationError> {
        if payload.len() > MAX_PAYLOAD_LENGTH {
            return Err(TicketValidationError::Malformed);
        }
        let mut reader = PayloadReader::new(payload);
        if reader.u8()? != TICKET_VERSION {
            return Err(TicketValidationError::UnsupportedVersion);
        }
        let mutation = MutationId::from_bytes(reader.array::<16>()?);
        let principal = PrincipalId::from_bytes(reader.array::<16>()?);
        let scope_len = usize::from(reader.u8()?);
        let scope = std::str::from_utf8(reader.take(scope_len)?)
            .ok()
            .and_then(|value| MutationScope::parse(value).ok())
            .ok_or(TicketValidationError::Malformed)?;
        let issued_at = reader.i64()?;
        let expires_at = reader.i64()?;
        let recovery_epoch = reader.i64()?;
        let key_id_len = usize::from(reader.u8()?);
        let key_id = std::str::from_utf8(reader.take(key_id_len)?)
            .ok()
            .and_then(|value| TicketKeyId::parse(value).ok())
            .ok_or(TicketValidationError::Malformed)?;
        reader.finish()?;
        Ok(Self {
            scoped_id: ScopedMutationId::new(principal, mutation),
            scope,
            issued_at,
            expires_at,
            recovery_epoch,
            key_id,
            encoded: String::new(),
        })
    }

    pub fn scoped_id(&self) -> &ScopedMutationId {
        &self.scoped_id
    }

    pub fn principal(&self) -> &PrincipalId {
        self.scoped_id.principal()
    }

    pub fn mutation_id(&self) -> &MutationId {
        self.scoped_id.mutation_id()
    }

    pub fn scope(&self) -> &MutationScope {
        &self.scope
    }

    pub fn issued_at(&self) -> i64 {
        self.issued_at
    }

    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }

    pub fn recovery_epoch(&self) -> i64 {
        self.recovery_epoch
    }

    pub fn key_id(&self) -> &TicketKeyId {
        &self.key_id
    }

    pub fn as_str(&self) -> &str {
        &self.encoded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketIssueError {
    InvalidLifetime,
    EntropyUnavailable,
    SigningUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketValidationError {
    Malformed,
    UnsupportedVersion,
    UnknownKey,
    Tampered,
    WrongPrincipal,
    WrongScope,
    InvalidClock,
    InvalidLifetime,
    IssuedInFuture,
    Expired,
    EpochMismatch,
}

fn decode_canonical_base64(encoded: &str) -> Result<Vec<u8>, TicketValidationError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| TicketValidationError::Malformed)?;
    if URL_SAFE_NO_PAD.encode(&decoded) != encoded {
        return Err(TicketValidationError::Malformed);
    }
    Ok(decoded)
}

struct PayloadReader<'a> {
    payload: &'a [u8],
    position: usize,
}

impl<'a> PayloadReader<'a> {
    fn new(payload: &'a [u8]) -> Self {
        Self {
            payload,
            position: 0,
        }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], TicketValidationError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(TicketValidationError::Malformed)?;
        if end > self.payload.len() {
            return Err(TicketValidationError::Malformed);
        }
        let bytes = &self.payload[self.position..end];
        self.position = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], TicketValidationError> {
        self.take(N)?
            .try_into()
            .map_err(|_| TicketValidationError::Malformed)
    }

    fn u8(&mut self) -> Result<u8, TicketValidationError> {
        Ok(self.array::<1>()?[0])
    }

    fn i64(&mut self) -> Result<i64, TicketValidationError> {
        Ok(i64::from_be_bytes(self.array::<8>()?))
    }

    fn finish(&self) -> Result<(), TicketValidationError> {
        if self.position == self.payload.len() {
            Ok(())
        } else {
            Err(TicketValidationError::Malformed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    const PRINCIPAL: &str = "11111111111111111111111111111111";
    const MUTATION: &str = "22222222222222222222222222222222";

    #[test]
    fn mutation_ids_are_server_random_canonical_lower_hex() {
        let generated = MutationId::generate();
        assert!(
            generated.is_ok(),
            "the server must generate an operation ID"
        );
        let id = generated.unwrap();
        assert_eq!(id.as_str().len(), 32);
        assert!(
            id.as_str()
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_eq!(MutationId::parse(id.as_str()).unwrap(), id);
        assert_ne!(MutationId::generate().unwrap(), id);
    }

    #[test]
    fn scoped_identifiers_reject_noncanonical_components() {
        assert!(MutationId::parse(MUTATION).is_ok());
        assert!(PrincipalId::parse(PRINCIPAL).is_ok());
        assert!(MutationScope::parse("upload_create").is_ok());
        assert!(TicketKeyId::parse("key-2026-01").is_ok());

        for invalid in ["", "../secret", "A0000000000000000000000000000000", "1111"] {
            assert_eq!(MutationId::parse(invalid), Err(IdError::Invalid));
            assert_eq!(PrincipalId::parse(invalid), Err(IdError::Invalid));
        }
        let long_scope = "a".repeat(65);
        for invalid in [
            "",
            "UPLOAD",
            "upload create",
            "../upload",
            long_scope.as_str(),
        ] {
            assert_eq!(MutationScope::parse(invalid), Err(IdError::Invalid));
        }
        let long_key = "k".repeat(65);
        for invalid in ["", "key with spaces", "../key", long_key.as_str()] {
            assert_eq!(TicketKeyId::parse(invalid), Err(IdError::Invalid));
        }
    }

    #[test]
    fn scoped_mutation_identity_binds_principal_and_operation() {
        let principal = PrincipalId::parse(PRINCIPAL).unwrap();
        let mutation = MutationId::parse(MUTATION).unwrap();
        let scoped = ScopedMutationId::new(principal.clone(), mutation.clone());
        assert_eq!(scoped.principal(), &principal);
        assert_eq!(scoped.mutation_id(), &mutation);
    }

    fn principal(value: &str) -> PrincipalId {
        PrincipalId::parse(value).unwrap()
    }

    fn scope(value: &str) -> MutationScope {
        MutationScope::parse(value).unwrap()
    }

    fn signing_key() -> TicketSigningKey {
        TicketSigningKey::new(TicketKeyId::parse("key-2026-01").unwrap(), [7_u8; 32])
    }

    fn issue(issued_at: i64, expires_at: i64) -> MutationTicket {
        MutationTicket::issue(
            principal(PRINCIPAL),
            scope("upload_create"),
            issued_at,
            expires_at,
            7,
            &signing_key(),
        )
        .unwrap()
    }

    fn expectation<'a>(
        principal: &'a PrincipalId,
        scope: &'a MutationScope,
        now: i64,
    ) -> TicketExpectation<'a> {
        TicketExpectation::new(principal, scope, 7, now)
    }

    #[test]
    fn issued_ticket_round_trips_all_signed_fields() {
        let ticket = issue(900, 2_000);
        let principal = principal(PRINCIPAL);
        let scope = scope("upload_create");
        let expectation = expectation(&principal, &scope, 1_000);
        let key = signing_key();
        let validated =
            MutationTicket::validate(ticket.as_str(), &expectation, std::slice::from_ref(&key));
        assert!(
            validated.is_ok(),
            "an untampered in-scope ticket must validate"
        );
        let validated = validated.unwrap();
        assert_eq!(validated.scoped_id(), ticket.scoped_id());
        assert_eq!(validated.scope(), ticket.scope());
        assert_eq!(validated.issued_at(), 900);
        assert_eq!(validated.expires_at(), 2_000);
        assert_eq!(validated.recovery_epoch(), 7);
        assert_eq!(validated.key_id(), key.id());
        assert_eq!(validated.as_str(), ticket.as_str());
        assert!(ticket.as_str().starts_with("v1."));
        assert!(ticket.as_str().len() <= MAX_TICKET_LENGTH);
        assert!(!format!("{ticket:?}").contains(ticket.as_str()));
    }

    #[test]
    fn ticket_payload_tampering_is_detected_before_scope_checks() {
        let ticket = issue(900, 2_000);
        assert!(
            ticket.as_str().starts_with("v1."),
            "issue must produce a signed envelope"
        );
        let (prefix, signature) = ticket.as_str().rsplit_once('.').unwrap();
        let (version, payload) = prefix.split_once('.').unwrap();
        let mut decoded = URL_SAFE_NO_PAD.decode(payload).unwrap();
        let position = decoded
            .windows(b"upload_create".len())
            .position(|window| window == b"upload_create")
            .unwrap();
        decoded[position + b"upload_create".len() - 1] = b'f';
        let tampered = format!(
            "{version}.{}.{}",
            URL_SAFE_NO_PAD.encode(decoded),
            signature
        );
        let principal = principal(PRINCIPAL);
        let scope = scope("upload_create");
        assert_eq!(
            MutationTicket::validate(
                &tampered,
                &expectation(&principal, &scope, 1_000),
                &[signing_key()],
            ),
            Err(TicketValidationError::Tampered)
        );
    }

    #[test]
    fn ticket_validation_rejects_wrong_principal() {
        let ticket = issue(900, 2_000);
        let wrong_principal = principal("33333333333333333333333333333333");
        let scope = scope("upload_create");
        assert_eq!(
            MutationTicket::validate(
                ticket.as_str(),
                &expectation(&wrong_principal, &scope, 1_000),
                &[signing_key()],
            ),
            Err(TicketValidationError::WrongPrincipal)
        );
    }

    #[test]
    fn ticket_validation_rejects_wrong_scope() {
        let ticket = issue(900, 2_000);
        let principal = principal(PRINCIPAL);
        let wrong_scope = scope("folder_create");
        assert_eq!(
            MutationTicket::validate(
                ticket.as_str(),
                &expectation(&principal, &wrong_scope, 1_000),
                &[signing_key()],
            ),
            Err(TicketValidationError::WrongScope)
        );
    }

    #[test]
    fn ticket_validation_rejects_future_issuance_and_negative_clock() {
        let ticket = issue(1_001, 2_000);
        let principal = principal(PRINCIPAL);
        let scope = scope("upload_create");
        assert_eq!(
            MutationTicket::validate(
                ticket.as_str(),
                &expectation(&principal, &scope, 1_000),
                &[signing_key()],
            ),
            Err(TicketValidationError::IssuedInFuture)
        );
        assert_eq!(
            MutationTicket::validate(
                ticket.as_str(),
                &expectation(&principal, &scope, -1),
                &[signing_key()],
            ),
            Err(TicketValidationError::InvalidClock)
        );
    }

    #[test]
    fn ticket_validation_rejects_expiry_at_the_expiry_second() {
        let ticket = issue(900, 1_000);
        let principal = principal(PRINCIPAL);
        let scope = scope("upload_create");
        assert_eq!(
            MutationTicket::validate(
                ticket.as_str(),
                &expectation(&principal, &scope, 1_000),
                &[signing_key()],
            ),
            Err(TicketValidationError::Expired)
        );
    }

    #[test]
    fn ticket_validation_rejects_wrong_epoch() {
        let ticket = issue(900, 2_000);
        let principal = principal(PRINCIPAL);
        let scope = scope("upload_create");
        let expectation = TicketExpectation::new(&principal, &scope, 8, 1_000);
        assert_eq!(
            MutationTicket::validate(ticket.as_str(), &expectation, &[signing_key()]),
            Err(TicketValidationError::EpochMismatch)
        );
    }

    #[test]
    fn ticket_validation_rejects_unknown_keys_and_wrong_key_material() {
        let ticket = issue(900, 2_000);
        let principal = principal(PRINCIPAL);
        let scope = scope("upload_create");
        let expectation = expectation(&principal, &scope, 1_000);
        let unknown = TicketSigningKey::new(TicketKeyId::parse("key-other").unwrap(), [7_u8; 32]);
        assert_eq!(
            MutationTicket::validate(ticket.as_str(), &expectation, &[unknown]),
            Err(TicketValidationError::UnknownKey)
        );
        let wrong_secret =
            TicketSigningKey::new(TicketKeyId::parse("key-2026-01").unwrap(), [8_u8; 32]);
        assert_eq!(
            MutationTicket::validate(ticket.as_str(), &expectation, &[wrong_secret]),
            Err(TicketValidationError::Tampered)
        );
    }

    #[test]
    fn ticket_issue_rejects_invalid_lifetimes_and_epochs() {
        for (issued, expires, epoch) in [(1_000, 1_000, 7), (-1, 2_000, 7), (900, 2_000, -1)] {
            assert_eq!(
                MutationTicket::issue(
                    principal(PRINCIPAL),
                    scope("upload_create"),
                    issued,
                    expires,
                    epoch,
                    &signing_key(),
                ),
                Err(TicketIssueError::InvalidLifetime)
            );
        }
    }

    #[test]
    fn ticket_parser_rejects_wrong_versions_and_malformed_envelopes() {
        let principal = principal(PRINCIPAL);
        let scope = scope("upload_create");
        let expectation = expectation(&principal, &scope, 1_000);
        assert_eq!(
            MutationTicket::validate("v2.aa.bb", &expectation, &[signing_key()]),
            Err(TicketValidationError::UnsupportedVersion)
        );
        assert_eq!(
            MutationTicket::validate("v1.not-base64!", &expectation, &[signing_key()]),
            Err(TicketValidationError::Malformed)
        );
        let oversized = format!("v1.{}.bb", "a".repeat(MAX_TICKET_LENGTH));
        assert_eq!(
            MutationTicket::validate(&oversized, &expectation, &[signing_key()]),
            Err(TicketValidationError::Malformed)
        );
    }
}
