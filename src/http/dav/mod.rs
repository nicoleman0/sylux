mod namespace;
mod path;
mod properties;

use self::{
    namespace::{Resource, child, children, namespace_error, resolve, writable_parent},
    path::DavPath,
    properties::Query,
};
use super::{AuthState, content, middleware::origin_absent_or_matches, uploads};
use crate::domain::{
    dav_credentials::{DavGrant, DavProof},
    library_publication,
    names::EntryName,
    namespace as storage_namespace,
    upload::{MAX_UPLOAD_LENGTH, UploadAdmission},
};
use axum::{
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{
        HeaderMap, HeaderValue, Method, StatusCode,
        header::{
            ALLOW, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG,
            IF_MATCH, IF_NONE_MATCH, WWW_AUTHENTICATE, X_CONTENT_TYPE_OPTIONS,
        },
    },
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::Connection;
use std::time::Duration;

const UNAVAILABLE: StatusCode = StatusCode::SERVICE_UNAVAILABLE;
const UNAUTHORIZED: StatusCode = StatusCode::UNAUTHORIZED;
const READ: &str = "OPTIONS, PROPFIND, GET, HEAD";
const READ_WRITE: &str = "OPTIONS, PROPFIND, GET, HEAD, PUT, MKCOL, MOVE, DELETE";
const XML_LIMIT: usize = 16 * 1024;

pub(super) async fn request(State(state): State<AuthState>, request: Request) -> Response {
    let mut response = match dispatch(&state, request).await {
        Ok(response) => response,
        Err(status) => status.into_response(),
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if response.status() == UNAUTHORIZED {
        response.headers_mut().insert(
            WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"Sylux WebDAV\", charset=\"UTF-8\""),
        );
    }
    response
}

async fn dispatch(state: &AuthState, request: Request) -> Result<Response, StatusCode> {
    if !state.webdav {
        return Err(StatusCode::NOT_FOUND);
    }
    let (parts, body) = request.into_parts();
    let headers = &parts.headers;
    if !origin_absent_or_matches(headers, &state.origin) {
        return Err(StatusCode::FORBIDDEN);
    }
    let proof = parse_basic(headers).ok_or(UNAUTHORIZED)?;
    let path = DavPath::parse(&parts.uri)?;
    let method = parts.method.as_str();
    let is_write = matches!(method, "PUT" | "MKCOL" | "MOVE" | "DELETE");
    let grant = state
        .workers
        .read({
            let proof = proof.clone();
            move |db| proof.authenticate(db)
        })
        .await
        .map_err(|_| UNAVAILABLE)?
        .ok_or(UNAUTHORIZED)?;
    if is_write && grant.read_only {
        return Err(StatusCode::FORBIDDEN);
    }
    if headers.contains_key("if") {
        return Err(StatusCode::PRECONDITION_FAILED);
    }
    if is_write
        && (headers.contains_key("if-unmodified-since")
            || headers.contains_key("if-modified-since"))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    match method {
        "OPTIONS" => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            let headers = response.headers_mut();
            headers.insert(ALLOW, allowed(&grant));
            headers.insert("dav", HeaderValue::from_static("sylux"));
            Ok(response)
        }
        "PROPFIND" => propfind(state, proof, path, headers, body).await,
        "GET" | "HEAD" => download(state, proof, path, grant, parts.method, parts.headers).await,
        "PUT" => put(state, proof, path, headers, body).await,
        "MKCOL" | "DELETE" | "MOVE" => {
            let write = NamespaceWrite::parse(method, headers, &state.origin)?;
            if !read_body(body, 1).await?.is_empty() {
                return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
            }
            let conditions = Conditions::parse(headers)?;
            let result = state
                .coordinator
                .transact_dav(grant, UNAVAILABLE, UNAUTHORIZED, move |tx, owner| {
                    write.apply(tx, owner, &path, &conditions)
                })
                .await?;
            Ok(result.into_response())
        }
        _ => Ok(method_not_allowed(&grant)),
    }
}

