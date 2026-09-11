use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use httpdate::fmt_http_date;
use reqwest::header::{ACCEPT, CONTENT_ENCODING, CONTENT_TYPE, DATE, HOST, LOCATION};
use reqwest::redirect::Policy;
use serde_json::Value;
use tokio::net::lookup_host;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::{Host, Url};

use crate::mastodon::{
    HttpSignatureRequest, HttpSignatureSigner, body_digest_header, random_auth_token,
    sign_http_signature,
};

const DEFAULT_MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_REQUEST_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_REDIRECTS: usize = 3;
const MAX_CONFIGURED_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONFIGURED_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONFIGURED_REDIRECTS: usize = 3;
const MAX_CONFIGURED_TIMEOUT: Duration = Duration::from_mins(1);
const DEFAULT_REMOTE_DOMAIN_IN_FLIGHT: usize = 2;
const REMOTE_DOMAIN_LEASE_TTL: Duration = Duration::from_secs(75);
const MAX_REMOTE_DOMAIN_BUCKETS: usize = 4096;
const MAX_REMOTE_USERNAME_CHARS: usize = 2048;
const MAX_REMOTE_DISPLAY_NAME_CHARS: usize = 2048;
const MAX_REMOTE_NOTE_CHARS: usize = 20 * 1024;
const MAX_REMOTE_PUBLIC_KEYS: usize = 10;
const MAX_REMOTE_KEY_ID_CHARS: usize = 4096;
const MAX_REMOTE_PUBLIC_KEY_CHARS: usize = 64 * 1024;
const MAX_RESOLVED_ADDRESSES: usize = 32;
const SECURITY_CONTEXT: &str = "https://w3id.org/security/v1";
const WEBFINGER_CONTENT_TYPES: &[&str] = &["application/jrd+json", "application/json"];
const HOST_META_CONTENT_TYPES: &[&str] = &["application/xrd+xml", "application/xml", "text/xml"];
const ACTIVITYPUB_CONTENT_TYPES: &[&str] = &[
    "application/activity+json",
    "application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\"",
];
const SUPPORTED_ACTOR_TYPES: &[&str] =
    &["Application", "Group", "Organization", "Person", "Service"];

#[derive(Clone, Copy, Debug)]
pub struct RemoteFetchLimits {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub max_redirects: usize,
}

impl Default for RemoteFetchLimits {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            max_redirects: DEFAULT_MAX_REDIRECTS,
        }
    }
}

#[derive(Debug)]
pub enum RemoteFetchError {
    InvalidUrl,
    NoAddresses,
    BlockedAddress(IpAddr),
    Dns,
    Client,
    Request,
    Redirect,
    TooManyRedirects,
    UnexpectedStatus(StatusCode),
    MissingContentType,
    UnsupportedContentType,
    UnsupportedEncoding,
    BodyTooLarge,
    BodyRead,
    InvalidRepresentation,
    IdentityMismatch,
    OriginMismatch,
    PolicyDenied,
    Signing,
    DomainBudgetExceeded,
}

impl fmt::Display for RemoteFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidUrl => "remote URL is invalid",
            Self::NoAddresses => "remote host has no addresses",
            Self::BlockedAddress(_) => "remote host resolves to a blocked address",
            Self::Dns => "remote host lookup failed",
            Self::Client => "remote HTTP client could not be built",
            Self::Request => "remote HTTP request failed",
            Self::Redirect => "remote redirect is invalid",
            Self::TooManyRedirects => "remote redirect limit exceeded",
            Self::UnexpectedStatus(_) => "remote HTTP response was unsuccessful",
            Self::MissingContentType => "remote response has no content type",
            Self::UnsupportedContentType => "remote response content type is unsupported",
            Self::UnsupportedEncoding => "remote response content encoding is unsupported",
            Self::BodyTooLarge => "remote response body is too large",
            Self::BodyRead => "remote response body could not be read",
            Self::InvalidRepresentation => "remote response is not valid JSON",
            Self::IdentityMismatch => "remote response identity does not match the requested ID",
            Self::OriginMismatch => "remote response origin does not match the requested account",
            Self::PolicyDenied => "remote URL is denied by policy",
            Self::Signing => "remote request could not be signed",
            Self::DomainBudgetExceeded => "remote host request budget is exhausted",
        })
    }
}

impl std::error::Error for RemoteFetchError {}

#[derive(Debug)]
pub struct RemoteResponse {
    pub url: Url,
    pub status: StatusCode,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

enum RemoteFetchHop {
    Redirect(Url),
    Response(RemoteResponse),
}

#[derive(Clone, Debug)]
pub struct RemoteFetcher {
    #[cfg(feature = "test-support")]
    test_endpoint: Option<SocketAddr>,
    limits: RemoteFetchLimits,
    domain_budget: RemoteDomainBudget,
    #[cfg(all(debug_assertions, feature = "test-support"))]
    test_peer: Result<Option<Arc<TestPeerTransport>>, ()>,
}

// This capability is absent unless BOTH build gates are enabled. Configuration is
// snapshotted at construction; invalid/partial opt-ins poison the fetcher rather
// than falling back to public DNS. Synthetic endpoint fixtures remain separate.
#[cfg(all(debug_assertions, feature = "test-support"))]
#[derive(Debug)]
struct TestPeerTransport {
    origins: HashMap<String, SocketAddr>,
    ca: reqwest::Certificate,
}

#[cfg(all(debug_assertions, feature = "test-support"))]
impl TestPeerTransport {
    fn from_env() -> Result<Option<Arc<Self>>, ()> {
        use rustls::pki_types::{CertificateDer, pem::PemObject};

        let origins = std::env::var_os("RUSTODON_TEST_PEER_ORIGINS");
        let ca = std::env::var_os("RUSTODON_TEST_PEER_CA");
        match (origins, ca) {
            (None, None) => Ok(None),
            (Some(origins), Some(ca)) => {
                let origins = Self::parse_origins(origins.to_str().ok_or(())?)?;
                let pem = std::fs::read_to_string(ca).map_err(|_| ())?;
                // Accept exactly one explicit PEM certificate, not an empty bundle,
                // unrelated PEM objects, or a valid certificate followed by junk.
                let body = pem
                    .trim()
                    .strip_prefix("-----BEGIN CERTIFICATE-----")
                    .and_then(|pem| pem.strip_suffix("-----END CERTIFICATE-----"))
                    .ok_or(())?;
                if body.contains("-----") {
                    return Err(());
                }
                let der = CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|_| ())?;
                rustls::RootCertStore::empty()
                    .add(der.clone())
                    .map_err(|_| ())?;
                let ca = reqwest::Certificate::from_der(der.as_ref()).map_err(|_| ())?;
                Ok(Some(Arc::new(Self { origins, ca })))
            }
            _ => Err(()),
        }
    }

    fn parse_origins(json: &str) -> Result<HashMap<String, SocketAddr>, ()> {
        // Reject duplicate JSON keys rather than silently taking the last endpoint.
        struct Origins;
        impl<'de> serde::de::Visitor<'de> for Origins {
            type Value = HashMap<String, SocketAddr>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a nonempty map of canonical HTTPS .invalid origins to loopback sockets",
                )
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut origins = HashMap::new();
                while let Some((origin, endpoint)) = map.next_entry::<String, SocketAddr>()? {
                    let valid = Url::parse(&origin).is_ok_and(|url| {
                        validate_remote_url(&url).is_ok()
                            && url.scheme() == "https"
                            && url.origin().ascii_serialization() == origin
                            && url.host_str().is_some_and(|host| {
                                host.len() <= 253
                                    && host.ends_with(".invalid")
                                    && host.split('.').all(|label| {
                                        !label.is_empty()
                                            && label.len() <= 63
                                            && !label.starts_with('-')
                                            && !label.ends_with('-')
                                            && label.bytes().all(|byte| {
                                                byte.is_ascii_lowercase()
                                                    || byte.is_ascii_digit()
                                                    || byte == b'-'
                                            })
                                    })
                            })
                    });
                    if !valid
                        || !endpoint.ip().is_loopback()
                        || endpoint.port() == 0
                        || origins.insert(origin, endpoint).is_some()
                    {
                        return Err(serde::de::Error::custom(
                            "invalid or duplicate test peer origin/endpoint",
                        ));
                    }
                }
                if origins.is_empty() {
                    return Err(serde::de::Error::custom("empty test peer map"));
                }
                Ok(origins)
            }
        }
        use serde::Deserializer;
        let mut deserializer = serde_json::Deserializer::from_str(json);
        let origins = deserializer.deserialize_map(Origins).map_err(|_| ())?;
        deserializer.end().map_err(|_| ())?;
        Ok(origins)
    }

    fn endpoint(&self, url: &Url) -> Result<SocketAddr, RemoteFetchError> {
        self.origins
            .get(&url.origin().ascii_serialization())
            .copied()
            .ok_or(RemoteFetchError::InvalidUrl)
    }
}

/// Per-host cap for concurrent work against each canonical remote host.
#[derive(Clone, Debug)]
pub struct RemoteDomainBudget {
    max_in_flight: usize,
    semaphores: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    operational_pool: Option<sqlx::PgPool>,
}

struct RemoteDomainPermit {
    _local: Option<OwnedSemaphorePermit>,
    shared: Option<RemoteDomainLease>,
}

struct RemoteDomainLease {
    pool: sqlx::PgPool,
    host: String,
    lease_id: String,
}

impl RemoteDomainBudget {
    /// Creates a budget with the supplied per-host concurrency cap.
    #[must_use]
    pub fn new(max_in_flight: usize) -> Self {
        Self {
            max_in_flight: max_in_flight.max(1),
            semaphores: Arc::new(Mutex::new(HashMap::new())),
            operational_pool: None,
        }
    }

    /// Creates a budget whose leases coordinate through the Rustodon operational database.
    #[must_use]
    pub fn with_operational_pool(max_in_flight: usize, pool: sqlx::PgPool) -> Self {
        Self {
            max_in_flight: max_in_flight.max(1),
            semaphores: Arc::new(Mutex::new(HashMap::new())),
            operational_pool: Some(pool),
        }
    }

    fn with_pool(&self, pool: sqlx::PgPool) -> Self {
        Self {
            max_in_flight: self.max_in_flight,
            semaphores: Arc::clone(&self.semaphores),
            operational_pool: Some(pool),
        }
    }

    async fn acquire(&self, url: &Url) -> Result<RemoteDomainPermit, RemoteFetchError> {
        let host = canonical_remote_host_from_origin(url)?.to_ascii_lowercase();
        if host.len() > 255 {
            return Err(RemoteFetchError::InvalidUrl);
        }
        if let Some(pool) = &self.operational_pool {
            return self.acquire_shared(pool.clone(), host).await;
        }
        self.acquire_local(host)
    }

    fn acquire_local(&self, host: String) -> Result<RemoteDomainPermit, RemoteFetchError> {
        let mut semaphores = self
            .semaphores
            .lock()
            .map_err(|_| RemoteFetchError::DomainBudgetExceeded)?;
        let semaphore = if let Some(semaphore) = semaphores.get(&host) {
            Arc::clone(semaphore)
        } else {
            if semaphores.len() >= MAX_REMOTE_DOMAIN_BUCKETS {
                let idle_host = semaphores
                    .iter()
                    .find(|(_, semaphore)| semaphore.available_permits() == self.max_in_flight)
                    .map(|(host, _)| host.clone());
                let Some(idle_host) = idle_host else {
                    return Err(RemoteFetchError::DomainBudgetExceeded);
                };
                semaphores.remove(&idle_host);
            }
            let semaphore = Arc::new(Semaphore::new(self.max_in_flight));
            semaphores.insert(host, Arc::clone(&semaphore));
            semaphore
        };
        semaphore
            .try_acquire_owned()
            .map(|local| RemoteDomainPermit {
                _local: Some(local),
                shared: None,
            })
            .map_err(|_| RemoteFetchError::DomainBudgetExceeded)
    }

    async fn acquire_shared(
        &self,
        pool: sqlx::PgPool,
        host: String,
    ) -> Result<RemoteDomainPermit, RemoteFetchError> {
        let max_in_flight = i64::try_from(self.max_in_flight)
            .map_err(|_| RemoteFetchError::DomainBudgetExceeded)?;
        let lease_id = random_auth_token(24);
        let lease_ttl = i64::try_from(REMOTE_DOMAIN_LEASE_TTL.as_secs())
            .map_err(|_| RemoteFetchError::DomainBudgetExceeded)?;
        let mut transaction = pool
            .begin()
            .await
            .map_err(|_| RemoteFetchError::DomainBudgetExceeded)?;
        for query in [
            "SELECT pg_catalog.pg_advisory_xact_lock(\
               pg_catalog.hashtext('rustodon:remote_fetch:' || $1))",
            "DELETE FROM rustodon.remote_fetch_leases \
             WHERE host = $1 AND expires_at <= clock_timestamp()",
        ] {
            if sqlx::query(query)
                .bind(&host)
                .execute(&mut *transaction)
                .await
                .is_err()
            {
                return Err(RemoteFetchError::DomainBudgetExceeded);
            }
        }
        let active = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.remote_fetch_leases \
             WHERE host = $1 AND expires_at > clock_timestamp()",
        )
        .bind(&host)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RemoteFetchError::DomainBudgetExceeded)?;
        if active >= max_in_flight {
            transaction
                .commit()
                .await
                .map_err(|_| RemoteFetchError::DomainBudgetExceeded)?;
            return Err(RemoteFetchError::DomainBudgetExceeded);
        }
        sqlx::query(
            "INSERT INTO rustodon.remote_fetch_leases (host, lease_id, expires_at) \
             VALUES ($1, $2, clock_timestamp() + \
                     ($3::double precision * INTERVAL '1 second'))",
        )
        .bind(&host)
        .bind(&lease_id)
        .bind(lease_ttl)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RemoteFetchError::DomainBudgetExceeded)?;
        transaction
            .commit()
            .await
            .map_err(|_| RemoteFetchError::DomainBudgetExceeded)?;
        Ok(RemoteDomainPermit {
            _local: None,
            shared: Some(RemoteDomainLease {
                pool,
                host,
                lease_id,
            }),
        })
    }
}

impl RemoteDomainPermit {
    async fn release(mut self) {
        if let Some(lease) = self.shared.as_ref()
            && release_remote_domain_lease(lease).await.is_ok()
        {
            self.shared.take();
        }
    }
}

impl Drop for RemoteDomainPermit {
    fn drop(&mut self) {
        let Some(lease) = self.shared.take() else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            let _ = release_remote_domain_lease(&lease).await;
        });
    }
}

async fn release_remote_domain_lease(lease: &RemoteDomainLease) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM rustodon.remote_fetch_leases WHERE host = $1 AND lease_id = $2")
        .bind(&lease.host)
        .bind(&lease.lease_id)
        .execute(&lease.pool)
        .await
        .map(|_| ())
}

impl Default for RemoteDomainBudget {
    fn default() -> Self {
        Self::new(DEFAULT_REMOTE_DOMAIN_IN_FLIGHT)
    }
}

impl RemoteFetcher {
    #[must_use]
    pub fn new(limits: RemoteFetchLimits) -> Self {
        Self::with_domain_budget(limits, RemoteDomainBudget::default())
    }

    /// Creates a fetcher that shares a supplied per-host concurrency budget.
    #[must_use]
    pub fn with_domain_budget(
        limits: RemoteFetchLimits,
        domain_budget: RemoteDomainBudget,
    ) -> Self {
        Self {
            #[cfg(feature = "test-support")]
            test_endpoint: None,
            limits: limits.bounded(),
            domain_budget,
            #[cfg(all(debug_assertions, feature = "test-support"))]
            test_peer: TestPeerTransport::from_env(),
        }
    }

