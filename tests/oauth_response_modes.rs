//! R15: query-only OAuth response modes in a disposable restored fixture.
use std::{collections::BTreeMap, error::Error};

use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{Repository, WriteRepository};
use rustodon::web::{WebState, router};
use serde_json::{Value, json};
use url::Url;

const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const SESSION_COOKIE: &str = "_mastodon_session=r15-fixture-consent-session";
const CLIENT: &str = "r15-public-client";
const REDIRECT: &str = "https://client.fixture.invalid/callback?existing=kept";
const STATE: &str = "r15 state & + / ? # ü";
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
        active_month: 0,
        active_halfyear: 0,
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
    html.split("<input type=\"hidden\" name=\"")
        .skip(1)
        .filter_map(|input| {
            let (name, rest) = input.split_once('"')?;
            let value = rest.strip_prefix(" value=\"")?.split_once('"')?.0;
            Some((
                name.to_owned(),
                html_escape::decode_html_entities(value).into_owned(),
            ))
        })
        .collect()
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

#[tokio::test]
#[ignore = "requires tools/mastodon-fixture schema-read-test oauth_response_modes"]
#[allow(clippy::too_many_lines)]
async fn only_query_response_modes_are_advertised_and_accepted() -> TestResult {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let mut connection = <sqlx::PgConnection as sqlx::Connection>::connect(&owner_url).await?;
    rustodon::operational_schema::migrate(&mut connection).await?;
    let writer = WriteRepository::connect(&owner_url).await?;
    // Authenticated fixture session only; do not exercise password/login code.
    sqlx::query("INSERT INTO session_activations (user_id, session_id, access_token_id, created_at, updated_at) SELECT 101, 'r15-fixture-consent-session', id, clock_timestamp(), clock_timestamp() FROM oauth_access_tokens WHERE token = 'fixture-bearer-token-v4-6-5'")
        .execute(writer.pool()).await?;
    sqlx::query("INSERT INTO oauth_applications (name, uid, secret, redirect_uri, scopes, confidential, created_at, updated_at) VALUES ($1, $1, 'fixture-only-secret', $2, 'read', false, clock_timestamp(), clock_timestamp())")
        .bind(CLIENT).bind(REDIRECT).execute(writer.pool()).await?;
    let repository = Repository::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    let state = WebState::new(
        repository,
        Url::parse(&format!("https://{DOMAIN}/"))?,
        DOMAIN,
        "/system",
        std::env::temp_dir(),
        runtime(),
        Vec::new(),
        vec![DOMAIN.to_owned()],
    )?
    .with_write_repository(writer.clone());
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
    let mut retained_modes = Vec::new();
    for mode in [None, Some("query")] {
        for approve in [false, true] {
            let parameters = parameters(mode);
            // The login return target must retain all OAuth parameters (including explicit mode).
            let response = authorize(&client, &base, "GET", &parameters, None).await?;
            assert_eq!(response.status(), 302);
            let login = location(&response)?;
            assert_eq!(login.path(), "/auth/sign_in");
            let return_to = login
                .query_pairs()
                .find(|(key, _)| key == "return_to")
                .ok_or("missing return target")?
                .1
                .into_owned();
            let return_url = Url::parse(&format!("https://{DOMAIN}/"))?.join(&return_to)?;
            assert_eq!(return_url.path(), "/oauth/authorize");
            assert_eq!(
                return_url
                    .query_pairs()
                    .into_owned()
                    .collect::<Parameters>(),
                parameters
            );
            let response =
                authorize(&client, &base, "GET", &parameters, Some(SESSION_COOKIE)).await?;
            assert_eq!(response.status(), 200);
            let csrf_cookie = response
                .headers()
                .get_all("set-cookie")
                .iter()
                .filter_map(|value| value.to_str().ok())
                .filter_map(|value| value.split(';').next())
                .find(|value| value.starts_with("__Host-csrf_token="))
                .ok_or("missing CSRF cookie")?
                .to_owned();
            let cookie = format!("{SESSION_COOKIE}; {csrf_cookie}");
            // Submit only the actual form fields; do not reattach the GET query to hide dropped state/mode.
            let mut fields = hidden_fields(&response.text().await?);
            retained_modes.push(fields.get("response_mode").cloned());
            fields.insert("approve".to_owned(), approve.to_string());
            let response = authorize(&client, &base, "POST", &fields, Some(&cookie)).await?;
            assert_eq!(response.status(), 302);
            let callback = location(&response)?;
            assert_eq!(
                callback.origin().ascii_serialization(),
                "https://client.fixture.invalid"
            );
            assert_eq!(callback.path(), "/callback");
            assert_eq!(callback.fragment(), None);
            let pairs = callback.query_pairs().into_owned().collect::<Parameters>();
            assert_eq!(pairs.get("existing").map(String::as_str), Some("kept"));
            assert_eq!(pairs.get("state").map(String::as_str), Some(STATE));
            if approve {
                assert_eq!(pairs.len(), 3);
                let code = pairs.get("code").ok_or("approval code missing")?;
                assert!(
                    sqlx::query_scalar::<_, bool>(
                        "SELECT EXISTS (SELECT 1 FROM oauth_access_grants WHERE token = $1)"
                    )
                    .bind(code)
                    .fetch_one(writer.pool())
                    .await?
                );
            } else {
                assert_eq!(pairs.len(), 4);
                assert_eq!(
                    pairs.get("error").map(String::as_str),
                    Some("access_denied")
                );
                assert!(pairs.contains_key("error_description"));
            }
        }
    }
    let response = authorize(
        &client,
        &base,
        "GET",
        &parameters(Some("query")),
        Some(SESSION_COOKIE),
    )
    .await?;
    let csrf_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .find(|value| value.starts_with("__Host-csrf_token="))
        .ok_or("missing CSRF cookie")?
        .to_owned();
    let cookie = format!("{SESSION_COOKIE}; {csrf_cookie}");
    let fields = hidden_fields(&response.text().await?);
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_access_grants")
        .fetch_one(writer.pool())
        .await?;
    let mut rejected = Vec::new();
    let mut expected = Vec::new();
    for mode in ["fragment", "form_post", "unknown"] {
        for authenticated in [false, true] {
            for (method, approve) in [("GET", false), ("POST", false), ("POST", true)] {
                let mut fields = fields.clone();
                fields.insert("response_mode".to_owned(), mode.to_owned());
                fields.insert("approve".to_owned(), approve.to_string());
                let response = authorize(
                    &client,
                    &base,
                    method,
                    &fields,
                    authenticated.then_some(cookie.as_str()),
                )
                .await?;
                let status = response.status().as_u16();
                let redirected = response.headers().contains_key("location");
                let body = response.text().await?;
                let error = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|body| body["error"].as_str().map(str::to_owned));
                rejected.push((
                    mode,
                    authenticated,
                    method,
                    approve,
                    status,
                    redirected,
                    error,
                ));
                expected.push((
                    mode,
                    authenticated,
                    method,
                    approve,
                    400,
                    false,
                    Some("invalid_request".to_owned()),
                ));
            }
        }
    }
    // Even supported modes must never redirect to an unregistered callback on approval or denial.
    for approve in [false, true] {
        let mut fields = fields.clone();
        fields.insert("response_mode".to_owned(), "query".to_owned());
        fields.insert(
            "redirect_uri".to_owned(),
            "https://attacker.invalid/callback".to_owned(),
        );
        fields.insert("approve".to_owned(), approve.to_string());
        for method in ["GET", "POST"] {
            let response = authorize(&client, &base, method, &fields, Some(&cookie)).await?;
            assert_eq!(response.status(), 400);
            assert!(!response.headers().contains_key("location"));
        }
    }
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_access_grants")
        .fetch_one(writer.pool())
        .await?;
    server.abort();
    assert_eq!(
        (
            discovery["response_modes_supported"].clone(),
            retained_modes,
            rejected,
            after - before
        ),
        (
            json!(["query"]),
            vec![Some("query".to_owned()); 4],
            expected,
            0
        ),
        "(advertised modes, form modes, unsupported mode responses, unauthorized grant creation)"
    );
    Ok(())
}
