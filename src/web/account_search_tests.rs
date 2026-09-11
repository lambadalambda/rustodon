//! Source-first regressions; execute only in a disposable Secunda schema fixture.
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};

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
#[ignore = "requires tools/mastodon-fixture schema-read-test v2_account_search on Secunda"]
#[allow(clippy::too_many_lines)]
async fn v2_accounts_reuse_search_and_authenticated_resolution() -> TestResult {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let mut connection = <sqlx::PgConnection as sqlx::Connection>::connect(&owner_url).await?;
    crate::operational_schema::migrate(&mut connection).await?;
    let writer = WriteRepository::connect(&owner_url).await?;
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
        .execute(writer.pool())
        .await?;
    }
    sqlx::query("UPDATE oauth_access_tokens SET scopes = 'read:search' WHERE token = $1")
        .bind(APP_TOKEN)
        .execute(writer.pool())
        .await?;
    sqlx::query(
        "UPDATE accounts SET display_name = 'searchprobe' \
         WHERE domain IS NULL AND username IN ('alice', 'moderator')",
    )
    .execute(writer.pool())
    .await?;
    // Fixture-only signer: never use an instance environment or live credential.
    sqlx::query("UPDATE accounts SET private_key = $1 WHERE id = -99")
        .bind(include_str!(
            "../../tests/fixtures/http-signature-private.pem"
        ))
        .execute(writer.pool())
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
    assert_eq!(remote_rows(writer.pool()).await?, 0);
    // A configured writer and live counter mock make accidental anonymous resolution observable.
    for token in [None, Some(APP_TOKEN), Some("unknown-search-token")] {
        let body = results(&client, &base, &remote_query, token).await?;
        assert_eq!(body["accounts"], json!([]));
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
    assert_eq!(remote_rows(writer.pool()).await?, 0);
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
    assert_eq!(remote_rows(writer.pool()).await?, 1);
    assert_eq!(
        resolved["accounts"][0]["feature_approval"]["current_user"],
        "missing"
    );
    for token in [None, Some(SEARCH_TOKEN)] {
        let cached = results(&client, &base, &remote_query, token).await?;
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
