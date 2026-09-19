//! Run with the disposable schema-read fixture (see the owning issue).
use super::*;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const TOKEN: &str = "fixture-bearer-token-v4-6-5";
const APP: &str = "fixture-bearer-application-only-v4-6-5";
const WRONG: &str = "fixture-bearer-insufficient-v4-6-5";
const FOLLOW: &str = "fixture-bearer-follow-v4-6-5";
const PUBLIC_READS: &[&str] = &[
    "/api/v1/trends/tags",
    "/api/v1/trends/links",
    "/api/v1/trends/statuses",
    "/api/v1/directory",
    "/api/v1/timelines/link",
    "/api/v1/instance/domain_blocks",
];
const PRIVATE_READS: &[&str] = &[
    "/api/v2/suggestions",
    "/api/v1/domain_blocks",
    "/api/v1/accounts/familiar_followers",
];

#[test]
fn empty_reads_have_explicit_route_contracts() {
    for path in PUBLIC_READS.iter().chain(PRIVATE_READS) {
        let route = api_route(path).expect(path);
        assert_eq!(route.method, ApiMethod::Get);
        assert_eq!(
            route.support,
            if path.ends_with("domain_blocks") {
                ApiRouteSupport::Implemented
            } else {
                ApiRouteSupport::DisabledResponse
            }
        );
        assert_eq!(
            route.pagination,
            if *path == "/api/v1/domain_blocks" {
                PaginationContract::AssociationId
            } else {
                PaginationContract::None
            }
        );
        assert!(api_route_for_method(&Method::POST, path).is_none());
    }
    assert_eq!(
        api_route(PRIVATE_READS[2]).unwrap().authentication,
        ApiAuthentication::Required(&["read", "read:follows"])
    );
}

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

async fn request(
    client: &reqwest::Client,
    base: &str,
    method: Method,
    path: &str,
    token: Option<&str>,
) -> reqwest::Response {
    let mut request = client
        .request(method, format!("{base}{path}"))
        .header("host", DOMAIN)
        .header(ORIGIN, "https://client.example");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    request.send().await.expect("fixture HTTP response")
}

async fn expect_json(response: reqwest::Response, expected: Value, empty: bool) {
    let status = response.status();
    assert_eq!(status, StatusCode::OK, "{}", response.url());
    assert!(
        response.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    if empty {
        assert!(!response.headers().contains_key("link"));
    }
    let text = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), expected);
}

async fn setting(pool: &PgPool, name: &str, value: &str) {
    sqlx::query("DELETE FROM settings WHERE var = $1")
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO settings (var, value, created_at, updated_at) VALUES ($1, $2, now(), now())",
    )
    .bind(name)
    .bind(value)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires isolated schema-read fixture; see meta/issues/return-empty-unimplemented-api-reads.md"]