fn allowed(grant: &DavGrant) -> HeaderValue {
    HeaderValue::from_static(if grant.read_only { READ } else { READ_WRITE })
}

fn method_not_allowed(grant: &DavGrant) -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(ALLOW, allowed(grant))]).into_response()
}

fn parse_basic(headers: &HeaderMap) -> Option<DavProof> {
    let raw = header(headers, AUTHORIZATION.as_str()).ok()??;
    let (scheme, value) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") || value.len() > 240 {
        return None;
    }
    let decoded = STANDARD.decode(value).ok()?;
    let (handle, password) = std::str::from_utf8(&decoded).ok()?.split_once(':')?;
    DavProof::parse(handle, password)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, StatusCode> {
    let mut values = headers.get_all(name).iter();
    let value = values.next();
    if values.next().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    value
        .map(|value| value.to_str().map_err(|_| StatusCode::BAD_REQUEST))
        .transpose()
}

async fn read_body(body: Body, limit: usize) -> Result<Bytes, StatusCode> {
    tokio::time::timeout(Duration::from_secs(30), to_bytes(body, limit))
        .await
        .map_err(|_| StatusCode::REQUEST_TIMEOUT)?
        .map_err(|_| StatusCode::BAD_REQUEST)
}

async fn authorized_snapshot<T: Send + 'static>(
    state: &AuthState,
    proof: DavProof,
    operation: impl FnOnce(&Connection, &DavGrant) -> Result<T, StatusCode> + Send + 'static,
) -> Result<T, StatusCode> {
    state
        .workers
        .read(move |db| {
            let tx = db.unchecked_transaction()?;
            let Some(grant) = proof.authenticate(&tx)? else {
                return Ok(Err(UNAUTHORIZED));
            };
            let result = operation(&tx, &grant);
            tx.commit()?;
            Ok(result)
        })
        .await
        .map_err(|_| UNAVAILABLE)?
}

fn located(db: &Connection, owner: &str, path: &DavPath) -> Result<Resource, StatusCode> {
    let resource = resolve(db, owner, &path.names)?;
    if path.trailing_slash && !resource.folder() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(resource)
}

