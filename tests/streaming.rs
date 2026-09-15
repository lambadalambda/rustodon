use std::path::PathBuf;

use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use rustodon::jobs::{Queue, record_stream_event_in};
use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{Repository, WriteRepository};
use rustodon::streaming::{
    STREAM_EVENT_KIND, SYSTEM_KILL_EVENT, TOKEN_KILL_EVENT, event_logical_key,
};
use rustodon::web::{WebState, router};
use serde_json::json;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::{Duration, timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest, http::header::SEC_WEBSOCKET_PROTOCOL},
};
use url::Url;

const ACCESS_TOKEN: &str = "fixture-bearer-token-v4-6-5";
const ACCOUNT_ID: i64 = 116_844_606_259_201_001;
const MODERATOR_ID: i64 = 116_844_606_259_201_002;
const PUBLIC_STATUS_ID: i64 = 116_844_842_188_805_001;
const REMOTE_MEDIA_STATUS_ID: i64 = 116_845_105_643_525_105;
const UNAUTHORIZED_STATUS_ID: i64 = -312;
const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
const REVOKED_STREAM_TOKEN: &str = "fixture-stream-revoked-token-v4-6-5";
const ACTIVE_STREAM_TOKEN: &str = "fixture-stream-active-token-v4-6-5";
const REVOKED_STREAM_TOKEN_ID: i64 = 9_000_000_001;
const ACTIVE_STREAM_TOKEN_ID: i64 = 9_000_000_002;

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn websocket_protocol_token_is_echoed_in_handshake() -> Result<(), Box<dyn std::error::Error>>
{
    let read_url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let write_url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let repository = Repository::connect(&read_url).await?;
    let read_pool = PgPool::connect(&read_url).await?;
    let write_pool = PgPool::connect(&write_url).await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let media_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("streaming-media-protocol-{}", std::process::id()));
    std::fs::create_dir_all(&media_root)?;
    let state = WebState::new(
        repository,
        Url::parse(ORIGIN)?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        media_root.clone(),
        InstanceRuntimeConfig {
            domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            version: "4.6.5".to_owned(),
            source_url: "https://github.com/mastodon/mastodon".to_owned(),
            streaming_api: "wss://fixture-v4-6-5.rustodon.invalid".to_owned(),
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
        },
        Vec::new(),
        vec![format!("127.0.0.1:{}", address.port())],
    )?
    .with_queue(Queue::new(read_pool));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let mut request = format!("ws://{address}/api/v1/streaming/").into_client_request()?;
    request
        .headers_mut()
        .insert(SEC_WEBSOCKET_PROTOCOL, ACCESS_TOKEN.parse()?);
    let (mut socket, response) = connect_async(request).await?;
    assert_eq!(
        response
            .headers()
            .get(SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok()),
        Some(ACCESS_TOKEN)
    );

    socket
        .send(Message::Text(
            json!({"type": "subscribe", "stream": "user"})
                .to_string()
                .into(),
        ))
        .await?;
    let logical_key = event_logical_key(ACCOUNT_ID, "delete", 42, Utc::now().timestamp_micros());
    let mut transaction = write_pool.begin().await?;
    let event_id =
        record_stream_event_in(&mut transaction, ACCOUNT_ID, "delete", 42, &logical_key).await?;
    transaction.commit().await?;
    let message = timeout(Duration::from_secs(5), socket.next())
        .await?
        .ok_or_else(|| std::io::Error::other("protocol-token stream closed before event"))??;
    let Message::Text(message) = message else {
        return Err(
            std::io::Error::other("protocol-token stream returned a non-text event").into(),
        );
    };
    let envelope: serde_json::Value = serde_json::from_str(message.as_ref())?;
    assert_eq!(envelope["stream"], json!(["user"]));
    assert_eq!(envelope["event"], "delete");
    assert_eq!(envelope["payload"], "42");
    socket.close(None).await?;

    let query_endpoint = format!("ws://{address}/api/v1/streaming/?access_token={ACCESS_TOKEN}");
    let (mut query_socket, query_response) = connect_async(query_endpoint).await?;
    assert_eq!(query_response.status().as_u16(), 101);
    assert!(
        !query_response
            .headers()
            .contains_key(SEC_WEBSOCKET_PROTOCOL)
    );
    query_socket.close(None).await?;

    sqlx::query("DELETE FROM rustodon.outbox_events WHERE id = $1")
        .bind(event_id)
        .execute(&write_pool)
        .await?;
    let _ = shutdown_tx.send(());
    server.await??;
    std::fs::remove_dir_all(media_root)?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn websocket_stream_delivers_one_deduplicated_mastodon_envelope()