    /// Routes resolver GETs through a fixture endpoint, never in release builds.
    #[cfg(feature = "test-support")]
    pub(crate) fn with_test_endpoint(mut self, endpoint: Option<SocketAddr>) -> Self {
        self.test_endpoint = endpoint;
        self
    }

    /// Creates a fetcher with different transport limits while retaining this fetcher's budget.
    #[must_use]
    pub fn with_limits(&self, limits: RemoteFetchLimits) -> Self {
        Self {
            limits: limits.bounded(),
            ..self.clone()
        }
    }

    /// Creates a fetcher that coordinates its host budget through the operational database.
    #[must_use]
    pub fn with_operational_pool(&self, pool: sqlx::PgPool) -> Self {
        Self {
            domain_budget: self.domain_budget.with_pool(pool),
            ..self.clone()
        }
    }

    #[must_use]
    pub const fn limits(&self) -> RemoteFetchLimits {
        self.limits
    }

    /// Fetches one bounded remote representation without following unvalidated redirects.
    ///
    /// # Errors
    ///
    /// Returns a stable error when URL validation, DNS validation, HTTP transport, content-type,
    /// status, redirect, timeout, or body-size checks fail.
    pub async fn get(
        &self,
        url: Url,
        accepted_content_types: &[&str],
    ) -> Result<RemoteResponse, RemoteFetchError> {
        self.get_bound_to_origin_with_signer(url, accepted_content_types, None, None)
            .await
    }

    /// Check policy before every hop, retaining SSRF, timeout, byte and redirect
    /// bounds. Return every visited URL for installation-time policy fencing.
    pub(crate) async fn get_with_policy<F, Fut>(
        &self,
        url: Url,
        policy: F,
    ) -> Result<(RemoteResponse, Vec<Url>), RemoteFetchError>
    where
        F: Fn(Url) -> Fut,
        Fut: Future<Output = Result<(), RemoteFetchError>>,
    {
        let endpoint = None;
        #[cfg(feature = "test-support")]
        let endpoint = {
            if self.test_endpoint.is_some() && !cfg!(debug_assertions) {
                return Err(RemoteFetchError::Client);
            }
            self.test_endpoint.or(endpoint)
        };
        let addresses = endpoint.map(|_| {
            vec![
                vec![SocketAddr::new(Ipv4Addr::new(8, 8, 8, 8).into(), 443)];
                self.limits.max_redirects + 1
            ]
        });
        let visited = std::sync::Mutex::new(Vec::new());
        let response = tokio::time::timeout(
            self.limits.request_timeout,
            self.get_with_redirects(url, &[], None, None, endpoint, addresses, |url| {
                let policy = &policy;
                let visited = &visited;
                async move {
                    policy(url.clone()).await?;
                    visited
                        .lock()
                        .map_err(|_| RemoteFetchError::Client)?
                        .push(url);
                    Ok(())
                }
            }),
        )
        .await
        .map_err(|_| RemoteFetchError::Request)??;
        Ok((
            response,
            visited.into_inner().map_err(|_| RemoteFetchError::Client)?,
        ))
    }

    /// Fetches a response through an explicit test endpoint without changing production DNS
    /// policy. This exists only for deterministic integration fixtures.
    ///
    /// # Errors
    ///
    /// Returns the same URL, origin, content-type, status, encoding, timeout, and body-size
    /// errors as the production fetch path.
    #[cfg(feature = "test-support")]
    pub async fn get_for_test_endpoint(
        &self,
        url: Url,
        accepted_content_types: &[&str],
        endpoint: SocketAddr,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        self.get_for_test_endpoint_with_resolved_addresses(
            url,
            accepted_content_types,
            endpoint,
            vec![SocketAddr::new(Ipv4Addr::new(8, 8, 8, 8).into(), 443)],
        )
        .await
    }

    /// Fetches through a local test endpoint while applying an explicit DNS result to the normal
    /// address policy. This keeps deterministic fixtures off the network without bypassing the
    /// production mixed-answer and private-address checks.
    ///
    /// # Errors
    ///
    /// Returns the same URL, origin, DNS, timeout, content-type, status, and body-size errors as
    /// the production fetch path.
    #[cfg(feature = "test-support")]
    pub async fn get_for_test_endpoint_with_resolved_addresses(
        &self,
        url: Url,
        accepted_content_types: &[&str],
        endpoint: SocketAddr,
        resolved_addresses: Vec<SocketAddr>,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        if !cfg!(debug_assertions) {
            return Err(RemoteFetchError::Client);
        }
        let expected_origin = remote_url_origin(&url)?;
        tokio::time::timeout(
            self.limits.request_timeout,
            self.get_with_redirects(
                url,
                accepted_content_types,
                Some(&expected_origin),
                None,
                Some(endpoint),
                Some(vec![resolved_addresses]),
                |_| std::future::ready(Ok(())),
            ),
        )
        .await
        .map_err(|_| RemoteFetchError::Request)?
    }

    /// Fetches through a local test endpoint with one explicit DNS result per redirect hop.
    ///
    /// # Errors
    ///
    /// Returns the same URL, origin, DNS, timeout, content-type, status, and body-size errors as
    /// the production fetch path.
    #[cfg(feature = "test-support")]
    pub async fn get_for_test_endpoint_with_resolved_address_sets(
        &self,
        url: Url,
        accepted_content_types: &[&str],
        endpoint: SocketAddr,
        resolved_address_sets: Vec<Vec<SocketAddr>>,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        if !cfg!(debug_assertions) {
            return Err(RemoteFetchError::Client);
        }
        let expected_origin = remote_url_origin(&url)?;
        tokio::time::timeout(
            self.limits.request_timeout,
            self.get_with_redirects(
                url,
                accepted_content_types,
                Some(&expected_origin),
                None,
                Some(endpoint),
                Some(resolved_address_sets),
                |_| std::future::ready(Ok(())),
            ),
        )
        .await
        .map_err(|_| RemoteFetchError::Request)?
    }

    /// Fetches one bounded remote representation with a legacy Mastodon GET signature.
    ///
    /// The signature is rebuilt for every validated redirect destination. Callers remain
    /// responsible for choosing a local account whose private key is allowed to represent the
    /// fetch.
    ///
    /// # Errors
    ///
    /// Returns a stable error when URL validation, DNS validation, signing, HTTP transport,
    /// content-type, status, redirect, timeout, or body-size checks fail.
    pub async fn get_signed(
        &self,
        url: Url,
        accepted_content_types: &[&str],
        signer: &HttpSignatureSigner<'_>,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        let expected_origin = remote_url_origin(&url)?;
        self.get_bound_to_origin_with_signer(
            url,
            accepted_content_types,
            Some(expected_origin),
            Some(signer),
        )
        .await
    }

    /// Sends one bounded `ActivityPub` JSON POST with a legacy Mastodon signature.
    ///
    /// Only same-origin `307` and `308` redirects are followed, and the request is re-signed for
    /// every validated destination.
    ///
    /// # Errors
    ///
    /// Returns a stable error when URL validation, request-size, DNS validation, signing, HTTP
    /// transport, redirect, timeout, response, or body-size checks fail.
    pub async fn post_signed_json(
        &self,
        url: Url,
        body: &[u8],
        signer: &HttpSignatureSigner<'_>,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        if body.len() > self.limits.max_request_bytes {
            return Err(RemoteFetchError::BodyTooLarge);
        }
        let expected_origin = remote_url_origin(&url)?;
        tokio::time::timeout(
            self.limits.request_timeout,
            self.post_with_redirects(url, body, &expected_origin, signer, None, None),
        )
        .await
        .map_err(|_| RemoteFetchError::Request)?
    }

    /// Sends a bounded signed `ActivityPub` JSON POST through an explicit test endpoint.
    ///
    /// This is available only to debug test-support builds so deterministic local fixtures can
    /// exercise the production signing and response path without weakening production DNS policy.
    ///
    /// # Errors
    ///
    /// Returns the same URL, signing, timeout, redirect, response, and body-size errors as the
    /// production POST path.
    #[cfg(feature = "test-support")]
    pub async fn post_signed_json_for_test_endpoint(
        &self,
        url: Url,
        body: &[u8],
        signer: &HttpSignatureSigner<'_>,
        endpoint: SocketAddr,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        self.post_signed_json_for_test_endpoint_with_resolved_addresses(
            url,
            body,
            signer,
            endpoint,
            vec![SocketAddr::new(Ipv4Addr::new(8, 8, 8, 8).into(), 443)],
        )
        .await
    }

    /// Sends a signed JSON POST through a local test endpoint while applying an explicit DNS
    /// result to the normal address policy.
    ///
    /// This exists only for deterministic transport fixtures and is available only to debug
    /// test-support builds.
    ///
    /// # Errors
    ///
    /// Returns the same URL, signing, DNS, timeout, redirect, response, and body-size errors as
    /// the production POST path.
    #[cfg(feature = "test-support")]
    pub async fn post_signed_json_for_test_endpoint_with_resolved_addresses(
        &self,
        url: Url,
        body: &[u8],
        signer: &HttpSignatureSigner<'_>,
        endpoint: SocketAddr,
        resolved_addresses: Vec<SocketAddr>,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        self.post_signed_json_for_test_endpoint_with_resolved_address_sets(
            url,
            body,
            signer,
            endpoint,
            vec![resolved_addresses],
        )
        .await
    }

    /// Sends a signed JSON POST through a local test endpoint with one explicit DNS result per
    /// redirect hop.
    ///
    /// This exists only for deterministic transport fixtures and is available only to debug
    /// test-support builds.
    ///
    /// # Errors
    ///
    /// Returns the same URL, signing, DNS, timeout, redirect, response, and body-size errors as
    /// the production POST path.
    #[cfg(feature = "test-support")]
    pub async fn post_signed_json_for_test_endpoint_with_resolved_address_sets(
        &self,
        url: Url,
        body: &[u8],
        signer: &HttpSignatureSigner<'_>,
        endpoint: SocketAddr,
        resolved_address_sets: Vec<Vec<SocketAddr>>,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        if !cfg!(debug_assertions) {
            return Err(RemoteFetchError::Client);
        }
        if body.len() > self.limits.max_request_bytes {
            return Err(RemoteFetchError::BodyTooLarge);
        }
        let expected_origin = remote_url_origin(&url)?;
        tokio::time::timeout(
            self.limits.request_timeout,
            self.post_with_redirects(
                url,
                body,
                &expected_origin,
                signer,
                Some(endpoint),
                Some(resolved_address_sets),
            ),
        )
        .await
        .map_err(|_| RemoteFetchError::Request)?
    }

    async fn get_bound_to_origin(
        &self,
        url: Url,
        accepted_content_types: &[&str],
        expected_origin: Option<Url>,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        self.get_bound_to_origin_with_signer(url, accepted_content_types, expected_origin, None)
            .await
    }

    async fn get_bound_to_origin_with_signer(
        &self,
        url: Url,
        accepted_content_types: &[&str],
        expected_origin: Option<Url>,
        signer: Option<&HttpSignatureSigner<'_>>,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        let endpoint = None;
        #[cfg(feature = "test-support")]
        let endpoint = {
            if self.test_endpoint.is_some() && !cfg!(debug_assertions) {
                return Err(RemoteFetchError::Client);
            }
            self.test_endpoint.or(endpoint)
        };
        let policy_addresses = endpoint.map(|_| {
            vec![
                vec![SocketAddr::new(Ipv4Addr::new(8, 8, 8, 8).into(), 443)];
                self.limits.max_redirects + 1
            ]
        });
        tokio::time::timeout(
            self.limits.request_timeout,
            self.get_with_redirects(
                url,
                accepted_content_types,
                expected_origin.as_ref(),
                signer,
                endpoint,
                policy_addresses,
                |_| std::future::ready(Ok(())),
            ),
        )
        .await
        .map_err(|_| RemoteFetchError::Request)?
    }

    /// Validates a remote URL and every address currently returned by DNS without opening it.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the URL is invalid, DNS fails, or any resolved address is not
    /// publicly routable.
    pub async fn validate_target(&self, url: &Url) -> Result<(), RemoteFetchError> {
        validate_remote_url(url)?;
        #[cfg(all(debug_assertions, feature = "test-support"))]
        if let Some(peer) = self.test_peer()? {
            peer.endpoint(url)?;
        }
        #[cfg(feature = "test-support")]
        if self.test_endpoint.is_some() {
            if !cfg!(debug_assertions) {
                return Err(RemoteFetchError::Client);
            }
            return validate_resolved_addresses(&[SocketAddr::new(
                Ipv4Addr::new(8, 8, 8, 8).into(),
                443,
            )]);
        }
        self.with_domain_permit(url, || async {
            #[cfg(all(debug_assertions, feature = "test-support"))]
            if self.test_peer()?.is_some() {
                return Ok(());
            }
            let addresses =
                tokio::time::timeout(self.limits.request_timeout, resolve_remote_addresses(url))
                    .await
                    .map_err(|_| RemoteFetchError::Request)??;
            validate_resolved_addresses(&addresses)
        })
        .await
    }

    #[cfg(all(debug_assertions, feature = "test-support"))]
    fn test_peer(&self) -> Result<Option<&TestPeerTransport>, RemoteFetchError> {
        self.test_peer
            .as_ref()
            .map(|peer| peer.as_deref())
            .map_err(|()| RemoteFetchError::Client)
    }

    async fn transport_client(
        &self,
        url: &Url,
        endpoint_override: Option<SocketAddr>,
        policy_addresses: Option<&[SocketAddr]>,
    ) -> Result<(reqwest::Client, Url), RemoteFetchError> {
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .connect_timeout(self.limits.connect_timeout)
            .timeout(self.limits.request_timeout)
            .user_agent("Rustodon/0.1");
        let host = url.host_str().ok_or(RemoteFetchError::InvalidUrl)?;
        #[cfg(all(debug_assertions, feature = "test-support"))]
        if let Some(peer) = self.test_peer()? {
            // Never combine peer routing with the synthetic DNS-policy fixtures.
            if endpoint_override.is_some() || policy_addresses.is_some() {
                return Err(RemoteFetchError::Client);
            }
            let endpoint = peer.endpoint(url)?;
            let mut transport_url = url.clone();
            // Reqwest gives explicit URL ports precedence over resolver overrides.
            // Only the connection port changes: SNI/verification use the original
            // hostname, and Host/signatures/redirects/response URLs use `url`.
            transport_url
                .set_port(Some(endpoint.port()))
                .map_err(|()| RemoteFetchError::InvalidUrl)?;
            let client = builder
                .resolve_to_addrs(host, &[endpoint])
                .tls_built_in_root_certs(false)
                .add_root_certificate(peer.ca.clone())
                .build()
                .map_err(|_| RemoteFetchError::Client)?;
            return Ok((client, transport_url));
        }
        let addresses = if let Some(endpoint) = endpoint_override {
            vec![endpoint]
        } else {
            resolve_remote_addresses(url).await?
        };
        validate_resolved_addresses(policy_addresses.unwrap_or(&addresses))?;
        let client = builder
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|_| RemoteFetchError::Client)?;
        Ok((client, url.clone()))
    }

    async fn with_domain_permit<T, F, Fut>(
        &self,
        url: &Url,
        operation: F,
    ) -> Result<T, RemoteFetchError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, RemoteFetchError>>,
    {
        let permit = self.domain_budget.acquire(url).await?;
        let result = operation().await;
        permit.release().await;
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn get_with_redirects<F, Fut>(
        &self,
        mut url: Url,
        accepted_content_types: &[&str],
        expected_origin: Option<&Url>,
        signer: Option<&HttpSignatureSigner<'_>>,
        endpoint_override: Option<SocketAddr>,
        policy_address_sets: Option<Vec<Vec<SocketAddr>>>,
        policy: F,
    ) -> Result<RemoteResponse, RemoteFetchError>
    where
        F: Fn(Url) -> Fut,
        Fut: Future<Output = Result<(), RemoteFetchError>>,
    {
        for redirect_count in 0..=self.limits.max_redirects {
            validate_remote_url(&url)?;
            policy(url.clone()).await?;
            if expected_origin.is_some_and(|expected| !same_origin_url(&url, expected)) {
                return Err(RemoteFetchError::OriginMismatch);
            }
            #[cfg(all(debug_assertions, feature = "test-support"))]
            if let Some(peer) = self.test_peer()? {
                peer.endpoint(&url)?;
            }
            let hop_url = url.clone();
            let hop = self
                .with_domain_permit(&url, || async {
                    let policy_addresses = policy_address_sets
                        .as_ref()
                        .and_then(|sets| sets.get(redirect_count))
                        .map(Vec::as_slice);
                    let (client, transport_url) = self
                        .transport_client(&hop_url, endpoint_override, policy_addresses)
                        .await?;
                    let mut request = client
                        .get(transport_url)
                        .header(HOST, remote_request_host(&hop_url)?);
                    if !accepted_content_types.is_empty() {
                        request = request.header(ACCEPT, accepted_content_types.join(", "));
                    }
                    if let Some(signer) = signer {
                        let headers = signed_get_headers(&hop_url, signer)?;
                        request = request.headers(headers);
                    }
                    let response = request
                        .send()
                        .await
                        .map_err(|_| RemoteFetchError::Request)?;
                    if response.status().is_redirection() {
                        if redirect_count == self.limits.max_redirects {
                            return Err(RemoteFetchError::TooManyRedirects);
                        }
                        let location = response
                            .headers()
                            .get(LOCATION)
                            .and_then(|value| value.to_str().ok())
                            .ok_or(RemoteFetchError::Redirect)?;
                        return Ok(RemoteFetchHop::Redirect(
                            hop_url
                                .join(location)
                                .map_err(|_| RemoteFetchError::Redirect)?,
                        ));
                    }
                    Ok(RemoteFetchHop::Response(
                        read_remote_response(
                            response,
                            hop_url,
                            &self.limits,
                            accepted_content_types,
                            false,
                            true,
                        )
                        .await?,
                    ))
                })
                .await?;
            match hop {
                RemoteFetchHop::Redirect(next_url) => url = next_url,
                RemoteFetchHop::Response(response) => return Ok(response),
            }
        }
        Err(RemoteFetchError::TooManyRedirects)
    }

    async fn post_with_redirects(
        &self,
        mut url: Url,
        body: &[u8],
        expected_origin: &Url,
        signer: &HttpSignatureSigner<'_>,
        endpoint_override: Option<SocketAddr>,
        policy_address_sets: Option<Vec<Vec<SocketAddr>>>,
    ) -> Result<RemoteResponse, RemoteFetchError> {
        for redirect_count in 0..=self.limits.max_redirects {
            validate_remote_url(&url)?;
            if !same_origin_url(&url, expected_origin) {
                return Err(RemoteFetchError::OriginMismatch);
            }
            #[cfg(all(debug_assertions, feature = "test-support"))]
            if let Some(peer) = self.test_peer()? {
                peer.endpoint(&url)?;
            }
            let hop_url = url.clone();
            let hop = self
                .with_domain_permit(&url, || async {
                    let policy_addresses = policy_address_sets
                        .as_ref()
                        .and_then(|sets| sets.get(redirect_count))
                        .map(Vec::as_slice);
                    let (client, transport_url) = self
                        .transport_client(&hop_url, endpoint_override, policy_addresses)
                        .await?;
                    let headers = signed_post_headers(&hop_url, body, signer)?;
                    let response = client
                        .post(transport_url)
                        .headers(headers)
                        .header(CONTENT_TYPE, "application/activity+json")
                        .body(body.to_owned())
                        .send()
                        .await
                        .map_err(|_| RemoteFetchError::Request)?;
                    if response.status().is_redirection() {
                        if redirect_count == self.limits.max_redirects {
                            return Err(RemoteFetchError::TooManyRedirects);
                        }
                        if !matches!(
                            response.status(),
                            StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT
                        ) {
                            return Err(RemoteFetchError::Redirect);
                        }
                        let location = response
                            .headers()
                            .get(LOCATION)
                            .and_then(|value| value.to_str().ok())
                            .ok_or(RemoteFetchError::Redirect)?;
                        return Ok(RemoteFetchHop::Redirect(
                            hop_url
                                .join(location)
                                .map_err(|_| RemoteFetchError::Redirect)?,
                        ));
                    }
                    Ok(RemoteFetchHop::Response(
                        read_remote_response(response, hop_url, &self.limits, &[], true, false)
                            .await?,
                    ))
                })
                .await?;
            match hop {
                RemoteFetchHop::Redirect(next_url) => url = next_url,
                RemoteFetchHop::Response(response) => return Ok(response),
            }
        }
        Err(RemoteFetchError::TooManyRedirects)
    }
}

