//! Owner-only profile reads on a disposable restricted-role PG14 fixture.
use super::*;
use serde_json::Value;
const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const TOKEN: &str = "fixture-bearer-token-v4-6-5";
type TestResult = Result<(), Box<dyn std::error::Error>>;

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

#[test]
fn profile_route_is_private_read_only() {
    let route = api_route_for_method(&Method::GET, "/api/v1/profile").unwrap();
    assert_eq!(route.support, ApiRouteSupport::Implemented);
    assert_eq!(
        route.authentication,
        ApiAuthentication::Required(VERIFY_CREDENTIALS.as_slice())
    );
    assert!(api_route_for_method(&Method::PATCH, "/api/v1/profile").is_none());
    assert!(api_route_for_method(&Method::PUT, "/api/v1/profile").is_none());
}

#[tokio::test]
#[ignore = "requires disposable restored PG14 with restricted runtime role"]
#[allow(clippy::too_many_lines)]
async fn profile_read_restricted_http() -> TestResult {
    let owner = PgPool::connect(&std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?).await?;
    let repository = Repository::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
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
    let get = |path: &str| client.get(format!("{base}{path}")).header("host", DOMAIN);
    assert_eq!(
        get("/api/v1/profile").send().await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get("/api/v1/profile")
            .header("cookie", "_mastodon_session=invalid")
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get("/api/v1/profile")
            .bearer_auth("invalid-profile-token")
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let (user, account): (i64, i64) = sqlx::query_as("SELECT users.id, account_id FROM users JOIN oauth_access_tokens t ON t.resource_owner_id = users.id WHERE t.token = $1")
        .bind(TOKEN).fetch_one(&owner).await?;
    for (token, scope, resource_owner, status) in [
        (
            "profile-read-profile",
            "profile",
            Some(user),
            StatusCode::OK,
        ),
        ("profile-read-read", "read", Some(user), StatusCode::OK),
        (
            "profile-read-accounts",
            "read:accounts",
            Some(user),
            StatusCode::OK,
        ),
        (
            "profile-read-write",
            "write:accounts",
            Some(user),
            StatusCode::FORBIDDEN,
        ),
        (
            "profile-read-app",
            "read",
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        sqlx::query("INSERT INTO oauth_access_tokens (token, scopes, resource_owner_id, created_at) VALUES ($1,$2,$3,now())")
            .bind(token).bind(scope).bind(resource_owner).execute(&owner).await?;
        for path in ["/api/v1/profile", "/api/v1/profile/"] {
            let response = get(path).bearer_auth(token).send().await?;
            if status == StatusCode::OK {
                assert!(
                    response.headers()[CACHE_CONTROL]
                        .to_str()?
                        .contains("private")
                );
            }
            let actual = response.status();
            let body = response.text().await?;
            assert_eq!(actual, status, "{token} {path}: {body}");
            if status == StatusCode::OK {
                let value: Value = serde_json::from_str(&body)?;
                assert_eq!(value["id"], account.to_string());
                for field in [
                    "note",
                    "formatted_note",
                    "fields",
                    "formatted_fields",
                    "avatar",
                    "avatar_static",
                    "header",
                    "header_static",
                    "hide_collections",
                    "discoverable",
                    "indexable",
                    "show_media",
                    "show_media_replies",
                    "show_featured",
                    "attribution_domains",
                    "featured_tags",
                ] {
                    assert!(value.get(field).is_some(), "{field}");
                }
                assert!(value.get("source").is_none());
                assert!(value.get("email").is_none());
                let tags: Value = serde_json::from_str(
                    &get("/api/v1/featured_tags")
                        .bearer_auth(TOKEN)
                        .send()
                        .await?
                        .text()
                        .await?,
                )?;
                assert_eq!(value["featured_tags"], tags);
                // Parameters cannot redirect the owner-only read to another account.
                let targeted: Value = serde_json::from_str(
                    &get("/api/v1/profile?id=1&account_id=1")
                        .bearer_auth(token)
                        .send()
                        .await?
                        .text()
                        .await?,
                )?;
                assert_eq!(targeted, value);
            }
        }
    }
    for (column, value, restore) in [
        ("disabled", "true", "false"),
        ("approved", "false", "true"),
        ("confirmed_at", "NULL", "now()"),
    ] {
        sqlx::query(&format!(
            "UPDATE users SET {column} = {value} WHERE id = $1"
        ))
        .bind(user)
        .execute(&owner)
        .await?;
        let response = get("/api/v1/profile").bearer_auth(TOKEN).send().await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{column}");
        sqlx::query(&format!(
            "UPDATE users SET {column} = {restore} WHERE id = $1"
        ))
        .bind(user)
        .execute(&owner)
        .await?;
    }
    let other: Value = serde_json::from_str(
        &get("/api/v1/profile")
            .bearer_auth("fixture-bearer-api-moderator-v4-6-5")
            .send()
            .await?
            .text()
            .await?,
    )?;
    assert!(other["id"].is_string(), "{other}");
    assert_ne!(other["id"], account.to_string());
    server.abort();
    owner.close().await;
    Ok(())
}
