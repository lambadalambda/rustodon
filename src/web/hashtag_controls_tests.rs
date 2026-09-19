//! Local hashtag controls; requires disposable PG14 with separate runtime/writer roles.
use super::*;
use serde_json::Value;
use sqlx::Connection;
const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const TOKEN: &str = "fixture-bearer-token-v4-6-5";
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn hashtag_control_routes_are_implemented() {
    for (method, path) in [
        (Method::GET, "/api/v1/tags/rust"),
        (Method::POST, "/api/v1/tags/rust/follow"),
        (Method::POST, "/api/v1/tags/rust/unfollow"),
        (Method::POST, "/api/v1/tags/rust/feature"),
        (Method::POST, "/api/v1/tags/rust/unfeature"),
        (Method::POST, "/api/v1/featured_tags"),
        (Method::DELETE, "/api/v1/featured_tags/42"),
    ] {
        assert_eq!(
            api_route_for_method(&method, path).expect(path).support,
            ApiRouteSupport::Implemented
        );
    }
}

#[test]
fn hashtag_grants_stay_inside_the_grant_transaction() {
    let sql = include_str!("../../docs/mastodon-writer-grants.sql");
    let (transaction, after_commit) = sql.split_once("COMMIT;").unwrap();
    assert!(transaction.contains("BEGIN;"));
    assert!(transaction.contains("GRANT INSERT ON TABLE public.tag_follows"));
    assert!(transaction.contains(
        "GRANT USAGE ON SEQUENCE public.tag_follows_id_seq, public.featured_tags_id_seq"
    ));
    assert!(!after_commit.contains("GRANT "));
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
#[ignore = "requires disposable restored PG14 runtime and writer roles"]
async fn hashtag_controls_persist_through_http() -> TestResult {
    let owner = PgPool::connect(&std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?).await?;
    let repository = Repository::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer =
        WriteRepository::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let writer_url = std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?;
    let writer_role = Url::parse(&writer_url)?.username().to_owned();
    assert!(
        writer_role
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    );
    let mut writer_connection = sqlx::PgConnection::connect(&writer_url).await?;
    crate::preflight::validate_writer_connection(&mut writer_connection).await?;
    for object in [
        "TABLE public.tag_follows",
        "TABLE public.featured_tags",
        "SEQUENCE public.tag_follows_id_seq",
        "SEQUENCE public.featured_tags_id_seq",
    ] {
        let privilege = if object.starts_with("TABLE") {
            "INSERT"
        } else {
            "USAGE"
        };
        sqlx::query(&format!(
            "REVOKE {privilege} ON {object} FROM {writer_role}"
        ))
        .execute(&owner)
        .await?;
        assert!(
            crate::preflight::validate_writer_connection(&mut writer_connection)
                .await
                .is_err(),
            "{object}"
        );
        sqlx::query(&format!("GRANT {privilege} ON {object} TO {writer_role}"))
            .execute(&owner)
            .await?;
    }
    crate::preflight::validate_writer_connection(&mut writer_connection).await?;
    writer_connection.close().await?;
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
    let client = reqwest::Client::new();
    let request = |method, path: &str| {
        client
            .request(method, format!("{base}{path}"))
            .header("host", DOMAIN)
    };
    let response = request(Method::GET, "/api/v1/tags/UnknownLocalSlice")
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let unknown: Value = response_json(response).await;
    assert_eq!(unknown["id"], "");
    assert_eq!(unknown["name"], "UnknownLocalSlice");
    assert!(unknown.get("following").is_none());
    assert_eq!(unknown["history"].as_array().unwrap().len(), 7);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tags WHERE name = 'unknownlocalslice'")
            .fetch_one(&owner)
            .await?,
        0
    );
    for invalid in ["123", "bad%20tag", "%23rust"] {
        assert_eq!(
            request(Method::GET, &format!("/api/v1/tags/{invalid}"))
                .send()
                .await?
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    for action in ["follow", "unfollow", "feature", "unfeature"] {
        assert_eq!(
            request(
                Method::POST,
                &format!("/api/v1/tags/UnknownLocalSlice/{action}")
            )
            .send()
            .await?
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for (action, following) in [
        ("follow", true),
        ("follow", true),
        ("unfollow", false),
        ("unfollow", false),
    ] {
        let response = request(
            Method::POST,
            &format!("/api/v1/tags/Caf%C3%A9Slice/{action}"),
        )
        .bearer_auth(TOKEN)
        .send()
        .await?;
        let status = response.status();
        let body: Value = response_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["following"], following);
        let loaded: Value = response_json(
            request(Method::GET, "/api/v1/tags/CAFESLICE")
                .bearer_auth(TOKEN)
                .send()
                .await?,
        )
        .await;
        assert_eq!(loaded["following"], following);
    }
    let response = request(Method::POST, "/api/v1/featured_tags")
        .bearer_auth(TOKEN)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body("name=+%23Caf%C3%A9Slice+")
        .send()
        .await?;
    let status = response.status();
    let feature: Value = response_json(response).await;
    assert_eq!(status, StatusCode::OK, "{feature}");
    let id = feature["id"].as_str().unwrap();
    let same: Value = response_json(
        request(Method::POST, "/api/v1/featured_tags")
            .bearer_auth(TOKEN)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body("name=Caf%C3%A9Slice")
            .send()
            .await?,
    )
    .await;
    assert_eq!(same["id"], id);
    assert_eq!(
        request(Method::POST, "/api/v1/featured_tags")
            .bearer_auth(TOKEN)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body("name=CAFESLICE")
            .send()
            .await?
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let response = request(Method::POST, "/api/v1/tags/cafeslice/feature")
        .bearer_auth(TOKEN)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["featuring"], true);
    assert_eq!(
        request(Method::DELETE, &format!("/api/v1/featured_tags/{id}"))
            .bearer_auth(TOKEN)
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(Method::DELETE, &format!("/api/v1/featured_tags/{id}"))
            .bearer_auth(TOKEN)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE oauth_access_tokens SET scopes = scopes || ' write' WHERE token = 'fixture-bearer-api-moderator-v4-6-5'").execute(&owner).await?;
    let account: i64 = sqlx::query_scalar("SELECT account_id FROM users JOIN oauth_access_tokens token ON token.resource_owner_id = users.id WHERE token.token = $1")
        .bind(TOKEN).fetch_one(&owner).await?;
    let user: i64 =
        sqlx::query_scalar("SELECT resource_owner_id FROM oauth_access_tokens WHERE token = $1")
            .bind(TOKEN)
            .fetch_one(&owner)
            .await?;
    // Every mutating surface rejects application credentials and wrong scopes.
    for (token, scopes, resource_owner) in [
        ("hashtag-read", "read", Some(user)),
        ("hashtag-follow", "follow", Some(user)),
        ("hashtag-write-follows", "write:follows", Some(user)),
        ("hashtag-write-accounts", "write:accounts", Some(user)),
        ("hashtag-app", "read write follow", None),
    ] {
        sqlx::query("INSERT INTO oauth_access_tokens (token, scopes, resource_owner_id, created_at) VALUES ($1,$2,$3,now())")
            .bind(token).bind(scopes).bind(resource_owner).execute(&owner).await?;
    }
    for (token, follow_ok, feature_ok) in [
        ("hashtag-read", false, false),
        ("hashtag-follow", true, false),
        ("hashtag-write-follows", true, false),
        ("hashtag-write-accounts", false, true),
        ("hashtag-app", false, false),
    ] {
        for (action, allowed) in [
            ("follow", follow_ok),
            ("unfollow", follow_ok),
            ("feature", feature_ok),
            ("unfeature", feature_ok),
        ] {
            let response = request(Method::POST, &format!("/api/v1/tags/ScopeSlice/{action}"))
                .bearer_auth(token)
                .send()
                .await?;
            assert_eq!(
                response.status(),
                if allowed {
                    StatusCode::OK
                } else if token == "hashtag-app" {
                    StatusCode::UNPROCESSABLE_ENTITY
                } else {
                    StatusCode::FORBIDDEN
                },
                "{token} {action}: {}",
                response.text().await?
            );
        }
        let response = request(Method::POST, "/api/v1/featured_tags")
            .bearer_auth(token)
            .header(CONTENT_TYPE, "application/json")
            .body(r#"{"name":"CollectionScopeSlice"}"#)
            .send()
            .await?;
        assert_eq!(
            response.status(),
            if feature_ok {
                StatusCode::OK
            } else if token == "hashtag-app" {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::FORBIDDEN
            }
        );
    }
    let app_tag = response_json(
        request(Method::GET, "/api/v1/tags/cafeslice")
            .bearer_auth("hashtag-app")
            .send()
            .await?,
    )
    .await;
    assert!(app_tag.get("following").is_none());
    sqlx::query("UPDATE accounts SET suspended_at = now() WHERE id = $1")
        .bind(account)
        .execute(&owner)
        .await?;
    for action in ["follow", "unfollow", "feature", "unfeature"] {
        assert_eq!(
            request(
                Method::POST,
                &format!("/api/v1/tags/SuspendedSlice/{action}")
            )
            .bearer_auth(TOKEN)
            .send()
            .await?
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    sqlx::query("UPDATE accounts SET suspended_at = NULL WHERE id = $1")
        .bind(account)
        .execute(&owner)
        .await?;
    let feature = response_json(
        request(Method::POST, "/api/v1/featured_tags")
            .bearer_auth(TOKEN)
            .header(CONTENT_TYPE, "application/json")
            .body(r#"{"name":"OwnedSlice"}"#)
            .send()
            .await?,
    )
    .await;
    let id = feature["id"].as_str().unwrap();
    assert_eq!(
        request(Method::DELETE, &format!("/api/v1/featured_tags/{id}"))
            .bearer_auth("fixture-bearer-api-moderator-v4-6-5")
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM featured_tags WHERE id = $1")
            .bind(id.parse::<i64>()?)
            .fetch_one(&owner)
            .await?,
        1
    );
    // Initialize counts from existing distributable statuses, then use ordinary
    // HTTP create/delete maintenance; private and direct statuses never count.
    let mut statuses = Vec::new();
    for visibility in ["public", "unlisted", "private", "direct"] {
        let response = request(Method::POST, "/api/v1/statuses")
            .bearer_auth(TOKEN)
            .header(CONTENT_TYPE, "application/json")
            .body(
                serde_json::json!({"status":"#CountSlice lifecycle", "visibility":visibility})
                    .to_string(),
            )
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        statuses.push(
            response_json(response).await["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let feature = response_json(
        request(Method::POST, "/api/v1/featured_tags")
            .bearer_auth(TOKEN)
            .header(CONTENT_TYPE, "application/json")
            .body(r#"{"name":"CountSlice"}"#)
            .send()
            .await?,
    )
    .await;
    assert_eq!(feature["statuses_count"], "2");
    assert!(!feature["last_status_at"].is_null());
    let response = request(Method::POST, "/api/v1/statuses")
        .bearer_auth(TOKEN)
        .header(CONTENT_TYPE, "application/json")
        .body(r##"{"status":"#CountSlice added after featuring","visibility":"public"}"##)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let added = response_json(response).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT statuses_count FROM featured_tags WHERE id = $1")
            .bind(feature["id"].as_str().unwrap().parse::<i64>()?)
            .fetch_one(&owner)
            .await?,
        3
    );
    assert_eq!(
        request(Method::PUT, &format!("/api/v1/statuses/{added}"))
            .bearer_auth(TOKEN)
            .header(CONTENT_TYPE, "application/json")
            .body(r#"{"status":"tag removed by edit"}"#)
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT statuses_count FROM featured_tags WHERE id = $1")
            .bind(feature["id"].as_str().unwrap().parse::<i64>()?)
            .fetch_one(&owner)
            .await?,
        2
    );
    let public_features = response_json(
        request(
            Method::GET,
            &format!("/api/v1/accounts/{account}/featured_tags"),
        )
        .send()
        .await?,
    )
    .await;
    assert!(
        public_features
            .as_array()
            .unwrap()
            .iter()
            .any(|tag| tag["id"] == feature["id"])
    );

    let tag = response_json(
        request(Method::GET, "/api/v1/tags/countslice")
            .send()
            .await?,
    )
    .await;
    assert_eq!(tag["history"][0]["uses"], "1");
    assert_eq!(tag["history"][0]["accounts"], "1");
    let feature_id = feature["id"].as_str().unwrap();
    for (index, id) in statuses.iter().take(2).enumerate() {
        assert_eq!(
            request(Method::DELETE, &format!("/api/v1/statuses/{id}"))
                .bearer_auth(TOKEN)
                .send()
                .await?
                .status(),
            StatusCode::OK
        );
        let features = response_json(
            request(Method::GET, "/api/v1/featured_tags")
                .bearer_auth(TOKEN)
                .send()
                .await?,
        )
        .await;
        let feature = features
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == feature_id)
            .unwrap();
        assert_eq!(feature["statuses_count"], (1 - index).to_string());
        if index == 1 {
            assert!(feature["last_status_at"].is_null());
        }
    }
    let history = response_json(
        request(Method::GET, "/api/v1/tags/countslice")
            .send()
            .await?,
    )
    .await;
    assert_eq!(history["history"][0]["uses"], "0");
    // Concurrent distinct names compete for the last slot, not count-then-insert races.
    sqlx::query("DELETE FROM featured_tags WHERE account_id = $1")
        .bind(account)
        .execute(&owner)
        .await?;
    for index in 0..9 {
        assert_eq!(
            request(
                Method::POST,
                &format!("/api/v1/tags/LimitSlice{index}/feature")
            )
            .bearer_auth(TOKEN)
            .send()
            .await?
            .status(),
            StatusCode::OK
        );
    }
    let requests = (9..11).map(|index| {
        request(
            Method::POST,
            &format!("/api/v1/tags/LimitSlice{index}/feature"),
        )
        .bearer_auth(TOKEN)
        .send()
    });
    let results = futures_util::future::join_all(requests).await;
    let codes: Vec<_> = results.into_iter().map(|r| r.unwrap().status()).collect();
    assert_eq!(
        codes.iter().filter(|code| **code == StatusCode::OK).count(),
        1
    );
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == StatusCode::UNPROCESSABLE_ENTITY)
            .count(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM featured_tags WHERE account_id = $1")
            .bind(account)
            .fetch_one(&owner)
            .await?,
        10
    );
    assert_eq!(
        request(Method::POST, "/api/v1/tags/LimitSlice0/feature")
            .bearer_auth(TOKEN)
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    // Repeated concurrent follows remain one row; unfollow never creates unknown tags.
    let requests = (0..2).map(|_| {
        request(Method::POST, "/api/v1/tags/ConcurrentSlice/follow")
            .bearer_auth(TOKEN)
            .send()
    });
    for response in futures_util::future::join_all(requests).await {
        assert_eq!(response?.status(), StatusCode::OK);
    }
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM tag_follows JOIN tags ON tags.id = tag_follows.tag_id WHERE account_id = $1 AND name = 'concurrentslice'").bind(account).fetch_one(&owner).await?, 1);
    for action in ["unfollow", "unfeature"] {
        let tag = response_json(
            request(
                Method::POST,
                &format!("/api/v1/tags/StillUnknownSlice/{action}"),
            )
            .bearer_auth(TOKEN)
            .send()
            .await?,
        )
        .await;
        assert_eq!(tag["id"], "");
    }
    assert_history_boundaries_and_readback(&owner, &client, &base, account).await?;
    sqlx::query("UPDATE rustodon.rate_limit_windows SET attempts = 400 WHERE window_key = $1")
        .bind(format!("follows:{account}"))
        .execute(&owner)
        .await?;
    let limited = request(Method::POST, "/api/v1/tags/RateLimitedSlice/follow")
        .bearer_auth(TOKEN)
        .send()
        .await?;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers()["x-ratelimit-limit"], "400");
    assert_eq!(
        request(Method::POST, "/api/v1/tags/ConcurrentSlice/follow")
            .bearer_auth(TOKEN)
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(Method::POST, "/api/v1/tags/ConcurrentSlice/unfollow")
            .bearer_auth(TOKEN)
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    server.abort();
    Ok(())
}

async fn response_json(response: reqwest::Response) -> Value {
    serde_json::from_str(&response.text().await.unwrap()).unwrap()
}

// Owner credentials seed adversarial timestamps only; the aggregate and HTTP
// mutation are exercised through the restricted runtime/writer connections.
#[allow(clippy::too_many_lines)]
async fn assert_history_boundaries_and_readback(
    owner: &PgPool,
    client: &reqwest::Client,
    base: &str,
    account: i64,
) -> TestResult {
    let others = sqlx::query_scalar::<_, i64>("SELECT id FROM accounts WHERE id > 0 AND id <> $1 AND suspended_at IS NULL AND silenced_at IS NULL ORDER BY id LIMIT 3")
        .bind(account).fetch_all(owner).await?;
    assert_eq!(others.len(), 3);
    sqlx::query("UPDATE accounts SET silenced_at = now() WHERE id = $1")
        .bind(others[1])
        .execute(owner)
        .await?;
    sqlx::query("UPDATE accounts SET suspended_at = now() WHERE id = $1")
        .bind(others[2])
        .execute(owner)
        .await?;
    let tag: i64 = sqlx::query_scalar("INSERT INTO tags (name, created_at, updated_at) VALUES ('historyboundaryslice', now(), now()) RETURNING id")
        .fetch_one(owner).await?;
    let today: chrono::NaiveDateTime =
        sqlx::query_scalar("SELECT date_trunc('day', clock_timestamp() AT TIME ZONE 'UTC')")
            .fetch_one(owner)
            .await?;
    let day_two = today - chrono::Duration::days(2) + chrono::Duration::hours(12);
    let mut rows: Vec<_> = (0..7)
        .map(|day| {
            (
                account,
                today - chrono::Duration::days(day),
                0,
                false,
                false,
            )
        })
        .collect();
    rows.extend([
        (
            account,
            today - chrono::Duration::microseconds(1),
            0,
            false,
            false,
        ),
        (account, day_two, 0, false, false),
        (others[0], day_two, 0, false, false),
        (
            account,
            today - chrono::Duration::days(6) - chrono::Duration::microseconds(1),
            0,
            false,
            false,
        ),
        (account, today + chrono::Duration::days(1), 0, false, false),
        (account, day_two, 1, false, false),
        (account, day_two, 2, false, false),
        (account, day_two, 3, false, false),
        (account, day_two, 0, true, false),
        (account, day_two, 0, false, true),
        (others[1], day_two, 0, false, false),
        (others[2], day_two, 0, false, false),
    ]);
    let mut original = None;
    for (author, at, visibility, deleted, boost) in rows {
        let id: i64 = sqlx::query_scalar("INSERT INTO statuses (account_id, text, created_at, updated_at, visibility, deleted_at, reblog_of_id, local) VALUES ($1, 'bounded history fixture', $2, $2, $3, $4, $5, true) RETURNING id")
            .bind(author).bind(at).bind(visibility).bind(deleted.then_some(at))
            .bind(if boost {original} else {None}).fetch_one(owner).await?;
        original.get_or_insert(id);
        sqlx::query("INSERT INTO statuses_tags (status_id, tag_id) VALUES ($1, $2)")
            .bind(id)
            .bind(tag)
            .execute(owner)
            .await?;
    }
    let response = client
        .get(format!("{base}/api/v1/tags/historyboundaryslice"))
        .header("host", DOMAIN)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let history = body["history"].as_array().unwrap();
    assert_eq!(history.len(), 7);
    for (day, row) in history.iter().enumerate() {
        assert_eq!(
            row["day"],
            (today.and_utc().timestamp() - i64::try_from(day)? * 86400).to_string()
        );
        assert_eq!(
            row["uses"],
            match day {
                1 => "2",
                2 => "3",
                _ => "1",
            }
        );
        assert_eq!(row["accounts"], if day == 2 { "2" } else { "1" });
    }
    sqlx::query("UPDATE accounts SET silenced_at = NULL, suspended_at = NULL WHERE id = ANY($1)")
        .bind(&others[1..])
        .execute(owner)
        .await?;

    // A lock forces the two-second history timeout. Follow must not aggregate
    // before writing: commit survives a failed response readback, and retry is
    // idempotent. No successful response may substitute fabricated zero history.
    let mut lock = owner.begin().await?;
    sqlx::query("LOCK TABLE statuses IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await?;
    for _ in 0..2 {
        let response = tokio::time::timeout(
            StdDuration::from_secs(8),
            client
                .post(format!("{base}/api/v1/tags/historyboundaryslice/follow"))
                .header("host", DOMAIN)
                .bearer_auth(TOKEN)
                .send(),
        )
        .await??;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(response_json(response).await.get("history").is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM tag_follows WHERE account_id = $1 AND tag_id = $2"
            )
            .bind(account)
            .bind(tag)
            .fetch_one(owner)
            .await?,
            1
        );
    }
    lock.rollback().await?;
    let response = client
        .post(format!("{base}/api/v1/tags/historyboundaryslice/follow"))
        .header("host", DOMAIN)
        .bearer_auth(TOKEN)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["following"], true);
    Ok(())
}