fn signed_get_headers(
    url: &Url,
    signer: &HttpSignatureSigner<'_>,
) -> Result<HeaderMap, RemoteFetchError> {
    let host = remote_request_host(url)?;
    let date = fmt_http_date(std::time::SystemTime::now());
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_str(&host).map_err(|_| RemoteFetchError::Signing)?,
    );
    headers.insert(
        DATE,
        HeaderValue::from_str(&date).map_err(|_| RemoteFetchError::Signing)?,
    );
    let path_and_query = remote_path_and_query(url);
    let signature = sign_http_signature(
        &HttpSignatureRequest::new(&Method::GET, &path_and_query, &headers, &[]),
        signer,
    )
    .map_err(|_| RemoteFetchError::Signing)?;
    headers.insert(
        HeaderName::from_static("signature"),
        HeaderValue::from_str(&signature).map_err(|_| RemoteFetchError::Signing)?,
    );
    Ok(headers)
}

fn signed_post_headers(
    url: &Url,
    body: &[u8],
    signer: &HttpSignatureSigner<'_>,
) -> Result<HeaderMap, RemoteFetchError> {
    let host = remote_request_host(url)?;
    let date = fmt_http_date(std::time::SystemTime::now());
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_str(&host).map_err(|_| RemoteFetchError::Signing)?,
    );
    headers.insert(
        DATE,
        HeaderValue::from_str(&date).map_err(|_| RemoteFetchError::Signing)?,
    );
    headers.insert(
        HeaderName::from_static("digest"),
        HeaderValue::from_str(&body_digest_header(body)).map_err(|_| RemoteFetchError::Signing)?,
    );
    let path_and_query = remote_path_and_query(url);
    let signature = sign_http_signature(
        &HttpSignatureRequest::new(&Method::POST, &path_and_query, &headers, body),
        signer,
    )
    .map_err(|_| RemoteFetchError::Signing)?;
    headers.insert(
        HeaderName::from_static("signature"),
        HeaderValue::from_str(&signature).map_err(|_| RemoteFetchError::Signing)?,
    );
    Ok(headers)
}

