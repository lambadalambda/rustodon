use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, NaiveDateTime, Utc};
use http::HeaderMap;
use http::header::{AUTHORIZATION, HeaderValue};
#[cfg(feature = "test-support")]
use rustodon::jobs::StreamEventStagingProbe;
use rustodon::jobs::{
    JobSpec, Lane, Queue, record_global_stream_transition_in, record_stream_event_in,
};
use rustodon::mastodon::{
    BearerAuthenticator, IdempotencyKey, Repository, WRITE_STATUSES, WriteError, WriteOptions,
    WriteOutcome, WriteRepository,
};
use rustodon::operational_schema::{
    CURRENT_VERSION, MigrationError, MigrationRecord, migrate, migration_plan,
};
use rustodon::streaming::{StreamName, Subscription, TimelineRouteSnapshot};
use sqlx::{Connection, PgConnection, PgPool};
use tokio::sync::Barrier;
use tokio::time::{Duration, timeout};

const TABLES: &[&str] = &[
    "activity_buckets",
    "activity_members",
    "domain_health",
    "durable_jobs",
    "heartbeats",
    "idempotency_keys",
    "local_uploads",
    "ordering_markers",
    "outbox_events",
    "rate_limit_windows",
    "remote_fetch_leases",
];

#[test]
fn migration_plan_requires_an_exact_known_prefix() {
    assert_eq!(migration_plan(&[]).unwrap(), vec![1, 2, 3, 4, 5, 6]);
    let current = vec![MigrationRecord::known(1).expect("migration 1 exists")];
    assert_eq!(migration_plan(&current).unwrap(), vec![2, 3, 4, 5, 6]);

    let unknown = vec![MigrationRecord {
        version: CURRENT_VERSION + 1,
        checksum: [0; 32],
    }];
    assert!(matches!(
        migration_plan(&unknown),
        Err(MigrationError::UnknownVersion(7))
    ));

    let wrong_checksum = vec![MigrationRecord {
        version: 1,
        checksum: [0; 32],
    }];
    assert!(matches!(
        migration_plan(&wrong_checksum),
        Err(MigrationError::ChecksumMismatch(1))
    ));
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn operational_schema_lifecycle_is_isolated_and_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let mut connection = PgConnection::connect(&url).await?;

    migrate(&mut connection).await?;
    assert_schema(&mut connection).await?;
    assert_active_record_migration_lock(&url).await?;

    sqlx::raw_sql("DROP SCHEMA rustodon CASCADE; CREATE SCHEMA rustodon")
        .execute(&mut connection)
        .await?;
    let attempts = (0..8)
        .map(|_| {
            let url = url.clone();
            async move {
                let mut connection = PgConnection::connect(&url).await?;
                migrate(&mut connection).await
            }
        })
        .collect::<Vec<_>>();
    for attempt in futures_util::future::join_all(attempts).await {
        attempt?;
    }
    assert_schema(&mut connection).await?;
    let first_applied_at = applied_at(&mut connection).await?;
    migrate(&mut connection).await?;
    assert_eq!(applied_at(&mut connection).await?, first_applied_at);

    sqlx::query("DROP SCHEMA rustodon CASCADE")
        .execute(&mut connection)
        .await?;
    let attempts = (0..8)
        .map(|_| {
            let url = url.clone();
            async move {
                let mut connection = PgConnection::connect(&url).await?;
                migrate(&mut connection).await
            }
        })
        .collect::<Vec<_>>();
    for attempt in futures_util::future::join_all(attempts).await {
        attempt?;
    }
    assert_schema(&mut connection).await?;

    sqlx::query("UPDATE rustodon.schema_migrations SET version = $1 WHERE version = $2")
        .bind(CURRENT_VERSION + 1)
        .bind(CURRENT_VERSION)
        .execute(&mut connection)
        .await?;
    let error = migrate(&mut connection)
        .await
        .expect_err("future schema versions must be rejected");
    assert!(matches!(
        error,
        MigrationError::UnknownVersion(version) if version == CURRENT_VERSION + 1
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT version FROM rustodon.schema_migrations ORDER BY version DESC LIMIT 1",
        )
        .fetch_one(&mut connection)
        .await?,
        CURRENT_VERSION + 1
    );

    sqlx::query("UPDATE rustodon.schema_migrations SET checksum = $1 WHERE version = 1")
        .bind([0_u8; 32].as_slice())
        .execute(&mut connection)
        .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::ChecksumMismatch(1))
    ));

    let checksum = MigrationRecord::known(1).unwrap().checksum;
    sqlx::query("UPDATE rustodon.schema_migrations SET checksum = $1 WHERE version = 1")
        .bind(checksum.as_slice())
        .execute(&mut connection)
        .await?;
    sqlx::query("UPDATE rustodon.schema_migrations SET version = $1 WHERE version = $2")
        .bind(CURRENT_VERSION)
        .bind(CURRENT_VERSION + 1)
        .execute(&mut connection)
        .await?;
    sqlx::query("ALTER TABLE rustodon.heartbeats ADD COLUMN drift text")
        .execute(&mut connection)
        .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::SchemaDrift(_))
    ));
    sqlx::query("ALTER TABLE rustodon.heartbeats DROP COLUMN drift")
        .execute(&mut connection)
        .await?;
    migrate(&mut connection).await?;

    sqlx::raw_sql(
        "DROP SCHEMA rustodon CASCADE; CREATE SCHEMA rustodon; \
         CREATE TABLE rustodon.unknown (id bigint)",
    )
    .execute(&mut connection)
    .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::UnversionedSchema)
    ));
    sqlx::raw_sql(
        "DROP SCHEMA rustodon CASCADE; CREATE SCHEMA rustodon; \
         CREATE TYPE rustodon.unknown AS ENUM ('x')",
    )
    .execute(&mut connection)
    .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::UnversionedSchema)
    ));
    sqlx::query("DROP SCHEMA rustodon CASCADE")
        .execute(&mut connection)
        .await?;
    migrate(&mut connection).await?;
    assert_schema(&mut connection).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn operational_schema_upgrade_grants_new_runtime_table_access()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let runtime_role = std::env::var("RUSTODON_OPERATIONAL_RUNTIME_ROLE")?;
    let mut connection = PgConnection::connect(&url).await?;

    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT to_regclass('rustodon.remote_fetch_leases') IS NOT NULL",
        )
        .fetch_one(&mut connection)
        .await?
    );

    migrate(&mut connection).await?;

    for privilege in ["SELECT", "INSERT", "DELETE"] {
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT has_table_privilege($1, 'rustodon.remote_fetch_leases', $2)",
            )
            .bind(&runtime_role)
            .bind(privilege)
            .fetch_one(&mut connection)
            .await?,
            "runtime role should have {privilege} on the new lease table",
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn operational_write_composition_is_atomic_and_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let read_url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let write_url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let reader = Repository::connect(&read_url).await?;
    let authenticator = BearerAuthenticator::new(reader);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let writer = WriteRepository::connect(&write_url).await?;
    let pool = sqlx::PgPool::connect(&write_url).await?;
    let (last_read_id, lock_version, updated_at): (i64, i32, NaiveDateTime) = sqlx::query_as(
        "SELECT last_read_id, lock_version, updated_at FROM markers \
         WHERE user_id = $1 AND timeline = $2",
    )
    .bind(101_i64)
    .bind("home")
    .fetch_one(&pool)
    .await?;
    let idempotency = IdempotencyKey {
        scope: "operational-write-test",
        key: "marker-1",
        fingerprint: [7; 32],
        expires_at: Utc::now() + ChronoDuration::hours(1),
    };
    let job = JobSpec::new(
        Lane::Maintenance,
        "operational_write_probe",
        serde_json::json!({ "timeline": "home" }),
    )
    .logical_key("operational-write-probe-1");
    let options = || WriteOptions {
        idempotency: Some(idempotency),
        outbox: Some(&job),
    };

    let first = writer
        .update_marker_with_options(
            &authenticated,
            "home",
            last_read_id + 1,
            Some(lock_version),
            options(),
        )
        .await?;
    let replay = writer
        .update_marker_with_options(
            &authenticated,
            "home",
            last_read_id + 2,
            Some(lock_version),
            options(),
        )
        .await?;
    let fingerprint_conflict = writer
        .update_marker_with_options(
            &authenticated,
            "home",
            last_read_id + 3,
            Some(lock_version + 1),
            WriteOptions {
                idempotency: Some(IdempotencyKey {
                    fingerprint: [8; 32],
                    ..idempotency
                }),
                outbox: None,
            },
        )
        .await;
    let stored_marker: (i64, i32) = sqlx::query_as(
        "SELECT last_read_id, lock_version FROM markers WHERE user_id = $1 AND timeline = $2",
    )
    .bind(101_i64)
    .bind("home")
    .fetch_one(&pool)
    .await?;
    let stored_idempotency: (Vec<u8>, serde_json::Value) = sqlx::query_as(
        "SELECT fingerprint, result FROM rustodon.idempotency_keys \
         WHERE scope = $1 AND key = $2",
    )
    .bind(idempotency.scope)
    .bind(idempotency.key)
    .fetch_one(&pool)
    .await?;
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events \
         WHERE kind = $1 AND logical_key = $2",
    )
    .bind("operational_write_probe")
    .bind("operational-write-probe-1")
    .fetch_one(&pool)
    .await?;

    sqlx::query(
        "UPDATE markers SET last_read_id = $1, lock_version = $2, updated_at = $3 \
         WHERE user_id = $4 AND timeline = $5",
    )
    .bind(last_read_id)
    .bind(lock_version)
    .bind(updated_at)
    .bind(101_i64)
    .bind("home")
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
        .bind("operational_write_probe")
        .bind("operational-write-probe-1")
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM rustodon.idempotency_keys WHERE scope = $1 AND key = $2")
        .bind(idempotency.scope)
        .bind(idempotency.key)
        .execute(&pool)
        .await?;

    let first = match first {
        WriteOutcome::Applied(marker) => marker,
        WriteOutcome::Replayed(_) => panic!("first idempotent write must apply"),
    };
    let replay = match replay {
        WriteOutcome::Replayed(marker) => marker,
        WriteOutcome::Applied(_) => panic!("duplicate idempotent write must replay"),
    };
    assert_eq!(first.last_read_id, last_read_id + 1);
    assert_eq!(replay.last_read_id, first.last_read_id);
    assert_eq!(stored_marker, (last_read_id + 1, lock_version + 1));
    assert_eq!(stored_idempotency.0, vec![7; 32]);
    assert_eq!(stored_idempotency.1["last_read_id"], last_read_id + 1);
    assert_eq!(outbox_count, 1);
    assert!(matches!(fingerprint_conflict, Err(WriteError::Conflict)));
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn stream_events_are_retained_and_not_dispatchable() -> Result<(), Box<dyn std::error::Error>>
{
    let url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let pool = sqlx::PgPool::connect(&url).await?;
    let queue = Queue::new(pool.clone());
    let logical_key = "stream:test:101:update:42:1";
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
        .bind(rustodon::streaming::STREAM_EVENT_KIND)
        .bind(logical_key)
        .execute(&pool)
        .await?;

    let cursor = queue.stream_cursor().await?;
    let mut transaction = pool.begin().await?;
    let first = record_stream_event_in(&mut transaction, 101, "update", 42, logical_key).await?;
    let duplicate =
        record_stream_event_in(&mut transaction, 101, "update", 42, logical_key).await?;
    transaction.commit().await?;

    assert_eq!(first, duplicate);
    let first_read = queue.stream_events_after(cursor, 10).await?;
    let second_read = queue.stream_events_after(cursor, 10).await?;
    assert_eq!(first_read, second_read);
    assert_eq!(
        first_read.iter().filter(|event| event.id == first).count(),
        1
    );

    queue.dispatch_outbox(100).await?;
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT dispatched_at IS NOT NULL FROM rustodon.outbox_events WHERE id = $1",
        )
        .bind(first)
        .fetch_one(&pool)
        .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 AND logical_key = $2",
        )
        .bind(rustodon::streaming::STREAM_EVENT_KIND)
        .bind(logical_key)
        .fetch_one(&pool)
        .await?,
        0
    );

    sqlx::query("DELETE FROM rustodon.outbox_events WHERE id = $1")
        .bind(first)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn stream_history_is_durable_and_bounded_without_pruning_other_outbox_kinds()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let pool = sqlx::PgPool::connect(&url).await?;
    let queue = Queue::new(pool.clone());
    let prefix = format!("stream:retention:{}", std::process::id());
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE logical_key LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await?;

    let mut ids = Vec::new();
    for index in 0..4_i64 {
        let mut transaction = pool.begin().await?;
        ids.push(
            record_stream_event_in(
                &mut transaction,
                101,
                "update",
                900_100_000 + index,
                &format!("{prefix}:stream:{index}"),
            )
            .await?,
        );
        transaction.commit().await?;
    }
    sqlx::query(
        "UPDATE rustodon.outbox_events SET created_at = clock_timestamp() - INTERVAL '2 hours' \
         WHERE id = $1",
    )
    .bind(ids[0])
    .execute(&pool)
    .await?;
    let ordinary_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload) \
         VALUES ('retention-test', $1, '{}'::jsonb) RETURNING id",
    )
    .bind(format!("{prefix}:ordinary"))
    .fetch_one(&pool)
    .await?;

    let retained_cursor = queue
        .prune_stream_history_with_limits(ChronoDuration::hours(1), 2)
        .await?;
    let retained = queue.stream_events_after(retained_cursor, 10_000).await?;
    assert_eq!(
        retained
            .iter()
            .filter(|event| ids.contains(&event.id))
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        ids[2..]
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM rustodon.outbox_events WHERE id = $1)",
        )
        .bind(ordinary_id)
        .fetch_one(&pool)
        .await?
    );

    sqlx::query("DELETE FROM rustodon.outbox_events WHERE logical_key LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn timeline_replay_is_bounded_by_event_class_and_commit_cursor()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let pool = PgPool::connect(&url).await?;
    let queue = Queue::new(pool.clone());
    let prefix = format!("stream:replay:{}", std::process::id());
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE logical_key LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await?;
    let snapshot = TimelineRouteSnapshot {
        public: true,
        hashtag: true,
        local: true,
        had_media: false,
        language: Some("en".to_owned()),
        tags: vec!["replay".to_owned()],
        lists: Vec::new(),
    };
    let mut inserted = Vec::new();
    for index in 0..3_i64 {
        let mut transaction = pool.begin().await?;
        inserted.push(
            record_global_stream_transition_in(
                &mut transaction,
                "update",
                901_000_000 + index,
                &format!("{prefix}:update:{index}"),
                None,
                Some(&snapshot),
            )
            .await?,
        );
        transaction.commit().await?;
    }
    for index in 0..3_i64 {
        let event = if index == 2 {
            "delete"
        } else {
            "status.update"
        };
        let mut transaction = pool.begin().await?;
        inserted.push(
            record_global_stream_transition_in(
                &mut transaction,
                event,
                902_000_000 + index,
                &format!("{prefix}:transition:{index}"),
                Some(&snapshot),
                (event != "delete").then_some(&snapshot),
            )
            .await?,
        );
        transaction.commit().await?;
    }
    let through = queue.stream_cursor().await?;
    let mut transaction = pool.begin().await?;
    let excluded_account_event = record_stream_event_in(
        &mut transaction,
        101,
        "delete",
        903_000_000,
        &format!("{prefix}:account"),
    )
    .await?;
    transaction.commit().await?;

    let replay = queue
        .stream_replay_events_with_limits(through, 2, 1)
        .await?;
    assert_eq!(
        replay.iter().map(|event| event.id).collect::<Vec<_>>(),
        vec![inserted[2], inserted[4], inserted[5]],
    );
    assert!(replay.iter().all(|event| event.account_id == 0));
    assert!(replay.iter().all(|event| event.id <= through));
    assert!(
        !replay
            .iter()
            .any(|event| event.id == excluded_account_event)
    );

    let unrelated_snapshot = TimelineRouteSnapshot {
        tags: vec!["unrelated".to_owned()],
        ..snapshot.clone()
    };
    let mut transaction = pool.begin().await?;
    for index in 0..41_i64 {
        record_global_stream_transition_in(
            &mut transaction,
            "update",
            903_100_000 + index,
            &format!("{prefix}:unrelated-update:{index}"),
            None,
            Some(&unrelated_snapshot),
        )
        .await?;
    }
    for index in 0..129_i64 {
        record_global_stream_transition_in(
            &mut transaction,
            "status.update",
            903_200_000 + index,
            &format!("{prefix}:unrelated-transition:{index}"),
            Some(&unrelated_snapshot),
            Some(&unrelated_snapshot),
        )
        .await?;
    }
    let mut relevant_updates = Vec::new();
    for index in 0..41_i64 {
        relevant_updates.push(
            record_global_stream_transition_in(
                &mut transaction,
                "update",
                903_300_000 + index,
                &format!("{prefix}:relevant-update:{index}"),
                None,
                Some(&snapshot),
            )
            .await?,
        );
    }
    let mut relevant_transitions = Vec::new();
    for index in 0..129_i64 {
        relevant_transitions.push(
            record_global_stream_transition_in(
                &mut transaction,
                "status.update",
                903_400_000 + index,
                &format!("{prefix}:relevant-transition:{index}"),
                Some(&snapshot),
                Some(&snapshot),
            )
            .await?,
        );
    }
    transaction.commit().await?;
    let route_replay = queue
        .stream_replay_events_for_subscription(
            queue.stream_cursor().await?,
            0,
            &Subscription::new(StreamName::Hashtag, Some("RePlay".to_owned())),
            101,
        )
        .await?;
    assert!(
        route_replay.iter().any(|event| event.id == inserted[5]),
        "a route-relevant delete was evicted by later edits and creates"
    );
    let expected_updates = inserted[..3]
        .iter()
        .chain(&relevant_updates)
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(
        route_replay
            .iter()
            .filter(|event| event.event == "update")
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        expected_updates,
        "every create after the subscribe boundary must survive the handoff"
    );
    let expected_transitions = inserted[3..5]
        .iter()
        .chain(&relevant_transitions)
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(
        route_replay
            .iter()
            .filter(|event| event.event == "status.update")
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        expected_transitions,
        "every edit after the subscribe boundary must survive the handoff"
    );
    let hidden_snapshot = TimelineRouteSnapshot {
        tags: vec!["elsewhere".to_owned()],
        ..snapshot.clone()
    };
    let mut transaction = pool.begin().await?;
    let historical_route_entry = record_global_stream_transition_in(
        &mut transaction,
        "status.update",
        903_500_000,
        &format!("{prefix}:historical-route-entry"),
        Some(&hidden_snapshot),
        Some(&snapshot),
    )
    .await?;
    transaction.commit().await?;
    let freshness_boundary = queue.stream_cursor().await?;
    let fresh_replay = queue
        .stream_replay_events_for_subscription(
            freshness_boundary,
            freshness_boundary,
            &Subscription::new(StreamName::Hashtag, Some("RePlay".to_owned())),
            101,
        )
        .await?;
    assert!(
        !fresh_replay
            .iter()
            .any(|event| event.id == historical_route_entry),
        "a historical route entry that becomes a wire create must not be replayed"
    );
    assert!(
        fresh_replay
            .iter()
            .any(|event| event.id == relevant_transitions[128]),
        "a historical idempotent edit must remain replayable"
    );
    assert!(
        fresh_replay.iter().any(|event| event.id == inserted[5]),
        "a historical actual delete must remain replayable"
    );

    let mut transaction = pool.begin().await?;
    let racing_route_entry = record_global_stream_transition_in(
        &mut transaction,
        "status.update",
        903_500_001,
        &format!("{prefix}:racing-route-entry"),
        Some(&hidden_snapshot),
        Some(&snapshot),
    )
    .await?;
    transaction.commit().await?;
    let replay_through = queue.stream_cursor().await?;
    let subscribe_boundary_replay = queue
        .stream_replay_events_for_subscription(
            replay_through,
            freshness_boundary,
            &Subscription::new(StreamName::Hashtag, Some("RePlay".to_owned())),
            101,
        )
        .await?;
    assert!(
        subscribe_boundary_replay
            .iter()
            .any(|event| event.id == racing_route_entry),
        "a route entry racing the actual subscribe boundary must not be lost"
    );

    let pruned_window_replay = queue
        .stream_replay_events_for_subscription(
            0,
            replay_through,
            &Subscription::new(StreamName::Hashtag, Some("RePlay".to_owned())),
            101,
        )
        .await?;
    assert!(
        pruned_window_replay.is_empty(),
        "pruning every retained row between connect and subscribe must leave an empty replay window"
    );
    let subscribe_race_replay = queue
        .stream_replay_events_for_subscription(
            replay_through,
            relevant_updates[39],
            &Subscription::new(StreamName::Hashtag, Some("RePlay".to_owned())),
            101,
        )
        .await?;
    assert_eq!(
        subscribe_race_replay
            .iter()
            .filter(|event| event.event == "update")
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        relevant_updates[40..],
        "only creates committed after the connection cursor close the subscribe race"
    );
    assert!(route_replay.iter().all(|event| {
        event
            .before
            .as_ref()
            .or(event.after.as_ref())
            .is_some_and(|snapshot| snapshot.tags.iter().any(|tag| tag == "replay"))
    }));

    sqlx::query("DELETE FROM rustodon.outbox_events WHERE logical_key LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn retention_pruning_does_not_hold_the_stream_writer_order_lock()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let pool = PgPool::connect(&url).await?;
    let queue = Queue::new(pool.clone());
    let prefix = format!("stream:prune-contention:{}", std::process::id());
    let old_key = format!("{prefix}:old");
    let writer_key = format!("{prefix}:writer");
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE logical_key LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await?;
    let mut transaction = pool.begin().await?;
    let old_id =
        record_stream_event_in(&mut transaction, 101, "delete", 904_000_001, &old_key).await?;
    transaction.commit().await?;
    sqlx::query(
        "UPDATE rustodon.outbox_events SET created_at = clock_timestamp() - INTERVAL '2 hours' \
         WHERE id = $1",
    )
    .bind(old_id)
    .execute(&pool)
    .await?;

    let mut blocker = pool.begin().await?;
    sqlx::query("SELECT id FROM rustodon.outbox_events WHERE id = $1 FOR UPDATE")
        .bind(old_id)
        .fetch_one(&mut *blocker)
        .await?;
    let prune = {
        let queue = queue.clone();
        tokio::spawn(async move {
            queue
                .prune_stream_history_with_limits(ChronoDuration::hours(1), 20_000)
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !prune.is_finished(),
        "prune did not reach the blocked old row"
    );

    let writer = {
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut transaction = pool.begin().await?;
            let id =
                record_stream_event_in(&mut transaction, 101, "delete", 904_000_002, &writer_key)
                    .await?;
            transaction.commit().await?;
            Ok::<i64, rustodon::jobs::JobError>(id)
        })
    };
    let writer_id = timeout(Duration::from_secs(5), writer).await???;
    assert!(writer_id > old_id);
    blocker.rollback().await?;
    timeout(Duration::from_secs(5), prune).await???;

    sqlx::query("DELETE FROM rustodon.outbox_events WHERE logical_key LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn staged_stream_flush_is_bounded_ordered_and_does_not_lock_early()
-> Result<(), Box<dyn std::error::Error>> {
    const FIRST_OBJECT: i64 = 905_000_000;
    const EVENT_COUNT: i64 = 513;
    const UNRELATED_OBJECT: i64 = 905_999_999;

    let url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let pool = PgPool::connect(&url).await?;
    let queue = Queue::new(pool.clone());
    let prefix = format!("stream:staged-flush:{}", std::process::id());
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE logical_key LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await?;
    let cursor = queue.stream_cursor().await?;
    let mut transaction = pool.begin().await?;
    let mut staged = StreamEventStagingProbe::new();
    for index in 0..300_i64 {
        staged.push(
            101,
            "delete",
            FIRST_OBJECT + index,
            &format!("{prefix}:event:{index}"),
        )?;
    }
    staged.stage(&mut transaction).await?;
    for index in 300..EVENT_COUNT {
        staged.push(
            101,
            "delete",
            FIRST_OBJECT + index,
            &format!("{prefix}:event:{index}"),
        )?;
    }
    staged.push(
        101,
        "delete",
        FIRST_OBJECT + 200,
        &format!("{prefix}:event:200"),
    )?;
    staged.stage(&mut transaction).await?;

    let unrelated_key = format!("{prefix}:unrelated");
    let unrelated_id = timeout(Duration::from_secs(5), async {
        let mut writer = pool.begin().await?;
        let id =
            record_stream_event_in(&mut writer, 101, "delete", UNRELATED_OBJECT, &unrelated_key)
                .await?;
        writer.commit().await?;
        Ok::<i64, rustodon::jobs::JobError>(id)
    })
    .await??;

    staged.flush(&mut transaction).await?;
    transaction.commit().await?;
    let events = queue.stream_events_after(cursor, 1_000).await?;
    let relevant = events
        .into_iter()
        .filter(|event| {
            event.object_id == UNRELATED_OBJECT
                || (FIRST_OBJECT..FIRST_OBJECT + EVENT_COUNT).contains(&event.object_id)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        relevant.len(),
        usize::try_from(EVENT_COUNT).expect("event count fits usize") + 1
    );
    assert_eq!(relevant[0].id, unrelated_id);
    assert_eq!(relevant[0].object_id, UNRELATED_OBJECT);
    assert_eq!(
        relevant[1..]
            .iter()
            .map(|event| event.object_id)
            .collect::<Vec<_>>(),
        (FIRST_OBJECT..FIRST_OBJECT + EVENT_COUNT).collect::<Vec<_>>(),
        "multiple staged chunks must flush in insertion order and deduplicate logical keys"
    );

    sqlx::query("DELETE FROM rustodon.outbox_events WHERE logical_key LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn stream_cursor_does_not_skip_inflight_commits() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let pool = PgPool::connect(&url).await?;
    let queue = Queue::new(pool.clone());
    let first_key = "stream:commit-order:101:update:900000001:1";
    let second_key = "stream:commit-order:101:update:900000002:1";
    for key in [first_key, second_key] {
        sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
            .bind(rustodon::streaming::STREAM_EVENT_KIND)
            .bind(key)
            .execute(&pool)
            .await?;
    }

    let cursor = queue.stream_cursor().await?;
    let mut first_transaction = pool.begin().await?;
    let first_id = record_stream_event_in(
        &mut first_transaction,
        101,
        "update",
        900_000_001,
        first_key,
    )
    .await?;

    let reader_barrier = Arc::new(Barrier::new(2));
    let mut reader = {
        let barrier = Arc::clone(&reader_barrier);
        let queue = queue.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            queue.stream_events_after(cursor, 10).await
        })
    };
    let writer_barrier = Arc::new(Barrier::new(2));
    let second = {
        let barrier = Arc::clone(&writer_barrier);
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut transaction = pool.begin().await?;
            barrier.wait().await;
            let second_id =
                record_stream_event_in(&mut transaction, 101, "update", 900_000_002, second_key)
                    .await?;
            transaction.commit().await?;
            Ok::<i64, rustodon::jobs::JobError>(second_id)
        })
    };
    reader_barrier.wait().await;
    writer_barrier.wait().await;
    let reader_events = timeout(Duration::from_secs(5), &mut reader).await??;
    assert!(
        reader_events?
            .iter()
            .all(|event| event.object_id != 900_000_001),
        "a lock-free stream reader must not observe an uncommitted event"
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !second.is_finished(),
        "a later stream writer must wait for the earlier transaction to commit"
    );

    first_transaction.commit().await?;
    let second_result = timeout(Duration::from_secs(5), second).await??;
    let second_id = second_result?;
    let events = queue.stream_events_after(cursor, 10).await?;
    let event_ids = events
        .iter()
        .filter(|event| matches!(event.object_id, 900_000_001 | 900_000_002))
        .map(|event| event.id)
        .collect::<Vec<_>>();
    assert_eq!(event_ids, vec![first_id, second_id]);

    for key in [first_key, second_key] {
        sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
            .bind(rustodon::streaming::STREAM_EVENT_KIND)
            .bind(key)
            .execute(&pool)
            .await?;
    }
    Ok(())
}

async fn assert_active_record_migration_lock(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut blocker = PgConnection::connect(url).await?;
    let database = sqlx::query_scalar::<_, String>("SELECT pg_catalog.current_database()")
        .fetch_one(&mut blocker)
        .await?;
    let lock_id = 2_053_462_845_i64 * i64::from(crc32(database.as_bytes()));
    sqlx::query("SELECT pg_catalog.pg_advisory_lock($1)")
        .bind(lock_id)
        .execute(&mut blocker)
        .await?;
    let mut migrator = PgConnection::connect(url).await?;
    assert!(
        timeout(Duration::from_millis(250), migrate(&mut migrator))
            .await
            .is_err(),
        "migration did not wait for Active Record's advisory lock"
    );
    sqlx::query("SELECT pg_catalog.pg_advisory_unlock($1)")
        .bind(lock_id)
        .execute(&mut blocker)
        .await?;
    timeout(Duration::from_secs(30), migrate(&mut migrator)).await??;
    Ok(())
}

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(u32::MAX, |mut crc, byte| {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
        crc
    })
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn operational_schema_rejects_namespace_and_access_drift()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let admin_url = std::env::var("RUSTODON_OPERATIONAL_ADMIN_DATABASE_URL")?;
    let mut connection = PgConnection::connect(&url).await?;
    let mut admin = PgConnection::connect(&admin_url).await?;
    migrate(&mut connection).await?;

    sqlx::query("GRANT SELECT ON rustodon.heartbeats TO rustodon_fixture")
        .execute(&mut connection)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("REVOKE SELECT ON rustodon.heartbeats FROM rustodon_fixture")
        .execute(&mut connection)
        .await?;

    sqlx::query("CREATE RULE heartbeat_noop AS ON UPDATE TO rustodon.heartbeats DO ALSO NOTHING")
        .execute(&mut connection)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP RULE heartbeat_noop ON rustodon.heartbeats")
        .execute(&mut connection)
        .await?;

    sqlx::query(
        "CREATE TRIGGER heartbeat_noop BEFORE UPDATE ON rustodon.heartbeats \
         FOR EACH ROW EXECUTE FUNCTION pg_catalog.suppress_redundant_updates_trigger()",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP TRIGGER heartbeat_noop ON rustodon.heartbeats")
        .execute(&mut connection)
        .await?;

    sqlx::query("CREATE POLICY heartbeat_policy ON rustodon.heartbeats USING (true)")
        .execute(&mut connection)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP POLICY heartbeat_policy ON rustodon.heartbeats")
        .execute(&mut connection)
        .await?;

    sqlx::query(
        "ALTER DEFAULT PRIVILEGES IN SCHEMA rustodon \
         GRANT SELECT ON TABLES TO rustodon_fixture",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query(
        "ALTER DEFAULT PRIVILEGES IN SCHEMA rustodon \
         REVOKE SELECT ON TABLES FROM rustodon_fixture",
    )
    .execute(&mut connection)
    .await?;

    sqlx::raw_sql(
        "CREATE COLLATION rustodon.\"C\" FROM pg_catalog.\"C\"; \
         ALTER TABLE rustodon.domain_health ALTER COLUMN last_error \
           TYPE text COLLATE rustodon.\"C\"",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::raw_sql(
        "ALTER TABLE rustodon.domain_health ALTER COLUMN last_error \
           TYPE text COLLATE pg_catalog.\"default\"; \
         DROP COLLATION rustodon.\"C\"",
    )
    .execute(&mut connection)
    .await?;

    sqlx::query(
        "CREATE TEXT SEARCH DICTIONARY rustodon.unexpected \
         (TEMPLATE = pg_catalog.simple)",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP TEXT SEARCH DICTIONARY rustodon.unexpected")
        .execute(&mut connection)
        .await?;

    sqlx::query("CREATE TYPE rustodon.unexpected AS ENUM ('x')")
        .execute(&mut connection)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP TYPE rustodon.unexpected")
        .execute(&mut connection)
        .await?;

    sqlx::query("COMMENT ON TABLE rustodon.heartbeats IS 'unexpected'")
        .execute(&mut connection)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("COMMENT ON TABLE rustodon.heartbeats IS NULL")
        .execute(&mut connection)
        .await?;

    sqlx::query(
        "COMMENT ON CONSTRAINT durable_jobs_kind_check ON rustodon.durable_jobs \
         IS 'unexpected'",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("COMMENT ON CONSTRAINT durable_jobs_kind_check ON rustodon.durable_jobs IS NULL")
        .execute(&mut connection)
        .await?;

    sqlx::query(
        "REVOKE SELECT, UPDATE ON SEQUENCE rustodon.durable_jobs_id_seq \
         FROM rustodon_schema_migrator",
    )
    .execute(&mut connection)
    .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query(
        "GRANT SELECT, UPDATE, USAGE ON SEQUENCE rustodon.durable_jobs_id_seq \
         TO rustodon_schema_migrator",
    )
    .execute(&mut connection)
    .await?;

    sqlx::query("ALTER EXTENSION plpgsql ADD SCHEMA rustodon")
        .execute(&mut admin)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("ALTER EXTENSION plpgsql DROP SCHEMA rustodon")
        .execute(&mut admin)
        .await?;

    sqlx::query("ALTER EXTENSION plpgsql ADD TABLE rustodon.heartbeats")
        .execute(&mut admin)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("ALTER EXTENSION plpgsql DROP TABLE rustodon.heartbeats")
        .execute(&mut admin)
        .await?;

    sqlx::query("CREATE VIEW public.rustodon_dependency AS SELECT * FROM rustodon.heartbeats")
        .execute(&mut admin)
        .await?;
    assert_schema_drift(&mut connection).await;
    sqlx::query("DROP VIEW public.rustodon_dependency")
        .execute(&mut admin)
        .await?;

    sqlx::raw_sql(
        "CREATE FUNCTION public.rustodon_test_event_trigger() RETURNS event_trigger \
           LANGUAGE plpgsql AS $$ BEGIN END $$; \
         CREATE EVENT TRIGGER rustodon_test_event_trigger ON ddl_command_end \
           EXECUTE FUNCTION public.rustodon_test_event_trigger()",
    )
    .execute(&mut admin)
    .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::UnsupportedMastodonSchema(_))
    ));
    sqlx::raw_sql(
        "DROP EVENT TRIGGER rustodon_test_event_trigger; \
         DROP FUNCTION public.rustodon_test_event_trigger()",
    )
    .execute(&mut admin)
    .await?;

    sqlx::query("CREATE PUBLICATION rustodon_test_publication FOR ALL TABLES")
        .execute(&mut admin)
        .await?;
    assert!(matches!(
        migrate(&mut connection).await,
        Err(MigrationError::UnsupportedMastodonSchema(_))
    ));
    sqlx::query("DROP PUBLICATION rustodon_test_publication")
        .execute(&mut admin)
        .await?;

    sqlx::raw_sql(
        "REASSIGN OWNED BY rustodon_schema_migrator TO rustodon_displaced_owner; \
         GRANT USAGE ON SCHEMA public TO rustodon_displaced_owner; \
         GRANT SELECT ON ALL TABLES IN SCHEMA public TO rustodon_displaced_owner; \
         GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO rustodon_displaced_owner; \
         SET ROLE rustodon_displaced_owner",
    )
    .execute(&mut admin)
    .await?;
    let ownership_result = migrate(&mut admin).await;
    sqlx::raw_sql(
        "RESET ROLE; \
         REASSIGN OWNED BY rustodon_displaced_owner TO rustodon_schema_migrator; \
         REVOKE USAGE ON SCHEMA public FROM rustodon_displaced_owner; \
         REVOKE SELECT ON ALL TABLES IN SCHEMA public FROM rustodon_displaced_owner; \
         REVOKE SELECT ON ALL SEQUENCES IN SCHEMA public FROM rustodon_displaced_owner",
    )
    .execute(&mut admin)
    .await?;
    assert!(matches!(
        ownership_result,
        Err(MigrationError::SchemaDrift(_))
    ));

    migrate(&mut connection).await?;
    Ok(())
}

async fn assert_schema_drift(connection: &mut PgConnection) {
    let result = migrate(connection).await;
    assert!(
        matches!(result, Err(MigrationError::SchemaDrift(_))),
        "expected schema drift, got {result:?}"
    );
}

async fn assert_schema(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    let tables = sqlx::query_scalar::<_, String>(
        "SELECT c.relname::text FROM pg_catalog.pg_class c \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'rustodon' AND c.relkind = 'r' \
           AND c.relname <> 'schema_migrations' ORDER BY c.relname",
    )
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(
        tables,
        TABLES
            .iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT version FROM rustodon.schema_migrations ORDER BY version DESC LIMIT 1",
        )
        .fetch_one(&mut *connection)
        .await?,
        CURRENT_VERSION
    );
    let stream_indexes = sqlx::query_scalar::<_, String>(
        "SELECT indexdef FROM pg_indexes WHERE schemaname = 'rustodon' \
           AND indexname IN ('outbox_events_pending_idx', \
             'outbox_events_stream_created_at_idx', 'outbox_events_stream_id_idx') \
         ORDER BY indexname",
    )
    .fetch_all(&mut *connection)
    .await?;
    assert_eq!(
        stream_indexes,
        vec![
            "CREATE INDEX outbox_events_pending_idx ON rustodon.outbox_events USING btree (id) WHERE ((dispatched_at IS NULL) AND (kind <> 'rustodon.mastodon.stream_event'::text))",
            "CREATE INDEX outbox_events_stream_created_at_idx ON rustodon.outbox_events USING btree (kind, created_at) WHERE (kind = 'rustodon.mastodon.stream_event'::text)",
            "CREATE INDEX outbox_events_stream_id_idx ON rustodon.outbox_events USING btree (kind, id) WHERE (kind = 'rustodon.mastodon.stream_event'::text)",
        ]
    );
    Ok(())
}

async fn applied_at(connection: &mut PgConnection) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT applied_at::text FROM rustodon.schema_migrations WHERE version = 1")
        .fetch_one(connection)
        .await
}

