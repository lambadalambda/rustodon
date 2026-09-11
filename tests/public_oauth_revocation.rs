//! R14: public PKCE revocation against the disposable restored fixture only.
use std::error::Error;

use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{Repository, WriteRepository};
use rustodon::web::{WebState, router};
use serde_json::Value;
use url::Url;

const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const SESSION: &str = "r14-fixture-consent-session";
const REDIRECT: &str = "https://client.fixture.invalid/callback";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const SECRET: &str = "r14-fixture-only-secret";
type TestResult<T = ()> = Result<T, Box<dyn Error>>;

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

fn form(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

async fn post(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    pairs: &[(&str, &str)],
) -> TestResult<reqwest::Response> {
    Ok(client
        .post(format!("{base}{path}"))
        .header("host", DOMAIN)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(pairs))
        .send()
        .await?)
}

async fn bearer_status(client: &reqwest::Client, base: &str, token: &str) -> TestResult<u16> {
    Ok(client
        .get(format!("{base}/api/v1/accounts/verify_credentials"))
        .header("host", DOMAIN)
        .bearer_auth(token)
        .send()
        .await?
        .status()
        .as_u16())
}

async fn authorize(
    client: &reqwest::Client,
    base: &str,
    application: &str,
    secret: Option<&str>,
) -> TestResult<String> {
    let parameters = form(&[
        ("client_id", application),
        ("redirect_uri", REDIRECT),
        ("response_type", "code"),
        ("scope", "read"),
        ("state", "r14-state"),
        ("code_challenge", CHALLENGE),
        ("code_challenge_method", "S256"),
    ]);
    let response = client
        .get(format!("{base}/oauth/authorize?{parameters}"))
        .header("host", DOMAIN)
        .header("cookie", format!("_mastodon_session={SESSION}"))
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    let csrf_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .find(|value| value.starts_with("__Host-csrf_token="))
        .ok_or("consent CSRF cookie missing")?
        .to_owned();
    let csrf = csrf_cookie
        .split_once('=')
        .ok_or("CSRF cookie malformed")?
        .1;
    let response = client
        .post(format!("{base}/oauth/authorize?{parameters}"))
        .header("host", DOMAIN)
        .header(
            "cookie",
            format!("_mastodon_session={SESSION}; {csrf_cookie}"),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(&[("csrf_token", csrf), ("approve", "true")]))
        .send()
        .await?;
    assert_eq!(response.status(), 302);
    let redirect = Url::parse(
        response
            .headers()
            .get("location")
            .ok_or("missing redirect")?
            .to_str()?,
    )?;
    assert_eq!(
        redirect.origin().ascii_serialization(),
        "https://client.fixture.invalid"
    );
    assert!(
        redirect
            .query_pairs()
            .any(|(key, value)| key == "state" && value == "r14-state")
    );
    let code = redirect
        .query_pairs()
        .find(|(key, _)| key == "code")
        .ok_or("authorization code missing")?
        .1
        .into_owned();
    let mut exchange = vec![
        ("grant_type", "authorization_code"),
        ("client_id", application),
        ("code", code.as_str()),
        ("redirect_uri", REDIRECT),
        ("code_verifier", VERIFIER),
    ];
    if let Some(secret) = secret {
        exchange.push(("client_secret", secret));
    }
    let response = post(client, base, "/oauth/token", &exchange).await?;
    let status = response.status();
    let body: Value = serde_json::from_str(&response.text().await?)?;
    assert_eq!(status, 200, "S256 exchange: {body}");
    let token = body["access_token"]
        .as_str()
        .ok_or("missing bearer")?
        .to_owned();
    assert_eq!(bearer_status(client, base, &token).await?, 200);
    Ok(token)
}

