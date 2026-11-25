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
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

use lifecycle::{Admission, Approval, Session, Snapshot};
pub use protocol::ProtocolError;
use protocol::{Provider, ProviderConfig, VerifiedIdentity};

const COOKIE: &str = "__Host-sylux-oidc";
const DENIED: StatusCode = StatusCode::UNAUTHORIZED;
const INTERNAL: StatusCode = StatusCode::INTERNAL_SERVER_ERROR;
const UNAVAILABLE: StatusCode = StatusCode::SERVICE_UNAVAILABLE;
const MAX_ATTEMPTS: usize = 128;
const MAX_PROVIDER_FLOWS: usize = 8;

struct Policy {
    attempt: Duration,
    session: Duration,
    max_age: Option<Duration>,
    assurance: fn(Option<&str>, &[String]) -> bool,
}

impl Policy {
    fn admits(&self, identity: &VerifiedIdentity, started: i64) -> bool {
        if let Some(age) = self.max_age {
            let earliest = started.saturating_sub(i64::try_from(age.as_secs()).unwrap_or(i64::MAX));
            if !identity
                .auth_time
                .is_some_and(|time| time >= earliest && time <= auth::now())
            {
                return false;
            }
        }
        (self.assurance)(identity.acr.as_deref(), &identity.amr)
    }
}

pub(super) struct Oidc {
    provider: Provider,
    registration: [u8; 32],
    policy: Policy,
    attempts: Mutex<HashMap<[u8; 32], Attempt>>,
    provider_flows: Semaphore,
}

struct Attempt {
    started: i64,
    expires: Instant,
    handle: String,
    target: Option<Snapshot>,
    local: Option<SessionCookie>,
    grant: AuthorizationCodeGrant,
    pending: PendingState,
}

async fn configure(
    state: &mut AuthState,
    config: Option<(ProviderConfig, Policy)>,
) -> Result<(), ProtocolError> {
    let oidc = config
        .map(|(config, policy)| -> Result<_, ProtocolError> {
            let provider = Provider::new(config, &state.origin)?;
            Ok(Arc::new(Oidc {
                registration: provider.registration(),
                provider,
                policy,
                attempts: Mutex::default(),
                provider_flows: Semaphore::new(MAX_PROVIDER_FLOWS),
            }))
        })
        .transpose()?;
    lifecycle::set_registration(&state.workers, oidc.as_ref().map(|x| x.registration)).await?;
    state.oidc = oidc;
    Ok(())
}

pub(super) async fn disable(state: &mut AuthState) -> Result<(), ProtocolError> {
    configure(state, None).await
}

#[cfg(feature = "test-harness")]
pub struct TestOidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub secret: String,
    pub ca_pem: Vec<u8>,
    pub attempt: Duration,
    pub session: Duration,
}

#[cfg(feature = "test-harness")]
pub(super) async fn configure_test(
    state: &mut AuthState,
    config: TestOidcConfig,
) -> Result<(), ProtocolError> {
    if config.attempt.is_zero()
        || config.session.is_zero()
        || config.attempt.as_secs() > i64::MAX as u64
        || config.session.as_secs() > i64::MAX as u64
    {
        return Err(ProtocolError::Configuration);
    }
    configure(
        state,
        Some((
            ProviderConfig {
                issuer: config.issuer,
                client_id: config.client_id,
                secret: config.secret.into(),
                ca_pem: config.ca_pem,
            },
            Policy {
                attempt: config.attempt,
                session: config.session,
                max_age: Some(Duration::ZERO),
                // The disposable provider is a password-only qualification fixture.
                assurance: |_, _| true,
            },
        )),
    )
    .await
}

pub(super) fn routes(app: Router<AuthState>) -> Router<AuthState> {
    app.route("/auth/oidc/start", post(start))
        .route(protocol::CALLBACK, get(callback))
        .route("/auth/oidc/session", get(session))
        .route("/auth/oidc/approvals", post(approve))
        .route("/auth/oidc/approvals/{id}/revoke", post(revoke))
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
    if !state
        .limiter
        .verify_login_async(password.into_bytes(), encoded)
        .await
    {
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
    let started = auth::now();
    let expires = Instant::now()
        .checked_add(oidc.policy.attempt)
        .ok_or(INTERNAL)?;
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
        consume_anonymous(state, headers)?;
        None
    };
    let _permit = oidc.provider_flows.try_acquire().map_err(|_| UNAVAILABLE)?;
    let (grant, output) = oidc
        .provider
        .start(oidc.policy.max_age)
        .await
        .map_err(|_| UNAVAILABLE)?;
    let (browser, cookie) = auth::random_secret().ok_or(INTERNAL)?;
    let mut attempts = oidc.attempts.lock().map_err(|_| INTERNAL)?;
    let now = Instant::now();
    attempts.retain(|_, attempt| attempt.expires > now);
    if attempts.len() >= MAX_ATTEMPTS || expires <= now {
        return Err(UNAVAILABLE);
    }
    attempts.insert(
        authz::verifier(&browser),
        Attempt {
            started,
            expires,
            handle: input.handle,
            target,
            local,
            grant,
            pending: output.pending_state,
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
        Err(status) => failed(status),
    }
}

async fn finish(
    state: &AuthState,
    headers: &HeaderMap,
    query: &str,
) -> Result<Response, StatusCode> {
    let oidc = state.oidc.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let binding = middleware::cookie_value(headers, COOKIE)
        .and_then(|cookie| middleware::decode_secret(&cookie))
        .ok_or(DENIED)?;
    let attempt = oidc
        .attempts
        .lock()
        .map_err(|_| INTERNAL)?
        .remove(&authz::verifier(&binding))
        .ok_or(DENIED)?;
    if attempt.expires <= Instant::now() || !valid_callback(query) {
        return Err(DENIED);
    }
    let callback = CompleteInput::builder_from_callback(query)
        .map_err(|_| DENIED)?
        .build();
    if let Some(local) = &attempt.local {
        let current = middleware::session_cookie(headers).ok_or(DENIED)?;
        if current.session_id != local.session_id
            || !authz::verifier_matches(&current.secret, &local.secret)
        {
            return Err(DENIED);
        }
    } else if middleware::session_cookie(headers).is_some() {
        return Err(DENIED);
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
    if attempt.expires <= Instant::now() || !oidc.policy.admits(&proof, attempt.started) {
        return Err(DENIED);
    }
    let target = attempt.target.ok_or(DENIED)?;
    let expires_at = attempt
        .started
        .checked_add(
            oidc.policy
                .session
                .as_secs()
                .try_into()
                .map_err(|_| INTERNAL)?,
        )
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
            if attempt.expires <= Instant::now()
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
                lifecycle::revoke(db, &actor, &id)
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
    private((status, "external_authentication_failed").into_response())
}