#[tokio::test]
#[ignore = "requires a disposable migrated PG14 database with restricted runtime/writer roles"]
async fn instance_activity_upgrade_from_five_preserves_history_and_grants()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owner =
        PgConnection::connect(&std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?).await?;
    let mut writer =
        PgConnection::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let mut reader = PgConnection::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer_name: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&mut writer)
        .await?;
    rustodon::preflight::validate_writer_connection(&mut writer).await?;
    let public_before: serde_json::Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(u) ORDER BY id) FROM public.users u")
            .fetch_one(&mut owner)
            .await?;
    sqlx::raw_sql("DROP TABLE rustodon.activity_members, rustodon.activity_buckets; DELETE FROM rustodon.schema_migrations WHERE version=6")
        .execute(&mut owner).await?;
    for _ in 0..2 {
        rustodon::operational_schema::migrate_with_writer_role(&mut owner, Some(&writer_name))
            .await?;
    }
    let public_after: serde_json::Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(u) ORDER BY id) FROM public.users u")
            .fetch_one(&mut owner)
            .await?;
    assert_eq!(public_before, public_after);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM rustodon.activity_members")
            .fetch_one(&mut reader)
            .await?,
        0,
        "upgrade does not invent activation history"
    );
    sqlx::query("SELECT set_config('rustodon.writer_role', $1, false)")
        .bind(&writer_name)
        .execute(&mut reader)
        .await?;
    rustodon::operational_schema::validate(&mut reader).await?;
    for table in ["activity_buckets", "activity_members"] {
        for privilege in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
            assert!(
                sqlx::query_scalar::<_, bool>("SELECT has_table_privilege(current_user, $1, $2)")
                    .bind(format!("rustodon.{table}"))
                    .bind(privilege)
                    .fetch_one(&mut writer)
                    .await?
            );
        }
        for privilege in [
            "INSERT",
            "UPDATE",
            "DELETE",
            "TRUNCATE",
            "REFERENCES",
            "TRIGGER",
        ] {
            assert!(
                !sqlx::query_scalar::<_, bool>("SELECT has_table_privilege(current_user, $1, $2)")
                    .bind(format!("rustodon.{table}"))
                    .bind(privilege)
                    .fetch_one(&mut reader)
                    .await?
            );
        }
    }
    sqlx::query("ALTER TABLE rustodon.activity_buckets ADD COLUMN drift bigint")
        .execute(&mut owner)
        .await?;
    assert!(matches!(
        rustodon::operational_schema::validate(&mut reader).await,
        Err(MigrationError::SchemaDrift(_))
    ));
    sqlx::query("ALTER TABLE rustodon.activity_buckets DROP COLUMN drift")
        .execute(&mut owner)
        .await?;
    rustodon::operational_schema::validate(&mut reader).await?;
    Ok(())
}