async fn propfind(
    state: &AuthState,
    proof: DavProof,
    path: DavPath,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    let depth = match header(headers, "depth")? {
        Some("0") => false,
        Some("1") => true,
        Some("infinity") | None => {
            return Ok((
                StatusCode::FORBIDDEN,
                [(CONTENT_TYPE, "application/xml; charset=utf-8")],
                "<D:error xmlns:D=\"DAV:\"><D:propfind-finite-depth/></D:error>",
            )
                .into_response());
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let query = Query::parse(&read_body(body, XML_LIMIT).await?)?;
    let resources = authorized_snapshot(state, proof, move |db, grant| {
        let resource = located(db, &grant.account_id, &path)?;
        let children = if depth && resource.folder() {
            children(db, &grant.account_id, &resource)?
        } else {
            Vec::new()
        };
        Ok(std::iter::once(resource)
            .chain(children)
            .collect::<Vec<_>>())
    })
    .await?;
    Ok((
        StatusCode::MULTI_STATUS,
        [(CONTENT_TYPE, "application/xml; charset=utf-8")],
        query.multistatus(&resources)?,
    )
        .into_response())
}

async fn download(
    state: &AuthState,
    proof: DavProof,
    path: DavPath,
    grant: DavGrant,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let resource = authorized_snapshot(state, proof, move |db, grant| {
        located(db, &grant.account_id, &path)
    })
    .await?;
    let head = method == Method::HEAD;
    let conditions = Conditions::parse(&headers)?;
    let tag = resource.entry.as_ref().map(|entry| entry.http_etag());
    if !conditions.matches(tag.as_deref(), true) {
        return Err(StatusCode::PRECONDITION_FAILED);
    }
    if !conditions.none_match(tag.as_deref(), true) {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Some(tag) = tag {
            response
                .headers_mut()
                .insert(ETAG, HeaderValue::from_str(&tag).map_err(|_| UNAVAILABLE)?);
        }
        return Ok(response);
    }
    match resource.entry.and_then(|entry| entry.content) {
        Some(reference) => Ok(content::serve_reference(state, reference, headers, head).await),
        None if head => {
            let mut response = (StatusCode::OK, [(CONTENT_LENGTH, "0")]).into_response();
            if let Some(tag) = tag {
                response
                    .headers_mut()
                    .insert(ETAG, HeaderValue::from_str(&tag).map_err(|_| UNAVAILABLE)?);
            }
            Ok(response)
        }
        None => Ok(method_not_allowed(&grant)),
    }
}

#[derive(Default)]
struct Conditions {
    if_match: Option<String>,
    if_none_match: Option<String>,
}

impl Conditions {
    fn parse(headers: &HeaderMap) -> Result<Self, StatusCode> {
        fn condition(headers: &HeaderMap, name: &str) -> Result<Option<String>, StatusCode> {
            let Some(value) = header(headers, name)? else {
                return Ok(None);
            };
            if value != "*"
                && !value.split(',').all(|item| {
                    let item = item.trim();
                    item.strip_prefix("W/")
                        .unwrap_or(item)
                        .strip_prefix('"')
                        .and_then(|item| item.strip_suffix('"'))
                        .is_some_and(|tag| {
                            tag.bytes()
                                .all(|byte| byte == 0x21 || (0x23..=0x7e).contains(&byte))
                        })
                })
            {
                return Err(StatusCode::BAD_REQUEST);
            }
            Ok(Some(value.to_owned()))
        }
        Ok(Self {
            if_match: condition(headers, IF_MATCH.as_str())?,
            if_none_match: condition(headers, IF_NONE_MATCH.as_str())?,
        })
    }

    fn matches(&self, tag: Option<&str>, exists: bool) -> bool {
        self.if_match.as_deref().is_none_or(|value| {
            if value == "*" {
                exists
            } else {
                value.split(',').any(|item| Some(item.trim()) == tag)
            }
        })
    }

    fn none_match(&self, tag: Option<&str>, exists: bool) -> bool {
        self.if_none_match.as_deref().is_none_or(|value| {
            if value == "*" {
                !exists
            } else {
                !value.split(',').any(|item| {
                    let item = item.trim();
                    Some(item.strip_prefix("W/").unwrap_or(item)) == tag
                })
            }
        })
    }

    fn check(&self, tag: Option<&str>, exists: bool) -> Result<(), StatusCode> {
        if !self.matches(tag, exists) || !self.none_match(tag, exists) {
            return Err(StatusCode::PRECONDITION_FAILED);
        }
        Ok(())
    }
}

enum NamespaceWrite {
    CreateCollection,
    Trash,
    Move { destination: DavPath },
}

impl NamespaceWrite {
    fn parse(method: &str, headers: &HeaderMap, origin: &str) -> Result<Self, StatusCode> {
        if header(headers, "depth")?.is_some_and(|depth| depth != "infinity") {
            return Err(StatusCode::BAD_REQUEST);
        }
        if method == "MOVE" {
            if header(headers, "overwrite")?.is_some_and(|value| !matches!(value, "T" | "F")) {
                return Err(StatusCode::BAD_REQUEST);
            }
            let destination = header(headers, "destination")?.ok_or(StatusCode::BAD_REQUEST)?;
            return Ok(Self::Move {
                destination: DavPath::destination(destination, origin)?,
            });
        }
        Ok(if method == "MKCOL" {
            Self::CreateCollection
        } else {
            Self::Trash
        })
    }

    fn apply(
        self,
        db: &rusqlite::Transaction<'_>,
        owner: &str,
        path: &DavPath,
        conditions: &Conditions,
    ) -> Result<StatusCode, StatusCode> {
        if matches!(self, Self::CreateCollection) {
            let (parent, name) = writable_parent(db, owner, &path.names)?;
            let existing = child(db, owner, Some(&parent.id), name)?;
            conditions.check(
                existing.as_ref().map(|entry| entry.http_etag()).as_deref(),
                existing.is_some(),
            )?;
            if existing.is_some() {
                return Err(StatusCode::METHOD_NOT_ALLOWED);
            }
            library_publication::require_unpublished_parent(db, owner, &parent.id)
                .map_err(namespace_error)?;
            storage_namespace::create_folder(db, owner, &parent.id, &name.display)
                .map_err(namespace_error)?;
            return Ok(StatusCode::CREATED);
        }

        let entry = located(db, owner, path)?
            .entry
            .ok_or(StatusCode::FORBIDDEN)?;
        let source_parent = entry.parent_id.as_deref().ok_or(StatusCode::FORBIDDEN)?;
        conditions.check(Some(&entry.http_etag()), true)?;
        let Self::Move { destination } = self else {
            storage_namespace::trash(db, owner, &entry.id, &entry.etag).map_err(namespace_error)?;
            return Ok(StatusCode::NO_CONTENT);
        };
        if destination.trailing_slash && !entry.folder {
            return Err(StatusCode::CONFLICT);
        }
        let (parent, name) = writable_parent(db, owner, &destination.names)?;
        if let Some(existing) = child(db, owner, Some(&parent.id), name)? {
            if existing.id != entry.id {
                return Err(StatusCode::PRECONDITION_FAILED);
            }
            if existing.name == name.display {
                return Ok(StatusCode::NO_CONTENT);
            }
        }
        if parent.id == source_parent {
            storage_namespace::rename(db, owner, &entry.id, &name.display, &entry.etag)
                .map_err(namespace_error)?;
        } else {
            storage_namespace::move_entry_as(
                db,
                owner,
                &entry.id,
                &parent.id,
                &entry.etag,
                Some(name),
            )
            .map_err(namespace_error)?;
        }
        Ok(StatusCode::CREATED)
    }
}

async fn put(
    state: &AuthState,
    proof: DavProof,
    path: DavPath,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    if path.trailing_slash
        || headers.contains_key(CONTENT_RANGE)
        || headers.contains_key("content-encoding")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let declared = header(headers, CONTENT_LENGTH.as_str())?.ok_or(StatusCode::LENGTH_REQUIRED)?;
    if declared.is_empty() || !declared.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let length = declared
        .parse::<u64>()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if length > MAX_UPLOAD_LENGTH {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let conditions = Conditions::parse(headers)?;
    let (grant, admission, parent_path) = authorized_snapshot(state, proof, move |db, grant| {
        if grant.read_only {
            return Err(StatusCode::FORBIDDEN);
        }
        let (parent, name) = writable_parent(db, &grant.account_id, &path.names)?;
        let existing = child(db, &grant.account_id, Some(&parent.id), name)?;
        conditions.check(
            existing.as_ref().map(|entry| entry.http_etag()).as_deref(),
            existing.is_some(),
        )?;
        if existing.is_some() {
            return Err(StatusCode::PRECONDITION_FAILED);
        }
        library_publication::require_unpublished_parent(db, &grant.account_id, &parent.id)
            .map_err(namespace_error)?;
        let admission = UploadAdmission {
            parent_id: parent.id,
            parent_etag: parent.etag,
            name: name.display.clone(),
            length,
            sha256: None,
            replacement: None,
        };
        Ok((
            grant.clone(),
            admission,
            path.names[..path.names.len() - 1].to_vec(),
        ))
    })
    .await?;
    Ok(uploads::dav_put(state.clone(), grant, parent_path, admission, body).await)
}

pub(super) fn check_upload_path(
    db: &Connection,
    account: &str,
    path: &[EntryName],
    id: &str,
) -> Result<(), StatusCode> {
    match resolve(db, account, path) {
        Ok(resource) if resource.entry.as_ref().is_some_and(|entry| entry.id == id) => {
            library_publication::require_unpublished_parent(db, account, id)
                .map_err(namespace_error)
        }
        Err(status) if status != StatusCode::NOT_FOUND => Err(status),
        _ => Err(StatusCode::PRECONDITION_FAILED),
    }
}
