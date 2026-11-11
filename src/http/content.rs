#![allow(clippy::result_large_err)]
// Axum responses are the deliberate non-allocating control-flow error channel
// for these request-local database/validation helpers.

use super::{AuthState, middleware::session_cookie};
use crate::domain::{
    authz::validate_session,
    upload::{revision_etag, validate_id},
};
use crate::persistence::payload::PayloadError;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{
            ACCEPT_RANGES, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE,
            CONTENT_TYPE, ETAG, IF_NONE_MATCH, IF_RANGE, RANGE, REFERRER_POLICY,
            X_CONTENT_TYPE_OPTIONS,
        },
    },
    response::{IntoResponse, Response},
};
use rusqlite::params;
use serde_json::json;
use std::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

struct ContentRef {
    name: String,
    revision_id: String,
    blob_id: String,
    length: u64,
    sha256: [u8; 32],
}

fn finish(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn error(status: StatusCode, code: &'static str) -> Response {
    finish(
        (
            status,
            JsonBody(json!({"code":code,"message":code,"request_id":"poc"})),
        )
            .into_response(),
    )
}

struct JsonBody(serde_json::Value);

impl IntoResponse for JsonBody {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

fn one_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let name = axum::http::HeaderName::from_bytes(name.as_bytes()).ok()?;
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}

async fn resolve(
    state: &AuthState,
    headers: &HeaderMap,
    entry_id: &str,
) -> Result<ContentRef, Response> {
    let cookie = session_cookie(headers)
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "authentication_required"))?;
    let session_id = cookie.session_id;
    let secret = cookie.secret;
    let entry_id = entry_id.to_owned();
    state.workers.read(move |db| {
        let Some((account_id, _, _)) = validate_session(db, &session_id, &secret, now()) else {
            return Ok(Err(error(StatusCode::UNAUTHORIZED, "authentication_required")));
        };
        let content = db.query_row(
            "SELECT entry.original_name,revision.revision_id,blob.blob_id,blob.length,blob.sha256 FROM entry JOIN revision ON revision.revision_id=entry.current_revision_id JOIN blob ON blob.blob_id=revision.blob_id WHERE entry.entry_id=?1 AND entry.owner_account_id=?2 AND entry.state='active' AND entry.kind='file' AND revision.entry_id=entry.entry_id AND blob.lifecycle='live'",
            params![entry_id, account_id],
            |row| {
                let length: i64 = row.get(3)?;
                let digest: Vec<u8> = row.get(4)?;
                let sha256: [u8; 32] = digest.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?;
                if length < 0 { return Err(rusqlite::Error::InvalidQuery); }
                Ok(ContentRef { name: row.get(0)?, revision_id: row.get(1)?, blob_id: row.get(2)?, length: length as u64, sha256 })
            },
        ).map_err(|_| error(StatusCode::NOT_FOUND, "entry_not_found"));
        Ok(content)
    }).await.map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable"))?
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn attachment_filename(name: &str) -> String {
    let mut fallback = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if fallback.is_empty() {
        fallback.push_str("download");
    }
    let mut encoded = String::new();
    for byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    one_header(headers, IF_NONE_MATCH.as_str()).is_some_and(|value| {
        value == "*"
            || value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate.strip_prefix("W/").unwrap_or(candidate) == etag
            })
    })
}

enum ByteRange {
    Full,
    Single(u64, u64),
    Invalid,
}

fn parse_range(value: &str, length: u64) -> ByteRange {
    let Some(specification) = value.strip_prefix("bytes=") else {
        return ByteRange::Invalid;
    };
    // A syntactically valid, satisfiable multi-range request may be answered
    // with the complete entity. Unsatisfiable members do not invalidate a set
    // that contains another satisfiable member; malformed members do.
    if specification.contains(',') {
        let members = specification
            .split(',')
            .map(|member| parse_range_member(member.trim(), length))
            .collect::<Vec<_>>();
        if members
            .iter()
            .any(|member| matches!(member, RangeMember::Malformed))
        {
            return ByteRange::Invalid;
        }
        return if members
            .iter()
            .any(|member| matches!(member, RangeMember::Satisfiable(_, _)))
        {
            ByteRange::Full
        } else {
            ByteRange::Invalid
        };
    }
    match parse_range_member(specification, length) {
        RangeMember::Satisfiable(start, count) => ByteRange::Single(start, count),
        RangeMember::Unsatisfiable | RangeMember::Malformed => ByteRange::Invalid,
    }
}

enum RangeMember {
    Satisfiable(u64, u64),
    Unsatisfiable,
    Malformed,
}

