use super::*;

use rustodon::mastodon::AuthenticatedBearer;
use rustodon::web::{WebState, router};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const ALICE: i64 = 116_844_606_259_201_001;
const BOB: i64 = 116_844_606_259_202_001;
const TOKEN: &str = "fixture-bearer-token-v4-6-5";
const AUTHOR: &str = "https://remote.fixture.invalid/users/bob";
const AUTHOR_INBOX: &str = "http://remote.fixture.invalid/inbox";
const FOLLOWER_INBOX: &str = "http://unreblog-follower.fixture.invalid/inbox";
const TARGET_URI: &str = "https://remote.fixture.invalid/users/bob/statuses/private-unreblog";

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn restricted_writer_private_unreblog_http_after_delivered_announce() -> TestResult {
    private_unreblog(true, StatsSetup::Existing).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn restricted_writer_private_unreblog_repository_after_delivered_announce() -> TestResult {
    private_unreblog(false, StatsSetup::Existing).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn restricted_writer_private_unreblog_http_without_account_stats() -> TestResult {
    private_unreblog(true, StatsSetup::MissingAtCreate).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn restricted_writer_private_unreblog_repository_without_account_stats() -> TestResult {
    private_unreblog(false, StatsSetup::MissingAtCreate).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn restricted_writer_private_unreblog_imported_wrapper_without_account_stats() -> TestResult {
    private_unreblog(false, StatsSetup::MissingAtRemove).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
#[allow(clippy::too_many_lines)]
async fn restricted_writer_fresh_boost_counters_serialize_without_resetting_existing_stats()
-> TestResult {
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let restricted =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    reset().await?;
    assert_eq!(
        sqlx::query_as::<_, (String, bool)>(
            "SELECT current_user::text, rolsuper FROM pg_roles WHERE rolname = current_user",
        )
        .fetch_one(&restricted)
        .await?,
        ("rustodon_differential_writer".into(), false)
    );
    // Owner creates a disposable fresh identity, then removes the normally eager
    // Rustodon stats row to model a fresh Rails-seeded local account.
    let fresh = WriteRepository::from_pool(owner.clone())
        .create_local_user(
            "unreblog-counters@fixture.invalid",
            "unreblog_counters",
            "fixture-counter-password",
        )
        .await
        .map_err(|error| safe_write_error(&error))?;
    sqlx::query(
        "INSERT INTO oauth_access_tokens (resource_owner_id, token, scopes, created_at)
                 VALUES ($1, 'fixture-unreblog-counters', 'write:statuses', clock_timestamp())",
    )
    .bind(fresh.user_id)
    .execute(&owner)
    .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(fresh.account_id)
        .execute(&owner)
        .await?;
    // Neither direct nor deleted posts belong in the initial status count/date.
    sqlx::query("INSERT INTO statuses (account_id, text, spoiler_text, visibility, local,
                     sensitive, reply, created_at, updated_at, deleted_at)
                 VALUES ($1, '', '', 3, true, false, false, '2000-01-01', '2000-01-01', NULL),
                        ($1, '', '', 0, true, false, false, '2099-01-01', '2099-01-01', '2099-01-01')")
        .bind(fresh.account_id).execute(&owner).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        "Bearer fixture-unreblog-counters".parse()?,
    );
    let authenticated = BearerAuthenticator::new(Repository::from_pool(runtime.clone()))
        .authenticate(&headers, WRITE_STATUSES)
        .await?;
    let writer = WriteRepository::from_pool(restricted);
    let targets = [116_844_842_188_805_001_i64, 116_845_078_118_405_101_i64];
    let target_counts: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT status_id, reblogs_count FROM status_stats WHERE status_id = ANY($1) ORDER BY status_id",
    ).bind(targets.to_vec()).fetch_all(&runtime).await?;
    // Same-target duplicate and different-target mutations overlap. They must
    // serialize on the account, not just the per-(account,target) advisory lock.
    let (first, duplicate, second) = tokio::join!(
        writer.set_reblog(&authenticated, targets[0], Some("private"), true),
        writer.set_reblog(&authenticated, targets[0], Some("private"), true),
        writer.set_reblog(&authenticated, targets[1], Some("private"), true),
    );
    let first = first.map_err(|error| safe_write_error(&error))?;
    let duplicate = duplicate.map_err(|error| safe_write_error(&error))?;
    let second = second.map_err(|error| safe_write_error(&error))?;
    assert_eq!(first.status_id, duplicate.status_id);
    assert_ne!(first.created, duplicate.created);
    assert!(second.created);
    let stats: (i64, i64, i64, i64, bool) = sqlx::query_as(
        "SELECT id, statuses_count, following_count, followers_count,
                last_status_at > '2000-01-01'::timestamp AND last_status_at <= clock_timestamp()
         FROM account_stats WHERE account_id = $1",
    )
    .bind(fresh.account_id)
    .fetch_one(&runtime)
    .await?;
    assert_eq!((stats.1, stats.2, stats.3, stats.4), (2, 0, 0, true));
    assert_eq!(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM statuses WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3",
    ).bind(fresh.account_id).fetch_one(&runtime).await?, 2);
    // Imported cached counters can intentionally differ from locally stored rows.
    // Once present they are authoritative and must receive deltas, never a reset.
    sqlx::query(
        "UPDATE account_stats SET statuses_count = 41, following_count = 7, followers_count = 11
                 WHERE account_id = $1",
    )
    .bind(fresh.account_id)
    .execute(&owner)
    .await?;
    let (first, duplicate, second) = tokio::join!(
        writer.set_reblog(&authenticated, targets[0], None, false),
        writer.set_reblog(&authenticated, targets[0], None, false),
        writer.set_reblog(&authenticated, targets[1], None, false),
    );
    let first = first.map_err(|error| safe_write_error(&error))?;
    let duplicate = duplicate.map_err(|error| safe_write_error(&error))?;
    let second = second.map_err(|error| safe_write_error(&error))?;
    assert_ne!(first.removed, duplicate.removed);
    assert!(second.removed);
    assert_eq!(sqlx::query_as::<_, (i64, i64, i64, i64)>(
        "SELECT id, statuses_count, following_count, followers_count FROM account_stats WHERE account_id = $1",
    ).bind(fresh.account_id).fetch_one(&runtime).await?, (stats.0, 39, 7, 11));
    assert_eq!(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM statuses WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3",
    ).bind(fresh.account_id).fetch_one(&runtime).await?, 0);
    assert_eq!(sqlx::query_as::<_, (i64, i64)>(
        "SELECT status_id, reblogs_count FROM status_stats WHERE status_id = ANY($1) ORDER BY status_id",
    ).bind(targets.to_vec()).fetch_all(&runtime).await?, target_counts);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1
         AND payload->'arguments'->>'activity_type' = 'Delete'",
        )
        .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
        .fetch_one(&runtime)
        .await?,
        2
    );
    sqlx::query("DELETE FROM statuses WHERE account_id = $1")
        .bind(fresh.account_id)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE parent_account_id = $1")
        .bind(fresh.account_id)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(fresh.account_id)
        .execute(&owner)
        .await?;
    reset().await?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatsSetup {
    Existing,
    MissingAtCreate,
    MissingAtRemove,
}

