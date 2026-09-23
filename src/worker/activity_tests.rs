use super::*;

#[tokio::test]
#[ignore = "requires disposable migrated PG14 and restricted runtime/writer roles"]
async fn maintenance_prune_uses_writer_for_activity() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer = PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    sqlx::raw_sql("DELETE FROM rustodon.activity_members; DELETE FROM rustodon.activity_buckets; \
        INSERT INTO rustodon.activity_buckets VALUES ('2000-01-01', clock_timestamp()-interval '1 second'); \
        INSERT INTO rustodon.activity_members SELECT '2000-01-01'::date, generate_series(1,1001)")
        .execute(&writer).await?;
    assert!(
        sqlx::query("DELETE FROM rustodon.activity_members WHERE false")
            .execute(&runtime)
            .await
            .is_err()
    );
    let queue = Queue::new(runtime.clone());
    let handlers = infrastructure_handlers_with_writer(&queue, Some(writer))?;
    let handler = handlers
        .get("rustodon.maintenance.prune")?
        .expect("existing maintenance handler");
    let job = ClaimedJob {
        id: 1,
        lane: Lane::Maintenance,
        kind: "rustodon.maintenance.prune".into(),
        arguments: serde_json::json!({}),
        logical_key: None,
        run_at: Utc::now(),
        attempt: 1,
        max_attempts: 3,
        generation: 1,
        lease_owner: "activity-fixture".into(),
        lease_expires_at: Utc::now() + chrono::Duration::minutes(1),
    };
    (handler.run)(job)
        .await
        .map_err(|error| format!("{error:?}"))?;
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM rustodon.activity_members")
        .fetch_one(&runtime)
        .await?;
    assert_eq!(
        members, 1,
        "one bounded chunk through real registered handler"
    );
    let buckets: i64 = sqlx::query_scalar("SELECT count(*) FROM rustodon.activity_buckets")
        .fetch_one(&runtime)
        .await?;
    assert_eq!(buckets, 1, "partial bucket retained atomically");
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable migrated PG14 and restricted runtime/writer roles"]
async fn maintenance_prune_failure_still_runs_operational_cleanup()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer = PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    sqlx::query(
        "INSERT INTO rustodon.idempotency_keys \
             (scope, key, fingerprint, result, created_at, expires_at) \
         VALUES ('activity-prune-test', 'expired', $1, '{}'::jsonb, \
                 clock_timestamp() - interval '2 seconds', \
                 clock_timestamp() - interval '1 second')",
    )
    .bind([0_u8; 32].as_slice())
    .execute(&runtime)
    .await?;
    let mut blocker = writer.begin().await?;
    sqlx::query("LOCK rustodon.activity_buckets IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let queue = Queue::new(runtime.clone());
    let handlers = infrastructure_handlers_with_writer(&queue, Some(writer.clone()))?;
    let handler = handlers
        .get("rustodon.maintenance.prune")?
        .expect("existing maintenance handler");
    let job = ClaimedJob {
        id: 1,
        lane: Lane::Maintenance,
        kind: "rustodon.maintenance.prune".into(),
        arguments: serde_json::json!({}),
        logical_key: None,
        run_at: Utc::now(),
        attempt: 1,
        max_attempts: 3,
        generation: 1,
        lease_owner: "activity-fixture".into(),
        lease_expires_at: Utc::now() + chrono::Duration::minutes(1),
    };
    let result = (handler.run)(job).await;
    blocker.rollback().await?;
    assert!(result.is_err(), "activity failure still retries the job");
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.idempotency_keys WHERE scope = 'activity-prune-test'",
    )
    .fetch_one(&runtime)
    .await?;
    assert_eq!(
        remaining, 0,
        "operational cleanup ran despite activity failure"
    );
    Ok(())
}
