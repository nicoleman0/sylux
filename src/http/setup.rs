//! Host-authorized one-use creation of the initial owner and private root.
use crate::{
    domain::{authz, names::NAME_KEY_VERSION},
    http::AuthState,
};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::params;
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[cfg(feature = "test-harness")]
use std::sync::{Arc, Condvar, Mutex};

#[cfg(feature = "test-harness")]
#[derive(Default)]
pub struct SetupCommitTestBarrier {
    state: Mutex<SetupCommitBarrierState>,
    changed: Condvar,
}

#[cfg(feature = "test-harness")]
#[derive(Default)]
struct SetupCommitBarrierState {
    paused: bool,
    reached: bool,
    released: bool,
}

#[cfg(feature = "test-harness")]
impl SetupCommitTestBarrier {
    pub fn pause(&self) {
        let mut state = self.state.lock().expect("setup commit barrier poisoned");
        state.paused = true;
        state.reached = false;
        state.released = false;
    }

    pub fn wait_reached(&self) {
        let mut state = self.state.lock().expect("setup commit barrier poisoned");
        while !state.reached {
            state = self
                .changed
                .wait(state)
                .expect("setup commit barrier poisoned");
        }
    }

    pub fn release(&self) {
        let mut state = self.state.lock().expect("setup commit barrier poisoned");
        state.released = true;
        self.changed.notify_all();
    }

    fn pause_at_setup_commit(&self) {
        let mut state = self.state.lock().expect("setup commit barrier poisoned");
        if !state.paused {
            return;
        }
        state.reached = true;
        self.changed.notify_all();
        while !state.released {
            state = self
                .changed
                .wait(state)
                .expect("setup commit barrier poisoned");
        }
        state.paused = false;
    }
}

#[cfg(feature = "test-harness")]
static SETUP_COMMIT_BARRIER: std::sync::OnceLock<Mutex<Option<Arc<SetupCommitTestBarrier>>>> =
    std::sync::OnceLock::new();

#[cfg(feature = "test-harness")]
pub fn set_setup_commit_test_barrier(barrier: Option<Arc<SetupCommitTestBarrier>>) {
    *SETUP_COMMIT_BARRIER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("setup commit barrier poisoned") = barrier;
}

#[cfg(feature = "test-harness")]
fn pause_at_setup_commit() {
    let barrier = SETUP_COMMIT_BARRIER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("setup commit barrier poisoned")
        .clone();
    if let Some(barrier) = barrier {
        barrier.pause_at_setup_commit();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSetup {
    handle: String,
    password: String,
}
fn reply(status: StatusCode, code: &'static str) -> Response {
    (status, Json(serde_json::json!({"code":code}))).into_response()
}
fn authorized(headers: &HeaderMap, state: &AuthState, require_origin: bool) -> bool {
    let origin_ok = if require_origin {
        crate::http::middleware::origin_matches(headers, &state.origin)
    } else {
        crate::http::middleware::origin_absent_or_matches(headers, &state.origin)
    };
    if !origin_ok {
        return false;
    }
    let Some(v) = headers
        .get("x-setup-authorization")
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let expected: [u8; 32] = Sha256::digest(state.setup_authorization.as_bytes()).into();
    authz::verifier_matches(&expected, &authz::verifier(v.as_bytes()))
}
pub async fn status(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state, false) {
        return reply(StatusCode::NOT_FOUND, "not_found");
    }
    let consumed: Result<i64, _> = state
        .workers
        .read(|db| {
            db.query_row(
                "SELECT setup_consumed FROM installation WHERE singleton=1",
                [],
                |r| r.get::<_, i64>(0),
            )
        })
        .await;
    let Ok(consumed) = consumed else {
        return reply(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
    };
    if consumed == 1 {
        reply(StatusCode::NOT_FOUND, "not_found")
    } else {
        (
            StatusCode::OK,
            Json(serde_json::json!({"initialized":true})),
        )
            .into_response()
    }
}
pub async fn create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<AdminSetup>,
) -> Response {
    if !authorized(&headers, &state, true) {
        return reply(StatusCode::NOT_FOUND, "not_found");
    }
    if input.handle.is_empty()
        || input.handle.len() > 64
        || !input.handle.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || (i > 0 && matches!(b, b'.' | b'_' | b'-'))
        })
        || !authz::validate_password(&input.password)
    {
        return reply(StatusCode::BAD_REQUEST, "invalid_setup");
    }
    let Some(verifier) = state
        .limiter
        .hash_password(input.password.into_bytes())
        .await
    else {
        return reply(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
    };
    let mut ids = [0u8; 32];
    if getrandom::fill(&mut ids).is_err() {
        return reply(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
    }
    let account: String = ids[..16].iter().map(|b| format!("{b:02x}")).collect();
    let root: String = ids[16..].iter().map(|b| format!("{b:02x}")).collect();
    #[cfg(feature = "test-harness")]
    {
        // Recheck one-use state before the test seam; the writer transaction
        // repeats this check atomically with creation and verifier consumption.
        let available: Result<i64, _> = state
            .workers
            .read(|db| {
                db.query_row(
                    "SELECT setup_consumed FROM installation WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )
            })
            .await;
        if !matches!(available, Ok(0)) {
            return reply(StatusCode::NOT_FOUND, "not_found");
        }
        pause_at_setup_commit();
    }

    let setup_verifier: [u8; 32] = Sha256::digest(state.setup_authorization.as_bytes()).into();
    let result = state.workers.transact(move |tx| {
        let (consumed, stored_verifier): (i64, Vec<u8>) = tx.query_row(
            "SELECT setup_consumed,setup_verifier FROM installation WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if consumed != 0
            || !authz::verifier_matches(&setup_verifier, &stored_verifier)
        {
            return Ok(false);
        }
        tx.execute("INSERT INTO account(account_id,handle,role,enabled,credential_epoch,password_verifier) VALUES(?1,?2,'administrator',1,0,?3)",params![account,input.handle,verifier])?;
        tx.execute("INSERT INTO entry(entry_id,owner_account_id,parent_id,kind,original_name,name_key,name_key_version) VALUES(?1,?2,NULL,'folder','','',?3)",params![root,account,NAME_KEY_VERSION])?;
        tx.execute("UPDATE installation SET setup_consumed=1,setup_verifier=zeroblob(32) WHERE singleton=1 AND setup_consumed=0",[])?;
        Ok(true)
    }).await;
    match result {
        Ok(true) => reply(StatusCode::CREATED, "setup_complete"),
        Ok(false) => reply(StatusCode::NOT_FOUND, "not_found"),
        Err(_) => reply(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

#[cfg(all(test, feature = "test-harness"))]
mod setup_commit_barrier_tests {
    use super::*;
    use std::{sync::Arc, thread};

    #[test]
    fn setup_commit_barrier_can_pause_and_release() {
        let barrier = Arc::new(SetupCommitTestBarrier::default());
        barrier.pause();
        set_setup_commit_test_barrier(Some(Arc::clone(&barrier)));
        let worker = thread::spawn(pause_at_setup_commit);
        barrier.wait_reached();
        barrier.release();
        worker.join().expect("barrier worker should finish");
        set_setup_commit_test_barrier(None);
    }
}
