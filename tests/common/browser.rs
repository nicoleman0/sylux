//! Real-route browser account seed for the test-harness TLS fixture only.
use super::{server::tls_connect, sha256_hex};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io, net::SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const BROWSER_HANDLE: &str = "browser-owner";
pub const BROWSER_PASSWORD: &str = "correct horse battery staple";
pub const BROWSER_FOLDER: &str = "browser-fixtures";
pub const BROWSER_NESTED_FOLDER: &str = "nested";
pub const BROWSER_FILE: &str = "seeded.txt";
const FILE_BODY: &[u8] = b"S1 browser API fixture\n";

#[derive(Debug, Clone, Serialize)]
pub struct SeededBrowserAccount {
    pub handle: String,
    pub folder: String,
    pub nested: String,
    pub file: String,
    pub folder_id: String,
    pub nested_id: String,
    pub file_id: String,
    pub sha256: String,
    pub length: usize,
}
impl SeededBrowserAccount {
    pub fn fixtures_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Create the account and inventory exclusively through the real setup, auth,
/// folder, upload and commit endpoints on the verified loopback TLS origin.
pub async fn seed_browser_account(
    address: SocketAddr,
    ca_der: &[u8],
    origin: &str,
    setup_authorization: &str,
) -> io::Result<SeededBrowserAccount> {
    let client = BrowserApiClient {
        address,
        ca_der,
        host: origin
            .strip_prefix("https://")
            .ok_or_else(|| io::Error::other("fixture origin must use HTTPS"))?
            .to_owned(),
    };

    let response = client
        .request(
            "POST",
            "/setup/admin",
            None,
            &[
                ("X-Setup-Authorization", setup_authorization.to_owned()),
                ("Origin", origin.to_owned()),
                ("Content-Type", "application/json".to_owned()),
            ],
            json_body(&json!({"handle": BROWSER_HANDLE, "password": BROWSER_PASSWORD}))?,
        )
        .await?;
    response.require_status(201, "setup admin")?;

    let csrf = client
        .request("GET", "/auth/csrf", None, &[], Vec::new())
        .await?;
    csrf.require_status(200, "CSRF bootstrap")?;
    let anonymous_cookie = csrf.cookie()?;
    let anonymous_token = string_field(&csrf.json()?, "csrf_token", "anonymous CSRF token")?;
    let login = client
        .request(
            "POST",
            "/auth/local",
            Some(&anonymous_cookie),
            &[
                ("Origin", origin.to_owned()),
                ("X-CSRF-Token", anonymous_token),
                ("Content-Type", "application/json".to_owned()),
            ],
            json_body(&json!({"handle": BROWSER_HANDLE, "password": BROWSER_PASSWORD}))?,
        )
        .await?;
    login.require_status(200, "local login")?;
    let cookie = login.cookie()?;
    let csrf_token = string_field(&login.json()?, "csrf_token", "session CSRF token")?;

    let root_response = client
        .request("GET", "/folders/children", Some(&cookie), &[], Vec::new())
        .await?;
    root_response.require_status(200, "root listing")?;
    let root_id = string_field(&root_response.json()?["root"], "id", "root entry id")?;

    let folder_response = client
        .request(
            "POST",
            &format!("/folders/{root_id}/children"),
            Some(&cookie),
            &mutation_headers(origin, &csrf_token),
            json_body(&json!({"name": BROWSER_FOLDER}))?,
        )
        .await?;
    folder_response.require_status(201, "create browser fixture folder")?;
    let folder = folder_response.json()?;
    let folder_id = string_field(&folder, "id", "fixture folder id")?;

    let nested_response = client
        .request(
            "POST",
            &format!("/folders/{folder_id}/children"),
            Some(&cookie),
            &mutation_headers(origin, &csrf_token),
            json_body(&json!({"name": BROWSER_NESTED_FOLDER}))?,
        )
        .await?;
    nested_response.require_status(201, "create nested fixture folder")?;
    let nested = nested_response.json()?;
    let nested_id = string_field(&nested, "id", "nested folder id")?;

    let parent_etag = string_field(&nested, "etag", "nested folder ETag")?;
    let digest = sha256_hex(FILE_BODY);

    let ticket_response = client
        .request(
            "POST",
            "/mutation-tickets",
            Some(&cookie),
            &mutation_headers(origin, &csrf_token),
            json_body(&json!({
                "scope": "upload_create",
                "parent_id": nested_id,
                "parent_etag": parent_etag,
                "name": BROWSER_FILE,
                "length": FILE_BODY.len(),
                "sha256": digest,
            }))?,
        )
        .await?;
    ticket_response.require_status(201, "create upload ticket")?;
    let ticket = ticket_response
        .header("x-mutation-id")
        .ok_or_else(|| io::Error::other("upload ticket omitted mutation identifier"))?
        .to_owned();

    let mut admission_headers = mutation_headers(origin, &csrf_token);
    admission_headers.push(("X-Mutation-ID", ticket.clone()));
    admission_headers.push(("If-None-Match", "*".to_owned()));
    let admission = client
        .request(
            "POST",
            "/uploads",
            Some(&cookie),
            &admission_headers,
            json_body(&json!({
                "parent_id": nested_id,
                "parent_etag": parent_etag,
                "name": BROWSER_FILE,
                "length": FILE_BODY.len(),
                "sha256": digest,
            }))?,
        )
        .await?;
    admission.require_status(201, "admit upload")?;
    let upload_id = string_field(&admission.json()?, "upload_id", "upload id")?;

    let mut content_headers = mutation_headers(origin, &csrf_token);
    content_headers.push(("X-Mutation-ID", ticket.clone()));
    content_headers.push(("Content-Type", "application/octet-stream".to_owned()));
    let content = client
        .request(
            "PUT",
            &format!("/uploads/{upload_id}/content"),
            Some(&cookie),
            &content_headers,
            FILE_BODY.to_vec(),
        )
        .await?;
    content.require_status(200, "upload content")?;
    if string_field(&content.json()?, "sha256", "uploaded content digest")? != digest {
        return Err(io::Error::other(
            "real upload route returned a different content digest",
        ));
    }

    let mut commit_headers = mutation_headers(origin, &csrf_token);
    commit_headers.push(("X-Mutation-ID", ticket));
    commit_headers.push(("If-None-Match", "*".to_owned()));
    let commit = client
        .request(
            "POST",
            &format!("/uploads/{upload_id}/commit"),
            Some(&cookie),
            &commit_headers,
            json_body(&json!({"sha256": digest, "parent_etag": parent_etag}))?,
        )
        .await?;
    commit.require_status(201, "commit upload")?;
    let file = commit.json()?;
    let file_id = string_field(&file, "id", "committed file id")?;
    if string_field(&file, "sha256", "committed file digest")? != digest
        || file["size"].as_u64() != Some(FILE_BODY.len() as u64)
    {
        return Err(io::Error::other(
            "commit route returned unexpected file metadata",
        ));
    }

    // Prove the acknowledged account is listable through the same real routes
    // before exposing any fixture details to Playwright.
    let root_list = client
        .request("GET", "/folders/children", Some(&cookie), &[], Vec::new())
        .await?;
    root_list.require_status(200, "verify seeded root listing")?;
    if !contains_entry(&root_list.json()?, &folder_id, BROWSER_FOLDER) {
        return Err(io::Error::other(
            "seeded folder absent from real root listing",
        ));
    }
    let nested_list = client
        .request(
            "GET",
            &format!("/folders/{nested_id}/children"),
            Some(&cookie),
            &[],
            Vec::new(),
        )
        .await?;
    nested_list.require_status(200, "verify seeded nested listing")?;
    if !contains_entry(&nested_list.json()?, &file_id, BROWSER_FILE) {
        return Err(io::Error::other(
            "committed file absent from real nested listing",
        ));
    }

    Ok(SeededBrowserAccount {
        handle: BROWSER_HANDLE.to_owned(),
        folder: BROWSER_FOLDER.to_owned(),
        nested: BROWSER_NESTED_FOLDER.to_owned(),
        file: BROWSER_FILE.to_owned(),
        folder_id,
        nested_id,
        file_id,
        sha256: digest,
        length: FILE_BODY.len(),
    })
}

struct BrowserApiClient<'a> {
    address: SocketAddr,
    ca_der: &'a [u8],
    host: String,
}
impl BrowserApiClient<'_> {
    async fn request(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> io::Result<Response> {
        let mut stream = tls_connect(self.address, self.ca_der, "fixture.test").await?;
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n",
            self.host
        );
        if let Some(cookie) = cookie {
            request.push_str(&format!("Cookie: {cookie}\r\n"));
        }
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        stream.write_all(request.as_bytes()).await?;
        stream.write_all(&body).await?;
        let mut response = Vec::new();
        if let Err(error) = stream.read_to_end(&mut response).await
            && error.kind() != io::ErrorKind::UnexpectedEof
        {
            return Err(error);
        }
        Response::parse(&response)
    }
}

