//! HTTP contracts for instance descriptions and limited-mode access.
//!
//! Oracle: parent-extracted cached Mastodon 4.6.5 image, digest
//! 696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf,
//! pinned-extended-description-source.log. No upstream HTTP comparison is claimed.
//! Run only on a disposable, guarded NAS fixture, sequentially with settings tests.

use std::error::Error;
use std::fmt::Debug;
use std::path::Path;

use reqwest::Method;
use reqwest::header::{
    ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_REQUEST_METHOD,
    AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST, HeaderMap, HeaderValue, ORIGIN, VARY,
};
use rustodon::mastodon::Repository;
use rustodon::web::{
    API_ROUTE_INVENTORY, ApiAuthentication, ApiMethod, ApiRouteSupport, PaginationContract,
    WebState, router,
};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use url::Url;

use super::comparison::CapturedResponse;
use super::harness::{RequestSpec, send_single};
use super::safety::DifferentialConfig;

const ENDPOINT: &str = "/api/v1/instance/extended_description";
const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const TIMESTAMP: &str = "2024-11-28T16:20:00+00:00";
const PUBLIC_CACHE: &str = "max-age=300, public, stale-while-revalidate=30, stale-if-error=86400";
type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[test]
fn extended_description_route_is_implemented_public_instance_read() {
    let route = API_ROUTE_INVENTORY
        .iter()
        .find(|route| route.path == ENDPOINT && route.method == ApiMethod::Get)
        .expect("the browser /about startup endpoint must be inventoried");
    assert_eq!(route.support, ApiRouteSupport::Implemented);
    // Match the intentionally public v1/v2 instance and rules inventory. The
    // handler applies Instances::BaseController's conditional limited-mode gate.
    assert_eq!(route.authentication, ApiAuthentication::Public);
    assert_eq!(route.pagination, PaginationContract::None);
}