// Run separately/sequentially on a disposable fixture. On failure deliberately retain
// state for the parent's inspection; never retry the mutation to diagnose an HTTP 500.
#[allow(clippy::too_many_lines)]
async fn private_unreblog(http: bool, stats_setup: StatsSetup) -> TestResult {
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let restricted =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    reset().await?;
    assert_eq!(
        sqlx::query_as::<_, (String, bool)>(
            "SELECT current_user::text, rolsuper FROM pg_roles WHERE rolname = current_user",
        )
        .fetch_one(&restricted)
        .await?,
        ("rustodon_differential_writer".into(), false)
    );
    let writer = WriteRepository::from_pool(restricted.clone());
    let repository = Repository::from_pool(runtime.clone());
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {TOKEN}").parse()?,
    );
    let authenticated: AuthenticatedBearer = BearerAuthenticator::new(repository.clone())
        .authenticate(&headers, WRITE_STATUSES)
        .await?;
    let actor = activitypub::actor_url(
        &Url::parse(ORIGIN)?,
        &repository
            .account(ALICE)
            .await?
            .ok_or("fixture actor missing")?,
    );
    let baseline: (i64, Option<NaiveDateTime>) = sqlx::query_as(
        "SELECT statuses_count, last_status_at FROM account_stats WHERE account_id = $1",
    )
    .bind(ALICE)
    .fetch_one(&owner)
    .await?;
    // Retain every stats column (including ID/timestamps/follow counts), not just
    // statuses_count. This is fixture-only setup, before boost, never before undo.
    let saved_stats: Value =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&owner)
            .await?;
    let live_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM statuses WHERE account_id = $1 AND deleted_at IS NULL",
    )
    .bind(ALICE)
    .fetch_one(&runtime)
    .await?;
    if stats_setup == StatsSetup::MissingAtCreate {
        assert_eq!(
            sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
                .bind(ALICE)
                .execute(&owner)
                .await?
                .rows_affected(),
            1
        );
        assert!(
            !sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM account_stats WHERE account_id = $1)",
            )
            .bind(ALICE)
            .fetch_one(&runtime)
            .await?
        );
    }
    let inboxes: (String, String, i32) =
        sqlx::query_as("SELECT inbox_url, shared_inbox_url, protocol FROM accounts WHERE id = $1")
            .bind(BOB)
            .fetch_one(&owner)
            .await?;
    // Owner only arranges/cleans fixture rows; all application/worker writes use restricted.
    // The restored Bob defaults to protocol 0; this scenario requires ActivityPub.
    sqlx::query(
        "UPDATE accounts SET inbox_url = $2, shared_inbox_url = $2, protocol = 1 WHERE id = $1",
    )
    .bind(BOB)
    .bind(AUTHOR_INBOX)
    .execute(&owner)
    .await?;
    let follower: i64 = sqlx::query_scalar(
        "INSERT INTO accounts (username, domain, uri, inbox_url, shared_inbox_url, protocol, created_at, updated_at)
         VALUES ('unreblog_follower', 'unreblog-follower.fixture.invalid',
             'http://unreblog-follower.fixture.invalid/users/follower', $1, $1, 1,
             clock_timestamp(), clock_timestamp()) RETURNING id",
    ).bind(FOLLOWER_INBOX).fetch_one(&owner).await?;
    sqlx::query(
        "INSERT INTO follows (account_id, target_account_id, show_reblogs, notify, created_at, updated_at)
         VALUES ($1, $2, true, false, clock_timestamp(), clock_timestamp())",
    ).bind(follower).bind(ALICE).execute(&owner).await?;
    let target: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (account_id, text, spoiler_text, visibility, local, uri, sensitive, reply, created_at, updated_at)
         VALUES ($1, 'Private boost target', '', 0, false, $2, false, false,
             clock_timestamp(), clock_timestamp()) RETURNING id",
    ).bind(BOB).bind(TARGET_URI).fetch_one(&owner).await?;
    sqlx::query("INSERT INTO status_stats (status_id, created_at, updated_at) VALUES ($1, clock_timestamp(), clock_timestamp())")
        .bind(target).execute(&owner).await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let queue = Queue::new(runtime.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(restricted),
        None,
        Some(ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
            local_domain: DOMAIN.into(),
            media_root_url: "/system".into(),
            media_root: None,
            limited_federation: false,
            remote_media_endpoint: None,
            remote_delivery_endpoint: Some(endpoint),
            remote_fetch_endpoint: None,
        }),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let counted_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM statuses WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3",
    ).bind(ALICE).fetch_one(&runtime).await?;
    assert!(
        counted_before < live_before,
        "fixture must include an excluded direct status"
    );
    let follow_counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM follows WHERE account_id = $1),
                (SELECT count(*) FROM follows WHERE target_account_id = $1)",
    )
    .bind(ALICE)
    .fetch_one(&runtime)
    .await?;
    let boost = writer
        .set_reblog_with_origin(
            &authenticated,
            target,
            Some("private"),
            true,
            Some(ORIGIN),
            false,
        )
        .await
        .map_err(|error| safe_write_error(&error))?;
    assert!(boost.created && !boost.removed);
    assert_eq!(boost.target_status_id, target);
    assert_ne!(boost.status_id, target);
    let wrapper = boost.status_id;
    let conversation: i64 =
        sqlx::query_scalar("SELECT conversation_id FROM statuses WHERE id = $1")
            .bind(wrapper)
            .fetch_one(&runtime)
            .await?;
    let announce_id = format!("{actor}/statuses/{wrapper}/activity");
    let before_delivery = snapshot(&runtime, target, wrapper).await?;
    assert!(!before_delivery.deleted);
    // Missing stats must be initialized before the increment, not seeded with
    // zero (losing imported posts) or counted after insertion and incremented twice.
    let expected_count_before = if stats_setup == StatsSetup::MissingAtCreate {
        counted_before + 1
    } else {
        baseline.0 + 1
    };
    assert_eq!(before_delivery.statuses_count, Some(expected_count_before));
    assert_eq!(before_delivery.live_statuses_count, live_before + 1);
    assert_eq!(before_delivery.reblogs_count, 1);
    let announces = deliver_pair(
        &queue,
        &executor,
        &runtime,
        listener,
        &announce_id,
        &actor,
        false,
    )
    .await?;
    assert!(
        announces[0] == announces[1],
        "author and follower Announce differ"
    );
    let delivered = snapshot(&runtime, target, wrapper).await?;
    assert_eq!(
        delivered, before_delivery,
        "delivery must not alter removal preconditions"
    );
    if stats_setup == StatsSetup::MissingAtRemove {
        // Model a previously delivered/imported wrapper with no stats row. This
        // independently exercises removal's initializer even once creation is fixed.
        assert_eq!(
            sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
                .bind(ALICE)
                .execute(&owner)
                .await?
                .rows_affected(),
            1
        );
    }
    let before = snapshot(&runtime, target, wrapper).await?;
    assert_eq!(
        before.statuses_count,
        if stats_setup == StatsSetup::MissingAtRemove {
            None
        } else {
            Some(expected_count_before)
        }
    );
    assert_eq!(before.delete_intents, 0);
    assert_eq!(before.undo_intents, 0);

    let removal = if http {
        unreblog_http(repository, writer, &runtime, target).await
    } else {
        match writer
            .set_reblog_with_origin(&authenticated, target, None, false, Some(ORIGIN), false)
            .await
        {
            Ok(outcome) => {
                assert!(outcome.removed && !outcome.created);
                assert_eq!(outcome.status_id, wrapper);
                assert_eq!(outcome.target_status_id, target);
                assert_eq!(
                    outcome.account_statuses_count_before_removal,
                    Some(if stats_setup == StatsSetup::MissingAtRemove {
                        counted_before + 1
                    } else {
                        expected_count_before
                    })
                );
                Ok(())
            }
            Err(error) => Err(safe_write_error(&error).into()),
        }
    };
    // This is before worker dispatch or cleanup: distinguish rollback from committed
    // removal followed by reload/serialization failure. Never dump outbox bodies.
    let after = snapshot(&runtime, target, wrapper).await;
    if removal.is_err() {
        eprintln!(
            "private-unreblog http={http} stats_setup={stats_setup:?} target={target} wrapper={wrapper} before={before:?}"
        );
        match &after {
            Ok(state) => eprintln!(
                "private-unreblog after={state:?} transaction_unchanged={}",
                state == &before
            ),
            Err(_) => eprintln!("private-unreblog after_snapshot_unavailable=true"),
        }
    }
    removal?;
    let after = after?;
    assert!(after.deleted);
    assert_eq!(
        after.statuses_count,
        Some(if stats_setup == StatsSetup::Existing {
            baseline.0
        } else {
            counted_before
        })
    );
    assert_eq!(after.live_statuses_count, live_before);
    let stats: Value =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&runtime)
            .await?;
    if stats_setup == StatsSetup::Existing {
        for field in ["id", "created_at", "following_count", "followers_count"] {
            assert!(
                stats[field] == saved_stats[field],
                "existing stats metadata was reset"
            );
        }
    } else {
        assert!(
            stats["following_count"] == follow_counts.0,
            "following count lost"
        );
        assert!(
            stats["followers_count"] == follow_counts.1,
            "followers count lost"
        );
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT last_status_at IS NOT NULL AND last_status_at <= clock_timestamp()
             FROM account_stats WHERE account_id = $1",
            )
            .bind(ALICE)
            .fetch_one(&runtime)
            .await?
        );
    }
    assert_eq!(after.reblogs_count, 0);
    assert_eq!(after.delete_intents, 1);
    assert_eq!(
        after.undo_intents, 1,
        "author Undo commits with deletion/counters"
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT deleted_at IS NULL AND visibility = 0 AND uri = $2 FROM statuses WHERE id = $1",
        )
        .bind(target)
        .bind(TARGET_URI)
        .fetch_one(&runtime)
        .await?
    );

    let listener = TcpListener::bind(endpoint).await?;
    let undos = deliver_pair(
        &queue,
        &executor,
        &runtime,
        listener,
        &announce_id,
        &actor,
        true,
    )
    .await?;
    for undo in undos {
        assert!(
            undo["id"] == format!("{actor}#announces/{wrapper}/undo"),
            "Undo identity changed"
        );
        assert!(
            undo["object"] == announces[0],
            "Undo must embed the exact delivered Announce"
        );
    }
    // Success-only cleanup: failure evidence must survive until fixture disposal.
    sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
        .bind(vec![wrapper, target])
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(conversation)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(follower)
        .execute(&owner)
        .await?;
    sqlx::query(
        "UPDATE accounts SET inbox_url = $2, shared_inbox_url = $3, protocol = $4 WHERE id = $1",
    )
    .bind(BOB)
    .bind(inboxes.0)
    .bind(inboxes.1)
    .bind(inboxes.2)
    .execute(&owner)
    .await?;
    sqlx::query(
        "UPDATE account_stats SET statuses_count = $2, last_status_at = $3 WHERE account_id = $1",
    )
    .bind(ALICE)
    .bind(baseline.0)
    .bind(baseline.1)
    .execute(&owner)
    .await?;
    if stats_setup != StatsSetup::Existing {
        sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
            .bind(ALICE)
            .execute(&owner)
            .await?;
        sqlx::query("INSERT INTO account_stats SELECT * FROM jsonb_populate_record(NULL::account_stats, $1)")
            .bind(&saved_stats).execute(&owner).await?;
        let restored: Value = sqlx::query_scalar(
            "SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1",
        )
        .bind(ALICE)
        .fetch_one(&owner)
        .await?;
        assert!(
            restored == saved_stats,
            "fixture account_stats restoration changed columns"
        );
    }
    reset().await?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct TransactionState {
    deleted: bool,
    statuses_count: Option<i64>,
    live_statuses_count: i64,
    reblogs_count: i64,
    delete_intents: i64,
    undo_intents: i64,
}