#[tokio::test]
#[ignore = "requires tools/mastodon-fixture schema-read-test public_oauth_revocation"]
#[allow(clippy::too_many_lines)]
async fn public_pkce_clients_revoke_only_their_own_tokens() -> TestResult {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let mut connection = <sqlx::PgConnection as sqlx::Connection>::connect(&owner_url).await?;
    rustodon::operational_schema::migrate(&mut connection).await?;
    let writer = WriteRepository::connect(&owner_url).await?;
    // Fixture setup only: seed an authenticated consent session, not a password/login test.
    sqlx::query("INSERT INTO session_activations (user_id, session_id, access_token_id, created_at, updated_at) SELECT 101, $1, id, clock_timestamp(), clock_timestamp() FROM oauth_access_tokens WHERE token = 'fixture-bearer-token-v4-6-5'")
        .bind(SESSION).execute(writer.pool()).await?;
    for (uid, confidential) in [
        ("r14-public-a", false),
        ("r14-public-b", false),
        ("r14-confidential", true),
    ] {
        sqlx::query("INSERT INTO oauth_applications (name, uid, secret, redirect_uri, scopes, confidential, created_at, updated_at) VALUES ($1, $1, $2, $3, 'read', $4, clock_timestamp(), clock_timestamp())")
            .bind(uid).bind(SECRET).bind(REDIRECT).bind(confidential).execute(writer.pool()).await?;
    }
    sqlx::query("INSERT INTO oauth_access_tokens (resource_owner_id, application_id, token, scopes, created_at) VALUES (101, NULL, 'r14-unowned-token', 'read', clock_timestamp())")
        .execute(writer.pool()).await?;
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
    .with_write_repository(writer);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    // Never follow the authorization callback or contact its origin.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let public_a = authorize(&client, &base, "r14-public-a", None).await?;
    let public_b = authorize(&client, &base, "r14-public-b", None).await?;
    let confidential = authorize(&client, &base, "r14-confidential", Some(SECRET)).await?;

    for token in [&public_b, &confidential] {
        let response = post(
            &client,
            &base,
            "/oauth/revoke",
            &[("client_id", "r14-public-a"), ("token", token)],
        )
        .await?;
        assert_eq!(
            response.status(),
            403,
            "public cross-application revocation denied"
        );
        let body: Value = serde_json::from_str(&response.text().await?)?;
        assert_eq!(body["error"], "unauthorized_client");
        assert_eq!(
            bearer_status(&client, &base, token).await?,
            200,
            "victim remains usable"
        );
    }
    for secret in [None, Some(""), Some("wrong-secret")] {
        let mut parameters = vec![
            ("client_id", "r14-confidential"),
            ("token", confidential.as_str()),
        ];
        if let Some(secret) = secret {
            parameters.push(("client_secret", secret));
        }
        assert_eq!(
            post(&client, &base, "/oauth/revoke", &parameters)
                .await?
                .status(),
            403
        );
        assert_eq!(bearer_status(&client, &base, &confidential).await?, 200);
    }
    // A token with no application is not owned by an arbitrary authenticated client.
    let unowned = post(
        &client,
        &base,
        "/oauth/revoke",
        &[
            ("client_id", "r14-confidential"),
            ("client_secret", SECRET),
            ("token", "r14-unowned-token"),
        ],
    )
    .await?
    .status()
    .as_u16();
    let unowned_bearer = bearer_status(&client, &base, "r14-unowned-token").await?;
    let mut actual = Vec::new();
    for (application, token, secret) in [
        ("r14-public-a", public_a.as_str(), None),
        ("r14-public-b", public_b.as_str(), Some("")),
        ("r14-confidential", confidential.as_str(), Some(SECRET)),
    ] {
        let response = if let Some(secret) = secret {
            client
                .post(format!("{base}/oauth/revoke"))
                .header("host", DOMAIN)
                .basic_auth(application, Some(secret))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(form(&[("token", token)]))
                .send()
                .await?
        } else {
            post(
                &client,
                &base,
                "/oauth/revoke",
                &[("client_id", application), ("token", token)],
            )
            .await?
        };
        let status = response.status().as_u16();
        let body: Value = serde_json::from_str(&response.text().await?)?;
        actual.push((
            application,
            status,
            body,
            bearer_status(&client, &base, token).await?,
        ));
    }
    server.abort();
    assert_eq!(
        actual,
        vec![
            ("r14-public-a", 200, serde_json::json!({}), 401),
            ("r14-public-b", 200, serde_json::json!({}), 401),
            ("r14-confidential", 200, serde_json::json!({}), 401),
        ],
        "(client, revocation HTTP, body, subsequent bearer HTTP)"
    );
    assert_eq!((unowned, unowned_bearer), (403, 200));
    Ok(())
}
