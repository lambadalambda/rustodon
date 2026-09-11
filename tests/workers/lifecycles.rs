use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
const ALICE: i64 = 116_844_606_259_201_001;
const API_MODERATOR: i64 = 116_844_606_259_201_004;
const BOB: i64 = 116_844_606_259_202_001;

fn federation_config() -> ActivityPubDeliveryConfig {
    ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN).unwrap(),
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: None,
        limited_federation: false,
        #[cfg(feature = "test-support")]
        remote_media_endpoint: None,
        #[cfg(feature = "test-support")]
        remote_delivery_endpoint: None,
        #[cfg(feature = "test-support")]
        remote_fetch_endpoint: None,
    }
}

async fn authenticate(
    pool: &sqlx::PgPool,
    token: &str,
    scope: rustodon::mastodon::RequiredScopes,
) -> Result<rustodon::mastodon::AuthenticatedBearer, Box<dyn std::error::Error>> {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        format!("Bearer {token}").parse()?,
    );
    Ok(
        BearerAuthenticator::new(Repository::from_pool(pool.clone()))
            .authenticate(&headers, scope)
            .await?,
    )
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn accepted_locked_follow_preferences_preserve_relationship() -> TestResult {
    accepted_follow_preferences(false, false).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn accepted_remote_follow_preferences_preserve_relationship() -> TestResult {
    accepted_follow_preferences(true, false).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn accepted_silenced_follow_preferences_preserve_relationship() -> TestResult {
    accepted_follow_preferences(false, true).await
}

#[allow(clippy::too_many_lines)]
async fn accepted_follow_preferences(remote: bool, silenced: bool) -> TestResult {
    let pool = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    let writer = WriteRepository::from_pool(pool.clone());
    let authenticated = authenticate(&pool, "fixture-bearer-token-v4-6-5", WRITE_FOLLOWS).await?;
    let target = if remote { BOB } else { API_MODERATOR };
    for table in ["follows", "follow_requests"] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE account_id = $1 AND target_account_id = $2"
        ))
        .bind(ALICE)
        .bind(target)
        .execute(&pool)
        .await?;
    }
    let moderator_scopes: String = sqlx::query_scalar("SELECT scopes FROM oauth_access_tokens WHERE token = 'fixture-bearer-api-moderator-v4-6-5'")
        .fetch_one(&pool).await?;
    sqlx::query("UPDATE oauth_access_tokens SET scopes = scopes || ' write:follows' WHERE token = 'fixture-bearer-api-moderator-v4-6-5'")
        .execute(&pool).await?;
    let original_protocol: i32 = sqlx::query_scalar("SELECT protocol FROM accounts WHERE id = $1")
        .bind(target)
        .fetch_one(&pool)
        .await?;
    if remote {
        sqlx::query("UPDATE accounts SET protocol = 1 WHERE id = $1")
            .bind(target)
            .execute(&pool)
            .await?;
    }
    let original_locked: bool = sqlx::query_scalar("SELECT locked FROM accounts WHERE id = $1")
        .bind(target)
        .fetch_one(&pool)
        .await?;
    sqlx::query("UPDATE accounts SET locked = $2 WHERE id = $1")
        .bind(target)
        .bind(!remote && !silenced)
        .execute(&pool)
        .await?;
    if silenced {
        sqlx::query("UPDATE accounts SET silenced_at = clock_timestamp() WHERE id = $1")
            .bind(ALICE)
            .execute(&pool)
            .await?;
    }
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(pool.clone()),
        None,
        Some(federation_config()),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let first = writer
        .set_follow_with_origin(
            &authenticated,
            target,
            true,
            None,
            None,
            None,
            Some(ORIGIN),
            false,
        )
        .await?;
    assert!(first.request);
    assert!(first.activity_id.is_some());
    let accept = json!({
        "type": "Accept", "actor": "https://remote.fixture.invalid/users/bob",
        "object": first.activity_uri,
    });
    if remote {
        enqueue_accept(&queue, &executor, &accept, "initial").await?;
    } else {
        let target_auth =
            authenticate(&pool, "fixture-bearer-api-moderator-v4-6-5", WRITE_FOLLOWS).await?;
        writer.authorize_follow_request(&target_auth, ALICE).await?;
    }
    let before: (i64, Option<String>) = sqlx::query_as(
        "SELECT id, uri FROM follows WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(ALICE)
    .bind(target)
    .fetch_one(&pool)
    .await?;
    let outbox_before: i64 = sqlx::query_scalar("SELECT count(*) FROM rustodon.outbox_events")
        .fetch_one(&pool)
        .await?;
    let counts_before: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT account_id, following_count, followers_count FROM account_stats WHERE account_id = ANY($1) ORDER BY account_id",
    ).bind(vec![ALICE, target]).fetch_all(&pool).await?;
    for (reblogs, notify, languages) in [
        (
            Some(false),
            Some(true),
            Some(vec!["de".to_owned(), "en".to_owned()]),
        ),
        (None, None, None),
    ] {
        let changed = writer
            .set_follow_with_origin(
                &authenticated,
                target,
                true,
                reblogs,
                notify,
                languages,
                Some(ORIGIN),
                false,
            )
            .await?;
        assert!(
            changed.activity_id.is_none(),
            "preferences must not create another Follow/request"
        );
        assert!(!changed.request);
        let follow: (i64, Option<String>, bool, bool, Option<Vec<String>>) = sqlx::query_as(
            "SELECT id, uri, show_reblogs, notify, languages FROM follows WHERE account_id = $1 AND target_account_id = $2",
        ).bind(ALICE).bind(target).fetch_one(&pool).await?;
        assert_eq!(
            follow,
            (
                before.0,
                before.1.clone(),
                false,
                true,
                Some(vec!["de".to_owned(), "en".to_owned()])
            )
        );
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
            .bind(ALICE).bind(target).fetch_one(&pool).await?, 0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM rustodon.outbox_events")
                .fetch_one(&pool)
                .await?,
            outbox_before,
            "no extra Follow or request notification intent"
        );
    }
    if remote {
        enqueue_accept(&queue, &executor, &accept, "replay").await?;
    }
    let counts_after: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT account_id, following_count, followers_count FROM account_stats WHERE account_id = ANY($1) ORDER BY account_id",
    ).bind(vec![ALICE, target]).fetch_all(&pool).await?;
    assert_eq!(counts_after, counts_before);
    let undone = writer
        .set_follow_with_origin(
            &authenticated,
            target,
            false,
            None,
            None,
            None,
            Some(ORIGIN),
            false,
        )
        .await?;
    if remote {
        assert_eq!(undone.activity_uri, before.1);
        let undo: Value = sqlx::query_scalar("SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events WHERE logical_key LIKE 'activitypub:undo-follow:%'")
            .fetch_one(&pool).await?;
        assert_eq!(undo["object"]["id"], first.activity_uri.unwrap());
    }
    for table in ["follows", "follow_requests"] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!(
                "SELECT count(*) FROM {table} WHERE account_id = $1 AND target_account_id = $2"
            ))
            .bind(ALICE)
            .bind(target)
            .fetch_one(&pool)
            .await?,
            0
        );
    }
    sqlx::query("UPDATE accounts SET locked = $2 WHERE id = $1")
        .bind(target)
        .bind(original_locked)
        .execute(&pool)
        .await?;
    if silenced {
        sqlx::query("UPDATE accounts SET silenced_at = NULL WHERE id = $1")
            .bind(ALICE)
            .execute(&pool)
            .await?;
    }
    sqlx::query("UPDATE accounts SET protocol = $2 WHERE id = $1")
        .bind(target)
        .bind(original_protocol)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE oauth_access_tokens SET scopes = $1 WHERE token = 'fixture-bearer-api-moderator-v4-6-5'")
        .bind(moderator_scopes).execute(&pool).await?;
    reset().await?;
    Ok(())
}

async fn enqueue_accept(
    queue: &Queue,
    executor: &WorkerExecutor,
    body: &Value,
    suffix: &str,
) -> TestResult {
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": body.to_string(), "delivery_target_account_id": ALICE,
                    "signature_key_id": "https://remote.fixture.invalid/users/bob#secondary-key",
                    "remote_domain": "remote.fixture.invalid",
                }),
            )
            .logical_key(format!("lifecycle:accept:{suffix}")),
        )
        .await?;
    assert!(
        executor
            .process_one("accept-worker", &[Lane::Ingress], Duration::seconds(30))
            .await?
    );
    let remaining: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT logical_key, last_error FROM rustodon.durable_jobs WHERE kind = $1")
            .bind(ACTIVITYPUB_INBOX_JOB_KIND)
            .fetch_all(queue.pool())
            .await?;
    assert!(
        remaining.is_empty(),
        "Accept must complete, not retry or dead-letter: {remaining:?}"
    );
    Ok(())
}
