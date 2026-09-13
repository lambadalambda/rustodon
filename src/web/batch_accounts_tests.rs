//! Pinned 4.6.5 AccountsController#index contracts; parent wires the test wrappers.
//! Execute only on podman-worker with the disposable schema-read fixture.
use super::*;
use serde_json::{Value, json};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const TOKEN: &str = "fixture-bearer-token-v4-6-5";
const ACCOUNTS_TOKEN: &str = "fixture-bearer-read-accounts-v4-6-5";
const APP_TOKEN: &str = "fixture-bearer-application-only-v4-6-5";
const ALICE: &str = "116844606259201001";
const BOB: &str = "116844606259202001";
const SUSPENDED: &str = "116844606259202003";
const PATH: &str = "/api/v1/accounts";

#[tokio::test]
pub(crate) async fn batch_accounts_route_contract() -> TestResult {
    let route = api_route_for_method(&Method::GET, PATH).expect("batch account GET route");
    assert_eq!(route.support, ApiRouteSupport::Implemented);
    assert_eq!(
        route.authentication,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice())
    );
    assert_eq!(route.pagination, PaginationContract::None);
    assert_eq!(route.cache, ApiCachePolicy::Private);
    // This work adds only GET; account registration is not implemented by this router.
    assert!(api_route_for_method(&Method::POST, PATH).is_none());
    Ok(())
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

async fn request(
    client: &reqwest::Client,
    base: &str,
    method: Method,
    path: &str,
    token: Option<&str>,
) -> TestResult<reqwest::Response> {
    let mut request = client
        .request(method, format!("{base}{path}"))
        .header(HOST, DOMAIN)
        .header(ORIGIN, "https://client.example");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    Ok(request.send().await?)
}

async fn json_body(response: reqwest::Response, status: StatusCode) -> TestResult<Value> {
    assert_eq!(response.status(), status, "{}", response.url());
    assert!(
        response.headers()[CONTENT_TYPE]
            .to_str()?
            .starts_with("application/json")
    );
    assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
    assert!(
        response.headers()[VARY]
            .to_str()?
            .split(',')
            .any(|value| value.trim().eq_ignore_ascii_case("Authorization"))
    );
    assert!(!response.headers().contains_key("link"));
    Ok(serde_json::from_str(&response.text().await?)?)
}

fn sorted_accounts(body: Value) -> Vec<Value> {
    let Value::Array(mut accounts) = body else {
        panic!("REST account array");
    };
    accounts.sort_by_key(|account| account["id"].as_str().expect("string ID").to_owned());
    accounts
}

