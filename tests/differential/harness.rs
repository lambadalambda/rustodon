use std::fmt;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HOST, HeaderMap};
use reqwest::{Client, Method, redirect::Policy};
use url::Url;

use super::comparison::CapturedResponse;
use super::safety::HttpTargets;

pub(crate) const MAX_RESPONSE_BODY_BYTES: usize = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct RequestSpec {
    method: Method,
    path: String,
    query: Option<String>,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl fmt::Debug for RequestSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers = self
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str(),
                    if name == AUTHORIZATION {
                        "[REDACTED]".to_owned()
                    } else {
                        value.to_str().unwrap_or("[NON-UTF8]").to_owned()
                    },
                )
            })
            .collect::<Vec<_>>();
        formatter
            .debug_struct("RequestSpec")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &self.query)
            .field("headers", &headers)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

impl RequestSpec {
    pub(crate) fn new(
        method: Method,
        path: impl Into<String>,
        query: Option<String>,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> Result<Self, HarnessError> {
        let path = path.into();
        if !path.starts_with('/') || path.starts_with("//") || path.contains(['?', '#']) {
            return Err(HarnessError::InvalidRequest(
                "path must be an absolute origin path without query or fragment".to_owned(),
            ));
        }
        if query.as_deref().is_some_and(|query| query.starts_with('?')) {
            return Err(HarnessError::InvalidRequest(
                "query must not include a leading question mark".to_owned(),
            ));
        }
        let host_values = headers.get_all(HOST).iter().collect::<Vec<_>>();
        if !matches!(host_values.as_slice(), [value] if !value.is_empty()) {
            return Err(HarnessError::InvalidRequest(
                "exactly one nonempty Host header is required so both targets receive the same authority"
                    .to_owned(),
            ));
        }
        Ok(Self {
            method,
            path,
            query,
            headers,
            body,
        })
    }

    pub(crate) fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResponsePair {
    pub(crate) mastodon: CapturedResponse,
    pub(crate) rust: CapturedResponse,
}

#[derive(Debug)]
pub(crate) enum HarnessError {
    InvalidRequest(String),
    Client(reqwest::Error),
    Request {
        side: &'static str,
        source: reqwest::Error,
    },
    ResponseBodyTooLarge {
        side: &'static str,
        limit: usize,
    },
}

impl fmt::Display for HarnessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(formatter, "invalid request spec: {message}"),
            Self::Client(error) => {
                write!(formatter, "could not build differential client: {error}")
            }
            Self::Request { side, source } => {
                write!(formatter, "{side} HTTP request failed: {source}")
            }
            Self::ResponseBodyTooLarge { side, limit } => {
                write!(formatter, "{side} response body exceeds {limit} bytes")
            }
        }
    }
}

impl std::error::Error for HarnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::Request { source, .. } => Some(source),
            Self::InvalidRequest(_) | Self::ResponseBodyTooLarge { .. } => None,
        }
    }
}

pub(crate) async fn send_identically(
    targets: &HttpTargets,
    request: &RequestSpec,
) -> Result<ResponsePair, HarnessError> {
    // reqwest has no compression implementations when built without its default
    // features; the builder also forbids redirects and bounds every wait.
    let client = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .pool_max_idle_per_host(0)
        .build()
        .map_err(HarnessError::Client)?;

    let mastodon_request = build_request(&client, targets.mastodon(), request)?;
    let rust_request = build_request(&client, targets.rust(), request)?;
    let (mastodon, rust) = tokio::join!(
        send_one(&client, mastodon_request, "Mastodon"),
        send_one(&client, rust_request, "Rust")
    );
    Ok(ResponsePair {
        mastodon: mastodon?,
        rust: rust?,
    })
}

pub(crate) async fn send_single(
    target: &Url,
    request: &RequestSpec,
    side: &'static str,
) -> Result<CapturedResponse, HarnessError> {
    let client = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .pool_max_idle_per_host(0)
        .build()
        .map_err(HarnessError::Client)?;
    let target_request = build_request(&client, target, request)?;
    send_one(&client, target_request, side).await
}

fn build_request(
    client: &Client,
    target: &Url,
    request: &RequestSpec,
) -> Result<reqwest::Request, HarnessError> {
    let mut url = format!("{}{}", target.as_str().trim_end_matches('/'), request.path);
    if let Some(query) = &request.query {
        url.push('?');
        url.push_str(query);
    }
    client
        .request(request.method.clone(), url)
        .headers(request.headers.clone())
        .body(request.body.clone())
        .build()
        .map_err(HarnessError::Client)
}

