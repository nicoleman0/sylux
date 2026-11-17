//! Owner-scoped permanent removal with immutable metadata and signed snapshots.
use crate::domain::upload::{folder_etag, validate_id};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    pub etag: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Selection {
    Selected { items: Vec<Item> },
    All,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    version: u8,
    owner: String,
    fingerprint: String,
    selection: Selection,
    count: i64,
    expires_at: i64,
}
#[derive(Serialize)]
pub struct Preview {
    pub confirmation: String,
    pub count: i64,
}
#[derive(Serialize)]
pub struct ResultState {
    pub operation_id: String,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<&'static str>,
}
#[derive(Debug)]
pub enum RemovalError {
    Invalid,
    Changed,
    NotFound,
    Conflict,
    Sql,
}
impl From<rusqlite::Error> for RemovalError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Sql
    }
}
// Ancestor generations also change for unrelated siblings; hash their path only.
fn fingerprint(
    db: &Connection,
    owner: &str,
    selection: &Selection,
) -> Result<(String, i64), RemovalError> {
    let mut digest = Sha256::new();
    let mut count = 0;
    let mut after = String::new();
    loop {
        let page = roots(db, owner, selection, &after)?;
        if page.is_empty() {
            break;
        }
        for root in &page {
            count += 1;
            let mut stmt = db.prepare("WITH RECURSIVE tree(id) AS (
            SELECT entry_id FROM entry WHERE entry_id=?1 AND owner_account_id=?2
            UNION ALL SELECT e.entry_id FROM entry e JOIN tree t ON e.parent_id=t.id
            WHERE e.state='active' AND e.owner_account_id=?2
              AND NOT EXISTS(SELECT 1 FROM removal_member m WHERE m.entry_id=e.entry_id)
        ) SELECT e.entry_id,e.parent_id,e.original_name,e.kind,e.state,e.generation,e.current_revision_id
          FROM entry e JOIN tree t ON t.id=e.entry_id ORDER BY e.entry_id")?;
            let mut rows = stmt.query(params![root, owner])?;
            while let Some(row) = rows.next()? {
                let values: (
                    String,
                    Option<String>,
                    String,
                    String,
                    String,
                    i64,
                    Option<String>,
                ) = (
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                );
                digest.update(serde_json::to_vec(&values).map_err(|_| RemovalError::Sql)?);
            }
            let mut stmt = db.prepare("WITH RECURSIVE tree(id) AS (
                SELECT entry_id FROM entry WHERE entry_id=?1 AND owner_account_id=?2
                UNION ALL SELECT e.entry_id FROM entry e JOIN tree t ON e.parent_id=t.id
                WHERE e.state='active' AND e.owner_account_id=?2
                  AND NOT EXISTS(SELECT 1 FROM removal_member m WHERE m.entry_id=e.entry_id)
            ) SELECT r.entry_id,r.revision_id,r.blob_id FROM revision r JOIN tree t ON t.id=r.entry_id ORDER BY r.entry_id,r.revision_id")?;
            let mut rows = stmt.query(params![root, owner])?;
            while let Some(row) = rows.next()? {
                let values: (String, String, String) = (row.get(0)?, row.get(1)?, row.get(2)?);
                digest.update(serde_json::to_vec(&values).map_err(|_| RemovalError::Sql)?);
            }
            let mut stmt = db.prepare("WITH RECURSIVE ancestors(id,parent_id,original_name,kind,state) AS (
            SELECT p.entry_id,p.parent_id,p.original_name,p.kind,p.state FROM entry e JOIN entry p ON p.entry_id=e.parent_id WHERE e.entry_id=?1
            UNION ALL SELECT p.entry_id,p.parent_id,p.original_name,p.kind,p.state FROM entry p JOIN ancestors a ON a.parent_id=p.entry_id
        ) SELECT id,parent_id,original_name,kind,state FROM ancestors ORDER BY id")?;
            let mut rows = stmt.query([root])?;
            while let Some(row) = rows.next()? {
                let values: (String, Option<String>, String, String, String) = (
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                );
                digest.update(serde_json::to_vec(&values).map_err(|_| RemovalError::Sql)?);
            }
        }
        after = page.last().expect("nonempty roots page").clone();
    }
    Ok((URL_SAFE_NO_PAD.encode(digest.finalize()), count))
}