async fn expect_ids(response: reqwest::Response, expected: &[&str]) -> TestResult {
    let accounts = sorted_accounts(json_body(response, StatusCode::OK).await?);
    let ids = accounts
        .iter()
        .map(|account| account["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(ids, expected);
    for account in accounts {
        for secret in [
            "source",
            "email",
            "private_key",
            "encrypted_password",
            "otp_secret",
        ] {
            assert!(
                account.get(secret).is_none(),
                "unexpected credential field {secret}"
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
#[ignore = "requires disposable schema-read PostgreSQL fixture"]
pub(crate) async fn batch_accounts_reads_preserve_ids_auth_privacy_and_headers() -> TestResult {
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
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(5))
        .build()?;

    // Exact fixture entities, not a second serializer or an order assumption.
    for token in [None, Some(TOKEN), Some(ACCOUNTS_TOKEN), Some(APP_TOKEN)] {
        let mut expected = Vec::new();
        for id in [ALICE, BOB] {
            let response =
                request(&client, &base, Method::GET, &format!("{PATH}/{id}"), token).await?;
            assert_eq!(response.status(), StatusCode::OK);
            expected.push(serde_json::from_str::<Value>(&response.text().await?)?);
        }
        for slash in ["", "/"] {
            let path = format!(
                "{PATH}{slash}?id[]={BOB}&id[]={ALICE}&id[]=999999999&id[]={BOB}&limit=1&max_id=1"
            );
            let body = json_body(
                request(&client, &base, Method::GET, &path, token).await?,
                StatusCode::OK,
            )
            .await?;
            assert_eq!(sorted_accounts(body), sorted_accounts(json!(expected)));
        }
    }

    // Strong parameters discard scalar/object IDs; Ruby to_i keeps numeric prefixes.
    for (query, expected) in [
        (String::new(), vec![]),
        (format!("id={ALICE}"), vec![]),
        (format!("id={ALICE}&id={BOB}"), vec![]),
        (format!("id[key]={ALICE}"), vec![]),
        (format!("id[][key]={ALICE}"), vec![]),
        ("id[]=&id[]=nonsense&id[]=999999999".to_owned(), vec![]),
        (
            format!("id[]={ALICE}suffix&id[]=+%2B{BOB}tail&id[]=invalid"),
            vec![ALICE, BOB],
        ),
        (
            format!("id[]={ALICE}&id[]=0{ALICE}&id[]={ALICE}tail"),
            vec![ALICE],
        ),
    ] {
        expect_ids(
            request(
                &client,
                &base,
                Method::GET,
                &format!("{PATH}?{query}"),
                Some(TOKEN),
            )
            .await?,
            &expected,
        )
        .await?;
    }
    let malformed = request(
        &client,
        &base,
        Method::GET,
        &format!("{PATH}?id[]={ALICE}&id[key]={BOB}"),
        Some(TOKEN),
    )
    .await?;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    // Deduplicate raw IDs BEFORE coercion and limit; missing rows still count.
    for (count, same_raw, same_number, status) in [
        (40, false, false, StatusCode::OK),
        (41, false, false, StatusCode::UNPROCESSABLE_ENTITY),
        (41, true, false, StatusCode::OK),
        (41, false, true, StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let query = (0..count)
            .map(|index| {
                if same_raw {
                    format!("id[]={ALICE}")
                } else if same_number {
                    format!("id[]={ALICE}suffix{index}")
                } else {
                    format!("id[]={}", 900_000 + index)
                }
            })
            .collect::<Vec<_>>()
            .join("&");
        let body = json_body(
            request(
                &client,
                &base,
                Method::GET,
                &format!("{PATH}?{query}"),
                Some(TOKEN),
            )
            .await?,
            status,
        )
        .await?;
        if status == StatusCode::OK {
            assert_eq!(body.as_array().unwrap().len(), usize::from(same_raw));
        } else {
            assert_eq!(body, json!({"error": "Validation failed"}));
        }
    }

    // Exact index scope: remote OR approved+confirmed user. No searchable scope.
    // Both pending and unconfirmed local fixture users, and the userless actor, disappear.
    // The approved negative-ID viewer is a control against rejecting all fixture IDs.
    let privacy_path = format!(
        "{PATH}?id[]={ALICE}&id[]={BOB}&id[]={SUSPENDED}&id[]=-321&id[]=-322&id[]=-99&id[]=-323"
    );
    for token in [None, Some(TOKEN)] {
        expect_ids(
            request(&client, &base, Method::GET, &privacy_path, token).await?,
            &[ALICE, BOB, SUSPENDED, "-323"],
        )
        .await?;
    }

    for (token, status) in [
        ("not-a-known-token", StatusCode::OK),
        ("fixture-bearer-insufficient-v4-6-5", StatusCode::FORBIDDEN),
        ("fixture-bearer-read-statuses-v4-6-5", StatusCode::FORBIDDEN),
        ("fixture-bearer-revoked-v4-6-5", StatusCode::UNAUTHORIZED),
        ("fixture-bearer-expired-v4-6-5", StatusCode::UNAUTHORIZED),
    ] {
        let body = json_body(
            request(&client, &base, Method::GET, PATH, Some(token)).await?,
            status,
        )
        .await?;
        match status {
            StatusCode::OK => assert_eq!(body, json!([])),
            StatusCode::FORBIDDEN => assert_eq!(
                body,
                json!({"error": "This action is outside the authorized scopes"})
            ),
            _ => assert!(body["error"].is_string()),
        }
    }

    for slash in ["", "/"] {
        let path = format!("{PATH}{slash}?id[]={ALICE}");
        let response = request(&client, &base, Method::HEAD, &path, Some(TOKEN)).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[CONTENT_TYPE]
                .to_str()?
                .starts_with("application/json")
        );
        assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert!(!response.headers().contains_key("link"));
        assert!(response.bytes().await?.is_empty());
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            let response = request(&client, &base, method, &path, Some(TOKEN)).await?;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        let response = client
            .request(Method::OPTIONS, format!("{base}{path}"))
            .header(HOST, DOMAIN)
            .header(ORIGIN, "https://client.example")
            .header(ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_METHODS],
            CORS_METHODS
        );
    }
    server.abort();
    Ok(())
}