async fn read_remote_response(
    response: reqwest::Response,
    url: Url,
    limits: &RemoteFetchLimits,
    accepted_content_types: &[&str],
    allow_success_status: bool,
    require_content_type: bool,
) -> Result<RemoteResponse, RemoteFetchError> {
    let status = response.status();
    if (allow_success_status && !status.is_success())
        || (!allow_success_status && status != StatusCode::OK)
    {
        return Err(RemoteFetchError::UnexpectedStatus(status));
    }
    if response
        .headers()
        .get(CONTENT_ENCODING)
        .is_some_and(|value| {
            value
                .to_str()
                .map_or(true, |encoding| !encoding.eq_ignore_ascii_case("identity"))
        })
    {
        return Err(RemoteFetchError::UnsupportedEncoding);
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if require_content_type
        && !content_type_allowed(content_type.as_deref(), accepted_content_types)
    {
        return Err(if content_type.is_some() {
            RemoteFetchError::UnsupportedContentType
        } else {
            RemoteFetchError::MissingContentType
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > limits.max_response_bytes as u64)
    {
        return Err(RemoteFetchError::BodyTooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| RemoteFetchError::BodyRead)?;
        if body.len().saturating_add(chunk.len()) > limits.max_response_bytes {
            return Err(RemoteFetchError::BodyTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(RemoteResponse {
        url,
        status,
        content_type,
        body,
    })
}

fn remote_path_and_query(url: &Url) -> String {
    let mut path = url.path().to_owned();
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    path
}

fn remote_request_host(url: &Url) -> Result<String, RemoteFetchError> {
    let host = match url.host().ok_or(RemoteFetchError::InvalidUrl)? {
        Host::Domain(host) => host.to_owned(),
        Host::Ipv4(host) => host.to_string(),
        Host::Ipv6(host) => format!("[{host}]"),
    };
    let default_port = match url.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    Ok(url
        .port()
        .filter(|port| Some(*port) != default_port)
        .map_or(host.clone(), |port| format!("{host}:{port}")))
}

fn remote_url_origin(url: &Url) -> Result<Url, RemoteFetchError> {
    validate_remote_url(url)?;
    let mut origin = url.clone();
    origin.set_path("/");
    origin.set_query(None);
    origin.set_fragment(None);
    Ok(origin)
}

impl Default for RemoteFetcher {
    fn default() -> Self {
        Self::new(RemoteFetchLimits::default())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemotePublicKey {
    pub id: Url,
    pub owner: Url,
    pub pem: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteActor {
    pub id: Url,
    pub username: String,
    pub actor_type: String,
    pub display_name: String,
    pub note: String,
    pub suspended: bool,
    pub profile_url: Option<Url>,
    /// Absent field, explicit removal, or replacement URL, respectively.
    pub avatar: Option<Option<String>>,
    pub header: Option<Option<String>>,
    pub inbox: Url,
    pub shared_inbox: Option<Url>,
    pub followers: Option<Url>,
    pub following: Option<Url>,
    pub public_keys: Vec<RemotePublicKey>,
    pub key_set_complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteKeyResolution {
    pub actor: RemoteActor,
    pub key: RemotePublicKey,
    pub domain: String,
}

#[derive(Clone, Debug)]
pub struct RemoteAccountResolver {
    fetcher: RemoteFetcher,
}

impl RemoteAccountResolver {
    #[must_use]
    pub const fn new(fetcher: RemoteFetcher) -> Self {
        Self { fetcher }
    }

    #[must_use]
    pub fn fetcher(&self) -> RemoteFetcher {
        self.fetcher.clone()
    }

    /// Resolves a remote account through `WebFinger` and a canonical `ActivityPub` actor document.
    ///
    /// # Errors
    ///
    /// Returns a stable error when either representation is malformed, the account is redirected,
    /// the actor identity does not match, or the bounded remote transport rejects a response.
    pub async fn resolve(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<RemoteActor, RemoteFetchError> {
        self.resolve_with_signer(username, domain, None).await
    }

    /// Resolves a canonical `ActivityPub` actor URI without relying on a guessed local handle.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the actor document, `WebFinger` identity, endpoint, key, or
    /// bounded remote transport is invalid.
    pub async fn resolve_actor_uri(
        &self,
        actor_url: &Url,
    ) -> Result<RemoteActor, RemoteFetchError> {
        self.resolve_actor_uri_with_signer(actor_url, None).await
    }

    /// Resolves a canonical `ActivityPub` actor URI with an optional local signed GET.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::resolve_actor_uri`], including signing failures when
    /// a signer is supplied.
    pub async fn resolve_actor_uri_with_signer(
        &self,
        actor_url: &Url,
        signer: Option<&HttpSignatureSigner<'_>>,
    ) -> Result<RemoteActor, RemoteFetchError> {
        tokio::time::timeout(
            self.fetcher.limits().request_timeout,
            self.resolve_actor_uri_inner(actor_url, signer),
        )
        .await
        .map_err(|_| RemoteFetchError::Request)?
    }

    /// Resolves a remote account and signs the actor GET with a local Mastodon keypair.
    ///
    /// `WebFinger` and host-meta discovery remain unsigned, matching Mastodon's discovery flow;
    /// the canonical actor request is signed when a signer is supplied.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::resolve`], including a signing failure when the
    /// supplied keypair cannot create a valid request signature.
    pub async fn resolve_with_signer(
        &self,
        username: &str,
        domain: &str,
        signer: Option<&HttpSignatureSigner<'_>>,
    ) -> Result<RemoteActor, RemoteFetchError> {
        tokio::time::timeout(
            self.fetcher.limits().request_timeout,
            self.resolve_with_signer_inner(username, domain, signer),
        )
        .await
        .map_err(|_| RemoteFetchError::Request)?
    }

    /// Resolves a canonical remote key ID or legacy `acct:` key alias.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the key document, owner actor, identity, origin, or bounded
    /// transport checks fail.
    pub async fn resolve_key(
        &self,
        key_id: &str,
        signer: Option<&HttpSignatureSigner<'_>>,
    ) -> Result<RemoteKeyResolution, RemoteFetchError> {
        tokio::time::timeout(
            self.fetcher.limits().request_timeout,
            self.resolve_key_inner(key_id, signer),
        )
        .await
        .map_err(|_| RemoteFetchError::Request)?
    }

    async fn resolve_key_inner(
        &self,
        key_id: &str,
        signer: Option<&HttpSignatureSigner<'_>>,
    ) -> Result<RemoteKeyResolution, RemoteFetchError> {
        if let Some(account) = key_id.strip_prefix("acct:") {
            let (username, domain) =
                parse_account_subject(account).ok_or(RemoteFetchError::InvalidRepresentation)?;
            let domain = canonical_remote_domain(domain)?;
            if !valid_remote_username(username) {
                return Err(RemoteFetchError::InvalidRepresentation);
            }
            let actor = self
                .resolve_with_signer_inner(username, &domain, signer)
                .await?;
            let key = actor
                .public_keys
                .first()
                .cloned()
                .ok_or(RemoteFetchError::InvalidRepresentation)?;
            return Ok(RemoteKeyResolution { actor, key, domain });
        }

        let key_id = Url::parse(key_id).map_err(|_| RemoteFetchError::InvalidRepresentation)?;
        self.resolve_key_url(&key_id, signer).await
    }

    async fn resolve_key_url(
        &self,
        key_id: &Url,
        signer: Option<&HttpSignatureSigner<'_>>,
    ) -> Result<RemoteKeyResolution, RemoteFetchError> {
        let document_url = remote_key_document_url(key_id)?;
        let expected_origin = remote_url_origin(&document_url)?;
        let response = self
            .fetcher
            .get_bound_to_origin_with_signer(
                document_url,
                ACTIVITYPUB_CONTENT_TYPES,
                Some(expected_origin.clone()),
                signer,
            )
            .await?;
        if !same_origin_url(&response.url, &expected_origin) {
            return Err(RemoteFetchError::OriginMismatch);
        }
        let document = serde_json::from_slice::<Value>(&response.body)
            .map_err(|_| RemoteFetchError::InvalidRepresentation)?;
        if document.get("publicKeyPem").is_some() {
            if !supported_key_context(document.get("@context")) {
                return Err(RemoteFetchError::InvalidRepresentation);
            }
            let key = parse_remote_public_key_object_unbound(&document, Some(key_id))?;
            let (actor, references) = self
                .fetch_remote_key_owner(&key.owner, key_id, signer)
                .await?;
            if !actor_contains_key(&actor, &references, key_id) {
                return Err(RemoteFetchError::IdentityMismatch);
            }
            let key = actor
                .public_keys
                .iter()
                .find(|candidate| candidate.id == *key_id)
                .cloned()
                .unwrap_or(key);
            return Ok(RemoteKeyResolution {
                domain: remote_actor_domain(&actor.id)?,
                actor,
                key,
            });
        }

        let actor_url = document
            .get("id")
            .and_then(|value| value.as_str())
            .and_then(|value| Url::parse(value).ok())
            .ok_or(RemoteFetchError::InvalidRepresentation)?;
        if !same_origin_url(&response.url, &actor_url) {
            return Err(RemoteFetchError::OriginMismatch);
        }
        let (username, domain) = remote_actor_handle(&document, &actor_url)?;
        self.verify_remote_actor_webfinger(&username, &domain, &actor_url)
            .await?;
        let (actor, references) =
            parse_remote_actor_document(&response.body, &actor_url, &username, &domain)?;
        let key = actor
            .public_keys
            .iter()
            .find(|candidate| candidate.id == *key_id)
            .cloned()
            .ok_or_else(|| {
                if references.iter().any(|reference| reference == key_id) {
                    RemoteFetchError::InvalidRepresentation
                } else {
                    RemoteFetchError::IdentityMismatch
                }
            })?;
        Ok(RemoteKeyResolution { actor, key, domain })
    }

    async fn fetch_remote_key_owner(
        &self,
        owner_url: &Url,
        key_id: &Url,
        signer: Option<&HttpSignatureSigner<'_>>,
    ) -> Result<(RemoteActor, Vec<Url>), RemoteFetchError> {
        let expected_origin = remote_url_origin(owner_url)?;
        let response = self
            .fetcher
            .get_bound_to_origin_with_signer(
                owner_url.clone(),
                ACTIVITYPUB_CONTENT_TYPES,
                Some(expected_origin.clone()),
                signer,
            )
            .await?;
        if !same_origin_url(&response.url, &expected_origin) {
            return Err(RemoteFetchError::OriginMismatch);
        }
        let document = serde_json::from_slice::<Value>(&response.body)
            .map_err(|_| RemoteFetchError::InvalidRepresentation)?;
        let actor_url = document
            .get("id")
            .and_then(|value| value.as_str())
            .and_then(|value| Url::parse(value).ok())
            .ok_or(RemoteFetchError::InvalidRepresentation)?;
        if actor_url != *owner_url {
            return Err(RemoteFetchError::IdentityMismatch);
        }
        let (username, domain) = remote_actor_handle(&document, &actor_url)?;
        self.verify_remote_actor_webfinger(&username, &domain, owner_url)
            .await?;
        let parsed = parse_remote_actor_document(&response.body, &actor_url, &username, &domain)?;
        if !actor_contains_key(&parsed.0, &parsed.1, key_id) {
            return Err(RemoteFetchError::IdentityMismatch);
        }
        Ok(parsed)
    }

    async fn verify_remote_actor_webfinger(
        &self,
        username: &str,
        domain: &str,
        actor_url: &Url,
    ) -> Result<(), RemoteFetchError> {
        let expected_origin = remote_url_origin(actor_url)?;
        let domain = canonical_domain_for_url(&expected_origin, domain)?;
        let response = self
            .fetcher
            .get_bound_to_origin(
                webfinger_url_at_origin(username, &domain, &expected_origin)?,
                WEBFINGER_CONTENT_TYPES,
                Some(expected_origin.clone()),
            )
            .await?;
        let document = parse_webfinger_document(&response.body, username, &expected_origin)?;
        if document.self_link == *actor_url {
            Ok(())
        } else {
            Err(RemoteFetchError::IdentityMismatch)
        }
    }

    async fn resolve_with_signer_inner(
        &self,
        username: &str,
        domain: &str,
        signer: Option<&HttpSignatureSigner<'_>>,
    ) -> Result<RemoteActor, RemoteFetchError> {
        let webfinger = webfinger_url(username, domain)?;
        let expected_origin = remote_origin(domain)?;
        let response = match self
            .fetcher
            .get_bound_to_origin(
                webfinger,
                WEBFINGER_CONTENT_TYPES,
                Some(expected_origin.clone()),
            )
            .await
        {
            Ok(response) => response,
            Err(RemoteFetchError::UnexpectedStatus(StatusCode::NOT_FOUND)) => {
                let host_meta_url = expected_origin
                    .join(".well-known/host-meta")
                    .map_err(|_| RemoteFetchError::InvalidUrl)?;
                let host_meta = self
                    .fetcher
                    .get_bound_to_origin(
                        host_meta_url,
                        HOST_META_CONTENT_TYPES,
                        Some(expected_origin.clone()),
                    )
                    .await?;
                let webfinger = parse_host_meta_webfinger_url(&host_meta.body, username, domain)?;
                self.fetcher
                    .get_bound_to_origin(
                        webfinger,
                        WEBFINGER_CONTENT_TYPES,
                        Some(expected_origin.clone()),
                    )
                    .await?
            }
            Err(error) => return Err(error),
        };
        ensure_remote_origin(&response.url, domain)?;
        let document = parse_webfinger_document(&response.body, username, &expected_origin)?;
        let actor_response = self
            .fetcher
            .get_bound_to_origin_with_signer(
                document.self_link.clone(),
                ACTIVITYPUB_CONTENT_TYPES,
                Some(expected_origin.clone()),
                signer,
            )
            .await?;
        ensure_remote_origin(&actor_response.url, domain)?;
        let (mut actor, key_references) = parse_remote_actor_document(
            &actor_response.body,
            &document.self_link,
            username,
            domain,
        )?;
        let mut key_set_complete = true;
        for key_id in key_references {
            match self
                .fetch_remote_key(&key_id, &actor.id, &expected_origin, signer)
                .await
            {
                Ok(key) => actor.public_keys.push(key),
                Err(RemoteFetchError::Signing) => return Err(RemoteFetchError::Signing),
                Err(_) => key_set_complete = false,
            }
        }
        actor.key_set_complete = key_set_complete;
        self.fetcher.validate_target(&actor.inbox).await?;
        if let Some(shared_inbox) = &actor.shared_inbox {
            self.fetcher.validate_target(shared_inbox).await?;
        }
        if let Some(profile_url) = &actor.profile_url {
            self.fetcher.validate_target(profile_url).await?;
        }
        Ok(actor)
    }

    async fn resolve_actor_uri_inner(
        &self,
        actor_url: &Url,
        signer: Option<&HttpSignatureSigner<'_>>,
    ) -> Result<RemoteActor, RemoteFetchError> {
        let expected_origin = remote_url_origin(actor_url)?;
        let response = self
            .fetcher
            .get_bound_to_origin_with_signer(
                actor_url.clone(),
                ACTIVITYPUB_CONTENT_TYPES,
                Some(expected_origin.clone()),
                signer,
            )
            .await?;
        if !same_origin_url(&response.url, &expected_origin) {
            return Err(RemoteFetchError::OriginMismatch);
        }
        let document = serde_json::from_slice::<Value>(&response.body)
            .map_err(|_| RemoteFetchError::InvalidRepresentation)?;
        if document.get("id").and_then(Value::as_str) != Some(actor_url.as_str()) {
            return Err(RemoteFetchError::IdentityMismatch);
        }
        let (username, domain) = remote_actor_handle(&document, actor_url)?;
        self.verify_remote_actor_webfinger(&username, &domain, actor_url)
            .await?;
        let (mut actor, key_references) =
            parse_remote_actor_document(&response.body, actor_url, &username, &domain)?;
        actor.key_set_complete = true;
        for key_id in key_references {
            match self
                .fetch_remote_key(&key_id, actor_url, &expected_origin, signer)
                .await
            {
                Ok(key) => actor.public_keys.push(key),
                Err(_) => actor.key_set_complete = false,
            }
        }
        self.fetcher.validate_target(&actor.inbox).await?;
        if let Some(shared_inbox) = &actor.shared_inbox {
            self.fetcher.validate_target(shared_inbox).await?;
        }
        if let Some(profile_url) = &actor.profile_url {
            self.fetcher.validate_target(profile_url).await?;
        }
        Ok(actor)
    }

    async fn fetch_remote_key(
        &self,
        key_id: &Url,
        actor_url: &Url,
        expected_origin: &Url,
        signer: Option<&HttpSignatureSigner<'_>>,
    ) -> Result<RemotePublicKey, RemoteFetchError> {
        validate_remote_key_id(key_id, actor_url)?;
        let document_url = remote_key_document_url(key_id)?;
        let response = self
            .fetcher
            .get_bound_to_origin_with_signer(
                document_url,
                ACTIVITYPUB_CONTENT_TYPES,
                Some(expected_origin.clone()),
                signer,
            )
            .await?;
        if !same_origin_url(&response.url, expected_origin) {
            return Err(RemoteFetchError::OriginMismatch);
        }
        parse_remote_key_document(&response.body, key_id, actor_url)
    }
}

#[derive(Debug, Eq, PartialEq)]
struct WebFingerDocument {
    subject: String,
    self_link: Url,
}

/// Builds the canonical `WebFinger` URL for a remote account handle.
///
/// # Errors
///
/// Returns [`RemoteFetchError::InvalidUrl`] when the username or domain cannot form a safe
/// account origin.
pub fn webfinger_url(username: &str, domain: &str) -> Result<Url, RemoteFetchError> {
    let domain = canonical_remote_domain(domain)?;
    let origin = remote_origin(&domain)?;
    webfinger_url_at_origin(username, &domain, &origin)
}

fn webfinger_url_at_origin(
    username: &str,
    domain: &str,
    origin: &Url,
) -> Result<Url, RemoteFetchError> {
    let domain = canonical_domain_for_url(origin, domain)?;
    if !valid_remote_username(username) {
        return Err(RemoteFetchError::InvalidUrl);
    }
    let mut url = origin
        .join(".well-known/webfinger")
        .map_err(|_| RemoteFetchError::InvalidUrl)?;
    url.query_pairs_mut()
        .append_pair("resource", &format!("acct:{username}@{domain}"));
    Ok(url)
}

#[must_use]
pub fn valid_remote_username(username: &str) -> bool {
    if username.chars().count() > MAX_REMOTE_USERNAME_CHARS {
        return false;
    }
    let mut has_word = false;
    let mut ends_in_word = false;
    for byte in username.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'_' {
            has_word = true;
            ends_in_word = true;
        } else if matches!(byte, b'.' | b'-') {
            if !has_word {
                return false;
            }
            ends_in_word = false;
        } else {
            return false;
        }
    }
    has_word && ends_in_word
}

/// Normalizes a remote account domain using URL/IDNA parsing and removes default ports.
///
/// # Errors
///
/// Returns [`RemoteFetchError::InvalidUrl`] when the value cannot form a safe remote origin.
pub fn canonical_remote_domain(domain: &str) -> Result<String, RemoteFetchError> {
    let origin = remote_origin(domain)?;
    canonical_remote_domain_from_origin(&origin)
}

/// Normalizes a remote account URL using its actual scheme and removes only its default port.
///
/// # Errors
///
/// Returns [`RemoteFetchError::InvalidUrl`] when the URL cannot form a safe remote account
/// origin.
pub fn canonical_remote_domain_from_url(url: &Url) -> Result<String, RemoteFetchError> {
    validate_remote_url(url)?;
    canonical_remote_domain_from_origin(url)
}

fn canonical_remote_domain_from_origin(origin: &Url) -> Result<String, RemoteFetchError> {
    let host = canonical_remote_host_from_origin(origin)?;
    if host.is_empty() {
        return Err(RemoteFetchError::InvalidUrl);
    }
    let default_port = match origin.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    let port = origin.port().filter(|port| Some(*port) != default_port);
    Ok(match port {
        Some(port) if host.contains(':') => format!("[{host}]:{port}"),
        Some(port) => format!("{host}:{port}"),
        None if host.contains(':') => format!("[{host}]"),
        None => host.to_owned(),
    })
}

fn parse_webfinger_document(
    body: &[u8],
    username: &str,
    expected_origin: &Url,
) -> Result<WebFingerDocument, RemoteFetchError> {
    let document = serde_json::from_slice::<Value>(body)
        .map_err(|_| RemoteFetchError::InvalidRepresentation)?;
    let subject = document
        .get("subject")
        .and_then(Value::as_str)
        .filter(|subject| !subject.trim().is_empty())
        .ok_or(RemoteFetchError::InvalidRepresentation)?;
    let domain = canonical_remote_domain_from_url(expected_origin)?;
    let expected_subject = format!("acct:{username}@{domain}");
    if !subject.eq_ignore_ascii_case(&expected_subject) {
        return Err(RemoteFetchError::IdentityMismatch);
    }
    let links = document
        .get("links")
        .and_then(Value::as_array)
        .ok_or(RemoteFetchError::InvalidRepresentation)?;
    let link = links
        .iter()
        .find(|link| {
            link.get("rel").and_then(Value::as_str) == Some("self")
                && content_type_allowed(
                    link.get("type").and_then(Value::as_str),
                    ACTIVITYPUB_CONTENT_TYPES,
                )
        })
        .ok_or(RemoteFetchError::InvalidRepresentation)?;
    let self_link = link
        .get("href")
        .and_then(Value::as_str)
        .and_then(|href| Url::parse(href).ok())
        .ok_or(RemoteFetchError::InvalidRepresentation)?;
    validate_remote_url(&self_link)?;
    if !same_origin_url(&self_link, expected_origin) {
        return Err(RemoteFetchError::OriginMismatch);
    }
    Ok(WebFingerDocument {
        subject: subject.to_owned(),
        self_link,
    })
}

fn parse_host_meta_webfinger_url(
    body: &[u8],
    username: &str,
    domain: &str,
) -> Result<Url, RemoteFetchError> {
    let body = std::str::from_utf8(body).map_err(|_| RemoteFetchError::InvalidRepresentation)?;
    let domain = canonical_remote_domain(domain)?;
    let resource = format!("acct:{username}@{domain}");
    let mut remainder = body;
    while let Some(start) = remainder.find("<Link") {
        remainder = &remainder[start + "<Link".len()..];
        if !remainder.chars().next().is_some_and(|character| {
            character.is_ascii_whitespace() || character == '/' || character == '>'
        }) {
            continue;
        }
        let Some(end) = remainder.find('>') else {
            return Err(RemoteFetchError::InvalidRepresentation);
        };
        let tag = &remainder[..end];
        let rel = xml_attribute(tag, "rel");
        let template = xml_attribute(tag, "template");
        remainder = &remainder[end + 1..];
        if rel.as_deref() != Some("lrdd") {
            continue;
        }
        let template = template.ok_or(RemoteFetchError::InvalidRepresentation)?;
        let template = html_escape::decode_html_entities(&template);
        let url = Url::parse(&template.replace("{uri}", &resource))
            .map_err(|_| RemoteFetchError::InvalidRepresentation)?;
        validate_remote_url(&url)?;
        ensure_remote_origin(&url, &domain)?;
        return Ok(url);
    }
    Err(RemoteFetchError::InvalidRepresentation)
}

fn canonical_domain_for_url(url: &Url, domain: &str) -> Result<String, RemoteFetchError> {
    let url_domain = canonical_remote_domain_from_url(url)?;
    if domain.eq_ignore_ascii_case(&url_domain) {
        return Ok(url_domain);
    }
    let domain = canonical_remote_domain(domain)?;
    if domain.eq_ignore_ascii_case(&url_domain) {
        Ok(url_domain)
    } else {
        Err(RemoteFetchError::OriginMismatch)
    }
}

fn xml_attribute(tag: &str, name: &str) -> Option<String> {
    tag.match_indices(name).find_map(|(offset, _)| {
        if offset > 0 && !tag.as_bytes()[offset - 1].is_ascii_whitespace() {
            return None;
        }
        let rest = tag.get(offset + name.len()..)?.trim_start();
        let rest = rest.strip_prefix('=')?.trim_start();
        let quote = rest.as_bytes().first().copied()?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        let end = rest[1..].find(char::from(quote))? + 1;
        Some(rest[1..end].to_owned())
    })
}

#[allow(clippy::too_many_lines)]
#[cfg(test)]
fn parse_remote_actor(
    body: &[u8],
    actor_url: &Url,
    username: &str,
    domain: &str,
) -> Result<RemoteActor, RemoteFetchError> {
    parse_remote_actor_document(body, actor_url, username, domain).map(|(actor, _)| actor)
}

fn parse_remote_actor_document(
    body: &[u8],
    actor_url: &Url,
    username: &str,
    domain: &str,
) -> Result<(RemoteActor, Vec<Url>), RemoteFetchError> {
    let actor_domain = remote_actor_domain(actor_url)?;
    let domain = if domain.eq_ignore_ascii_case(&actor_domain) {
        actor_domain.clone()
    } else {
        let domain = canonical_remote_domain(domain)?;
        if !domain.eq_ignore_ascii_case(&actor_domain) {
            return Err(RemoteFetchError::OriginMismatch);
        }
        domain
    };
    validate_activitypub_identity(body, actor_url)?;
    let actor = serde_json::from_slice::<Value>(body)
        .map_err(|_| RemoteFetchError::InvalidRepresentation)?;
    if !supported_activitypub_context(actor.get("@context")) {
        return Err(RemoteFetchError::InvalidRepresentation);
    }
    let actor_type = actor
        .get("type")
        .and_then(actor_type)
        .ok_or(RemoteFetchError::InvalidRepresentation)?;
    let actor_username = actor
        .get("webfinger")
        .and_then(Value::as_str)
        .and_then(parse_account_subject)
        .filter(|(_, actor_domain)| actor_domain.eq_ignore_ascii_case(&domain))
        .map(|(actor_username, _)| actor_username.to_owned())
        .or_else(|| {
            actor
                .get("preferredUsername")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        })
        .ok_or(RemoteFetchError::InvalidRepresentation)?;
    if !valid_remote_username(&actor_username) || !actor_username.eq_ignore_ascii_case(username) {
        return Err(RemoteFetchError::IdentityMismatch);
    }
    let display_name = actor
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(MAX_REMOTE_DISPLAY_NAME_CHARS)
        .collect();
    let note = actor
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(MAX_REMOTE_NOTE_CHARS)
        .collect();
    let suspended = match actor.get("suspended") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        _ => return Err(RemoteFetchError::InvalidRepresentation),
    };
    let inbox = actor
        .get("inbox")
        .and_then(url_value)
        .ok_or(RemoteFetchError::InvalidRepresentation)?;
    validate_remote_url(&inbox)?;
    let profile_url = optional_remote_url(&actor, "url")?;
    if profile_url
        .as_ref()
        .is_some_and(|profile| !same_host(profile, actor_url))
    {
        return Err(RemoteFetchError::OriginMismatch);
    }
    let shared_inbox = actor
        .get("endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get("sharedInbox"))
        .or_else(|| actor.get("sharedInbox"))
        .and_then(url_value);
    if let Some(shared_inbox) = &shared_inbox {
        validate_remote_url(shared_inbox)?;
    }
    let parsed_public_keys = parse_public_keys(&actor, actor_url)?;
    Ok((
        RemoteActor {
            id: actor_url.clone(),
            username: actor_username,
            actor_type,
            display_name,
            note,
            suspended,
            profile_url,
            avatar: actor_image_field(actor.get("icon"))?,
            header: actor_image_field(actor.get("image"))?,
            inbox,
            shared_inbox,
            followers: optional_remote_url(&actor, "followers")?,
            following: optional_remote_url(&actor, "following")?,
            public_keys: parsed_public_keys.embedded,
            key_set_complete: parsed_public_keys.references.is_empty(),
        },
        parsed_public_keys.references,
    ))
}

// Nested options encode absent/null/value, matching Update presence semantics.
#[allow(clippy::option_option)]
fn actor_image_field(value: Option<&Value>) -> Result<Option<Option<String>>, RemoteFetchError> {
    value
        .map(|value| {
            crate::mastodon::activitypub_inbox::actor_image_uri(Some(value))
                .map_err(|_| RemoteFetchError::InvalidRepresentation)
        })
        .transpose()
}

fn parse_account_subject(value: &str) -> Option<(&str, &str)> {
    let value = value.strip_prefix("acct:").unwrap_or(value);
    let (username, domain) = value.split_once('@')?;
    if username.is_empty() || domain.is_empty() || domain.contains('@') {
        None
    } else {
        Some((username, domain))
    }
}

/// Returns the normalized hostname used by hostname-based domain policies.
///
/// # Errors
///
/// Returns [`RemoteFetchError::InvalidUrl`] when the value cannot form a safe remote origin.
pub fn canonical_remote_host(domain: &str) -> Result<String, RemoteFetchError> {
    canonical_remote_host_from_origin(&remote_origin(domain)?).map(ToOwned::to_owned)
}

fn canonical_remote_host_from_origin(origin: &Url) -> Result<&str, RemoteFetchError> {
    origin
        .host_str()
        .ok_or(RemoteFetchError::InvalidUrl)
        .map(|host| {
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .trim_end_matches('.')
        })
}

fn remote_actor_domain(actor_url: &Url) -> Result<String, RemoteFetchError> {
    canonical_remote_domain_from_url(actor_url)
}

fn remote_actor_handle(
    document: &Value,
    actor_url: &Url,
) -> Result<(String, String), RemoteFetchError> {
    let domain = remote_actor_domain(actor_url)?;
    let username = document
        .get("webfinger")
        .and_then(Value::as_str)
        .and_then(parse_account_subject)
        .filter(|(_, actor_domain)| actor_domain.eq_ignore_ascii_case(&domain))
        .map(|(username, _)| username.to_owned())
        .or_else(|| {
            document
                .get("preferredUsername")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        })
        .filter(|username| valid_remote_username(username))
        .ok_or(RemoteFetchError::InvalidRepresentation)?;
    Ok((username, domain))
}

fn actor_contains_key(actor: &RemoteActor, references: &[Url], key_id: &Url) -> bool {
    actor.public_keys.iter().any(|key| key.id == *key_id)
        || references.iter().any(|reference| reference == key_id)
}

fn supported_activitypub_context(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(value)) => value == "https://www.w3.org/ns/activitystreams",
        Some(Value::Array(values)) => values
            .iter()
            .any(|value| value.as_str() == Some("https://www.w3.org/ns/activitystreams")),
        _ => false,
    }
}

fn url_value(value: &Value) -> Option<Url> {
    match value {
        Value::String(value) => Url::parse(value).ok(),
        Value::Array(values) => values.iter().find_map(url_value),
        Value::Object(object) => object
            .get("id")
            .or_else(|| object.get("href"))
            .and_then(url_value),
        _ => None,
    }
}

struct ParsedPublicKeys {
    embedded: Vec<RemotePublicKey>,
    references: Vec<Url>,
}

fn parse_public_keys(actor: &Value, actor_url: &Url) -> Result<ParsedPublicKeys, RemoteFetchError> {
    let Some(value) = actor.get("publicKey") else {
        return Ok(ParsedPublicKeys {
            embedded: Vec::new(),
            references: Vec::new(),
        });
    };
    let values = match value {
        Value::Array(values) => values.iter().collect::<Vec<_>>(),
        Value::Object(_) | Value::String(_) => vec![value],
        Value::Null => Vec::new(),
        _ => return Err(RemoteFetchError::InvalidRepresentation),
    };
    if values.len() > MAX_REMOTE_PUBLIC_KEYS {
        return Err(RemoteFetchError::InvalidRepresentation);
    }
    let mut key_ids = HashSet::with_capacity(values.len());
    let mut embedded = Vec::new();
    let mut references = Vec::new();
    for value in values {
        if value.is_null() {
            continue;
        }
        let Some(public_key) = value.as_object() else {
            let id = parse_remote_key_id(value)?;
            validate_remote_key_id(&id, actor_url)?;
            if !key_ids.insert(id.clone()) {
                return Err(RemoteFetchError::InvalidRepresentation);
            }
            references.push(id);
            continue;
        };
        let id = public_key
            .get("id")
            .ok_or(RemoteFetchError::InvalidRepresentation)
            .and_then(parse_remote_key_id)?;
        validate_remote_key_id(&id, actor_url)?;
        if !key_ids.insert(id.clone()) {
            return Err(RemoteFetchError::InvalidRepresentation);
        }
        let owner = public_key
            .get("owner")
            .map(parse_remote_key_id_or_url)
            .transpose()?
            .or_else(|| Some(actor_url.clone()));
        if owner.as_ref() != Some(actor_url) {
            return Err(RemoteFetchError::IdentityMismatch);
        }
        if public_key.contains_key("publicKeyPem") {
            embedded.push(parse_remote_public_key_object(value, Some(&id), actor_url)?);
        } else {
            references.push(id);
        }
    }
    Ok(ParsedPublicKeys {
        embedded,
        references,
    })
}

fn parse_remote_key_id(value: &Value) -> Result<Url, RemoteFetchError> {
    value
        .as_str()
        .and_then(|value| Url::parse(value).ok())
        .ok_or(RemoteFetchError::InvalidRepresentation)
}

fn parse_remote_key_id_or_url(value: &Value) -> Result<Url, RemoteFetchError> {
    match value {
        Value::String(_) => parse_remote_key_id(value),
        Value::Object(_) => value
            .get("id")
            .or_else(|| value.get("href"))
            .ok_or(RemoteFetchError::InvalidRepresentation)
            .and_then(parse_remote_key_id),
        _ => Err(RemoteFetchError::InvalidRepresentation),
    }
}

fn parse_remote_public_key_object(
    value: &Value,
    expected_id: Option<&Url>,
    actor_url: &Url,
) -> Result<RemotePublicKey, RemoteFetchError> {
    let key = parse_remote_public_key_object_unbound(value, expected_id)?;
    if key.owner != *actor_url {
        return Err(RemoteFetchError::IdentityMismatch);
    }
    validate_remote_key_id(&key.id, actor_url)?;
    Ok(key)
}

fn parse_remote_public_key_object_unbound(
    value: &Value,
    expected_id: Option<&Url>,
) -> Result<RemotePublicKey, RemoteFetchError> {
    let public_key = value
        .as_object()
        .ok_or(RemoteFetchError::InvalidRepresentation)?;
    let owner = public_key
        .get("owner")
        .ok_or(RemoteFetchError::InvalidRepresentation)
        .and_then(parse_remote_key_id_or_url)?;
    validate_remote_url(&owner)?;
    let id = public_key
        .get("id")
        .ok_or(RemoteFetchError::InvalidRepresentation)
        .and_then(parse_remote_key_id)?;
    validate_remote_key_uri(&id)?;
    if expected_id.is_some_and(|expected| expected != &id) {
        return Err(RemoteFetchError::IdentityMismatch);
    }
    let pem = public_key
        .get("publicKeyPem")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .filter(|value| value.chars().count() <= MAX_REMOTE_PUBLIC_KEY_CHARS)
        .map(str::to_owned)
        .ok_or(RemoteFetchError::InvalidRepresentation)?;
    Ok(RemotePublicKey { id, owner, pem })
}

fn parse_remote_key_document(
    body: &[u8],
    key_id: &Url,
    actor_url: &Url,
) -> Result<RemotePublicKey, RemoteFetchError> {
    validate_remote_key_id(key_id, actor_url)?;
    let document = serde_json::from_slice::<Value>(body)
        .map_err(|_| RemoteFetchError::InvalidRepresentation)?;
    if document.get("publicKeyPem").is_some() {
        if !supported_key_context(document.get("@context")) {
            return Err(RemoteFetchError::InvalidRepresentation);
        }
        return parse_remote_public_key_object(&document, Some(key_id), actor_url);
    }
    validate_activitypub_identity(body, actor_url)?;
    if !supported_activitypub_context(document.get("@context"))
        || document.get("type").and_then(actor_type).is_none()
    {
        return Err(RemoteFetchError::InvalidRepresentation);
    }
    parse_public_keys(&document, actor_url)?
        .embedded
        .into_iter()
        .find(|key| key.id == *key_id)
        .ok_or(RemoteFetchError::InvalidRepresentation)
}

fn supported_key_context(value: Option<&Value>) -> bool {
    supported_activitypub_context(value)
        || match value {
            Some(Value::String(value)) => value == SECURITY_CONTEXT,
            Some(Value::Array(values)) => values
                .iter()
                .any(|value| value.as_str() == Some(SECURITY_CONTEXT)),
            _ => false,
        }
}

fn remote_key_document_url(key_id: &Url) -> Result<Url, RemoteFetchError> {
    let mut document_url = key_id.clone();
    document_url.set_fragment(None);
    validate_remote_url(&document_url)?;
    Ok(document_url)
}

fn validate_remote_key_id(key_id: &Url, actor_url: &Url) -> Result<(), RemoteFetchError> {
    validate_remote_key_uri(key_id)?;
    let mut key_document = key_id.clone();
    key_document.set_fragment(None);
    if !same_origin_url(&key_document, actor_url) {
        return Err(RemoteFetchError::OriginMismatch);
    }
    Ok(())
}

fn validate_remote_key_uri(key_id: &Url) -> Result<(), RemoteFetchError> {
    if key_id.as_str().chars().count() > MAX_REMOTE_KEY_ID_CHARS {
        return Err(RemoteFetchError::InvalidRepresentation);
    }
    remote_key_document_url(key_id)?;
    Ok(())
}

fn actor_type(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if SUPPORTED_ACTOR_TYPES.contains(&value.as_str()) => {
            Some(value.clone())
        }
        Value::Array(values) => values.iter().find_map(actor_type),
        _ => None,
    }
}

fn optional_remote_url(document: &Value, key: &str) -> Result<Option<Url>, RemoteFetchError> {
    let Some(value) = document.get(key) else {
        return Ok(None);
    };
    if value.is_null() || matches!(value, Value::Array(values) if values.is_empty()) {
        return Ok(None);
    }
    let url = url_value(value).ok_or(RemoteFetchError::InvalidRepresentation)?;
    validate_remote_url(&url)?;
    Ok(Some(url))
}

fn same_host(left: &Url, right: &Url) -> bool {
    left.host_str().is_some_and(|left| {
        right
            .host_str()
            .is_some_and(|right| left.eq_ignore_ascii_case(right))
    })
}

fn same_origin_url(left: &Url, right: &Url) -> bool {
    left.scheme().eq_ignore_ascii_case(right.scheme())
        && same_host(left, right)
        && left.port_or_known_default() == right.port_or_known_default()
}

fn remote_origin(domain: &str) -> Result<Url, RemoteFetchError> {
    let domain = domain.trim_end_matches('.');
    if domain.trim().is_empty()
        || domain
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'@' | b'/' | b'?' | b'#'))
    {
        return Err(RemoteFetchError::InvalidUrl);
    }
    let scheme = if domain.to_ascii_lowercase().ends_with(".onion") {
        "http"
    } else {
        "https"
    };
    let origin =
        Url::parse(&format!("{scheme}://{domain}/")).map_err(|_| RemoteFetchError::InvalidUrl)?;
    if origin.host_str().is_none()
        || origin.username() != ""
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(RemoteFetchError::InvalidUrl);
    }
    Ok(origin)
}

fn ensure_remote_origin(url: &Url, domain: &str) -> Result<(), RemoteFetchError> {
    let expected = remote_origin(domain)?;
    if same_origin_url(url, &expected) {
        Ok(())
    } else {
        Err(RemoteFetchError::OriginMismatch)
    }
}

impl RemoteFetchLimits {
    fn bounded(self) -> Self {
        Self {
            connect_timeout: self
                .connect_timeout
                .max(Duration::from_millis(1))
                .min(MAX_CONFIGURED_TIMEOUT),
            request_timeout: self
                .request_timeout
                .max(Duration::from_millis(1))
                .min(MAX_CONFIGURED_TIMEOUT),
            max_request_bytes: self
                .max_request_bytes
                .clamp(1, MAX_CONFIGURED_REQUEST_BYTES),
            max_response_bytes: self
                .max_response_bytes
                .clamp(1, MAX_CONFIGURED_RESPONSE_BYTES),
            max_redirects: self.max_redirects.min(MAX_CONFIGURED_REDIRECTS),
        }
    }
}

/// Validates the scheme, authority, and URL components accepted for remote fetching.
///
/// # Errors
///
/// Returns [`RemoteFetchError::InvalidUrl`] for unsupported schemes, missing hosts, embedded
/// credentials, or fragments.
pub fn validate_remote_url(url: &Url) -> Result<(), RemoteFetchError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(RemoteFetchError::InvalidUrl);
    }
    Ok(())
}

/// Validates every address returned for a remote host before a connection is attempted.
///
/// # Errors
///
/// Returns [`RemoteFetchError::NoAddresses`] for an empty result or
/// [`RemoteFetchError::BlockedAddress`] when any result is not publicly routable.
pub fn validate_resolved_addresses(addresses: &[SocketAddr]) -> Result<(), RemoteFetchError> {
    if addresses.is_empty() {
        return Err(RemoteFetchError::NoAddresses);
    }
    if let Some(address) = addresses.iter().find(|address| is_blocked_ip(address.ip())) {
        return Err(RemoteFetchError::BlockedAddress(address.ip()));
    }
    Ok(())
}

#[must_use]
pub fn is_blocked_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => blocked_ipv4(address),
        IpAddr::V6(address) => blocked_ipv6(address),
    }
}

