//! Pinned web settings contract; disposable NAS fixture only.
use super::*;
use serde_json::{Value, json};
use std::error::Error;

const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const TOKEN: &str = "fixture-bearer-token-v4-6-5";
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

fn request(
    client: &reqwest::Client,
    base: &str,
    method: Method,
    csrf: &str,
    session: Option<&str>,
    token: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut request = client
        .request(method, format!("{base}/api/web/settings"))
        .header("host", DOMAIN)
        .header(
            "cookie",
            format!(
                "{SECURE_BROWSER_CSRF_COOKIE}={csrf}; {BROWSER_SESSION_COOKIE}={}",
                session.unwrap_or_default()
            ),
        )
        .header("x-csrf-token", csrf)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    request
}

async fn expect(request: reqwest::RequestBuilder, status: StatusCode, body: Value) -> TestResult {
    let response = request.send().await?;
    let actual_status = response.status();
    assert!(
        actual_status == status,
        "expected {status}, got {actual_status}: {}",
        response.text().await?
    );
    assert!(
        response.headers()[CONTENT_TYPE]
            .to_str()?
            .starts_with("application/json")
    );
    assert!(
        response.headers()[CACHE_CONTROL]
            .to_str()?
            .contains("private")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response.text().await?)?,
        body
    );
    Ok(())
}

async fn bootstrap(
    client: &reqwest::Client,
    base: &str,
    session: Option<&str>,
) -> TestResult<Value> {
    let response = client
        .get(format!("{base}/"))
        .header("host", DOMAIN)
        .header(
            "cookie",
            format!("{BROWSER_SESSION_COOKIE}={}", session.unwrap_or_default()),
        )
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let html = response.text().await?;
    let script = html
        .split("<script id=\"initial-state\"")
        .nth(1)
        .ok_or("missing bootstrap")?;
    Ok(serde_json::from_str(
        script
            .split_once('>')
            .ok_or("missing script")?
            .1
            .split("</script>")
            .next()
            .ok_or("missing script end")?,
    )?)
}

#[test]
fn web_settings_route_inventory_is_private_and_write_only() {
    for method in [Method::PUT, Method::PATCH] {
        let route = api_route_for_method(&method, "/api/web/settings").expect("inventoried");
        assert_eq!(route.support, ApiRouteSupport::Implemented);
        assert_eq!(
            route.authentication,
            ApiAuthentication::Optional(NO_SCOPE.as_slice())
        );
        assert_eq!(route.cache, ApiCachePolicy::Private);
    }
    for method in [Method::GET, Method::POST, Method::DELETE] {
        assert!(api_route_for_method(&method, "/api/web/settings").is_none());
    }
}

