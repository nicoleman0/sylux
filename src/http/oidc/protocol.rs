use std::{collections::HashSet, sync::Arc, time::Duration};

use axum::{
    body::Bytes,
    http::{Request, Uri},
};
use http_body_util::{BodyExt, Full, Limited};
use huskarl::{
    core::{
        EndpointUrl, Error, RetryAdvice,
        client_auth::ClientSecret,
        crypto::{
            KeyMatchStrength,
            verifier::{
                CreateVerifierError, JwsVerifier, JwsVerifierFactory, JwsVerifierPlatform,
                KeyMatch, MultiKeyVerifier, VerifyError,
            },
        },
        http::{HttpClient, HttpResponse, Idempotency},
        jwk::{KeyOperation, KeyUse, PublicJwk, PublicKey},
        platform::MaybeSendBoxFuture,
        secrets::{ProvidedSecret, SecretString},
        server_metadata::AuthorizationServerMetadata,
    },
    grant::authorization_code::{
        AuthorizationCodeGrant, CompleteInput, PendingState, ResponseMode, StartInput, StartOutput,
    },
};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use rustls::{
    ClientConfig, RootCertStore,
    pki_types::{CertificateDer, pem::PemObject},
};
use url::Url;

pub(super) const CALLBACK: &str = "/auth/oidc/callback";
pub(super) const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE: usize = 1024 * 1024;
const MAX_KEYS: usize = 32;

pub struct ProviderConfig {
    pub issuer: String,
    pub client_id: String,
    pub secret: SecretString,
    pub ca_pem: Vec<u8>,
}

pub(super) struct Provider {
    pub issuer: String,
    callback: String,
    client_id: String,
    auth: ClientSecret,
    transport: Transport,
}

#[derive(Clone)]
struct Transport {
    origin: url::Origin,
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
}

#[derive(Clone, Copy, Debug)]
pub enum ProtocolError {
    Configuration,
    Unavailable,
    InvalidProof,
}

fn https(value: &str) -> Result<Url, ProtocolError> {
    let valid_bytes = value
        .bytes()
        .all(|byte| byte.is_ascii_graphic() && byte != b'\\');
    let endpoint = Url::parse(value).map_err(|_| ProtocolError::Configuration)?;
    if !valid_bytes
        || value.len() > 2048
        || !value.starts_with("https://")
        || endpoint.host().is_none()
        || endpoint.username() != ""
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || value
            .split('/')
            .nth(2)
            .is_some_and(|host| host.contains('@'))
    {
        return Err(ProtocolError::Configuration);
    }
    Ok(endpoint)
}

impl Provider {
    pub fn new(config: ProviderConfig, origin: &str) -> Result<Self, ProtocolError> {
        let issuer = https(&config.issuer)?;
        let app = https(origin)?;
        if app.path() != "/"
            || origin.ends_with('/')
            || config.client_id.is_empty()
            || config.client_id.len() > 255
            || config.secret.expose_secret().is_empty()
        {
            return Err(ProtocolError::Configuration);
        }
        Ok(Self {
            issuer: config.issuer,
            callback: format!("{origin}{CALLBACK}"),
            client_id: config.client_id,
            auth: ClientSecret::builder()
                .client_secret(ProvidedSecret::new(config.secret))
                .prefer_basic_auth(true)
                .build(),
            transport: Transport::new(issuer.origin(), &config.ca_pem)?,
        })
    }

    pub fn registration(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        for value in [&self.issuer, &self.client_id, &self.callback] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        hash.finalize().into()
    }

    pub async fn start(
        &self,
        max_age: Option<Duration>,
    ) -> Result<(AuthorizationCodeGrant, StartOutput), ProtocolError> {
        tokio::time::timeout(EXCHANGE_TIMEOUT, self.start_inner(max_age))
            .await
            .map_err(|_| ProtocolError::Unavailable)?
    }