async fn snapshot(
    pool: &sqlx::PgPool,
    target: i64,
    wrapper: i64,
) -> Result<TransactionState, sqlx::Error> {
    sqlx::query_as(
        "SELECT s.deleted_at IS NOT NULL AS deleted, a.statuses_count, t.reblogs_count,
            (SELECT count(*) FROM statuses WHERE account_id = $3 AND deleted_at IS NULL) AS live_statuses_count,
            (SELECT count(*) FROM rustodon.outbox_events WHERE kind = $4
                AND payload->'arguments'->>'status_id' = $2::bigint::text
                AND payload->'arguments'->>'activity_type' = 'Delete') AS delete_intents,
            (SELECT count(*) FROM rustodon.outbox_events WHERE kind = $5
                AND payload->'arguments'->'body'->>'type' = 'Undo'
                AND payload->'arguments'->'body'->'object'->>'object' = $6) AS undo_intents
         FROM statuses s LEFT JOIN account_stats a ON a.account_id = $3
         JOIN status_stats t ON t.status_id = $1 WHERE s.id = $2",
    ).bind(target).bind(wrapper).bind(ALICE)
        .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND).bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(TARGET_URI).fetch_one(pool).await
}

// Inspect the original repository error, not its deliberately generic Display mapper.
// PostgreSQL message/detail/context and Debug may contain row values or SQL parameters;
// retain only the typed source and SQLSTATE. Return this safe error to the test runner too.
fn safe_write_error(error: &WriteError) -> std::io::Error {
    let sql = match error {
        WriteError::Sqlx(sql) | WriteError::Job(JobError::Sqlx(sql)) => Some(sql),
        _ => None,
    };
    std::io::Error::other(format!(
        "private-unreblog repository_error={:?} sqlx_variant={:?} row_not_found={} SQLSTATE={}",
        std::mem::discriminant(error),
        sql.map(std::mem::discriminant),
        matches!(sql, Some(sqlx::Error::RowNotFound)),
        sql.and_then(sqlx::Error::as_database_error)
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref()
            .unwrap_or("unavailable"),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn deliver_pair(
    queue: &Queue,
    executor: &WorkerExecutor,
    pool: &sqlx::PgPool,
    listener: TcpListener,
    announce_id: &str,
    actor: &str,
    undo: bool,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    // Two destinations: the original author (also a fixture follower), plus an
    // independent follower. All other fixture accounts must receive no delivery.
    let server = tokio::spawn(async move {
        let mut received = Vec::new();
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await?;
            let request = fixture_delivery_request(&mut socket).await?;
            let start = request
                .windows(4)
                .position(|v| v == b"\r\n\r\n")
                .ok_or_else(|| std::io::Error::other("missing delivery header separator"))?
                + 4;
            let body: Value = serde_json::from_slice(&request[start..])
                .map_err(|_| std::io::Error::other("invalid delivery JSON"))?;
            // No credentials/headers/bodies escape via failure output.
            received.push(body);
            socket
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
        }
        Ok::<_, std::io::Error>(received)
    });
    let operation = async {
        // Distribution precedes the writer's author delivery; it deduplicates the
        // author inbox and creates the second follower intent.
        queue.dispatch_outbox(100).await?;
        assert!(
            executor
                .process_one(
                    "private-unreblog-distribute",
                    &[Lane::Push],
                    Duration::seconds(30)
                )
                .await?
        );
        queue.dispatch_outbox(100).await?;
        for _ in 0..2 {
            assert!(
                executor
                    .process_one(
                        "private-unreblog-deliver",
                        &[Lane::Push],
                        Duration::seconds(30)
                    )
                    .await?
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs WHERE lane = 'push'",
            )
            .fetch_one(pool)
            .await?,
            0,
            "delivery must complete, not merely be attempted"
        );
        let destinations: Vec<String> = sqlx::query_scalar(
            "SELECT payload->'arguments'->>'inbox_url' FROM rustodon.outbox_events
             WHERE kind = $1 AND payload->'arguments'->'body'->>'type' = $2 ORDER BY 1",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(if undo { "Undo" } else { "Announce" })
        .fetch_all(pool)
        .await?;
        assert_eq!(
            destinations,
            vec![AUTHOR_INBOX.to_owned(), FOLLOWER_INBOX.to_owned()]
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    if let Err(error) = operation {
        server.abort();
        return Err(error);
    }
    let received = tokio::time::timeout(std::time::Duration::from_secs(5), server).await???;
    assert_eq!(received.len(), 2);
    for body in &received {
        let announce = if undo {
            assert!(
                body["type"] == "Undo" && body["actor"] == actor,
                "wrong Undo envelope"
            );
            &body["object"]
        } else {
            body
        };
        assert!(announce["type"] == "Announce", "wrong activity type");
        assert!(announce["id"] == announce_id, "Announce identity changed");
        assert!(announce["actor"] == actor, "Announce actor changed");
        assert!(announce["object"] == TARGET_URI, "Announce target changed");
        assert!(
            announce["to"] == json!([format!("{actor}/followers")]),
            "not followers-only"
        );
        assert!(announce["cc"] == json!([AUTHOR]), "unexpected Announce cc");
    }
    Ok(received)
}

async fn unreblog_http(
    repository: Repository,
    writer: WriteRepository,
    pool: &sqlx::PgPool,
    original: i64,
) -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let host = listener.local_addr()?.to_string();
    let media_root = std::env::temp_dir().join(format!("rustodon-unreblog-{}", std::process::id()));
    fs::create_dir_all(&media_root)?;
    let state = WebState::new(
        repository,
        Url::parse(ORIGIN)?,
        DOMAIN,
        "/system",
        media_root.clone(),
        InstanceRuntimeConfig {
            domain: DOMAIN.into(),
            version: "4.6.5".into(),
            source_url: "https://github.com/mastodon/mastodon".into(),
            streaming_api: format!("ws://{host}"),
            vapid_public_key: None,
            thumbnail_url: String::new(),
            thumbnail_description: String::new(),
            thumbnail_blurhash: None,
            thumbnail_versions: None,
            icons: Vec::new(),
            languages: vec!["en".into()],
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
        vec![host.clone()],
    )?
    .with_write_repository(writer)
    .with_queue(Queue::new(pool.clone()));
    let server = tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let operation = async {
        let response = reqwest::Client::new()
            .post(format!("http://{host}/api/v1/statuses/{original}/unreblog"))
            .bearer_auth(TOKEN)
            .send()
            .await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(format!(
                "original-ID private unreblog HTTP status={}",
                response.status()
            )
            .into());
        }
        let body: Value = serde_json::from_str(&response.text().await?)?;
        let expected_id = original.to_string();
        assert!(
            body["id"].as_str() == Some(expected_id.as_str()),
            "response must identify the original status"
        );
        assert!(
            body["reblogged"] == false,
            "original must no longer be reblogged"
        );
        assert!(
            body["visibility"] == "public",
            "original visibility changed"
        );
        assert!(body["reblog"].is_null(), "response must not be the wrapper");
        assert!(
            body["reblogs_count"] == 0,
            "response counter was not decremented"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    server.abort();
    fs::remove_dir_all(media_root)?;
    operation
}