async fn send_one(
    client: &Client,
    request: reqwest::Request,
    side: &'static str,
) -> Result<CapturedResponse, HarnessError> {
    let mut response = client
        .execute(request)
        .await
        .map_err(|source| HarnessError::Request { side, source })?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| HarnessError::Request { side, source })?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BODY_BYTES {
            return Err(HarnessError::ResponseBodyTooLarge {
                side,
                limit: MAX_RESPONSE_BODY_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(CapturedResponse {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap as AxumHeaderMap, Method as AxumMethod, Uri};
    use axum::response::Response;
    use axum::routing::any;
    use reqwest::header::{CONTENT_TYPE, HeaderValue};
    use tokio::sync::oneshot;

    use super::super::comparison::{DEFAULT_MISMATCH_LIMIT, compare_responses};
    use super::super::safety::HttpTargets;
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct ReceivedRequest {
        method: String,
        path_and_query: String,
        host: Option<Vec<u8>>,
        probe_header: Option<Vec<u8>>,
        content_type: Option<Vec<u8>>,
        body: Vec<u8>,
    }

    #[derive(Clone)]
    struct ServerState {
        received: Arc<Mutex<Vec<ReceivedRequest>>>,
        response_body: Arc<Vec<u8>>,
    }

    async fn record_request(
        State(state): State<ServerState>,
        method: AxumMethod,
        uri: Uri,
        headers: AxumHeaderMap,
        body: Bytes,
    ) -> Response {
        state
            .received
            .lock()
            .expect("recording mutex should not be poisoned")
            .push(ReceivedRequest {
                method: method.to_string(),
                path_and_query: uri
                    .path_and_query()
                    .expect("an origin request has a path")
                    .to_string(),
                host: headers.get(HOST).map(|value| value.as_bytes().to_vec()),
                probe_header: headers
                    .get("x-probe")
                    .map(|value| value.as_bytes().to_vec()),
                content_type: headers
                    .get(CONTENT_TYPE)
                    .map(|value| value.as_bytes().to_vec()),
                body: body.to_vec(),
            });
        Response::builder()
            .status(418)
            .header(CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(state.response_body.as_ref().clone()))
            .expect("test response should be valid")
    }

    async fn server(
        body: Vec<u8>,
    ) -> (
        Url,
        Arc<Mutex<Vec<ReceivedRequest>>>,
        oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener has an address");
        let received = Arc::new(Mutex::new(Vec::new()));
        let state = ServerState {
            received: Arc::clone(&received),
            response_body: Arc::new(body),
        };
        let app = Router::new()
            .fallback(any(record_request))
            .with_state(state);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("test server should run");
        });
        (
            Url::parse(&format!("http://{address}")).expect("listener address should form a URL"),
            received,
            shutdown_tx,
            task,
        )
    }

    #[tokio::test]
    async fn both_servers_receive_the_identical_request_and_non_success_is_retained() {
        let (mastodon_url, mastodon_received, mastodon_shutdown, mastodon_task) =
            server(br#"{"z":1,"a":2}"#.to_vec()).await;
        let (rust_url, rust_received, rust_shutdown, rust_task) =
            server(br#"{"a":2,"z":1}"#.to_vec()).await;
        let targets = HttpTargets::new(mastodon_url.as_str(), rust_url.as_str())
            .expect("loopback test targets are safe");
        let mut headers = HeaderMap::new();
        headers.insert("x-probe", HeaderValue::from_static("same-value"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(HOST, HeaderValue::from_static("fixture.invalid"));
        let request = RequestSpec::new(
            Method::POST,
            "/api/test",
            Some("first=1&first=2&encoded=%2F".to_owned()),
            headers,
            br#"{"large":123456789012345678901234567890}"#.to_vec(),
        )
        .expect("test request is valid");

        let responses = send_identically(&targets, &request)
            .await
            .expect("both requests should complete");
        assert_eq!(responses.mastodon.status, 418);
        assert_eq!(responses.rust.status, 418);
        assert!(
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[CONTENT_TYPE],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .is_ok()
        );

        let mastodon_request = mastodon_received
            .lock()
            .expect("recording mutex should not be poisoned")[0]
            .clone();
        let rust_request = rust_received
            .lock()
            .expect("recording mutex should not be poisoned")[0]
            .clone();
        assert_eq!(mastodon_request, rust_request);
        assert_eq!(mastodon_request.method, "POST");
        assert_eq!(
            mastodon_request.host.as_deref(),
            Some(b"fixture.invalid".as_slice())
        );
        assert_eq!(
            mastodon_request.path_and_query,
            "/api/test?first=1&first=2&encoded=%2F"
        );

        mastodon_shutdown.send(()).expect("server is still running");
        rust_shutdown.send(()).expect("server is still running");
        mastodon_task.await.expect("server task should stop");
        rust_task.await.expect("server task should stop");
    }

    #[tokio::test]
    async fn oversized_response_body_is_rejected() {
        let body = vec![b'x'; MAX_RESPONSE_BODY_BYTES + 1];
        let (mastodon_url, _, mastodon_shutdown, mastodon_task) = server(body.clone()).await;
        let (rust_url, _, rust_shutdown, rust_task) = server(body).await;
        let targets = HttpTargets::new(mastodon_url.as_str(), rust_url.as_str())
            .expect("loopback test targets are safe");
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("fixture.invalid"));
        let request = RequestSpec::new(Method::GET, "/", None, headers, Vec::new())
            .expect("test request is valid");

        let error = send_identically(&targets, &request)
            .await
            .expect_err("oversized responses must fail");
        assert!(matches!(error, HarnessError::ResponseBodyTooLarge { .. }));

        mastodon_shutdown.send(()).expect("server is still running");
        rust_shutdown.send(()).expect("server is still running");
        mastodon_task.await.expect("server task should stop");
        rust_task.await.expect("server task should stop");
    }

    #[test]
    fn requests_require_one_explicit_stable_host() {
        let error = RequestSpec::new(Method::GET, "/", None, HeaderMap::new(), Vec::new())
            .expect_err("transport-derived Host values would differ by target");
        assert!(error.to_string().contains("Host"));
    }

    #[test]
    fn request_diagnostics_redact_authorization_and_body() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("fixture.invalid"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-sensitive-token"),
        );
        headers.insert("x-probe", HeaderValue::from_static("visible"));
        let request = RequestSpec::new(
            Method::POST,
            "/api/test",
            None,
            headers,
            b"fixture-sensitive-body".to_vec(),
        )
        .expect("test request is valid");

        let diagnostic = format!("{request:?}");
        assert!(diagnostic.contains("[REDACTED]"));
        assert!(diagnostic.contains("visible"));
        assert!(!diagnostic.contains("fixture-sensitive-token"));
        assert!(!diagnostic.contains("fixture-sensitive-body"));
    }
}