struct Response {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
impl Response {
    fn parse(bytes: &[u8]) -> io::Result<Self> {
        let split = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| io::Error::other("invalid HTTP response from fixture route"))?;
        let head = std::str::from_utf8(&bytes[..split])
            .map_err(|_| io::Error::other("invalid HTTP header from fixture route"))?;
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|status| status.parse::<u16>().ok())
            .ok_or_else(|| io::Error::other("invalid HTTP status from fixture route"))?;
        let mut headers = BTreeMap::new();
        for line in lines {
            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| io::Error::other("invalid HTTP header from fixture route"))?;
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
        let body = &bytes[split + 4..];
        let body = if headers
            .get("transfer-encoding")
            .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
        {
            decode_chunked(body)?
        } else if let Some(length) = headers.get("content-length") {
            let length = length
                .parse::<usize>()
                .map_err(|_| io::Error::other("invalid HTTP content length"))?;
            if body.len() < length {
                return Err(io::Error::other(
                    "truncated HTTP response from fixture route",
                ));
            }
            body[..length].to_vec()
        } else {
            body.to_vec()
        };
        Ok(Self {
            status,
            headers,
            body,
        })
    }
    fn require_status(&self, expected: u16, route: &str) -> io::Result<()> {
        if self.status == expected {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "real {route} route returned HTTP {} instead of {expected}",
                self.status
            )))
        }
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
    fn cookie(&self) -> io::Result<String> {
        self.header("set-cookie")
            .and_then(|cookie| cookie.split(';').next())
            .map(str::to_owned)
            .ok_or_else(|| io::Error::other("auth route omitted session cookie"))
    }
    fn json(&self) -> io::Result<Value> {
        serde_json::from_slice(&self.body)
            .map_err(|_| io::Error::other("real route returned invalid JSON"))
    }
}

fn mutation_headers(origin: &str, csrf: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Origin", origin.to_owned()),
        ("X-CSRF-Token", csrf.to_owned()),
        ("Content-Type", "application/json".to_owned()),
    ]
}
fn json_body(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(value)
}
fn string_field(value: &Value, field: &str, description: &str) -> io::Result<String> {
    value[field]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other(format!("real route omitted {description}")))
}
fn contains_entry(listing: &Value, id: &str, name: &str) -> bool {
    listing["items"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["id"] == id && item["name"] == name)
    })
}
fn decode_chunked(mut bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let end = bytes
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| io::Error::other("invalid chunked HTTP response"))?;
        let size = std::str::from_utf8(&bytes[..end])
            .ok()
            .and_then(|size| usize::from_str_radix(size, 16).ok())
            .ok_or_else(|| io::Error::other("invalid chunk size from fixture route"))?;
        bytes = &bytes[end + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        if bytes.len() < size + 2 || &bytes[size..size + 2] != b"\r\n" {
            return Err(io::Error::other("truncated chunked HTTP response"));
        }
        decoded.extend_from_slice(&bytes[..size]);
        bytes = &bytes[size + 2..];
    }
}