pub(crate) async fn extended_description_http_contract() -> TestResult {
    let config =
        DifferentialConfig::from_process_environment(Path::new(env!("CARGO_MANIFEST_DIR")))?;
    config.validate_database_comments().await?;
    let owner_url = config
        .rust_owner_database
        .as_ref()
        .ok_or("extended-description setup needs the guarded Rust fixture owner URL")?
        .url();
    let mut owner = PgConnection::connect(owner_url).await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let state = WebState::new(
        repository,
        Url::parse(&format!("https://{DOMAIN}/"))?,
        DOMAIN,
        "/system",
        config.rust_media.clone(),
        crate::fixture_instance_runtime(),
        Vec::new(),
        vec![DOMAIN.to_owned()],
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = Url::parse(&format!("http://{}/", listener.local_addr()?))?;
    // Only this admin-controlled setting is changed; the serving pool remains read-only.
    sqlx::query(
        "CREATE TEMP TABLE extended_description_backup AS \
         SELECT * FROM settings WHERE var = 'site_extended_description'",
    )
    .execute(&mut owner)
    .await?;
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let result = run_cases(&mut owner, &base).await;
    server.abort();
    let _ = server.await;
    // Restore on assertion failures too: checks return errors rather than panic.
    let restored = restore_setting(&mut owner).await;
    restored?;
    result
}

pub(crate) async fn extended_description_limited_mode_contract() -> TestResult {
    let config =
        DifferentialConfig::from_process_environment(Path::new(env!("CARGO_MANIFEST_DIR")))?;
    config.validate_database_comments().await?;
    let mut runtime = crate::fixture_instance_runtime();
    runtime.limited_federation = true;
    let owner_url = config
        .rust_owner_database
        .as_ref()
        .ok_or("limited-mode fixture setup needs the guarded owner URL")?
        .url();
    let mut owner = PgConnection::connect(owner_url).await?;
    let state = WebState::new(
        Repository::connect(config.rust_database.url()).await?,
        Url::parse(&format!("https://{DOMAIN}/"))?,
        DOMAIN,
        "/system",
        config.rust_media.clone(),
        runtime,
        Vec::new(),
        vec![DOMAIN.to_owned()],
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = Url::parse(&format!("http://{}/", listener.local_addr()?))?;
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let result: TestResult = async {
        for token in [
            None,
            Some("invalid-token"),
            Some("fixture-bearer-revoked-v4-6-5"),
            Some("fixture-bearer-expired-v4-6-5"),
            Some("fixture-bearer-application-only-v4-6-5"),
        ] {
            let get = request(&base, Method::GET, ENDPOINT, token).await?;
            equal(get.status, 401, "limited instance requires a user")?;
            equal(
                serde_json::from_slice::<Value>(&get.body)?,
                json!({"error": "This method requires an authenticated user"}),
                "limited instance authentication error",
            )?;
            equal(
                get.headers
                    .get(CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok()),
                Some("private, no-store"),
                "authentication errors must not be shared",
            )?;
            let head = request(&base, Method::HEAD, ENDPOINT, token).await?;
            equal(head.status, 401, "limited HEAD requires a user")?;
            equal(head.body.is_empty(), true, "limited HEAD has no body")?;
        }
        let get = request(
            &base,
            Method::GET,
            ENDPOINT,
            Some("fixture-bearer-insufficient-v4-6-5"),
        )
        .await?;
        equal(
            get.status,
            200,
            "limited instance does not require an OAuth read scope",
        )?;
        // Pinned show explicitly calls cache_even_if_authenticated!, even in limited mode.
        public_headers(&get)?;
        limited_session_cases(&mut owner, &base).await?;
        let options = request(&base, Method::OPTIONS, ENDPOINT, None).await?;
        equal(
            options.status,
            200,
            "limited preflight remains unauthenticated",
        )?;
        Ok(())
    }
    .await;
    server.abort();
    let _ = server.await;
    result
}

async fn limited_session_cases(owner: &mut PgConnection, base: &Url) -> TestResult {
    const ACTIVE: &str = "rustodon-description-active";
    let collision: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM session_activations WHERE session_id LIKE 'rustodon-description-%') \
         OR EXISTS (SELECT 1 FROM oauth_access_tokens WHERE token LIKE 'rustodon-description-%')",
    ).fetch_one(&mut *owner).await?;
    equal(
        collision,
        false,
        "description test identities must not already exist",
    )?;
    let mut transaction = owner.begin().await?;
    sqlx::raw_sql(
        "CREATE TEMP TABLE description_account_backup AS \
         SELECT id, suspended_at, moved_to_account_id FROM accounts WHERE id = -323; \
         INSERT INTO oauth_access_tokens (id, resource_owner_id, application_id, token, scopes, created_at) \
         SELECT (SELECT LEAST(COALESCE(MIN(id), 0), 0) FROM oauth_access_tokens) - row_number() OVER (), \
                user_id, 301, token, '', clock_timestamp() \
         FROM (VALUES (105, 'rustodon-description-pending'), (106, 'rustodon-description-unconfirmed')) AS v(user_id, token); \
         INSERT INTO session_activations (id, user_id, access_token_id, session_id, created_at, updated_at) \
         SELECT (SELECT LEAST(COALESCE(MIN(id), 0), 0) FROM session_activations) - row_number() OVER (), \
                101, token_id, session_id, clock_timestamp(), clock_timestamp() - age \
         FROM (VALUES ('rustodon-description-active', 401, INTERVAL '0 days'), \
                      ('rustodon-description-expired', 401, INTERVAL '31 days'), \
                      ('rustodon-description-revoked', 405, INTERVAL '0 days')) AS v(session_id, token_id, age);",
    ).execute(&mut *transaction).await?;
    transaction.commit().await?;
    let result: TestResult = async {
        for token in [None, Some("invalid-token"), Some("fixture-bearer-application-only-v4-6-5"),
            Some("fixture-bearer-revoked-v4-6-5"), Some("fixture-bearer-expired-v4-6-5")] {
            let response = request_with_session(base, Method::GET, ENDPOINT, token, Some(ACTIVE)).await?;
            equal(response.status, 200, "functional session fallback")?;
            public_headers(&response)?;
        }
        for session in ["rustodon-description-expired", "rustodon-description-revoked"] {
            let response = request_with_session(base, Method::GET, ENDPOINT, None, Some(session)).await?;
            equal(response.status, 401, "expired/revoked sessions cannot authenticate")?;
        }
        for token in ["fixture-bearer-disabled-user-v4-6-5", "fixture-bearer-missing-2fa-v4-6-5",
            "rustodon-description-pending", "rustodon-description-unconfirmed"] {
            for session in [None, Some(ACTIVE)] {
                let response = request_with_session(base, Method::GET, ENDPOINT, Some(token), session).await?;
                equal(response.status, 403, "nonfunctional bearer takes precedence over session")?;
            }
        }
        for mutation in [
            "UPDATE accounts SET suspended_at = clock_timestamp(), moved_to_account_id = NULL WHERE id = -323",
            "UPDATE accounts SET suspended_at = NULL, moved_to_account_id = 116844606259201001 WHERE id = -323",
        ] {
            sqlx::query(mutation).execute(&mut *owner).await?;
            let response = request_with_session(base, Method::GET, ENDPOINT,
                Some("fixture-bearer-matrix-viewer-v4-6-5"), Some(ACTIVE)).await?;
            equal(response.status, 403, "suspended/moved bearer cannot use another user's session")?;
        }
        Ok(())
    }.await;
    // Only these temporary identities and two original account fields are touched.
    // Restore after ordinary test failures before propagating their diagnostics.
    let mut transaction = owner.begin().await?;
    sqlx::raw_sql(
        "UPDATE accounts a SET suspended_at = b.suspended_at, moved_to_account_id = b.moved_to_account_id \
         FROM description_account_backup b WHERE a.id = b.id; \
         DELETE FROM session_activations WHERE session_id IN \
            ('rustodon-description-active', 'rustodon-description-expired', 'rustodon-description-revoked'); \
         DELETE FROM oauth_access_tokens WHERE token IN \
            ('rustodon-description-pending', 'rustodon-description-unconfirmed');",
    ).execute(&mut *transaction).await?;
    transaction.commit().await?;
    result
}