-> Result<(), Box<dyn std::error::Error>> {
    let read_url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let write_url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let repository = Repository::connect(&read_url).await?;
    let read_pool = PgPool::connect(&read_url).await?;
    let write_pool = PgPool::connect(&write_url).await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let media_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("streaming-media-{}", std::process::id()));
    std::fs::create_dir_all(&media_root)?;

    let state = WebState::new(
        repository,
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        media_root.clone(),
        InstanceRuntimeConfig {
            domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            version: "4.6.5".to_owned(),
            source_url: "https://github.com/mastodon/mastodon".to_owned(),
            streaming_api: "wss://fixture-v4-6-5.rustodon.invalid".to_owned(),
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
        },
        Vec::new(),
        vec![format!("127.0.0.1:{}", address.port())],
    )?
    .with_queue(Queue::new(read_pool));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let version = Utc::now().timestamp_micros();
    let logical_keys = [
        event_logical_key(ACCOUNT_ID, "delete", 42, version),
        event_logical_key(ACCOUNT_ID, "update", UNAUTHORIZED_STATUS_ID, version + 1),
        event_logical_key(ACCOUNT_ID, "update", PUBLIC_STATUS_ID, version + 2),
        event_logical_key(
            ACCOUNT_ID,
            "status.update",
            REMOTE_MEDIA_STATUS_ID,
            version + 3,
        ),
    ];
    let reconnect_logical_key =
        event_logical_key(ACCOUNT_ID, "update", PUBLIC_STATUS_ID, version + 4);
    for logical_key in &logical_keys {
        sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
            .bind(STREAM_EVENT_KIND)
            .bind(logical_key)
            .execute(&write_pool)
            .await?;
    }
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
        .bind(STREAM_EVENT_KIND)
        .bind(&reconnect_logical_key)
        .execute(&write_pool)
        .await?;

    let endpoint = format!("ws://{address}/api/v1/streaming/user?access_token={ACCESS_TOKEN}");
    let (mut socket, response) = connect_async(endpoint).await?;
    assert_eq!(response.status().as_u16(), 101);

    let mut transaction = write_pool.begin().await?;
    let first =
        record_stream_event_in(&mut transaction, ACCOUNT_ID, "delete", 42, &logical_keys[0])
            .await?;
    let duplicate =
        record_stream_event_in(&mut transaction, ACCOUNT_ID, "delete", 42, &logical_keys[0])
            .await?;
    let unauthorized = record_stream_event_in(
        &mut transaction,
        ACCOUNT_ID,
        "update",
        UNAUTHORIZED_STATUS_ID,
        &logical_keys[1],
    )
    .await?;
    let allowed = record_stream_event_in(
        &mut transaction,
        ACCOUNT_ID,
        "update",
        PUBLIC_STATUS_ID,
        &logical_keys[2],
    )
    .await?;
    let media_update = record_stream_event_in(
        &mut transaction,
        ACCOUNT_ID,
        "status.update",
        REMOTE_MEDIA_STATUS_ID,
        &logical_keys[3],
    )
    .await?;
    transaction.commit().await?;
    assert_eq!(first, duplicate);

    socket
        .send(Message::Text(
            json!({"type": "subscribe", "stream": "user"})
                .to_string()
                .into(),
        ))
        .await?;

    let message = timeout(Duration::from_secs(5), socket.next())
        .await?
        .ok_or_else(|| std::io::Error::other("stream closed before event"))??;
    let Message::Text(message) = message else {
        return Err(std::io::Error::other("stream returned a non-text event").into());
    };
    let envelope: serde_json::Value = serde_json::from_str(message.as_ref())?;
    assert_eq!(envelope["stream"], json!(["user"]));
    assert_eq!(envelope["event"], "delete");
    assert_eq!(envelope["payload"], "42");

    let message = timeout(Duration::from_secs(5), socket.next())
        .await?
        .ok_or_else(|| std::io::Error::other("stream closed before authorized status event"))??;
    let Message::Text(message) = message else {
        return Err(std::io::Error::other("stream returned a non-text status event").into());
    };
    let envelope: serde_json::Value = serde_json::from_str(message.as_ref())?;
    assert_eq!(envelope["stream"], json!(["user"]));
    assert_eq!(envelope["event"], "update");
    let payload = envelope["payload"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("status event payload was not serialized JSON"))?;
    let payload: serde_json::Value = serde_json::from_str(payload)?;
    assert_eq!(payload["id"], json!(PUBLIC_STATUS_ID.to_string()));

    let message = timeout(Duration::from_secs(5), socket.next())
        .await?
        .ok_or_else(|| std::io::Error::other("stream closed before media status update"))??;
    let Message::Text(message) = message else {
        return Err(std::io::Error::other("stream returned a non-text media update").into());
    };
    let envelope: serde_json::Value = serde_json::from_str(message.as_ref())?;
    assert_eq!(envelope["stream"], json!(["user"]));
    assert_eq!(envelope["event"], "status.update");
    let payload = envelope["payload"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("media update payload was not serialized JSON"))?;
    let payload: serde_json::Value = serde_json::from_str(payload)?;
    assert_eq!(payload["id"], json!(REMOTE_MEDIA_STATUS_ID.to_string()));
    let attachment = &payload["media_attachments"][0];
    let local_media_prefix = format!("{ORIGIN}system/cache/media_attachments/files/");
    let url = attachment["url"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("media update has no local original URL"))?;
    let preview_url = attachment["preview_url"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("media update has no local preview URL"))?;
    assert!(
        url.starts_with(&local_media_prefix),
        "unexpected media URL: {url}"
    );
    assert!(
        preview_url.starts_with(&local_media_prefix),
        "unexpected preview URL: {preview_url}"
    );
    assert_ne!(url, "https://remote.fixture.invalid/media/cached.jpg");
    assert_eq!(attachment["meta"]["original"]["aspect"], json!(1.5));

    socket.close(None).await?;
    let reconnect_endpoint =
        format!("ws://{address}/api/v1/streaming/user?access_token={ACCESS_TOKEN}");
    let (mut reconnected, reconnect_response) = connect_async(reconnect_endpoint).await?;
    assert_eq!(reconnect_response.status().as_u16(), 101);

    let mut transaction = write_pool.begin().await?;
    let reconnected_event = record_stream_event_in(
        &mut transaction,
        ACCOUNT_ID,
        "update",
        PUBLIC_STATUS_ID,
        &reconnect_logical_key,
    )
    .await?;
    transaction.commit().await?;

    reconnected
        .send(Message::Text(
            json!({"type": "subscribe", "stream": "user"})
                .to_string()
                .into(),
        ))
        .await?;
    let message = timeout(Duration::from_secs(5), reconnected.next())
        .await?
        .ok_or_else(|| std::io::Error::other("reconnected stream closed before unseen event"))??;
    let Message::Text(message) = message else {
        return Err(std::io::Error::other("reconnected stream returned a non-text event").into());
    };
    let envelope: serde_json::Value = serde_json::from_str(message.as_ref())?;
    assert_eq!(envelope["stream"], json!(["user"]));
    assert_eq!(envelope["event"], "update");
    let payload = envelope["payload"].as_str().ok_or_else(|| {
        std::io::Error::other("reconnected event payload was not serialized JSON")
    })?;
    let payload: serde_json::Value = serde_json::from_str(payload)?;
    assert_eq!(payload["id"], json!(PUBLIC_STATUS_ID.to_string()));
    assert!(
        timeout(Duration::from_millis(500), reconnected.next())
            .await
            .is_err()
    );
    reconnected.close(None).await?;

    for event_id in [
        first,
        unauthorized,
        allowed,
        media_update,
        reconnected_event,
    ] {
        sqlx::query("DELETE FROM rustodon.outbox_events WHERE id = $1")
            .bind(event_id)
            .execute(&write_pool)
            .await?;
    }
    let _ = shutdown_tx.send(());
    server.await??;
    std::fs::remove_dir_all(media_root)?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn websocket_stream_revocation_is_token_specific() -> Result<(), Box<dyn std::error::Error>> {
    let read_url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let write_url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let repository = Repository::connect(&read_url).await?;
    let read_pool = PgPool::connect(&read_url).await?;
    let write_pool = PgPool::connect(&write_url).await?;
    let (application_id, user_id, account_id) = sqlx::query_as::<_, (Option<i64>, i64, i64)>(
        "SELECT access_token.application_id, access_token.resource_owner_id, user_record.account_id
           FROM oauth_access_tokens access_token
           JOIN users user_record ON user_record.id = access_token.resource_owner_id
          WHERE access_token.token = $1",
    )
    .bind(ACCESS_TOKEN)
    .fetch_one(&write_pool)
    .await?;
    let application_id = application_id
        .ok_or_else(|| std::io::Error::other("fixture token has no OAuth application"))?;
    let (client_id, client_secret) = sqlx::query_as::<_, (String, String)>(
        "SELECT uid, secret FROM oauth_applications WHERE id = $1",
    )
    .bind(application_id)
    .fetch_one(&write_pool)
    .await?;
    sqlx::query(
        "DELETE FROM oauth_access_tokens
           WHERE token IN ($1, $2)",
    )
    .bind(REVOKED_STREAM_TOKEN)
    .bind(ACTIVE_STREAM_TOKEN)
    .execute(&write_pool)
    .await?;
    let revoked_token_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO oauth_access_tokens (id,
             application_id, created_at, expires_in, last_used_at, last_used_ip,
             refresh_token, resource_owner_id, revoked_at, scopes, token)
         VALUES ($1, $2, clock_timestamp(), NULL, NULL, NULL, NULL, $3, NULL, 'read write', $4)
         RETURNING id",
    )
    .bind(REVOKED_STREAM_TOKEN_ID)
    .bind(application_id)
    .bind(user_id)
    .bind(REVOKED_STREAM_TOKEN)
    .fetch_one(&write_pool)
    .await?;
    let active_token_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO oauth_access_tokens (id,
             application_id, created_at, expires_in, last_used_at, last_used_ip,
             refresh_token, resource_owner_id, revoked_at, scopes, token)
         VALUES ($1, $2, clock_timestamp(), NULL, NULL, NULL, NULL, $3, NULL, 'read write', $4)
         RETURNING id",
    )
    .bind(ACTIVE_STREAM_TOKEN_ID)
    .bind(application_id)
    .bind(user_id)
    .bind(ACTIVE_STREAM_TOKEN)
    .fetch_one(&write_pool)
    .await?;

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let media_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("streaming-media-revocation-{}", std::process::id()));
    std::fs::create_dir_all(&media_root)?;
    let state = WebState::new(
        repository,
        Url::parse(ORIGIN)?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        media_root.clone(),
        InstanceRuntimeConfig {
            domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            version: "4.6.5".to_owned(),
            source_url: "https://github.com/mastodon/mastodon".to_owned(),
            streaming_api: "wss://fixture-v4-6-5.rustodon.invalid".to_owned(),
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
            sso_signup_url: None,
            terms_of_service_url: None,
            wrapstodon: None,
        },
        Vec::new(),
        vec![format!("127.0.0.1:{}", address.port())],
    )?
    .with_queue(Queue::new(read_pool));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let revoked_endpoint =
        format!("ws://{address}/api/v1/streaming/user?access_token={REVOKED_STREAM_TOKEN}");
    let active_endpoint =
        format!("ws://{address}/api/v1/streaming/user?access_token={ACTIVE_STREAM_TOKEN}");
    let (mut revoked_socket, revoked_response) = connect_async(&revoked_endpoint).await?;
    let (mut active_socket, active_response) = connect_async(&active_endpoint).await?;
    assert_eq!(revoked_response.status().as_u16(), 101);
    assert_eq!(active_response.status().as_u16(), 101);
    let subscribe = Message::Text(
        json!({"type": "subscribe", "stream": "user"})
            .to_string()
            .into(),
    );
    revoked_socket.send(subscribe.clone()).await?;
    active_socket.send(subscribe).await?;

    let writer = WriteRepository::from_pool(write_pool.clone());
    writer
        .revoke_oauth_token(&client_id, &client_secret, Some(REVOKED_STREAM_TOKEN), None)
        .await
        .map_err(|error| {
            let message = match error {
                rustodon::mastodon::OAuthTokenRevocationError::InvalidClient => "invalid client",
                rustodon::mastodon::OAuthTokenRevocationError::UnauthorizedClient => {
                    "unauthorized client"
                }
                rustodon::mastodon::OAuthTokenRevocationError::Database(error) => {
                    return std::io::Error::other(format!(
                        "fixture token revocation failed: database error: {error:?}"
                    ));
                }
            };
            std::io::Error::other(format!("fixture token revocation failed: {message}"))
        })?;

    let close = timeout(Duration::from_secs(3), revoked_socket.next())
        .await?
        .ok_or_else(|| std::io::Error::other("revoked stream closed without a close frame"))??;
    let Message::Close(Some(frame)) = close else {
        return Err(std::io::Error::other("revoked stream returned a non-close event").into());
    };
    assert_eq!(u16::from(frame.code), 1000);
    assert!(
        timeout(Duration::from_millis(500), active_socket.next())
            .await
            .is_err()
    );

    let reconnect = connect_async(&revoked_endpoint).await;
    let Err(tokio_tungstenite::tungstenite::Error::Http(response)) = reconnect else {
        return Err(std::io::Error::other("revoked token reconnected successfully").into());
    };
    assert_eq!(response.status().as_u16(), 401);
    active_socket.close(None).await?;

    let logical_key = event_logical_key(account_id, TOKEN_KILL_EVENT, revoked_token_id, 0);
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
        .bind(STREAM_EVENT_KIND)
        .bind(logical_key)
        .execute(&write_pool)
        .await?;
    sqlx::query("DELETE FROM oauth_access_tokens WHERE id IN ($1, $2)")
        .bind(revoked_token_id)
        .bind(active_token_id)
        .execute(&write_pool)
        .await?;
    let _ = shutdown_tx.send(());
    server.await??;
    std::fs::remove_dir_all(media_root)?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn websocket_stream_terminates_when_local_account_is_suspended()
