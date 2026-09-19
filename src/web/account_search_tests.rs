//! Source-first regressions; execute only through the named disposable restored-schema fixture.
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::response::IntoResponse;
use serde_json::{Value, json};

use super::*;

const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
// The existing resolver selects HTTP for .onion handles, allowing a loopback HTTP
// positive control without new TLS/trust hooks. This does not test HTTPS or contact Tor.
const REMOTE_DOMAIN: &str = "search-discovery.onion";
const TOKEN: &str = "fixture-bearer-token-v4-6-5";
const SEARCH_TOKEN: &str = "fixture-v2-search-only";
const ACCOUNTS_TOKEN: &str = "fixture-v2-accounts-only";
const APP_TOKEN: &str = "fixture-bearer-application-only-v4-6-5";
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

async fn search(
    client: &reqwest::Client,
    base: &str,
    v2: bool,
    query: &str,
    token: Option<&str>,
) -> TestResult<(u16, Value)> {
    let path = if v2 {
        "v2/search"
    } else {
        "v1/accounts/search"
    };
    let mut request = client
        .get(format!("{base}/api/{path}?{query}"))
        .header("host", DOMAIN);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await?;
    let code = response.status().as_u16();
    Ok((code, serde_json::from_str(&response.text().await?)?))
}

async fn results(
    client: &reqwest::Client,
    base: &str,
    query: &str,
    token: Option<&str>,
) -> TestResult<Value> {
    let (code, body) = search(client, base, true, query, token).await?;
    assert_eq!(code, 200, "{query}: {body}");
    for field in ["accounts", "statuses", "hashtags", "collections"] {
        assert!(body[field].is_array(), "{field}: {body}");
    }
    assert_eq!(body["statuses"], json!([]));
    assert_eq!(body["collections"], json!([]));
    Ok(body)
}

// Anonymous viewers cannot feature accounts; keep every other field and the
// authenticated response comparison intact. Never normalize the actual response.
fn expected_accounts_for_viewer(accounts: &Value, authenticated: bool) -> Value {
    let mut expected = accounts.clone();
    if !authenticated {
        for account in expected.as_array_mut().expect("accounts") {
            account["feature_approval"]["current_user"] = json!("denied");
        }
    }
    expected
}

async fn remote_rows(pool: &PgPool) -> TestResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM accounts WHERE domain = $1")
            .bind(REMOTE_DOMAIN)
            .fetch_one(pool)
            .await?,
    )
}

