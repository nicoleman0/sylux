mod lifecycle;
mod protocol;

use super::{
    AuthState, accounts, auth,
    middleware::{self, SessionCookie},
};
use crate::domain::authz;
use axum::{
    Json, Router,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use huskarl::grant::authorization_code::{AuthorizationCodeGrant, CompleteInput, PendingState};
use rusqlite::OptionalExtension;
use std::{
    collections::{HashMap, HashSet},
    path::Path as FilePath,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

pub(super) use lifecycle::audit;
pub(crate) use lifecycle::remove_registration;
use lifecycle::{Admission, Approval, Revoke, Session, Snapshot};
pub use protocol::ProtocolError;
use protocol::Provider;

const COOKIE: &str = "__Host-sylux-oidc";
const DENIED: StatusCode = StatusCode::UNAUTHORIZED;
const INTERNAL: StatusCode = StatusCode::INTERNAL_SERVER_ERROR;
const UNAVAILABLE: StatusCode = StatusCode::SERVICE_UNAVAILABLE;
const MAX_ATTEMPTS: usize = 128;
const MAX_PROVIDER_FLOWS: usize = 8;

const ATTEMPT: Duration = Duration::from_secs(5 * 60);
const SESSION_SECONDS: i64 = 15 * 60;

pub(super) struct Oidc {
    provider: Provider,
    registration: [u8; 32],
    attempts: Mutex<HashMap<[u8; 32], AttemptSlot>>,
    provider_flows: Semaphore,
}

struct AttemptSlot {
    expires: Instant,
    pending: Option<Attempt>,
}

struct Exchange {
    oidc: Arc<Oidc>,
    binding: [u8; 32],
}

impl Exchange {
    fn is_current(&self) -> Result<bool, StatusCode> {
        let attempts = self.oidc.attempts.lock().map_err(|_| INTERNAL)?;
        Ok(attempts.contains_key(&self.binding))
    }
}

impl Drop for Exchange {
    fn drop(&mut self) {
        let _ = self.oidc.cancel_attempt(self.binding);
    }
}

impl Oidc {
    pub(super) fn cancel_attempt(&self, binding: [u8; 32]) -> Result<(), StatusCode> {
        self.attempts.lock().map_err(|_| INTERNAL)?.remove(&binding);
        Ok(())
    }
}

struct Attempt {
    started: i64,
    handle: String,
    target: Option<Snapshot>,
    local: Option<SessionCookie>,
    grant: AuthorizationCodeGrant,
    pending: PendingState,
}

pub(super) async fn configure(state: &mut AuthState, path: &FilePath) -> Result<(), ProtocolError> {
    let path = path.to_owned();
    let origin = state.origin.clone();
    let provider = tokio::task::spawn_blocking(move || Provider::load(&path, &origin))
        .await
        .map_err(|_| ProtocolError::Unavailable)??;
    let registration = provider.registration();
    lifecycle::register(&state.workers, registration).await?;
    state.oidc = Some(Arc::new(Oidc {
        registration,
        provider,
        attempts: Mutex::default(),
        provider_flows: Semaphore::new(MAX_PROVIDER_FLOWS),
    }));
    Ok(())
}

pub(super) fn routes(app: Router<AuthState>) -> Router<AuthState> {
    app.route("/auth/oidc/status", get(status))
        .route("/auth/oidc/start", post(start))
        .route(protocol::CALLBACK, get(callback))
        .route("/auth/oidc/session", get(session))
        .route("/auth/oidc/approvals", get(list).post(approve))
        .route("/auth/oidc/approvals/{id}/revoke", post(revoke))
}

async fn status(State(state): State<AuthState>) -> Response {
    private(Json(serde_json::json!({ "enabled": state.oidc.is_some() })).into_response())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    handle: String,
    conversion_password: Option<String>,
}

async fn check_local_proof(
    state: &AuthState,
    headers: &HeaderMap,
    target: Option<&Snapshot>,
    password: String,
) -> Result<SessionCookie, StatusCode> {
    let (cookie, csrf) = accounts::credentials(headers, state, true)?;
    let account = target
        .and_then(|x| x.account.as_ref())
        .filter(|x| x.enabled && x.local)
        .ok_or(DENIED)?;
    let id = account.id.clone();
    let epoch = account.epoch;
    let probe = SessionCookie {
        session_id: cookie.session_id.clone(),
        secret: cookie.secret,
    };
    let encoded = state
        .workers
        .read(move |db| {
            if accounts::authorize(db, &probe, csrf.as_ref(), false)
                .ok()
                .as_ref()
                != Some(&id)
            {
                return Ok(None);
            }
            db.query_row(
                "SELECT password_verifier FROM account WHERE account_id=?1 AND credential_epoch=?2
             AND role='user' AND credential_mode='local'",
                rusqlite::params![id, epoch],
                |r| r.get(0),
            )
            .optional()
            .map(Option::flatten)
        })
        .await
        .map_err(|_| INTERNAL)?;
    let has_local_verifier = encoded.is_some();
    let valid = state
        .limiter
        .verify_login_async(password.into_bytes(), encoded)
        .await;
    if !has_local_verifier || !valid {
        return Err(DENIED);
    }
    Ok(cookie)
}

fn consume_anonymous(state: &AuthState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let browser = middleware::anonymous_cookie(headers).ok_or(DENIED)?;
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .and_then(middleware::decode_secret)
        .ok_or(StatusCode::FORBIDDEN)?;
    let mut sessions = state.anonymous.lock().map_err(|_| INTERNAL)?;
    let anonymous = sessions.get(&authz::verifier(&browser)).ok_or(DENIED)?;
    if anonymous.expires_at <= auth::now()
        || !authz::verifier_matches(&anonymous.csrf_verifier, &authz::verifier(&csrf))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    sessions.remove(&authz::verifier(&browser));
    Ok(())
}

async fn start(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<Start>,
) -> Response {
    match begin(&state, &headers, input).await {
        Ok(response) => response,
        Err(status) => failed(status),
    }
}

async fn begin(
    state: &AuthState,
    headers: &HeaderMap,
    input: Start,
) -> Result<Response, StatusCode> {
    let oidc = state.oidc.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    if !middleware::origin_matches(headers, &state.origin) {
        return Err(StatusCode::FORBIDDEN);
    }
    if !accounts::valid_handle(&input.handle) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if input.conversion_password.is_none() {
        consume_anonymous(state, headers)?;
    }
    let started = auth::now();
    let expires = Instant::now().checked_add(ATTEMPT).ok_or(INTERNAL)?;
    let handle = input.handle.clone();
    let issuer = oidc.provider.issuer.clone();
    let registration = oidc.registration;
    let target = state
        .workers
        .read(move |db| {
            if !lifecycle::registration_matches(db, &registration)? {
                return Ok(None);
            }
            lifecycle::snapshot(db, &issuer, &handle)
        })
        .await
        .map_err(|_| INTERNAL)?;
    let local = if let Some(password) = input.conversion_password {
        Some(check_local_proof(state, headers, target.as_ref(), password).await?)
    } else {
        None
    };
    let previous = attempt_binding(headers);
    {
        let mut attempts = oidc.attempts.lock().map_err(|_| INTERNAL)?;
        attempts.retain(|key, attempt| Some(*key) != previous && attempt.expires > Instant::now());
        if attempts.len() >= MAX_ATTEMPTS {
            return Err(UNAVAILABLE);
        }
    }
    let _permit = oidc.provider_flows.try_acquire().map_err(|_| UNAVAILABLE)?;
    let (grant, output) = oidc.provider.start().await.map_err(|_| UNAVAILABLE)?;
    let (browser, cookie) = auth::random_secret().ok_or(INTERNAL)?;
    let mut attempts = oidc.attempts.lock().map_err(|_| INTERNAL)?;
    let now = Instant::now();
    attempts.retain(|_, attempt| attempt.expires > now);
    if attempts.len() >= MAX_ATTEMPTS || expires <= now {
        return Err(UNAVAILABLE);
    }
    attempts.insert(
        authz::verifier(&browser),
        AttemptSlot {
            expires,
            pending: Some(Attempt {
                started,
                handle: input.handle,
                target,
                local,
                grant,
                pending: output.pending_state,
            }),
        },
    );
    let mut response = private(
        Json(serde_json::json!({ "authorization_url": output.authorization_url.to_string() }))
            .into_response(),
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        attempt_cookie(&cookie, (expires - now).as_secs().max(1))?,
    );
    Ok(response)
}

pub(super) fn attempt_binding(headers: &HeaderMap) -> Option<[u8; 32]> {
    middleware::cookie_value(headers, COOKIE)
        .and_then(|cookie| middleware::decode_secret(&cookie))
        .map(|secret| authz::verifier(&secret))
}

fn attempt_cookie(value: &str, lifetime: u64) -> Result<HeaderValue, StatusCode> {
    HeaderValue::from_str(&format!(
        "{COOKIE}={value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={lifetime}"
    ))
    .map_err(|_| INTERNAL)
}

async fn callback(
    State(state): State<AuthState>,
    method: Method,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    if method != Method::GET {
        return failed(StatusCode::METHOD_NOT_ALLOWED);
    }
    match finish(&state, &headers, query.as_deref().unwrap_or_default()).await {
        Ok(response) => response,
        Err(status) if browser_navigation(&headers) => {
            let location = if status == UNAVAILABLE {
                "/?oidc=unavailable"
            } else {
                "/?oidc=failed"
            };
            private((StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response())
        }
        Err(status) => failed(status),
    }
}

fn browser_navigation(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| {
            accept
                .split(',')
                .any(|value| value.trim().starts_with("text/html"))
        })
}

async fn finish(
    state: &AuthState,
    headers: &HeaderMap,
    query: &str,
) -> Result<Response, StatusCode> {
    let oidc = state.oidc.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let binding = attempt_binding(headers).ok_or(DENIED)?;
    let (expires, attempt) = oidc
        .attempts
        .lock()
        .map_err(|_| INTERNAL)?
        .get_mut(&binding)
        .and_then(|slot| slot.pending.take().map(|attempt| (slot.expires, attempt)))
        .ok_or(DENIED)?;
    let exchange = Exchange {
        oidc: Arc::clone(oidc),
        binding,
    };
    if expires <= Instant::now() || !valid_callback(query) {
        return Err(DENIED);
    }
    let callback = CompleteInput::builder_from_callback(query)
        .map_err(|_| DENIED)?
        .build();
    let presented = middleware::session_cookie(headers);
    if let Some(local) = &attempt.local {
        let current = presented.as_ref().ok_or(DENIED)?;
        if current.session_id != local.session_id
            || !authz::verifier_matches(&current.secret, &local.secret)
        {
            return Err(DENIED);
        }
    }
    let _permit = oidc.provider_flows.try_acquire().map_err(|_| UNAVAILABLE)?;
    let proof = oidc
        .provider
        .complete(&attempt.grant, &attempt.pending, callback, attempt.started)
        .await
        .map_err(|failure| match failure {
            ProtocolError::InvalidProof => DENIED,
            _ => UNAVAILABLE,
        })?;
    if expires <= Instant::now() {
        return Err(DENIED);
    }
    let target = attempt.target.ok_or(DENIED)?;
    let expires_at = attempt
        .started
        .checked_add(SESSION_SECONDS)
        .ok_or(INTERNAL)?;
    let session_id = auth::random_session_id().ok_or(INTERNAL)?;
    let (secret, encoded) = auth::random_secret().ok_or(INTERNAL)?;
    let session = Session {
        id: session_id.clone(),
        verifier: authz::verifier(&secret),
        csrf: authz::verifier(&session_csrf(&secret)),
        expires_at,
    };
    let registration = oidc.registration;
    let issuer = oidc.provider.issuer.clone();
    state
        .workers
        .transact_result(move |db| {
            if expires <= Instant::now()
                || !exchange.is_current()?
                || !lifecycle::registration_matches(db, &registration).map_err(|_| INTERNAL)?
            {
                return Err(DENIED);
            }
            let local_proof = if let Some(local) = attempt.local {
                let account = accounts::authorize(db, &local, None, false)?;
                if target
                    .account
                    .as_ref()
                    .is_none_or(|target| target.id != account)
                {
                    return Err(DENIED);
                }
                true
            } else {
                if let Some(cookie) = presented
                    && authz::validate_session_result(
                        db,
                        &cookie.session_id,
                        &cookie.secret,
                        auth::now(),
                    )
                    .map_err(|_| INTERNAL)?
                    .is_some()
                {
                    return Err(DENIED);
                }
                false
            };
            lifecycle::admit(
                db,
                Admission {
                    issuer: &issuer,
                    subject: &proof.subject,
                    handle: &attempt.handle,
                    expected: &target,
                    local_proof,
                    session: &session,
                },
            )
        })
        .await
        .map_err(|_| INTERNAL)??;
    let response = (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response();
    let mut response = private(auth::with_cookie(
        response,
        &format!("{session_id}.{encoded}"),
        expires_at.saturating_sub(auth::now()).max(0),
    ));
    response
        .headers_mut()
        .append(header::SET_COOKIE, attempt_cookie("", 0)?);
    Ok(response)
}

fn valid_callback(query: &str) -> bool {
    let mut seen = HashSet::new();
    query.len() <= 8192
        && query.bytes().enumerate().all(|(i, byte)| {
            byte != b'%'
                || query
                    .as_bytes()
                    .get(i + 1..i + 3)
                    .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
        })
        && url::form_urlencoded::parse(query.as_bytes()).all(|(name, value)| {
            matches!(name.as_ref(), "code" | "state" | "iss" | "session_state")
                && !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_graphic())
                && (name != "state" || middleware::decode_secret(&value).is_some())
                && seen.insert(name)
        })
        && seen.contains("code")
        && seen.contains("state")
}

fn session_csrf(secret: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"Sylux OIDC session CSRF\0");
    hash.update(secret);
    hash.finalize().into()
}

async fn session(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !middleware::origin_absent_or_matches(&headers, &state.origin) {
        return failed(StatusCode::FORBIDDEN);
    }
    let Some(cookie) = middleware::session_cookie(&headers) else {
        return failed(DENIED);
    };
    match state.workers.read(move |db| {
        let csrf = session_csrf(&cookie.secret);
        let principal = authz::validate_session(db, &cookie.session_id, &cookie.secret, auth::now());
        Ok(principal.filter(|_| authz::csrf_matches(db, &cookie.session_id, &csrf)).map(|(id, handle, role)| {
            serde_json::json!({"account_id": id, "handle": handle, "role": role, "csrf_token": URL_SAFE_NO_PAD.encode(csrf)})
        }))
    }).await {
        Ok(Some(data)) => private(Json(data).into_response()),
        Ok(None) => failed(DENIED),
        Err(_) => failed(INTERNAL),
    }
}

async fn list(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let (cookie, _) = match accounts::credentials(&headers, &state, false) {
        Ok(value) => value,
        Err(status) => return failed(status),
    };
    match state
        .workers
        .read(move |db| {
            if let Err(status) = accounts::authorize(db, &cookie, None, true) {
                return Ok(Err(status));
            }
            lifecycle::list(db).map(Ok)
        })
        .await
    {
        Ok(Ok(links)) => private(Json(links).into_response()),
        Ok(Err(status)) => failed(status),
        Err(_) => failed(INTERNAL),
    }
}

async fn approve(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<Approval>,
) -> Response {
    let Some(oidc) = &state.oidc else {
        return failed(StatusCode::NOT_FOUND);
    };
    let (cookie, csrf) = match accounts::credentials(&headers, &state, true) {
        Ok(value) => value,
        Err(status) => return failed(status),
    };
    let issuer = oidc.provider.issuer.clone();
    let registration = oidc.registration;
    match state
        .workers
        .transact_result(move |db| {
            let actor = accounts::authorize(db, &cookie, csrf.as_ref(), true)?;
            if !lifecycle::registration_matches(db, &registration).map_err(|_| INTERNAL)? {
                return Err(UNAVAILABLE);
            }
            lifecycle::approve(db, &actor, &issuer, input)
        })
        .await
    {
        Ok(Ok(id)) => {
            private((StatusCode::CREATED, Json(serde_json::json!({"id": id}))).into_response())
        }
        Ok(Err(status)) => failed(status),
        Err(_) => failed(INTERNAL),
    }
}

async fn revoke(
    State(state): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(expected): Json<Revoke>,
) -> Response {
    let (cookie, csrf) = match accounts::credentials(&headers, &state, true) {
        Ok(value) => value,
        Err(status) => return failed(status),
    };
    private(accounts::response(
        state
            .workers
            .transact_result(move |db| {
                let actor = accounts::authorize(db, &cookie, csrf.as_ref(), true)?;
                lifecycle::revoke(db, &actor, &id, expected)
            })
            .await,
    ))
}

fn private(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn failed(status: StatusCode) -> Response {
    private(
        (
            status,
            Json(serde_json::json!({ "code": "external_authentication_failed" })),
        )
            .into_response(),
    )
}