-> Result<(), Box<dyn std::error::Error>> {
    let read_url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let write_url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let repository = Repository::connect(&read_url).await?;
    let read_pool = PgPool::connect(&read_url).await?;
    let write_pool = PgPool::connect(&write_url).await?;
    let writer = WriteRepository::from_pool(write_pool.clone());

    let already_suspended = sqlx::query_scalar::<_, bool>(
        "SELECT suspended_at IS NOT NULL FROM accounts WHERE id = $1",
    )
    .bind(ACCOUNT_ID)
    .fetch_one(&write_pool)
    .await?;
    if already_suspended {
        writer
            .set_account_suspension(MODERATOR_ID, ACCOUNT_ID, false, ORIGIN)
            .await?;
    }
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE kind = $1 AND payload ->> 'account_id' = $2 AND payload ->> 'event' = $3",
    )
    .bind(STREAM_EVENT_KIND)
    .bind(ACCOUNT_ID.to_string())
    .bind(SYSTEM_KILL_EVENT)
    .execute(&write_pool)
    .await?;

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let media_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("streaming-media-suspension-{}", std::process::id()));
    std::fs::create_dir_all(&media_root)?;
    let state = WebState::new(
        repository,
        Url::parse(ORIGIN)?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        media_root.clone(),
        InstanceRuntimeConfig {
            domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            version: "4.6.5".to_owned(),
            source_url: "https://github.com/mastodon/mastodon".to_owned(),
            streaming_api: "wss://fixture-v4-6-5.rustodon.invalid".to_owned(),
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
        },
        Vec::new(),
        vec![format!("127.0.0.1:{}", address.port())],
    )?
    .with_queue(Queue::new(read_pool));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let endpoint = format!("ws://{address}/api/v1/streaming?access_token={ACCESS_TOKEN}");
    let (mut socket, response) = connect_async(endpoint).await?;
    assert_eq!(response.status().as_u16(), 101);

    let operation = async {
        writer
            .set_account_suspension(MODERATOR_ID, ACCOUNT_ID, true, ORIGIN)
            .await?;
        let message = timeout(Duration::from_secs(3), socket.next())
            .await?
            .ok_or_else(|| std::io::Error::other("stream closed without a close frame"))??;
        let Message::Close(Some(frame)) = message else {
            return Err(std::io::Error::other("stream returned a non-close event").into());
        };
        assert_eq!(u16::from(frame.code), 1000);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    drop(socket);

    let cleanup = async {
        let suspended = sqlx::query_scalar::<_, bool>(
            "SELECT suspended_at IS NOT NULL FROM accounts WHERE id = $1",
        )
        .bind(ACCOUNT_ID)
        .fetch_one(&write_pool)
        .await?;
        if suspended {
            writer
                .set_account_suspension(MODERATOR_ID, ACCOUNT_ID, false, ORIGIN)
                .await?;
        }
        sqlx::query(
            "DELETE FROM rustodon.outbox_events
              WHERE kind = $1 AND payload ->> 'account_id' = $2 AND payload ->> 'event' = $3",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(ACCOUNT_ID.to_string())
        .bind(SYSTEM_KILL_EVENT)
        .execute(&write_pool)
        .await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    let _ = shutdown_tx.send(());
    server.await??;
    std::fs::remove_dir_all(media_root)?;
    cleanup?;
    operation?;
    Ok(())
}
