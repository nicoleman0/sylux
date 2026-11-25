#![allow(clippy::result_large_err)]

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

use super::{
    AuthState,
    auth::now,
    middleware::{decode_secret, origin_matches, session_cookie},
};
use crate::domain::{
    authz, direct_grants,
    namespace::{Entry, NamespaceError},
};

pub(super) fn private(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "code": code }))).into_response()
}

async fn read(
    state: AuthState,
    headers: HeaderMap,
    call: impl FnOnce(&Connection, &str) -> Result<Response, NamespaceError> + Send + 'static,
) -> Response {
    let Some(cookie) = session_cookie(&headers) else {
        return private(error(StatusCode::UNAUTHORIZED, "unauthorized"));
    };
    let result = state
        .workers
        .read(move |db| {
            let snapshot = db.unchecked_transaction()?;
            let Some((actor, _, _)) =
                authz::validate_session(&snapshot, &cookie.session_id, &cookie.secret, now())
            else {
                return Ok(error(StatusCode::UNAUTHORIZED, "unauthorized"));
            };
            let response = match call(&snapshot, &actor) {
                Ok(response) => response,
                Err(NamespaceError::NotFound) => error(StatusCode::NOT_FOUND, "not_found"),
                Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            };
            snapshot.commit()?;
            Ok(response)
        })
        .await;
    private(result.unwrap_or_else(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")))
}

pub(super) async fn incoming(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    read(state, headers, |db, actor| {
        let items = direct_grants::incoming(db, actor).map_err(|_| NamespaceError::Sql)?;
        Ok(Json(json!({ "items": items })).into_response())
    })
    .await
}

pub(super) async fn path(
    State(state): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    read(state, headers, move |db, actor| {
        let path = direct_grants::path(db, actor, &id)?;
        Ok(Json(json!({ "items": path.folders })).into_response())
    })
    .await
}

pub(super) async fn file(
    State(state): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    read(state, headers, move |db, actor| {
        Ok(Json(direct_grants::file(db, actor, &id)?).into_response())
    })
    .await
}

pub(super) async fn manage(
    State(state): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    read(state, headers, move |db, actor| {
        let items = direct_grants::manage(db, actor, &id)?;
        Ok(Json(json!({ "items": items })).into_response())
    })
    .await
}

async fn write(
    state: AuthState,
    headers: HeaderMap,
    call: impl FnOnce(&Transaction<'_>, &str) -> Result<Response, NamespaceError> + Send + 'static,
) -> Response {
    let Ok(guard) = state.coordinator.serialize() else {
        return private(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "mutation_unavailable",
        ));
    };
    drop(guard);
    if !origin_matches(&headers, &state.origin) {
        return private(error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let Some(cookie) = session_cookie(&headers) else {
        return private(error(StatusCode::UNAUTHORIZED, "unauthorized"));
    };
    let Some(csrf) = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .and_then(decode_secret)
    else {
        return private(error(StatusCode::FORBIDDEN, "forbidden"));
    };
    let result = state
        .workers
        .transact_result(move |tx| {
            let Some((actor, _, _)) =
                authz::validate_session(tx, &cookie.session_id, &cookie.secret, now())
            else {
                return Err(error(StatusCode::UNAUTHORIZED, "unauthorized"));
            };
            if !authz::csrf_matches(tx, &cookie.session_id, &csrf) {
                return Err(error(StatusCode::FORBIDDEN, "forbidden"));
            }
            call(tx, &actor).map_err(|failure| match failure {
                NamespaceError::NotFound => error(StatusCode::NOT_FOUND, "not_found"),
                _ => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            })
        })
        .await;
    private(match result {
        Ok(Ok(response) | Err(response)) => response,
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "mutation_unavailable"),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GrantAccount {
    handle: String,
}

pub(super) async fn grant(
    State(state): State<AuthState>,
    Path(root): Path<String>,
    headers: HeaderMap,
    Json(input): Json<GrantAccount>,
) -> Response {
    write(state, headers, move |tx, actor| {
        let grant = direct_grants::grant(tx, actor, &root, &input.handle)?;
        Ok(Json(grant).into_response())
    })
    .await
}

pub(super) async fn revoke(
    State(state): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    write(state, headers, move |tx, actor| {
        if !direct_grants::revoke(tx, actor, &id).map_err(|_| NamespaceError::Sql)? {
            return Err(NamespaceError::NotFound);
        }
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

#[derive(Serialize)]
pub(super) struct Confirmation {
    token: String,
    new_recipients: std::collections::BTreeSet<String>,
    lost_recipients: std::collections::BTreeSet<String>,
    carried_recipients: std::collections::BTreeSet<String>,
}

pub(super) enum ConfirmationError {
    Required(Confirmation),
    AudienceChanged,
    Sql,
}

impl IntoResponse for ConfirmationError {
    fn into_response(self) -> Response {
        private(match self {
            Self::Required(confirmation) => (
                StatusCode::CONFLICT,
                Json(json!({
                    "code": "sharing_confirmation_required",
                    "confirmation": confirmation
                })),
            )
                .into_response(),
            Self::AudienceChanged => error(StatusCode::CONFLICT, "sharing_audience_changed"),
            Self::Sql => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        })
    }
}

pub(super) struct ConfirmTree<'a> {
    pub key: Option<&'a [u8; 32]>,
    pub actor: &'a str,
    pub entry: &'a str,
    pub tag: &'a str,
    pub token: Option<&'a str>,
}

impl ConfirmTree<'_> {
    pub fn authorize(
        &self,
        db: &Transaction<'_>,
        source: &[Entry],
        destination: &[Entry],
    ) -> Result<(), ConfirmationError> {
        let impact = direct_grants::tree_impact(db, self.actor, self.entry, source, destination)
            .map_err(|_| ConfirmationError::Sql)?;
        if !impact.requires_confirmation() {
            return if self.token.is_some() {
                Err(ConfirmationError::AudienceChanged)
            } else {
                Ok(())
            };
        }
        let operation = if source.is_empty() { "restore" } else { "move" };
        let body = serde_json::to_vec(&(operation, self.actor, self.entry, self.tag, &impact))
            .map_err(|_| ConfirmationError::Sql)?;
        let key = self.key.ok_or(ConfirmationError::Sql)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| ConfirmationError::Sql)?;
        mac.update(b"sylux:direct-grant-tree-confirmation:v1\0");
        mac.update(&body);
        if let Some(signature) = self
            .token
            .filter(|token| token.len() == 43)
            .and_then(|token| URL_SAFE_NO_PAD.decode(token).ok())
            && mac.clone().verify_slice(&signature).is_ok()
        {
            return Ok(());
        }
        Err(ConfirmationError::Required(Confirmation {
            token: URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()),
            new_recipients: impact.new_recipients,
            lost_recipients: impact.lost_recipients,
            carried_recipients: impact.carried_recipients,
        }))
    }
}
