use std::error::Error;

use reqwest::Method;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderValue};
use serde_json::Value;
use url::Url;

use super::super::stable_request_headers;
use super::comparison::CapturedResponse;
use super::harness::RequestSpec;
use super::read_only::ReadOnlyGuard;
use super::safety::DifferentialConfig;

const ACTIVITY_ACCEPT: &str = "application/activity+json";

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_federation_discovery_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    let guard = ReadOnlyGuard::begin(config, rust_url).await?;
    let activity_headers = || {
        let mut headers = stable_request_headers();
        headers.insert(ACCEPT, HeaderValue::from_static(ACTIVITY_ACCEPT));
        headers
    };
    let json_headers = || {
        let mut headers = stable_request_headers();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers
    };
    let html_headers = || {
        let mut headers = stable_request_headers();
        headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
        headers
    };
    let xml_headers = || {
        let mut headers = stable_request_headers();
        headers.insert(ACCEPT, HeaderValue::from_static("application/xrd+xml"));
        headers
    };
    let requests = [
        (
            "webfinger missing resource",
            RequestSpec::new(
                Method::GET,
                "/.well-known/webfinger",
                None,
                json_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "webfinger malformed resource",
            RequestSpec::new(
                Method::GET,
                "/.well-known/webfinger",
                Some("resource=acct%3Aalice".to_owned()),
                json_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "webfinger unknown account",
            RequestSpec::new(
                Method::GET,
                "/.well-known/webfinger",
                Some("resource=acct:missing@fixture-v4-6-5.rustodon.invalid".to_owned()),
                json_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "webfinger alice",
            RequestSpec::new(
                Method::GET,
                "/.well-known/webfinger",
                Some("resource=acct:alice@fixture-v4-6-5.rustodon.invalid".to_owned()),
                json_headers(),
                Vec::new(),
            )?,
            &["/subject", "/aliases", "/links"][..],
        ),
        (
            "webfinger alice without acct prefix",
            RequestSpec::new(
                Method::GET,
                "/.well-known/webfinger",
                Some("resource=alice@fixture-v4-6-5.rustodon.invalid".to_owned()),
                json_headers(),
                Vec::new(),
            )?,
            &["/subject", "/aliases", "/links"][..],
        ),
        (
            "webfinger alice URL",
            RequestSpec::new(
                Method::GET,
                "/.well-known/webfinger",
                Some(
                    "resource=https%3A%2F%2Ffixture-v4-6-5.rustodon.invalid%2Fusers%2Falice"
                        .to_owned(),
                ),
                json_headers(),
                Vec::new(),
            )?,
            &["/subject", "/aliases", "/links"][..],
        ),
        (
            "webfinger alice short URL",
            RequestSpec::new(
                Method::GET,
                "/.well-known/webfinger",
                Some(
                    "resource=https%3A%2F%2Ffixture-v4-6-5.rustodon.invalid%2F%40alice".to_owned(),
                ),
                json_headers(),
                Vec::new(),
            )?,
            &["/subject", "/aliases", "/links"][..],
        ),
        (
            "webfinger instance actor",
            RequestSpec::new(
                Method::GET,
                "/.well-known/webfinger",
                Some(
                    "resource=acct:fixture-v4-6-5.rustodon.invalid@fixture-v4-6-5.rustodon.invalid"
                        .to_owned(),
                ),
                json_headers(),
                Vec::new(),
            )?,
            &["/subject", "/aliases", "/links"][..],
        ),
        (
            "host-meta",
            RequestSpec::new(
                Method::GET,
                "/.well-known/host-meta",
                None,
                stable_request_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "host-meta json",
            RequestSpec::new(
                Method::GET,
                "/.well-known/host-meta.json",
                None,
                json_headers(),
                Vec::new(),
            )?,
            &["/links/0/rel", "/links/0/template"][..],
        ),
        (
            "host-meta xml",
            RequestSpec::new(
                Method::GET,
                "/.well-known/host-meta",
                None,
                xml_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "nodeinfo discovery",
            RequestSpec::new(
                Method::GET,
                "/.well-known/nodeinfo",
                None,
                json_headers(),
                Vec::new(),
            )?,
            &["/links/0/rel", "/links/0/href"][..],
        ),
        (
            "nodeinfo schema",
            RequestSpec::new(
                Method::GET,
                "/nodeinfo/2.0",
                None,
                json_headers(),
                Vec::new(),
            )?,
            &[
                "/version",
                "/software/name",
                "/software/version",
                "/protocols/0",
                "/services",
                "/usage",
                "/openRegistrations",
                "/metadata",
            ][..],
        ),
        (
            "alice actor",
            RequestSpec::new(
                Method::GET,
                "/users/alice",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[
                "/@context/0",
                "/@context/1",
                "/@context/2",
                "/@context/3",
                "/id",
                "/type",
                "/preferredUsername",
                "/name",
                "/summary",
                "/published",
                "/inbox",
                "/outbox",
                "/followers",
                "/following",
                "/url",
                "/publicKey/id",
                "/publicKey/owner",
                "/endpoints/sharedInbox",
            ][..],
        ),
        (
            "alice actor application json",
            RequestSpec::new(
                Method::GET,
                "/users/alice",
                None,
                json_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/preferredUsername"][..],
        ),
        (
            "alice numeric actor route",
            RequestSpec::new(
                Method::GET,
                "/ap/users/116844606259201001",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/preferredUsername", "/inbox", "/outbox"][..],
        ),
        (
            "malformed numeric actor route",
            RequestSpec::new(
                Method::GET,
                "/ap/users/116844606259201001junk",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "remote numeric actor route",
            RequestSpec::new(
                Method::GET,
                "/ap/users/116844606259202001",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "pending local actor",
            RequestSpec::new(
                Method::GET,
                "/users/pending",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "unconfirmed local actor",
            RequestSpec::new(
                Method::GET,
                "/users/unconfirmed",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "public note",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[
                "/@context/0",
                "/@context/1",
                "/id",
                "/type",
                "/published",
                "/url",
                "/attributedTo",
                "/to",
                "/cc",
                "/sensitive",
                "/atomUri",
                "/content",
                "/attachment/0/url",
                "/attachment/1/url",
                "/quote",
                "/quoteUri",
                "/replies",
                "/context",
                "/tag",
            ][..],
        ),
        (
            "public note encoded trailing id",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001%3Fjunk",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "alice outbox",
            RequestSpec::new(
                Method::GET,
                "/users/alice/outbox",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/totalItems", "/first", "/last"][..],
        ),
        (
            "alice outbox page",
            RequestSpec::new(
                Method::GET,
                "/users/alice/outbox",
                Some("page=true".to_owned()),
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/partOf", "/orderedItems/0/type"][..],
        ),
        (
            "alice outbox max cursor",
            RequestSpec::new(
                Method::GET,
                "/users/alice/outbox",
                Some("page=true&max_id=116844842188805001".to_owned()),
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/partOf"][..],
        ),
        (
            "alice outbox min cursor",
            RequestSpec::new(
                Method::GET,
                "/users/alice/outbox",
                Some("page=true&min_id=0".to_owned()),
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/partOf", "/orderedItems/0/id"][..],
        ),
        (
            "alice followers",
            RequestSpec::new(
                Method::GET,
                "/users/alice/followers",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/totalItems", "/first"][..],
        ),
        (
            "alice following",
            RequestSpec::new(
                Method::GET,
                "/users/alice/following",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/totalItems", "/first"][..],
        ),
        (
            "alice followers page one",
            RequestSpec::new(
                Method::GET,
                "/users/alice/followers",
                Some("page=1".to_owned()),
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/partOf", "/totalItems", "/orderedItems/0"][..],
        ),
        (
            "alice following page one",
            RequestSpec::new(
                Method::GET,
                "/users/alice/following",
                Some("page=1".to_owned()),
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/partOf", "/totalItems", "/orderedItems/0"][..],
        ),
    ];

    for (label, request, paths) in requests {
        let responses = guard.send(&request).await?;
        if responses.mastodon.status != responses.rust.status {
            return Err(format!(
                "{label}: status mismatch: Mastodon={} body={:?}, Rust={} body={:?}",
                responses.mastodon.status,
                String::from_utf8_lossy(&responses.mastodon.body),
                responses.rust.status,
                String::from_utf8_lossy(&responses.rust.body)
            )
            .into());
        }
        if responses.rust.status == 200 {
            let expected_content_type = if label.starts_with("webfinger") {
                "application/jrd+json"
            } else if label == "host-meta xml" {
                "application/xrd+xml"
            } else if label == "host-meta"
                || label == "host-meta json"
                || label.starts_with("nodeinfo")
            {
                "application/json"
            } else {
                "application/activity+json"
            };
            for (side, response) in [("Mastodon", &responses.mastodon), ("Rust", &responses.rust)] {
                let content_type = response
                    .headers
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default();
                if !content_type.starts_with(expected_content_type) {
                    return Err(format!(
                        "{label}: {side} content type {content_type:?} does not start with {expected_content_type:?}"
                    )
                    .into());
                }
            }
        }
        if responses.rust.status == 200 && !paths.is_empty() {
            compare_json_paths(label, &responses.mastodon, &responses.rust, paths)?;
        }
    }

    for (label, path, headers) in [
        ("HTML account", "/users/alice", html_headers()),
        (
            "HTML status",
            "/users/alice/statuses/116844842188805001",
            html_headers(),
        ),
        ("HTML followers", "/users/alice/followers", html_headers()),
    ] {
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        if responses.mastodon.status != responses.rust.status
            || responses.mastodon.headers.get("location") != responses.rust.headers.get("location")
        {
            return Err(format!(
                "{label} redirect {path} differs: Mastodon={} {:?}, Rust={} {:?}",
                responses.mastodon.status,
                responses.mastodon.headers.get("location"),
                responses.rust.status,
                responses.rust.headers.get("location")
            )
            .into());
        }
    }

    guard.finish().await
}

fn compare_json_paths(
    label: &str,
    mastodon: &CapturedResponse,
    rust: &CapturedResponse,
    paths: &[&str],
) -> Result<(), Box<dyn Error>> {
    let mastodon_json: Value = serde_json::from_slice(&mastodon.body)
        .map_err(|error| format!("{label}: Mastodon returned invalid JSON: {error}"))?;
    let rust_json: Value = serde_json::from_slice(&rust.body)
        .map_err(|error| format!("{label}: Rust returned invalid JSON: {error}"))?;
    for path in paths {
        if mastodon_json.pointer(path) != rust_json.pointer(path) {
            return Err(format!(
                "{label}: JSON {path} mismatch: Mastodon={}, Rust={}",
                mastodon_json
                    .pointer(path)
                    .map_or("<missing>".to_owned(), Value::to_string),
                rust_json
                    .pointer(path)
                    .map_or("<missing>".to_owned(), Value::to_string),
            )
            .into());
        }
    }
    Ok(())
}
