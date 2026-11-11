use axum::http::{HeaderMap, header::COOKIE};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

/// Parsed authenticated cookie; deliberately not `Debug` because of the bearer secret.
pub struct SessionCookie {
    pub session_id: String,
    pub secret: [u8; 32],
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for raw in headers.get_all(COOKIE) {
        let raw = raw.to_str().ok()?;
        for pair in raw.split(';') {
            let Some((key, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if key == name {
                if found.is_some() {
                    return None;
                }
                found = Some(value.to_owned());
            }
        }
    }
    found
}

pub fn decode_secret(value: &str) -> Option<[u8; 32]> {
    if value.len() != 43 {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(value).ok()?;
    decoded.try_into().ok()
}

pub fn anonymous_cookie(headers: &HeaderMap) -> Option<[u8; 32]> {
    let value = cookie_value(headers, "__Host-sylux")?;
    let token = value.strip_prefix("anon.")?;
    decode_secret(token)
}

pub fn session_cookie(headers: &HeaderMap) -> Option<SessionCookie> {
    let value = cookie_value(headers, "__Host-sylux")?;
    let (session_id, secret) = value.split_once('.')?;
    if session_id.len() != 32
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || secret.contains('.')
    {
        return None;
    }
    Some(SessionCookie {
        session_id: session_id.to_owned(),
        secret: decode_secret(secret)?,
    })
}

pub fn origin_matches(headers: &HeaderMap, expected: &str) -> bool {
    let mut origins = headers.get_all(axum::http::header::ORIGIN).iter();
    let Some(origin) = origins.next() else {
        return false;
    };
    origins.next().is_none() && origin.to_str().ok() == Some(expected)
}

/// Origin gate for safe (GET) bootstrap endpoints such as `GET /auth/csrf` and
/// `GET /setup`. Browsers omit `Origin` on same-origin GET requests, so an
/// absent Origin is treated as same-origin; a present-but-mismatched Origin is
/// still rejected. State-changing handlers must keep using the strict
/// `origin_matches` (their CSRF/setup token is the primary defense).
pub fn origin_absent_or_matches(headers: &HeaderMap, expected: &str) -> bool {
    let mut origins = headers.get_all(axum::http::header::ORIGIN).iter();
    let Some(origin) = origins.next() else {
        return true;
    };
    origins.next().is_none() && origin.to_str().ok() == Some(expected)
}

pub fn secure_cookie(value: &str, max_age: i64) -> String {
    format!("__Host-sylux={value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={max_age}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header::ORIGIN};

    #[test]
    fn duplicate_session_cookies_and_multiple_origins_are_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("__Host-sylux=one; __Host-sylux=two"),
        );
        assert!(cookie_value(&headers, "__Host-sylux").is_none());

        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("https://one.example"));
        headers.append(ORIGIN, HeaderValue::from_static("https://two.example"));
        assert!(!origin_matches(&headers, "https://one.example"));
    }

    #[test]
    fn absent_origin_is_same_origin_but_mismatch_is_rejected() {
        let absent = HeaderMap::new();
        assert!(origin_absent_or_matches(&absent, "https://app.example"));

        let mut matching = HeaderMap::new();
        matching.insert(ORIGIN, HeaderValue::from_static("https://app.example"));
        assert!(origin_absent_or_matches(&matching, "https://app.example"));

        let mut mismatched = HeaderMap::new();
        mismatched.insert(ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert!(!origin_absent_or_matches(
            &mismatched,
            "https://app.example"
        ));
    }

    #[test]
    fn public_origin_is_accepted_and_foreign_origin_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("https://sylux.example"));
        assert!(origin_matches(&headers, "https://sylux.example"));
        headers.insert(ORIGIN, HeaderValue::from_static("https://foreign.lan"));
        assert!(!origin_matches(&headers, "https://sylux.example"));
    }

    #[test]
    fn authenticated_cookie_requires_strict_id_and_secret_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static(
                "__Host-sylux=0123456789abcdef0123456789abcdef.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            ),
        );
        let cookie = session_cookie(&headers).unwrap();
        assert_eq!(cookie.session_id, "0123456789abcdef0123456789abcdef");
        assert_eq!(cookie.secret, [0; 32]);
    }
}