fn parse_range_member(specification: &str, length: u64) -> RangeMember {
    let Some((first, last)) = specification.split_once('-') else {
        return RangeMember::Malformed;
    };
    if last.contains('-') {
        return RangeMember::Malformed;
    }
    if first.is_empty() {
        let Some(suffix) = decimal(last) else {
            return RangeMember::Malformed;
        };
        if suffix == 0 || length == 0 {
            return RangeMember::Unsatisfiable;
        }
        let count = suffix.min(length);
        return RangeMember::Satisfiable(length - count, count);
    }
    let Some(start) = decimal(first) else {
        return RangeMember::Malformed;
    };
    if last.is_empty() {
        return if start >= length {
            RangeMember::Unsatisfiable
        } else {
            RangeMember::Satisfiable(start, length - start)
        };
    }
    let Some(end) = decimal(last) else {
        return RangeMember::Malformed;
    };
    if end < start || start >= length {
        return RangeMember::Unsatisfiable;
    }
    let end = end.min(length - 1);
    RangeMember::Satisfiable(start, end - start + 1)
}

fn decimal(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn insert_header(
    response: &mut Response,
    name: axum::http::HeaderName,
    value: &str,
) -> Result<(), Response> {
    let value = HeaderValue::from_str(value)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "response_header_failed"))?;
    response.headers_mut().insert(name, value);
    Ok(())
}

async fn serve(state: AuthState, entry_id: String, headers: HeaderMap, head: bool) -> Response {
    if validate_id(&entry_id).is_err() {
        return error(StatusCode::NOT_FOUND, "entry_not_found");
    }
    let reference = match resolve(&state, &headers, &entry_id).await {
        Ok(reference) => reference,
        Err(response) => return response,
    };
    let storage = match state.storage.clone() {
        Some(storage) => storage,
        None => return error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"),
    };
    let blob_id = reference.blob_id.clone();
    let length = reference.length;
    let digest = reference.sha256;
    let verified = match tokio::task::spawn_blocking(move || {
        storage.payloads.open_verified(&blob_id, length, digest)
    })
    .await
    {
        Ok(Ok(verified)) => verified,
        Ok(Err(PayloadError::Missing | PayloadError::Integrity)) | Err(_) => {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "content_integrity_error");
        }
        Ok(Err(_)) => {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "content_integrity_error");
        }
    };
    let etag = revision_etag(&reference.revision_id);
    let mut response = if if_none_match(&headers, &etag) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        response
    } else {
        let range = one_header(&headers, RANGE.as_str()).map_or(ByteRange::Full, |range| {
            if one_header(&headers, IF_RANGE.as_str()).is_some_and(|value| value != etag) {
                ByteRange::Full
            } else {
                parse_range(range, reference.length)
            }
        });
        let (status, start, body_length) = match range {
            ByteRange::Full => (StatusCode::OK, 0, reference.length),
            ByteRange::Single(start, length) => (StatusCode::PARTIAL_CONTENT, start, length),
            ByteRange::Invalid => {
                let mut response = error(StatusCode::RANGE_NOT_SATISFIABLE, "invalid_range");
                let _ = insert_header(
                    &mut response,
                    CONTENT_RANGE,
                    &format!("bytes */{}", reference.length),
                );
                return response;
            }
        };
        let body = if head {
            Body::empty()
        } else {
            let mut file = tokio::fs::File::from_std(verified.file);
            if file.seek(SeekFrom::Start(start)).await.is_err() {
                return error(StatusCode::INTERNAL_SERVER_ERROR, "content_stream_failed");
            }
            Body::from_stream(ReaderStream::new(file.take(body_length)))
        };
        let mut response = Response::new(body);
        *response.status_mut() = status;
        if status == StatusCode::PARTIAL_CONTENT {
            let end = start + body_length - 1;
            if let Err(response) = insert_header(
                &mut response,
                CONTENT_RANGE,
                &format!("bytes {start}-{end}/{}", reference.length),
            ) {
                return response;
            }
        }
        if let Err(response) =
            insert_header(&mut response, CONTENT_LENGTH, &body_length.to_string())
        {
            return response;
        }
        response
    };
    for (name, value) in [
        (CONTENT_TYPE, "application/octet-stream".to_owned()),
        (X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        (CACHE_CONTROL, "no-store".to_owned()),
        (REFERRER_POLICY, "no-referrer".to_owned()),
        (ACCEPT_RANGES, "bytes".to_owned()),
        (ETAG, etag),
        (CONTENT_DISPOSITION, attachment_filename(&reference.name)),
    ] {
        if let Err(response) = insert_header(&mut response, name, &value) {
            return response;
        }
    }
    response
}

pub async fn get(
    State(state): State<AuthState>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve(state, entry_id, headers, false).await
}

pub async fn head(
    State(state): State<AuthState>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let mut response = serve(state, entry_id, headers, true).await;
    *response.body_mut() = Body::empty();
    response
}

#[cfg(test)]
mod tests {
    use super::{ByteRange, parse_range};

    #[test]
    fn multi_range_with_satisfiable_and_unsatisfiable_members_uses_full_response() {
        assert!(matches!(
            parse_range("bytes=0-1,99-100", 16),
            ByteRange::Full
        ));
    }

    #[test]
    fn malformed_multi_range_is_not_treated_as_a_supported_range_set() {
        assert!(matches!(
            parse_range("bytes=0-1,garbage", 16),
            ByteRange::Invalid
        ));
    }
}
