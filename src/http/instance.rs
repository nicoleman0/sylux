use super::{AuthState, accounts};
use crate::persistence::instance::{
    Appearance, AppearanceChange, AssetChange, AssetUpload, DEFAULT_NAME, LOGIN_IMAGE_BYTES,
    LOGO_BYTES, normalized_color, normalized_name,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{
        HeaderMap, StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, X_CONTENT_TYPE_OPTIONS},
    },
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeepAsset {
    keep: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum AssetInput {
    Keep(KeepAsset),
    Upload(AssetUpload),
    Remove,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppearanceUpdate {
    #[serde(deserialize_with = "required_option")]
    accent_color: Option<String>,
    #[serde(deserialize_with = "required_option")]
    login_background_color: Option<String>,
    logo: AssetInput,
    login_image: AssetInput,
}

fn required_option<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Option::deserialize(deserializer)
}

impl AppearanceUpdate {
    fn validate(self) -> Result<AppearanceChange, &'static str> {
        let color = |value: Option<String>| {
            value
                .map(|value| {
                    normalized_color(&value).ok_or("Use a six-digit hex color, such as #336699.")
                })
                .transpose()
        };
        let asset = |value, maximum_bytes| match value {
            AssetInput::Keep(KeepAsset { keep: true }) => Ok(AssetChange::Keep),
            AssetInput::Keep(_) => Err("Use keep: true to retain the current image."),
            AssetInput::Upload(upload) => upload
                .validate(maximum_bytes)
                .map(|asset| AssetChange::Replace(Some(asset))),
            AssetInput::Remove => Ok(AssetChange::Replace(None)),
        };
        Ok(AppearanceChange {
            accent_color: color(self.accent_color)?,
            login_background_color: color(self.login_background_color)?,
            logo: asset(self.logo, LOGO_BYTES)?,
            login_image: asset(self.login_image, LOGIN_IMAGE_BYTES)?,
        })
    }
}

fn name_response(name: String) -> Response {
    (
        [(CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"name": name})),
    )
        .into_response()
}

fn appearance_response(appearance: Appearance) -> Response {
    let url = |asset: crate::persistence::instance::Asset| {
        format!("/instance/appearance/assets/{}", asset.digest())
    };
    (
        [(CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({
            "accentColor": appearance.accent_color,
            "loginBackgroundColor": appearance.login_background_color,
            "logoUrl": appearance.logo.map(url),
            "loginImageUrl": appearance.login_image.map(url),
        })),
    )
        .into_response()
}

pub async fn get(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !super::middleware::origin_absent_or_matches(&headers, &state.origin) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(settings) = state.instance_settings else {
        return name_response(DEFAULT_NAME.to_owned());
    };
    match tokio::task::spawn_blocking(move || settings.name()).await {
        Ok(Ok(name)) => name_response(name),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn update(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<Update>,
) -> Response {
    let (cookie, csrf) = match accounts::credentials(&headers, &state, true) {
        Ok(value) => value,
        Err(status) => return status.into_response(),
    };
    let Some(settings) = state.instance_settings else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let saved = state
        .workers
        .serialize_writer(move |db| {
            if let Err(status) = accounts::authorize(db, &cookie, csrf.as_ref(), true) {
                return Ok(Err(status));
            }
            let Some(name) = normalized_name(&input.name) else {
                return Ok(Err(StatusCode::BAD_REQUEST));
            };
            Ok(settings
                .save(&name)
                .map(|()| name)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR))
        })
        .await;
    match saved {
        Ok(Ok(name)) => name_response(name),
        Ok(Err(status)) => status.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn appearance(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !super::middleware::origin_absent_or_matches(&headers, &state.origin) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(settings) = state.instance_settings else {
        return appearance_response(Appearance::default());
    };
    match tokio::task::spawn_blocking(move || settings.appearance()).await {
        Ok(Ok(appearance)) => appearance_response(appearance),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn update_appearance(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<AppearanceUpdate>,
) -> Response {
    let (cookie, csrf) = match accounts::credentials(&headers, &state, true) {
        Ok(value) => value,
        Err(status) => return status.into_response(),
    };
    let Some(settings) = state.instance_settings else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let cookie_copy = super::middleware::SessionCookie {
        session_id: cookie.session_id.clone(),
        secret: cookie.secret,
    };
    match state
        .workers
        .read(move |db| Ok(accounts::authorize(db, &cookie_copy, csrf.as_ref(), true)))
        .await
    {
        Ok(Ok(_)) => (),
        Ok(Err(status)) => return status.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    let change = match tokio::task::spawn_blocking(move || input.validate()).await {
        Ok(Ok(change)) => change,
        Ok(Err(error)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": error})),
            )
                .into_response();
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let saved = state
        .workers
        .serialize_writer(move |db| {
            if let Err(status) = accounts::authorize(db, &cookie, csrf.as_ref(), true) {
                return Ok(Err(status));
            }
            Ok(settings
                .save_appearance(change)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR))
        })
        .await;
    match saved {
        Ok(Ok(appearance)) => appearance_response(appearance),
        Ok(Err(status)) => status.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn appearance_asset(
    State(state): State<AuthState>,
    Path(digest): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !super::middleware::origin_absent_or_matches(&headers, &state.origin) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let missing = || (StatusCode::NOT_FOUND, [(CACHE_CONTROL, "no-store")]).into_response();
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return missing();
    }
    let Some(settings) = state.instance_settings else {
        return missing();
    };
    let appearance = match tokio::task::spawn_blocking(move || settings.appearance()).await {
        Ok(Ok(appearance)) => appearance,
        _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let Some(asset) = [appearance.logo, appearance.login_image]
        .into_iter()
        .flatten()
        .find(|asset| asset.digest() == digest)
    else {
        return missing();
    };
    (
        [
            (
                CACHE_CONTROL,
                "public, max-age=31536000, immutable".to_owned(),
            ),
            (CONTENT_TYPE, asset.media_type().to_owned()),
            (X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (ETAG, format!("\"{}\"", asset.digest())),
        ],
        asset.bytes().to_vec(),
    )
        .into_response()
}
