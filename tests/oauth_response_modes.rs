//! OAuth query, fragment and form-post response modes in a disposable restored fixture.
use std::{collections::BTreeMap, error::Error};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{Repository, WriteRepository};
use rustodon::web::{WebState, router};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use url::Url;

const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const SESSION_COOKIE: &str = "_mastodon_session=r15-fixture-consent-session";
const CLIENT: &str = "r15-public-client";
const REDIRECT: &str = "https://client.fixture.invalid/callback?existing=kept&quoted=%22%3C";
const HTTP_REDIRECT: &str = "http://client.fixture.invalid/callback?existing=kept&quoted=%22%3C";
const CUSTOM_REDIRECT: &str = "rustodon-fixture://oauth/callback?existing=kept";
const OOB_REDIRECT: &str = "urn:ietf:wg:oauth:2.0:oob";
const STATE: &str = "r15 state & + / ? # ü \" ' ><script>alert(1)</script>\r\n";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const MODES: [Option<&str>; 6] = [
    None,
    Some("query"),
    Some("fragment"),
    Some("form_post"),
    Some(""),
    Some(" \t"),
];
type TestResult<T = ()> = Result<T, Box<dyn Error>>;
type Parameters = BTreeMap<String, String>;

fn runtime() -> InstanceRuntimeConfig {
    InstanceRuntimeConfig {
        domain: DOMAIN.to_owned(),
        version: "4.6.5".to_owned(),
        source_url: String::new(),
        streaming_api: String::new(),
        vapid_public_key: None,
        thumbnail_url: String::new(),
        thumbnail_description: String::new(),
        thumbnail_blurhash: None,
        thumbnail_versions: None,
        icons: Vec::new(),
        languages: vec!["en".to_owned()],
        translation_enabled: false,
        limited_federation: false,
        single_user_mode: false,
        terms_of_service_url: None,
        sso_signup_url: None,
        wrapstodon: None,
    }
}

