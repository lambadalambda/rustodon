use super::*;

#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn startup_poll_reconciliation_has_a_hard_readiness_bound()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let startup_writer = PgPoolOptions::new()
        .max_connections(1)
        .connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?)
        .await?;
    reset().await?;

    let queue = Queue::new(runtime);
    let handlers_a = infrastructure_handlers_with_writer(&queue, Some(startup_writer.clone()))?;
    let handlers_b = infrastructure_handlers_with_writer(&queue, Some(startup_writer.clone()))?;
    let config = WorkerConfig {
        lanes: [Lane::Maintenance].into_iter().collect(),
        concurrency: 1,
        remote_http_concurrency: 1,
        media_concurrency: 1,
        lease_seconds: 30,
        poll_milliseconds: 10,
        heartbeat_seconds: 60,
        shutdown_seconds: 2,
    };
    let held_writer = startup_writer.acquire().await?;
    let start = Arc::new(Barrier::new(3));
    let start_a = start.clone();
    let queue_a = queue.clone();
    let writer_a = startup_writer.clone();
    let config_a = config.clone();
    let runtime_a = tokio::spawn(async move {
        start_a.wait().await;
        run_until_shutdown(
            queue_a,
            handlers_a,
            config_a,
            "poll-reconciliation-bounded-owner".to_owned(),
            Some(writer_a),
            std::future::pending::<()>(),
        )
        .await
    });
    let start_b = start.clone();
    let queue_b = queue.clone();
    let writer_b = startup_writer.clone();
    let runtime_b = tokio::spawn(async move {
        start_b.wait().await;
        run_until_shutdown(
            queue_b,
            handlers_b,
            config,
            "poll-reconciliation-bounded-peer".to_owned(),
            Some(writer_b),
            std::future::pending::<()>(),
        )
        .await
    });
    let started = tokio::time::Instant::now();
    start.wait().await;
    let (result_a, result_b) = tokio::join!(runtime_a, runtime_b);
    let elapsed = started.elapsed();

    assert!(matches!(
        result_a?,
        Err(WorkerError::StartupReconciliationFailed)
    ));
    assert!(matches!(
        result_b?,
        Err(WorkerError::StartupReconciliationFailed)
    ));
    assert!(
        elapsed < std::time::Duration::from_millis(2_250),
        "startup readiness exceeded its two-second reconciliation bound: {elapsed:?}"
    );
    let heartbeats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.heartbeats \
         WHERE process_id LIKE 'poll-reconciliation-bounded-%'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        heartbeats, 0,
        "neither a failed owner nor its waiting peer may advertise readiness"
    );
    let (retry_reservations, generation, lease_expires_at): (i64, i64, Option<DateTime<Utc>>) =
        sqlx::query_as(
            "SELECT count(*) OVER (), lease_generation, lease_expires_at \
             FROM rustodon.durable_jobs WHERE kind = $1 AND dead_at IS NULL",
        )
        .bind(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)
        .fetch_one(&owner)
        .await?;
    let lease_expires_at = lease_expires_at
        .ok_or("startup left no lease on its reservation")?;
    assert_eq!(retry_reservations, 1);
    assert_eq!(generation, 1, "startup must claim the exact reservation");
    assert!(
        lease_expires_at > Utc::now(),
        "timed-out startup retains its nonrenewed lease"
    );
    assert!(
        lease_expires_at <= Utc::now() + Duration::seconds(2),
        "startup must not renew or extend its fixed three-second lease"
    );
    let success_watermarks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.idempotency_keys \
         WHERE scope = 'rustodon.mastodon.poll_expiration_reconcile_success'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(success_watermarks, 0);

    drop(held_writer);
    let wait = (lease_expires_at - Utc::now())
        .to_std()
        .unwrap_or_default()
        .saturating_add(std::time::Duration::from_millis(50));
    tokio::time::sleep(wait).await;
    let restart_handlers =
        infrastructure_handlers_with_writer(&queue, Some(startup_writer.clone()))?;
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let restart_owner = startup_writer.clone();
    let restart = tokio::spawn(async move {
        run_until_shutdown(
            queue,
            restart_handlers,
            WorkerConfig {
                lanes: [Lane::Maintenance].into_iter().collect(),
                concurrency: 1,
                remote_http_concurrency: 1,
                media_concurrency: 1,
                lease_seconds: 30,
                poll_milliseconds: 10,
                heartbeat_seconds: 60,
                shutdown_seconds: 2,
            },
            "poll-reconciliation-timeout-restart".to_owned(),
            Some(restart_owner),
            async move {
                let _ = shutdown_receiver.await;
            },
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let ready: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.heartbeats \
                 WHERE process_id LIKE 'poll-reconciliation-timeout-restart:%'",
            )
            .fetch_one(&owner)
            .await?;
            if ready == 2 {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 AND dead_at IS NULL",
    )
    .bind(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        retained, 0,
        "restart completes the same timed-out reservation"
    );
    shutdown_sender.send(()).expect("restart is listening");
    restart.await??;
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn startup_zero_progress_consumes_attempt_and_dead_letters_before_readiness()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    queue.ensure_poll_expiration_activation_for_test().await?;
    let scan_started_at = Utc::now();
    let arguments = json!({
        "scan_started_at": scan_started_at.to_rfc3339(),
        "through_poll_id": null,
        "after_poll_id": 0,
        "segment": 0,
    });
    let logical_key = format!(
        "poll-expiration-reconcile:{}:0:0",
        scan_started_at.timestamp_micros()
    );
    let job_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Maintenance,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                arguments.clone(),
            )
            .logical_key(&logical_key)
            .max_attempts(1)
            .run_at(Utc::now() + Duration::days(1)),
        )
        .await?;
    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let mut blocker = owner.begin().await?;
    sqlx::query("LOCK TABLE polls IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;

    let result = run_until_shutdown(
        queue,
        handlers,
        WorkerConfig {
            lanes: [Lane::Maintenance].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 10,
            heartbeat_seconds: 60,
            shutdown_seconds: 2,
        },
        "poll-reconciliation-no-progress-startup".to_owned(),
        Some(owner.clone()),
        std::future::pending::<()>(),
    )
    .await;
    assert!(matches!(
        result,
        Err(WorkerError::StartupReconciliationFailed)
    ));
    blocker.rollback().await?;

    let persisted: (i64, i32, i32, Value, String, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT id, attempts, max_attempts, arguments, logical_key, dead_at \
         FROM rustodon.durable_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_one(&owner)
    .await?;
    assert_eq!(persisted.0, job_id);
    assert_eq!((persisted.1, persisted.2), (1, 1));
    assert_eq!(persisted.3, arguments);
    assert_eq!(persisted.4, logical_key);
    assert!(persisted.5.is_some(), "startup failure must dead-letter");
    let history: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.durable_jobs \
         WHERE kind = $1 AND arguments ->> 'scan_started_at' = $2",
    )
    .bind(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)
    .bind(scan_started_at.to_rfc3339())
    .fetch_one(&owner)
    .await?;
    assert_eq!(history, 1, "startup must not enqueue a successor");
    let watermarks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.idempotency_keys \
         WHERE scope = 'rustodon.mastodon.poll_expiration_reconcile_success' \
           AND result ->> 'job_id' = $1",
    )
    .bind(job_id.to_string())
    .fetch_one(&owner)
    .await?;
    assert_eq!(watermarks, 0);
    let heartbeats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.heartbeats \
         WHERE process_id LIKE 'poll-reconciliation-no-progress-startup:%'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(heartbeats, 0, "failed startup must not advertise readiness");

    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn zero_progress_poll_reconciliation_retries_same_rows_until_dead_letter()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    let handlers = HandlerRegistry::new();
    let handler_queue = queue.clone();
    let handler_pool = owner.clone();
    handlers.register(
        MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
        Lane::Maintenance,
        ResourceClass::None,
        move |job| {
            let queue = handler_queue.clone();
            let pool = handler_pool.clone();
            async move {
                if job.arguments.get("scan_mode").and_then(Value::as_str) == Some("raw") {
                    reconcile_poll_expirations_with_exhausted_raw_budget_for_test(
                        queue,
                        pool,
                        job.arguments,
                        job.attempt,
                    )
                    .await
                } else {
                    reconcile_poll_expirations_with_exhausted_primary_timeout_for_test(
                        queue,
                        pool,
                        job.arguments,
                        job.attempt,
                    )
                    .await
                }
            }
        },
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    // Worker startup normally creates the activation marker before any executor runs;
    // reset() truncated it, so establish it as the startup would.
    queue.ensure_poll_expiration_activation_for_test().await?;
    let scan_seed = Utc::now();

    for (offset, (label, explicit_raw)) in [("optimized", false), ("raw", true)]
        .into_iter()
        .enumerate()
    {
        let scan_started_at = scan_seed
            + Duration::microseconds(i64::try_from(offset).expect("two cases fit in i64"));
        let segment = i64::from(explicit_raw);
        let mut arguments = json!({
            "scan_started_at": scan_started_at.to_rfc3339(),
            "through_poll_id": 1,
            "after_poll_id": 0,
            "segment": segment,
        });
        if explicit_raw {
            arguments["scan_mode"] = json!("raw");
        }
        let logical_key = if explicit_raw {
            format!(
                "poll-expiration-reconcile:{}:raw:1:0",
                scan_started_at.timestamp_micros()
            )
        } else {
            format!(
                "poll-expiration-reconcile:{}:0:0",
                scan_started_at.timestamp_micros()
            )
        };
        let job_id = queue
            .enqueue(
                &JobSpec::new(
                    Lane::Maintenance,
                    MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                    arguments.clone(),
                )
                .logical_key(&logical_key)
                .max_attempts(2),
            )
            .await?;

        for attempt in 1..=2 {
            assert!(
                executor
                    .process_one(
                        &format!("{label}-no-progress-{attempt}"),
                        &[Lane::Maintenance],
                        Duration::seconds(30),
                    )
                    .await?
            );
            let persisted: (i64, i32, i32, Value, String, Option<DateTime<Utc>>) = sqlx::query_as(
                "SELECT id, attempts, max_attempts, arguments, logical_key, dead_at \
                     FROM rustodon.durable_jobs WHERE id = $1",
            )
            .bind(job_id)
            .fetch_one(&owner)
            .await?;
            let last_error: Option<String> =
                sqlx::query_scalar("SELECT last_error FROM rustodon.durable_jobs WHERE id = $1")
                    .bind(job_id)
                    .fetch_one(&owner)
                    .await?;
            assert_eq!(persisted.0, job_id, "{label} retry changed job ID");
            assert_eq!(persisted.1, attempt);
            assert_eq!(persisted.2, 2);
            assert_eq!(persisted.3, arguments, "{label} retry mutated arguments");
            assert_eq!(persisted.4, logical_key, "{label} retry mutated its key");
            assert_eq!(
                persisted.5.is_some(),
                attempt == 2,
                "{label} attempt {attempt}: {last_error:?}"
            );

            let durable_history: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.durable_jobs \
                 WHERE kind = $1 AND arguments ->> 'scan_started_at' = $2",
            )
            .bind(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)
            .bind(scan_started_at.to_rfc3339())
            .fetch_one(&owner)
            .await?;
            assert_eq!(durable_history, 1, "{label} created a successor row");
            let watermarks: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.idempotency_keys \
                 WHERE scope = 'rustodon.mastodon.poll_expiration_reconcile_success' \
                   AND result ->> 'job_id' = $1",
            )
            .bind(job_id.to_string())
            .fetch_one(&owner)
            .await?;
            assert_eq!(watermarks, 0, "{label} published a success watermark");

            if attempt == 1 {
                sqlx::query(
                    "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE id = $1",
                )
                .bind(job_id)
                .execute(&owner)
                .await?;
            }
        }
    }

    reset().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn blocked_activation_setup_is_inside_the_hard_readiness_bound()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let mut blocker = owner.begin().await?;
    sqlx::query(
        "INSERT INTO rustodon.outbox_events \
             (kind, logical_key, payload, created_at, dispatched_at) \
         VALUES ('rustodon.mastodon.poll_expiration_activation', 'v1', '{}', \
                 clock_timestamp(), clock_timestamp())",
    )
    .execute(&mut *blocker)
    .await?;

    let queue = Queue::new(runtime);
    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let started = tokio::time::Instant::now();
    let result = run_until_shutdown(
        queue.clone(),
        handlers,
        WorkerConfig {
            lanes: [Lane::Maintenance].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 10,
            heartbeat_seconds: 60,
            shutdown_seconds: 2,
        },
        "poll-activation-blocked".to_owned(),
        Some(owner.clone()),
        std::future::pending::<()>(),
    )
    .await;
    assert!(matches!(
        result,
        Err(WorkerError::StartupReconciliationFailed)
    ));
    assert!(
        started.elapsed() < std::time::Duration::from_millis(2_250),
        "activation setup must share the advertised two-second readiness bound"
    );
    blocker.rollback().await?;
    let activation_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events \
         WHERE kind = 'rustodon.mastodon.poll_expiration_activation' AND logical_key = 'v1'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(activation_rows, 0, "cancelled setup rolls back safely");
    let heartbeats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.heartbeats WHERE process_id LIKE 'poll-activation-blocked:%'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(heartbeats, 0);

    let mut core_blocker = owner.begin().await?;
    sqlx::query(
        "INSERT INTO rustodon.outbox_events \
             (kind, logical_key, payload, created_at, dispatched_at) \
         VALUES ('rustodon.mastodon.poll_expiration_activation', 'v1', '{}', \
                 clock_timestamp(), clock_timestamp())",
    )
    .execute(&mut *core_blocker)
    .await?;
    let core_handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let core_started = tokio::time::Instant::now();
    let core_result = run_until_shutdown(
        queue,
        core_handlers,
        WorkerConfig {
            lanes: [Lane::Core].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 10,
            heartbeat_seconds: 60,
            shutdown_seconds: 2,
        },
        "poll-core-activation-blocked".to_owned(),
        Some(owner.clone()),
        std::future::pending::<()>(),
    )
    .await;
    assert!(matches!(
        core_result,
        Err(WorkerError::StartupReconciliationFailed)
    ));
    assert!(
        core_started.elapsed() < std::time::Duration::from_millis(2_250),
        "Core-only activation must share the hard startup bound"
    );
    core_blocker.rollback().await?;
    let core_heartbeats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.heartbeats \
         WHERE process_id LIKE 'poll-core-activation-blocked:%'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(core_heartbeats, 0);
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn wrong_lane_reconciliation_fails_closed_without_a_claim()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    let scan_started_at = Utc::now();
    let logical_key = format!(
        "poll-expiration-reconcile:{}:0:0",
        scan_started_at.timestamp_micros()
    );
    let arguments = json!({
        "scan_started_at": scan_started_at.to_rfc3339(),
        "through_poll_id": 0,
        "after_poll_id": 0,
        "segment": 0,
    });
    let wrong_lane_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Core,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                arguments.clone(),
            )
            .logical_key(&logical_key),
        )
        .await?;
    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let result = run_until_shutdown(
        queue.clone(),
        handlers,
        WorkerConfig {
            lanes: [Lane::Maintenance].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 10,
            heartbeat_seconds: 60,
            shutdown_seconds: 2,
        },
        "poll-reconciliation-wrong-lane".to_owned(),
        Some(owner.clone()),
        std::future::pending::<()>(),
    )
    .await;
    assert!(result.is_err());
    let exact_continuation = JobSpec::new(
        Lane::Maintenance,
        MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
        arguments,
    )
    .logical_key(&logical_key);
    assert!(
        queue
            .enqueue_exact_for_test(&exact_continuation)
            .await
            .is_err(),
        "a wrong-lane logical-key conflict cannot satisfy exact continuation persistence"
    );
    let future_key = format!("{logical_key}:future");
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Maintenance,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                exact_continuation.arguments().clone(),
            )
            .logical_key(&future_key)
            .run_at(Utc::now() + Duration::days(7)),
        )
        .await?;
    let due_continuation = JobSpec::new(
        Lane::Maintenance,
        MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
        exact_continuation.arguments().clone(),
    )
    .logical_key(future_key);
    assert!(
        queue
            .enqueue_exact_for_test(&due_continuation)
            .await
            .is_err(),
        "a delayed conflict cannot postpone an exact due continuation"
    );
    let retained: (String, i32, i64, Option<String>) = sqlx::query_as(
        "SELECT lane, attempts, lease_generation, lease_owner FROM rustodon.durable_jobs \
         WHERE id = $1",
    )
    .bind(wrong_lane_id)
    .fetch_one(&owner)
    .await?;
    assert_eq!(retained, ("core".to_owned(), 0, 0, None));
    let watermarks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.idempotency_keys \
         WHERE scope = 'rustodon.mastodon.poll_expiration_reconcile_success'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(watermarks, 0);
    let heartbeats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.heartbeats \
         WHERE process_id LIKE 'poll-reconciliation-wrong-lane:%'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(heartbeats, 0);
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn existing_retry_without_a_success_watermark_cannot_publish_readiness()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;

    let queue = Queue::new(runtime);
    let scan_started_at = Utc::now();
    let retry_job_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Maintenance,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                json!({
                    "scan_started_at": scan_started_at.to_rfc3339(),
                    "through_poll_id": 0,
                    "after_poll_id": 0,
                    "segment": 0,
                }),
            )
            .logical_key(format!(
                "poll-expiration-reconcile:{}:0:0",
                scan_started_at.timestamp_micros()
            )),
        )
        .await?;
    let external_claim = queue
        .claim(
            "poll-reconciliation-active-external-owner",
            &[Lane::Maintenance],
            Duration::seconds(30),
        )
        .await?
        .expect("external owner claims the exact reconciliation first");
    let startup_handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let startup_result = run_until_shutdown(
        queue,
        startup_handlers,
        WorkerConfig {
            lanes: [Lane::Maintenance].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 10,
            heartbeat_seconds: 60,
            shutdown_seconds: 2,
        },
        "poll-reconciliation-failed-existing-startup".to_owned(),
        Some(owner.clone()),
        std::future::pending::<()>(),
    )
    .await;
    assert!(matches!(
        startup_result,
        Err(WorkerError::StartupReconciliationFailed)
    ));
    let retained: (String, i64, i32) = sqlx::query_as(
        "SELECT lease_owner, lease_generation, attempts FROM rustodon.durable_jobs \
         WHERE id = $1 AND dead_at IS NULL",
    )
    .bind(retry_job_id)
    .fetch_one(&owner)
    .await?;
    assert_eq!(retained.0, external_claim.lease_owner);
    assert_eq!(retained.1, external_claim.generation);
    assert_eq!(retained.2, external_claim.attempt);
    let success_watermarks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.idempotency_keys \
         WHERE scope = 'rustodon.mastodon.poll_expiration_reconcile_success'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        success_watermarks, 0,
        "a retained retry must not publish a success watermark"
    );
    let heartbeats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.heartbeats \
         WHERE process_id LIKE 'poll-reconciliation-failed-existing-startup:%'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(heartbeats, 0, "a waiting startup must not become ready");

    reset().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
