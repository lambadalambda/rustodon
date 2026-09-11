use std::sync::Arc;

use ipnetwork::IpNetwork;
use rustodon::mastodon::{WriteRepository, verify_password};
use sqlx::Connection;
use tokio::sync::Barrier;

use super::safety::DifferentialConfig;

/// Exercise the actual split authentication/write boundaries, not a timing sleep.
pub async fn recovery_fences(config: DifferentialConfig) -> Result<(), Box<dyn std::error::Error>> {
    config.validate_database_comments().await?;
    let url = config.rust_write_database.as_ref().unwrap().url();
    let writer = WriteRepository::connect(url).await?;
    let ip = IpNetwork::from("192.0.2.80".parse::<std::net::IpAddr>()?);
    let mut rejected = Vec::new();
    for (username, login) in [("fencelogin", true), ("fencechange", false)] {
        let email = format!("{username}@fixture.invalid");
        let user = writer
            .create_local_user(&email, username, "old-fixture-password")
            .await?;
        let token = writer.create_password_reset_token(&email).await?.unwrap();
        let checked = Arc::new(Barrier::new(2));
        let recovered = Arc::new(Barrier::new(2));
        let request_writer = WriteRepository::connect(url).await?;
        let request = {
            let checked = checked.clone();
            let recovered = recovered.clone();
            let email = email.clone();
            tokio::spawn(async move {
                let authentication = request_writer
                    .authenticate_browser_user(
                        &email,
                        "old-fixture-password",
                        None,
                        1_000_000_000,
                        ip,
                        "fence-test",
                    )
                    .await
                    .unwrap();
                let password_authentication = request_writer
                    .verify_current_password(user.user_id, "old-fixture-password")
                    .await
                    .unwrap();
                request_writer
                    .create_browser_session(&authentication, ip, "pre-reset-session")
                    .await
                    .unwrap();
                checked.wait().await;
                recovered.wait().await;
                if login {
                    matches!(
                        request_writer
                            .create_browser_session(&authentication, ip, "fence-test")
                            .await,
                        Err(rustodon::mastodon::WriteError::Unauthorized)
                    )
                } else {
                    matches!(
                        request_writer
                            .change_user_password(
                                &password_authentication,
                                "stale-fixture-password"
                            )
                            .await,
                        Err(rustodon::mastodon::WriteError::Unauthorized)
                    )
                }
            })
        };
        checked.wait().await;
        assert!(
            writer
                .reset_password_with_token(&token, "recovered-fixture-password")
                .await?
        );
        recovered.wait().await;
        let denied = request.await?;
        let mut connection = sqlx::PgConnection::connect(url).await?;
        let current: String =
            sqlx::query_scalar("SELECT encrypted_password FROM users WHERE id = $1")
                .bind(user.user_id)
                .fetch_one(&mut connection)
                .await?;
        let password_preserved = verify_password("recovered-fixture-password", &current);
        let sessions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM session_activations WHERE user_id = $1")
                .bind(user.user_id)
                .fetch_one(&mut connection)
                .await?;
        let tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oauth_access_tokens WHERE resource_owner_id = $1 AND revoked_at IS NULL")
            .bind(user.user_id).fetch_one(&mut connection).await?;
        eprintln!(
            "{username}: rejected={denied}, recovered_password={password_preserved}, sessions={sessions}, live_tokens={tokens}"
        );
        rejected.push(denied && password_preserved && sessions == 0 && tokens == 0);
        fresh_credentials_work(&writer, &email, user.user_id, ip).await?;
    }
    assert_eq!(
        rejected,
        [true, true],
        "recovery must fence both stale requests"
    );
    Ok(())
}

async fn fresh_credentials_work(
    writer: &WriteRepository,
    email: &str,
    user_id: i64,
    ip: IpNetwork,
) -> Result<(), Box<dyn std::error::Error>> {
    let authentication = writer
        .authenticate_browser_user(
            email,
            "recovered-fixture-password",
            None,
            1_000_000_000,
            ip,
            "fresh-login",
        )
        .await?;
    writer
        .create_browser_session(&authentication, ip, "fresh-login")
        .await?;
    let password = writer
        .verify_current_password(user_id, "recovered-fixture-password")
        .await?;
    writer
        .change_user_password(&password, "legitimate-fixture-password")
        .await?;
    writer
        .verify_current_password(user_id, "legitimate-fixture-password")
        .await?;
    Ok(())
}

// Privileged fixture seeding, deliberately not an application authentication API.
pub async fn fixture_browser_session(
    database_url: &str,
    user_id: i64,
    ip: IpNetwork,
    user_agent: &str,
) -> sqlx::Result<String> {
    let session_id = rustodon::mastodon::random_auth_token(32);
    let access_token = rustodon::mastodon::random_auth_token(32);
    let mut connection = sqlx::PgConnection::connect(database_url).await?;
    let mut transaction = connection.begin().await?;
    let application_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM oauth_applications WHERE superapp = true ORDER BY id LIMIT 1",
    )
    .fetch_optional(&mut *transaction)
    .await?;
    let access_token_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO oauth_access_tokens ( \
                application_id, created_at, expires_in, last_used_at, last_used_ip, \
                refresh_token, resource_owner_id, revoked_at, scopes, token) \
             VALUES ($1, clock_timestamp(), NULL, NULL, NULL, NULL, $2, NULL, \
                     'read write follow', $3) \
             RETURNING id",
    )
    .bind(application_id)
    .bind(user_id)
    .bind(access_token)
    .fetch_one(&mut *transaction)
    .await?;
    let session_id = sqlx::query_scalar::<_, String>(
        "INSERT INTO session_activations ( \
                access_token_id, created_at, ip, session_id, updated_at, user_agent, user_id, \
                web_push_subscription_id) \
             VALUES ($1, clock_timestamp(), $2, $3, clock_timestamp(), $4, $5, NULL) \
             RETURNING session_id",
    )
    .bind(access_token_id)
    .bind(ip)
    .bind(session_id)
    .bind(user_agent)
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(session_id)
}