pub fn content_type_allowed(content_type: Option<&str>, accepted: &[&str]) -> bool {
    if accepted.is_empty() {
        return true;
    }
    let Some((content_type, content_parameters)) = content_type.and_then(content_type_parts) else {
        return false;
    };
    accepted.iter().any(|expected| {
        let Some((expected_type, expected_parameters)) = content_type_parts(expected) else {
            return false;
        };
        let media_type_matches = expected_type == "*/*"
            || content_type.eq_ignore_ascii_case(expected_type)
            || (expected_type.ends_with("/*")
                && content_type
                    .get(..expected_type.len() - 1)
                    .is_some_and(|actual| {
                        actual.eq_ignore_ascii_case(&expected_type[..expected_type.len() - 1])
                    }));
        media_type_matches
            && expected_parameters.iter().all(|(name, value)| {
                content_parameters
                    .iter()
                    .any(|(actual_name, actual_value)| {
                        actual_name.eq_ignore_ascii_case(name) && actual_value == value
                    })
            })
    })
}

fn content_type_parts(value: &str) -> Option<(&str, Vec<(&str, &str)>)> {
    let mut parts = value.split(';');
    let media_type = parts.next()?.trim();
    if media_type.is_empty() {
        return None;
    }
    let parameters = parts
        .map(str::trim)
        .map(|parameter| {
            let (name, value) = parameter.split_once('=')?;
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            Some((name.trim(), value))
        })
        .collect::<Option<Vec<_>>>()?;
    Some((media_type, parameters))
}