async fn startup_claims_exact_unleased_future_reconciliation()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    let scan_started_at = Utc::now();
    let logical_key = format!(
        "poll-expiration-reconcile:{}:0:0",
        scan_started_at.timestamp_micros()
    );
    let original_arguments = json!({
        "scan_started_at": scan_started_at.to_rfc3339(),
        "through_poll_id": 0,
        "after_poll_id": 0,
        "segment": 0,
        "fixture": "preserve-original-arguments",
    });
    let job_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Maintenance,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                original_arguments.clone(),
            )
            .logical_key(&logical_key)
            .run_at(Utc::now() + Duration::days(7)),
        )
        .await?;
    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    run_until_shutdown(
        queue,
        handlers,
        WorkerConfig {
            lanes: [Lane::Maintenance].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 10,
            heartbeat_seconds: 60,
            shutdown_seconds: 2,
        },
        "poll-reconciliation-exact-future".to_owned(),
        Some(owner.clone()),
        async {},
    )
    .await?;
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rustodon.durable_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&owner)
            .await?;
    assert_eq!(remaining, 0, "startup executes the exact existing row");
    let success: Value = sqlx::query_scalar(
        "SELECT result FROM rustodon.idempotency_keys \
         WHERE scope = 'rustodon.mastodon.poll_expiration_reconcile_success'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        success,
        json!({
            "completed": true,
            "job_id": job_id,
            "logical_key": logical_key,
            "arguments": original_arguments,
        }),
        "completion watermark is bound to the exact original row and key"
    );
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
async fn prior_success_watermark_cannot_satisfy_replacement_reconciliation()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    let scan_started_at = Utc::now();
    let logical_key = format!(
        "poll-expiration-reconcile:{}:0:0",
        scan_started_at.timestamp_micros()
    );
    let spec = || {
        JobSpec::new(
            Lane::Maintenance,
            MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
            json!({
                "scan_started_at": scan_started_at.to_rfc3339(),
                "through_poll_id": 0,
                "after_poll_id": 0,
                "segment": 0,
            }),
        )
        .logical_key(&logical_key)
    };
    let old_id = queue.enqueue(&spec()).await?;
    let old_claim = queue
        .claim(
            "poll-reconciliation-prior-owner",
            &[Lane::Maintenance],
            Duration::seconds(30),
        )
        .await?
        .expect("the prior reconciliation is due");
    assert_eq!(old_claim.id, old_id);
    assert!(
        queue
            .complete_poll_expiration_reconciliation_success_for_test(&old_claim)
            .await?
    );

    let replacement_id = queue
        .enqueue(&spec().run_at(Utc::now() + Duration::days(7)))
        .await?;
    assert_ne!(replacement_id, old_id);
    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    run_until_shutdown(
        queue,
        handlers,
        WorkerConfig {
            lanes: [Lane::Maintenance].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 10,
            heartbeat_seconds: 60,
            shutdown_seconds: 2,
        },
        "poll-reconciliation-replacement".to_owned(),
        Some(owner.clone()),
        async {},
    )
    .await?;

    let replacement_remains: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM rustodon.durable_jobs WHERE id = $1)")
            .bind(replacement_id)
            .fetch_one(&owner)
            .await?;
    assert!(!replacement_remains, "startup executes the replacement row");
    let successful_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT (result ->> 'job_id')::bigint FROM rustodon.idempotency_keys \
         WHERE scope = 'rustodon.mastodon.poll_expiration_reconcile_success' ORDER BY 1",
    )
    .fetch_all(&owner)
    .await?;
    assert_eq!(successful_ids, [old_id, replacement_id]);
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn startup_reclaims_stale_reconciliation_and_fences_the_old_generation()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;

    let queue = Queue::new(runtime);
    let scan_started_at = Utc::now();
    let job_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Maintenance,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                json!({
                    "scan_started_at": scan_started_at.to_rfc3339(),
                    "through_poll_id": null,
                    "after_poll_id": 0,
                    "segment": 0,
                }),
            )
            .logical_key(format!(
                "poll-expiration-reconcile:{}:0:0",
                scan_started_at.timestamp_micros()
            )),
        )
        .await?;
    let stale_claim = queue
        .claim(
            "poll-reconciliation-stale-owner",
            &[Lane::Maintenance],
            Duration::seconds(30),
        )
        .await?
        .expect("the reconciliation fixture job is due");
    sqlx::query(
        "UPDATE rustodon.durable_jobs \
         SET lease_owner = 'poll-reconciliation-new-owner', \
             lease_generation = lease_generation + 1, \
             lease_expires_at = clock_timestamp() + interval '30 seconds' \
         WHERE id = $1",
    )
    .bind(job_id)
    .execute(&owner)
    .await?;
    assert!(
        !queue
            .complete_poll_expiration_reconciliation_success_for_test(&stale_claim)
            .await?,
        "a stale lease must lose the fenced completion"
    );
    sqlx::query(
        "UPDATE rustodon.durable_jobs \
         SET lease_owner = $2, lease_generation = $3, \
             lease_expires_at = clock_timestamp() - interval '1 second' \
         WHERE id = $1",
    )
    .bind(job_id)
    .bind(&stale_claim.lease_owner)
    .bind(stale_claim.generation)
    .execute(&owner)
    .await?;
    assert!(
        !queue
            .complete_poll_expiration_reconciliation_success_for_test(&stale_claim)
            .await?,
        "an expired matching lease must not publish successful completion"
    );

    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let result = run_until_shutdown(
        queue.clone(),
        handlers,
        WorkerConfig {
            lanes: [Lane::Maintenance].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 10,
            heartbeat_seconds: 60,
            shutdown_seconds: 2,
        },
        "poll-reconciliation-stale-startup".to_owned(),
        Some(owner.clone()),
        async {},
    )
    .await;
    assert!(result.is_ok(), "startup must take over the stale exact row");
    assert!(
        !queue
            .complete_poll_expiration_reconciliation_success_for_test(&stale_claim)
            .await?,
        "the prior generation remains fenced after startup takeover"
    );
    let retained_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rustodon.durable_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&owner)
            .await?;
    assert_eq!(retained_rows, 0, "startup completes the exact stale row");
    let success_watermarks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.idempotency_keys \
         WHERE scope = 'rustodon.mastodon.poll_expiration_reconcile_success'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(success_watermarks, 1);

    reset().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn concurrent_startups_share_one_poll_reconciliation_scan()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    sqlx::query(
        "INSERT INTO rustodon.idempotency_keys \
             (scope, key, fingerprint, result, expires_at) \
         VALUES ('rustodon.mastodon.poll_expiration_reconcile_success', \
                 'unrelated-reconciliation', decode(repeat('00', 32), 'hex'), \
                 '{\"completed\": true}'::jsonb, clock_timestamp() + interval '1 day')",
    )
    .execute(&owner)
    .await?;

    let queue = Queue::new(runtime);
    let handlers_a = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let handlers_b = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let config = WorkerConfig {
        lanes: [Lane::Maintenance].into_iter().collect(),
        concurrency: 1,
        remote_http_concurrency: 1,
        media_concurrency: 1,
        lease_seconds: 30,
        poll_milliseconds: 10,
        heartbeat_seconds: 60,
        shutdown_seconds: 2,
    };
    let mut scan_fence = owner.begin().await?;
    sqlx::query("LOCK TABLE polls IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *scan_fence)
        .await?;

    let (owner_stop_sender, owner_stop_receiver) = oneshot::channel();
    let (peer_done_sender, peer_done_receiver) = oneshot::channel();
    let start = Arc::new(Barrier::new(3));
    let start_a = start.clone();
    let queue_a = queue.clone();
    let owner_a = owner.clone();
    let config_a = config.clone();
    let runtime_a = tokio::spawn(async move {
        start_a.wait().await;
        run_until_shutdown(
            queue_a,
            handlers_a,
            config_a,
            "poll-reconciliation-race-a".to_owned(),
            Some(owner_a),
            async move {
                let _ = owner_stop_receiver.await;
            },
        )
        .await
    });
    let start_b = start.clone();
    let queue_b = queue.clone();
    let owner_b = owner.clone();
    let runtime_b = tokio::spawn(async move {
        start_b.wait().await;
        run_until_shutdown(
            queue_b,
            handlers_b,
            config,
            "poll-reconciliation-race-b".to_owned(),
            Some(owner_b),
            async move {
                let _ = peer_done_receiver.await;
            },
        )
        .await
    });
    start.wait().await;

    tokio::time::timeout(std::time::Duration::from_millis(750), async {
        loop {
            let blocked_scans: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE pid <> pg_backend_pid() \
                   AND query LIKE 'SELECT max(id) FROM polls%' \
                   AND wait_event_type = 'Lock'",
            )
            .fetch_one(&owner)
            .await?;
            let ready_heartbeats: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.heartbeats \
                 WHERE process_id LIKE 'poll-reconciliation-race-%'",
            )
            .fetch_one(&owner)
            .await?;
            let live_reconciliations: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.durable_jobs \
                 WHERE kind = $1 AND dead_at IS NULL",
            )
            .bind(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)
            .fetch_one(&owner)
            .await?;
            if blocked_scans == 1 && ready_heartbeats == 0 && live_reconciliations == 1 {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    scan_fence.rollback().await?;

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let ready_heartbeats: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.heartbeats \
                 WHERE process_id LIKE 'poll-reconciliation-race-%'",
            )
            .fetch_one(&owner)
            .await?;
            if ready_heartbeats == 4 {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    owner_stop_sender
        .send(())
        .expect("first startup runtime is listening");
    peer_done_sender
        .send(())
        .expect("second startup runtime is listening");
    runtime_a.await??;
    runtime_b.await??;

    reset().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn startups_defer_to_one_existing_periodic_poll_reconciliation()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;

    let queue = Queue::new(runtime);
    let scan_started_at = Utc::now();
    let periodic_job_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Maintenance,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                json!({
                    "scan_started_at": scan_started_at.to_rfc3339(),
                    "through_poll_id": null,
                    "after_poll_id": 0,
                    "segment": 0,
                }),
            )
            .logical_key(format!(
                "poll-expiration-reconcile:{}:0:0",
                scan_started_at.timestamp_micros()
            )),
        )
        .await?;
    let repair_handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let repair_executor = WorkerExecutor::new(queue.clone(), repair_handlers, 1, 1)?;
    let handlers_a = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let handlers_b = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let config = WorkerConfig {
        lanes: [Lane::Maintenance].into_iter().collect(),
        concurrency: 1,
        remote_http_concurrency: 1,
        media_concurrency: 1,
        lease_seconds: 30,
        poll_milliseconds: 10,
        heartbeat_seconds: 60,
        shutdown_seconds: 2,
    };
    let mut scan_fence = owner.begin().await?;
    sqlx::query("LOCK TABLE polls IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *scan_fence)
        .await?;

    let (first_stop_sender, first_stop_receiver) = oneshot::channel();
    let (other_done_sender, other_done_receiver) = oneshot::channel();
    let start = Arc::new(Barrier::new(4));
    let repair_start = start.clone();
    let repair_runtime = tokio::spawn(async move {
        repair_start.wait().await;
        repair_executor
            .process_one(
                "poll-reconciliation-periodic-fixture",
                &[Lane::Maintenance],
                Duration::seconds(30),
            )
            .await
    });
    let start_a = start.clone();
    let queue_a = queue.clone();
    let owner_a = owner.clone();
    let config_a = config.clone();
    let runtime_a = tokio::spawn(async move {
        start_a.wait().await;
        run_until_shutdown(
            queue_a,
            handlers_a,
            config_a,
            "poll-reconciliation-periodic-a".to_owned(),
            Some(owner_a),
            async move {
                let _ = first_stop_receiver.await;
            },
        )
        .await
    });
    let start_b = start.clone();
    let queue_b = queue.clone();
    let owner_b = owner.clone();
    let runtime_b = tokio::spawn(async move {
        start_b.wait().await;
        run_until_shutdown(
            queue_b,
            handlers_b,
            config,
            "poll-reconciliation-periodic-b".to_owned(),
            Some(owner_b),
            async move {
                let _ = other_done_receiver.await;
            },
        )
        .await
    });
    start.wait().await;

    tokio::time::timeout(std::time::Duration::from_millis(750), async {
        loop {
            let blocked_scans: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE pid <> pg_backend_pid() \
                   AND query LIKE 'SELECT max(id) FROM polls%' \
                   AND wait_event_type = 'Lock'",
            )
            .fetch_one(&owner)
            .await?;
            let ready_heartbeats: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.heartbeats \
                 WHERE process_id LIKE 'poll-reconciliation-periodic-%'",
            )
            .fetch_one(&owner)
            .await?;
            let live_reconciliations: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.durable_jobs \
                 WHERE kind = $1 AND dead_at IS NULL",
            )
            .bind(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)
            .fetch_one(&owner)
            .await?;
            if blocked_scans == 1 && ready_heartbeats == 0 && live_reconciliations == 1 {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    scan_fence.rollback().await?;

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let live_reconciliations: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.durable_jobs \
                 WHERE kind = $1 AND dead_at IS NULL",
            )
            .bind(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)
            .fetch_one(&owner)
            .await?;
            let ready_heartbeats: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.heartbeats \
                 WHERE process_id LIKE 'poll-reconciliation-periodic-%'",
            )
            .fetch_one(&owner)
            .await?;
            if live_reconciliations == 0 && ready_heartbeats == 4 {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    assert!(
        repair_runtime.await??,
        "the existing periodic reconciliation must be claimed"
    );
    let periodic_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rustodon.durable_jobs WHERE id = $1")
            .bind(periodic_job_id)
            .fetch_one(&owner)
            .await?;
    assert_eq!(
        periodic_rows, 0,
        "successful reconciliation must complete rather than dead-letter the periodic job"
    );
    first_stop_sender
        .send(())
        .expect("first startup runtime is listening");
    other_done_sender
        .send(())
        .expect("second startup runtime is listening");
    runtime_a.await??;
    runtime_b.await??;

    reset().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
async fn malformed_activation_marker_blocks_startup_readiness()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    sqlx::query(
        "INSERT INTO rustodon.outbox_events \
             (kind, logical_key, payload, created_at, dispatched_at) \
         VALUES ('rustodon.mastodon.poll_expiration_activation', 'v1', '{}', \
                 clock_timestamp(), clock_timestamp())",
    )
    .execute(&owner)
    .await?;
    let queue = Queue::new(runtime);
    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let result = run_until_shutdown(
        queue,
        handlers,
        WorkerConfig {
            lanes: [Lane::Maintenance].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 10,
            heartbeat_seconds: 60,
            shutdown_seconds: 2,
        },
        "poll-reconciliation-malformed-activation".to_owned(),
        Some(owner.clone()),
        std::future::pending::<()>(),
    )
    .await;
    assert!(result.is_err(), "malformed activation must fail closed");
    let heartbeats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.heartbeats \
         WHERE process_id LIKE 'poll-reconciliation-malformed-activation:%'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(heartbeats, 0, "failed startup cannot advertise readiness");
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn poll_expiration_activation_converges_and_fixed_boundary_manages_forever()
-> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    let activations = futures_util::future::join_all((0..8).map(|_| {
        let queue = queue.clone();
        async move { queue.ensure_poll_expiration_activation_for_test().await }
    }))
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
    assert!(activations.iter().all(|value| *value == activations[0]));
    let activation = activations[0];
    let activation_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events \
         WHERE kind = 'rustodon.mastodon.poll_expiration_activation' \
           AND logical_key = 'v1' AND dispatched_at = created_at \
           AND payload = jsonb_build_object( \
               'version', 1, 'activated_at_micros', \
               (extract(epoch FROM created_at) * 1000000)::bigint)",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(activation_rows, 1);

    for (poll_id, expires_at) in [
        (9_100_000_000_000_001_i64, activation),
        (
            9_100_000_000_000_002_i64,
            activation - Duration::microseconds(1),
        ),
    ] {
        assert_eq!(
            queue
                .reconcile_poll_expiration_for_test(poll_id, expires_at, expires_at, activation,)
                .await?,
            rustodon::jobs::PollExpirationRepairAction::Completed
        );
    }
    let historical_payloads: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events \
         WHERE kind = $1 AND payload ->> 'outcome' = 'historical_baseline'",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .fetch_one(&owner)
    .await?;
    assert_eq!(historical_payloads, 2, "equality belongs to history");

    let future_poll = 9_100_000_000_000_003_i64;
    let future_expiry = activation + Duration::days(2);
    assert_eq!(
        queue
            .reconcile_poll_expiration_for_test(
                future_poll,
                future_expiry,
                future_expiry,
                activation,
            )
            .await?,
        rustodon::jobs::PollExpirationRepairAction::Create
    );
    let future_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 \
         AND arguments ->> 'poll_id' = $2",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(future_poll.to_string())
    .fetch_one(&owner)
    .await?;
    assert_eq!(future_jobs, 1, "future work beyond one day is managed");

    reset().await?;
    let old_activation = Utc::now() - Duration::days(30);
    sqlx::query(
        "INSERT INTO rustodon.outbox_events \
             (kind, logical_key, payload, created_at, dispatched_at) \
         VALUES ('rustodon.mastodon.poll_expiration_activation', 'v1', \
                 jsonb_build_object('version', 1, 'activated_at_micros', $1::bigint), \
                 $2, $2)",
    )
    .bind(old_activation.timestamp_micros())
    .bind(old_activation)
    .execute(&owner)
    .await?;
    let missed_expiry = old_activation + Duration::days(2);
    assert!(missed_expiry < Utc::now() - Duration::days(1));
    assert_eq!(
        queue
            .reconcile_poll_expiration_for_test(
                9_100_000_000_000_004_i64,
                missed_expiry,
                missed_expiry,
                old_activation,
            )
            .await?,
        rustodon::jobs::PollExpirationRepairAction::Create
    );

    let malformed_poll = 9_100_000_000_000_005_i64;
    let malformed_generation = missed_expiry.timestamp_micros();
    let malformed_arguments: Value = serde_json::from_str(&format!(
        "{{\"poll_id\":{malformed_poll}.0,\"expires_at_micros\":{malformed_generation}.0}}"
    ))?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Core,
                MASTODON_POLL_EXPIRATION_JOB_KIND,
                malformed_arguments,
            )
            .logical_key(format!(
                "poll-expiration:{malformed_poll}:generation:{malformed_generation}:repair"
            )),
        )
        .await?;
    assert_eq!(
        queue
            .reconcile_poll_expiration_for_test(
                malformed_poll,
                missed_expiry,
                missed_expiry,
                old_activation,
            )
            .await?,
        rustodon::jobs::PollExpirationRepairAction::Create
    );
    let repaired_arguments: Value = sqlx::query_scalar(
        "SELECT arguments FROM rustodon.durable_jobs \
         WHERE kind = $1 AND logical_key LIKE $2 \
           AND arguments -> 'poll_id' = to_jsonb($3::bigint)",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(format!(
        "poll-expiration:{malformed_poll}:generation:{malformed_generation}:repair:recovery:%"
    ))
    .bind(malformed_poll)
    .fetch_one(&owner)
    .await?;
    assert_eq!(repaired_arguments["poll_id"], malformed_poll);
    assert_eq!(
        repaired_arguments["expires_at_micros"],
        malformed_generation
    );
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn remote_expiry_replacement_finalizes_due_generation_before_suppression()
-> Result<(), Box<dyn std::error::Error>> {
    const LOCAL_VOTER: i64 = 116_844_606_259_201_001;
    const STATUS_BASE: i64 = 8_000_000_000_096_700;
    const POLL_BASE: i64 = 8_000_000_000_096_800;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    reset().await?;
    let activation = Utc::now() - Duration::days(30);
    sqlx::query(
        "INSERT INTO rustodon.outbox_events \
             (kind, logical_key, payload, created_at, dispatched_at) \
         VALUES ('rustodon.mastodon.poll_expiration_activation', 'v1', \
                 jsonb_build_object('version', 1, 'activated_at_micros', $1::bigint), $2, $2)",
    )
    .bind(activation.timestamp_micros())
    .bind(activation)
    .execute(&owner)
    .await?;
    let (remote_author, actor_uri): (i64, String) =
        sqlx::query_as("SELECT id, uri FROM accounts WHERE domain IS NOT NULL ORDER BY id LIMIT 1")
            .fetch_one(&owner)
            .await?;
    let due_expiry = Utc::now() - Duration::days(1);
    let historical_expiry = activation - Duration::microseconds(1);
    let future_expiry = Utc::now() + Duration::days(1);
    let previous_expiries = [
        Some(due_expiry),
        Some(due_expiry - Duration::hours(1)),
        Some(due_expiry - Duration::hours(2)),
        Some(historical_expiry),
        Some(due_expiry - Duration::hours(3)),
        None,
        Some(due_expiry - Duration::hours(4)),
        Some(future_expiry),
    ];
    for (offset, previous_expiry) in previous_expiries.into_iter().enumerate() {
        let offset = i64::try_from(offset)?;
        let status_id = STATUS_BASE + offset;
        let poll_id = POLL_BASE + offset;
        let status_uri = format!("{actor_uri}/statuses/{status_id}");
        sqlx::query(
            "INSERT INTO statuses (id, account_id, text, spoiler_text, visibility, local, uri, url, \
                 language, sensitive, reply, created_at, updated_at) \
             VALUES ($1, $2, 'remote expiry change', '', 0, false, $3, $3, 'en', false, false, \
                 clock_timestamp(), clock_timestamp())",
        )
        .bind(status_id)
        .bind(remote_author)
        .bind(status_uri)
        .execute(&owner)
        .await?;
        sqlx::query(
            "INSERT INTO polls (id, account_id, status_id, options, cached_tallies, votes_count, \
                 voters_count, multiple, hide_totals, expires_at, created_at, updated_at) \
             VALUES ($1, $2, $3, ARRAY['Yes', 'No'], ARRAY[1, 0]::bigint[], 1, 1, false, \
                 false, $4, clock_timestamp(), clock_timestamp())",
        )
        .bind(poll_id)
        .bind(remote_author)
        .bind(status_id)
        .bind(previous_expiry)
        .execute(&owner)
        .await?;
        sqlx::query("UPDATE statuses SET poll_id = $1 WHERE id = $2")
            .bind(poll_id)
            .bind(status_id)
            .execute(&owner)
            .await?;
        sqlx::query(
            "INSERT INTO poll_votes (id, account_id, choice, poll_id, created_at, updated_at) \
             VALUES ($1, $2, 0, $3, clock_timestamp(), clock_timestamp())",
        )
        .bind(-(POLL_BASE + offset))
        .bind(LOCAL_VOTER)
        .bind(poll_id)
        .execute(&owner)
        .await?;
    }

    let completed_poll = POLL_BASE + 4;
    let completed_expiry = previous_expiries[4].expect("completed generation has an expiry");
    sqlx::query(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload, created_at, dispatched_at) \
         VALUES ($1, $2, jsonb_build_object( \
             'version', 1, 'poll_id', $3::bigint, 'expires_at_micros', $4::bigint, \
             'outcome', 'effects_enqueued'), now(), now())",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .bind(format!(
        "poll-expiration-effect:{completed_poll}:generation:{}",
        completed_expiry.timestamp_micros()
    ))
    .bind(completed_poll)
    .bind(completed_expiry.timestamp_micros())
    .execute(&owner)
    .await?;
    sqlx::query(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload) \
         VALUES ($1, $2, jsonb_build_object('fixture', true))",
    )
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(format!("notification:poll:{LOCAL_VOTER}:{completed_poll}"))
    .execute(&owner)
    .await?;

    let malformed_poll = POLL_BASE + 6;
    let malformed_expiry = previous_expiries[6].expect("malformed generation has an expiry");
    sqlx::query(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload, created_at, dispatched_at) \
         VALUES ($1, $2, jsonb_build_object('version', 0), now(), now())",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .bind(format!(
        "poll-expiration-effect:{malformed_poll}:generation:{}",
        malformed_expiry.timestamp_micros()
    ))
    .execute(&owner)
    .await?;

    let future_poll = POLL_BASE + 7;
    let future_generation = future_expiry.timestamp_micros();
    sqlx::query(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload) \
         VALUES ($1, $2, jsonb_build_object( \
             'lane', 'core', \
             'arguments', jsonb_build_object( \
                 'poll_id', $3::bigint, 'expires_at_micros', $4::bigint), \
             'run_at', $5::text, 'max_attempts', 25))",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(format!(
        "poll-expiration:{future_poll}:generation:{future_generation}:reschedule"
    ))
    .bind(future_poll)
    .bind(future_generation)
    .bind((future_expiry + Duration::minutes(5)).to_rfc3339())
    .execute(&owner)
    .await?;

    let writer = WriteRepository::from_pool(owner.clone());
    let incoming_future = Utc::now() + Duration::days(2);
    let incoming_past = Utc::now() - Duration::hours(12);
    let later_future = future_expiry + Duration::days(3);
    let replacements = [
        (0_i64, Some(incoming_future)),
        (1, Some(incoming_past)),
        (2, None),
        (3, Some(incoming_future + Duration::hours(1))),
        (4, Some(incoming_future + Duration::hours(2))),
        (5, Some(incoming_future + Duration::hours(3))),
        (7, Some(later_future)),
    ];
    let question = |status_id: i64, expires_at: Option<DateTime<Utc>>, tallies: [i64; 2]| {
        let mut object = json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": format!("{actor_uri}/statuses/{status_id}"),
            "type": "Question",
            "attributedTo": &actor_uri,
            "content": "remote expiry change",
            "published": Utc::now().to_rfc3339(),
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [],
            "oneOf": [
                {"type": "Note", "name": "Yes", "replies": {"totalItems": tallies[0]}},
                {"type": "Note", "name": "No", "replies": {"totalItems": tallies[1]}}
            ]
        });
        if let Some(expires_at) = expires_at {
            object["endTime"] = json!(expires_at.to_rfc3339());
        }
        object
    };
    for (offset, replacement) in replacements {
        let poll_id = POLL_BASE + offset;
        let lock_version: i32 = sqlx::query_scalar("SELECT lock_version FROM polls WHERE id = $1")
            .bind(poll_id)
            .fetch_one(&owner)
            .await?;
        writer
            .apply_signed_remote_poll_refresh_for_test(
                remote_author,
                &actor_uri,
                &question(STATUS_BASE + offset, replacement, [1, 0]),
                ORIGIN,
                poll_id,
                lock_version,
            )
            .await?;
    }
    let malformed_lock_version: i32 =
        sqlx::query_scalar("SELECT lock_version FROM polls WHERE id = $1")
            .bind(malformed_poll)
            .fetch_one(&owner)
            .await?;
    assert!(
        writer
            .apply_signed_remote_poll_refresh_for_test(
                remote_author,
                &actor_uri,
                &question(
                    STATUS_BASE + 6,
                    Some(incoming_future + Duration::hours(4)),
                    [9, 0],
                ),
                ORIGIN,
                malformed_poll,
                malformed_lock_version,
            )
            .await
            .is_err(),
        "a malformed exact prior marker must roll back the replacement"
    );
    let rolled_back: (DateTime<Utc>, Vec<i64>) = sqlx::query_as(
        "SELECT expires_at AT TIME ZONE 'UTC', cached_tallies FROM polls WHERE id = $1",
    )
    .bind(malformed_poll)
    .fetch_one(&owner)
    .await?;
    // PostgreSQL keeps microseconds; the Rust-side expiry may carry nanoseconds.
    assert_eq!(
        (rolled_back.0.timestamp_micros(), rolled_back.1),
        (malformed_expiry.timestamp_micros(), vec![1, 0])
    );

    for offset in 0_i64..3 {
        let poll_id = POLL_BASE + offset;
        let previous_expiry =
            previous_expiries[usize::try_from(offset)?].expect("due generation has an expiry");
        let prior_outcome: String = sqlx::query_scalar(
            "SELECT payload ->> 'outcome' FROM rustodon.outbox_events \
             WHERE kind = $1 AND logical_key = $2",
        )
        .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
        .bind(format!(
            "poll-expiration-effect:{poll_id}:generation:{}",
            previous_expiry.timestamp_micros()
        ))
        .fetch_one(&owner)
        .await?;
        assert_eq!(prior_outcome, "effects_enqueued");
        let notifications: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2",
        )
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(format!("notification:poll:{LOCAL_VOTER}:{poll_id}"))
        .fetch_one(&owner)
        .await?;
        assert_eq!(notifications, 1, "the exact prior effects are emitted once");
    }
    let historical_poll = POLL_BASE + 3;
    let historical_outcome: String = sqlx::query_scalar(
        "SELECT payload ->> 'outcome' FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .bind(format!(
        "poll-expiration-effect:{historical_poll}:generation:{}",
        historical_expiry.timestamp_micros()
    ))
    .fetch_one(&owner)
    .await?;
    assert_eq!(historical_outcome, "historical_baseline");
    let historical_notifications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2",
    )
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(format!("notification:poll:{LOCAL_VOTER}:{historical_poll}"))
    .fetch_one(&owner)
    .await?;
    assert_eq!(historical_notifications, 0, "history emits no effects");
    let completed_notifications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2",
    )
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(format!("notification:poll:{LOCAL_VOTER}:{completed_poll}"))
    .fetch_one(&owner)
    .await?;
    assert_eq!(completed_notifications, 1, "completion is not duplicated");

    for (offset, incoming) in [
        (0_i64, incoming_future),
        (1, incoming_past),
        (3, incoming_future + Duration::hours(1)),
        (4, incoming_future + Duration::hours(2)),
    ] {
        let poll_id = POLL_BASE + offset;
        let incoming_outcome: String = sqlx::query_scalar(
            "SELECT payload ->> 'outcome' FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2",
        )
        .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
        .bind(format!(
            "poll-expiration-effect:{poll_id}:generation:{}",
            incoming.timestamp_micros()
        ))
        .fetch_one(&owner)
        .await?;
        assert_eq!(incoming_outcome, "remote_past_expiry_suppressed");
        let incoming_jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1 \
             AND payload -> 'arguments' ->> 'poll_id' = $2 \
             AND payload -> 'arguments' ->> 'expires_at_micros' = $3",
        )
        .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
        .bind(poll_id.to_string())
        .bind(incoming.timestamp_micros().to_string())
        .fetch_one(&owner)
        .await?;
        assert_eq!(
            incoming_jobs, 0,
            "suppressed replacements are not scheduled"
        );
    }
    let removed_expiry: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT expires_at AT TIME ZONE 'UTC' FROM polls WHERE id = $1")
            .bind(POLL_BASE + 2)
            .fetch_one(&owner)
            .await?;
    assert_eq!(removed_expiry, None);
    let ordinary_poll = POLL_BASE + 5;
    let ordinary_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1 \
         AND payload -> 'arguments' ->> 'poll_id' = $2 \
         AND payload -> 'arguments' ->> 'expires_at_micros' = $3",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(ordinary_poll.to_string())
    .bind(
        (incoming_future + Duration::hours(3))
            .timestamp_micros()
            .to_string(),
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        ordinary_jobs, 1,
        "None to future still schedules expiration"
    );

    let later_generation = later_future.timestamp_micros();
    let later_key =
        format!("poll-expiration:{future_poll}:generation:{later_generation}:reschedule");
    let later_schedule: (String, String) = sqlx::query_as(
        "SELECT logical_key, payload ->> 'run_at' FROM rustodon.outbox_events \
         WHERE kind = $1 AND logical_key = $2",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(&later_key)
    .fetch_one(&owner)
    .await?;
    assert_eq!(later_schedule.0, later_key);
    assert_eq!(
        later_schedule.1,
        (later_future + Duration::minutes(5)).to_rfc3339(),
        "future-to-later refresh uses Mastodon's five-minute remote expiry delay"
    );
    let future_generation_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1 \
         AND payload -> 'arguments' ->> 'poll_id' = $2",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(future_poll.to_string())
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        future_generation_jobs, 2,
        "the harmless old exact generation may coexist with the newly scheduled generation"
    );

    sqlx::query("UPDATE statuses SET poll_id = NULL WHERE id BETWEEN $1 AND $2")
        .bind(STATUS_BASE)
        .bind(STATUS_BASE + 7)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM poll_votes WHERE poll_id BETWEEN $1 AND $2")
        .bind(POLL_BASE)
        .bind(POLL_BASE + 7)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM polls WHERE id BETWEEN $1 AND $2")
        .bind(POLL_BASE)
        .bind(POLL_BASE + 7)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id BETWEEN $1 AND $2")
        .bind(STATUS_BASE)
        .bind(STATUS_BASE + 7)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id BETWEEN $1 AND $2")
        .bind(STATUS_BASE)
        .bind(STATUS_BASE + 7)
        .execute(&owner)
        .await?;
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn historical_local_and_remote_polls_baseline_without_effects_and_dismissed_managed_stays_dismissed()
-> Result<(), Box<dyn std::error::Error>> {
    const LOCAL_AUTHOR: i64 = 116_844_606_259_201_001;
    const STATUS_BASE: i64 = 8_000_000_000_097_100;
    const POLL_BASE: i64 = 8_000_000_000_097_200;
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    let activation = queue.ensure_poll_expiration_activation_for_test().await?;
    let remote_author: i64 =
        sqlx::query_scalar("SELECT id FROM accounts WHERE domain IS NOT NULL ORDER BY id LIMIT 1")
            .fetch_one(&owner)
            .await?;
    for (offset, account_id, local, expires_at) in [
        (0_i64, LOCAL_AUTHOR, true, activation),
        (
            1_i64,
            remote_author,
            false,
            activation - Duration::microseconds(1),
        ),
    ] {
        let status_id = STATUS_BASE + offset;
        let poll_id = POLL_BASE + offset;
        sqlx::query(
            "INSERT INTO statuses (id, account_id, text, spoiler_text, visibility, local, \
                 language, sensitive, reply, created_at, updated_at) \
             VALUES ($1, $2, 'historical expiry fixture', '', 0, $3, 'en', false, false, \
                 clock_timestamp(), clock_timestamp())",
        )
        .bind(status_id)
        .bind(account_id)
        .bind(local)
        .execute(&owner)
        .await?;
        sqlx::query(
            "INSERT INTO polls (id, account_id, status_id, options, cached_tallies, votes_count, \
                 voters_count, multiple, hide_totals, expires_at, created_at, updated_at) \
             VALUES ($1, $2, $3, ARRAY['Yes', 'No'], ARRAY[1, 0]::bigint[], 1, 1, false, \
                 false, $4, clock_timestamp(), clock_timestamp())",
        )
        .bind(poll_id)
        .bind(account_id)
        .bind(status_id)
        .bind(expires_at)
        .execute(&owner)
        .await?;
        sqlx::query("UPDATE statuses SET poll_id = $1 WHERE id = $2")
            .bind(poll_id)
            .bind(status_id)
            .execute(&owner)
            .await?;
        if !local {
            sqlx::query(
                "INSERT INTO poll_votes (id, account_id, choice, poll_id, created_at, updated_at) \
                 VALUES ($1, $2, 0, $3, clock_timestamp(), clock_timestamp())",
            )
            .bind(-(POLL_BASE + offset))
            .bind(LOCAL_AUTHOR)
            .bind(poll_id)
            .execute(&owner)
            .await?;
        }
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Core,
                    MASTODON_POLL_EXPIRATION_JOB_KIND,
                    json!({
                        "poll_id": poll_id,
                        "expires_at_micros": expires_at.timestamp_micros(),
                    }),
                )
                .logical_key(format!(
                    "poll-expiration:{poll_id}:generation:{}:repair",
                    expires_at.timestamp_micros()
                )),
            )
            .await?;
    }
    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    for _ in 0..2 {
        assert!(
            executor
                .process_one("historical-expiry", &[Lane::Core], Duration::seconds(30))
                .await?
        );
    }
    let baseline_markers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1 \
         AND payload ->> 'outcome' = 'historical_baseline' \
         AND (payload ->> 'poll_id')::bigint BETWEEN $2 AND $3",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .bind(POLL_BASE)
    .bind(POLL_BASE + 1)
    .fetch_one(&owner)
    .await?;
    assert_eq!(baseline_markers, 2);
    let historical_effects: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events WHERE \
            (kind = $1 AND payload -> 'arguments' ->> 'activity_id' IN ($2, $3)) OR \
            (kind = $4 AND payload -> 'arguments' ->> 'status_id' IN ($5, $6))",
    )
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(POLL_BASE.to_string())
    .bind((POLL_BASE + 1).to_string())
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(STATUS_BASE.to_string())
    .bind((STATUS_BASE + 1).to_string())
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        historical_effects, 0,
        "history emits no notification or federation work"
    );

    let managed_status = STATUS_BASE + 2;
    let managed_poll = POLL_BASE + 2;
    reset().await?;
    let old_activation = activation - Duration::days(30);
    sqlx::query(
        "INSERT INTO rustodon.outbox_events \
             (kind, logical_key, payload, created_at, dispatched_at) \
         VALUES ('rustodon.mastodon.poll_expiration_activation', 'v1', \
                 jsonb_build_object('version', 1, 'activated_at_micros', $1::bigint), $2, $2)",
    )
    .bind(old_activation.timestamp_micros())
    .bind(old_activation)
    .execute(&owner)
    .await?;
    let managed_expiry = old_activation + Duration::days(2);
    sqlx::query(
        "INSERT INTO statuses (id, account_id, text, spoiler_text, visibility, local, language, \
             sensitive, reply, created_at, updated_at) \
         VALUES ($1, $2, 'managed outage fixture', '', 0, true, 'en', false, false, \
             clock_timestamp(), clock_timestamp())",
    )
    .bind(managed_status)
    .bind(LOCAL_AUTHOR)
    .execute(&owner)
    .await?;
    sqlx::query(
        "INSERT INTO polls (id, account_id, status_id, options, cached_tallies, votes_count, \
             voters_count, multiple, hide_totals, expires_at, created_at, updated_at) \
         VALUES ($1, $2, $3, ARRAY['Yes', 'No'], ARRAY[0, 0]::bigint[], 0, 0, false, false, \
             $4, clock_timestamp(), clock_timestamp())",
    )
    .bind(managed_poll)
    .bind(LOCAL_AUTHOR)
    .bind(managed_status)
    .bind(managed_expiry)
    .execute(&owner)
    .await?;
    sqlx::query("UPDATE statuses SET poll_id = $1 WHERE id = $2")
        .bind(managed_poll)
        .bind(managed_status)
        .execute(&owner)
        .await?;
    for suffix in ["first", "replay"] {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Core,
                    MASTODON_POLL_EXPIRATION_JOB_KIND,
                    json!({
                        "poll_id": managed_poll,
                        "expires_at_micros": managed_expiry.timestamp_micros(),
                    }),
                )
                .logical_key(format!("managed-expiry:{managed_poll}:{suffix}")),
            )
            .await?;
        assert!(
            executor
                .process_one("managed-expiry", &[Lane::Core], Duration::seconds(30))
                .await?
        );
        if suffix == "first" {
            queue.dispatch_outbox(100).await?;
            assert!(
                executor
                    .process_one(
                        "managed-expiry-notification",
                        &[Lane::Core],
                        Duration::seconds(30),
                    )
                    .await?
            );
            let delivered: i64 =
                sqlx::query_scalar("SELECT count(*) FROM notifications WHERE activity_id = $1")
                    .bind(managed_poll)
                    .fetch_one(&owner)
                    .await?;
            assert_eq!(
                delivered, 1,
                "managed expiration creates the notification once"
            );
            sqlx::query("DELETE FROM notifications WHERE activity_id = $1")
                .bind(managed_poll)
                .execute(&owner)
                .await?;
        }
    }
    let managed_notification_intents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1 \
         AND payload -> 'arguments' ->> 'activity_id' = $2",
    )
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(managed_poll.to_string())
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        managed_notification_intents, 1,
        "replay cannot recreate dismissed work"
    );
    let recreated: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE activity_id = $1")
            .bind(managed_poll)
            .fetch_one(&owner)
            .await?;
    assert_eq!(recreated, 0, "dismissed notification stays dismissed");

    sqlx::query("UPDATE statuses SET poll_id = NULL WHERE id BETWEEN $1 AND $2")
        .bind(STATUS_BASE)
        .bind(managed_status)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM poll_votes WHERE poll_id BETWEEN $1 AND $2")
        .bind(POLL_BASE)
        .bind(managed_poll)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM polls WHERE id BETWEEN $1 AND $2")
        .bind(POLL_BASE)
        .bind(managed_poll)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id BETWEEN $1 AND $2")
        .bind(STATUS_BASE)
        .bind(managed_status)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id BETWEEN $1 AND $2")
        .bind(STATUS_BASE)
        .bind(managed_status)
        .execute(&owner)
        .await?;
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn bounded_startup_persists_continuation_and_core_baselines_remaining_history()
-> Result<(), Box<dyn std::error::Error>> {
    const LOCAL_AUTHOR: i64 = 116_844_606_259_201_001;
    const STATUS_BASE: i64 = 8_000_000_000_096_000;
    const POLL_BASE: i64 = 8_000_000_000_096_100;
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    let expires_at = Utc::now() - Duration::days(1);
    for offset in 0_i64..26 {
        let status_id = STATUS_BASE + offset;
        let poll_id = POLL_BASE + offset;
        sqlx::query(
            "INSERT INTO statuses (id, account_id, text, spoiler_text, visibility, local, \
                 language, sensitive, reply, created_at, updated_at) \
             VALUES ($1, $2, 'bounded startup history', '', 0, true, 'en', false, false, \
                 clock_timestamp(), clock_timestamp())",
        )
        .bind(status_id)
        .bind(LOCAL_AUTHOR)
        .execute(&owner)
        .await?;
        sqlx::query(
            "INSERT INTO polls (id, account_id, status_id, options, cached_tallies, votes_count, \
                 voters_count, multiple, hide_totals, expires_at, created_at, updated_at) \
             VALUES ($1, $2, $3, ARRAY['Yes', 'No'], ARRAY[0, 0]::bigint[], 0, 0, false, \
                 false, $4, clock_timestamp(), clock_timestamp())",
        )
        .bind(poll_id)
        .bind(LOCAL_AUTHOR)
        .bind(status_id)
        .bind(expires_at)
        .execute(&owner)
        .await?;
        sqlx::query("UPDATE statuses SET poll_id = $1 WHERE id = $2")
            .bind(poll_id)
            .bind(status_id)
            .execute(&owner)
            .await?;
    }
    let scan_started_at = Utc::now();
    let initial_arguments = json!({
        "scan_started_at": scan_started_at.to_rfc3339(),
        "through_poll_id": POLL_BASE + 25,
        "after_poll_id": POLL_BASE - 1,
        "segment": 0,
    });
    let initial_logical_key = format!(
        "poll-expiration-reconcile:{}:0:{}",
        scan_started_at.timestamp_micros(),
        POLL_BASE - 1
    );
    let initial_job_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Maintenance,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                initial_arguments.clone(),
            )
            .logical_key(&initial_logical_key)
            .run_at(Utc::now() + Duration::days(7)),
        )
        .await?;

    reconcile_poll_expirations_at_startup_for_test(
        queue.clone(),
        owner.clone(),
        "bounded-startup-segment",
    )
    .await?;
    let continuation: (i64, i64, i64) = sqlx::query_as(
        "SELECT (arguments ->> 'segment')::bigint, \
                (arguments ->> 'after_poll_id')::bigint, \
                (arguments ->> 'through_poll_id')::bigint \
         FROM rustodon.durable_jobs WHERE kind = $1 AND lane = 'maintenance' AND dead_at IS NULL",
    )
    .bind(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)
    .fetch_one(&owner)
    .await?;
    assert_eq!(continuation, (1, POLL_BASE + 24, POLL_BASE + 25));
    let completion: Value = sqlx::query_scalar(
        "SELECT result FROM rustodon.idempotency_keys \
         WHERE scope = 'rustodon.mastodon.poll_expiration_reconcile_success' \
           AND result ->> 'job_id' = $1",
    )
    .bind(initial_job_id.to_string())
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        completion,
        json!({
            "completed": true,
            "job_id": initial_job_id,
            "logical_key": initial_logical_key,
            "arguments": initial_arguments,
        }),
        "cursor progress publishes success for exactly the completed segment"
    );
    let baseline_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1 \
           AND payload ->> 'outcome' = 'historical_baseline' \
           AND (payload ->> 'poll_id')::bigint BETWEEN $2 AND $3",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .bind(POLL_BASE)
    .bind(POLL_BASE + 25)
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        baseline_count, 25,
        "startup executes only one bounded segment"
    );

    let final_poll = POLL_BASE + 25;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Core,
                MASTODON_POLL_EXPIRATION_JOB_KIND,
                json!({
                    "poll_id": final_poll,
                    "expires_at_micros": expires_at.timestamp_micros(),
                }),
            )
            .logical_key(format!(
                "poll-expiration:{final_poll}:generation:{}:fixture",
                expires_at.timestamp_micros()
            )),
        )
        .await?;
    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let core = tokio::spawn(run_until_shutdown(
        queue.clone(),
        handlers,
        WorkerConfig {
            lanes: [Lane::Core].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 10,
            heartbeat_seconds: 60,
            shutdown_seconds: 2,
        },
        "core-only-historical-expiration".to_owned(),
        Some(owner.clone()),
        async move {
            let _ = shutdown_receiver.await;
        },
    ));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let baselined: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM rustodon.outbox_events WHERE kind = $1 \
                   AND logical_key = $2 AND payload ->> 'outcome' = 'historical_baseline')",
            )
            .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
            .bind(format!(
                "poll-expiration-effect:{final_poll}:generation:{}",
                expires_at.timestamp_micros()
            ))
            .fetch_one(&owner)
            .await?;
            if baselined {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    shutdown_sender.send(()).expect("Core worker is listening");
    core.await??;

    let emitted_effects: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events WHERE \
            (kind = $1 AND payload -> 'arguments' ->> 'activity_id' IN \
                (SELECT generate_series($2::bigint, $3::bigint)::text)) OR \
            (kind = $4 AND payload -> 'arguments' ->> 'status_id' IN \
                (SELECT generate_series($5::bigint, $6::bigint)::text))",
    )
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(POLL_BASE)
    .bind(POLL_BASE + 25)
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(STATUS_BASE)
    .bind(STATUS_BASE + 25)
    .fetch_one(&owner)
    .await?;
    assert_eq!(emitted_effects, 0, "historical handlers emit no effects");
    let continuation_persisted: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rustodon.durable_jobs \
         WHERE kind = $1 AND lane = 'maintenance' AND dead_at IS NULL)",
    )
    .bind(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)
    .fetch_one(&owner)
    .await?;
    assert!(
        continuation_persisted,
        "Core-only startup does not drain maintenance history"
    );

    sqlx::query("UPDATE statuses SET poll_id = NULL WHERE id BETWEEN $1 AND $2")
        .bind(STATUS_BASE)
        .bind(STATUS_BASE + 25)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM polls WHERE id BETWEEN $1 AND $2")
        .bind(POLL_BASE)
        .bind(POLL_BASE + 25)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id BETWEEN $1 AND $2")
        .bind(STATUS_BASE)
        .bind(STATUS_BASE + 25)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id BETWEEN $1 AND $2")
        .bind(STATUS_BASE)
        .bind(STATUS_BASE + 25)
        .execute(&owner)
        .await?;
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires the disposable restored Mastodon worker fixture"]
#[allow(clippy::too_many_lines)]
async fn poll_expiration_reconciliation_repairs_exact_generations_and_preserves_audit_rows()
-> Result<(), Box<dyn std::error::Error>> {
    const LOCAL_AUTHOR: i64 = 116_844_606_259_201_001;
    const STATUS_BASE: i64 = 8_000_000_000_098_500;
    const POLL_BASE: i64 = 8_000_000_000_098_600;

    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    let activation = queue.ensure_poll_expiration_activation_for_test().await?;
    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;

    for offset in 0_i64..40 {
        let status_id = STATUS_BASE - offset;
        let poll_id = POLL_BASE - offset;
        sqlx::query(
            "INSERT INTO statuses (id, account_id, text, spoiler_text, visibility, local, \
                 language, sensitive, reply, created_at, updated_at) \
             VALUES ($1, $2, 'reconciliation fixture', '', 0, true, 'en', false, false, \
                 clock_timestamp(), clock_timestamp())",
        )
        .bind(status_id)
        .bind(LOCAL_AUTHOR)
        .execute(&owner)
        .await?;
        sqlx::query(
            "INSERT INTO polls (id, account_id, status_id, options, cached_tallies, votes_count, \
                 voters_count, multiple, hide_totals, expires_at, created_at, updated_at) \
             VALUES ($1, $2, $3, ARRAY['Yes', 'No'], ARRAY[0, 0]::bigint[], 0, 0, false, \
                 false, $4 + interval '2 days', clock_timestamp(), clock_timestamp())",
        )
        .bind(poll_id)
        .bind(LOCAL_AUTHOR)
        .bind(status_id)
        .bind(activation)
        .execute(&owner)
        .await?;
        sqlx::query("UPDATE statuses SET poll_id = $1 WHERE id = $2")
            .bind(poll_id)
            .bind(status_id)
            .execute(&owner)
            .await?;
    }
    let expirations = sqlx::query_as::<_, (i64, DateTime<Utc>)>(
        "SELECT id, expires_at AT TIME ZONE 'UTC' FROM polls WHERE id BETWEEN $1 AND $2",
    )
    .bind(POLL_BASE - 39)
    .bind(POLL_BASE)
    .fetch_all(&owner)
    .await?
    .into_iter()
    .collect::<std::collections::HashMap<_, _>>();
    let expiration = |poll_id: i64| expirations[&poll_id];
    let generation = |poll_id: i64| expiration(poll_id).timestamp_micros();

    let terminal_first = POLL_BASE - 39;
    let terminal_last = POLL_BASE - 7;
    for poll_id in terminal_first..=terminal_last {
        let outcome = match poll_id.rem_euclid(3) {
            0 => "historical_baseline",
            1 => "effects_enqueued",
            _ => "remote_past_expiry_suppressed",
        };
        sqlx::query(
            "INSERT INTO rustodon.outbox_events (kind, logical_key, payload, created_at, dispatched_at) \
             VALUES ($1, $2, jsonb_build_object( \
                'version', 1, 'poll_id', $3::bigint, 'expires_at_micros', $4::bigint, \
                'outcome', $5::text), now(), now())",
        )
        .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
        .bind(format!(
            "poll-expiration-effect:{poll_id}:generation:{}",
            generation(poll_id)
        ))
        .bind(poll_id)
        .bind(generation(poll_id))
        .bind(outcome)
        .execute(&owner)
        .await?;
    }
    for pass in 0_i64..2 {
        let terminal_scan_started_at = Utc::now() + Duration::microseconds(pass);
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Maintenance,
                    MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                    json!({
                        "scan_started_at": terminal_scan_started_at.to_rfc3339(),
                        "through_poll_id": terminal_last,
                        "after_poll_id": terminal_first - 1,
                        "segment": 0,
                    }),
                )
                .logical_key(format!(
                    "poll-expiration-reconcile:{}:0:{}",
                    terminal_scan_started_at.timestamp_micros(),
                    terminal_first - 1
                )),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "terminal-poll-reconciliation-fixture",
                    &[Lane::Maintenance],
                    Duration::seconds(30),
                )
                .await?,
            "the requested terminal-only scan runs"
        );
        assert!(
            !executor
                .process_one(
                    "terminal-poll-reconciliation-fixture",
                    &[Lane::Maintenance],
                    Duration::seconds(30),
                )
                .await?,
            "more than 25 terminal rows create no continuation"
        );
    }
    let terminal_markers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1 \
         AND (payload ->> 'poll_id')::bigint BETWEEN $2 AND $3",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .bind(terminal_first)
    .bind(terminal_last)
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        terminal_markers, 33,
        "periodic scans do not churn terminal history"
    );

    let missing_poll = POLL_BASE;
    let old_generation = generation(missing_poll) - 1;
    sqlx::query(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload, created_at, dispatched_at) \
         VALUES ($1, $2, jsonb_build_object( \
            'version', 1, 'poll_id', $3::bigint, 'expires_at_micros', $4::bigint, \
            'outcome', 'effects_enqueued'), now(), now())",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .bind(format!(
        "poll-expiration-effect:{missing_poll}:generation:{old_generation}"
    ))
    .bind(missing_poll)
    .bind(old_generation)
    .execute(&owner)
    .await?;

    let dead_poll = POLL_BASE - 1;
    let dead_key = format!(
        "poll-expiration:{dead_poll}:generation:{}:initial",
        generation(dead_poll)
    );
    let dead_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Core,
                MASTODON_POLL_EXPIRATION_JOB_KIND,
                json!({"poll_id": dead_poll, "expires_at_micros": generation(dead_poll)}),
            )
            .logical_key(&dead_key),
        )
        .await?;
    sqlx::query("UPDATE rustodon.durable_jobs SET dead_at = clock_timestamp() WHERE id = $1")
        .bind(dead_id)
        .execute(&owner)
        .await?;

    let healthy_poll = POLL_BASE - 2;
    let healthy_key = format!(
        "poll-expiration:{healthy_poll}:generation:{}:initial",
        generation(healthy_poll)
    );
    let healthy_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Core,
                MASTODON_POLL_EXPIRATION_JOB_KIND,
                json!({"poll_id": healthy_poll, "expires_at_micros": generation(healthy_poll)}),
            )
            .logical_key(&healthy_key)
            .run_at(expiration(healthy_poll)),
        )
        .await?;

    let late_poll = POLL_BASE - 3;
    let late_key = format!(
        "poll-expiration:{late_poll}:generation:{}:initial",
        generation(late_poll)
    );
    let late_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Core,
                MASTODON_POLL_EXPIRATION_JOB_KIND,
                json!({"poll_id": late_poll, "expires_at_micros": generation(late_poll)}),
            )
            .logical_key(&late_key)
            .run_at(Utc::now() + Duration::days(7)),
        )
        .await?;

    let completed_poll = POLL_BASE - 4;
    sqlx::query(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload, created_at, dispatched_at) \
         VALUES ($1, $2, $3, now(), now())",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .bind(format!(
        "poll-expiration-effect:{completed_poll}:generation:{}",
        generation(completed_poll)
    ))
    .bind(json!({
        "version": 1,
        "poll_id": completed_poll,
        "expires_at_micros": generation(completed_poll),
        "outcome": "effects_enqueued",
    }))
    .execute(&owner)
    .await?;

    let rescheduled_poll = POLL_BASE - 5;
    let rescheduled_key = format!(
        "poll-expiration:{rescheduled_poll}:generation:{}:reschedule",
        generation(rescheduled_poll)
    );
    let rescheduled_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Core,
                MASTODON_POLL_EXPIRATION_JOB_KIND,
                json!({
                    "poll_id": rescheduled_poll,
                    "expires_at_micros": generation(rescheduled_poll)
                }),
            )
            .logical_key(&rescheduled_key)
            .run_at(Utc::now() + Duration::days(7)),
        )
        .await?;

    let pending_rescheduled_poll = POLL_BASE - 6;
    let pending_rescheduled_key = format!(
        "poll-expiration:{pending_rescheduled_poll}:generation:{}:reschedule",
        generation(pending_rescheduled_poll)
    );
    sqlx::query(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload) \
         VALUES ($1, $2, jsonb_build_object( \
             'lane', 'core', 'arguments', jsonb_build_object( \
                 'poll_id', $3::bigint, 'expires_at_micros', $4::bigint), \
             'run_at', to_jsonb((clock_timestamp() + interval '7 days')::text)))",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(&pending_rescheduled_key)
    .bind(pending_rescheduled_poll)
    .bind(generation(pending_rescheduled_poll))
    .execute(&owner)
    .await?;
    let pending_rescheduled_id: i64 = sqlx::query_scalar(
        "SELECT id FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(&pending_rescheduled_key)
    .fetch_one(&owner)
    .await?;

    let fallback_writer = PgPoolOptions::new()
        .max_connections(1)
        .connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?)
        .await?;

    let fallback_scan_started_at = Utc::now();
    let fallback_started = tokio::time::Instant::now();
    reconcile_poll_expirations_with_primary_timeout_for_test(
        queue.clone(),
        fallback_writer.clone(),
        json!({
            "scan_started_at": fallback_scan_started_at.to_rfc3339(),
            "through_poll_id": POLL_BASE,
            "after_poll_id": POLL_BASE - 40,
            "segment": 0,
        }),
        1,
    )
    .await?;
    assert!(
        fallback_started.elapsed() < std::time::Duration::from_millis(2_250),
        "the timed-out optimized scan retains the hard wall bound"
    );
    let fallback_continuations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 \
         AND arguments ->> 'scan_started_at' = $2 AND arguments ->> 'segment' = '1'",
    )
    .bind(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)
    .bind(fallback_scan_started_at.to_rfc3339())
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        fallback_continuations, 0,
        "a short fallback page advances through the terminal prefix and reaches damaged work"
    );
    for poll_id in [missing_poll, dead_poll] {
        let repaired: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 AND dead_at IS NULL \
             AND logical_key = $2",
        )
        .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
        .bind(format!(
            "poll-expiration:{poll_id}:generation:{}:repair",
            generation(poll_id)
        ))
        .fetch_one(&owner)
        .await?;
        assert_eq!(
            repaired, 1,
            "fallback skips terminal rows without spending the actionable budget"
        );
    }
    fallback_writer.close().await;

    let scan_started_at = Utc::now();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Maintenance,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                json!({
                    "scan_started_at": scan_started_at.to_rfc3339(),
                    "after_poll_id": POLL_BASE - 40,
                    "segment": 0
                }),
            )
            .logical_key(format!(
                "poll-expiration-reconcile:{}:0:{}",
                scan_started_at.timestamp_micros(),
                POLL_BASE - 40
            )),
        )
        .await?;
    let mut reconciliation_passes = 0;
    while executor
        .process_one(
            "poll-reconciliation-fixture",
            &[Lane::Maintenance],
            Duration::seconds(30),
        )
        .await?
    {
        reconciliation_passes += 1;
        assert!(
            reconciliation_passes <= 4,
            "reconciliation did not converge"
        );
    }
    assert_eq!(
        reconciliation_passes, 1,
        "terminal history is skipped before the actionable candidate bound"
    );

    for poll_id in [missing_poll, dead_poll] {
        let repairs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 AND dead_at IS NULL \
             AND logical_key = $2",
        )
        .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
        .bind(format!(
            "poll-expiration:{poll_id}:generation:{}:repair",
            generation(poll_id)
        ))
        .fetch_one(&owner)
        .await?;
        assert_eq!(repairs, 1, "missing exact generation must be repaired");
    }
    let dead_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 AND dead_at IS NOT NULL \
         AND logical_key = $2",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(&dead_key)
    .fetch_one(&owner)
    .await?;
    assert_eq!(dead_rows, 1, "dead audit history must remain untouched");
    let (healthy_after_id, healthy_after_run_at): (i64, DateTime<Utc>) = sqlx::query_as(
        "SELECT id, run_at FROM rustodon.durable_jobs \
         WHERE kind = $1 AND logical_key = $2 AND dead_at IS NULL",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(&healthy_key)
    .fetch_one(&owner)
    .await?;
    assert_eq!(healthy_after_id, healthy_id);
    assert_eq!(healthy_after_run_at, expiration(healthy_poll));
    let (rescheduled_after_id, rescheduled_after_run_at): (i64, DateTime<Utc>) = sqlx::query_as(
        "SELECT id, run_at FROM rustodon.durable_jobs \
             WHERE kind = $1 AND logical_key = $2 AND dead_at IS NULL",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(&rescheduled_key)
    .fetch_one(&owner)
    .await?;
    assert_eq!(rescheduled_after_id, rescheduled_id);
    assert_eq!(
        rescheduled_after_run_at,
        expiration(rescheduled_poll) + Duration::minutes(5)
    );
    let rescheduled_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.durable_jobs \
         WHERE kind = $1 AND logical_key = $2 AND dead_at IS NULL",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(&rescheduled_key)
    .fetch_one(&owner)
    .await?;
    assert_eq!(rescheduled_count, 1);
    let pending_after: (i64, DateTime<Utc>) = sqlx::query_as(
        "SELECT id, (payload ->> 'run_at')::timestamptz FROM rustodon.outbox_events \
         WHERE kind = $1 AND logical_key = $2 AND dispatched_at IS NULL",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(&pending_rescheduled_key)
    .fetch_one(&owner)
    .await?;
    assert_eq!(pending_after.0, pending_rescheduled_id);
    assert_eq!(
        pending_after.1,
        expiration(pending_rescheduled_poll) + Duration::minutes(5)
    );
    let pending_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events \
         WHERE kind = $1 AND logical_key = $2 AND dispatched_at IS NULL",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(&pending_rescheduled_key)
    .fetch_one(&owner)
    .await?;
    assert_eq!(pending_count, 1);
    for poll_id in [rescheduled_poll, pending_rescheduled_poll] {
        let duplicate_repairs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rustodon.durable_jobs \
             WHERE kind = $1 AND logical_key = $2 AND dead_at IS NULL",
        )
        .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
        .bind(format!(
            "poll-expiration:{poll_id}:generation:{}:repair",
            generation(poll_id)
        ))
        .fetch_one(&owner)
        .await?;
        assert_eq!(duplicate_repairs, 0);
    }
    let healthy_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.durable_jobs \
         WHERE kind = $1 AND logical_key = $2 AND dead_at IS NULL",
    )
    .bind(MASTODON_POLL_EXPIRATION_JOB_KIND)
    .bind(&healthy_key)
    .fetch_one(&owner)
    .await?;
    assert_eq!(healthy_count, 1);
    let late_after: (i64, DateTime<Utc>) = sqlx::query_as(
        "SELECT id, run_at FROM rustodon.durable_jobs \
         WHERE logical_key = $1 AND dead_at IS NULL",
    )
    .bind(&late_key)
    .fetch_one(&owner)
    .await?;
    assert_eq!(late_after.0, late_id);
    let repaired_late = late_after.1;
    // The late job (scheduled a week out) is pulled forward to its poll's expiry.
    assert_eq!(
        repaired_late.timestamp_micros(),
        generation(late_poll),
        "late job run_at {repaired_late} (scan start {scan_started_at})"
    );
    let completed_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.durable_jobs WHERE arguments ->> 'poll_id' = $1 \
         AND dead_at IS NULL",
    )
    .bind(completed_poll.to_string())
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        completed_jobs, 0,
        "effect marker proves exact generation completion"
    );

    // The same bounded pass is used synchronously at startup; readiness appears only afterward.
    let startup_queue = queue.clone();
    let startup_pool = owner.clone();
    let startup_handlers =
        infrastructure_handlers_with_writer(&startup_queue, Some(owner.clone()))?;
    let config = WorkerConfig {
        lanes: [Lane::Maintenance].into_iter().collect(),
        concurrency: 1,
        remote_http_concurrency: 1,
        media_concurrency: 1,
        lease_seconds: 30,
        poll_milliseconds: 10,
        heartbeat_seconds: 60,
        shutdown_seconds: 2,
    };
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let mut startup_fence = owner.begin().await?;
    sqlx::query("LOCK TABLE polls IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *startup_fence)
        .await?;
    let runtime = tokio::spawn(async move {
        run_until_shutdown(
            startup_queue,
            startup_handlers,
            config,
            "poll-reconciliation-startup".to_owned(),
            Some(startup_pool),
            async move {
                let _ = shutdown_receiver.await;
            },
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_millis(750), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
                 WHERE pid <> pg_backend_pid() \
                   AND query LIKE 'SELECT max(id) FROM polls%' \
                   AND wait_event_type = 'Lock')",
            )
            .fetch_one(&owner)
            .await?;
            if blocked {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    let heartbeats_before_reconciliation: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.heartbeats \
         WHERE process_id LIKE 'poll-reconciliation-startup:%'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(
        heartbeats_before_reconciliation, 0,
        "readiness must wait for startup reconciliation"
    );
    startup_fence.rollback().await?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let ready: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rustodon.heartbeats \
                 WHERE process_id LIKE 'poll-reconciliation-startup:%'",
            )
            .fetch_one(&owner)
            .await?;
            if ready == 2 {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    shutdown_sender
        .send(())
        .expect("startup runtime is listening");
    runtime.await??;

    sqlx::query(
        "UPDATE rustodon.outbox_events SET payload = jsonb_build_object('version', 0) \
         WHERE kind = $1 AND logical_key = $2",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .bind(format!(
        "poll-expiration-effect:{completed_poll}:generation:{}",
        generation(completed_poll)
    ))
    .execute(&owner)
    .await?;
    let malformed_scan_started_at = Utc::now();
    let malformed_scan_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Maintenance,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                json!({
                    "scan_started_at": malformed_scan_started_at.to_rfc3339(),
                    "through_poll_id": completed_poll,
                    "after_poll_id": completed_poll - 1,
                    "segment": 0,
                }),
            )
            .logical_key(format!(
                "poll-expiration-reconcile:{}:0:{}",
                malformed_scan_started_at.timestamp_micros(),
                completed_poll - 1
            )),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "malformed-terminal-marker-fixture",
                &[Lane::Maintenance],
                Duration::seconds(30),
            )
            .await?,
        "a malformed exact marker remains an actionable candidate"
    );
    let malformed_retry: (i32, Option<String>) =
        sqlx::query_as("SELECT attempts, last_error FROM rustodon.durable_jobs WHERE id = $1")
            .bind(malformed_scan_id)
            .fetch_one(&owner)
            .await?;
    assert_eq!(malformed_retry.0, 1);
    assert_eq!(
        malformed_retry.1.as_deref(),
        Some("poll expiration durable repair failed")
    );
    let malformed_fallback = reconcile_poll_expirations_with_primary_timeout_for_test(
        queue.clone(),
        owner.clone(),
        json!({
            "scan_started_at": Utc::now().to_rfc3339(),
            "through_poll_id": completed_poll,
            "after_poll_id": completed_poll - 1,
            "segment": 0,
        }),
        1,
    )
    .await
    .expect_err("fallback must reject a malformed exact effect marker");
    assert_eq!(
        malformed_fallback.to_string(),
        "poll expiration fallback marker is invalid"
    );

    sqlx::query(
        "UPDATE rustodon.outbox_events \
         SET payload = jsonb_build_object( \
             'version', 1, 'poll_id', $3::bigint, 'expires_at_micros', $4::bigint, \
             'outcome', 'effects_enqueued'), dispatched_at = NULL \
         WHERE kind = $1 AND logical_key = $2",
    )
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
    .bind(format!(
        "poll-expiration-effect:{completed_poll}:generation:{}",
        generation(completed_poll)
    ))
    .bind(completed_poll)
    .bind(generation(completed_poll))
    .execute(&owner)
    .await?;
    let undispatched_scan_started_at = Utc::now();
    let undispatched_scan_id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Maintenance,
                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                json!({
                    "scan_started_at": undispatched_scan_started_at.to_rfc3339(),
                    "through_poll_id": completed_poll,
                    "after_poll_id": completed_poll - 1,
                    "segment": 0,
                }),
            )
            .logical_key(format!(
                "poll-expiration-reconcile:{}:0:{}",
                undispatched_scan_started_at.timestamp_micros(),
                completed_poll - 1
            )),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "undispatched-terminal-marker-fixture",
                &[Lane::Maintenance],
                Duration::seconds(30),
            )
            .await?,
        "an undispatched exact effect remains an actionable candidate"
    );
    let undispatched_retry: (i32, Option<String>) =
        sqlx::query_as("SELECT attempts, last_error FROM rustodon.durable_jobs WHERE id = $1")
            .bind(undispatched_scan_id)
            .fetch_one(&owner)
            .await?;
    assert_eq!(undispatched_retry.0, 1);
    assert_eq!(
        undispatched_retry.1.as_deref(),
        Some("poll expiration durable repair failed")
    );
    let undispatched_fallback = reconcile_poll_expirations_with_primary_timeout_for_test(
        queue.clone(),
        owner.clone(),
        json!({
            "scan_started_at": Utc::now().to_rfc3339(),
            "through_poll_id": completed_poll,
            "after_poll_id": completed_poll - 1,
            "segment": 0,
        }),
        1,
    )
    .await
    .expect_err("fallback must keep an undispatched exact effect actionable");
    assert_eq!(
        undispatched_fallback.to_string(),
        "poll expiration fallback marker is invalid"
    );

    let mut cleanup = owner.begin().await?;
    sqlx::query("UPDATE statuses SET poll_id = NULL WHERE id BETWEEN $1 AND $2")
        .bind(STATUS_BASE - 39)
        .bind(STATUS_BASE)
        .execute(&mut *cleanup)
        .await?;
    sqlx::query("DELETE FROM polls WHERE id BETWEEN $1 AND $2")
        .bind(POLL_BASE - 39)
        .bind(POLL_BASE)
        .execute(&mut *cleanup)
        .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id BETWEEN $1 AND $2")
        .bind(STATUS_BASE - 39)
        .bind(STATUS_BASE)
        .execute(&mut *cleanup)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id BETWEEN $1 AND $2")
        .bind(STATUS_BASE - 39)
        .bind(STATUS_BASE)
        .execute(&mut *cleanup)
        .await?;
    cleanup.commit().await?;
    reset().await?;
    Ok(())
}
