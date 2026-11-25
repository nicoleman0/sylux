use axum::http::StatusCode;
use rusqlite::{Connection, OptionalExtension, params};

use super::{ProtocolError, protocol::valid_subject};
use crate::{
    domain::names::NAME_KEY_VERSION,
    http::{accounts, auth::now},
    persistence::sqlite::SqliteWorkers,
};

const DENIED: StatusCode = StatusCode::UNAUTHORIZED;
const CONFLICT: StatusCode = StatusCode::CONFLICT;
const INTERNAL: StatusCode = StatusCode::INTERNAL_SERVER_ERROR;

const APPROVAL_SECONDS: i64 = 24 * 60 * 60;
const MAX_PENDING_APPROVALS: i64 = 128;

pub(super) async fn register(
    workers: &SqliteWorkers,
    registration: [u8; 32],
) -> Result<(), ProtocolError> {
    let matches = workers
        .transact(move |db| {
            db.execute(
                "INSERT INTO external_provider VALUES(1,?1) ON CONFLICT(singleton) DO NOTHING",
                [registration],
            )?;
            registration_matches(db, &registration)
        })
        .await
        .map_err(|_| ProtocolError::Unavailable)?;
    if !matches {
        return Err(ProtocolError::RegistrationChanged);
    }
    Ok(())
}

pub(crate) fn remove_registration(db: &rusqlite::Transaction<'_>) -> rusqlite::Result<bool> {
    let removed = db.execute("DELETE FROM external_provider WHERE singleton=1", [])?;
    if removed == 0 {
        return Ok(false);
    }
    let time = now();
    db.execute(
        "INSERT INTO event(action,occurred_at,outcome,target_account_id,external_identity_id)
         SELECT 'oidc.link_revoked',?1,'ok',account_id,identity_id
         FROM external_identity WHERE state<>'revoked'",
        [time],
    )?;
    db.execute(
        "UPDATE session SET revoked=1 WHERE external_identity_id IS NOT NULL AND revoked=0",
        [],
    )?;
    db.execute(
        "UPDATE account SET enabled=0,credential_epoch=credential_epoch+1
         WHERE credential_mode='external' AND enabled=1
           AND account_id IN (SELECT account_id FROM external_identity WHERE state='active')",
        [],
    )?;
    db.execute(
        "UPDATE external_identity SET state='revoked' WHERE state<>'revoked'",
        [],
    )?;
    db.execute(
        "INSERT INTO event(action,occurred_at,outcome) VALUES('oidc.provider_removed',?1,'ok')",
        [time],
    )?;
    Ok(true)
}

pub(super) fn registration_matches(
    db: &Connection,
    registration: &[u8; 32],
) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM external_provider WHERE registration=?1)",
        [registration],
        |r| r.get(0),
    )
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Approval {
    pub subject: String,
    pub handle: String,
    pub account_id: Option<String>,
}

#[derive(serde::Serialize)]
pub(super) struct Link {
    id: String,
    handle: String,
    pending: bool,
    expires_at: Option<i64>,
    generation: i64,
}

pub(super) fn list(db: &Connection) -> rusqlite::Result<Vec<Link>> {
    db.prepare(
        "SELECT identity_id,handle,state='pending',
                CASE WHEN state='pending' THEN approval_expires_at END,generation
         FROM external_identity WHERE state='active'
           OR (state='pending' AND approval_expires_at>?1)
         ORDER BY handle",
    )?
    .query_map([now()], |row| {
        Ok(Link {
            id: row.get(0)?,
            handle: row.get(1)?,
            pending: row.get(2)?,
            expires_at: row.get(3)?,
            generation: row.get(4)?,
        })
    })?
    .collect()
}

