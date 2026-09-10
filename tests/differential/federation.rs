use std::error::Error;
use std::time::SystemTime;

use http::Method as HttpMethod;
use reqwest::Method;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use serde_json::Value;
use url::Url;

use super::super::stable_request_headers;
use super::comparison::CapturedResponse;
use super::harness::RequestSpec;
use super::read_only::ReadOnlyGuard;
use super::safety::DifferentialConfig;
use rustodon::mastodon::{
    HttpSignatureRequest, HttpSignatureSigner, sign_http_signature_with_headers,
};

const ACTIVITY_ACCEPT: &str = "application/activity+json";
const FIXTURE_SEED: &str = include_str!("../../fixtures/mastodon/v4.6.5/seed.sql");

fn fixture_literal(delimiter: &str) -> String {
    let (_, remainder) = FIXTURE_SEED
        .split_once(delimiter)
        .expect("fixture key delimiter should be present");
    let (value, _) = remainder
        .split_once(delimiter)
        .expect("fixture key delimiter should be paired");
    value.to_owned()
}

fn signed_actor_headers() -> reqwest::header::HeaderMap {
    signed_actor_headers_for("/users/alice")
}

fn signed_actor_headers_for(path: &str) -> reqwest::header::HeaderMap {
    signed_actor_headers_with_key(
        path,
        "https://fixture-v4-6-5.rustodon.invalid/users/alice#main-key",
    )
}

fn signed_remote_actor_headers_for(path: &str) -> reqwest::header::HeaderMap {
    signed_actor_headers_with_key(
        path,
        "https://remote.fixture.invalid/users/bob#secondary-key",
    )
}

fn signed_actor_headers_with_key(path: &str, key_id: &str) -> reqwest::header::HeaderMap {
    let mut headers = super::super::stable_request_headers();
    let date = httpdate::fmt_http_date(SystemTime::now());
    headers.insert(
        "Date",
        HeaderValue::from_str(&date).expect("HTTP date is a valid header"),
    );
    let request = HttpSignatureRequest::new(&HttpMethod::GET, path, &headers, &[]);
    let private_key = fixture_literal("$fixture_private$");
    let signature = sign_http_signature_with_headers(
        &request,
        &HttpSignatureSigner {
            key_id,
            private_key_pem: &private_key,
        },
        &["date", "host", "(request-target)"],
    )
    .expect("fixture actor request should sign");
    headers.insert(
        "Signature",
        HeaderValue::from_str(&signature).expect("signature is a valid header"),
    );
    headers
}

fn invalid_signature_headers_for(path: &str) -> reqwest::header::HeaderMap {
    let mut headers = signed_actor_headers_for(path);
    headers.insert(
        "Signature",
        HeaderValue::from_static(
            r#"keyId="https://fixture-v4-6-5.rustodon.invalid/users/alice#main-key",headers="date host (request-target)",signature="AAAA""#,
        ),
    );
    headers
}