#[derive(Clone, Copy)]
enum SettingValue<'a> {
    Absent,
    SqlNull,
    Yaml(&'a str),
}

async fn run_cases(owner: &mut PgConnection, base: &Url) -> TestResult {
    use SettingValue::{Absent, SqlNull, Yaml};

    // Persisted YAML must be decoded, not stripped with a scalar-only parser
    // or rendered as YAML source text.
    for (label, yaml, timestamp, content, updated_at) in [
        ("absent", Absent, Some(TIMESTAMP), "", None),
        ("SQL null", SqlNull, Some(TIMESTAMP), "", None),
        ("YAML null", Yaml("---\n"), Some(TIMESTAMP), "", None),
        ("empty", Yaml("--- ''\n"), Some(TIMESTAMP), "", None),
        (
            "blank",
            Yaml("--- \" \\t\\n\\u00a0\"\n"),
            Some(TIMESTAMP),
            "",
            None,
        ),
        (
            "pinned serializer example",
            Yaml("--- Hello world\n"),
            Some(TIMESTAMP),
            "<p>Hello world</p>\n",
            Some(TIMESTAMP),
        ),
        (
            "nullable timestamp",
            Yaml("--- Hello world\n"),
            None,
            "<p>Hello world</p>\n",
            None,
        ),
        (
            "whole-second ISO8601",
            Yaml("--- Hello world\n"),
            Some("2024-11-28T16:20:00.987654+00:00"),
            "<p>Hello world</p>\n",
            Some(TIMESTAMP),
        ),
        (
            "literal block Markdown",
            Yaml("--- |\n  ## About\n\n  Hello **world** & friends.\n"),
            Some(TIMESTAMP),
            "<h2>About</h2>\n\n<p>Hello <strong>world</strong> &amp; friends.</p>\n",
            Some(TIMESTAMP),
        ),
        (
            "quoted Markdown and trusted inline HTML",
            Yaml("--- '[Rules](https://example.org/rules) and <em>welcome</em>'\n"),
            Some(TIMESTAMP),
            "<p><a href=\"https://example.org/rules\">Rules</a> and <em>welcome</em></p>\n",
            Some(TIMESTAMP),
        ),
    ] {
        set_setting(owner, yaml, timestamp).await?;
        let response = request(base, Method::GET, ENDPOINT, None).await?;
        equal(response.status, 200, label)?;
        equal(
            serde_json::from_slice::<Value>(&response.body)?,
            json!({"updated_at": updated_at, "content": content}),
            label,
        )?;
        public_headers(&response)?;
    }

    check_public_methods(base).await
}