/// Validates that a JSON `ActivityPub` document declares the requested canonical ID.
///
/// # Errors
///
/// Returns [`RemoteFetchError::InvalidRepresentation`] for invalid JSON or
/// [`RemoteFetchError::IdentityMismatch`] when the document's `id` differs.
pub fn validate_activitypub_identity(
    body: &[u8],
    expected_id: &Url,
) -> Result<(), RemoteFetchError> {
    let document = serde_json::from_slice::<serde_json::Value>(body)
        .map_err(|_| RemoteFetchError::InvalidRepresentation)?;
    if document.get("id").and_then(serde_json::Value::as_str) == Some(expected_id.as_str()) {
        Ok(())
    } else {
        Err(RemoteFetchError::IdentityMismatch)
    }
}

fn collect_remote_addresses(
    addresses: impl IntoIterator<Item = SocketAddr>,
) -> Result<Vec<SocketAddr>, RemoteFetchError> {
    let mut collected = Vec::with_capacity(MAX_RESOLVED_ADDRESSES);
    for address in addresses {
        if collected.len() >= MAX_RESOLVED_ADDRESSES {
            return Err(RemoteFetchError::Dns);
        }
        collected.push(address);
    }
    Ok(collected)
}

async fn resolve_remote_addresses(url: &Url) -> Result<Vec<SocketAddr>, RemoteFetchError> {
    let port = url
        .port_or_known_default()
        .ok_or(RemoteFetchError::InvalidUrl)?;
    match url.host().ok_or(RemoteFetchError::InvalidUrl)? {
        Host::Ipv4(address) => Ok(vec![SocketAddr::new(IpAddr::V4(address), port)]),
        Host::Ipv6(address) => Ok(vec![SocketAddr::new(IpAddr::V6(address), port)]),
        Host::Domain(domain) => {
            let addresses = lookup_host((domain, port))
                .await
                .map_err(|_| RemoteFetchError::Dns)?;
            collect_remote_addresses(addresses)
        }
    }
}

fn blocked_ipv4(address: Ipv4Addr) -> bool {
    let value = u32::from(address);
    address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_multicast()
        || value == u32::MAX
        || in_ipv4_range(value, 0, 0x00ff_ffff)
        || in_ipv4_range(value, 0x6440_0000, 0x647f_ffff)
        || in_ipv4_range(value, 0xc000_0000, 0xc000_00ff)
        || in_ipv4_range(value, 0xc000_0200, 0xc000_02ff)
        || in_ipv4_range(value, 0xc058_6300, 0xc058_63ff)
        || in_ipv4_range(value, 0xc612_0000, 0xc613_ffff)
        || in_ipv4_range(value, 0xc633_6400, 0xc633_64ff)
        || in_ipv4_range(value, 0xcb00_7100, 0xcb00_71ff)
        || in_ipv4_range(value, 0xf000_0000, u32::MAX)
}

fn blocked_ipv6(address: Ipv6Addr) -> bool {
    let bits = address.to_bits();
    let segments = address.segments();
    let mapped_ipv4 =
        segments[..5].iter().all(|segment| *segment == 0) && matches!(segments[5], 0 | 0xffff);
    let compatible_ipv4 = segments[..6].iter().all(|segment| *segment == 0);
    let embedded_ipv4 =
        Ipv4Addr::from(u32::try_from(bits & u128::from(u32::MAX)).expect("IPv4 bits are bounded"));
    (mapped_ipv4 || compatible_ipv4) && blocked_ipv4(embedded_ipv4)
        || address.is_unspecified()
        || address.is_loopback()
        || prefix_matches(bits, 0xfe80_0000_0000_0000_0000_0000_0000_0000, 10)
        || prefix_matches(bits, 0x0064_ff9b_0000_0000_0000_0000_0000_0000, 96)
        || prefix_matches(bits, 0x0064_ff9b_0001_0000_0000_0000_0000_0000, 48)
        || prefix_matches(bits, 0x0100_0000_0000_0000_0000_0000_0000_0000, 64)
        || prefix_matches(bits, 0x2001_0000_0000_0000_0000_0000_0000_0000, 32)
        || prefix_matches(bits, 0x2001_0010_0000_0000_0000_0000_0000_0000, 28)
        || prefix_matches(bits, 0x2001_0020_0000_0000_0000_0000_0000_0000, 28)
        || prefix_matches(bits, 0x2001_0db8_0000_0000_0000_0000_0000_0000, 32)
        || prefix_matches(bits, 0x2002_0000_0000_0000_0000_0000_0000_0000, 16)
        || prefix_matches(bits, 0xfc00_0000_0000_0000_0000_0000_0000_0000, 7)
        || prefix_matches(bits, 0x3fff_0000_0000_0000_0000_0000_0000_0000, 20)
        || prefix_matches(bits, 0xff00_0000_0000_0000_0000_0000_0000_0000, 8)
}

fn in_ipv4_range(value: u32, start: u32, end: u32) -> bool {
    (start..=end).contains(&value)
}

