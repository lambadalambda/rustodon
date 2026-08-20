use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{Duration, Utc};
use rustodon::config::WorkerConfig;
use rustodon::jobs::{
    JobSpec, Lane, Queue, RetryResult, WorkerHeartbeat, enqueue_in, record_outbox_in,
};
use rustodon::operational_schema::{MigrationError, validate};
use rustodon::worker::{
    HandlerFailure, HandlerRegistry, ResourceClass, WorkerError, WorkerExecutor,
    infrastructure_handlers, run_until_shutdown,
};
use serde_json::json;
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use tokio::sync::Notify;

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn runtime_role_is_distinct_and_least_privileged() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let admin_url = std::env::var("RUSTODON_WORKER_ADMIN_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    reset().await?;
    let runtime_user = sqlx::query_scalar::<_, String>("SELECT CURRENT_USER::text")
        .fetch_one(&pool)
        .await?;
    let mut admin = PgConnection::connect(&admin_url).await?;
    let schema_owner = sqlx::query_scalar::<_, String>(
        "SELECT owner::text FROM rustodon.schema_migrations ORDER BY version LIMIT 1",
    )
    .fetch_one(&mut admin)
    .await?;
    assert_ne!(runtime_user, schema_owner);
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT pg_catalog.has_database_privilege( \
               CURRENT_USER, pg_catalog.current_database(), 'TEMP')",
        )
        .fetch_one(&pool)
        .await?
    );
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM public.accounts")
            .fetch_one(&pool)
            .await?
            > 0
    );
    for statement in [
        "SET ROLE rustodon_schema_migrator",
        "CREATE TABLE rustodon.runtime_forbidden (id bigint)",
        "CREATE TABLE public.runtime_forbidden (id bigint)",
        "CREATE TEMPORARY TABLE runtime_forbidden (id bigint)",
        "TRUNCATE rustodon.durable_jobs",
        "UPDATE rustodon.schema_migrations SET applied_at = clock_timestamp()",
        "UPDATE public.accounts SET username = username WHERE false",
    ] {
        assert!(
            sqlx::query(statement).execute(&pool).await.is_err(),
            "runtime role unexpectedly executed {statement}"
        );
    }
    let mut owner = PgConnection::connect(&owner_url).await?;
    sqlx::query("GRANT UPDATE ON public.accounts TO rustodon_worker_runtime")
        .execute(&mut owner)
        .await?;
    let mut validation = PgConnection::connect(&url).await?;
    assert!(matches!(
        validate(&mut validation).await,
        Err(MigrationError::SchemaDrift(_))
    ));
    sqlx::query("REVOKE UPDATE ON public.accounts FROM rustodon_worker_runtime")
        .execute(&mut owner)
        .await?;
    validate(&mut validation).await?;
    sqlx::query("GRANT UPDATE ON public.accounts TO PUBLIC")
        .execute(&mut owner)
        .await?;
    assert!(matches!(
        validate(&mut validation).await,
        Err(MigrationError::SchemaDrift(_))
    ));
    sqlx::query("REVOKE UPDATE ON public.accounts FROM PUBLIC")
        .execute(&mut owner)
        .await?;
    validate(&mut validation).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn durable_queue_recovers_without_losing_or_double_claiming_work()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(12)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool.clone());
    reset().await?;

    let mut transaction = pool.begin().await?;
    enqueue_in(
        &mut transaction,
        &JobSpec::new(Lane::Core, "rolled-back", json!({"value": 1})),
    )
    .await?;
    transaction.rollback().await?;
    assert_eq!(queue.queued_count().await?, 0);

    let due = queue
        .enqueue(&JobSpec::new(
            Lane::Core,
            "fixture.core",
            json!({"value": 2}),
        ))
        .await?;
    let delayed = queue
        .enqueue(
            &JobSpec::new(Lane::Core, "fixture.delayed", json!({}))
                .run_at(Utc::now() + Duration::hours(1)),
        )
        .await?;
    assert!(delayed > due);

    let claims = (0..8)
        .map(|index| {
            let queue = queue.clone();
            async move {
                queue
                    .claim(
                        &format!("worker-{index}"),
                        &[Lane::Core],
                        Duration::seconds(30),
                    )
                    .await
            }
        })
        .collect::<Vec<_>>();
    let claimed = futures_util::future::join_all(claims)
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, due);
    assert_eq!(claimed[0].attempt, 1);

    assert!(!queue.complete(due, "wrong", claimed[0].generation).await?);
    assert!(
        queue
            .renew(
                due,
                &claimed[0].lease_owner,
                claimed[0].generation,
                Duration::seconds(30),
            )
            .await?
    );
    sqlx::query(
        "UPDATE rustodon.durable_jobs SET lease_expires_at = clock_timestamp() - interval '1 second' \
         WHERE id = $1",
    )
    .bind(due)
    .execute(&pool)
    .await?;
    assert!(
        !queue
            .complete(due, &claimed[0].lease_owner, claimed[0].generation)
            .await?,
        "an expired lease cannot acknowledge work"
    );
    let recovered = queue
        .claim("recovery", &[Lane::Core], Duration::seconds(30))
        .await?
        .expect("expired lease is reclaimable");
    assert_eq!(recovered.id, due);
    assert_eq!(recovered.attempt, 2);
    assert!(recovered.generation > claimed[0].generation);
    assert!(
        !queue
            .complete(due, &claimed[0].lease_owner, claimed[0].generation)
            .await?
    );
    assert!(
        queue
            .complete(due, &recovered.lease_owner, recovered.generation)
            .await?
    );
    assert!(
        queue
            .claim("idle", &[Lane::Core], Duration::seconds(30))
            .await?
            .is_none()
    );

    let final_attempt = queue
        .enqueue(&JobSpec::new(Lane::Core, "fixture.crash-final", json!({})).max_attempts(1))
        .await?;
    queue
        .claim("crashed-final", &[Lane::Core], Duration::milliseconds(25))
        .await?
        .expect("final attempt is claimed");
    tokio::time::sleep(std::time::Duration::from_millis(35)).await;
    let recovered_final = queue
        .claim("recovery-final", &[Lane::Core], Duration::seconds(1))
        .await?
        .expect("a crash never consumes the final execution opportunity");
    assert_eq!(recovered_final.id, final_attempt);
    assert_eq!(recovered_final.attempt, 1);
    assert!(
        queue
            .complete(
                recovered_final.id,
                &recovered_final.lease_owner,
                recovered_final.generation,
            )
            .await?
    );
    assert!(queue.dead_letters(10).await?.is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn retries_cancellation_outbox_dead_letters_and_readiness_are_operational()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool.clone());
    reset().await?;

    let duplicate = JobSpec::new(Lane::Push, "deliver", json!({"id": 1}))
        .logical_key("https://remote.invalid/inbox")
        .max_attempts(2);
    let first = queue.enqueue(&duplicate).await?;
    assert_eq!(queue.enqueue(&duplicate).await?, first);
    assert_eq!(
        queue
            .cancel("deliver", "https://remote.invalid/inbox")
            .await?,
        1
    );

    let leased_id = queue.enqueue(&duplicate).await?;
    let leased = queue
        .claim("push-active", &[Lane::Push], Duration::seconds(30))
        .await?
        .expect("the replacement delivery is claimed");
    assert_eq!(leased.id, leased_id);
    assert_eq!(
        queue
            .cancel("deliver", "https://remote.invalid/inbox")
            .await?,
        0,
        "cancellation must not delete a lease while its handler is running"
    );
    assert!(
        queue
            .complete(leased.id, &leased.lease_owner, leased.generation)
            .await?
    );

    let abandoned_id = queue.enqueue(&duplicate).await?;
    queue
        .claim("push-crashed", &[Lane::Push], Duration::milliseconds(25))
        .await?
        .expect("the cancellable delivery is claimed before its worker crashes");
    tokio::time::sleep(std::time::Duration::from_millis(35)).await;
    assert_eq!(
        queue
            .cancel("deliver", "https://remote.invalid/inbox")
            .await?,
        1
    );
    assert!(
        queue
            .claim("after-cancel", &[Lane::Push], Duration::seconds(1))
            .await?
            .is_none(),
        "an expired crash-abandoned job stays cancelled"
    );
    assert_ne!(abandoned_id, leased_id);

    let dead_id = queue
        .enqueue(&JobSpec::new(Lane::Pull, "fetch", json!({})).max_attempts(2))
        .await?;
    let first_claim = queue
        .claim("pull-1", &[Lane::Pull], Duration::seconds(30))
        .await?
        .unwrap();
    let retry_at = Utc::now() + Duration::milliseconds(50);
    assert_eq!(
        queue
            .retry(&first_claim, retry_at, "temporary remote failure",)
            .await?,
        RetryResult::Scheduled
    );
    assert!(
        queue
            .claim("too-early", &[Lane::Pull], Duration::seconds(30))
            .await?
            .is_none()
    );
    tokio::time::sleep(std::time::Duration::from_millis(75)).await;
    let final_claim = queue
        .claim("pull-2", &[Lane::Pull], Duration::milliseconds(25))
        .await?
        .unwrap();
    assert_eq!(final_claim.id, dead_id);
    assert_eq!(final_claim.attempt, 2);
    assert_eq!(
        queue.retry(&final_claim, Utc::now(), "exhausted").await?,
        RetryResult::Dead
    );
    let dead = queue.dead_letters(10).await?;
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].id, dead_id);
    assert_eq!(dead[0].last_error.as_deref(), Some("exhausted"));

    let mut transaction = pool.begin().await?;
    record_outbox_in(
        &mut transaction,
        &JobSpec::new(Lane::Ingress, "process.activity", json!({"id": "a"}))
            .logical_key("activity:a"),
    )
    .await?;
    transaction.commit().await?;
    assert_eq!(queue.dispatch_outbox(10).await?, 1);
    assert_eq!(queue.dispatch_outbox(10).await?, 0);
    let first_outbox_job = queue
        .claim("ingress", &[Lane::Ingress], Duration::seconds(30))
        .await?
        .expect("the first outbox event becomes a durable job");
    let mut transaction = pool.begin().await?;
    record_outbox_in(
        &mut transaction,
        &JobSpec::new(Lane::Ingress, "process.activity", json!({"id": "b"}))
            .logical_key("activity:a"),
    )
    .await?;
    transaction.commit().await?;
    assert_eq!(
        queue.dispatch_outbox(10).await?,
        0,
        "a newer event must remain pending while the previous keyed job is live"
    );
    assert!(
        queue
            .complete(
                first_outbox_job.id,
                &first_outbox_job.lease_owner,
                first_outbox_job.generation,
            )
            .await?
    );
    assert_eq!(queue.dispatch_outbox(10).await?, 1);
    let second_outbox_job = queue
        .claim("ingress-next", &[Lane::Ingress], Duration::seconds(30))
        .await?
        .expect("the newer keyed event dispatches after the previous job completes");
    assert_eq!(second_outbox_job.arguments, json!({"id": "b"}));
    assert!(
        queue
            .complete(
                second_outbox_job.id,
                &second_outbox_job.lease_owner,
                second_outbox_job.generation,
            )
            .await?
    );

    let mut transaction = pool.begin().await?;
    record_outbox_in(
        &mut transaction,
        &JobSpec::new(Lane::Ingress, "process.activity", json!({"id": "race"}))
            .logical_key("activity:race"),
    )
    .await?;
    transaction.commit().await?;
    let mut blocker = pool.begin().await?;
    sqlx::query("LOCK TABLE rustodon.durable_jobs IN SHARE MODE")
        .execute(&mut *blocker)
        .await?;
    let dispatch_queue = queue.clone();
    let dispatch = tokio::spawn(async move { dispatch_queue.dispatch_outbox(10).await });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS ( \
                   SELECT 1 FROM pg_catalog.pg_stat_activity \
                   WHERE usename = CURRENT_USER AND wait_event_type = 'Lock' \
                     AND query LIKE 'INSERT INTO rustodon.durable_jobs%')",
            )
            .fetch_one(&pool)
            .await
            .expect("dispatch lock inspection succeeds")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    let cancel_queue = queue.clone();
    let cancel = tokio::spawn(async move {
        cancel_queue
            .cancel("process.activity", "activity:race")
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    blocker.commit().await?;
    assert_eq!(dispatch.await??, 1);
    assert_eq!(cancel.await??, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.durable_jobs \
             WHERE kind = 'process.activity' AND logical_key = 'activity:race'",
        )
        .fetch_one(&pool)
        .await?,
        0
    );

    queue
        .heartbeat(&WorkerHeartbeat::worker(
            "worker-ready",
            [
                Lane::Ingress,
                Lane::Core,
                Lane::Push,
                Lane::Pull,
                Lane::Mail,
                Lane::Maintenance,
            ],
            json!({"concurrency": 6}),
        ))
        .await?;
    queue
        .heartbeat(&WorkerHeartbeat::scheduler("scheduler-ready", json!({})))
        .await?;
    let readiness = queue
        .readiness(
            &Lane::ALL.into_iter().collect::<BTreeSet<_>>(),
            Duration::seconds(30),
        )
        .await?;
    assert!(readiness.ready());
    assert_eq!(readiness.dead_letters, 1);
    assert!(readiness.missing_lanes.is_empty());
    assert!(readiness.scheduler_alive);
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn executor_bounds_resources_and_duplicate_execution_keeps_one_effect()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(12)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool.clone());
    reset().await?;

    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let registry = HandlerRegistry::new();
    registry.register("fixture.http", Lane::Pull, ResourceClass::RemoteHttp, {
        let pool = pool.clone();
        let active = Arc::clone(&active);
        let maximum = Arc::clone(&maximum);
        move |job| {
            let pool = pool.clone();
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            Box::pin(async move {
                let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(now_active, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                sqlx::query(
                    "INSERT INTO rustodon.idempotency_keys \
                           (scope, key, fingerprint, result, expires_at) \
                         VALUES ('worker-test', $1, $2, '{}'::jsonb, \
                                 clock_timestamp() + interval '1 hour') \
                         ON CONFLICT (scope, key) DO NOTHING",
                )
                .bind(job.logical_key.as_deref().unwrap_or("missing"))
                .bind([0_u8; 32].as_slice())
                .execute(&pool)
                .await
                .map_err(|_| HandlerFailure::retry("effect write failed"))?;
                active.fetch_sub(1, Ordering::SeqCst);
                if job.attempt == 1 {
                    Err(HandlerFailure::retry("retry after committed effect"))
                } else {
                    Ok(())
                }
            })
        }
    })?;
    for index in 0..3 {
        queue
            .enqueue(
                &JobSpec::new(Lane::Pull, "fixture.http", json!({}))
                    .logical_key(format!("effect-{index}"))
                    .max_attempts(2),
            )
            .await?;
    }
    let executor = WorkerExecutor::new(queue.clone(), registry, 1, 1)?;
    for round in 0..2 {
        let attempts = (0..3)
            .map(|slot| {
                let executor = executor.clone();
                async move {
                    executor
                        .process_one(
                            &format!("runtime-{round}-{slot}"),
                            &[Lane::Pull],
                            Duration::seconds(1),
                        )
                        .await
                }
            })
            .collect::<Vec<_>>();
        for result in futures_util::future::join_all(attempts).await {
            assert!(result?);
        }
        if round == 0 {
            sqlx::query(
                "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() \
                 WHERE kind = 'fixture.http' AND dead_at IS NULL",
            )
            .execute(&pool)
            .await?;
        }
    }
    assert_eq!(maximum.load(Ordering::SeqCst), 1);
    assert_eq!(queue.queued_count().await?, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.idempotency_keys WHERE scope = 'worker-test'",
        )
        .fetch_one(&pool)
        .await?,
        3
    );
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn executor_renews_a_lease_while_waiting_for_a_resource_permit()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool);
    reset().await?;

    let first_started = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());
    let executions = Arc::new(AtomicUsize::new(0));
    let registry = HandlerRegistry::new();
    registry.register(
        "fixture.permit-wait",
        Lane::Pull,
        ResourceClass::RemoteHttp,
        {
            let first_started = Arc::clone(&first_started);
            let release_first = Arc::clone(&release_first);
            let executions = Arc::clone(&executions);
            move |job| {
                let first_started = Arc::clone(&first_started);
                let release_first = Arc::clone(&release_first);
                let executions = Arc::clone(&executions);
                async move {
                    executions.fetch_add(1, Ordering::SeqCst);
                    if job.arguments["position"] == 1 {
                        first_started.notify_one();
                        release_first.notified().await;
                    }
                    Ok(())
                }
            }
        },
    )?;
    queue
        .enqueue(&JobSpec::new(
            Lane::Pull,
            "fixture.permit-wait",
            json!({"position": 1}),
        ))
        .await?;
    let second_id = queue
        .enqueue(&JobSpec::new(
            Lane::Pull,
            "fixture.permit-wait",
            json!({"position": 2}),
        ))
        .await?;
    let executor = WorkerExecutor::new(queue.clone(), registry, 1, 1)?;
    let first_executor = executor.clone();
    let first = tokio::spawn(async move {
        first_executor
            .process_one("permit-first", &[Lane::Pull], Duration::milliseconds(300))
            .await
    });
    first_started.notified().await;
    let second_executor = executor.clone();
    let second = tokio::spawn(async move {
        second_executor
            .process_one("permit-second", &[Lane::Pull], Duration::milliseconds(300))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert!(
        queue
            .claim("lease-thief", &[Lane::Pull], Duration::seconds(1))
            .await?
            .is_none(),
        "a permit waiter must renew its already-claimed lease"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM rustodon.durable_jobs WHERE lease_owner = 'permit-second'",
        )
        .fetch_one(queue.pool())
        .await?,
        second_id
    );
    release_first.notify_one();
    assert!(first.await??);
    assert!(second.await??);
    assert_eq!(executions.load(Ordering::SeqCst), 2);
    assert_eq!(queue.queued_count().await?, 0);
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn runtime_publishes_readiness_and_removes_it_on_graceful_shutdown()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool.clone());
    reset().await?;
    let handlers = infrastructure_handlers(&queue)?;
    let config = WorkerConfig {
        lanes: [Lane::Maintenance].into_iter().collect(),
        concurrency: 2,
        remote_http_concurrency: 1,
        media_concurrency: 1,
        lease_seconds: 5,
        poll_milliseconds: 10,
        heartbeat_seconds: 3600,
        shutdown_seconds: 5,
    };
    let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        run_until_shutdown(
            runtime_queue,
            handlers,
            config,
            "runtime-test".to_owned(),
            async move {
                let _ = shutdown_receiver.await;
            },
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let readiness = queue
                .readiness(
                    &[Lane::Maintenance].into_iter().collect(),
                    Duration::seconds(3),
                )
                .await
                .expect("readiness query succeeds");
            if readiness.ready() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await?;
    let advertised_lanes = sqlx::query_scalar::<_, Vec<String>>(
        "SELECT lanes FROM rustodon.heartbeats WHERE process_id = 'runtime-test:worker'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(advertised_lanes, vec!["maintenance"]);
    assert!(
        !queue
            .readiness(&Lane::ALL.into_iter().collect(), Duration::seconds(3))
            .await?
            .ready(),
        "readiness must not claim lanes without registered handlers"
    );
    let mut transaction = pool.begin().await?;
    let outbox_id = record_outbox_in(
        &mut transaction,
        &JobSpec::new(
            Lane::Maintenance,
            "rustodon.maintenance.prune",
            json!({"source": "outbox-poll"}),
        )
        .logical_key("outbox-poll"),
    )
    .await?;
    transaction.commit().await?;
    tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if sqlx::query_scalar::<_, bool>(
                "SELECT dispatched_at IS NOT NULL FROM rustodon.outbox_events WHERE id = $1",
            )
            .bind(outbox_id)
            .fetch_one(&pool)
            .await
            .expect("outbox inspection succeeds")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    shutdown_sender.send(()).expect("runtime is listening");
    runtime.await??;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.heartbeats WHERE process_id LIKE 'runtime-test:%'",
        )
        .fetch_one(&pool)
        .await?,
        0
    );
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn runtime_rejects_configured_lanes_without_handlers()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool);
    reset().await?;
    let handlers = infrastructure_handlers(&queue)?;
    let config = WorkerConfig {
        lanes: Lane::ALL.into_iter().collect(),
        concurrency: 1,
        remote_http_concurrency: 1,
        media_concurrency: 1,
        lease_seconds: 5,
        poll_milliseconds: 10,
        heartbeat_seconds: 1,
        shutdown_seconds: 1,
    };
    assert!(matches!(
        run_until_shutdown(
            queue,
            handlers,
            config,
            "unsupported-lanes".to_owned(),
            std::future::pending(),
        )
        .await,
        Err(WorkerError::InvalidConfiguration(_))
    ));
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn shutdown_deadline_aborts_without_acknowledging_and_returns_failure()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool.clone());
    reset().await?;
    queue
        .enqueue(&JobSpec::new(Lane::Core, "fixture.never-finish", json!({})))
        .await?;
    let started = Arc::new(Notify::new());
    let registry = HandlerRegistry::new();
    registry.register("fixture.never-finish", Lane::Core, ResourceClass::None, {
        let started = Arc::clone(&started);
        move |_job| {
            let started = Arc::clone(&started);
            async move {
                started.notify_one();
                std::future::pending::<()>().await;
                Ok(())
            }
        }
    })?;
    let config = WorkerConfig {
        lanes: [Lane::Core].into_iter().collect(),
        concurrency: 1,
        remote_http_concurrency: 1,
        media_concurrency: 1,
        lease_seconds: 5,
        poll_milliseconds: 10,
        heartbeat_seconds: 1,
        shutdown_seconds: 1,
    };
    let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        run_until_shutdown(
            runtime_queue,
            registry,
            config,
            "shutdown-timeout".to_owned(),
            async move {
                let _ = shutdown_receiver.await;
            },
        )
        .await
    });
    started.notified().await;
    shutdown_sender.send(()).expect("runtime is listening");
    tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            let heartbeats = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.heartbeats \
                 WHERE process_id LIKE 'shutdown-timeout:%'",
            )
            .fetch_one(&pool)
            .await
            .expect("heartbeat inspection succeeds");
            if heartbeats == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    assert!(!runtime.is_finished(), "the handler is still draining");
    let result = runtime.await?;
    assert!(matches!(result, Err(WorkerError::ShutdownTimedOut)));
    assert_eq!(queue.queued_count().await?, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.heartbeats WHERE process_id LIKE 'shutdown-timeout:%'",
        )
        .fetch_one(&pool)
        .await?,
        0
    );
    Ok(())
}

async fn reset() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_ADMIN_DATABASE_URL")?;
    let mut connection = PgConnection::connect(&url).await?;
    sqlx::raw_sql(
        "TRUNCATE rustodon.durable_jobs, rustodon.outbox_events, rustodon.heartbeats, \
                  rustodon.idempotency_keys, rustodon.ordering_markers, rustodon.domain_health \
         RESTART IDENTITY CASCADE",
    )
    .execute(&mut connection)
    .await?;
    Ok(())
}
