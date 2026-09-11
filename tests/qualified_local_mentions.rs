//! R11: runs only against the disposable restored fixture from tools/mastodon-fixture.
use std::error::Error;

use chrono::Duration;
use rustodon::jobs::{Lane, Queue};
use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{Repository, WriteRepository};
use rustodon::web::{WebState, router};
use rustodon::worker::{WorkerExecutor, infrastructure_handlers_with_writer};
use serde_json::{Value, json};
use url::Url;

const VIEWER: i64 = 116_844_606_259_201_004;
const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const AUTHOR_TOKEN: &str = "fixture-bearer-token-v4-6-5";
const VIEWER_TOKEN: &str = "fixture-bearer-api-moderator-v4-6-5";
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

async fn publish(
    client: &reqwest::Client,
    base: &str,
    status: Option<i64>,
    text: &str,
) -> TestResult<i64> {
    let request = if let Some(id) = status {
        client.put(format!("{base}/api/v1/statuses/{id}"))
    } else {
        client.post(format!("{base}/api/v1/statuses"))
    };
    let response = request
        .header("host", DOMAIN)
        .bearer_auth(AUTHOR_TOKEN)
        .header("content-type", "application/json")
        .body(json!({"status": text, "visibility": "direct"}).to_string())
        .send()
        .await?;
    let code = response.status();
    let body: Value = serde_json::from_str(&response.text().await?)?;
    assert_eq!(code, 200, "publish: {body}");
    Ok(body["id"].as_str().expect("status ID").parse()?)
}

#[tokio::test]
#[ignore = "requires a disposable restored fixture from tools/mastodon-fixture schema-read-test qualified_local_mentions"]
#[allow(clippy::too_many_lines)]
async fn local_mention_spellings_grant_identical_access_and_notifications() -> TestResult {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let mut connection = <sqlx::PgConnection as sqlx::Connection>::connect(&owner_url).await?;
    rustodon::operational_schema::migrate(&mut connection).await?;
    let writer = WriteRepository::connect(&owner_url).await?;
    let repository = Repository::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    let state = WebState::new(
        repository,
        // Deliberately distinct from LOCAL_DOMAIN: account identity is not the web host.
        Url::parse("https://web.fixture.invalid/")?,
        DOMAIN,
        "/system",
        std::env::temp_dir(),
        runtime(),
        Vec::new(),
        vec![DOMAIN.to_owned()],
    )?
    .with_write_repository(writer.clone());
    let queue = Queue::new(writer.pool().clone());
    let executor = WorkerExecutor::new(
        queue.clone(),
        infrastructure_handlers_with_writer(&queue, Some(writer.pool().clone()))?,
        1,
        1,
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let client = reqwest::Client::new();
    let spellings = [
        "@api_moderator".to_owned(),
        format!("@api_moderator@{DOMAIN}"),
        format!("@API_MODERATOR@{}", DOMAIN.to_ascii_uppercase()),
        format!("@api_moderator @api_moderator@{DOMAIN}"),
        "@api_moderator@remote.fixture.invalid".to_owned(),
        "@api_moderator@web.fixture.invalid".to_owned(),
    ];
    let mut actual = Vec::new();
    let mut expected = Vec::new();
    for edit in [false, true] {
        for (index, spelling) in spellings.iter().enumerate() {
            let initial = if edit {
                Some(publish(&client, &base, None, "no recipient yet").await?)
            } else {
                None
            };
            if let Some(id) = initial {
                let response = client
                    .get(format!("{base}/api/v1/statuses/{id}"))
                    .header("host", DOMAIN)
                    .bearer_auth(VIEWER_TOKEN)
                    .send()
                    .await?;
                assert_eq!(response.status(), 404, "no grant before edit");
            }
            let id = publish(&client, &base, initial, &format!("{spelling} R11 direct")).await?;
            // Exercise the real durable outbox -> core worker -> notification writer path.
            queue.dispatch_outbox(1000).await?;
            while executor
                .process_one("r11-notifications", &[Lane::Core], Duration::seconds(30))
                .await?
            {}
            let mentions: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM mentions WHERE status_id = $1 AND account_id = $2 AND NOT silent",
            )
            .bind(id)
            .bind(VIEWER)
            .fetch_one(writer.pool())
            .await?;
            let notifications: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM notifications n JOIN mentions m ON m.id = n.activity_id \
                 WHERE n.activity_type = 'Mention' AND n.account_id = $2 AND m.status_id = $1",
            )
            .bind(id)
            .bind(VIEWER)
            .fetch_one(writer.pool())
            .await?;
            let response = client
                .get(format!("{base}/api/v1/statuses/{id}"))
                .header("host", DOMAIN)
                .bearer_auth(VIEWER_TOKEN)
                .send()
                .await?;
            let code = response.status().as_u16();
            if code == 200 {
                let body: Value = serde_json::from_str(&response.text().await?)?;
                assert_eq!(body["visibility"], "direct");
                assert_eq!(body["mentions"][0]["id"], VIEWER.to_string());
                assert!(
                    body["content"]
                        .as_str()
                        .expect("content")
                        .contains("R11 direct")
                );
            }
            let response = client
                .get(format!(
                    "{base}/api/v1/notifications?types[]=mention&limit=80"
                ))
                .header("host", DOMAIN)
                .bearer_auth(VIEWER_TOKEN)
                .send()
                .await?;
            assert_eq!(response.status(), 200);
            let body: Value = serde_json::from_str(&response.text().await?)?;
            let status_id = id.to_string();
            let visible_notifications = body
                .as_array()
                .expect("notifications")
                .iter()
                .filter(|notification| notification["status"]["id"] == status_id.as_str())
                .count();
            // Anonymous users must not gain access even when the recipient does.
            assert_eq!(
                client
                    .get(format!("{base}/api/v1/statuses/{id}"))
                    .header("host", DOMAIN)
                    .send()
                    .await?
                    .status(),
                404
            );
            actual.push((
                edit,
                index,
                mentions,
                notifications,
                code,
                visible_notifications,
            ));
            expected.push(if index < 4 {
                (edit, index, 1, 1, 200, 1)
            } else {
                (edit, index, 0, 0, 404, 0)
            });
        }
    }
    server.abort();
    // Collect the whole matrix first so RED exposes both create and edit failures.
    assert_eq!(
        actual, expected,
        "(edit, spelling, mentions, notifications, HTTP, visible notifications)"
    );
    Ok(())
}