#[tokio::test]
#[ignore = "requires tools/mastodon-fixture schema-read-test v2_account_search"]
#[allow(clippy::too_many_lines)]
async fn v2_accounts_reuse_search_and_authenticated_resolution() -> TestResult {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let mut connection = <sqlx::PgConnection as sqlx::Connection>::connect(&owner_url).await?;
    let writer =
        WriteRepository::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let writer_role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(writer.pool())
        .await?;
    crate::operational_schema::migrate_with_writer_role(&mut connection, Some(&writer_role))
        .await?;
    let setup = WriteRepository::connect(&owner_url).await?;
    let repository = Repository::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    for (token, scopes) in [
        (SEARCH_TOKEN, "read:search"),
        (ACCOUNTS_TOKEN, "read:accounts"),
    ] {
        sqlx::query(
            "INSERT INTO oauth_access_tokens (token, scopes, resource_owner_id, application_id, created_at) \
             SELECT $1, $2, resource_owner_id, application_id, clock_timestamp() \
             FROM oauth_access_tokens WHERE token = $3",
        )
        .bind(token)
        .bind(scopes)
        .bind(TOKEN)
        .execute(setup.pool())
        .await?;
    }
    sqlx::query("UPDATE oauth_access_tokens SET scopes = 'read:search' WHERE token = $1")
        .bind(APP_TOKEN)
        .execute(setup.pool())
        .await?;
    sqlx::query(
        "UPDATE accounts SET display_name = 'searchprobe' \
         WHERE domain IS NULL AND username IN ('alice', 'moderator')",
    )
    .execute(setup.pool())
    .await?;
    // Fixture-only signer: never use an instance environment or live credential.
    sqlx::query("UPDATE accounts SET private_key = $1 WHERE id = -99")
        .bind(include_str!(
            "../../tests/fixtures/http-signature-private.pem"
        ))
        .execute(setup.pool())
        .await?;

    let fetches = Arc::new(AtomicUsize::new(0));
    let signed_fetches = Arc::new(AtomicUsize::new(0));
    let mock_fetches = fetches.clone();
    let mock_signed = signed_fetches.clone();
    let mock = Router::new().fallback(get(move |uri: Uri, headers: HeaderMap| {
        let fetches = mock_fetches.clone();
        let signed = mock_signed.clone();
        async move {
            fetches.fetch_add(1, Ordering::SeqCst);
            let (content_type, body) = if uri.path() == "/.well-known/webfinger" {
                (
                    "application/jrd+json",
                    json!({
                        "subject": format!("acct:discovered@{REMOTE_DOMAIN}"),
                        "links": [{"rel": "self", "type": "application/activity+json",
                            "href": format!("http://{REMOTE_DOMAIN}/users/discovered")}]
                    }),
                )
            } else {
                assert_eq!(uri.path(), "/users/discovered");
                if headers.contains_key("signature") {
                    signed.fetch_add(1, Ordering::SeqCst);
                }
                (
                    "application/activity+json",
                    json!({
                        "@context": "https://www.w3.org/ns/activitystreams",
                        "id": format!("http://{REMOTE_DOMAIN}/users/discovered"),
                        "type": "Person", "preferredUsername": "discovered",
                        // validate_target checks a literal public address, with no DNS or HTTP request.
                        "inbox": "https://8.8.8.8/inbox"
                    }),
                )
            };
            ([(CONTENT_TYPE, content_type)], body.to_string())
        }
    }));
    let mock_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let mock_endpoint = mock_listener.local_addr()?;
    let mock_server = tokio::spawn(async move { axum::serve(mock_listener, mock).await });
    let mut state = WebState::new(
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
    // Existing debug-only transport hook; no new production injection API.
    state.remote_account_resolver = RemoteAccountResolver::new(
        state
            .remote_fetcher
            .clone()
            .with_test_endpoint(Some(mock_endpoint)),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let client = reqwest::Client::new();

    for query in ["q=alice", "q=bob%40remote.fixture.invalid"] {
        let (code, v1) = search(&client, &base, false, query, Some(TOKEN)).await?;
        assert_eq!(code, 200);
        assert!(!v1.as_array().expect("v1 accounts").is_empty());
        if query == "q=alice" {
            assert_eq!(v1[0]["feature_approval"]["current_user"], "automatic");
        }
        for token in [Some(SEARCH_TOKEN), None] {
            let expected = expected_accounts_for_viewer(&v1, token.is_some());
            for suffix in ["", "&type=accounts"] {
                let v2 = results(&client, &base, &format!("{query}{suffix}"), token).await?;
                assert_eq!(v2["accounts"], expected);
            }
        }
    }
    for query in [
        "q=searchprobe&limit=1&offset=0",
        "q=searchprobe&limit=1&offset=1",
        "q=searchprobe&limit=1&offset=2",
        "q=bob&following=true",
        "q=carol&following=true",
    ] {
        let (code, v1) = search(&client, &base, false, query, Some(TOKEN)).await?;
        assert_eq!(code, 200);
        let v2 = results(
            &client,
            &base,
            &format!("{query}&type=accounts"),
            Some(SEARCH_TOKEN),
        )
        .await?;
        assert_eq!(v2["accounts"], v1, "{query}");
    }
    let anonymous_following = results(&client, &base, "q=alice&following=true", None).await?;
    assert_eq!(anonymous_following["accounts"], json!([]));
    let zero = results(
        &client,
        &base,
        "q=alice&type=accounts&limit=0",
        Some(SEARCH_TOKEN),
    )
    .await?;
    assert_eq!(zero["accounts"], json!([]));
    let untyped = results(&client, &base, "q=alice&offset=999", Some(SEARCH_TOKEN)).await?;
    let first = results(&client, &base, "q=alice", Some(SEARCH_TOKEN)).await?;
    assert_eq!(untyped, first, "existing untyped search ignores offset");
    for kind in ["hashtags", "statuses", "unknown"] {
        let body = results(
            &client,
            &base,
            &format!("q=alice&type={kind}"),
            Some(SEARCH_TOKEN),
        )
        .await?;
        assert_eq!(body["accounts"], json!([]));
    }
    let tags = results(
        &client,
        &base,
        "q=fixturetag&type=hashtags",
        Some(SEARCH_TOKEN),
    )
    .await?;
    assert!(!tags["hashtags"].as_array().expect("hashtags").is_empty());
    let all = results(&client, &base, "q=fixturetag", Some(SEARCH_TOKEN)).await?;
    assert_eq!(all["hashtags"], tags["hashtags"]);
    let accounts = results(
        &client,
        &base,
        "q=fixturetag&type=accounts",
        Some(SEARCH_TOKEN),
    )
    .await?;
    assert_eq!(accounts["hashtags"], json!([]));
    for (v2, query, token, code) in [
        (true, "q=alice", Some(ACCOUNTS_TOKEN), 403),
        (false, "q=alice", Some(SEARCH_TOKEN), 403),
        (false, "q=alice", None, 401),
        (true, "q=alice&type=accounts&offset=0", None, 401),
        (
            true,
            "q=alice&type=accounts&offset=-1",
            Some(SEARCH_TOKEN),
            400,
        ),
        (true, "q=alice&limit=-1", Some(SEARCH_TOKEN), 400),
        (true, "type=accounts", Some(SEARCH_TOKEN), 400),
    ] {
        assert_eq!(
            search(&client, &base, v2, query, token).await?.0,
            code,
            "{query}"
        );
    }

    let remote_query = format!("q=discovered%40{REMOTE_DOMAIN}&resolve=true");
    assert_eq!(remote_rows(setup.pool()).await?, 0);
    // A configured writer and live counter mock make accidental anonymous resolution observable.
    for token in [None, Some(APP_TOKEN), Some("unknown-search-token")] {
        assert_eq!(
            search(&client, &base, true, &remote_query, token).await?.0,
            401
        );
    }
    for suffix in [
        "&type=hashtags",
        "&type=statuses",
        "&type=accounts&offset=1",
        "&limit=0",
    ] {
        let body = results(
            &client,
            &base,
            &format!("{remote_query}{suffix}"),
            Some(SEARCH_TOKEN),
        )
        .await?;
        assert_eq!(body["accounts"], json!([]));
    }
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
    assert_eq!(remote_rows(setup.pool()).await?, 0);
    let resolved = results(
        &client,
        &base,
        &format!("{remote_query}&type=accounts"),
        Some(SEARCH_TOKEN),
    )
    .await?;
    assert_eq!(
        resolved["accounts"]
            .as_array()
            .expect("resolved accounts")
            .len(),
        1
    );
    assert_eq!(
        resolved["accounts"][0]["acct"],
        format!("discovered@{REMOTE_DOMAIN}")
    );
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        2,
        "WebFinger and actor GET only"
    );
    assert_eq!(
        signed_fetches.load(Ordering::SeqCst),
        1,
        "actor fetch stays signed"
    );
    assert_eq!(remote_rows(setup.pool()).await?, 1);
    assert_eq!(
        resolved["accounts"][0]["feature_approval"]["current_user"],
        "missing"
    );
    for token in [None, Some(SEARCH_TOKEN)] {
        let cached_query = if token.is_none() {
            remote_query.replace("&resolve=true", "")
        } else {
            remote_query.clone()
        };
        let cached = results(&client, &base, &cached_query, token).await?;
        assert_eq!(
            cached["accounts"],
            expected_accounts_for_viewer(&resolved["accounts"], token.is_some())
        );
    }
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        2,
        "anonymous/cache hits never refetch"
    );
    server.abort();
    mock_server.abort();
    Ok(())
}

