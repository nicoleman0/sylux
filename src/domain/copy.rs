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
    OtherAccount,
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
            return Err(CopyError::OtherAccount);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{names::EntryName, upload::folder_etag},
        persistence::schema::MIGRATION_0001_S1,
    };

    const OWNER: &str = "00000000000000000000000000000001";
    const ROOT: &str = "00000000000000000000000000000002";

    fn hex(n: usize) -> String {
        format!("{n:032x}")
    }

    fn fixture() -> (Connection, String, String, String) {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        db.execute_batch(MIGRATION_0001_S1).unwrap();
        crate::persistence::schema::migrate_v1(&db, None).unwrap();
        crate::persistence::schema::migrate_v2(&db, None).unwrap();
        crate::persistence::schema::migrate_v3(&db, None).unwrap();
        db.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch) VALUES(?1,?1,'user',1,0)", [OWNER]).unwrap();
        db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,NULL,'folder','',x'',1)", params![ROOT,OWNER]).unwrap();
        let folder = |id: &str, parent: &str, name: &str| {
            let n = EntryName::parse(name).unwrap();
            db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'folder',?4,?5,1)", params![id,OWNER,parent,n.display,n.key]).unwrap();
        };
        let src = hex(0x10);
        let dst = hex(0x11);
        folder(&src, ROOT, "source");
        folder(&dst, ROOT, "dest");
        db.execute("INSERT INTO blob(blob_id,shard,length,sha256,lifecycle) VALUES(?1,substr(?1,1,2),1,zeroblob(32),'live')", [hex(0xb)]).unwrap();
        for i in 0..2 {
            let e = hex(0x100000 + i);
            let r = hex(0x900000 + i);
            let n = EntryName::parse(&format!("f{i:07}")).unwrap();
            db.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,?3,'file',?4,?5,1)", params![e,OWNER,src,n.display,n.key]).unwrap();
            db.execute("INSERT INTO revision(revision_id,entry_id,blob_id,length,sha256,committed_at,actor_account_id) VALUES(?1,?2,?3,1,zeroblob(32),1,?4)", params![r,e,hex(0xb),OWNER]).unwrap();
            db.execute(
                "UPDATE entry SET current_revision_id=?2 WHERE entry_id=?1",
                params![e, r],
            )
            .unwrap();
        }
        let generation: i64 = db
            .query_row(
                "SELECT generation FROM entry WHERE entry_id=?1",
                [&src],
                |r| r.get(0),
            )
            .unwrap();
        let etag = folder_etag(&src, generation as u64);
        (db, src, dst, etag)
    }

    #[test]
    fn ticket_expiry_and_committed_replay() {
        let key = [7u8; 32];
        let (mut db, src, dst, etag) = fixture();
        let token = Ticket::issue(&key, OWNER.into(), src, dst, etag, 1_000).unwrap();
        let ticket = Ticket::verify(&key, OWNER, &token).unwrap();
        let tx = db.transaction().unwrap();
        let expired = ticket.execute(&tx, 1_600);
        assert!(matches!(expired, Err(CopyError::Expired)));
        let ok = ticket.execute(&tx, 1_599).unwrap();
        assert!(matches!(ok, CopyState::Committed));
        let replay = ticket.execute(&tx, 99_999).unwrap();
        assert!(matches!(replay, CopyState::Committed));
        assert!(matches!(
            Ticket::verify(&key, "00000000000000000000000000000009", &token),
            Err(CopyError::OtherAccount)
        ));
        tx.commit().unwrap();
    }
}
