//! Runs only against the disposable restored fixture from tools/mastodon-fixture.
use std::error::Error;

use http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{
    BearerAuthenticator, Repository, StatusUpdate, WRITE_STATUSES, WriteRepository,
};
use rustodon::web::{WebState, router};
use serde_json::Value;
use url::Url;

const VIEWER: i64 = 116_844_606_259_201_004;
const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const VIEWER_TOKEN: &str = "fixture-bearer-api-moderator-v4-6-5";
const SAVED: [&str; 2] = ["bookmarks", "favourites"];

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

async fn saved_page(
    client: &reqwest::Client,
    base: &str,
    path: &str,
) -> TestResult<(Value, String)> {
    let response = client
        .get(format!("{base}/api/v1/{path}"))
        .header("host", DOMAIN)
        .bearer_auth(VIEWER_TOKEN)
        .send()
        .await?;
    assert_eq!(response.status(), 200, "{path}");
    let links = response
        .headers()
        .get("link")
        .map(|value| value.to_str().map(str::to_owned))
        .transpose()?
        .unwrap_or_default();
    Ok((serde_json::from_str(&response.text().await?)?, links))
}

fn ids(body: &Value) -> Vec<String> {
    body.as_array()
        .expect("status array")
        .iter()
        .map(|status| status["id"].as_str().expect("status ID").to_owned())
        .collect()
}

fn assert_links(links: &str, first: i64, last: Option<i64>) {
    assert!(links.contains(&format!("min_id={first}")), "{links}");
    assert!(links.contains("rel=\"prev\""), "{links}");
    if let Some(last) = last {
        assert!(links.contains(&format!("max_id={last}")), "{links}");
        assert!(links.contains("rel=\"next\""), "{links}");
    } else {
        assert!(!links.contains("rel=\"next\""), "{links}");
    }
}

#[tokio::test]
#[ignore = "requires a disposable restored fixture from tools/mastodon-fixture schema-read-test"]
#[allow(clippy::too_many_lines)]
async fn saved_statuses_reauthorize_after_follower_removal_and_edit() -> TestResult {
    let read_url = std::env::var("RUSTODON_MASTODON_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let writer = WriteRepository::connect(&owner_url).await?;
    let mut connection = <sqlx::PgConnection as sqlx::Connection>::connect(&owner_url).await?;
    rustodon::operational_schema::migrate(&mut connection).await?;
    let repository = Repository::connect(&read_url).await?;
    let authenticator = BearerAuthenticator::new(repository.clone());
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let author = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    // Fixture setup only: give the local viewer write scopes (its accepted follow is already seeded).
    sqlx::query("UPDATE oauth_access_tokens SET scopes = 'read write' WHERE token = $1")
        .bind(VIEWER_TOKEN)
        .execute(writer.pool())
        .await?;
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-api-moderator-v4-6-5"),
    );
    let viewer = authenticator.authenticate(&headers, WRITE_STATUSES).await?;

    let mut statuses = Vec::new();
    let mut cursors = [Vec::new(), Vec::new()];
    // Association order deliberately differs from status order: save these in reverse below.
    for (visibility, text) in [
        ("private", "newest saved private"),
        ("direct", "@api_moderator direct grant"),
        ("private", "middle saved private"),
        ("private", "@api_moderator private grant"),
        ("public", "ordinary public saved status"),
    ] {
        let status = writer
            .create_status(
                &author,
                text,
                &[],
                None,
                Some(false),
                Some(visibility),
                None,
                None,
                None,
            )
            .await?;
        statuses.push(status.status_id);
    }
    for &status in statuses.iter().rev() {
        writer.set_bookmark(&viewer, status, true).await?;
        writer.set_favourite(&viewer, status, true).await?;
        for (index, table) in SAVED.iter().enumerate() {
            let cursor: i64 = sqlx::query_scalar(&format!(
                "SELECT id FROM {table} WHERE account_id = $1 AND status_id = $2"
            ))
            .bind(VIEWER)
            .bind(status)
            .fetch_one(writer.pool())
            .await?;
            cursors[index].insert(0, cursor);
        }
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let client = reqwest::Client::new();
    let expected = statuses.iter().map(ToString::to_string).collect::<Vec<_>>();
    for endpoint in SAVED {
        let (body, _) = saved_page(&client, &base, endpoint).await?;
        assert_eq!(ids(&body), expected, "authorized {endpoint}");
    }

    writer.remove_follower(&author, VIEWER).await?;
    for &status in &[statuses[0], statuses[2]] {
        writer
            .update_status(
                &author,
                status,
                &StatusUpdate {
                    text: Some("R02 new private content after removal".to_owned()),
                    ..StatusUpdate::default()
                },
            )
            .await?;
        let response = client
            .get(format!("{base}/api/v1/statuses/{status}"))
            .header("host", DOMAIN)
            .bearer_auth(VIEWER_TOKEN)
            .send()
            .await?;
        assert_eq!(
            response.status(),
            404,
            "status-show must deny revoked access"
        );
    }
    let allowed = vec![
        expected[1].clone(),
        expected[3].clone(),
        expected[4].clone(),
    ];
    // Collect both responses before asserting: RED demonstrates both alternate-read leaks.
    let mut actual = Vec::new();
    for endpoint in SAVED {
        let (body, _) = saved_page(&client, &base, endpoint).await?;
        actual.push((
            endpoint,
            ids(&body),
            body.to_string().contains("R02 new private content"),
        ));
    }
    assert_eq!(
        actual,
        vec![
            ("bookmarks", allowed.clone(), false),
            ("favourites", allowed, false)
        ]
    );

    for (endpoint, cursor) in SAVED.into_iter().zip(cursors) {
        // A full association page can become empty but must still offer navigation.
        let (body, links) = saved_page(&client, &base, &format!("{endpoint}?limit=1")).await?;
        assert!(ids(&body).is_empty());
        assert_links(&links, cursor[0], Some(cursor[0]));
        // Filtering both boundaries must not replace association cursors with visible IDs.
        let (body, links) = saved_page(&client, &base, &format!("{endpoint}?limit=3")).await?;
        assert_eq!(ids(&body), vec![expected[1].clone()]);
        assert_links(&links, cursor[0], Some(cursor[2]));
        let (body, links) = saved_page(
            &client,
            &base,
            &format!("{endpoint}?limit=3&max_id={}", cursor[2]),
        )
        .await?;
        assert_eq!(ids(&body), vec![expected[3].clone(), expected[4].clone()]);
        assert_links(&links, cursor[3], None);
        // min_id selects the nearest newer associations, then returns descending order.
        for query in [
            format!("limit=2&min_id={}", cursor[4]),
            format!("limit=2&since_id={}", cursor[2]),
        ] {
            let (body, links) = saved_page(&client, &base, &format!("{endpoint}?{query}")).await?;
            let (visible, first, last) = if query.contains("min_id") {
                (&expected[3], cursor[2], cursor[3])
            } else {
                (&expected[1], cursor[0], cursor[1])
            };
            assert_eq!(ids(&body), vec![visible.clone()]);
            assert_links(&links, first, Some(last));
        }
        let (body, links) =
            saved_page(&client, &base, &format!("{endpoint}?max_id={}", cursor[4])).await?;
        assert!(ids(&body).is_empty());
        assert!(links.is_empty());
    }
    server.abort();
    Ok(())
}