fn prefix_matches(value: u128, prefix: u128, bits: u8) -> bool {
    let mask = u128::MAX << (128 - bits);
    value & mask == prefix & mask
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "test-support")]
    use std::io::{Read, Write};
    use std::net::{IpAddr, SocketAddr};
    #[cfg(feature = "test-support")]
    use std::thread::JoinHandle;
    #[cfg(feature = "test-support")]
    use std::time::Duration as StdDuration;

    use url::Url;

    use super::*;

    #[cfg(feature = "test-support")]
    fn local_http_response(
        response: Option<Vec<u8>>,
        delay: StdDuration,
    ) -> (SocketAddr, JoinHandle<()>) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(StdDuration::from_secs(1)))
                .unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            if let Some(response) = response {
                stream.write_all(&response).unwrap();
            }
        });
        (address, handle)
    }

    #[cfg(feature = "test-support")]
    fn local_http_responses(responses: Vec<Vec<u8>>) -> (SocketAddr, JoinHandle<()>) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let handle = std::thread::spawn(move || {
            for response in responses {
                let mut accepted = None;
                for _ in 0..100 {
                    match listener.accept() {
                        Ok(value) => {
                            accepted = Some(value.0);
                            break;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(StdDuration::from_millis(5));
                        }
                        Err(error) => panic!("test HTTP listener failed: {error}"),
                    }
                }
                let Some(mut stream) = accepted else {
                    return;
                };
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                stream.write_all(&response).unwrap();
            }
        });
        (address, handle)
    }

    #[cfg(feature = "test-support")]
    fn local_http_responses_with_requests(
        responses: Vec<Vec<u8>>,
    ) -> (SocketAddr, JoinHandle<Vec<Vec<u8>>>) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::with_capacity(responses.len());
            for response in responses {
                let mut accepted = None;
                for _ in 0..100 {
                    match listener.accept() {
                        Ok(value) => {
                            accepted = Some(value.0);
                            break;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(StdDuration::from_millis(5));
                        }
                        Err(error) => panic!("test HTTP listener failed: {error}"),
                    }
                }
                let Some(mut stream) = accepted else {
                    return requests;
                };
                stream
                    .set_read_timeout(Some(StdDuration::from_secs(1)))
                    .unwrap();
                let mut request = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(bytes) => {
                            request.extend_from_slice(&chunk[..bytes]);
                            let Some(header_end) = request
                                .windows(4)
                                .position(|window| window == b"\r\n\r\n")
                                .map(|position| position + 4)
                            else {
                                continue;
                            };
                            let content_length = String::from_utf8_lossy(&request[..header_end])
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                                .unwrap_or_default();
                            if request.len() >= header_end + content_length {
                                break;
                            }
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                            ) =>
                        {
                            break;
                        }
                        Err(error) => panic!("test HTTP request read failed: {error}"),
                    }
                }
                requests.push(request);
                stream.write_all(&response).unwrap();
            }
            requests
        });
        (address, handle)
    }

    #[cfg(feature = "test-support")]
    fn request_header_value(request: &[u8], name: &str) -> Option<String> {
        String::from_utf8_lossy(request).lines().find_map(|line| {
            let (header_name, value) = line.split_once(':')?;
            header_name
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    }

    fn fixture_private_key() -> String {
        include_str!("../tests/fixtures/http-signature-private.pem").to_owned()
    }

    #[test]
    fn remote_urls_require_http_without_embedded_credentials_or_fragments() {
        assert!(validate_remote_url(&Url::parse("https://example.com/a").unwrap()).is_ok());
        for value in [
            "ftp://example.com/a",
            "https://user@example.com/a",
            "https://user:password@example.com/a",
            "https://example.com/a#fragment",
        ] {
            assert!(
                validate_remote_url(&Url::parse(value).unwrap()).is_err(),
                "{value}"
            );
        }
        assert!(Url::parse("/relative/path").is_err());
        for value in ["bad:name", ".alice", "alice."] {
            assert!(webfinger_url(value, "example.com").is_err(), "{value}");
        }
        assert!(webfinger_url("alice..bob", "example.com").is_ok());
    }

    #[test]
    fn remote_address_policy_rejects_private_documentation_and_mapped_addresses() {
        for value in [
            "0.0.0.0",
            "0.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "192.0.2.1",
            "192.168.1.1",
            "198.51.100.1",
            "224.0.0.1",
            "250.0.0.1",
            "255.255.255.254",
            "::",
            "::1",
            "::127.0.0.1",
            "::ffff:127.0.0.1",
            "64:ff9b::c000:0201",
            "fc00::1",
            "ff02::1",
        ] {
            assert!(is_blocked_ip(value.parse().unwrap()), "{value}");
        }
        for value in ["8.8.8.8", "::8.8.8.8", "2001:4860:4860::8888"] {
            assert!(!is_blocked_ip(value.parse().unwrap()), "{value}");
        }
    }

    #[test]
    fn remote_dns_answer_sets_are_bounded() {
        let addresses = std::iter::repeat_n(
            SocketAddr::new("8.8.8.8".parse().unwrap(), 443),
            MAX_RESOLVED_ADDRESSES + 1,
        );
        assert!(matches!(
            collect_remote_addresses(addresses),
            Err(RemoteFetchError::Dns)
        ));
        assert_eq!(
            collect_remote_addresses(std::iter::repeat_n(
                SocketAddr::new("8.8.8.8".parse().unwrap(), 443),
                MAX_RESOLVED_ADDRESSES,
            ))
            .unwrap()
            .len(),
            MAX_RESOLVED_ADDRESSES
        );
    }

    #[test]
    fn every_dns_answer_must_be_public_before_connecting() {
        let addresses = [
            SocketAddr::new("8.8.8.8".parse().unwrap(), 443),
            SocketAddr::new("127.0.0.1".parse().unwrap(), 443),
        ];
        assert!(validate_resolved_addresses(&addresses).is_err());
        assert!(validate_resolved_addresses(&[]).is_err());
    }

    #[test]
    fn content_type_matching_honors_required_parameters() {
        assert!(content_type_allowed(
            Some("application/activity+json; profile=\"https://www.w3.org/ns/activitystreams\""),
            &["application/activity+json; profile=\"https://www.w3.org/ns/activitystreams\""]
        ));
        assert!(!content_type_allowed(
            Some("application/ld+json"),
            &["application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\""]
        ));
        assert!(content_type_allowed(Some("image/png"), &["image/*"]));
        assert!(!content_type_allowed(
            Some("text/html"),
            &["application/json"]
        ));
        assert!(!content_type_allowed(None, &["application/json"]));
    }

    #[test]
    fn ipv4_mapped_policy_is_checked_as_ipv4() {
        for value in [
            "::ffff:10.0.0.1",
            "::ffff:0.0.0.1",
            "::ffff:250.0.0.1",
            "::10.0.0.1",
        ] {
            let address: IpAddr = value.parse().unwrap();
            assert!(is_blocked_ip(address), "{value}");
        }
    }

    #[test]
    fn activitypub_identity_must_match_the_requested_id() {
        let expected = Url::parse("https://example.com/users/alice").unwrap();
        assert!(
            validate_activitypub_identity(
                br#"{"id":"https://example.com/users/alice"}"#,
                &expected
            )
            .is_ok()
        );
        assert!(matches!(
            validate_activitypub_identity(br#"{"id":"https://example.com/users/bob"}"#, &expected),
            Err(RemoteFetchError::IdentityMismatch)
        ));
        assert!(matches!(
            validate_activitypub_identity(b"not-json", &expected),
            Err(RemoteFetchError::InvalidRepresentation)
        ));
    }

    #[test]
    fn configured_fetch_limits_are_bounded() {
        let fetcher = RemoteFetcher::new(RemoteFetchLimits {
            connect_timeout: Duration::ZERO,
            request_timeout: Duration::from_mins(10),
            max_request_bytes: usize::MAX,
            max_response_bytes: usize::MAX,
            max_redirects: usize::MAX,
        });
        let limits = fetcher.limits();
        assert_eq!(limits.connect_timeout, Duration::from_millis(1));
        assert_eq!(limits.request_timeout, MAX_CONFIGURED_TIMEOUT);
        assert_eq!(limits.max_request_bytes, MAX_CONFIGURED_REQUEST_BYTES);
        assert_eq!(limits.max_response_bytes, MAX_CONFIGURED_RESPONSE_BYTES);
        assert_eq!(limits.max_redirects, MAX_CONFIGURED_REDIRECTS);
    }

    #[tokio::test]
    async fn remote_domain_budget_caps_hosts_without_blocking_other_hosts() {
        let budget = RemoteDomainBudget::new(2);
        let first = budget
            .acquire(&Url::parse("https://remote.example/first").unwrap())
            .await
            .unwrap();
        let second = budget
            .acquire(&Url::parse("https://REMOTE.example:8443/second").unwrap())
            .await
            .unwrap();
        assert!(matches!(
            budget
                .acquire(&Url::parse("https://remote.example:443/third").unwrap())
                .await,
            Err(RemoteFetchError::DomainBudgetExceeded)
        ));
        let other = budget
            .acquire(&Url::parse("https://other.example/first").unwrap())
            .await
            .unwrap();
        drop(first);
        assert!(
            budget
                .acquire(&Url::parse("https://remote.example/fourth").unwrap())
                .await
                .is_ok()
        );
        drop(second);
        drop(other);
    }

    #[tokio::test]
    async fn remote_domain_budget_releases_permits_after_cancellation() {
        let budget = RemoteDomainBudget::new(1);
        let permit = budget
            .acquire(&Url::parse("https://remote.example/first").unwrap())
            .await
            .unwrap();
        assert!(matches!(
            budget
                .acquire(&Url::parse("https://remote.example/second").unwrap())
                .await,
            Err(RemoteFetchError::DomainBudgetExceeded)
        ));
        drop(permit);
        assert!(
            budget
                .acquire(&Url::parse("https://remote.example/third").unwrap())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn fetcher_applies_the_shared_domain_budget_before_dns() {
        let url = Url::parse("https://remote.example/actor").unwrap();
        let budget = RemoteDomainBudget::new(1);
        let _permit = budget.acquire(&url).await.unwrap();
        let fetcher = RemoteFetcher::with_domain_budget(RemoteFetchLimits::default(), budget);
        assert!(matches!(
            fetcher.get(url, &["application/json"]).await,
            Err(RemoteFetchError::DomainBudgetExceeded)
        ));
    }

    #[tokio::test]
    #[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
    async fn remote_domain_budget_coordinates_independent_pools_and_reclaims_expired_leases()
    -> Result<(), Box<dyn std::error::Error>> {
        let url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
        let first_pool = sqlx::PgPool::connect(&url).await?;
        let second_pool = sqlx::PgPool::connect(&url).await?;
        let target = Url::parse("https://remote-fetch-lease-test.example/actor")?;
        let host = canonical_remote_host_from_origin(&target)?;
        for pool in [&first_pool, &second_pool] {
            sqlx::query("DELETE FROM rustodon.remote_fetch_leases WHERE host = $1")
                .bind(host)
                .execute(pool)
                .await?;
        }

        let first_budget = RemoteDomainBudget::with_operational_pool(2, first_pool.clone());
        let second_budget = RemoteDomainBudget::with_operational_pool(2, second_pool.clone());
        let first_lease = first_budget.acquire(&target).await?;
        let second_lease = second_budget.acquire(&target).await?;
        assert!(matches!(
            first_budget.acquire(&target).await,
            Err(RemoteFetchError::DomainBudgetExceeded)
        ));

        first_lease.release().await;
        let replacement = second_budget.acquire(&target).await?;
        second_lease.release().await;
        replacement.release().await;

        let cancelled = first_budget.acquire(&target).await?;
        drop(cancelled);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM rustodon.remote_fetch_leases WHERE host = $1",
                )
                .bind(host)
                .fetch_one(&first_pool)
                .await?
                    == 0
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Ok::<(), sqlx::Error>(())
        })
        .await??;

        sqlx::query(
            "INSERT INTO rustodon.remote_fetch_leases (host, lease_id, expires_at) \
             VALUES ($1, 'abandoned', clock_timestamp() - interval '1 second')",
        )
        .bind(host)
        .execute(&first_pool)
        .await?;
        let reclaimed = first_budget.acquire(&target).await?;
        reclaimed.release().await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.remote_fetch_leases WHERE host = $1",
            )
            .bind(host)
            .fetch_one(&first_pool)
            .await?,
            0
        );
        Ok(())
    }

    #[tokio::test]
    async fn fetcher_rejects_loopback_before_opening_a_connection() {
        let result = RemoteFetcher::default()
            .get(
                Url::parse("http://127.0.0.1:9/should-not-connect").unwrap(),
                &["application/json"],
            )
            .await;
        assert!(matches!(
            result,
            Err(RemoteFetchError::BlockedAddress(address))
                if address == "127.0.0.1".parse::<IpAddr>().unwrap()
        ));
    }

    #[tokio::test]
    async fn signed_post_rejects_oversized_requests_before_dns_or_signing() {
        let fetcher = RemoteFetcher::new(RemoteFetchLimits {
            max_request_bytes: 1,
            ..RemoteFetchLimits::default()
        });
        let signer = HttpSignatureSigner {
            key_id: "https://local.example/actor#main-key",
            private_key_pem: "not-a-private-key",
        };
        assert!(matches!(
            fetcher
                .post_signed_json(
                    Url::parse("https://remote.example/inbox").unwrap(),
                    b"{}",
                    &signer,
                )
                .await,
            Err(RemoteFetchError::BodyTooLarge)
        ));
    }

    #[tokio::test]
    async fn target_validation_rejects_loopback_without_opening_a_connection() {
        let result = RemoteFetcher::default()
            .validate_target(&Url::parse("http://127.0.0.1:9/inbox").unwrap())
            .await;

        assert!(matches!(
            result,
            Err(RemoteFetchError::BlockedAddress(address))
                if address == "127.0.0.1".parse::<IpAddr>().unwrap()
        ));
    }

    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn bounded_transport_fixtures_fail_closed() {
        let oversized_response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 8\r\nConnection: close\r\n\r\n12345678"
            .as_bytes()
            .to_vec();
        let (address, handle) = local_http_response(Some(oversized_response), StdDuration::ZERO);
        let result = RemoteFetcher::new(RemoteFetchLimits {
            max_response_bytes: 4,
            ..RemoteFetchLimits::default()
        })
        .get_for_test_endpoint(
            Url::parse("http://remote.example/oversized").unwrap(),
            &["application/json"],
            address,
        )
        .await;
        assert!(matches!(result, Err(RemoteFetchError::BodyTooLarge)));
        handle.join().unwrap();

        let redirect_response = b"HTTP/1.1 302 Found\r\nLocation: http://other.example/actor\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
        let (address, handle) = local_http_response(Some(redirect_response), StdDuration::ZERO);
        let result = RemoteFetcher::default()
            .get_for_test_endpoint(
                Url::parse("http://remote.example/redirect").unwrap(),
                &["application/json"],
                address,
            )
            .await;
        assert!(matches!(result, Err(RemoteFetchError::OriginMismatch)));
        handle.join().unwrap();

        let (address, handle) = local_http_response(None, StdDuration::from_millis(100));
        let result = RemoteFetcher::new(RemoteFetchLimits {
            request_timeout: StdDuration::from_millis(20),
            ..RemoteFetchLimits::default()
        })
        .get_for_test_endpoint(
            Url::parse("http://remote.example/slow").unwrap(),
            &["application/json"],
            address,
        )
        .await;
        assert!(matches!(result, Err(RemoteFetchError::Request)));
        handle.join().unwrap();
    }

    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn signed_post_transport_fixtures_enforce_dns_and_redirect_policy() {
        let private_key = fixture_private_key();
        let signer = HttpSignatureSigner {
            key_id: "https://local.example/actor#main-key",
            private_key_pem: &private_key,
        };
        let body = br#"{"type":"Create"}"#;
        let public_address = SocketAddr::new("8.8.8.8".parse().unwrap(), 80);
        let blocked_address = SocketAddr::new("127.0.0.1".parse().unwrap(), 80);

        let mixed_result = RemoteFetcher::default()
            .post_signed_json_for_test_endpoint_with_resolved_addresses(
                Url::parse("http://remote.example/inbox").unwrap(),
                body,
                &signer,
                "127.0.0.1:9".parse().unwrap(),
                vec![public_address, blocked_address],
            )
            .await;
        assert!(matches!(
            mixed_result,
            Err(RemoteFetchError::BlockedAddress(address))
                if address == blocked_address.ip()
        ));

        for redirect_status in ["307 Temporary Redirect", "308 Permanent Redirect"] {
            let redirect_response = format!(
                "HTTP/1.1 {redirect_status}\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .into_bytes();
            let final_response =
                b"HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                    .to_vec();
            let (address, handle) =
                local_http_responses_with_requests(vec![redirect_response, final_response]);
            let result = RemoteFetcher::default()
                .post_signed_json_for_test_endpoint_with_resolved_address_sets(
                    Url::parse("http://remote.example/start").unwrap(),
                    body,
                    &signer,
                    address,
                    vec![vec![public_address], vec![public_address]],
                )
                .await;
            assert_eq!(result.unwrap().status, StatusCode::ACCEPTED);
            let requests = handle.join().unwrap();
            assert_eq!(requests.len(), 2);
            assert!(String::from_utf8_lossy(&requests[0]).starts_with("POST /start HTTP/1.1"));
            assert!(String::from_utf8_lossy(&requests[1]).starts_with("POST /final HTTP/1.1"));
            assert!(requests[0].ends_with(body));
            assert!(requests[1].ends_with(body));
            assert_eq!(
                request_header_value(&requests[0], "digest"),
                Some(body_digest_header(body))
            );
            assert_eq!(
                request_header_value(&requests[1], "digest"),
                Some(body_digest_header(body))
            );
            assert_ne!(
                request_header_value(&requests[0], "signature"),
                request_header_value(&requests[1], "signature")
            );
        }

        let redirect_response = b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
        let (address, handle) = local_http_responses(vec![redirect_response]);
        let result = RemoteFetcher::default()
            .post_signed_json_for_test_endpoint_with_resolved_address_sets(
                Url::parse("http://remote.example/start").unwrap(),
                body,
                &signer,
                address,
                vec![vec![public_address], vec![public_address, blocked_address]],
            )
            .await;
        assert!(matches!(
            result,
            Err(RemoteFetchError::BlockedAddress(address))
                if address == blocked_address.ip()
        ));
        handle.join().unwrap();

        let rejected_redirect = b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
        let (address, handle) = local_http_responses(vec![rejected_redirect]);
        let result = RemoteFetcher::default()
            .post_signed_json_for_test_endpoint_with_resolved_address_sets(
                Url::parse("http://remote.example/start").unwrap(),
                body,
                &signer,
                address,
                vec![vec![public_address]],
            )
            .await;
        assert!(matches!(result, Err(RemoteFetchError::Redirect)));
        handle.join().unwrap();
    }

    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn signed_post_transport_fixtures_enforce_timeout_and_response_limits() {
        let private_key = fixture_private_key();
        let signer = HttpSignatureSigner {
            key_id: "https://local.example/actor#main-key",
            private_key_pem: &private_key,
        };
        let body = br#"{"type":"Create"}"#;
        let public_address = vec![SocketAddr::new("8.8.8.8".parse().unwrap(), 80)];

        let oversized_response =
            b"HTTP/1.1 202 Accepted\r\nContent-Length: 8\r\nConnection: close\r\n\r\n12345678"
                .to_vec();
        let (address, handle) = local_http_responses(vec![oversized_response]);
        let result = RemoteFetcher::new(RemoteFetchLimits {
            max_response_bytes: 4,
            ..RemoteFetchLimits::default()
        })
        .post_signed_json_for_test_endpoint_with_resolved_address_sets(
            Url::parse("http://remote.example/oversized").unwrap(),
            body,
            &signer,
            address,
            vec![public_address.clone()],
        )
        .await;
        assert!(matches!(result, Err(RemoteFetchError::BodyTooLarge)));
        handle.join().unwrap();

        let (address, handle) = local_http_response(None, StdDuration::from_millis(100));
        let result = RemoteFetcher::new(RemoteFetchLimits {
            request_timeout: StdDuration::from_millis(20),
            ..RemoteFetchLimits::default()
        })
        .post_signed_json_for_test_endpoint_with_resolved_address_sets(
            Url::parse("http://remote.example/slow").unwrap(),
            body,
            &signer,
            address,
            vec![public_address],
        )
        .await;
        assert!(matches!(result, Err(RemoteFetchError::Request)));
        handle.join().unwrap();
    }

    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn transport_fixture_rejects_mixed_resolved_addresses_before_connecting() {
        let result = RemoteFetcher::default()
            .get_for_test_endpoint_with_resolved_addresses(
                Url::parse("http://remote.example/rebinding").unwrap(),
                &["application/json"],
                "127.0.0.1:9".parse().unwrap(),
                vec![
                    SocketAddr::new("8.8.8.8".parse().unwrap(), 80),
                    SocketAddr::new("127.0.0.1".parse().unwrap(), 80),
                ],
            )
            .await;
        assert!(matches!(
            result,
            Err(RemoteFetchError::BlockedAddress(address))
                if address == "127.0.0.1".parse::<IpAddr>().unwrap()
        ));
    }

    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn transport_fixture_rechecks_resolved_addresses_after_redirects() {
        let redirect = b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
        let final_response =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                .to_vec();
        let (address, handle) = local_http_responses(vec![redirect, final_response]);
        let result = RemoteFetcher::default()
            .get_for_test_endpoint_with_resolved_address_sets(
                Url::parse("http://remote.example/start").unwrap(),
                &["application/json"],
                address,
                vec![
                    vec![SocketAddr::new("8.8.8.8".parse().unwrap(), 80)],
                    vec![
                        SocketAddr::new("8.8.8.8".parse().unwrap(), 80),
                        SocketAddr::new("127.0.0.1".parse().unwrap(), 80),
                    ],
                ],
            )
            .await;
        assert!(matches!(
            result,
            Err(RemoteFetchError::BlockedAddress(address))
                if address == "127.0.0.1".parse::<IpAddr>().unwrap()
        ));
        handle.join().unwrap();
    }

    #[test]
    fn webfinger_url_encodes_the_requested_account_resource() {
        let url = webfinger_url("Alice", "remote.example").unwrap();

        assert_eq!(
            url.as_str(),
            "https://remote.example/.well-known/webfinger?resource=acct%3AAlice%40remote.example"
        );
    }

    #[test]
    fn remote_domains_are_idna_normalized_and_default_ports_removed() {
        assert_eq!(
            canonical_remote_domain("BÜCHER.example.").unwrap(),
            "xn--bcher-kva.example"
        );
        assert_eq!(
            canonical_remote_domain("remote.example:443").unwrap(),
            "remote.example"
        );
        assert_eq!(
            canonical_remote_domain("remote.example:8443").unwrap(),
            "remote.example:8443"
        );
        assert_eq!(
            canonical_remote_domain("[2001:db8::1]").unwrap(),
            "[2001:db8::1]"
        );
        assert_eq!(
            canonical_remote_host("remote.example:8443").unwrap(),
            "remote.example"
        );
    }

    #[test]
    fn webfinger_requires_a_matching_activitypub_self_link() {
        let expected_origin = Url::parse("https://remote.example/").unwrap();
        let document = parse_webfinger_document(
            br#"{
                "subject": "acct:Alice@remote.example",
                "links": [
                    {"rel": "self", "type": "application/activity+json", "href": "https://remote.example/users/alice"}
                ]
            }"#,
            "Alice",
            &expected_origin,
        )
        .unwrap();

        assert_eq!(document.subject, "acct:Alice@remote.example");
        assert_eq!(
            document.self_link,
            Url::parse("https://remote.example/users/alice").unwrap()
        );
        assert!(matches!(
            parse_webfinger_document(
                br#"{"subject":"acct:Alice@remote.example","links":[]}"#,
                "Alice",
                &expected_origin
            ),
            Err(RemoteFetchError::InvalidRepresentation)
        ));
    }

    #[test]
    fn webfinger_documents_preserve_http_non_default_ports() {
        let expected_origin = Url::parse("http://remote.example:443/").unwrap();
        let url = webfinger_url_at_origin("Alice", "remote.example:443", &expected_origin).unwrap();
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.port(), Some(443));
        assert_eq!(url.path(), "/.well-known/webfinger");
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "resource")
                .map(|(_, value)| value.into_owned()),
            Some("acct:Alice@remote.example:443".to_owned())
        );
        let document = parse_webfinger_document(
            br#"{
                "subject": "acct:Alice@remote.example:443",
                "links": [
                    {"rel": "self", "type": "application/activity+json", "href": "http://remote.example:443/users/alice"}
                ]
            }"#,
            "Alice",
            &expected_origin,
        );
        assert!(document.is_ok());
    }

    #[test]
    fn host_meta_resolves_only_an_origin_bound_lrdd_template() {
        let url = parse_host_meta_webfinger_url(
            br#"<?xml version="1.0"?><XRD><Link rel="other" template="https://other.example/{uri}"/><Link rel="lrdd" template="https://remote.example/.well-known/webfinger?resource={uri}"/></XRD>"#,
            "Alice",
            "remote.example",
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://remote.example/.well-known/webfinger?resource=acct:Alice@remote.example"
        );
        assert!(matches!(
            parse_host_meta_webfinger_url(
                br#"<XRD><Link rel="lrdd" template="https://other.example/{uri}"/></XRD>"#,
                "Alice",
                "remote.example"
            ),
            Err(RemoteFetchError::OriginMismatch)
        ));
    }

    #[test]
    fn signed_get_headers_bind_host_date_and_query_target() {
        let private_key = fixture_private_key();
        let signer = HttpSignatureSigner {
            key_id: "https://local.example/actor#main-key",
            private_key_pem: &private_key,
        };
        let headers = signed_get_headers(
            &Url::parse(
                "https://remote.example:8443/users/alice?resource=acct%3AAlice%40remote.example",
            )
            .unwrap(),
            &signer,
        )
        .unwrap();
        assert_eq!(headers.get(HOST).unwrap(), "remote.example:8443");
        assert!(headers.get(DATE).is_some());
        assert!(headers.get("signature").is_some());
    }

    #[test]
    fn signed_post_headers_bind_host_date_digest_and_target() {
        let private_key = fixture_private_key();
        let signer = HttpSignatureSigner {
            key_id: "https://local.example/actor#main-key",
            private_key_pem: &private_key,
        };
        let body = br#"{"id":"https://local.example/activities/1"}"#;
        let headers = signed_post_headers(
            &Url::parse("https://remote.example/inbox?delivery=1").unwrap(),
            body,
            &signer,
        )
        .unwrap();
        assert_eq!(headers.get(HOST).unwrap(), "remote.example");
        assert_eq!(
            headers.get("digest").unwrap().to_str().unwrap(),
            body_digest_header(body)
        );
        assert!(headers.get(DATE).is_some());
        assert!(headers.get("signature").is_some());
    }

    #[test]
    fn remote_response_origins_cannot_cross_the_requested_domain() {
        let same_origin = Url::parse("https://remote.example/users/alice").unwrap();
        let other_origin = Url::parse("https://other.example/users/alice").unwrap();
        let other_port = Url::parse("https://remote.example:8443/users/alice").unwrap();
        assert!(ensure_remote_origin(&same_origin, "remote.example").is_ok());
        assert!(matches!(
            ensure_remote_origin(&other_origin, "remote.example"),
            Err(RemoteFetchError::OriginMismatch)
        ));
        assert!(matches!(
            ensure_remote_origin(&other_port, "remote.example"),
            Err(RemoteFetchError::OriginMismatch)
        ));
    }

    #[test]
    fn remote_actor_profile_fields_are_bounded_before_persistence() {
        let actor_url = Url::parse("https://remote.example/users/alice").unwrap();
        let body = serde_json::to_vec(&serde_json::json!({
            "id": actor_url.as_str(),
            "type": "Person",
            "@context": "https://www.w3.org/ns/activitystreams",
            "preferredUsername": "Alice",
            "name": "n".repeat(MAX_REMOTE_DISPLAY_NAME_CHARS + 1),
            "summary": "s".repeat(MAX_REMOTE_NOTE_CHARS + 1),
            "inbox": "https://remote.example/users/alice/inbox"
        }))
        .unwrap();
        let actor = parse_remote_actor(&body, &actor_url, "Alice", "remote.example").unwrap();
        assert_eq!(
            actor.display_name.chars().count(),
            MAX_REMOTE_DISPLAY_NAME_CHARS
        );
        assert_eq!(actor.note.chars().count(), MAX_REMOTE_NOTE_CHARS);
    }

    #[test]
    fn optional_actor_urls_accept_null_empty_arrays_and_href_links() {
        let null = serde_json::json!({"url": null});
        let empty = serde_json::json!({"url": []});
        let href = serde_json::json!({
            "url": {"href": "https://remote.example/@alice"}
        });
        assert!(optional_remote_url(&null, "url").unwrap().is_none());
        assert!(optional_remote_url(&empty, "url").unwrap().is_none());
        assert_eq!(
            optional_remote_url(&href, "url").unwrap().unwrap().as_str(),
            "https://remote.example/@alice"
        );
    }

    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn profile_fetch_checks_every_redirect_before_contact_and_retains_history() {
        let redirect = b"HTTP/1.1 302 Found\r\nLocation: http://cdn.fixture.invalid/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
        let (endpoint, server) = local_http_responses(vec![redirect.clone()]);
        let fetcher =
            RemoteFetcher::new(RemoteFetchLimits::default()).with_test_endpoint(Some(endpoint));
        let result = fetcher
            .get_with_policy(
                Url::parse("http://images.fixture.invalid/start").unwrap(),
                |url| async move {
                    if url.host_str() == Some("cdn.fixture.invalid") {
                        Err(RemoteFetchError::PolicyDenied)
                    } else {
                        Ok(())
                    }
                },
            )
            .await;
        assert!(matches!(result, Err(RemoteFetchError::PolicyDenied)));
        server.join().unwrap();
        let (endpoint, server) = local_http_responses(vec![redirect,
            b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 3\r\nConnection: close\r\n\r\nPNG".to_vec()]);
        let fetcher =
            RemoteFetcher::new(RemoteFetchLimits::default()).with_test_endpoint(Some(endpoint));
        let (response, visited) = fetcher
            .get_with_policy(
                Url::parse("http://images.fixture.invalid/start").unwrap(),
                |_| async { Ok(()) },
            )
            .await
            .unwrap();
        assert_eq!(response.body, b"PNG");
        assert_eq!(
            visited.iter().filter_map(Url::host_str).collect::<Vec<_>>(),
            ["images.fixture.invalid", "cdn.fixture.invalid"]
        );
        server.join().unwrap();
    }

    #[test]
    fn actor_profile_images_preserve_presence_and_image_url() {
        let url = Url::parse("https://remote.example/users/alice").unwrap();
        let mut document = serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": url.as_str(), "type": "Person", "preferredUsername": "alice",
            "inbox": "https://remote.example/inbox"
        });
        let parse = |doc: &Value| {
            parse_remote_actor(
                &serde_json::to_vec(doc).unwrap(),
                &url,
                "alice",
                "remote.example",
            )
        };
        let actor = parse(&document).unwrap();
        assert_eq!(actor.avatar, None);
        assert_eq!(actor.header, None);
        document["icon"] = serde_json::json!({"type": "Image", "id": "https://remote.example/image", "url": "https://cdn.example/avatar.png"});
        document["image"] = Value::Null;
        let actor = parse(&document).unwrap();
        assert_eq!(
            actor.avatar,
            Some(Some("https://cdn.example/avatar.png".into()))
        );
        assert_eq!(actor.header, Some(None));
        document["image"] =
            serde_json::json!({"type": "Image", "url": "https://cdn.example/banner.blob"});
        assert_eq!(
            parse(&document).unwrap().header,
            Some(Some("https://cdn.example/banner.blob".into()))
        );
        document["icon"] = serde_json::json!({"url": "file:///tmp/avatar"});
        assert!(parse(&document).is_err());
    }

    #[test]
    fn actor_documents_require_canonical_identity_and_supported_fields() {
        let actor_url = Url::parse("https://remote.example/users/alice").unwrap();
        let actor = parse_remote_actor(
            br#"{
                "id": "https://remote.example/users/alice",
                "type": ["Person", "Actor"],
                "@context": "https://www.w3.org/ns/activitystreams",
                "webfinger": "acct:Alice@remote.example",
                "preferredUsername": "Alice",
                "name": "Alice Example",
                "url": "https://remote.example/@alice",
                "inbox": "https://remote.example/users/alice/inbox",
                "followers": "https://remote.example/collections/alice-subscribers",
                "following": {"id": "https://remote.example/collections/alice-subscriptions"},
                "endpoints": {"sharedInbox": "https://remote.example/inbox"},
                "publicKey": {
                    "id": "https://remote.example/users/alice#main-key",
                    "owner": "https://remote.example/users/alice",
                    "publicKeyPem": "-----BEGIN PUBLIC KEY-----"
                }
            }"#,
            &actor_url,
            "Alice",
            "remote.example",
        )
        .unwrap();

        assert_eq!(actor.username, "Alice");
        assert_eq!(actor.actor_type, "Person");
        assert_eq!(
            actor.profile_url.unwrap().as_str(),
            "https://remote.example/@alice"
        );
        assert_eq!(
            actor.inbox.as_str(),
            "https://remote.example/users/alice/inbox"
        );
        assert_eq!(
            actor.shared_inbox.unwrap().as_str(),
            "https://remote.example/inbox"
        );
        assert_eq!(
            actor.followers.unwrap().as_str(),
            "https://remote.example/collections/alice-subscribers"
        );
        assert_eq!(
            actor.following.unwrap().as_str(),
            "https://remote.example/collections/alice-subscriptions"
        );
        assert_eq!(actor.public_keys.len(), 1);
        assert!(actor.key_set_complete);
        assert_eq!(
            actor.public_keys[0].id.as_str(),
            "https://remote.example/users/alice#main-key"
        );
        assert!(matches!(
            parse_remote_actor(
                br#"{"id":"https://other.example/users/alice","type":"Person","@context":"https://www.w3.org/ns/activitystreams","preferredUsername":"Alice","inbox":"https://other.example/inbox"}"#,
                &actor_url,
                "Alice",
                "remote.example"
            ),
            Err(RemoteFetchError::IdentityMismatch)
        ));
        assert!(matches!(
            parse_remote_actor(
                br#"{"id":"https://other.example/users/alice","type":"Person","@context":"https://www.w3.org/ns/activitystreams","preferredUsername":"Alice","inbox":"https://other.example/inbox"}"#,
                &Url::parse("https://other.example/users/alice").unwrap(),
                "Alice",
                "remote.example"
            ),
            Err(RemoteFetchError::OriginMismatch)
        ));
    }

    #[test]
    fn actor_documents_preserve_http_non_default_ports() {
        let actor_url = Url::parse("http://remote.example:443/users/alice").unwrap();
        let actor = parse_remote_actor(
            br#"{
                "id": "http://remote.example:443/users/alice",
                "type": "Person",
                "@context": "https://www.w3.org/ns/activitystreams",
                "preferredUsername": "Alice",
                "inbox": "http://remote.example:443/users/alice/inbox"
            }"#,
            &actor_url,
            "Alice",
            "remote.example:443",
        )
        .unwrap();
        assert_eq!(actor.id, actor_url);
    }

    #[test]
    fn actor_key_references_are_kept_for_bounded_fetching() {
        let actor_url = Url::parse("https://remote.example/users/alice").unwrap();
        let body = br#"{
            "id": "https://remote.example/users/alice",
            "type": "Person",
            "@context": "https://www.w3.org/ns/activitystreams",
            "preferredUsername": "Alice",
            "inbox": "https://remote.example/users/alice/inbox",
            "publicKey": "https://remote.example/keys/alice/main-key"
        }"#;

        let (actor, references) =
            parse_remote_actor_document(body, &actor_url, "Alice", "remote.example").unwrap();
        assert!(actor.public_keys.is_empty());
        assert!(!actor.key_set_complete);
        assert_eq!(
            references,
            [Url::parse("https://remote.example/keys/alice/main-key").unwrap()]
        );
    }

    #[test]
    fn remote_key_documents_bind_owner_and_requested_key_id() {
        let actor_url = Url::parse("https://remote.example/users/alice").unwrap();
        let key_id = Url::parse("https://remote.example/keys/alice/main-key").unwrap();
        let key = parse_remote_key_document(
            br#"{
                "@context": "https://w3id.org/security/v1",
                "id": "https://remote.example/keys/alice/main-key",
                "owner": "https://remote.example/users/alice",
                "publicKeyPem": "-----BEGIN PUBLIC KEY-----"
            }"#,
            &key_id,
            &actor_url,
        )
        .unwrap();
        assert_eq!(key.id, key_id);
        assert_eq!(key.owner, actor_url);

        assert!(matches!(
            parse_remote_key_document(
                br#"{
                    "@context": "https://w3id.org/security/v1",
                    "id": "https://remote.example/keys/alice/main-key",
                    "owner": "https://other.example/users/alice",
                    "publicKeyPem": "-----BEGIN PUBLIC KEY-----"
                }"#,
                &key_id,
                &actor_url,
            ),
            Err(RemoteFetchError::IdentityMismatch)
        ));
    }
}
