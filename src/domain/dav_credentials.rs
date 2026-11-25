use crate::domain::{authz, upload};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, params};

const PASSWORD_PREFIX: &str = "sylux.";

#[derive(Clone)]
pub(crate) struct DavProof {
    id: String,
    handle: String,
    verifier: [u8; 32],
}

impl DavProof {
    pub(crate) fn parse(handle: &str, password: &str) -> Option<Self> {
        let (id, secret) = password.strip_prefix(PASSWORD_PREFIX)?.split_once('.')?;
        if handle.is_empty() || handle.len() > 64 || secret.len() != 43 {
            return None;
        }
        upload::validate_id(id).ok()?;
        let secret: [u8; 32] = URL_SAFE_NO_PAD.decode(secret).ok()?.try_into().ok()?;
        Some(Self {
            id: id.to_owned(),
            handle: handle.to_owned(),
            verifier: authz::verifier(&secret),
        })
    }

    pub(crate) fn authenticate(&self, db: &Connection) -> rusqlite::Result<Option<DavGrant>> {
        let row = db
            .query_row(
                "SELECT a.account_id, c.read_only, c.verifier
                 FROM dav_credential c JOIN account a USING (account_id)
                 WHERE c.credential_id=?1 AND a.handle=?2 AND a.enabled=1
                   AND a.credential_mode='local' AND a.credential_epoch=c.credential_epoch",
                params![self.id, self.handle],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.and_then(|(account_id, read_only, verifier)| {
            authz::verifier_matches(&self.verifier, &verifier).then(|| DavGrant {
                proof: self.clone(),
                account_id,
                read_only,
            })
        }))
    }
}

#[derive(Clone)]
pub(crate) struct DavGrant {
    proof: DavProof,
    pub(crate) account_id: String,
    pub(crate) read_only: bool,
}

impl DavGrant {
    pub(crate) fn can_write(&self, db: &Connection) -> rusqlite::Result<bool> {
        Ok(!self.read_only
            && self
                .proof
                .authenticate(db)?
                .is_some_and(|grant| !grant.read_only && grant.account_id == self.account_id))
    }
}

pub(crate) fn generate() -> Result<(String, [u8; 32], String), &'static str> {
    let id = upload::random_id()?;
    let mut secret = [0; 32];
    getrandom::fill(&mut secret).map_err(|_| "entropy_unavailable")?;
    let password = format!("{PASSWORD_PREFIX}{id}.{}", URL_SAFE_NO_PAD.encode(secret));
    let verifier = authz::verifier(&secret);
    secret.fill(0);
    Ok((id, verifier, password))
}