    async fn start_inner(
        &self,
        max_age: Option<Duration>,
    ) -> Result<(AuthorizationCodeGrant, StartOutput), ProtocolError> {
        let metadata = AuthorizationServerMetadata::oidc_fetch()
            .http_client(&self.transport)
            .issuer(self.issuer.clone())
            .call()
            .await
            .map_err(|_| ProtocolError::Unavailable)?;
        let authorization = metadata
            .authorization_endpoint
            .ok_or(ProtocolError::Configuration)?;
        let jwks = metadata.jwks_uri.ok_or(ProtocolError::Configuration)?;
        for endpoint in [&authorization, &metadata.token_endpoint, &jwks] {
            self.transport
                .check(endpoint.as_uri())
                .map_err(|_| ProtocolError::Configuration)?;
        }
        if metadata.issuer != self.issuer
            || metadata.require_pushed_authorization_requests
            || !contains(&metadata.response_types_supported, "code")
            || !contains(&metadata.grant_types_supported, "authorization_code")
            || !contains(&metadata.response_modes_supported, "query")
            || !contains(&metadata.code_challenge_methods_supported, "S256")
            || !contains(
                &metadata.token_endpoint_auth_methods_supported,
                "client_secret_basic",
            )
            || !metadata
                .id_token_signing_alg_values_supported
                .as_ref()
                .is_some_and(|a| contains(a, "RS256"))
        {
            return Err(ProtocolError::Configuration);
        }
        let grant = AuthorizationCodeGrant::builder()
            .issuer(self.issuer.clone())
            .client_id(self.client_id.clone())
            .client_auth(self.auth.clone())
            .http_client(self.transport.clone())
            .authorization_endpoint(authorization)
            .token_endpoint(metadata.token_endpoint)
            .jwks_uri(jwks)
            .token_endpoint_auth_methods_supported(vec!["client_secret_basic".into()])
            .redirect_uri(self.callback.clone())
            .authorization_response_iss_parameter_supported(
                metadata.authorization_response_iss_parameter_supported,
            )
            .oidc(true)
            .response_mode(ResponseMode::Query)
            .allowed_id_token_signed_response_algs(HashSet::from(["RS256".into()]))
            .jws_verifier_platform(Arc::new(Ring))
            .jws_verifier_factory(self.transport.clone())
            .build()
            .await
            .map_err(|_| ProtocolError::Unavailable)?;
        let output = grant
            .start(
                StartInput::builder()
                    .scope(vec!["openid".into()])
                    .maybe_max_age(max_age)
                    .build(),
            )
            .await
            .map_err(|_| ProtocolError::Unavailable)?;
        Ok((grant, output))
    }

    pub async fn complete(
        &self,
        grant: &AuthorizationCodeGrant,
        pending: &PendingState,
        callback: CompleteInput,
        started: i64,
    ) -> Result<VerifiedIdentity, ProtocolError> {
        let result = tokio::time::timeout(EXCHANGE_TIMEOUT, grant.complete(pending, callback))
            .await
            .map_err(|_| ProtocolError::Unavailable)?
            .map_err(|_| ProtocolError::InvalidProof)?;
        let token = result.id_token.ok_or(ProtocolError::InvalidProof)?;
        let invalid = ProtocolError::InvalidProof;
        let now = super::super::auth::now();
        let issued = token.iat.and_then(epoch).ok_or(invalid)?;
        let expires = token.exp.and_then(epoch).ok_or(invalid)?;
        if token.iss.as_deref() != Some(self.issuer.as_str())
            || token.aud.as_slice() != [self.client_id.as_str()]
            || token
                .claims
                .azp
                .as_ref()
                .is_some_and(|azp| azp != &self.client_id)
            || expires <= now
            || expires <= issued
            || issued < started.saturating_sub(60)
            || issued > now.saturating_add(60)
        {
            return Err(invalid);
        }
        let subject = token.sub.filter(|sub| valid_subject(sub)).ok_or(invalid)?;
        Ok(VerifiedIdentity {
            subject,
            auth_time: token
                .claims
                .auth_time
                .and_then(|value| i64::try_from(value).ok()),
            acr: token.claims.acr,
            amr: token.claims.amr,
        })
    }
}

pub(super) struct VerifiedIdentity {
    pub subject: String,
    pub auth_time: Option<i64>,
    pub acr: Option<String>,
    pub amr: Vec<String>,
}

pub(super) fn valid_subject(subject: &str) -> bool {
    !subject.is_empty() && subject.len() <= 255 && subject.bytes().all(|b| b.is_ascii_graphic())
}

fn epoch(time: std::time::SystemTime) -> Option<i64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs()
        .try_into()
        .ok()
}

fn contains(list: &[String], value: &str) -> bool {
    list.iter().any(|item| item == value)
}

impl Transport {
    fn new(origin: url::Origin, ca: &[u8]) -> Result<Self, ProtocolError> {
        let mut roots = RootCertStore::empty();
        for certificate in CertificateDer::pem_slice_iter(ca) {
            roots
                .add(certificate.map_err(|_| ProtocolError::Configuration)?)
                .map_err(|_| ProtocolError::Configuration)?;
        }
        if roots.is_empty() {
            return Err(ProtocolError::Configuration);
        }
        let tls =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|_| ProtocolError::Configuration)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_only()
            .enable_http1()
            .build();
        let client = Client::builder(TokioExecutor::new())
            .retry_canceled_requests(false)
            .pool_max_idle_per_host(0)
            .build(connector);
        Ok(Self { origin, client })
    }

    fn check(&self, uri: &Uri) -> Result<(), Error> {
        let endpoint = https(&uri.to_string()).map_err(|_| transport_error())?;
        if endpoint.origin() != self.origin {
            return Err(transport_error());
        }
        Ok(())
    }
}

fn transport_error() -> Error {
    Error::new(RetryAdvice::No, "OIDC transport failed")
}

