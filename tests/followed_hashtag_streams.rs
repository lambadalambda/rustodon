//! R12: disposable restored fixture only; no live instance or federation network.
use std::error::Error;

use futures_util::StreamExt;
use rustodon::jobs::Queue;
use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{Repository, WriteRepository};
use rustodon::streaming::STREAM_EVENT_KIND;
use rustodon::web::{WebState, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::time::{Duration, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

const AUTHOR: i64 = 116_844_606_259_201_001;
const VIEWER: i64 = 116_844_606_259_201_004;
const MENTIONED: i64 = 116_844_606_259_201_002;
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
    id: Option<i64>,
    text: &str,
    visibility: &str,
) -> TestResult<i64> {
    let request = if let Some(id) = id {
        client.put(format!("{base}/api/v1/statuses/{id}"))
    } else {
        client.post(format!("{base}/api/v1/statuses"))
    };
    let response = request
        .header("host", DOMAIN)
        .bearer_auth(AUTHOR_TOKEN)
        .header("content-type", "application/json")
        .body(json!({"status": text, "visibility": visibility}).to_string())
        .send()
        .await?;
    let code = response.status();
    let body: Value = serde_json::from_str(&response.text().await?)?;
    assert_eq!(code, 200, "publish: {body}");
    Ok(body["id"].as_str().expect("status ID").parse()?)
}

async fn home_contains(client: &reqwest::Client, base: &str, id: i64) -> TestResult<bool> {
    let response = client
        .get(format!("{base}/api/v1/timelines/home?limit=40"))
        .header("host", DOMAIN)
        .bearer_auth(VIEWER_TOKEN)
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    let body: Value = serde_json::from_str(&response.text().await?)?;
    let id = id.to_string();
    Ok(body
        .as_array()
        .expect("home statuses")
        .iter()
        .any(|status| status["id"] == id.as_str()))
}

async fn events(pool: &PgPool, id: i64) -> TestResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT payload ->> 'event' FROM rustodon.outbox_events \
         WHERE kind = $1 AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3 ORDER BY id",
    ).bind(STREAM_EVENT_KIND).bind(VIEWER.to_string()).bind(id.to_string()).fetch_all(pool).await?)
}

async fn delete(client: &reqwest::Client, base: &str, id: i64) -> TestResult {
    let response = client
        .delete(format!("{base}/api/v1/statuses/{id}"))
        .header("host", DOMAIN)
        .bearer_auth(AUTHOR_TOKEN)
        .send()
        .await?;
    assert_eq!(response.status(), 200, "delete {}", response.text().await?);
    Ok(())
}