// Called after run_cases leaves the final populated setting in place.
async fn check_public_methods(base: &Url) -> TestResult {
    // Normal-mode public instance reads ignore bearer identity, including stale
    // credentials, and remain publicly cacheable. HEAD preserves the observable
    // GET status/selected headers while returning no body.
    let expected = json!({
        "updated_at": TIMESTAMP,
        "content": "<p><a href=\"https://example.org/rules\">Rules</a> and <em>welcome</em></p>\n",
    });
    for path in [ENDPOINT.to_owned(), format!("{ENDPOINT}/")] {
        for token in [
            None,
            Some("fixture-bearer-token-v4-6-5"),
            Some("invalid-token"),
        ] {
            let get = request(base, Method::GET, &path, token).await?;
            equal(get.status, 200, "GET public instance description")?;
            equal(
                serde_json::from_slice::<Value>(&get.body)?,
                expected.clone(),
                "GET description is independent of bearer identity and trailing slash",
            )?;
            public_headers(&get)?;
            let head = request(base, Method::HEAD, &path, token).await?;
            equal(head.status, 200, "HEAD public instance description")?;
            equal(head.body.is_empty(), true, "HEAD must not return a body")?;
            for header in [
                CONTENT_TYPE,
                CACHE_CONTROL,
                VARY,
                ACCESS_CONTROL_ALLOW_ORIGIN,
            ] {
                equal(
                    head.headers.get(&header),
                    get.headers.get(&header),
                    header.as_str(),
                )?;
            }
        }
        // Existing global preflight contract, not a new endpoint-specific OPTIONS route.
        let options = request(base, Method::OPTIONS, &path, Some("invalid-token")).await?;
        equal(options.status, 200, "OPTIONS preflight")?;
        equal(options.body.is_empty(), true, "OPTIONS empty body")?;
        equal(
            options
                .headers
                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|v| v.to_str().ok()),
            Some("*"),
            "preflight origin",
        )?;
        equal(
            options
                .headers
                .get(ACCESS_CONTROL_ALLOW_METHODS)
                .and_then(|v| v.to_str().ok()),
            Some("POST, PUT, DELETE, GET, PATCH, OPTIONS"),
            "existing preflight methods",
        )?;
    }
    Ok(())
}

async fn set_setting(
    owner: &mut PgConnection,
    setting: SettingValue<'_>,
    timestamp: Option<&str>,
) -> TestResult {
    let mut transaction = owner.begin().await?;
    sqlx::query("DELETE FROM settings WHERE var = 'site_extended_description'")
        .execute(&mut *transaction)
        .await?;
    let yaml = match setting {
        SettingValue::Absent => {
            transaction.commit().await?;
            return Ok(());
        }
        SettingValue::SqlNull => None,
        SettingValue::Yaml(text) => Some(text),
    };
    // Explicit unused ID avoids advancing fixture sequences.
    sqlx::query(
        "INSERT INTO settings (id, var, value, created_at, updated_at) \
         SELECT LEAST(COALESCE(MIN(id), 0), 0) - 1, 'site_extended_description', $1, \
                TIMESTAMP '2020-01-01 00:00:00', $2::text::timestamptz AT TIME ZONE 'UTC' \
         FROM settings",
    )
    .bind(yaml)
    .bind(timestamp)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn restore_setting(owner: &mut PgConnection) -> TestResult {
    let mut transaction = owner.begin().await?;
    sqlx::query("DELETE FROM settings WHERE var = 'site_extended_description'")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO settings SELECT * FROM extended_description_backup")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

async fn request(
    base: &Url,
    method: Method,
    path: &str,
    token: Option<&str>,
) -> TestResult<CapturedResponse> {
    request_with_session(base, method, path, token, None).await
}

async fn request_with_session(
    base: &Url,
    method: Method,
    path: &str,
    token: Option<&str>,
    session: Option<&str>,
) -> TestResult<CapturedResponse> {
    let mut headers = HeaderMap::new();
    if let Some(session) = session {
        headers.insert(
            reqwest::header::COOKIE,
            HeaderValue::from_str(&format!("_mastodon_session={session}"))?,
        );
    }
    headers.insert(HOST, HeaderValue::from_static(DOMAIN));
    headers.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
    if let Some(token) = token {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
    }
    if method == Method::OPTIONS {
        headers.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("GET"),
        );
    }
    let request = RequestSpec::new(method, path, None, headers, Vec::new())?;
    Ok(send_single(base, &request, "Rust").await?)
}

fn public_headers(response: &CapturedResponse) -> TestResult {
    for (header, expected) in [
        (CONTENT_TYPE, "application/json; charset=utf-8"),
        (CACHE_CONTROL, PUBLIC_CACHE),
        (VARY, "Accept, Origin"),
        (ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
    ] {
        equal(
            response
                .headers
                .get(&header)
                .and_then(|value| value.to_str().ok()),
            Some(expected),
            header.as_str(),
        )?;
    }
    Ok(())
}

// Value arguments keep assertion-style calls readable for temporaries and scalars.
// Return errors rather than panic so fixture cleanup still runs on mismatches.
#[allow(clippy::needless_pass_by_value)]
fn equal<T: PartialEq + Debug>(actual: T, expected: T, label: &str) -> TestResult {
    if actual != expected {
        return Err(format!("{label}: expected {expected:?}, got {actual:?}").into());
    }
    Ok(())
}