#[allow(clippy::too_many_lines)]
async fn frontend_empty_reads_preserve_shapes_auth_and_errors() -> TestResult {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let owner = PgPool::connect(&owner_url).await?;
    setting(&owner, "local_topic_feed_access", "public").await;
    setting(&owner, "remote_topic_feed_access", "public").await;
    setting(&owner, "show_domain_blocks", "disabled").await;
    setting(&owner, "show_domain_blocks_rationale", "disabled").await;
    // Enable every scope on the app-only fixture so require_user, not scope failure, is exercised.
    sqlx::query("UPDATE oauth_access_tokens SET scopes = 'read' WHERE token = $1")
        .bind(APP)
        .execute(&owner)
        .await?;
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
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let client = reqwest::Client::new();

    for path in PUBLIC_READS {
        for suffix in [
            "",
            "/",
            "?limit=0&offset=40",
            "?url=https%3A%2F%2Fexample.org",
        ] {
            for token in [None, Some(TOKEN), Some(APP)] {
                expect_json(
                    request(
                        &client,
                        &base,
                        Method::GET,
                        &format!("{path}{suffix}"),
                        token,
                    )
                    .await,
                    json!([]),
                    true,
                )
                .await;
            }
        }
    }
    for path in PRIVATE_READS {
        for suffix in ["", "/"] {
            let path = format!("{path}{suffix}");
            for (token, expected) in [
                (None, 401),
                (Some(WRONG), 403),
                (Some(APP), 422),
                (Some("fixture-bearer-revoked-v4-6-5"), 401),
                (Some("fixture-bearer-expired-v4-6-5"), 401),
                (Some("fixture-bearer-disabled-user-v4-6-5"), 403),
            ] {
                for method in [Method::GET, Method::HEAD] {
                    let response = request(&client, &base, method, &path, token).await;
                    assert_eq!(response.status().as_u16(), expected, "{path} {token:?}");
                }
            }
        }
    }
    for path in PUBLIC_READS.iter().chain(PRIVATE_READS) {
        for suffix in ["", "/"] {
            let path = format!("{path}{suffix}");
            let response = request(&client, &base, Method::HEAD, &path, Some(TOKEN)).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert!(
                response.headers()[CONTENT_TYPE]
                    .to_str()?
                    .starts_with("application/json")
            );
            assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
            assert!(response.bytes().await?.is_empty());
            for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
                assert_eq!(
                    request(&client, &base, method, &path, Some(TOKEN))
                        .await
                        .status(),
                    StatusCode::NOT_FOUND,
                    "{path}"
                );
            }
        }
    }
    expect_json(
        request(
            &client,
            &base,
            Method::GET,
            "/api/v2/suggestions",
            Some(TOKEN),
        )
        .await,
        json!([]),
        true,
    )
    .await;
    expect_json(
        request(
            &client,
            &base,
            Method::GET,
            "/api/v1/accounts/familiar_followers/?id[]=123&id[]=456",
            Some(TOKEN),
        )
        .await,
        json!([{"id":"123","accounts":[]},{"id":"456","accounts":[]}]),
        true,
    )
    .await;
    for query in ["", "?id=123", "?id[]=123&id[]=123"] {
        let expected = if query.is_empty() {
            json!([])
        } else {
            json!([{"id":"123","accounts":[]}])
        };
        expect_json(
            request(
                &client,
                &base,
                Method::GET,
                &format!("/api/v1/accounts/familiar_followers{query}"),
                Some(TOKEN),
            )
            .await,
            expected,
            true,
        )
        .await;
    }
    assert_eq!(
        request(&client, &base, Method::GET, PRIVATE_READS[2], Some(FOLLOW))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &client,
            &base,
            Method::GET,
            "/api/v1/timelines/link",
            Some(WRONG)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    // Public discovery reads do not require scopes, including with an app token.
    for path in &PUBLIC_READS[..4] {
        expect_json(
            request(&client, &base, Method::GET, path, Some(WRONG)).await,
            json!([]),
            true,
        )
        .await;
    }
    for path in [
        "/api/v1/unknown",
        "/api/v1/accounts/999999999999999999",
        "/api/v1/statuses/999999999999999999",
        "/api/v1/tags/unknown",
        "/api/v1/accounts/123/endorsements",
    ] {
        assert_eq!(
            request(&client, &base, Method::GET, path, Some(TOKEN))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    // Do not erase actual user moderation data. Cursors still return genuine empty pages.
    for token in [TOKEN, FOLLOW] {
        expect_json(
            request(
                &client,
                &base,
                Method::GET,
                "/api/v1/domain_blocks",
                Some(token),
            )
            .await,
            json!(["account-blocked.fixture.invalid"]),
            false,
        )
        .await;
    }
    expect_json(
        request(
            &client,
            &base,
            Method::GET,
            "/api/v1/domain_blocks?max_id=9504",
            Some(TOKEN),
        )
        .await,
        json!([]),
        true,
    )
    .await;
    expect_json(
        request(
            &client,
            &base,
            Method::GET,
            "/api/v1/domain_blocks?limit=0",
            Some(TOKEN),
        )
        .await,
        json!([]),
        true,
    )
    .await;

    // An unpublished instance block list is disabled, not proof that moderation is empty.
    sqlx::query(
        "UPDATE domain_blocks SET severity = 1, public_comment = 'Public reason' WHERE id = 9602",
    )
    .execute(&owner)
    .await?;
    setting(&owner, "show_domain_blocks", "users").await;
    expect_json(
        request(&client, &base, Method::GET, PUBLIC_READS[5], None).await,
        json!([]),
        true,
    )
    .await;
    expect_json(
        request(&client, &base, Method::GET, PUBLIC_READS[5], Some(APP)).await,
        json!([]),
        true,
    )
    .await;
    let response = request(&client, &base, Method::GET, PUBLIC_READS[5], Some(TOKEN)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "private, no-store");
    let body: Value = serde_json::from_str(&response.text().await?)?;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_ne!(body[0]["domain"], "blocked.fixture.invalid");
    assert!(body[0]["domain"].as_str().unwrap().contains('*'));
    assert_eq!(body[0]["severity"], "suspend");
    assert_eq!(body[0]["comment"], Value::Null);
    assert_eq!(body[0]["digest"].as_str().unwrap().len(), 64);
    assert!(body[0].get("private_comment").is_none());
    setting(&owner, "show_domain_blocks", "all").await;
    setting(&owner, "show_domain_blocks_rationale", "users").await;
    for token in [None, Some(APP)] {
        expect_json(
            request(&client, &base, Method::GET, PUBLIC_READS[5], token).await,
            body.clone(),
            true,
        )
        .await;
    }
    let mut with_reason = body.clone();
    with_reason[0]["comment"] = json!("Public reason");
    expect_json(
        request(&client, &base, Method::GET, PUBLIC_READS[5], Some(TOKEN)).await,
        with_reason,
        true,
    )
    .await;
    setting(&owner, "show_domain_blocks_rationale", "all").await;
    let mut published = body;
    published[0]["comment"] = json!("Public reason");
    expect_json(
        request(&client, &base, Method::GET, PUBLIC_READS[5], None).await,
        published,
        true,
    )
    .await;
    expect_json(
        request(
            &client,
            &base,
            Method::GET,
            &format!("/api/v2/suggestions/?access_token={TOKEN}"),
            None,
        )
        .await,
        json!([]),
        true,
    )
    .await;
    // Topic feed visibility continues to require a user even for the disabled link read.
    setting(&owner, "local_topic_feed_access", "authenticated").await;
    assert_eq!(
        request(&client, &base, Method::GET, "/api/v1/timelines/link", None)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request(
            &client,
            &base,
            Method::HEAD,
            "/api/v1/timelines/link/",
            Some(APP)
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    expect_json(
        request(
            &client,
            &base,
            Method::GET,
            "/api/v1/timelines/link",
            Some(TOKEN),
        )
        .await,
        json!([]),
        true,
    )
    .await;
    // Publishing uses functional_or_moved?, unlike mutation require_user!.
    setting(&owner, "show_domain_blocks", "users").await;
    setting(&owner, "show_domain_blocks_rationale", "users").await;
    sqlx::query("UPDATE accounts SET moved_to_account_id = (SELECT id FROM accounts WHERE domain IS NOT NULL LIMIT 1) WHERE id = (SELECT account_id FROM users WHERE id = 101)")
        .execute(&owner).await?;
    let response = request(&client, &base, Method::GET, PUBLIC_READS[5], Some(TOKEN)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let moved: Value = serde_json::from_str(&response.text().await?)?;
    assert_eq!(moved[0]["comment"], "Public reason");
    sqlx::query("UPDATE users SET disabled = true WHERE id = 101")
        .execute(&owner)
        .await?;
    expect_json(
        request(&client, &base, Method::GET, PUBLIC_READS[5], Some(TOKEN)).await,
        json!([]),
        true,
    )
    .await;
    setting(&owner, "show_domain_blocks", "all").await;
    let mut no_reason = moved;
    no_reason[0]["comment"] = Value::Null;
    expect_json(
        request(&client, &base, Method::GET, PUBLIC_READS[5], Some(TOKEN)).await,
        no_reason,
        true,
    )
    .await;
    server.abort();
    Ok(())
}