pub(super) fn approve(
    db: &Connection,
    actor: &str,
    issuer: &str,
    approval: Approval,
) -> Result<String, StatusCode> {
    if !accounts::valid_handle(&approval.handle) || !valid_subject(&approval.subject) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let time = now();
    db.execute(
        "INSERT INTO event(action,occurred_at,outcome,target_account_id,external_identity_id)
         SELECT 'oidc.approval_expired',?1,'ok',account_id,identity_id
         FROM external_identity WHERE state='pending' AND approval_expires_at<=?1",
        [time],
    )
    .map_err(|_| INTERNAL)?;
    db.execute(
        "UPDATE external_identity SET state='revoked'
         WHERE state='pending' AND approval_expires_at<=?1",
        [time],
    )
    .map_err(|_| INTERNAL)?;
    let pending: i64 = db
        .query_row(
            "SELECT count(*) FROM external_identity WHERE state='pending'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| INTERNAL)?;
    if pending >= MAX_PENDING_APPROVALS {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let expires_at = time.saturating_add(APPROVAL_SECONDS);
    let previous = db.query_row(
        "SELECT identity_id,account_id,handle,state FROM external_identity WHERE issuer=?1 AND subject=?2",
        params![issuer, approval.subject],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?, r.get::<_, String>(3)?)),
    ).optional().map_err(|_| INTERNAL)?;
    if previous.as_ref().is_some_and(|(_, owner, handle, state)| {
        state != "revoked" || owner != &approval.account_id || handle != &approval.handle
    }) {
        return Err(CONFLICT);
    }
    let account = db.query_row(
        "SELECT account_id,role,enabled,credential_epoch,credential_mode,password_verifier IS NOT NULL
         FROM account WHERE handle=?1",
        [&approval.handle],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?,
                r.get::<_, i64>(3)?, r.get::<_, String>(4)?, r.get::<_, bool>(5)?)),
    ).optional().map_err(|_| INTERNAL)?;
    let target_epoch = match (&approval.account_id, account) {
        (None, None) => None,
        (Some(expected), Some((id, role, enabled, epoch, mode, password)))
            if expected == &id
                && role == "user"
                && ((mode == "local" && enabled && password)
                    || (mode == "external" && !enabled && previous.is_some())) =>
        {
            Some(epoch)
        }
        _ => return Err(CONFLICT),
    };
    let collision: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM external_identity WHERE
            (account_id=?1 AND state IN ('pending','active')) OR (handle=?2 AND state='pending'))",
            params![approval.account_id, approval.handle],
            |r| r.get(0),
        )
        .map_err(|_| INTERNAL)?;
    if collision {
        return Err(CONFLICT);
    }
    let approver_epoch: i64 = db
        .query_row(
            "SELECT credential_epoch FROM account WHERE account_id=?1 AND role='administrator'
         AND enabled=1 AND credential_mode='local'",
            [actor],
            |r| r.get(0),
        )
        .map_err(|_| DENIED)?;
    let identity = if let Some((id, ..)) = previous {
        db.execute(
            "UPDATE external_identity SET state='pending',generation=generation+1,approved_by=?1,
             approver_epoch=?2,target_epoch=?3,approval_expires_at=?4 WHERE identity_id=?5",
            params![actor, approver_epoch, target_epoch, expires_at, id],
        )
        .map_err(|_| INTERNAL)?;
        id
    } else {
        let id = accounts::random_id()?;
        db.execute(
            "INSERT INTO external_identity(
                identity_id,issuer,subject,account_id,handle,state,generation,approved_by,approver_epoch,target_epoch,approval_expires_at
             ) VALUES(?1,?2,?3,?4,?5,'pending',0,?6,?7,?8,?9)",
            params![id, issuer, approval.subject, approval.account_id, approval.handle, actor, approver_epoch, target_epoch, expires_at],
        ).map_err(|_| INTERNAL)?;
        id
    };
    audit(
        db,
        Some(actor),
        "oidc.link_approved",
        approval.account_id.as_deref(),
        &identity,
    )?;
    Ok(identity)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Revoke {
    pending: bool,
    generation: i64,
}