#[tokio::test]
#[ignore = "requires isolated NAS schema-read-test fixture; see web settings issue"]
#[allow(clippy::too_many_lines)]
async fn web_settings_persist_frontend_snapshot() -> TestResult {
    let owner = PgPool::connect(&std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?).await?;
    let mut connection = owner.acquire().await?;
    crate::operational_schema::migrate(&mut connection).await?;
    drop(connection);
    let reader_url = std::env::var("RUSTODON_MASTODON_DATABASE_URL")?;
    let repository = Repository::connect(&reader_url).await?;
    let writer =
        WriteRepository::connect(&std::env::var("RUSTODON_MASTODON_WRITER_DATABASE_URL")?).await?;
    let (user_id, account_id): (i64, i64) = sqlx::query_as("SELECT u.id, u.account_id FROM users u JOIN oauth_access_tokens t ON t.resource_owner_id = u.id WHERE t.token = $1")
        .bind(TOKEN).fetch_one(&owner).await?;
    let (other_user, other_account): (i64, i64) = sqlx::query_as("SELECT u.id, u.account_id FROM users u JOIN accounts a ON a.id = u.account_id WHERE a.username = 'api_moderator' AND a.domain IS NULL")
        .fetch_one(&owner).await?;
    assert_ne!(user_id, other_user);
    sqlx::query("DELETE FROM web_settings WHERE user_id IN ($1, $2)")
        .bind(user_id)
        .bind(other_user)
        .execute(&owner)
        .await?;
    sqlx::query("INSERT INTO session_activations (user_id, access_token_id, session_id, created_at, updated_at) SELECT resource_owner_id, id, 'web-settings-session', now(), now() FROM oauth_access_tokens WHERE token = $1")
        .bind(TOKEN).execute(&owner).await?;
    sqlx::query("INSERT INTO oauth_access_tokens (resource_owner_id, token, scopes, created_at) VALUES ($1, 'web-settings-other', '', now())")
        .bind(other_user).execute(&owner).await?;
    let original_preferences: Option<String> =
        sqlx::query_scalar("SELECT settings FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&owner)
            .await?;
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
    let client = reqwest::Client::new();
    let session = Some("web-settings-session");
    // Obtain the actual bootstrap CSRF cookie/meta pair, just like the pinned client.
    // HTTPS host semantics are retained while the disposable listener uses loopback HTTP.
    let page = client
        .get(format!("{base}/"))
        .header("host", DOMAIN)
        .header("cookie", "_mastodon_session=web-settings-session")
        .send()
        .await?;
    let csrf = page
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| value.strip_prefix("__Host-csrf_token="))
        .and_then(|value| value.split(';').next())
        .ok_or("missing csrf cookie")?
        .to_owned();
    assert!(
        page.text()
            .await?
            .contains(&format!("name=\"csrf-token\" content=\"{csrf}\""))
    );
    assert_eq!(
        bootstrap(&client, &base, session).await?["settings"],
        json!({})
    );
    let data = json!({"onboarded": true, "skinTone": 3, "columns": [{"id": "HOME", "params": {}}], "arbitrary": [false, null, 1.5, "</script><script>bad()</script>"]});
    expect(
        request(&client, &base, Method::PUT, &csrf, session, Some(TOKEN))
            .body(json!({"data": data, "user_id": other_user}).to_string()),
        StatusCode::OK,
        json!({}),
    )
    .await?;
    let repository = Repository::connect(&reader_url).await?; // fresh pool, not an in-process cache
    assert_eq!(repository.web_settings(user_id, account_id).await?, data);
    assert_eq!(
        repository.web_settings(other_user, other_account).await?,
        json!({})
    );
    assert_eq!(bootstrap(&client, &base, session).await?["settings"], data);
    assert_eq!(
        bootstrap(&client, &base, None).await?["settings"],
        json!({})
    );
    // PATCH is also whole-document replacement, not a deep merge. Form strings survive.
    expect(
        request(&client, &base, Method::PATCH, &csrf, session, None)
            .headers(HeaderMap::from_iter([(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            )]))
            .body("data[onboarded]=true"),
        StatusCode::OK,
        json!({}),
    )
    .await?;
    let replacement = json!({"onboarded": "true"});
    assert_eq!(
        repository.web_settings(user_id, account_id).await?,
        replacement
    );
    assert_eq!(
        bootstrap(&client, &base, session).await?["settings"],
        replacement
    );
    // A bearer with no scopes is sufficient, and takes priority over the browser cookie.
    expect(
        request(
            &client,
            &base,
            Method::PUT,
            &csrf,
            session,
            Some("web-settings-other"),
        )
        .body(json!({"data": {"other": true}}).to_string()),
        StatusCode::OK,
        json!({}),
    )
    .await?;
    assert_eq!(
        repository.web_settings(other_user, other_account).await?,
        json!({"other": true})
    );
    for token in [None, Some("fixture-bearer-application-only-v4-6-5")] {
        expect(
            request(&client, &base, Method::PUT, &csrf, None, token).body("{\"data\":{}}"),
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error": "This method requires an authenticated user"}),
        )
        .await?;
    }
    for (cookie, attempt) in [(&csrf[..], ""), ("", &csrf[..]), ("forged", "forged")] {
        expect(request(&client, &base, Method::PUT, &csrf, session, Some(TOKEN))
            .headers(HeaderMap::from_iter([(COOKIE, HeaderValue::from_str(&format!("{SECURE_BROWSER_CSRF_COOKIE}={cookie}; {BROWSER_SESSION_COOKIE}=web-settings-session"))?), (http::header::HeaderName::from_static("x-csrf-token"), HeaderValue::from_str(attempt)?)])).body("{\"data\":{}}"), StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error": "Can't verify CSRF token authenticity."})).await?;
    }
    let invalid = request(&client, &base, Method::PUT, &csrf, session, Some("invalid"))
        .body("{\"data\":{}}")
        .send()
        .await?;
    assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
    assert!(serde_json::from_str::<Value>(&invalid.text().await?)?["error"].is_string());
    for body in [
        "{}",
        "{\"data\":null}",
        "{\"data\":true}",
        "{\"data\":[]}",
        "{\"data\":42}",
        "{\"data\":\"oops\"}",
    ] {
        expect(
            request(&client, &base, Method::PUT, &csrf, session, Some(TOKEN)).body(body),
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"error": "data must be an object"}),
        )
        .await?;
    }
    let malformed = request(&client, &base, Method::PUT, &csrf, session, Some(TOKEN))
        .body("{invalid")
        .send()
        .await?;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        serde_json::from_str::<Value>(&malformed.text().await?)?,
        json!({"status":400,"error":"Bad Request"})
    );
    expect(
        request(&client, &base, Method::PUT, &csrf, session, Some(TOKEN))
            .body("x".repeat(PUBLIC_REQUEST_BODY_LIMIT_BYTES + 1)),
        StatusCode::PAYLOAD_TOO_LARGE,
        json!({"error":"Payload Too Large"}),
    )
    .await?;
    // The streaming path enforces the same bound without Content-Length.
    let chunk = Bytes::from(vec![b'x'; PUBLIC_REQUEST_BODY_LIMIT_BYTES + 1]);
    let stream = futures_util::stream::once(async { Ok::<_, std::io::Error>(chunk) });
    expect(
        request(&client, &base, Method::PUT, &csrf, session, Some(TOKEN))
            .body(reqwest::Body::wrap_stream(stream)),
        StatusCode::PAYLOAD_TOO_LARGE,
        json!({"error":"Payload Too Large"}),
    )
    .await?;
    for (mutation, restore, code) in [
        (
            "UPDATE users SET confirmed_at = NULL WHERE id = $1",
            "UPDATE users SET confirmed_at = now() WHERE id = $1",
            StatusCode::FORBIDDEN,
        ),
        (
            "UPDATE users SET approved = false WHERE id = $1",
            "UPDATE users SET approved = true WHERE id = $1",
            StatusCode::FORBIDDEN,
        ),
        (
            "UPDATE users SET disabled = true WHERE id = $1",
            "UPDATE users SET disabled = false WHERE id = $1",
            StatusCode::FORBIDDEN,
        ),
        (
            "UPDATE oauth_access_tokens SET revoked_at = now() WHERE resource_owner_id = $1",
            "UPDATE oauth_access_tokens SET revoked_at = NULL WHERE resource_owner_id = $1",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "UPDATE oauth_access_tokens SET expires_in = -1 WHERE resource_owner_id = $1",
            "UPDATE oauth_access_tokens SET expires_in = NULL WHERE resource_owner_id = $1",
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        sqlx::query(mutation).bind(user_id).execute(&owner).await?;
        let response = request(&client, &base, Method::PUT, &csrf, session, Some(TOKEN))
            .body("{\"data\":{}}")
            .send()
            .await?;
        assert_eq!(response.status(), code);
        assert!(serde_json::from_str::<Value>(&response.text().await?)?["error"].is_string());
        sqlx::query(restore).bind(user_id).execute(&owner).await?;
    }
    for method in [Method::GET, Method::POST, Method::DELETE] {
        let response = request(&client, &base, method, &csrf, session, Some(TOKEN))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    assert_eq!(
        repository.web_settings(user_id, account_id).await?,
        replacement
    );
    let preferences: Option<String> =
        sqlx::query_scalar("SELECT settings FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&owner)
            .await?;
    assert_eq!(preferences, original_preferences);
    // Mismatched identities and bad shapes cannot write even via the writer API.
    assert!(matches!(
        writer
            .update_web_settings(other_user, account_id, &json!({}))
            .await,
        Err(WriteError::NotFound)
    ));
    assert!(matches!(
        writer
            .update_web_settings(user_id, account_id, &json!([]))
            .await,
        Err(WriteError::InvalidInput(_))
    ));
    sqlx::query("DELETE FROM web_settings WHERE user_id = $1")
        .bind(other_user)
        .execute(&owner)
        .await?;
    let first = json!({"first": [1,2,3]});
    let second = json!({"second": [4,5,6]});
    let (a, b) = tokio::join!(
        writer.update_web_settings(other_user, other_account, &first),
        writer.update_web_settings(other_user, other_account, &second)
    );
    a?;
    b?;
    let stored = repository.web_settings(other_user, other_account).await?;
    assert!(stored == first || stored == second);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM web_settings WHERE user_id = $1")
        .bind(other_user)
        .fetch_one(&owner)
        .await?;
    assert_eq!(count, 1);
    // Column-level grants forbid deleting snapshots or changing ownership/creation identity.
    for sql in [
        "DELETE FROM web_settings",
        "UPDATE web_settings SET user_id = user_id",
        "UPDATE web_settings SET created_at = now()",
        "UPDATE web_settings SET id = id",
    ] {
        assert!(
            sqlx::query(sql).execute(writer.pool()).await.is_err(),
            "{sql}"
        );
    }
    let reader = PgPool::connect(&reader_url).await?;
    assert!(
        sqlx::query("UPDATE web_settings SET data = '{}'::json")
            .execute(&reader)
            .await
            .is_err()
    );
    // A stale browser session is not a way around bearer/session expiry checks.
    sqlx::query("UPDATE session_activations SET updated_at = now() - INTERVAL '31 days' WHERE session_id = 'web-settings-session'").execute(&owner).await?;
    expect(
        request(&client, &base, Method::PUT, &csrf, session, None).body("{\"data\":{}}"),
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({"error":"This method requires an authenticated user"}),
    )
    .await?;
    sqlx::query("UPDATE session_activations SET updated_at = now() WHERE session_id = 'web-settings-session'").execute(&owner).await?;
    // A database failure must be an error, not a successful discarded write.
    sqlx::query("REVOKE UPDATE (data) ON public.web_settings FROM rustodon_differential_writer")
        .execute(&owner)
        .await?;
    expect(
        request(&client, &base, Method::PUT, &csrf, session, Some(TOKEN)).body("{\"data\":{}}"),
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"error":"Internal Server Error"}),
    )
    .await?;
    sqlx::query("GRANT UPDATE (data) ON public.web_settings TO rustodon_differential_writer")
        .execute(&owner)
        .await?;
    assert_eq!(
        repository.web_settings(user_id, account_id).await?,
        replacement
    );
    // Empty snapshots are real clearing writes, not fake success.
    expect(
        request(&client, &base, Method::PUT, &csrf, session, Some(TOKEN)).body("{\"data\":{}}"),
        StatusCode::OK,
        json!({}),
    )
    .await?;
    assert_eq!(
        bootstrap(&client, &base, session).await?["settings"],
        json!({})
    );
    server.abort();
    Ok(())
}
