//! A signed copy ticket binds the operation to its source, destination and owner.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use super::{
    ids::MutationId,
    namespace::{self, NamespaceError},
};

const TICKET_SECONDS: i64 = 600;
const TICKET_DOMAIN: &[u8] = b"sylux:copy:v1\0";

#[derive(Debug)]
pub(crate) enum CopyError {
    InvalidTicket,
    Expired,
    Namespace(NamespaceError),
    Sql,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Ticket {
    owner: String,
    operation: String,
    source: String,
    destination: String,
    etag: String,
    expires_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CopyState {
    Unknown,
    Committed,
}

fn signer(key: &[u8; 32], body: &[u8]) -> Hmac<Sha256> {
    let mut mac = Hmac::new_from_slice(key).expect("fixed HMAC key");
    mac.update(TICKET_DOMAIN);
    mac.update(body);
    mac
}

impl Ticket {
    pub(crate) fn issue(
        key: &[u8; 32],
        owner: String,
        source: String,
        destination: String,
        etag: String,
        now: i64,
    ) -> Result<String, CopyError> {
        MutationId::parse(&source).map_err(|_| CopyError::Namespace(NamespaceError::NotFound))?;
        MutationId::parse(&destination)
            .map_err(|_| CopyError::Namespace(NamespaceError::NotFound))?;
        if etag.len() > 64 {
            return Err(CopyError::Namespace(NamespaceError::Stale));
        }
        let operation = MutationId::generate().map_err(|_| CopyError::Sql)?;
        let ticket = Self {
            owner,
            operation: operation.as_str().to_owned(),
            source,
            destination,
            etag,
            expires_at: now + TICKET_SECONDS,
        };
        let body = serde_json::to_vec(&ticket).map_err(|_| CopyError::Sql)?;
        let signature = signer(key, &body).finalize().into_bytes();
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(body),
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    pub(crate) fn verify(key: &[u8; 32], owner: &str, token: &str) -> Result<Self, CopyError> {
        if token.len() > 1024 {
            return Err(CopyError::InvalidTicket);
        }
        let (body, signature) = token.split_once('.').ok_or(CopyError::InvalidTicket)?;
        let body = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| CopyError::InvalidTicket)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| CopyError::InvalidTicket)?;
        signer(key, &body)
            .verify_slice(&signature)
            .map_err(|_| CopyError::InvalidTicket)?;
        let ticket: Self = serde_json::from_slice(&body).map_err(|_| CopyError::InvalidTicket)?;
        if ticket.owner != owner {
            return Err(CopyError::InvalidTicket);
        }
        Ok(ticket)
    }

    pub(crate) fn status(&self, db: &Connection) -> Result<CopyState, CopyError> {
        // The signed operation identifies the copied root. Its row survives even permanent removal.
        let committed = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM entry WHERE entry_id=?1 AND owner_account_id=?2)",
                params![self.operation, self.owner],
                |row| row.get(0),
            )
            .map_err(|_| CopyError::Sql)?;
        Ok(if committed {
            CopyState::Committed
        } else {
            CopyState::Unknown
        })
    }

    pub(crate) fn execute(&self, tx: &Transaction<'_>, now: i64) -> Result<CopyState, CopyError> {
        if matches!(self.status(tx)?, CopyState::Committed) {
            return Ok(CopyState::Committed);
        }
        if now >= self.expires_at {
            return Err(CopyError::Expired);
        }
        namespace::copy_entry(
            tx,
            &self.owner,
            &self.source,
            &self.destination,
            &self.etag,
            &self.operation,
            now,
        )
        .map_err(CopyError::Namespace)?;
        Ok(CopyState::Committed)
    }
}