pub(super) fn revoke(
    db: &Connection,
    actor: &str,
    identity: &str,
    expected: Revoke,
) -> Result<(), StatusCode> {
    let state = if expected.pending {
        "pending"
    } else {
        "active"
    };
    let account = db
        .query_row(
            "UPDATE external_identity SET state='revoked'
             WHERE identity_id=?1 AND state=?2 AND generation=?3
         RETURNING account_id",
            params![identity, state, expected.generation],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|_| INTERNAL)?
        .ok_or(CONFLICT)?;
    if let Some(id) = &account {
        db.execute(
            "UPDATE account SET enabled=0,credential_epoch=credential_epoch+1
             WHERE account_id=?1 AND credential_mode='external' AND enabled=1",
            [id],
        )
        .map_err(|_| INTERNAL)?;
        db.execute(
            "UPDATE session SET revoked=1 WHERE external_identity_id=?1",
            [identity],
        )
        .map_err(|_| INTERNAL)?;
    }
    audit(
        db,
        Some(actor),
        "oidc.link_revoked",
        account.as_deref(),
        identity,
    )
}

pub(in crate::http) fn audit(
    db: &Connection,
    actor: Option<&str>,
    action: &'static str,
    target: Option<&str>,
    identity: &str,
) -> Result<(), StatusCode> {
    db.execute(
        "INSERT INTO event(actor_account_id,action,occurred_at,outcome,target_account_id,external_identity_id)
         VALUES(?1,?2,?3,'ok',?4,?5)",
        params![actor, action, now(), target, identity],
    ).map_err(|_| INTERNAL)?;
    Ok(())
}

#[derive(PartialEq)]
pub(super) struct Snapshot {
    pub identity_id: String,
    pub generation: i64,
    pub account: Option<Account>,
    pending: bool,
    approver: String,
    approver_epoch: i64,
    approved_epoch: Option<i64>,
}

#[derive(PartialEq)]
pub(super) struct Account {
    pub id: String,
    pub epoch: i64,
    pub local: bool,
    pub enabled: bool,
}

pub(super) fn snapshot(
    db: &Connection,
    issuer: &str,
    handle: &str,
) -> rusqlite::Result<Option<Snapshot>> {
    db.query_row(
        "SELECT x.identity_id,x.generation,x.state='pending',x.approved_by,x.approver_epoch,x.target_epoch,
                a.account_id,a.credential_epoch,a.credential_mode='local',a.enabled
         FROM external_identity x LEFT JOIN account a ON x.account_id=a.account_id
         WHERE x.issuer=?1 AND x.handle=?2
           AND (x.state='active' OR (x.state='pending' AND x.approval_expires_at>?3))
           AND (a.account_id IS NULL OR a.role='user')",
        params![issuer,handle,now()],
        |r| {
            let id: Option<String> = r.get(6)?;
            let account = if let Some(id) = id {
                Some(Account { id, epoch: r.get(7)?, local: r.get(8)?, enabled: r.get(9)? })
            } else {
                None
            };
            Ok(Snapshot {
                identity_id: r.get(0)?, generation: r.get(1)?, pending: r.get(2)?,
                approver: r.get(3)?, approver_epoch: r.get(4)?, approved_epoch: r.get(5)?, account,
            })
        },
    ).optional()
}

pub(super) struct Session {
    pub id: String,
    pub verifier: [u8; 32],
    pub csrf: [u8; 32],
    pub expires_at: i64,
}