// Policy rows are fixture setup, not application writes. Restore each before the next case.
async fn policy(pool: &PgPool, name: &str, enabled: bool) -> TestResult {
    if name == "silenced" {
        sqlx::query("UPDATE accounts SET silenced_at = CASE WHEN $2 THEN clock_timestamp() ELSE NULL END WHERE id = $1")
            .bind(AUTHOR).bind(enabled).execute(pool).await?;
    } else if matches!(
        name,
        "viewer_block" | "author_block" | "mentioned_block" | "mute" | "mentioned_mute"
    ) {
        let (source, target) = match name {
            "author_block" => (AUTHOR, VIEWER),
            "mentioned_block" | "mentioned_mute" => (VIEWER, MENTIONED),
            _ => (VIEWER, AUTHOR),
        };
        let table = if name.contains("mute") {
            "mutes"
        } else {
            "blocks"
        };
        if enabled {
            sqlx::query(&format!("INSERT INTO {table} (account_id, target_account_id, created_at, updated_at) VALUES ($1, $2, clock_timestamp(), clock_timestamp())"))
                .bind(source).bind(target).execute(pool).await?;
        } else {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE account_id = $1 AND target_account_id = $2"
            ))
            .bind(source)
            .bind(target)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires tools/mastodon-fixture schema-read-test followed_hashtag_streams"]
#[allow(clippy::too_many_lines)]
async fn hashtag_only_home_membership_matches_stream_lifecycle() -> TestResult {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let mut connection = <sqlx::PgConnection as sqlx::Connection>::connect(&owner_url).await?;
    let writer_url = std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?;
    rustodon::operational_schema::migrate_with_writer_role(
        &mut connection,
        Some(Url::parse(&writer_url)?.username()),
    )
    .await?;
    let writer =
        WriteRepository::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let owner = PgPool::connect(&owner_url).await?;
    let pool = &owner;
    sqlx::query(
        "UPDATE oauth_access_tokens SET scopes = scopes || ' write:follows' WHERE token = $1",
    )
    .bind(VIEWER_TOKEN)
    .execute(pool)
    .await?;
    // Two followed tags must still produce only one event. No accepted author follow.
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(VIEWER)
        .bind(AUTHOR)
        .execute(pool)
        .await?;
    let repository = Repository::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let base = format!("http://{address}");
    let runtime_pool = PgPool::connect(&std::env::var("RUSTODON_MASTODON_DATABASE_URL")?).await?;
    let state = WebState::new(
        repository,
        Url::parse(&format!("https://{DOMAIN}/"))?,
        DOMAIN,
        "/system",
        std::env::temp_dir(),
        runtime(),
        Vec::new(),
        vec![DOMAIN.to_owned(), address.to_string()],
    )?
    .with_write_repository(writer.clone())
    .with_queue(Queue::new(runtime_pool));
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let client = reqwest::Client::new();
    for name in ["r12one", "r12two"] {
        let response = client
            .post(format!("{base}/api/v1/tags/{name}/follow"))
            .header("host", DOMAIN)
            .bearer_auth(VIEWER_TOKEN)
            .send()
            .await?;
        assert_eq!(response.status(), 200, "{}", response.text().await?);
    }
    let mut actual = Vec::new();
    let mut expected = Vec::new();
    for (name, visibility, allowed) in [
        ("public", "public", true),
        ("unlisted", "unlisted", false),
        ("private", "private", false),
        ("direct", "direct", false),
        ("viewer_block", "public", false),
        ("author_block", "public", false),
        ("mute", "public", false),
        ("mentioned_block", "public", false),
        ("mentioned_mute", "public", false),
        ("silenced", "public", false),
        ("unfollowed_tag", "public", false),
    ] {
        policy(pool, name, true).await?;
        let tags = if name == "unfollowed_tag" {
            "#r12other"
        } else {
            "#r12one #r12two"
        };
        let id = publish(
            &client,
            &base,
            None,
            &format!("@moderator {tags} R12 create"),
            visibility,
        )
        .await?;
        actual.push((
            name,
            "create",
            home_contains(&client, &base, id).await?,
            events(pool, id).await?,
        ));
        expected.push((
            name,
            "create",
            allowed,
            if allowed {
                vec!["update".to_owned()]
            } else {
                vec![]
            },
        ));
        publish(
            &client,
            &base,
            Some(id),
            &format!("@moderator {tags} R12 edit"),
            visibility,
        )
        .await?;
        actual.push((
            name,
            "edit",
            home_contains(&client, &base, id).await?,
            events(pool, id).await?,
        ));
        expected.push((
            name,
            "edit",
            allowed,
            if allowed {
                vec!["update".to_owned(), "status.update".to_owned()]
            } else {
                vec![]
            },
        ));
        delete(&client, &base, id).await?;
        actual.push((
            name,
            "delete",
            home_contains(&client, &base, id).await?,
            events(pool, id).await?,
        ));
        expected.push((
            name,
            "delete",
            false,
            if allowed {
                vec![
                    "update".to_owned(),
                    "status.update".to_owned(),
                    "delete".to_owned(),
                ]
            } else if name == "silenced" {
                // Existing delete fanout intentionally bypasses author silencing:
                // a client may retain a status from before that policy changed.
                vec!["delete".to_owned()]
            } else {
                vec![]
            },
        ));
        policy(pool, name, false).await?;
    }
    // RED collects every lifecycle/negative case before asserting.
    assert_eq!(
        actual, expected,
        "(case, phase, REST home membership, cumulative user events)"
    );

    // Verify real WebSocket envelopes, not just the outbox rows, on a hashtag-only follow.
    let (mut socket, response) = connect_async(format!(
        "ws://{address}/api/v1/streaming/user?access_token={VIEWER_TOKEN}"
    ))
    .await?;
    assert_eq!(response.status().as_u16(), 101);
    let id = publish(
        &client,
        &base,
        None,
        "#r12one #r12two R12 socket create",
        "public",
    )
    .await?;
    for event in ["update", "status.update", "delete"] {
        if event == "status.update" {
            publish(
                &client,
                &base,
                Some(id),
                "#r12one #r12two R12 socket edit",
                "public",
            )
            .await?;
        } else if event == "delete" {
            delete(&client, &base, id).await?;
        }
        let message = timeout(Duration::from_secs(5), socket.next())
            .await?
            .ok_or("stream closed")??;
        let Message::Text(message) = message else {
            return Err("expected text stream event".into());
        };
        let envelope: Value = serde_json::from_str(message.as_ref())?;
        assert_eq!(envelope["stream"], json!(["user"]));
        assert_eq!(envelope["event"], event);
        let payload = envelope["payload"].as_str().expect("stream payload");
        if event == "delete" {
            assert_eq!(payload, id.to_string());
        } else {
            let status: Value = serde_json::from_str(payload)?;
            assert_eq!(status["id"], id.to_string());
            assert!(
                status["content"]
                    .as_str()
                    .expect("content")
                    .contains(if event == "update" {
                        "R12 socket create"
                    } else {
                        "R12 socket edit"
                    })
            );
            assert!(home_contains(&client, &base, id).await?);
        }
    }
    for name in ["r12one", "r12two"] {
        let response = client
            .post(format!("{base}/api/v1/tags/{name}/unfollow"))
            .header("host", DOMAIN)
            .bearer_auth(VIEWER_TOKEN)
            .send()
            .await?;
        assert_eq!(response.status(), 200);
    }
    let after = publish(
        &client,
        &base,
        None,
        "#r12one #r12two after unfollow",
        "public",
    )
    .await?;
    assert!(!home_contains(&client, &base, after).await?);
    assert!(events(pool, after).await?.is_empty());
    assert!(
        timeout(Duration::from_millis(500), socket.next())
            .await
            .is_err()
    );
    socket.close(None).await?;
    server.abort();
    Ok(())
}