fn roots(
    db: &Connection,
    owner: &str,
    selection: &Selection,
    after: &str,
) -> Result<Vec<String>, RemovalError> {
    match selection {
        Selection::All => {
            let mut stmt = db.prepare("SELECT entry_id FROM entry WHERE owner_account_id=?1 AND state='deleted' AND entry_id>?2 AND NOT EXISTS(SELECT 1 FROM removal_member m WHERE m.entry_id=entry.entry_id) ORDER BY entry_id LIMIT 200")?;
            Ok(stmt
                .query_map(params![owner, after], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        }
        Selection::Selected { items } => {
            if !after.is_empty() {
                return Ok(Vec::new());
            }
            if items.is_empty() || items.len() > 200 {
                return Err(RemovalError::Invalid);
            }
            let mut result = std::collections::BTreeSet::new();
            for item in items {
                validate_id(&item.id).map_err(|_| RemovalError::Invalid)?;
                let generation:Option<i64>=db.query_row("SELECT generation FROM entry WHERE entry_id=?1 AND owner_account_id=?2 AND state='deleted' AND parent_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM removal_member m WHERE m.entry_id=entry.entry_id)",params![item.id,owner],|r|r.get(0)).optional()?;
                let generation = generation.ok_or(RemovalError::NotFound)?;
                if folder_etag(&item.id, generation as u64) != item.etag {
                    return Err(RemovalError::Changed);
                }
                if !result.insert(item.id.clone()) {
                    return Err(RemovalError::Invalid);
                }
            }
            Ok(result.into_iter().collect())
        }
    }
}
fn remove_root(
    tx: &Transaction<'_>,
    owner: &str,
    operation: &str,
    root: &str,
) -> Result<(), RemovalError> {
    tx.execute("WITH RECURSIVE tree(entry_id,kind) AS (SELECT entry_id,kind FROM entry WHERE entry_id=?1 UNION ALL SELECT e.entry_id,e.kind FROM entry e JOIN tree t ON e.parent_id=t.entry_id WHERE t.kind='folder' AND e.state='active' AND e.owner_account_id=?2) INSERT OR IGNORE INTO removal_member(entry_id,operation_id) SELECT entry_id,?3 FROM tree",params![root,owner,operation])?;
    Ok(())
}
fn sign(key: &[u8; 32], body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("fixed HMAC key");
    mac.update(b"sylux:trash-removal:v1\0");
    mac.update(body);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(body),
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}
pub fn preview(
    db: &Connection,
    owner: &str,
    selection: Selection,
    key: &[u8; 32],
    now: i64,
) -> Result<Preview, RemovalError> {
    let (fingerprint, count) = fingerprint(db, owner, &selection)?;
    if count == 0 {
        return Err(RemovalError::NotFound);
    }
    let body = serde_json::to_vec(&Confirmation {
        version: 1,
        owner: owner.into(),
        fingerprint,
        selection,
        count,
        expires_at: now + 600,
    })
    .map_err(|_| RemovalError::Invalid)?;
    Ok(Preview {
        confirmation: sign(key, &body),
        count,
    })
}
pub fn status(db: &Connection, owner: &str, operation: &str) -> Result<ResultState, RemovalError> {
    validate_id(operation).map_err(|_| RemovalError::Invalid)?;
    let count: Option<i64> = db
        .query_row(
            "SELECT removed_count FROM removal WHERE operation_id=?1 AND owner_account_id=?2",
            params![operation, owner],
            |r| r.get(0),
        )
        .optional()?;
    let cleanup = if count.is_some() {
        Some(if db.query_row("SELECT EXISTS(SELECT 1 FROM removal_cleanup WHERE operation_id=?1 AND state='pending')",[operation],|r|r.get::<_,bool>(0))?{"pending"}else{"complete"})
    } else {
        None
    };
    Ok(ResultState {
        operation_id: operation.into(),
        state: if count.is_some() {
            "committed"
        } else {
            "unknown"
        },
        count,
        cleanup,
    })
}
pub fn execute(
    tx: &Transaction<'_>,
    owner: &str,
    operation: &str,
    token: &str,
    key: &[u8; 32],
    now: i64,
) -> Result<ResultState, RemovalError> {
    validate_id(operation).map_err(|_| RemovalError::Invalid)?;
    if token.len() > 60000 {
        return Err(RemovalError::Invalid);
    }
    let token_hash = Sha256::digest(token.as_bytes());
    let existing: Option<(String, Vec<u8>)> = tx
        .query_row(
            "SELECT owner_account_id,confirmation_hash FROM removal WHERE operation_id=?1",
            [operation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((recorded_owner, hash)) = existing {
        if recorded_owner != owner {
            return status(tx, owner, operation);
        }
        if hash != token_hash.as_slice() {
            return Err(RemovalError::Conflict);
        }
        return status(tx, owner, operation);
    }
    let (body, signature) = token.split_once('.').ok_or(RemovalError::Invalid)?;
    let body = URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| RemovalError::Invalid)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| RemovalError::Invalid)?;
    let mut mac = HmacSha256::new_from_slice(key).expect("fixed HMAC key");
    mac.update(b"sylux:trash-removal:v1\0");
    mac.update(&body);
    mac.verify_slice(&signature)
        .map_err(|_| RemovalError::Invalid)?;
    let confirmation: Confirmation =
        serde_json::from_slice(&body).map_err(|_| RemovalError::Invalid)?;
    if confirmation.version != 1 || confirmation.owner != owner || now >= confirmation.expires_at {
        return Err(RemovalError::Invalid);
    }
    let (fingerprint, count) =
        fingerprint(tx, owner, &confirmation.selection).map_err(|e| match e {
            RemovalError::NotFound => RemovalError::Changed,
            other => other,
        })?;
    if fingerprint != confirmation.fingerprint {
        return Err(RemovalError::Changed);
    }
    if count != confirmation.count {
        return Err(RemovalError::Changed);
    }
    let selected_roots = match &confirmation.selection {
        Selection::Selected { .. } => roots(tx, owner, &confirmation.selection, "")?,
        Selection::All => Vec::new(),
    };
    tx.execute("INSERT INTO removal(operation_id,owner_account_id,confirmation_hash,removed_count,committed_at) VALUES(?1,?2,?3,?4,?5)",params![operation,owner,token_hash.as_slice(),confirmation.count,now])?;
    match confirmation.selection {
        Selection::Selected { .. } => {
            for root in selected_roots {
                remove_root(tx, owner, operation, &root)?;
            }
        }
        Selection::All => {
            let mut after = String::new();
            loop {
                let page = roots(tx, owner, &Selection::All, &after)?;
                if page.is_empty() {
                    break;
                }
                for root in page {
                    remove_root(tx, owner, operation, &root)?;
                    after = root;
                }
            }
        }
    }
    // Generation changes invalidate every cursor that could describe these tombstones.
    tx.execute("UPDATE entry SET generation=generation+1 WHERE entry_id IN(SELECT entry_id FROM removal_member WHERE operation_id=?1)",[operation])?;
    tx.execute("INSERT INTO removal_cleanup(blob_id,operation_id,state) SELECT DISTINCT r.blob_id,?1,'pending' FROM revision r JOIN removal_member m ON m.entry_id=r.entry_id JOIN blob b ON b.blob_id=r.blob_id WHERE m.operation_id=?1 AND b.lifecycle='live' AND NOT EXISTS(SELECT 1 FROM revision retained WHERE retained.blob_id=r.blob_id AND NOT EXISTS(SELECT 1 FROM removal_member gone WHERE gone.entry_id=retained.entry_id))",[operation])?;
    tx.execute("UPDATE blob SET lifecycle='deleting' WHERE blob_id IN(SELECT blob_id FROM removal_cleanup WHERE operation_id=?1)",[operation])?;
    status(tx, owner, operation)
}