pub(super) fn admit(db: &Connection, admission: Admission<'_>) -> Result<(), StatusCode> {
    let Admission {
        issuer,
        subject,
        handle,
        expected,
        local_proof,
        session,
    } = admission;
    let current = snapshot(db, issuer, handle)
        .map_err(|_| INTERNAL)?
        .ok_or(DENIED)?;
    let eligible_key: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM external_identity WHERE identity_id=?1 AND issuer=?2 AND subject=?3)",
        params![current.identity_id, issuer, subject], |r| r.get(0),
    ).map_err(|_| INTERNAL)?;
    let time = now();
    if !eligible_key || &current != expected || session.expires_at <= time {
        return Err(DENIED);
    }

    let (account, epoch, generation) = if current.pending {
        activate(db, &current, handle, local_proof)?
    } else {
        let account = current
            .account
            .as_ref()
            .filter(|a| a.enabled && !a.local)
            .ok_or(DENIED)?;
        (account.id.clone(), account.epoch, current.generation)
    };
    db.execute(
        "INSERT INTO session(session_id,account_id,credential_epoch,verifier,csrf_verifier,created_at,
            idle_expires_at,absolute_expires_at,external_identity_id,external_generation)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?7,?8,?9)",
        params![session.id,account,epoch,session.verifier,session.csrf,time,
            session.expires_at,current.identity_id,generation],
    ).map_err(|_| INTERNAL)?;
    audit(
        db,
        None,
        "oidc.session_issued",
        Some(&account),
        &current.identity_id,
    )
}

pub(super) struct Admission<'a> {
    pub issuer: &'a str,
    pub subject: &'a str,
    pub handle: &'a str,
    pub expected: &'a Snapshot,
    pub local_proof: bool,
    pub session: &'a Session,
}

fn activate(
    db: &Connection,
    identity: &Snapshot,
    handle: &str,
    local_proof: bool,
) -> Result<(String, i64, i64), StatusCode> {
    let authorized: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM account WHERE account_id=?1 AND enabled=1
         AND role='administrator' AND credential_mode='local' AND credential_epoch=?2)",
            params![identity.approver, identity.approver_epoch],
            |r| r.get(0),
        )
        .map_err(|_| INTERNAL)?;
    if !authorized {
        return Err(DENIED);
    }
    let (account, epoch) = match &identity.account {
        Some(account)
            if Some(account.epoch) == identity.approved_epoch
                && ((account.local && account.enabled && local_proof)
                    || (!account.local && !account.enabled)) =>
        {
            let epoch = account.epoch.checked_add(1).ok_or(INTERNAL)?;
            db.execute(
                "UPDATE account SET password_verifier=NULL,enabled=1,credential_mode='external',
                   credential_epoch=?1 WHERE account_id=?2",
                params![epoch, account.id],
            )
            .map_err(|_| INTERNAL)?;
            db.execute(
                "UPDATE session SET revoked=1 WHERE account_id=?1",
                [&account.id],
            )
            .map_err(|_| INTERNAL)?;
            db.execute(
                "DELETE FROM dav_credential WHERE account_id=?1",
                [&account.id],
            )
            .map_err(|_| INTERNAL)?;
            (account.id.clone(), epoch)
        }
        Some(_) => return Err(DENIED),
        None => {
            let occupied: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM account WHERE handle=?1)",
                    [handle],
                    |r| r.get(0),
                )
                .map_err(|_| INTERNAL)?;
            if occupied {
                return Err(DENIED);
            }
            let account = accounts::random_id()?;
            let root = accounts::random_id()?;
            db.execute(
                "INSERT INTO account(account_id,handle,role,enabled,credential_epoch,credential_mode)
                 VALUES(?1,?2,'user',1,0,'external')", params![account,handle],
            ).map_err(|_| INTERNAL)?;
            db.execute(
                "INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version)
                 VALUES(?1,?2,NULL,'folder','','',?3)", params![root,account,NAME_KEY_VERSION],
            ).map_err(|_| INTERNAL)?;
            (account, 0)
        }
    };
    let generation = identity.generation.checked_add(1).ok_or(INTERNAL)?;
    db.execute(
        "UPDATE external_identity SET state='active',account_id=?1,target_epoch=?2,generation=?3
         WHERE identity_id=?4",
        params![account, epoch, generation, identity.identity_id],
    )
    .map_err(|_| INTERNAL)?;
    audit(
        db,
        None,
        "oidc.link_activated",
        Some(&account),
        &identity.identity_id,
    )?;
    Ok((account, epoch, generation))
}