#[test]
fn known_status_url_branch_contract() {
    for kind in [None, Some(""), Some("statuses")] {
        assert_eq!(
            status_search_url(" https://example.test/s/1 ", true, kind, 1, 0),
            Some("https://example.test/s/1")
        );
    }
    for (resolve, kind, limit, offset) in [
        (false, None, 1, 0),
        (true, Some("accounts"), 1, 0),
        (true, Some("hashtags"), 1, 0),
        (true, Some("unknown"), 1, 0),
        (true, None, 0, 0),
        (true, Some("statuses"), 1, 1),
    ] {
        assert_eq!(
            status_search_url("https://example.test/s/1", resolve, kind, limit, offset),
            None
        );
    }
    assert!(status_search_url("https://example.test/s/1", true, None, 1, 99).is_some());
    assert!(status_search_url("https://example.test/s/1", true, Some(""), 1, 99).is_some());
    for query in [
        "alice",
        "ftp://example.test/s/1",
        "https://",
        "https://user:pass@example.test/s/1",
    ] {
        assert_eq!(status_search_url(query, true, None, 1, 0), None);
    }
}

// Separate status assertions: account-only `results` deliberately requires an empty status array.
async fn status_results(
    client: &reqwest::Client,
    base: &str,
    url: &str,
    suffix: &str,
) -> TestResult<Value> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", url)
        .finish();
    let (code, body) = search(
        client,
        base,
        true,
        &format!("{query}&resolve=true{suffix}"),
        Some(SEARCH_TOKEN),
    )
    .await?;
    assert_eq!(code, 200, "{body}");
    for field in ["accounts", "hashtags", "collections"] {
        assert_eq!(body[field], json!([]), "{body}");
    }
    Ok(body["statuses"].clone())
}