fn response_header(response: &CapturedResponse, name: &str) -> Option<String> {
    response
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn response_header_tokens(response: &CapturedResponse, name: &str) -> Vec<String> {
    response_header(response, name)
        .unwrap_or_default()
        .split(',')
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn compare_activitypub_cache_headers(
    label: &str,
    mastodon: &CapturedResponse,
    rust: &CapturedResponse,
) -> Result<(), String> {
    let mastodon_vary = response_header_tokens(mastodon, "vary");
    let rust_vary = response_header_tokens(rust, "vary");
    let missing = mastodon_vary
        .iter()
        .filter(|token| !rust_vary.iter().any(|actual| actual == *token))
        .cloned()
        .collect::<Vec<_>>();
    let unexpected = rust_vary
        .iter()
        .filter(|token| {
            !mastodon_vary.iter().any(|expected| expected == *token)
                && !matches!(token.as_str(), "authorization" | "signature")
        })
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() || !unexpected.is_empty() {
        return Err(format!(
            "{label}: vary header mismatch: Mastodon={:?}, Rust={:?}, missing={missing:?}, unexpected={unexpected:?}",
            response_header(mastodon, "vary"),
            response_header(rust, "vary")
        ));
    }

    let mastodon_cache = response_header(mastodon, "cache-control");
    let rust_cache = response_header(rust, "cache-control");
    let security_override = rust_cache.as_deref() == Some("private, no-store")
        && matches!(
            mastodon_cache.as_deref(),
            Some("max-age=180, public" | "max-age=5, public")
        );
    if mastodon_cache != rust_cache && !security_override {
        return Err(format!(
            "{label}: cache-control header mismatch: Mastodon={mastodon_cache:?}, Rust={rust_cache:?}"
        ));
    }
    Ok(())
}

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
    let bearer_activity_headers = || {
        let mut headers = activity_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
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
            "webfinger alice mixed-case URL",
            RequestSpec::new(
                Method::GET,
                "/.well-known/webfinger",
                Some(
                    "resource=HTTp%3A%2F%2Ffixture-v4-6-5.rustodon.invalid%2Fusers%2Falice"
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
                "/icon",
                "/image",
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
            "alice actor signed GET",
            RequestSpec::new(
                Method::GET,
                "/users/alice",
                None,
                signed_actor_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/publicKey/id"][..],
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
                "/interactionPolicy/canQuote/automaticApproval/0",
                "/atomUri",
                "/inReplyTo",
                "/inReplyToAtomUri",
                "/content",
                "/attachment/0/url",
                "/attachment/0/width",
                "/attachment/0/height",
                "/attachment/1/url",
                "/quote",
                "/quoteUri",
                "/conversation",
                "/context",
                "/replies",
                "/replies/first/items/0",
                "/likes/totalItems",
                "/shares/totalItems",
                "/context",
                "/tag",
            ][..],
        ),
        (
            "public note signed",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001",
                None,
                signed_actor_headers_for("/users/alice/statuses/116844842188805001"),
                Vec::new(),
            )?,
            &[
                "/id",
                "/type",
                "/attributedTo",
                "/inReplyTo",
                "/inReplyToAtomUri",
                "/conversation",
                "/context",
                "/content",
            ][..],
        ),
        (
            "local quoted note",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116845314048005201",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[
                "/id",
                "/type",
                "/quote",
                "/quoteUri",
                "/_misskey_quote",
                "/quoteAuthorization",
            ][..],
        ),
        (
            "local quote authorization",
            RequestSpec::new(
                Method::GET,
                "/users/alice/quote_authorizations/116845317980168701",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[
                "/@context/0",
                "/@context/1",
                "/id",
                "/type",
                "/attributedTo",
                "/interactingObject",
                "/interactionTarget",
            ][..],
        ),
        (
            "local numeric quote authorization",
            RequestSpec::new(
                Method::GET,
                "/ap/users/116844606259201001/quote_authorizations/116845317980168701",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[
                "/@context/0",
                "/@context/1",
                "/id",
                "/type",
                "/attributedTo",
                "/interactingObject",
                "/interactionTarget",
            ][..],
        ),
        (
            "pending local quote authorization",
            RequestSpec::new(
                Method::GET,
                "/users/alice/quote_authorizations/-97",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "private local quote authorization anonymous",
            RequestSpec::new(
                Method::GET,
                "/users/alice/quote_authorizations/-90",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "public note invalid signature",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001",
                None,
                invalid_signature_headers_for("/users/alice/statuses/116844842188805001"),
                Vec::new(),
            )?,
            &["/id", "/type", "/attributedTo", "/content"][..],
        ),
        (
            "followers-only note anonymous",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844850053125003",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "followers-only note signed for follower",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844850053125003",
                None,
                signed_remote_actor_headers_for("/users/alice/statuses/116844850053125003"),
                Vec::new(),
            )?,
            &["/id", "/type", "/attributedTo", "/content"][..],
        ),
        (
            "followers-only numeric note signed for follower",
            RequestSpec::new(
                Method::GET,
                "/ap/users/116844606259201001/statuses/116844850053125003",
                None,
                signed_remote_actor_headers_for(
                    "/ap/users/116844606259201001/statuses/116844850053125003",
                ),
                Vec::new(),
            )?,
            &["/id", "/type", "/attributedTo", "/content"][..],
        ),
        (
            "direct note anonymous",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844853985285004",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "direct note signed for mentioned account",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844853985285004",
                None,
                signed_remote_actor_headers_for("/users/alice/statuses/116844853985285004"),
                Vec::new(),
            )?,
            &[
                "/id",
                "/type",
                "/attributedTo",
                "/inReplyTo",
                "/inReplyToAtomUri",
                "/conversation",
                "/context",
                "/content",
            ][..],
        ),
        (
            "public note signed by blocked viewer",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001",
                None,
                signed_actor_headers_with_key(
                    "/users/alice/statuses/116844842188805001",
                    "https://fixture-v4-6-5.rustodon.invalid/users/moderator#main-key",
                ),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "public note activity",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001/activity",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[
                "/@context/0",
                "/@context/1",
                "/id",
                "/type",
                "/actor",
                "/published",
                "/to",
                "/cc",
                "/object/id",
                "/object/type",
            ][..],
        ),
        (
            "local boost note redirects to original",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/-416",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "local boost likes collection",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/-416/likes",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/totalItems"][..],
        ),
        (
            "local boost activity",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/-416/activity",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/actor", "/object/id", "/object/type"][..],
        ),
        (
            "public numeric note activity",
            RequestSpec::new(
                Method::GET,
                "/ap/users/116844606259201001/statuses/116844842188805001/activity",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/actor", "/object/id", "/object/type"][..],
        ),
        (
            "private note activity anonymous",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844850053125003/activity",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "private note activity signed",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844850053125003/activity",
                None,
                signed_remote_actor_headers_for(
                    "/users/alice/statuses/116844850053125003/activity",
                ),
                Vec::new(),
            )?,
            &["/id", "/type", "/actor", "/object/id", "/object/type"][..],
        ),
        (
            "direct note activity anonymous",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844853985285004/activity",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "direct note activity signed",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844853985285004/activity",
                None,
                signed_remote_actor_headers_for(
                    "/users/alice/statuses/116844853985285004/activity",
                ),
                Vec::new(),
            )?,
            &["/id", "/type", "/actor", "/object/id", "/object/type"][..],
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
            "public status replies collection",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001/replies",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[
                "/id",
                "/type",
                "/first/type",
                "/first/items/0/id",
                "/first/items/0/type",
                "/first/next",
            ][..],
        ),
        (
            "public status replies page",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001/replies",
                Some("page=true".to_owned()),
                activity_headers(),
                Vec::new(),
            )?,
            &[
                "/id",
                "/type",
                "/partOf",
                "/items/0/id",
                "/items/0/type",
                "/next",
            ][..],
        ),
        (
            "public numeric status replies collection",
            RequestSpec::new(
                Method::GET,
                "/ap/users/116844606259201001/statuses/116844842188805001/replies",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/first/partOf", "/first/items/0/id"][..],
        ),
        (
            "public status replies other accounts page",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001/replies",
                Some("page=true&only_other_accounts=true".to_owned()),
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/partOf", "/items/0"][..],
        ),
        (
            "public status likes collection",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001/likes",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/totalItems"][..],
        ),
        (
            "public status shares collection",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001/shares",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/totalItems"][..],
        ),
        (
            "public numeric status likes collection",
            RequestSpec::new(
                Method::GET,
                "/ap/users/116844606259201001/statuses/116844842188805001/likes",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &["/id", "/type", "/totalItems"][..],
        ),
        (
            "followers-only status replies anonymous",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844850053125003/replies",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "followers-only status replies signed for follower",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844850053125003/replies",
                None,
                signed_remote_actor_headers_for("/users/alice/statuses/116844850053125003/replies"),
                Vec::new(),
            )?,
            &["/id", "/type"][..],
        ),
        (
            "followers-only status replies OAuth bearer",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844850053125003/replies",
                None,
                bearer_activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "direct status likes anonymous",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844853985285004/likes",
                None,
                activity_headers(),
                Vec::new(),
            )?,
            &[][..],
        ),
        (
            "direct status likes signed for mentioned account",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844853985285004/likes",
                None,
                signed_remote_actor_headers_for("/users/alice/statuses/116844853985285004/likes"),
                Vec::new(),
            )?,
            &["/id", "/type", "/totalItems"][..],
        ),
        (
            "public status likes blocked viewer",
            RequestSpec::new(
                Method::GET,
                "/users/alice/statuses/116844842188805001/likes",
                None,
                signed_actor_headers_with_key(
                    "/users/alice/statuses/116844842188805001/likes",
                    "https://fixture-v4-6-5.rustodon.invalid/users/moderator#main-key",
                ),
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
            "alice outbox signed",
            RequestSpec::new(
                Method::GET,
                "/users/alice/outbox",
                None,
                signed_actor_headers_for("/users/alice/outbox"),
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
            "alice outbox page OAuth bearer",
            RequestSpec::new(
                Method::GET,
                "/users/alice/outbox",
                Some("page=true".to_owned()),
                bearer_activity_headers(),
                Vec::new(),
            )?,
            &[
                "/orderedItems/0/id",
                "/orderedItems/1/id",
                "/orderedItems/2/id",
                "/orderedItems/3/id",
            ][..],
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
            "alice followers signed",
            RequestSpec::new(
                Method::GET,
                "/users/alice/followers",
                None,
                signed_actor_headers_for("/users/alice/followers"),
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
            "alice following signed",
            RequestSpec::new(
                Method::GET,
                "/users/alice/following",
                None,
                signed_actor_headers_for("/users/alice/following"),
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
        if label == "local boost note redirects to original" {
            if responses.mastodon.status != 302 {
                return Err(format!(
                    "{label}: fixture expected HTTP 302 from Mastodon, got {}",
                    responses.mastodon.status
                )
                .into());
            }
            if responses.mastodon.headers.get("location") != responses.rust.headers.get("location")
            {
                return Err(format!(
                    "{label}: Location header mismatch: Mastodon={:?}, Rust={:?}",
                    responses.mastodon.headers.get("location"),
                    responses.rust.headers.get("location")
                )
                .into());
            }
        }
        if (label.starts_with("public status ")
            || label.starts_with("public numeric status ")
            || label.starts_with("followers-only status replies signed")
            || label.starts_with("direct status likes signed"))
            && responses.mastodon.status != 200
        {
            return Err(format!(
                "{label}: fixture expected HTTP 200 from Mastodon, got {}",
                responses.mastodon.status
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
        let status_document = label.contains("note") || label == "local boost activity";
        if status_document {
            compare_activitypub_cache_headers(label, &responses.mastodon, &responses.rust)
                .map_err(|error| -> Box<dyn Error> { error.into() })?;
        }
        if responses.rust.status == 200
            && (label.contains("note") || label == "local boost activity")
            && !label.contains("collection")
            && responses.mastodon.headers.get("link") != responses.rust.headers.get("link")
        {
            return Err(format!(
                "{label}: Link header mismatch: Mastodon={:?}, Rust={:?}",
                responses.mastodon.headers.get("link"),
                responses.rust.headers.get("link")
            )
            .into());
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

pub(crate) async fn run_actor_media_case(
    config: DifferentialConfig,
    rust_url: &Url,
    media_root_url: &str,
) -> Result<(), Box<dyn Error>> {
    let guard = ReadOnlyGuard::begin(config, rust_url).await?;
    let request = RequestSpec::new(
        Method::GET,
        "/users/alice",
        None,
        {
            let mut headers = stable_request_headers();
            headers.insert(ACCEPT, HeaderValue::from_static(ACTIVITY_ACCEPT));
            headers
        },
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_json_paths(
        "actor profile media",
        &responses.mastodon,
        &responses.rust,
        &[
            "/icon/type",
            "/icon/mediaType",
            "/icon/url",
            "/image/type",
            "/image/mediaType",
            "/image/url",
        ],
    )?;
    for (side, response) in [("Mastodon", &responses.mastodon), ("Rust", &responses.rust)] {
        let actor: Value = serde_json::from_slice(&response.body)?;
        for field in ["icon", "image"] {
            let url = actor[field]["url"]
                .as_str()
                .ok_or_else(|| format!("{side} actor omitted {field}.url"))?;
            let expected_root = if media_root_url.starts_with("http") {
                media_root_url.to_owned()
            } else {
                format!("https://fixture-v4-6-5.rustodon.invalid{media_root_url}")
            };
            if !url.starts_with(&format!("{}/", expected_root.trim_end_matches('/'))) {
                return Err(format!(
                    "{side} actor {field}.url {url:?} does not use {expected_root:?}"
                )
                .into());
            }
        }
        let emoji = actor["tag"]
            .as_array()
            .and_then(|tags| tags.iter().find(|tag| tag["type"] == "Emoji"))
            .ok_or_else(|| format!("{side} actor omitted its profile emoji tag"))?;
        if emoji["name"] != ":actorprofileblob:"
            || emoji["id"] != "https://fixture-v4-6-5.rustodon.invalid/emojis/12991"
        {
            return Err(
                format!("{side} actor returned an unexpected profile emoji: {emoji}").into(),
            );
        }
    }
    let mastodon_actor: Value = serde_json::from_slice(&responses.mastodon.body)?;
    let rust_actor: Value = serde_json::from_slice(&responses.rust.body)?;
    let profile_emoji = |actor: &Value| {
        actor["tag"]
            .as_array()
            .and_then(|tags| tags.iter().find(|tag| tag["type"] == "Emoji"))
            .cloned()
    };
    if profile_emoji(&mastodon_actor) != profile_emoji(&rust_actor) {
        return Err("actor profile emoji differs from pinned Mastodon".into());
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