impl HttpClient for Transport {
    fn execute(
        &self,
        request: Request<Bytes>,
        _: Idempotency,
    ) -> MaybeSendBoxFuture<'_, Result<HttpResponse, Error>> {
        Box::pin(async move {
            self.check(request.uri())?;
            let response = self
                .client
                .request(request.map(Full::new))
                .await
                .map_err(|_| transport_error())?;
            let (parts, body) = response.into_parts();
            if parts.status.is_redirection() {
                return Err(transport_error());
            }
            let body = Limited::new(body, MAX_RESPONSE)
                .collect()
                .await
                .map_err(|_| transport_error())?
                .to_bytes();
            Ok(HttpResponse {
                status: parts.status,
                headers: parts.headers,
                body,
            })
        })
    }
}

// A fresh keyset is fetched for each attempt. A new attempt sees signing key rotation.
#[derive(serde::Deserialize)]
struct KeySet {
    keys: Vec<PublicJwk>,
}

impl JwsVerifierFactory for Transport {
    fn build(
        &self,
        uri: Option<&EndpointUrl>,
        platform: Arc<dyn JwsVerifierPlatform>,
    ) -> MaybeSendBoxFuture<'static, Result<Arc<dyn JwsVerifier>, Error>> {
        let transport = self.clone();
        let uri = uri.cloned();
        Box::pin(async move {
            let uri = uri.ok_or_else(transport_error)?;
            let request = Request::get(uri.as_uri().clone())
                .header("accept", "application/jwk-set+json, application/json")
                .body(Bytes::new())
                .map_err(|_| transport_error())?;
            let response = transport.execute(request, Idempotency::Idempotent).await?;
            if response.status != axum::http::StatusCode::OK {
                return Err(transport_error());
            }
            let keys: KeySet =
                serde_json::from_slice(&response.body).map_err(|_| transport_error())?;
            if keys.keys.len() > MAX_KEYS {
                return Err(transport_error());
            }
            let mut used = HashSet::new();
            let mut verifiers = Vec::new();
            for key in keys.keys {
                let Some(kid) = key.kid.clone() else { continue };
                if let Ok(verifier) = platform.create_verifier_from_jwk(key).await {
                    if !used.insert(kid) {
                        return Err(transport_error());
                    }
                    verifiers.push(verifier);
                }
            }
            if verifiers.is_empty() {
                return Err(transport_error());
            }
            Ok(Arc::new(MultiKeyVerifier::new(verifiers)) as Arc<dyn JwsVerifier>)
        })
    }
}

#[derive(Debug)]
struct Ring;

impl JwsVerifierPlatform for Ring {
    fn create_verifier_from_jwk(
        &self,
        jwk: PublicJwk,
    ) -> MaybeSendBoxFuture<'static, Result<Arc<dyn JwsVerifier>, CreateVerifierError>> {
        Box::pin(async move {
            let unsupported = || CreateVerifierError::UnsupportedKey {
                source: transport_error(),
            };
            let kid = jwk
                .kid
                .filter(|id| !id.is_empty() && id.len() <= 255)
                .ok_or_else(unsupported)?;
            let PublicKey::Rsa(key) = jwk.key else {
                return Err(unsupported());
            };
            if jwk.algorithm.as_deref().is_some_and(|alg| alg != "RS256")
                || jwk.key_use.is_some_and(|usage| usage != KeyUse::Sign)
                || jwk
                    .key_operations
                    .is_some_and(|ops| ops.as_slice() != [KeyOperation::Verify])
                || jwk.has_private_parameters
                || jwk.x5u.is_some()
                || !(256..=1024).contains(&key.n.len())
                || key.e.is_empty()
                || key.e.len() > 5
            {
                return Err(unsupported());
            }
            Ok(Arc::new(Rs256 {
                kid,
                components: RsaPublicKeyComponents { n: key.n, e: key.e },
            }) as Arc<dyn JwsVerifier>)
        })
    }

    fn supported_signature_algorithms(&self) -> &[&str] {
        &["RS256"]
    }
}

#[derive(Debug)]
struct Rs256 {
    kid: String,
    components: RsaPublicKeyComponents<Vec<u8>>,
}

impl JwsVerifier for Rs256 {
    fn key_match(&self, key: &KeyMatch<'_>) -> Option<KeyMatchStrength> {
        key.kid.filter(|kid| *kid == self.kid)?;
        key.strength_for(&["RS256"], Some(&self.kid))
    }

    fn verify<'a>(
        &'a self,
        input: &'a [u8],
        signature: &'a [u8],
        key: &'a KeyMatch<'a>,
    ) -> MaybeSendBoxFuture<'a, Result<(), VerifyError>> {
        Box::pin(async move {
            self.key_match(key).ok_or(VerifyError::NoMatchingKey)?;
            self.components
                .verify(&RSA_PKCS1_2048_8192_SHA256, input, signature)
                .map_err(|_| VerifyError::SignatureMismatch)
        })
    }
}