#[tokio::test]
#[ignore = "requires tools/mastodon-fixture schema-read-test v2_account_search"]
#[allow(clippy::too_many_lines)]
async fn v2_known_status_urls_use_authorized_projection() -> TestResult {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let writer = WriteRepository::connect(&owner_url).await?;
    let repository = Repository::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    // The HTTP application's repository must really be the restricted read role.
    let runtime_pool = PgPool::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    let denied = sqlx::query("UPDATE statuses SET text=text WHERE false")
        .execute(&runtime_pool)
        .await
        .expect_err("runtime cannot write statuses");
    assert_eq!(
        denied
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("42501")
    );
    let viewer: i64 =
        sqlx::query_scalar("SELECT id FROM accounts WHERE username = 'alice' AND domain IS NULL")
            .fetch_one(writer.pool())
            .await?;
    let remote: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE username = 'bob' AND domain = 'remote.fixture.invalid'",
    )
    .fetch_one(writer.pool())
    .await?;
    for (token, scopes) in [
        (SEARCH_TOKEN, "read:search"),
        (ACCOUNTS_TOKEN, "read:accounts"),
    ] {
        sqlx::query("INSERT INTO oauth_access_tokens (token, scopes, resource_owner_id, application_id, created_at) SELECT $1, $2, resource_owner_id, application_id, clock_timestamp() FROM oauth_access_tokens WHERE token = $3 ON CONFLICT (token) DO NOTHING")
            .bind(token).bind(scopes).bind(TOKEN).execute(writer.pool()).await?;
    }
    sqlx::query("DELETE FROM mutes WHERE account_id = $1 AND target_account_id = $2")
        .bind(viewer)
        .bind(remote)
        .execute(writer.pool())
        .await?;
    for (id, author, local, uri, url) in [
        (990_001_i64, viewer, true, None, None),
        (
            990_002,
            remote,
            false,
            Some("https://remote.fixture.invalid/objects/search-known"),
            Some("https://remote.fixture.invalid/@bob/search-known"),
        ),
    ] {
        sqlx::query("INSERT INTO statuses (id, account_id, local, uri, url, text, visibility, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,'known URL search fixture',0,clock_timestamp(),clock_timestamp())")
            .bind(id).bind(author).bind(local).bind(uri).bind(url).execute(writer.pool()).await?;
    }
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
    // Deliberately no writer/resolver configured; lookup is read-only.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let client = reqwest::Client::new();
    let local = format!("https://{DOMAIN}/@alice/990001");
    let remote_url = "https://remote.fixture.invalid/objects/search-known";
    for (url, id) in [
        // Existing rich media, nullable-local unlisted, poll, and quote fixtures.
        (
            format!("https://{DOMAIN}/@alice/116844842188805001"),
            "116844842188805001",
        ),
        (
            format!("https://{DOMAIN}/@alice/116844846120965002"),
            "116844846120965002",
        ),
        (
            "https://remote.fixture.invalid/users/bob/statuses/111680579174405102".to_owned(),
            "111680579174405102",
        ),
        (
            "https://remote.fixture.invalid/users/bob/statuses/116845317980165202".to_owned(),
            "116845317980165202",
        ),
        (local.clone(), "990001"),
        (
            format!("https://{DOMAIN}/users/alice/statuses/990001"),
            "990001",
        ),
        (
            format!("https://{DOMAIN}/ap/users/{viewer}/statuses/990001"),
            "990001",
        ),
        (remote_url.to_owned(), "990002"),
        (
            "https://remote.fixture.invalid/@bob/search-known".to_owned(),
            "990002",
        ),
    ] {
        let ordinary: Value = serde_json::from_str(
            &client
                .get(format!("{base}/api/v1/statuses/{id}"))
                .header("host", DOMAIN)
                .bearer_auth(TOKEN)
                .send()
                .await?
                .text()
                .await?,
        )?;
        for suffix in [
            "",
            "&type=",
            "&type=statuses",
            "&offset=99",
            "&type=&offset=99",
            "&type=statuses&account_id=1&min_id=999999999&max_id=1&following=true",
        ] {
            let statuses = status_results(&client, &base, &url, suffix).await?;
            assert_eq!(statuses, json!([ordinary]), "{url} {suffix}");
            assert_eq!(statuses[0]["id"], id);
        }
        for suffix in [
            "&limit=0",
            "&type=statuses&offset=1",
            "&type=accounts",
            "&type=hashtags",
            "&type=unknown",
        ] {
            assert_eq!(
                status_results(&client, &base, &url, suffix).await?,
                json!([])
            );
        }
    }
    for url in [
        format!("http://{DOMAIN}/@alice/990001"),
        format!("https://{DOMAIN}/@bob/990001"),
        "https://evil.invalid/@alice/990001".to_owned(),
        format!("{local}?extra=1"),
        "https://remote.fixture.invalid/objects/unknown".to_owned(),
    ] {
        assert_eq!(status_results(&client, &base, &url, "").await?, json!([]));
    }
    for (query, token, expected) in [
        (format!("q={local}&resolve=true"), None, 401),
        (format!("q={local}&resolve=true"), Some(APP_TOKEN), 401),
        (format!("q={local}&resolve=true"), Some(ACCOUNTS_TOKEN), 403),
        (format!("q={local}&offset=0"), None, 401),
        (format!("q={local}&resolve=false"), None, 200),
        (format!("q={local}&resolve=false"), Some(SEARCH_TOKEN), 200),
    ] {
        let (code, body) = search(&client, &base, true, &query, token).await?;
        assert_eq!(code, expected, "{body}");
        if code == 200 {
            assert_eq!(body["statuses"], json!([]));
        }
    }
    assert_status_url_collisions(&client, &base, writer.pool(), viewer, remote).await?;
    for (table, extra) in [("blocks", ""), ("mutes", ", hide_notifications")] {
        let values = if extra.is_empty() { "" } else { ", true" };
        sqlx::query(&format!("INSERT INTO {table} (account_id,target_account_id,created_at,updated_at{extra}) VALUES ($1,$2,clock_timestamp(),clock_timestamp(){values})"))
            .bind(viewer).bind(remote).execute(writer.pool()).await?;
        assert_eq!(
            status_results(&client, &base, remote_url, "").await?,
            json!([]),
            "{table}"
        );
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE account_id=$1 AND target_account_id=$2"
        ))
        .bind(viewer)
        .bind(remote)
        .execute(writer.pool())
        .await?;
    }
    sqlx::query("INSERT INTO account_domain_blocks (account_id,domain,created_at,updated_at) VALUES ($1,'remote.fixture.invalid',clock_timestamp(),clock_timestamp())").bind(viewer).execute(writer.pool()).await?;
    assert_eq!(
        status_results(&client, &base, remote_url, "").await?,
        json!([])
    );
    sqlx::query(
        "DELETE FROM account_domain_blocks WHERE account_id=$1 AND domain='remote.fixture.invalid'",
    )
    .bind(viewer)
    .execute(writer.pool())
    .await?;
    sqlx::query("UPDATE accounts SET suspended_at=clock_timestamp() WHERE id=$1")
        .bind(remote)
        .execute(writer.pool())
        .await?;
    assert_eq!(
        status_results(&client, &base, remote_url, "").await?,
        json!([])
    );
    sqlx::query("UPDATE accounts SET suspended_at=NULL WHERE id=$1")
        .bind(remote)
        .execute(writer.pool())
        .await?;
    sqlx::query("DELETE FROM follows WHERE account_id=$1 AND target_account_id=$2")
        .bind(viewer)
        .bind(remote)
        .execute(writer.pool())
        .await?;
    for visibility in [2, 3] {
        sqlx::query("UPDATE statuses SET visibility=$1 WHERE id=990002")
            .bind(visibility)
            .execute(writer.pool())
            .await?;
        assert_eq!(
            status_results(&client, &base, remote_url, "").await?,
            json!([])
        );
        sqlx::query("INSERT INTO mentions (account_id,status_id,created_at,updated_at) VALUES ($1,990002,clock_timestamp(),clock_timestamp())").bind(viewer).execute(writer.pool()).await?;
        assert_eq!(
            status_results(&client, &base, remote_url, "").await?[0]["id"],
            "990002"
        );
        sqlx::query("DELETE FROM mentions WHERE status_id=990002")
            .execute(writer.pool())
            .await?;
    }
    sqlx::query("UPDATE statuses SET visibility=0,deleted_at=clock_timestamp() WHERE id=990002")
        .execute(writer.pool())
        .await?;
    assert_eq!(
        status_results(&client, &base, remote_url, "").await?,
        json!([])
    );
    server.abort();
    Ok(())
}