fn parameters(mode: Option<&str>) -> Parameters {
    let mut parameters: Parameters = [
        ("client_id", CLIENT),
        ("redirect_uri", REDIRECT),
        ("response_type", "code"),
        ("scope", "read"),
        ("state", STATE),
        (
            "code_challenge",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        ),
        ("code_challenge_method", "S256"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect();
    if let Some(mode) = mode {
        parameters.insert("response_mode".to_owned(), mode.to_owned());
    }
    parameters
}

async fn authorize(
    client: &reqwest::Client,
    base: &str,
    method: &str,
    parameters: &Parameters,
    cookie: Option<&str>,
) -> TestResult<reqwest::Response> {
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(parameters)
        .finish();
    let mut request = if method == "GET" {
        client.get(format!("{base}/oauth/authorize?{encoded}"))
    } else {
        client
            .post(format!("{base}/oauth/authorize"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(encoded)
    }
    .header("host", DOMAIN);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    Ok(request.send().await?)
}

fn hidden_fields(html: &str) -> Parameters {
    // Model HTML source newline normalization before entity decoding, then the
    // CRLF normalization performed by URL-encoded browser form submission.
    // This deliberately small parser targets our renderer, not arbitrary HTML.
    let html = html.replace("\r\n", "\n").replace('\r', "\n");
    let mut fields = Parameters::new();
    for input in html.split("<input type=\"hidden\" name=\"").skip(1) {
        let (name, rest) = input.split_once('"').expect("hidden field name");
        let value = rest
            .strip_prefix(" value=\"")
            .expect("hidden field value")
            .split_once('"')
            .expect("hidden field end")
            .0;
        let value = html_escape::decode_html_entities(value)
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', "\r\n");
        assert!(
            fields.insert(name.to_owned(), value).is_none(),
            "duplicate field {name}"
        );
    }
    fields
}

fn single_form(html: &str) -> TestResult<(&str, &str)> {
    let (_, form) = html.split_once("<form ").ok_or("missing form")?;
    assert!(!form.contains("<form "), "only one form may be submitted");
    let (tag, rest) = form.split_once('>').ok_or("missing form tag end")?;
    let (body, _) = rest.split_once("</form>").ok_or("missing form end")?;
    assert!(tag.contains("method=\"post\""));
    Ok((tag, body))
}

fn csp_directives(csp: &str) -> BTreeMap<&str, Vec<&str>> {
    let mut directives = BTreeMap::new();
    for directive in csp.split(';') {
        let mut words = directive.split_whitespace();
        if let Some(name) = words.next() {
            assert!(
                directives.insert(name, words.collect()).is_none(),
                "duplicate CSP directive"
            );
        }
    }
    directives
}

#[test]
fn hidden_fields_model_browser_crlf_submission() {
    for value in ["line1\r\nline2", "line1\nline2", "line1&#13;&#10;line2"] {
        assert_eq!(
            hidden_fields(&format!(
                "<input type=\"hidden\" name=\"state\" value=\"{value}\">"
            ))["state"],
            "line1\r\nline2"
        );
    }
}

fn location(response: &reqwest::Response) -> TestResult<Url> {
    Ok(
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?.join(
            response
                .headers()
                .get("location")
                .ok_or("missing redirect")?
                .to_str()?,
        )?,
    )
}

fn web_state(repository: Repository) -> TestResult<WebState> {
    Ok(WebState::new(
        repository,
        Url::parse(&format!("https://{DOMAIN}/"))?,
        DOMAIN,
        "/system",
        std::env::temp_dir(),
        runtime(),
        Vec::new(),
        vec![DOMAIN.to_owned()],
    )?)
}

fn assert_no_store(response: &reqwest::Response) {
    assert!(
        response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .split(',')
            .any(|directive| directive.trim() == "no-store")
    );
}

async fn assert_local_error(response: reqwest::Response, status: u16, error: &str) -> TestResult {
    assert_eq!(response.status(), status);
    assert!(!response.headers().contains_key("location"));
    assert_no_store(&response);
    let body: Value = serde_json::from_str(&response.text().await?)?;
    assert_eq!(body["error"], error);
    Ok(())
}

async fn consent(
    client: &reqwest::Client,
    base: &str,
    parameters: &Parameters,
) -> TestResult<(Parameters, String)> {
    let response = authorize(client, base, "GET", parameters, Some(SESSION_COOKIE)).await?;
    assert_eq!(response.status(), 200);
    assert_no_store(&response);
    // A callback-specific form-action must never leak into the consent page.
    assert_eq!(
        csp_directives(response.headers()["content-security-policy"].to_str()?).get("form-action"),
        Some(&vec!["'self'"])
    );
    let csrf_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .find(|value| value.starts_with("__Host-csrf_token="))
        .ok_or("missing CSRF cookie")?
        .to_owned();
    let html = response.text().await?;
    assert!(!html.contains("<script>alert(1)</script>"));
    let (tag, body) = single_form(&html)?;
    assert!(tag.contains("action=\"/oauth/authorize\""));
    let fields = hidden_fields(body);
    for (name, value) in parameters {
        if name != "response_mode" {
            assert_eq!(fields.get(name), Some(value), "consent lost {name}");
        }
    }
    assert_eq!(
        fields.get("response_mode").map(String::as_str),
        Some(
            parameters
                .get("response_mode")
                .filter(|mode| !mode.trim().is_empty())
                .map_or("query", String::as_str)
        )
    );
    assert!(
        fields
            .get("csrf_token")
            .is_some_and(|token| !token.is_empty())
    );
    Ok((fields, format!("{SESSION_COOKIE}; {csrf_cookie}")))
}

fn assert_form_post_security(html: &str, csp: &str, redirect_uri: &str) -> TestResult {
    let registered = Url::parse(redirect_uri)?;
    assert!(matches!(registered.scheme(), "http" | "https"));
    let origin = registered.origin().ascii_serialization();
    let callback_path = format!("{origin}{}", registered.path());
    let directives = csp_directives(csp);
    for directive in ["default-src", "base-uri", "frame-ancestors"] {
        assert_eq!(directives.get(directive), Some(&vec!["'none'"]));
    }
    let actions = directives.get("form-action").ok_or("missing form-action")?;
    assert_eq!(
        actions.len(),
        1,
        "only the validated callback may receive the form"
    );
    assert!(actions[0] == origin || actions[0] == callback_path);
    let sources = directives.get("script-src").ok_or("missing script-src")?;
    assert!(!sources.is_empty());
    assert!(
        sources
            .iter()
            .all(|source| { source.starts_with("'sha256-") || source.starts_with("'nonce-") }),
        "auto-submit must not require wildcard, unsafe-inline, or unsafe-eval"
    );
    let (script_tag, rest) = html
        .split_once("<script")
        .ok_or("missing auto-submit script")?;
    assert!(!script_tag.contains(" onload="));
    let (attributes, rest) = rest.split_once('>').ok_or("missing script tag end")?;
    assert!(!attributes.contains("src="));
    let (script, after) = rest.split_once("</script>").ok_or("missing script end")?;
    assert!(
        !after.contains("<script"),
        "only the auto-submit script is needed"
    );
    // A static script must submit the one validated form, not merely contain a
    // submit call somewhere in inert text. This is our renderer's contract.
    assert_eq!(script.trim(), "document.forms[0].submit();");
    let hash = format!(
        "'sha256-{}'",
        STANDARD.encode(Sha256::digest(script.as_bytes()))
    );
    let nonce = attributes
        .split_once("nonce=\"")
        .and_then(|(_, value)| value.split_once('"'))
        .map(|(value, _)| format!("'nonce-{value}'"));
    assert!(
        sources.contains(&hash.as_str())
            || nonce
                .as_ref()
                .is_some_and(|nonce| sources.contains(&nonce.as_str()))
    );
    assert!(
        html.contains("<noscript>"),
        "provide a manual submission fallback"
    );
    assert!(html.contains("type=\"submit\""));
    Ok(())
}

async fn callback_fields(
    response: reqwest::Response,
    mode: Option<&str>,
    redirect_uri: &str,
) -> TestResult<Parameters> {
    assert_no_store(&response);
    let registered = Url::parse(redirect_uri)?;
    if mode == Some("form_post") {
        assert_eq!(response.status(), 200);
        assert!(!response.headers().contains_key("location"));
        assert_eq!(
            response.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert!(matches!(
            response.headers()["referrer-policy"].to_str()?,
            "no-referrer" | "same-origin"
        ));
        let csp = response.headers()["content-security-policy"]
            .to_str()?
            .to_owned();
        let html = response.text().await?;
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(
            !html.contains(&format!("action=\"{redirect_uri}\"")),
            "escape the action's ampersand"
        );
        let (tag, body) = single_form(&html)?;
        let action = tag
            .split_once("action=\"")
            .ok_or("missing form action")?
            .1
            .split_once('"')
            .ok_or("unterminated action")?
            .0;
        assert_eq!(html_escape::decode_html_entities(action), redirect_uri);
        assert_form_post_security(&html, &csp, redirect_uri)?;
        assert!(body.contains("<noscript>"));
        assert!(body.contains("type=\"submit\""));
        return Ok(hidden_fields(body));
    }
    assert_eq!(response.status(), 302);
    let callback = location(&response)?;
    let mut target = callback.clone();
    target.set_query(None);
    target.set_fragment(None);
    let mut expected_target = registered.clone();
    expected_target.set_query(None);
    expected_target.set_fragment(None);
    assert_eq!(
        target, expected_target,
        "preserve scheme/authority/path, including custom schemes"
    );
    if mode == Some("fragment") {
        assert_eq!(
            callback.query(),
            registered.query(),
            "do not leak response fields into query"
        );
        Ok(
            url::form_urlencoded::parse(callback.fragment().ok_or("missing fragment")?.as_bytes())
                .into_owned()
                .collect(),
        )
    } else {
        assert_eq!(callback.fragment(), None);
        let mut fields = callback.query_pairs().into_owned().collect::<Parameters>();
        for (name, value) in registered.query_pairs() {
            assert_eq!(
                fields.remove(name.as_ref()).as_deref(),
                Some(value.as_ref())
            );
        }
        Ok(fields)
    }
}

async fn assert_callback_error(
    response: reqwest::Response,
    mode: Option<&str>,
    error: &str,
    state: Option<&str>,
) -> TestResult {
    let fields = callback_fields(response, mode, REDIRECT).await?;
    assert_eq!(fields.get("error").map(String::as_str), Some(error));
    assert!(
        fields
            .get("error_description")
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(fields.get("state").map(String::as_str), state);
    assert_eq!(
        fields.len(),
        2 + usize::from(state.is_some()),
        "error must not carry a code"
    );
    Ok(())
}

async fn exchange(
    client: &reqwest::Client,
    base: &str,
    code: &str,
    verifier: Option<&str>,
    redirect: &str,
) -> TestResult<reqwest::Response> {
    let mut fields = vec![
        ("grant_type", "authorization_code"),
        ("client_id", CLIENT),
        ("code", code),
        ("redirect_uri", redirect),
    ];
    if let Some(verifier) = verifier {
        fields.push(("code_verifier", verifier));
    }
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    Ok(client
        .post(format!("{base}/oauth/token"))
        .header("host", DOMAIN)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?)
}

async fn grant_snapshot(writer: &WriteRepository) -> TestResult<Value> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(to_jsonb(grant_row) ORDER BY id), '[]'::jsonb) FROM oauth_access_grants grant_row"
    ).fetch_one(writer.pool()).await?)
}

#[allow(clippy::too_many_lines)]
async fn assert_callback_boundaries(
    client: &reqwest::Client,
    base: &str,
    writer: &WriteRepository,
) -> TestResult {
    // HTTP is supported too. Registered custom schemes and OOB retain their
    // existing query/code-display behavior, plus explicit fragment delivery.
    for redirect in [HTTP_REDIRECT, CUSTOM_REDIRECT, OOB_REDIRECT] {
        for mode in [None, Some("query"), Some("fragment"), Some("form_post")] {
            if mode == Some("form_post") && redirect != HTTP_REDIRECT {
                continue;
            }
            for approve in [false, true] {
                let mut request = parameters(mode);
                request.insert("redirect_uri".to_owned(), redirect.to_owned());
                let (mut fields, cookie) = consent(client, base, &request).await?;
                fields.insert("approve".to_owned(), approve.to_string());
                let before = grant_snapshot(writer).await?;
                let response = authorize(client, base, "POST", &fields, Some(&cookie)).await?;
                let pairs = if redirect == OOB_REDIRECT && approve {
                    // Preserve Rustodon's existing local HTML code display rather
                    // than copying Rails' additional local-show redirect.
                    assert_eq!(response.status(), 200);
                    assert!(!response.headers().contains_key("location"));
                    assert_no_store(&response);
                    assert_eq!(
                        response.headers()["content-type"],
                        "text/html; charset=utf-8"
                    );
                    let html = response.text().await?;
                    assert!(!html.contains("<form"));
                    assert!(!html.contains("alert(1)"));
                    let code = html
                        .split_once("<p>")
                        .ok_or("missing OOB code")?
                        .1
                        .split_once("</p>")
                        .ok_or("missing OOB code end")?
                        .0;
                    Parameters::from([(
                        "code".to_owned(),
                        html_escape::decode_html_entities(code).into_owned(),
                    )])
                } else {
                    let pairs = callback_fields(response, mode, redirect).await?;
                    assert_eq!(pairs.get("state").map(String::as_str), Some(STATE));
                    assert_eq!(pairs.len(), if approve { 2 } else { 3 });
                    pairs
                };
                let after = grant_snapshot(writer).await?;
                if approve {
                    assert_eq!(
                        after.as_array().unwrap().len(),
                        before.as_array().unwrap().len() + 1
                    );
                    let code = pairs.get("code").ok_or("missing callback code")?;
                    let persisted: (String, String, String) = sqlx::query_as(
                        "SELECT redirect_uri, code_challenge, code_challenge_method FROM oauth_access_grants WHERE token = $1"
                    ).bind(code).fetch_one(writer.pool()).await?;
                    assert_eq!(
                        persisted,
                        (
                            redirect.to_owned(),
                            request["code_challenge"].clone(),
                            "S256".to_owned()
                        )
                    );
                } else {
                    assert_eq!(after, before, "denial must not mutate grants");
                    assert_eq!(
                        pairs.get("error").map(String::as_str),
                        Some("access_denied")
                    );
                    assert!(
                        pairs
                            .get("error_description")
                            .is_some_and(|value| !value.is_empty())
                    );
                }
            }
        }
    }
    for redirect in [CUSTOM_REDIRECT, OOB_REDIRECT] {
        // Obtain valid consent/CSRF through supported query mode; changing only
        // the mode must reject this *registered* callback, not create/revoke a code.
        let mut request = parameters(Some("query"));
        request.insert("redirect_uri".to_owned(), redirect.to_owned());
        let (mut fields, cookie) = consent(client, base, &request).await?;
        fields.insert("response_mode".to_owned(), "form_post".to_owned());
        for (method, approve) in [("GET", false), ("POST", false), ("POST", true)] {
            fields.insert("approve".to_owned(), approve.to_string());
            let before = grant_snapshot(writer).await?;
            let response = authorize(client, base, method, &fields, Some(&cookie)).await?;
            assert_eq!(response.status(), 400);
            assert!(!response.headers().contains_key("location"));
            assert_no_store(&response);
            let body: Value = serde_json::from_str(&response.text().await?)?;
            assert_eq!(
                body,
                json!({
                    "error": "unsupported_response_mode",
                    "error_description": "form_post requires an HTTP(S) redirect_uri."
                })
            );
            assert_eq!(
                grant_snapshot(writer).await?,
                before,
                "unsupported combination mutated grants"
            );
        }
    }
    Ok(())
}

async fn session_guard_responses(
    client: &reqwest::Client,
    base: &str,
    forms: &[(Parameters, String)],
    session: &str,
) -> TestResult<Vec<(u16, Option<String>, String)>> {
    let mut responses = Vec::new();
    for (fields, cookie) in forms {
        // Keep a valid CSRF cookie/token so session checks decide the outcome.
        let csrf_cookie = cookie
            .split_once(';')
            .ok_or("missing CSRF cookie")?
            .1
            .trim();
        let cookie = if session.is_empty() {
            csrf_cookie.to_owned()
        } else {
            format!("{session}; {csrf_cookie}")
        };
        for (method, approve) in [("GET", false), ("POST", false), ("POST", true)] {
            let mut fields = fields.clone();
            fields.insert("approve".to_owned(), approve.to_string());
            let response = authorize(client, base, method, &fields, Some(&cookie)).await?;
            let redirect = location(&response)?;
            responses.push((
                response.status().as_u16(),
                redirect.host_str().map(str::to_owned),
                redirect.path().to_owned(),
            ));
        }
    }
    Ok(responses)
}

#[tokio::test]
async fn metadata_advertises_query_fragment_and_form_post() -> TestResult {
    // Discovery does not need a live database or fixture.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")?;
    let state = web_state(Repository::from_pool(pool))?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let response = reqwest::Client::new()
        .get(format!("{base}/.well-known/oauth-authorization-server"))
        .header("host", DOMAIN)
        .send()
        .await?;
    let status = response.status();
    let body: Value = serde_json::from_str(&response.text().await?)?;
    server.abort();
    assert_eq!(status, 200);
    assert_eq!(
        body["response_modes_supported"],
        json!(["query", "fragment", "form_post"])
    );
    assert_eq!(body["response_types_supported"], json!(["code"]));
    assert_eq!(
        body["grant_types_supported"],
        json!(["authorization_code", "client_credentials"])
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires tools/mastodon-fixture schema-read-test oauth_response_modes"]
#[allow(clippy::too_many_lines)]
async fn supported_response_modes_preserve_authorization_security() -> TestResult {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let mut connection = <sqlx::PgConnection as sqlx::Connection>::connect(&owner_url).await?;
    rustodon::operational_schema::migrate(&mut connection).await?;
    let writer = WriteRepository::connect(&owner_url).await?;
    // Authenticated fixture session only; do not exercise password/login code.
    sqlx::query("INSERT INTO session_activations (user_id, session_id, access_token_id, created_at, updated_at) SELECT 101, 'r15-fixture-consent-session', id, clock_timestamp(), clock_timestamp() FROM oauth_access_tokens WHERE token = 'fixture-bearer-token-v4-6-5'")
        .execute(writer.pool()).await?;
    sqlx::query("INSERT INTO session_activations (user_id, session_id, access_token_id, created_at, updated_at) SELECT user_id, 'r15-expired-session', access_token_id, created_at, clock_timestamp() - INTERVAL '31 days' FROM session_activations WHERE session_id = 'r15-fixture-consent-session'")
        .execute(writer.pool()).await?;
    sqlx::query("INSERT INTO oauth_applications (name, uid, secret, redirect_uri, scopes, confidential, created_at, updated_at) VALUES ($1, $1, 'fixture-only-secret', $2, 'read', false, clock_timestamp(), clock_timestamp())")
        .bind(CLIENT)
        .bind([REDIRECT, HTTP_REDIRECT, CUSTOM_REDIRECT, OOB_REDIRECT].join("\n"))
        .execute(writer.pool()).await?;
    let repository = Repository::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    let state = web_state(repository)?.with_write_repository(writer.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .get(format!("{base}/.well-known/oauth-authorization-server"))
        .header("host", DOMAIN)
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    let discovery: Value = serde_json::from_str(&response.text().await?)?;
    for mode in MODES {
        for (state, nonblank_state) in [
            (Some(STATE), Some(STATE)),
            (Some(""), None),
            (Some(" \t"), None),
            (None, None),
        ] {
            for approve in [false, true] {
                let mut parameters = parameters(mode);
                if let Some(state) = state {
                    parameters.insert("state".to_owned(), state.to_owned());
                } else {
                    parameters.remove("state");
                }
                // Login must retain parameters; stale sessions must not bypass authentication.
                for cookie in [None, Some("_mastodon_session=stale-session")] {
                    let response = authorize(&client, &base, "GET", &parameters, cookie).await?;
                    assert_eq!(response.status(), 302, "login mode {mode:?}");
                    let login = location(&response)?;
                    assert_eq!(login.path(), "/auth/sign_in");
                    assert_eq!(login.host_str(), Some(DOMAIN));
                    let return_to = login
                        .query_pairs()
                        .find(|(key, _)| key == "return_to")
                        .ok_or("missing return target")?
                        .1
                        .into_owned();
                    let return_url = Url::parse(&format!("https://{DOMAIN}/"))?.join(&return_to)?;
                    assert_eq!(return_url.path(), "/oauth/authorize");
                    assert_eq!(return_url.host_str(), Some(DOMAIN));
                    assert_eq!(
                        return_url
                            .query_pairs()
                            .into_owned()
                            .collect::<Parameters>(),
                        parameters
                    );
                }
                // Submit only actual consent fields; never repair dropped state/mode/PKCE in the test.
                let (mut fields, cookie) = consent(&client, &base, &parameters).await?;
                fields.insert("approve".to_owned(), approve.to_string());
                let before: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_access_grants")
                    .fetch_one(writer.pool())
                    .await?;
                let response = authorize(&client, &base, "POST", &fields, Some(&cookie)).await?;
                let pairs = callback_fields(response, mode, REDIRECT).await?;
                // Pinned URI/error serialization drops blank state; successful
                // form-post output compacts only nil, retaining empty/whitespace values.
                let expected_state = if approve && mode == Some("form_post") {
                    state
                } else {
                    nonblank_state
                };
                assert_eq!(pairs.get("state").map(String::as_str), expected_state);
                let after: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_access_grants")
                    .fetch_one(writer.pool())
                    .await?;
                if approve {
                    assert_eq!(after - before, 1);
                    assert_eq!(pairs.len(), 1 + usize::from(expected_state.is_some()));
                    let code = pairs.get("code").ok_or("approval code missing")?;
                    let grant: (String, String, String, i64, String) = sqlx::query_as(
                        "SELECT code_challenge, code_challenge_method, redirect_uri, resource_owner_id, scopes FROM oauth_access_grants WHERE token = $1"
                    ).bind(code).fetch_one(writer.pool()).await?;
                    assert_eq!(
                        grant,
                        (
                            parameters["code_challenge"].clone(),
                            "S256".to_owned(),
                            REDIRECT.to_owned(),
                            101,
                            "read".to_owned()
                        )
                    );
                    // Missing/wrong verifier and wrong redirect must not consume the code.
                    for (verifier, redirect) in [
                        (None, REDIRECT),
                        (
                            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                            REDIRECT,
                        ),
                        (Some(VERIFIER), "https://attacker.invalid/callback"),
                    ] {
                        assert_local_error(
                            exchange(&client, &base, code, verifier, redirect).await?,
                            400,
                            "invalid_grant",
                        )
                        .await?;
                    }
                    let response = exchange(&client, &base, code, Some(VERIFIER), REDIRECT).await?;
                    assert_eq!(response.status(), 200);
                    assert_no_store(&response);
                    let token: Value = serde_json::from_str(&response.text().await?)?;
                    assert!(
                        token["access_token"]
                            .as_str()
                            .is_some_and(|value| !value.is_empty())
                    );
                    assert_eq!(token["scope"], "read");
                    assert_local_error(
                        exchange(&client, &base, code, Some(VERIFIER), REDIRECT).await?,
                        400,
                        "invalid_grant",
                    )
                    .await?;
                } else {
                    assert_eq!(after, before, "denial must not create a grant");
                    assert_eq!(pairs.len(), 2 + usize::from(expected_state.is_some()));
                    assert_eq!(
                        pairs.get("error").map(String::as_str),
                        Some("access_denied")
                    );
                    assert!(
                        pairs
                            .get("error_description")
                            .is_some_and(|value| !value.is_empty())
                    );
                }
            }
        }
    }
    assert_callback_boundaries(&client, &base, &writer).await?;
    let (fields, cookie) = consent(&client, &base, &parameters(Some("query"))).await?;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_access_grants")
        .fetch_one(writer.pool())
        .await?;
    // Deliberate Rustodon fail-closed policy: unknown scalar modes are local
    // unsupported_response_mode on GET/POST, even before login. Rack shapes
    // remain invalid_request instead of Rails' scalar-filtering behavior.
    for (name, mode, error) in [
        ("response_mode", "unknown", "unsupported_response_mode"),
        ("response_mode", "QUERY", "unsupported_response_mode"),
        ("response_mode[]", "query", "invalid_request"),
        ("response_mode[nested]", "form_post", "invalid_request"),
    ] {
        for authenticated in [false, true] {
            for (method, approve) in [("GET", false), ("POST", false), ("POST", true)] {
                let mut fields = fields.clone();
                fields.remove("response_mode");
                fields.insert(name.to_owned(), mode.to_owned());
                fields.insert("approve".to_owned(), approve.to_string());
                assert_local_error(
                    authorize(
                        &client,
                        &base,
                        method,
                        &fields,
                        authenticated.then_some(cookie.as_str()),
                    )
                    .await?,
                    400,
                    error,
                )
                .await?;
            }
        }
    }
    for mode in MODES {
        let (mut fields, cookie) = consent(&client, &base, &parameters(mode)).await?;
        for approve in [false, true] {
            fields.insert("approve".to_owned(), approve.to_string());
            // A valid mode is never permission to bypass CSRF, even on denial.
            for csrf in [None, Some("invalid-signature")] {
                let mut bad_csrf = fields.clone();
                if let Some(csrf) = csrf {
                    bad_csrf.insert("csrf_token".to_owned(), csrf.to_owned());
                } else {
                    bad_csrf.remove("csrf_token");
                }
                assert_local_error(
                    authorize(&client, &base, "POST", &bad_csrf, Some(&cookie)).await?,
                    403,
                    "invalid_csrf_token",
                )
                .await?;
            }
            assert_local_error(
                authorize(&client, &base, "POST", &fields, Some(SESSION_COOKIE)).await?,
                403,
                "invalid_csrf_token",
            )
            .await?;
            // Both approval and denial must validate callback/client before producing any delivery.
            for (name, value, error) in [
                (
                    "redirect_uri",
                    "https://attacker.invalid/callback",
                    "invalid_request",
                ),
                (
                    "redirect_uri",
                    "https://client.fixture.invalid/callback?changed=1",
                    "invalid_request",
                ),
                (
                    "redirect_uri",
                    "https://client.fixture.invalid.attacker.invalid/callback",
                    "invalid_request",
                ),
                (
                    "redirect_uri",
                    "https://client.fixture.invalid@attacker.invalid/callback",
                    "invalid_request",
                ),
                ("redirect_uri", "javascript:alert(1)", "invalid_request"),
                (
                    "redirect_uri",
                    "//attacker.invalid/callback",
                    "invalid_request",
                ),
                (
                    "redirect_uri",
                    "https://attacker.invalid/\"><script>alert(1)</script>",
                    "invalid_request",
                ),
                ("client_id", "unknown-client", "invalid_client"),
            ] {
                let mut invalid = fields.clone();
                invalid.insert(name.to_owned(), value.to_owned());
                for method in ["GET", "POST"] {
                    assert_local_error(
                        authorize(&client, &base, method, &invalid, Some(&cookie)).await?,
                        400,
                        error,
                    )
                    .await?;
                }
            }
        }
        fields.insert("approve".to_owned(), "true".to_owned());
        // Pinned 5.9.2 sends trusted-callback POST errors through the selected
        // mode. Error bodies omit blank state even for form-post.
        for (name, value, error) in [
            ("scope", "write", "invalid_scope"),
            (
                "code_challenge_method",
                "plain",
                "invalid_code_challenge_method",
            ),
        ] {
            for (state, expected_state) in [
                (Some(STATE), Some(STATE)),
                (Some(""), None),
                (Some(" \t"), None),
                (None, None),
            ] {
                let mut invalid = fields.clone();
                invalid.insert(name.to_owned(), value.to_owned());
                if let Some(state) = state {
                    invalid.insert("state".to_owned(), state.to_owned());
                } else {
                    invalid.remove("state");
                }
                assert_callback_error(
                    authorize(&client, &base, "POST", &invalid, Some(&cookie)).await?,
                    mode,
                    error,
                    expected_state,
                )
                .await?;
            }
        }
        let mut missing_method = fields.clone();
        missing_method.remove("code_challenge_method");
        assert_callback_error(
            authorize(&client, &base, "POST", &missing_method, Some(&cookie)).await?,
            mode,
            "invalid_code_challenge_method",
            Some(STATE),
        )
        .await?;
        // Intentional existing Rustodon security profile: unlike pinned Rails,
        // public-client PKCE is mandatory and challenge syntax/length is checked.
        // These guards remain local invalid_request, not weakened to issue codes.
        for challenge in ["too-short", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!"] {
            let mut invalid = fields.clone();
            invalid.insert("code_challenge".to_owned(), challenge.to_owned());
            assert_local_error(
                authorize(&client, &base, "POST", &invalid, Some(&cookie)).await?,
                400,
                "invalid_request",
            )
            .await?;
        }
        let mut missing_pkce = fields.clone();
        missing_pkce.remove("code_challenge");
        missing_pkce.remove("code_challenge_method");
        assert_local_error(
            authorize(&client, &base, "POST", &missing_pkce, Some(&cookie)).await?,
            400,
            "invalid_request",
        )
        .await?;
    }
    let mut forms = Vec::new();
    for mode in MODES {
        forms.push(consent(&client, &base, &parameters(mode)).await?);
    }
    for session in [
        "",
        "_mastodon_session=stale-session",
        "_mastodon_session=r15-expired-session",
    ] {
        assert_eq!(
            session_guard_responses(&client, &base, &forms, session).await?,
            vec![(302, Some(DOMAIN.to_owned()), "/auth/sign_in".to_owned()); MODES.len() * 3]
        );
    }
    let disabled: bool = sqlx::query_scalar("SELECT disabled FROM users WHERE id = 101")
        .fetch_one(writer.pool())
        .await?;
    sqlx::query("UPDATE users SET disabled = true WHERE id = 101")
        .execute(writer.pool())
        .await?;
    let guarded = session_guard_responses(&client, &base, &forms, SESSION_COOKIE).await;
    // Restore the fixture user before propagating request errors or asserting.
    sqlx::query("UPDATE users SET disabled = $1 WHERE id = 101")
        .bind(disabled)
        .execute(writer.pool())
        .await?;
    assert_eq!(
        guarded?,
        vec![(302, Some(DOMAIN.to_owned()), "/settings/profile".to_owned()); MODES.len() * 3]
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_access_grants")
        .fetch_one(writer.pool())
        .await?;
    server.abort();
    assert_eq!(after, before, "rejected requests must not create grants");
    assert_eq!(
        discovery["response_modes_supported"],
        json!(["query", "fragment", "form_post"])
    );
    Ok(())
}