// Identity selection precedes all access filtering: a hidden authoritative row
// cannot turn a less authoritative display-URL collision into the search result.
#[allow(clippy::too_many_lines)]
async fn assert_status_url_collisions(
    client: &reqwest::Client,
    base: &str,
    pool: &PgPool,
    viewer: i64,
    remote: i64,
) -> TestResult {
    sqlx::query("INSERT INTO statuses (id,account_id,local,uri,url,text,visibility,created_at,updated_at) VALUES (980001,$1,false,'https://foreign.fixture.invalid/collision/1',NULL,'foreign collision',0,clock_timestamp(),clock_timestamp())")
        .bind(remote).execute(pool).await?;
    let mut checks = Vec::new();
    for (url, target) in [
        (format!("https://{DOMAIN}/@alice/990001"), 990_001_i64),
        (
            format!("https://{DOMAIN}/users/alice/statuses/990001"),
            990_001,
        ),
        (
            format!("https://{DOMAIN}/ap/users/{viewer}/statuses/990001"),
            990_001,
        ),
        (
            "https://remote.fixture.invalid/objects/search-known".to_owned(),
            990_002,
        ),
    ] {
        sqlx::query("UPDATE statuses SET url=$1 WHERE id=980001")
            .bind(&url)
            .execute(pool)
            .await?;
        let expected = json!([target.to_string()]);
        let ids = |statuses: Value| -> Value {
            statuses
                .as_array()
                .expect("statuses array")
                .iter()
                .map(|status| status["id"].clone())
                .collect()
        };
        checks.push((
            format!("authoritative target wins: {url}"),
            ids(status_results(client, base, &url, "").await?),
            expected.clone(),
        ));
        sqlx::query("UPDATE statuses SET deleted_at=clock_timestamp() WHERE id=980001")
            .execute(pool)
            .await?;
        checks.push((
            format!("denied collision cannot hide target: {url}"),
            ids(status_results(client, base, &url, "").await?),
            expected,
        ));
        sqlx::query("UPDATE statuses SET deleted_at=NULL WHERE id=980001")
            .execute(pool)
            .await?;
        sqlx::query("UPDATE statuses SET deleted_at=clock_timestamp() WHERE id=$1")
            .bind(target)
            .execute(pool)
            .await?;
        checks.push((
            format!("denied authoritative target has no fallback: {url}"),
            ids(status_results(client, base, &url, "").await?),
            json!([]),
        ));
        sqlx::query("UPDATE statuses SET deleted_at=NULL WHERE id=$1")
            .bind(target)
            .execute(pool)
            .await?;
    }
    // The configured-origin alias is more authoritative even than foreign uri.
    let local = format!("https://{DOMAIN}/@alice/990001");
    sqlx::query("UPDATE statuses SET uri=$1,url=NULL WHERE id=980001")
        .bind(&local)
        .execute(pool)
        .await?;
    let statuses = status_results(client, base, &local, "").await?;
    checks.push((
        "local alias precedes foreign canonical URI".to_owned(),
        statuses[0]["id"].clone(),
        json!("990001"),
    ));
    sqlx::query("UPDATE statuses SET uri='https://foreign.fixture.invalid/collision/1',url='https://foreign.fixture.invalid/shared-display' WHERE id=980001").execute(pool).await?;
    sqlx::query("INSERT INTO statuses (id,account_id,local,uri,url,text,visibility,created_at,updated_at) VALUES (980002,$1,false,'https://foreign.fixture.invalid/collision/2','https://foreign.fixture.invalid/shared-display','second foreign collision',0,clock_timestamp(),clock_timestamp())")
        .bind(remote).execute(pool).await?;
    for hidden in [false, true] {
        if hidden {
            sqlx::query("UPDATE statuses SET deleted_at=clock_timestamp() WHERE id=980001")
                .execute(pool)
                .await?;
        }
        checks.push((
            format!("ambiguous display URL fails closed, hidden={hidden}"),
            status_results(
                client,
                base,
                "https://foreign.fixture.invalid/shared-display",
                "",
            )
            .await?,
            json!([]),
        ));
    }
    sqlx::query("DELETE FROM statuses WHERE id IN (980001,980002)")
        .execute(pool)
        .await?;
    // Collect every mismatch so the red run proves each collision case, rather
    // than aborting at the first substituted ID.
    let failures: Vec<_> = checks
        .into_iter()
        .filter(|(_, actual, expected)| actual != expected)
        .collect();
    assert!(
        failures.is_empty(),
        "status URL identity collisions: {failures:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PG14 restricted runtime/writer fixture"]
#[allow(clippy::too_many_lines)]
async fn v2_uncached_status_urls_do_not_create_an_audience() -> TestResult {
    use rsa::pkcs1::DecodeRsaPrivateKey;
    use rsa::pkcs8::EncodePublicKey;
    const REMOTE_DOMAIN: &str = "status-resolution.onion";
    let setup = PgPool::connect(&std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?).await?;
    let writer =
        WriteRepository::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let repository = Repository::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    let role: (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolcreaterole FROM pg_roles WHERE rolname=current_user")
            .fetch_one(writer.pool())
            .await?;
    assert_eq!(role, (false, false));
    sqlx::query("INSERT INTO oauth_access_tokens (token, scopes, resource_owner_id, application_id, created_at) SELECT $1, 'read:search', resource_owner_id, application_id, clock_timestamp() FROM oauth_access_tokens WHERE token=$2 ON CONFLICT DO NOTHING")
        .bind(SEARCH_TOKEN).bind(TOKEN).execute(&setup).await?;
    sqlx::query("UPDATE accounts SET private_key=$1 WHERE id=-99")
        .bind(include_str!(
            "../../tests/fixtures/http-signature-private.pem"
        ))
        .execute(&setup)
        .await?;
    let key = rsa::RsaPrivateKey::from_pkcs1_pem(include_str!(
        "../../tests/fixtures/http-signature-private.pem"
    ))?;
    let public_key = rsa::RsaPublicKey::from(&key).to_public_key_pem(rsa::pkcs8::LineEnding::LF)?;
    let fetches = Arc::new(AtomicUsize::new(0));
    let count = fetches.clone();
    let mock = Router::new().fallback(get(move |uri: Uri, headers: HeaderMap| {
        let count = count.clone();
        let public_key = public_key.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            assert!(uri.path() == "/.well-known/webfinger" || headers.contains_key("signature"), "unsigned resolution: {uri}");
            if uri.path() != "/.well-known/webfinger" {
                let request = HttpSignatureRequest::new(&Method::GET, uri.path_and_query().unwrap().as_str(), &headers, &[]);
                let key_id = signature_key_id(&headers).expect("signature").expect("key id");
                verify_http_signature(&request, &HttpSignatureKey { key_id: &key_id, public_key_pem: &public_key }, SystemTime::now()).expect("valid signed GET");
            }
            if uri.path() == "/html" {
                return ([(CONTENT_TYPE, "text/html")], "<html>unsupported discovery</html>").into_response();
            }
            if uri.path() == "/missing" { return StatusCode::NOT_FOUND.into_response(); }
            if uri.path() == "/display" {
                return (StatusCode::FOUND, [(LOCATION, format!("http://{REMOTE_DOMAIN}/public"))]).into_response();
            }
            if uri.path() == "/redirect" {
                return (StatusCode::FOUND, [(LOCATION, "http://elsewhere.onion/public")]).into_response();
            }
            let actor = format!("http://{REMOTE_DOMAIN}/users/statusauthor");
            let public = "https://www.w3.org/ns/activitystreams#Public";
            let body = if uri.path() == "/.well-known/webfinger" {
                json!({"subject":format!("acct:statusauthor@{REMOTE_DOMAIN}"), "links":[{"rel":"self", "type":"application/activity+json", "href":actor}]})
            } else if uri.path() == "/users/statusauthor" {
                json!({"@context":"https://www.w3.org/ns/activitystreams", "id": actor, "type":"Person", "preferredUsername":"statusauthor", "inbox":"https://8.8.8.8/inbox", "followers":format!("{actor}/followers")})
            } else {
                let (to, cc) = match uri.path() {
                    "/private" | "/private-new" => (json!([format!("{actor}/followers")]), json!([])),
                    "/unlisted" => (json!([format!("{actor}/followers")]), json!([public])),
                    _ => (json!([public]), json!([])),
                };
                let mut note = json!({"id":format!("http://{REMOTE_DOMAIN}{}", uri.path()), "type":"Note", "attributedTo":actor, "content":"uncached search", "to":to, "cc":cc});
                match uri.path() {
                    "/mismatch" => note["id"] = json!(format!("http://{REMOTE_DOMAIN}/different")),
                    "/attribution" => note["attributedTo"] = json!("http://elsewhere.onion/users/attacker"),
                    "/invalid" => note["content"] = Value::Null,
                    "/article" => note["type"] = json!("Article"),
                    "/reply" => note["inReplyTo"] = json!(format!("http://{REMOTE_DOMAIN}/missing-parent")),
                    "/question" => { note["type"] = json!("Question"); note["oneOf"] = json!([{"type":"Note", "name":"yes", "replies":{"totalItems":0}}, {"type":"Note", "name":"no", "replies":{"totalItems":0}}]); },
                    "/direct" => { let recipient = format!("https://{DOMAIN}/users/alice"); note["to"] = json!([recipient]); note["tag"] = json!([{"type":"Mention", "href":recipient, "name":"@alice"}]); },
                    _ => (),
                }
                note
            };
            ([(CONTENT_TYPE, if uri.path() == "/.well-known/webfinger" { "application/jrd+json" } else { "application/activity+json" })], body.to_string()).into_response()
        }
    }));
    let mock_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = mock_listener.local_addr()?;
    let mock_server = tokio::spawn(async move { axum::serve(mock_listener, mock).await });
    let mut state = WebState::new(
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
    state.remote_fetcher = state
        .remote_fetcher
        .clone()
        .with_test_endpoint(Some(endpoint));
    state.remote_account_resolver = RemoteAccountResolver::new(state.remote_fetcher.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let client = reqwest::Client::new();
    let public_url = format!("http://{REMOTE_DOMAIN}/public");
    for suffix in ["&type=hashtags", "&limit=0", "&type=statuses&offset=1"] {
        assert_eq!(
            status_results(&client, &base, &public_url, suffix).await?,
            json!([])
        );
    }
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
    for (token, expected) in [(None, 401), (Some(APP_TOKEN), 401)] {
        let (code, _) = search(
            &client,
            &base,
            true,
            &format!("q={public_url}&resolve=true&type=statuses"),
            token,
        )
        .await?;
        assert_eq!(code, expected);
    }
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
    for path in [
        "private-new",
        "mismatch",
        "attribution",
        "invalid",
        "article",
        "html",
        "missing",
        "redirect",
    ] {
        let before = fetches.load(Ordering::SeqCst);
        assert_eq!(
            status_results(
                &client,
                &base,
                &format!("http://{REMOTE_DOMAIN}/{path}"),
                "&type=statuses"
            )
            .await?,
            json!([]),
            "{path}"
        );
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            before + 1,
            "no actor/redirect discovery for {path}"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounts WHERE domain=$1")
                .bind(REMOTE_DOMAIN)
                .fetch_one(&setup)
                .await?,
            0,
            "no account creation for {path}"
        );
    }
    for input in [
        format!("http://user:pass@{REMOTE_DOMAIN}/public"),
        format!("http://{REMOTE_DOMAIN}/public#fragment"),
        format!("http://{DOMAIN}/unknown"),
    ] {
        let before = fetches.load(Ordering::SeqCst);
        assert_eq!(
            status_results(&client, &base, &input, "&type=statuses").await?,
            json!([])
        );
        assert_eq!(fetches.load(Ordering::SeqCst), before);
    }
    let public = status_results(&client, &base, &public_url, "&type=statuses").await?;
    assert_eq!(public.as_array().unwrap().len(), 1, "uncached public URL");
    assert_eq!(public[0]["visibility"], "public");
    let request_count = fetches.load(Ordering::SeqCst);
    assert!(request_count >= 2);
    assert_eq!(
        status_results(&client, &base, &public_url, "&type=statuses").await?,
        public
    );
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        request_count,
        "cache must not fetch"
    );
    assert_eq!(
        status_results(
            &client,
            &base,
            &format!("http://{REMOTE_DOMAIN}/display"),
            "&type=statuses"
        )
        .await?,
        public,
        "same-origin redirect to exact canonical ID"
    );
    let unlisted = status_results(
        &client,
        &base,
        &format!("http://{REMOTE_DOMAIN}/unlisted"),
        "&type=statuses",
    )
    .await?;
    assert_eq!(unlisted[0]["visibility"], "unlisted");
    assert_eq!(
        status_results(
            &client,
            &base,
            &format!("http://{REMOTE_DOMAIN}/private"),
            "&type=statuses"
        )
        .await?,
        json!([])
    );
    let private_count: i64 = sqlx::query_scalar("SELECT count(*) FROM statuses WHERE uri=$1")
        .bind(format!("http://{REMOTE_DOMAIN}/private"))
        .fetch_one(&setup)
        .await?;
    assert_eq!(private_count, 0, "unauthorized search must not materialize");
    let mentions: i64 = sqlx::query_scalar("SELECT count(*) FROM mentions JOIN statuses ON statuses.id=mentions.status_id WHERE statuses.uri LIKE $1")
        .bind(format!("http://{REMOTE_DOMAIN}/%" )).fetch_one(&setup).await?;
    assert_eq!(mentions, 0, "searcher must never become a delivery target");
    let viewer: i64 =
        sqlx::query_scalar("SELECT id FROM accounts WHERE username='alice' AND domain IS NULL")
            .fetch_one(&setup)
            .await?;
    let author: i64 = sqlx::query_scalar("SELECT id FROM accounts WHERE domain=$1")
        .bind(REMOTE_DOMAIN)
        .fetch_one(&setup)
        .await?;
    sqlx::query("INSERT INTO follows (account_id,target_account_id,created_at,updated_at) VALUES ($1,$2,now(),now())")
        .bind(viewer).bind(author).execute(&setup).await?;
    let private = status_results(
        &client,
        &base,
        &format!("http://{REMOTE_DOMAIN}/private"),
        "&type=statuses",
    )
    .await?;
    assert_eq!(
        private[0]["visibility"], "private",
        "real follower may import private Note"
    );
    let private_mentions: i64 = sqlx::query_scalar("SELECT count(*) FROM mentions JOIN statuses ON statuses.id=mentions.status_id WHERE statuses.uri=$1")
        .bind(format!("http://{REMOTE_DOMAIN}/private")).fetch_one(&setup).await?;
    assert_eq!(private_mentions, 0);
    sqlx::query("DELETE FROM follows WHERE account_id=$1 AND target_account_id=$2")
        .bind(viewer)
        .bind(author)
        .execute(&setup)
        .await?;
    sqlx::query(
        "INSERT INTO tombstones (account_id,uri,created_at,updated_at) VALUES ($1,$2,now(),now())",
    )
    .bind(author)
    .bind(format!("http://{REMOTE_DOMAIN}/tombstoned"))
    .execute(&setup)
    .await?;
    let before = fetches.load(Ordering::SeqCst);
    for path in ["private", "tombstoned"] {
        assert_eq!(
            status_results(
                &client,
                &base,
                &format!("http://{REMOTE_DOMAIN}/{path}"),
                "&type=statuses"
            )
            .await?,
            json!([])
        );
    }
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        before,
        "known private denial and tombstone cannot fetch"
    );
    sqlx::query("UPDATE statuses SET deleted_at=now() WHERE uri=$1")
        .bind(&public_url)
        .execute(&setup)
        .await?;
    assert_eq!(
        status_results(&client, &base, &public_url, "&type=statuses").await?,
        json!([])
    );
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        before,
        "deleted authoritative status cannot fetch"
    );
    sqlx::query("UPDATE statuses SET deleted_at=NULL WHERE uri=$1")
        .bind(&public_url)
        .execute(&setup)
        .await?;

    for path in ["reply", "question", "direct"] {
        let result = status_results(
            &client,
            &base,
            &format!("http://{REMOTE_DOMAIN}/{path}"),
            "&type=statuses",
        )
        .await?;
        assert_eq!(result.as_array().unwrap().len(), 1, "{path}");
        if path == "direct" {
            assert_eq!(result[0]["visibility"], "direct");
        }
    }
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM rustodon.outbox_events WHERE kind='rustodon.activitypub.resolve_thread' AND payload->'arguments'->>'parent_url'=$1")
        .bind(format!("http://{REMOTE_DOMAIN}/missing-parent")).fetch_one(&setup).await?;
    assert_eq!(jobs, 1, "normal durable missing-parent resolver");
    let policy_baseline = status_results(&client, &base, &public_url, "&type=statuses").await?;
    assert_eq!(policy_baseline[0]["id"], public[0]["id"]);
    // Every case owns and removes exactly one policy. In particular, the domain
    // case must not inherit a viewer block that could mask a missing domain gate.
    for (policy, install, remove, expected_fetches) in [
        (
            "author-blocked",
            "INSERT INTO blocks (account_id,target_account_id,created_at,updated_at) SELECT viewer,author,now(),now() FROM policy",
            "DELETE FROM blocks USING policy WHERE account_id=viewer AND target_account_id=author",
            1,
        ),
        (
            "reverse-blocked",
            "INSERT INTO blocks (account_id,target_account_id,created_at,updated_at) SELECT author,viewer,now(),now() FROM policy",
            "DELETE FROM blocks USING policy WHERE account_id=author AND target_account_id=viewer",
            1,
        ),
        (
            "muted",
            "INSERT INTO mutes (account_id,target_account_id,created_at,updated_at) SELECT viewer,author,now(),now() FROM policy",
            "DELETE FROM mutes USING policy WHERE account_id=viewer AND target_account_id=author",
            1,
        ),
        (
            "suspended",
            "UPDATE accounts SET suspended_at=now() FROM policy WHERE id=author",
            "UPDATE accounts SET suspended_at=NULL FROM policy WHERE id=author",
            1,
        ),
        (
            "domain-blocked",
            "INSERT INTO account_domain_blocks (account_id,domain,created_at,updated_at) SELECT viewer,accounts.domain,now(),now() FROM policy JOIN accounts ON accounts.id=author",
            "DELETE FROM account_domain_blocks blocked USING policy, accounts WHERE accounts.id=author AND blocked.account_id=viewer AND blocked.domain=accounts.domain",
            0,
        ),
    ] {
        let policy_sql = |statement| {
            format!(
                "WITH policy AS (SELECT $1::bigint AS viewer, $2::bigint AS author) {statement}"
            )
        };
        assert_eq!(
            sqlx::query(&policy_sql(install))
                .bind(viewer)
                .bind(author)
                .execute(&setup)
                .await?
                .rows_affected(),
            1,
            "install {policy}"
        );
        let before = fetches.load(Ordering::SeqCst);
        assert_eq!(
            status_results(&client, &base, &public_url, "&type=statuses").await?,
            json!([]),
            "known {policy}"
        );
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            before,
            "known {policy} must not fetch"
        );
        let uncached_url = format!("http://{REMOTE_DOMAIN}/{policy}");
        assert_eq!(
            status_results(&client, &base, &uncached_url, "&type=statuses").await?,
            json!([]),
            "uncached {policy}"
        );
        let request_delta = fetches.load(Ordering::SeqCst) - before;
        let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM statuses WHERE uri=$1")
            .bind(&uncached_url)
            .fetch_one(&setup)
            .await?;
        assert_eq!(
            sqlx::query(&policy_sql(remove))
                .bind(viewer)
                .bind(author)
                .execute(&setup)
                .await?
                .rows_affected(),
            1,
            "cleanup {policy}"
        );
        assert_eq!(
            request_delta, expected_fetches,
            "{policy}: only domain denial precedes the object GET"
        );
        assert_eq!(persisted, 0, "{policy} must not materialize a status");
        assert_eq!(
            status_results(&client, &base, &public_url, "&type=statuses").await?,
            policy_baseline,
            "{policy} fully removed"
        );
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            before + expected_fetches,
            "cleanup uses cached positive control"
        );
    }
    server.abort();
    mock_server.abort();
    Ok(())
}
