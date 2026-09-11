#[path = "workers/lifecycles.rs"]
mod lifecycles;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use http::Method as HttpMethod;
use reqwest::header::{CONTENT_TYPE, HOST, HeaderMap, HeaderValue};
use rustodon::config::{
    Mailbox, SmtpAuthentication, SmtpConfig, SmtpDeliveryMethod, SmtpSettings, SmtpTransport,
    WorkerConfig,
};
use rustodon::jobs::{
    ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND, ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
    ACTIVITYPUB_ANNOUNCE_RESOLVE_JOB_KIND, ACTIVITYPUB_DELIVERY_JOB_KIND,
    ACTIVITYPUB_EMOJI_CLEANUP_JOB_KIND, ACTIVITYPUB_EMOJI_FETCH_JOB_KIND,
    ACTIVITYPUB_INBOX_JOB_KIND, ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
    ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND, ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
    ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND, JobError, JobSpec, LOCAL_MEDIA_CLEANUP_JOB_KIND, Lane,
    MASTODON_ACCOUNT_PURGE_JOB_KIND, MASTODON_DOMAIN_BLOCK_JOB_KIND,
    MASTODON_DOMAIN_PURGE_JOB_KIND, NOTIFICATION_CLEANUP_JOB_KIND, NOTIFICATION_CREATE_JOB_KIND,
    NOTIFICATION_UNFILTER_JOB_KIND, Queue, RetryResult, WorkerHeartbeat, enqueue_in,
    record_outbox_in, record_outbox_once_in,
};
use rustodon::mail::{MailConfig, REPORT_JOB_KIND};
use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{
    AccountProfileUpdate, AccountProfileValue, BearerAuthenticator, HttpSignatureRequest,
    HttpSignatureSigner, MediaAttachmentCreate, MediaAttachmentUpdate, NotificationPolicyUpdate,
    Repository, StatusUpdate, WRITE_BLOCKS, WRITE_FOLLOWS, WRITE_MEDIA, WRITE_NOTIFICATIONS,
    WRITE_REPORTS, WRITE_STATUSES, WriteError, WriteOptions, WriteRepository, activitypub,
    body_digest_header, sign_http_signature_with_headers,
};
use rustodon::operational_schema::{MigrationError, validate};
use rustodon::paperclip::{
    PaperclipAttachment, PaperclipMetadata, PaperclipRoot, prepare_media_attachment,
    write_prepared_media,
};
#[cfg(feature = "test-support")]
use rustodon::paperclip::{PaperclipCommitFault, PaperclipRemoveFault, PaperclipWriteFault};
use rustodon::secret::SecretString;
use rustodon::streaming::STREAM_EVENT_KIND;
#[cfg(feature = "test-support")]
use rustodon::web::cleanup_media_after_response_failure_for_test;
use rustodon::worker::{
    ActivityPubDeliveryConfig, HandlerFailure, HandlerRegistry, ResourceClass, WorkerError,
    WorkerExecutor, infrastructure_handlers, infrastructure_handlers_with_writer,
    infrastructure_handlers_with_writer_and_mail_and_federation, run_until_shutdown,
};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{Notify, oneshot};
use url::Url;

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn runtime_role_is_distinct_and_least_privileged() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let admin_url = std::env::var("RUSTODON_WORKER_ADMIN_DATABASE_URL")?;
    let writer_url = std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?;
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
    let mut writer = PgConnection::connect(&writer_url).await?;
    let writer_role = sqlx::query_scalar::<_, String>("SELECT CURRENT_USER::text")
        .fetch_one(&mut writer)
        .await?;
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
    sqlx::query("SELECT pg_catalog.set_config('rustodon.writer_role', $1, false)")
        .bind(&writer_role)
        .execute(&mut validation)
        .await?;
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

    assert!(
        queue
            .merge_job_arguments(&claimed[0], &json!({"lease_probe": true}))
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, Value>(
            "SELECT arguments FROM rustodon.durable_jobs WHERE id = $1",
        )
        .bind(due)
        .fetch_one(&pool)
        .await?["lease_probe"],
        true
    );
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
    assert!(
        !queue
            .merge_job_arguments(&claimed[0], &json!({"stale_probe": true}))
            .await?,
        "an expired lease cannot update job arguments"
    );
    let recovered = queue
        .claim("recovery", &[Lane::Core], Duration::seconds(30))
        .await?
        .expect("expired lease is reclaimable");
    assert_eq!(recovered.id, due);
    assert_eq!(recovered.attempt, 2);
    assert!(recovered.generation > claimed[0].generation);
    assert!(
        queue
            .initialize_job_arguments(&claimed[0], &json!({"stale_initialize": true}))
            .await?
            .is_none(),
        "a stale lease cannot initialize durable arguments"
    );
    let initialized = queue
        .initialize_job_arguments(&recovered, &json!({"recovered_initialize": true}))
        .await?
        .expect("the reclaimed lease can initialize durable arguments");
    assert_eq!(initialized["recovered_initialize"], true);
    assert_eq!(initialized["lease_probe"], true);
    let initialized_again = queue
        .initialize_job_arguments(&recovered, &json!({"recovered_initialize": false}))
        .await?
        .expect("the current lease remains valid");
    assert_eq!(initialized_again["recovered_initialize"], true);
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
async fn aborted_handlers_in_every_worker_lane_are_reclaimed()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool);
    reset().await?;
    let registry = HandlerRegistry::new();

    for lane in Lane::ALL {
        let kind = format!("fixture.abort-reclaim.{lane}");
        let started = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        registry.register(kind.clone(), lane, ResourceClass::None, {
            let started = Arc::clone(&started);
            let calls = Arc::clone(&calls);
            move |_job| {
                let started = Arc::clone(&started);
                let calls = Arc::clone(&calls);
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Ok(())
                }
            }
        })?;
        queue.enqueue(&JobSpec::new(lane, &kind, json!({}))).await?;
        let executor = WorkerExecutor::new(queue.clone(), registry.clone(), 1, 1)?;
        let recovery_executor = executor.clone();
        let lease_owner = format!("abort-{lane}");
        let worker = tokio::spawn(async move {
            executor
                .process_one(&lease_owner, &[lane], Duration::milliseconds(50))
                .await
        });
        started.notified().await;
        worker.abort();
        assert!(
            worker
                .await
                .expect_err("aborted handler must not finish")
                .is_cancelled()
        );
        tokio::time::sleep(std::time::Duration::from_millis(75)).await;

        assert!(
            recovery_executor
                .process_one(&format!("recover-{lane}"), &[lane], Duration::seconds(1),)
                .await?
        );
        assert!(
            queue
                .claim(&format!("verify-{lane}"), &[lane], Duration::seconds(1),)
                .await?
                .is_none(),
            "recovered {lane} job must be acknowledged",
        );
    }
    reset().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn worker_reclaims_job_after_database_failure_before_acknowledgement()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool.clone());
    reset().await?;

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let registry = HandlerRegistry::new();
    registry.register(
        "fixture.database-failure-before-ack",
        Lane::Core,
        ResourceClass::None,
        {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            move |_job| {
                let started = Arc::clone(&started);
                let release = Arc::clone(&release);
                async move {
                    started.notify_one();
                    release.notified().await;
                    Ok(())
                }
            }
        },
    )?;
    let job_id = queue
        .enqueue(&JobSpec::new(
            Lane::Core,
            "fixture.database-failure-before-ack",
            json!({}),
        ))
        .await?;
    let executor = WorkerExecutor::new(queue, registry, 1, 1)?;
    let worker = tokio::spawn(async move {
        executor
            .process_one(
                "database-failure",
                &[Lane::Core],
                Duration::milliseconds(50),
            )
            .await
    });
    started.notified().await;

    pool.close().await;
    release.notify_one();
    assert!(
        worker.await?.is_err(),
        "acknowledgement failure must surface"
    );

    tokio::time::sleep(std::time::Duration::from_millis(75)).await;
    let recovery_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let recovery_queue = Queue::new(recovery_pool);
    let recovered = recovery_queue
        .claim("database-recovery", &[Lane::Core], Duration::seconds(1))
        .await?
        .expect("a job whose acknowledgement failed must be reclaimable");
    assert_eq!(recovered.id, job_id);
    assert!(recovered.attempt >= 2);
    assert!(
        recovery_queue
            .complete(recovered.id, &recovered.lease_owner, recovered.generation)
            .await?
    );
    reset().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn smtp_acceptance_before_job_ack_is_retried_with_the_same_message_id()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    reset().await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let first_accepted = Arc::new(Notify::new());
    let server = tokio::spawn(fixture_retrying_smtp_server(
        listener,
        Arc::clone(&first_accepted),
    ));
    let mail = worker_mail_config(port, "smtp-first.invalid");
    let queue = Queue::new(pool.clone()).with_complete_fault();
    let current_job = mail.password_reset_job("person@example.invalid", "sealed-reset-token")?;
    let mut legacy_arguments = current_job.arguments().clone();
    legacy_arguments
        .as_object_mut()
        .unwrap()
        .remove("message_id_local");
    legacy_arguments
        .as_object_mut()
        .unwrap()
        .remove("message_id_domain");
    let first_job = JobSpec::new(Lane::Mail, current_job.kind(), legacy_arguments)
        .logical_key(current_job.logical_key_value().unwrap());
    let job_id = queue.enqueue(&first_job).await?;
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        None,
        mail.runtime()?,
        None,
    )?;
    let executor = WorkerExecutor::new(queue, handlers, 1, 1)?;
    let first_attempt = tokio::spawn(async move {
        executor
            .process_one(
                "smtp-before-ack",
                &[Lane::Mail],
                Duration::milliseconds(100),
            )
            .await
    });
    first_accepted.notified().await;
    assert!(matches!(
        first_attempt.await?,
        Err(WorkerError::Jobs(JobError::InvalidData(
            "injected durable-job completion failure"
        )))
    ));
    let persisted_arguments: Value =
        sqlx::query_scalar("SELECT arguments FROM rustodon.durable_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await?;
    let expected = format!(
        "Message-ID: <{}@{}>",
        persisted_arguments["message_id_local"].as_str().unwrap(),
        persisted_arguments["message_id_domain"].as_str().unwrap()
    );
    assert_eq!(persisted_arguments["message_id_domain"], "example.invalid");

    tokio::time::sleep(std::time::Duration::from_millis(125)).await;
    let recovery_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let recovery_queue = Queue::new(recovery_pool.clone());
    let changed_smtp_mail = worker_mail_config(port, "smtp-changed.invalid");
    let recovery_handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &recovery_queue,
        None,
        changed_smtp_mail.runtime()?,
        None,
    )?;
    let recovery = WorkerExecutor::new(recovery_queue.clone(), recovery_handlers, 1, 1)?;
    assert!(
        recovery
            .process_one("smtp-recovery", &[Lane::Mail], Duration::seconds(1))
            .await?,
        "the accepted but unacknowledged message must be retried"
    );
    let distinct_job = mail.confirmation_job("person@example.invalid", "another-sealed-token")?;
    let distinct_message_id = format!(
        "Message-ID: <{}@{}>",
        distinct_job.arguments()["message_id_local"]
            .as_str()
            .unwrap(),
        distinct_job.arguments()["message_id_domain"]
            .as_str()
            .unwrap()
    );
    recovery_queue.enqueue(&distinct_job).await?;
    assert!(
        recovery
            .process_one("smtp-distinct", &[Lane::Mail], Duration::seconds(1))
            .await?
    );
    assert!(
        recovery_queue
            .claim("smtp-idle", &[Lane::Mail], Duration::seconds(1))
            .await?
            .is_none()
    );

    let messages = server.await??;
    assert_eq!(messages.len(), 3);
    for message in &messages[..2] {
        let message = String::from_utf8_lossy(message);
        assert!(message.contains(&expected), "{message:?}");
        assert!(!message.contains("worker-mail-secret"), "{message:?}");
        assert!(!message.contains("smtp-first.invalid"), "{message:?}");
        assert!(!message.contains("smtp-changed.invalid"), "{message:?}");
        assert!(
            !message.contains(&format!("rustodon-mail-{job_id}@")),
            "{message:?}"
        );
    }
    assert_eq!(
        message_id_header(&messages[0]),
        message_id_header(&messages[1]),
        "an SMTP duplicate of one durable job must retain its identity"
    );
    assert_eq!(message_id_header(&messages[2]), Some(distinct_message_id));
    assert_ne!(
        message_id_header(&messages[0]),
        message_id_header(&messages[2]),
        "distinct durable messages must have distinct identities"
    );
    reset().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn ordered_inbox_enqueue_deduplicates_and_serializes_sender_work()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool.clone());
    reset().await?;
    let ordering_key = [7_u8; 32];
    let first = JobSpec::new(
        Lane::Ingress,
        ACTIVITYPUB_INBOX_JOB_KIND,
        json!({"body":"first"}),
    )
    .logical_key("activitypub:first");
    let second = JobSpec::new(
        Lane::Ingress,
        ACTIVITYPUB_INBOX_JOB_KIND,
        json!({"body":"second"}),
    )
    .logical_key("activitypub:second");

    assert!(
        queue
            .enqueue_ordered_once(&first, &ordering_key, &[1_u8; 32])
            .await?
    );
    assert!(matches!(
        queue
            .enqueue_ordered_once(&first, &ordering_key, &[9_u8; 32])
            .await,
        Err(JobError::Conflict(_))
    ));
    assert!(
        !queue
            .enqueue_ordered_once(&first, &ordering_key, &[1_u8; 32])
            .await?
    );
    assert!(
        queue
            .enqueue_ordered_once(&second, &ordering_key, &[2_u8; 32])
            .await?
    );
    assert_eq!(queue.queued_count().await?, 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.idempotency_keys \
             WHERE scope = 'rustodon.activitypub.inbox'",
        )
        .fetch_one(&pool)
        .await?,
        2
    );
    let run_times = sqlx::query_as::<_, (String, DateTime<Utc>)>(
        "SELECT logical_key, run_at FROM rustodon.durable_jobs \
         WHERE kind = $1 ORDER BY run_at, id",
    )
    .bind(ACTIVITYPUB_INBOX_JOB_KIND)
    .fetch_all(&pool)
    .await?;
    assert_eq!(run_times.len(), 2);
    assert_eq!(run_times[0].0, "activitypub:first");
    assert_eq!(run_times[1].0, "activitypub:second");
    assert!(run_times[1].1 > run_times[0].1);
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn concurrent_ordered_enqueue_serializes_the_first_marker_creation()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool.clone());
    reset().await?;
    let ordering_key = [8_u8; 32];
    let first = JobSpec::new(
        Lane::Ingress,
        ACTIVITYPUB_INBOX_JOB_KIND,
        json!({"body":"first"}),
    )
    .logical_key("activitypub:concurrent:first");
    let second = JobSpec::new(
        Lane::Ingress,
        ACTIVITYPUB_INBOX_JOB_KIND,
        json!({"body":"second"}),
    )
    .logical_key("activitypub:concurrent:second");
    let first_queue = queue.clone();
    let second_queue = queue.clone();
    let (first_result, second_result) = tokio::join!(
        first_queue.enqueue_ordered_once(&first, &ordering_key, &[1_u8; 32]),
        second_queue.enqueue_ordered_once(&second, &ordering_key, &[2_u8; 32]),
    );
    assert!(first_result?);
    assert!(second_result?);

    let jobs = sqlx::query_as::<_, (i64, String, Option<i64>)>(
        "SELECT id, arguments ->> 'body', \
                NULLIF(arguments ->> '_rustodon_ordering_predecessor', '')::bigint \
           FROM rustodon.durable_jobs WHERE kind = $1 ORDER BY run_at, id",
    )
    .bind(ACTIVITYPUB_INBOX_JOB_KIND)
    .fetch_all(&pool)
    .await?;
    assert_eq!(jobs.len(), 2);
    assert!(jobs[0].2.is_none());
    assert_eq!(jobs[1].2, Some(jobs[0].0));

    let claimed = queue
        .claim("concurrent-first", &[Lane::Ingress], Duration::seconds(1))
        .await?
        .expect("the first ordered job is claimable");
    assert_eq!(claimed.arguments["body"], jobs[0].1);
    assert!(
        queue
            .claim("concurrent-second", &[Lane::Ingress], Duration::seconds(1))
            .await?
            .is_none()
    );
    assert!(
        queue
            .complete(claimed.id, &claimed.lease_owner, claimed.generation)
            .await?
    );
    let successor = queue
        .claim("concurrent-second", &[Lane::Ingress], Duration::seconds(1))
        .await?
        .expect("the successor is claimable after the first job completes");
    assert_eq!(successor.arguments["body"], jobs[1].1);
    assert!(
        queue
            .complete(successor.id, &successor.lease_owner, successor.generation)
            .await?
    );
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn inbox_ingress_keeps_actor_ordering_across_key_rotation_and_rejects_conflicts()
-> Result<(), Box<dyn std::error::Error>> {
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const LOCAL_DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
    const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
    const PRIMARY_KEY: &str = "https://remote.fixture.invalid/users/bob#secondary-key";
    const ROTATED_KEY: &str = "https://remote.fixture.invalid/users/bob#rotated-key";
    const PRIMARY_KEY_ROW: i64 = 8901;
    const ROTATED_KEY_ROW: i64 = 8999;
    const ACTIVITY_ID: &str = "https://remote.fixture.invalid/activities/key-rotation";
    const SECOND_ACTIVITY_ID: &str =
        "https://remote.fixture.invalid/activities/key-rotation-successor";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let owner_pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&owner_url)
        .await?;
    reset().await?;
    sqlx::query("DELETE FROM keypairs WHERE id = $1 OR uri = $2")
        .bind(ROTATED_KEY_ROW)
        .bind(ROTATED_KEY)
        .execute(&owner_pool)
        .await?;
    sqlx::query(
        "INSERT INTO keypairs \
             (id, account_id, type, uri, public_key, private_key, revoked, expires_at, created_at, updated_at) \
         SELECT $1, account_id, type, $2, public_key, NULL, false, NULL, clock_timestamp(), clock_timestamp() \
           FROM keypairs WHERE id = $3",
    )
    .bind(ROTATED_KEY_ROW)
    .bind(ROTATED_KEY)
    .bind(PRIMARY_KEY_ROW)
    .execute(&owner_pool)
    .await?;

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let host = address.to_string();
    let media_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("inbox-ordering-media-{}", std::process::id()));
    fs::create_dir_all(&media_root)?;
    let state = rustodon::web::WebState::new(
        Repository::connect(&runtime_url).await?,
        Url::parse(ORIGIN)?,
        LOCAL_DOMAIN,
        "/system",
        media_root.clone(),
        InstanceRuntimeConfig {
            domain: LOCAL_DOMAIN.to_owned(),
            version: "4.6.5".to_owned(),
            source_url: "https://github.com/mastodon/mastodon".to_owned(),
            streaming_api: format!("ws://{host}"),
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
        vec![host.clone()],
    )?
    .with_queue(Queue::new(runtime_pool.clone()));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, rustodon::web::router(state))
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let private_key = {
        let (_, remainder) = include_str!("../fixtures/mastodon/v4.6.5/seed.sql")
            .split_once("$fixture_private$")
            .expect("fixture private-key delimiter should be present");
        let (private_key, _) = remainder
            .split_once("$fixture_private$")
            .expect("fixture private-key delimiter should be paired");
        private_key.to_owned()
    };
    let sign_headers = |key_id: &str, body: &[u8]| {
        let mut headers = HeaderMap::new();
        headers.insert(
            HOST,
            HeaderValue::from_str(&host).expect("fixture host is a valid header"),
        );
        headers.insert(
            "Date",
            HeaderValue::from_str(&httpdate::fmt_http_date(SystemTime::now()))
                .expect("HTTP date is a valid header"),
        );
        headers.insert(
            "Digest",
            HeaderValue::from_str(&body_digest_header(body))
                .expect("body digest is a valid header"),
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/activity+json"),
        );
        let request = HttpSignatureRequest::new(&HttpMethod::POST, "/inbox", &headers, body);
        let signature = sign_http_signature_with_headers(
            &request,
            &HttpSignatureSigner {
                key_id,
                private_key_pem: &private_key,
            },
            &["host", "date", "digest", "(request-target)"],
        )
        .expect("fixture inbox request should sign");
        headers.insert(
            "Signature",
            HeaderValue::from_str(&signature).expect("signature is a valid header"),
        );
        headers
    };
    let client = reqwest::Client::new();
    let endpoint = format!("http://{host}/inbox");
    let body = json!({
        "id": ACTIVITY_ID,
        "type": "Follow",
        "actor": ACTOR,
        "object": "https://fixture-v4-6-5.rustodon.invalid/users/alice"
    })
    .to_string();
    let primary = client
        .post(&endpoint)
        .headers(sign_headers(PRIMARY_KEY, body.as_bytes()))
        .body(body.clone())
        .send()
        .await?;
    assert_eq!(primary.status(), reqwest::StatusCode::ACCEPTED);
    let second_body = json!({
        "id": SECOND_ACTIVITY_ID,
        "type": "Follow",
        "actor": ACTOR,
        "object": "https://fixture-v4-6-5.rustodon.invalid/users/alice"
    })
    .to_string();
    let rotated = client
        .post(&endpoint)
        .headers(sign_headers(ROTATED_KEY, second_body.as_bytes()))
        .body(second_body)
        .send()
        .await?;
    assert_eq!(rotated.status(), reqwest::StatusCode::ACCEPTED);
    let duplicate = client
        .post(&endpoint)
        .headers(sign_headers(ROTATED_KEY, body.as_bytes()))
        .body(body.clone())
        .send()
        .await?;
    assert_eq!(duplicate.status(), reqwest::StatusCode::ACCEPTED);

    let conflicting_body = json!({
        "id": ACTIVITY_ID,
        "type": "Follow",
        "actor": ACTOR,
        "object": "https://fixture-v4-6-5.rustodon.invalid/users/moderator"
    })
    .to_string();
    let conflict = client
        .post(&endpoint)
        .headers(sign_headers(ROTATED_KEY, conflicting_body.as_bytes()))
        .body(conflicting_body)
        .send()
        .await?;
    assert_eq!(conflict.status(), reqwest::StatusCode::CONFLICT);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1",)
            .bind(ACTIVITYPUB_INBOX_JOB_KIND)
            .fetch_one(&runtime_pool)
            .await?,
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.idempotency_keys \
             WHERE scope = 'rustodon.activitypub.inbox'",
        )
        .fetch_one(&runtime_pool)
        .await?,
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.ordering_markers \
             WHERE kind = 'rustodon.activitypub.inbox'",
        )
        .fetch_one(&runtime_pool)
        .await?,
        1
    );
    let ordering_jobs = sqlx::query_as::<_, (i64, Option<i64>)>(
        "SELECT id, NULLIF(arguments ->> '_rustodon_ordering_predecessor', '')::bigint \
         FROM rustodon.durable_jobs WHERE kind = $1 ORDER BY run_at, id",
    )
    .bind(ACTIVITYPUB_INBOX_JOB_KIND)
    .fetch_all(&runtime_pool)
    .await?;
    assert_eq!(ordering_jobs.len(), 2);
    assert!(ordering_jobs[0].1.is_none());
    assert_eq!(ordering_jobs[1].1, Some(ordering_jobs[0].0));

    let _ = shutdown_tx.send(());
    server.await??;
    fs::remove_dir_all(media_root)?;
    sqlx::query("DELETE FROM keypairs WHERE id = $1")
        .bind(ROTATED_KEY_ROW)
        .execute(&owner_pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_follow_and_undo_are_processed_through_the_durable_worker()
-> Result<(), Box<dyn std::error::Error>> {
    const BOB: i64 = 116_844_606_259_202_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
    const KEY_ID: &str = "https://remote.fixture.invalid/users/bob#secondary-key";
    const FOLLOW_URI: &str = "https://remote.fixture.invalid/activities/worker-follow";
    const STALE_FOLLOW_URI: &str = "https://remote.fixture.invalid/activities/worker-stale-follow";
    const CURRENT_FOLLOW_URI: &str =
        "https://remote.fixture.invalid/activities/worker-current-follow";
    const PENDING_FOLLOW_URI: &str =
        "https://remote.fixture.invalid/activities/worker-pending-follow";
    const REJECT_FOLLOW_URI: &str =
        "https://remote.fixture.invalid/activities/worker-reject-follow";
    const INCOMING_FOLLOW_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/activities/worker-incoming-follow";
    const URI_ONLY_MISMATCHED_FOLLOW_URI: &str =
        "https://remote.fixture.invalid/activities/worker-uri-only-mismatched-follow";
    const BLOCK_URI: &str = "https://remote.fixture.invalid/activities/worker-block";
    const STALE_BLOCK_URI: &str = "https://remote.fixture.invalid/activities/worker-stale-block";
    const CURRENT_BLOCK_URI: &str =
        "https://remote.fixture.invalid/activities/worker-current-block";
    const BLOCK_BEFORE_FOLLOW_URI: &str =
        "https://remote.fixture.invalid/activities/worker-block-before-follow";
    const TARGET_URI: &str = "https://fixture-v4-6-5.rustodon.invalid/ap/users/116844606259201002";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    let queue = Queue::new(runtime_pool.clone());
    reset().await?;

    let baseline_stats = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        "SELECT account_id, following_count, followers_count, statuses_count
           FROM account_stats WHERE account_id = ANY($1) ORDER BY account_id",
    )
    .bind(vec![BOB, MODERATOR])
    .fetch_all(&writer_pool)
    .await?;
    let baseline_notifications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications
          WHERE account_id = $1 AND from_account_id = $2
            AND activity_type IN ('Follow', 'FollowRequest')",
    )
    .bind(MODERATOR)
    .bind(BOB)
    .fetch_one(&writer_pool)
    .await?;

    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
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
    };
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config.clone()),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let follow_body = json!({
        "id": FOLLOW_URI,
        "type": "Follow",
        "actor": ACTOR,
        "object": TARGET_URI
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": follow_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-follow"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert!(queue.dispatch_outbox(100).await? >= 1);
    while executor
        .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
        .await?
    {}
    let job_error = sqlx::query_scalar::<_, Option<String>>(
        "SELECT last_error FROM rustodon.durable_jobs WHERE logical_key = $1",
    )
    .bind("activitypub:test-worker-follow")
    .fetch_optional(&runtime_pool)
    .await?;
    assert!(
        job_error.as_ref().is_none_or(Option::is_none),
        "Follow job should complete instead of retrying: {job_error:?}"
    );

    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .bind(FOLLOW_URI)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "Follow was not applied"
    );
    let following_count: i64 =
        sqlx::query_scalar("SELECT following_count FROM account_stats WHERE account_id = $1")
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?;
    assert_eq!(
        following_count,
        baseline_stats
            .iter()
            .find(|row| row.0 == BOB)
            .expect("Bob stats should exist")
            .1
            + 1
    );
    let accept_body: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events \
          WHERE kind = $1 AND logical_key LIKE 'activitypub:accept:%'",
    )
    .bind("rustodon.activitypub.deliver")
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(accept_body["type"], "Accept");
    assert_eq!(accept_body["actor"], TARGET_URI);
    assert_eq!(accept_body["object"]["id"], FOLLOW_URI);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications
              WHERE account_id = $1 AND from_account_id = $2
                AND activity_type IN ('Follow', 'FollowRequest')",
        )
        .bind(MODERATOR)
        .bind(BOB)
        .fetch_one(&writer_pool)
        .await?,
        baseline_notifications + 1
    );

    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({
                        "id": FOLLOW_URI,
                        "type": "Follow",
                        "actor": ACTOR,
                        "object": TARGET_URI
                    })
                    .to_string(),
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-follow-duplicate"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications
              WHERE account_id = $1 AND from_account_id = $2
                AND activity_type IN ('Follow', 'FollowRequest')",
        )
        .bind(MODERATOR)
        .bind(BOB)
        .fetch_one(&writer_pool)
        .await?,
        baseline_notifications + 1
    );

    let undo_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": FOLLOW_URI
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": undo_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-undo"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .fetch_one(&writer_pool)
        .await?,
        0
    );

    for (activity_uri, logical_key) in [
        (STALE_FOLLOW_URI, "activitypub:test-worker-stale-follow"),
        (CURRENT_FOLLOW_URI, "activitypub:test-worker-current-follow"),
    ] {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": json!({
                            "id": activity_uri,
                            "type": "Follow",
                            "actor": ACTOR,
                            "object": TARGET_URI
                        })
                        .to_string(),
                        "delivery_target_account_id": MODERATOR,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key(logical_key),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "relationship-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
    }
    let stale_follow_undo_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Follow",
            "id": STALE_FOLLOW_URI,
            "actor": ACTOR,
            "object": TARGET_URI
        }
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": stale_follow_undo_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-stale-follow-undo"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows
               WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .bind(CURRENT_FOLLOW_URI)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "stale embedded Undo Follow must retain the newer activity"
    );
    let current_follow_undo_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Follow",
            "id": CURRENT_FOLLOW_URI,
            "actor": ACTOR,
            "object": TARGET_URI
        }
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": current_follow_undo_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-current-follow-undo"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .fetch_one(&writer_pool)
        .await?,
        0,
        "matching embedded Undo Follow must remove the current activity"
    );

    sqlx::query("UPDATE accounts SET locked = true WHERE id = $1")
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;
    let pending_body = json!({
        "id": PENDING_FOLLOW_URI,
        "type": "Follow",
        "actor": ACTOR,
        "object": TARGET_URI
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": pending_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-pending-follow"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follow_requests
              WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .bind(PENDING_FOLLOW_URI)
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT following_count FROM account_stats WHERE account_id = $1",
        )
        .bind(BOB)
        .fetch_one(&writer_pool)
        .await?,
        baseline_stats
            .iter()
            .find(|row| row.0 == BOB)
            .expect("Bob stats should exist")
            .1
    );
    assert!(queue.dispatch_outbox(100).await? >= 1);
    while executor
        .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
        .await?
    {}
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications
              WHERE account_id = $1 AND from_account_id = $2
                AND activity_type = 'FollowRequest'",
        )
        .bind(MODERATOR)
        .bind(BOB)
        .fetch_one(&writer_pool)
        .await?,
        baseline_notifications + 1
    );
    let mismatched_target_undo_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Follow",
            "id": PENDING_FOLLOW_URI,
            "actor": ACTOR,
            "object": "https://fixture-v4-6-5.rustodon.invalid/ap/users/missing"
        }
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": mismatched_target_undo_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-pending-undo-unknown-target"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follow_requests
              WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "an embedded Undo target that is not local must not fall back to its URI"
    );
    let pending_undo_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Follow",
            "id": PENDING_FOLLOW_URI,
            "actor": ACTOR,
            "object": TARGET_URI
        }
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": pending_undo_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-pending-undo"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follow_requests
              WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .fetch_one(&writer_pool)
        .await?,
        0
    );
    sqlx::query("UPDATE accounts SET locked = false WHERE id = $1")
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;

    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({
                        "id": FOLLOW_URI,
                        "type": "Follow",
                        "actor": ACTOR,
                        "object": TARGET_URI
                    })
                    .to_string(),
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-follow-retry"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .fetch_one(&writer_pool)
        .await?,
        0,
        "Undo-before-Follow must leave no relationship"
    );

    sqlx::query(
        "INSERT INTO blocks (account_id, created_at, target_account_id, updated_at, uri)
         VALUES ($1, clock_timestamp(), $2, clock_timestamp(), $3)
         ON CONFLICT (account_id, target_account_id) DO NOTHING",
    )
    .bind(BOB)
    .bind(MODERATOR)
    .bind("https://remote.fixture.invalid/activities/fixture-block")
    .execute(&writer_pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({
                        "id": REJECT_FOLLOW_URI,
                        "type": "Follow",
                        "actor": ACTOR,
                        "object": TARGET_URI
                    })
                    .to_string(),
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-reject-follow"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    let reject_body: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events \
          WHERE kind = 'rustodon.activitypub.deliver' \
            AND payload -> 'arguments' -> 'body' ->> 'type' = 'Reject'",
    )
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(reject_body["object"]["id"], REJECT_FOLLOW_URI);
    assert_eq!(reject_body["id"], format!("{TARGET_URI}#rejects/follows/"));
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2")
        .bind(BOB)
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;

    let null_uri_follow_request_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO follow_requests (
           account_id, target_account_id, show_reblogs, notify, languages, uri,
           created_at, updated_at)
         VALUES ($1, $2, true, false, NULL, NULL, clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .bind(MODERATOR)
    .bind(BOB)
    .fetch_one(&writer_pool)
    .await?;
    let mismatched_uri_only_accept = json!({
        "type": "Accept",
        "actor": ACTOR,
        "object": URI_ONLY_MISMATCHED_FOLLOW_URI
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": mismatched_uri_only_accept,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-accept-null-uri-mismatch"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follow_requests WHERE id = $1 AND uri IS NULL",
        )
        .bind(null_uri_follow_request_id)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "URI-only Accept must not match a NULL-URI follow request"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(MODERATOR)
        .bind(BOB)
        .fetch_one(&writer_pool)
        .await?,
        0,
        "URI-only Accept must not create a follow from a NULL-URI row"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT following_count FROM account_stats WHERE account_id = $1"
        )
        .bind(MODERATOR)
        .fetch_one(&writer_pool)
        .await?,
        baseline_stats
            .iter()
            .find(|row| row.0 == MODERATOR)
            .expect("Moderator stats should exist")
            .1
    );

    let mismatched_uri_only_reject = json!({
        "type": "Reject",
        "actor": ACTOR,
        "object": URI_ONLY_MISMATCHED_FOLLOW_URI
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": mismatched_uri_only_reject,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-reject-null-uri-mismatch"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follow_requests WHERE id = $1 AND uri IS NULL",
        )
        .bind(null_uri_follow_request_id)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "URI-only Reject must not match a NULL-URI follow request"
    );
    sqlx::query("DELETE FROM follow_requests WHERE id = $1")
        .bind(null_uri_follow_request_id)
        .execute(&writer_pool)
        .await?;

    sqlx::query(
        "INSERT INTO follow_requests (
           account_id, target_account_id, show_reblogs, notify, languages, uri,
           created_at, updated_at)
         VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(MODERATOR)
    .bind(BOB)
    .bind(INCOMING_FOLLOW_URI)
    .execute(&writer_pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({
                        "type": "Accept",
                        "actor": ACTOR,
                        "object": {
                            "type": "Follow",
                            "id": INCOMING_FOLLOW_URI,
                            "actor": TARGET_URI,
                            "object": ACTOR
                        }
                    })
                    .to_string(),
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-accept-follow"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows
              WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(MODERATOR)
        .bind(BOB)
        .bind(INCOMING_FOLLOW_URI)
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({
                        "type": "Accept",
                        "actor": ACTOR,
                        "object": {
                            "type": "Follow",
                            "id": INCOMING_FOLLOW_URI,
                            "actor": TARGET_URI,
                            "object": ACTOR
                        }
                    })
                    .to_string(),
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-accept-follow-duplicate"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows
               WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(MODERATOR)
        .bind(BOB)
        .bind(INCOMING_FOLLOW_URI)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "duplicate Accept must not create another follow"
    );
    let reject_after_accept = json!({
        "type": "Reject",
        "actor": ACTOR,
        "object": INCOMING_FOLLOW_URI
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": reject_after_accept,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-reject-accepted-follow"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(MODERATOR)
        .bind(BOB)
        .fetch_one(&writer_pool)
        .await?,
        0
    );
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({
                        "type": "Reject",
                        "actor": ACTOR,
                        "object": INCOMING_FOLLOW_URI
                    })
                    .to_string(),
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-reject-accepted-follow-duplicate"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(MODERATOR)
        .bind(BOB)
        .fetch_one(&writer_pool)
        .await?,
        0,
        "duplicate Reject must preserve the rejected state"
    );

    let block_body = json!({
        "id": BLOCK_URI,
        "type": "Block",
        "actor": ACTOR,
        "object": TARGET_URI
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": block_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-block"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM blocks
              WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .bind(BLOCK_URI)
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({
                        "id": BLOCK_URI,
                        "type": "Block",
                        "actor": ACTOR,
                        "object": TARGET_URI
                    })
                    .to_string(),
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-block-duplicate"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM blocks
               WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .bind(BLOCK_URI)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "duplicate Block must not create another relationship"
    );
    let block_undo_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Block",
            "id": BLOCK_URI,
            "actor": ACTOR,
            "object": TARGET_URI
        }
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": block_undo_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-undo-block"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM blocks
              WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .bind(BLOCK_URI)
        .fetch_one(&writer_pool)
        .await?,
        0
    );
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({
                        "type": "Undo",
                        "actor": ACTOR,
                        "object": {
                            "type": "Block",
                            "id": BLOCK_URI,
                            "actor": ACTOR,
                            "object": TARGET_URI
                        }
                    })
                    .to_string(),
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-undo-block-duplicate"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM blocks WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .fetch_one(&writer_pool)
        .await?,
        0,
        "duplicate Undo Block must preserve the removed state"
    );

    for (activity_uri, logical_key) in [
        (STALE_BLOCK_URI, "activitypub:test-worker-stale-block"),
        (CURRENT_BLOCK_URI, "activitypub:test-worker-current-block"),
    ] {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": json!({
                            "id": activity_uri,
                            "type": "Block",
                            "actor": ACTOR,
                            "object": TARGET_URI
                        })
                        .to_string(),
                        "delivery_target_account_id": MODERATOR,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key(logical_key),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "relationship-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
    }
    let stale_block_undo_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Block",
            "id": STALE_BLOCK_URI,
            "actor": ACTOR,
            "object": TARGET_URI
        }
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": stale_block_undo_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-stale-block-undo"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM blocks
              WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .bind(CURRENT_BLOCK_URI)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "stale embedded Undo Block must retain the newer activity"
    );
    let current_block_undo_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Block",
            "id": CURRENT_BLOCK_URI,
            "actor": ACTOR,
            "object": TARGET_URI
        }
    })
    .to_string();
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": current_block_undo_body,
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-current-block-undo"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM blocks WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .fetch_one(&writer_pool)
        .await?,
        0,
        "matching embedded Undo Block must remove the current activity"
    );

    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({
                        "type": "Undo",
                        "actor": ACTOR,
                        "object": {
                            "type": "Block",
                            "id": BLOCK_BEFORE_FOLLOW_URI,
                            "actor": ACTOR,
                            "object": TARGET_URI
                        }
                    })
                    .to_string(),
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-undo-block-before-follow"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({
                        "id": BLOCK_BEFORE_FOLLOW_URI,
                        "type": "Block",
                        "actor": ACTOR,
                        "object": TARGET_URI
                    })
                    .to_string(),
                    "delivery_target_account_id": MODERATOR,
                    "signature_key_id": KEY_ID,
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:test-worker-block-after-undo"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "relationship-worker",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM blocks
              WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .bind(BLOCK_BEFORE_FOLLOW_URI)
        .fetch_one(&writer_pool)
        .await?,
        0
    );

    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(BOB)
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2 \
         AND uri = ANY($3)",
    )
    .bind(BOB)
    .bind(MODERATOR)
    .bind(vec![BLOCK_URI, BLOCK_BEFORE_FOLLOW_URI, REJECT_FOLLOW_URI])
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
        .bind(BOB)
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "DELETE FROM notifications
          WHERE account_id = $1 AND from_account_id = $2
            AND activity_type IN ('Follow', 'FollowRequest')",
    )
    .bind(MODERATOR)
    .bind(BOB)
    .execute(&writer_pool)
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications
              WHERE account_id = $1 AND from_account_id = $2
                AND activity_type IN ('Follow', 'FollowRequest')",
        )
        .bind(MODERATOR)
        .bind(BOB)
        .fetch_one(&writer_pool)
        .await?,
        baseline_notifications
    );
    for (account_id, following, followers, statuses) in baseline_stats {
        sqlx::query(
            "UPDATE account_stats
                SET following_count = $2, followers_count = $3, statuses_count = $4
              WHERE account_id = $1",
        )
        .bind(account_id)
        .bind(following)
        .bind(followers)
        .bind(statuses)
        .execute(&writer_pool)
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_flag_reports_honor_remote_domain_rejection()
-> Result<(), Box<dyn std::error::Error>> {
    const ALICE: i64 = 116_844_606_259_201_001;
    const BOB: i64 = 116_844_606_259_202_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
    const KEY_ID: &str = "https://remote.fixture.invalid/users/bob#secondary-key";
    const REMOTE_DOMAIN: &str = "remote.fixture.invalid";
    const TARGET_URI: &str = "https://fixture-v4-6-5.rustodon.invalid/ap/users/116844606259201002";
    const ALICE_TARGET_URI: &str = "https://fixture-v4-6-5.rustodon.invalid/users/alice";
    const ACCEPTED_FLAG_URI: &str = "https://remote.fixture.invalid/activities/worker-flag";
    const SUSPENDED_FLAG_URI: &str =
        "https://remote.fixture.invalid/activities/worker-suspended-flag";
    const REJECTED_FLAG_URI: &str =
        "https://remote.fixture.invalid/activities/worker-rejected-flag";
    const FLAG_STATUS_ID: i64 = -9510;
    const FLAG_STATUS_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/users/moderator/statuses/worker-flag";
    const FLAG_PRIVATE_STATUS_ID: i64 = -9511;
    const FLAG_PRIVATE_STATUS_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/users/moderator/statuses/worker-private-flag";
    const FLAG_DIRECT_STATUS_ID: i64 = -9512;
    const FLAG_DIRECT_STATUS_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/users/moderator/statuses/worker-direct-flag";
    const FLAG_MENTIONED_STATUS_ID: i64 = -9513;
    const FLAG_MENTIONED_STATUS_URI: &str =
        "tag:fixture-v4-6-5.rustodon.invalid,2026:worker-mentioned-flag";
    const FLAG_MENTION_ID: i64 = -9514;
    const FLAG_ALICE_STATUS_ID: i64 = -9515;
    const FLAG_ALICE_STATUS_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/users/alice/statuses/worker-flag";
    const FLAG_GENERATED_STATUS_ID: i64 = -9516;
    const FLAG_GENERATED_STATUS_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/ap/users/116844606259201002/statuses/-9516";
    const FLAG_TAG_STATUS_ID: i64 = 116_846_257_766_400_502;
    const FLAG_TAG_STATUS_URI: &str =
        "tag:fixture-v4-6-5.rustodon.invalid;objectId=116846257766400502:objectType=Status";
    const FLAG_GENERATED_REBLOG_STATUS_ID: i64 = -9518;
    const FLAG_GENERATED_REBLOG_STATUS_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/@moderator/-9518/activity";
    const DOMAIN_BLOCK_ID: i64 = -9508;
    const FLAG_COLLECTION_ID: i64 = -9509;
    const FLAG_COLLECTION_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/collections/worker-flag";
    const FLAG_GENERATED_COLLECTION_ID: i64 = -9517;
    const FLAG_GENERATED_COLLECTION_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/ap/users/116844606259201002/collections/-9517";
    const FLAG_TAG_COLLECTION_ID: i64 = 9981;
    const FLAG_TAG_COLLECTION_URI: &str =
        "tag:fixture-v4-6-5.rustodon.invalid;objectId=9981:objectType=Collection";
    const FLAG_WEB_COLLECTION_ID: i64 = 9982;
    const FLAG_WEB_COLLECTION_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/collections/9982";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;

    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
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
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let original_domain_block: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(domain_block) FROM domain_blocks domain_block WHERE domain = $1",
    )
    .bind(REMOTE_DOMAIN)
    .fetch_optional(&writer_pool)
    .await?;
    let original_source_suspended_at: Option<NaiveDateTime> =
        sqlx::query_scalar("SELECT suspended_at FROM accounts WHERE id = $1")
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?;
    for (status_id, account_id, status_uri, visibility) in [
        (FLAG_STATUS_ID, MODERATOR, FLAG_STATUS_URI, 0_i32),
        (
            FLAG_PRIVATE_STATUS_ID,
            MODERATOR,
            FLAG_PRIVATE_STATUS_URI,
            2_i32,
        ),
        (
            FLAG_DIRECT_STATUS_ID,
            MODERATOR,
            FLAG_DIRECT_STATUS_URI,
            3_i32,
        ),
        (
            FLAG_MENTIONED_STATUS_ID,
            MODERATOR,
            FLAG_MENTIONED_STATUS_URI,
            3_i32,
        ),
        (FLAG_ALICE_STATUS_ID, ALICE, FLAG_ALICE_STATUS_URI, 0_i32),
    ] {
        sqlx::query(
            "INSERT INTO statuses (
                 id, account_id, created_at, local, quote_approval_policy, reply, sensitive,
                 spoiler_text, text, updated_at, uri, url, visibility)
             VALUES ($1, $2, clock_timestamp(), true, 0, false, false, '',
                     'Flag target fixture status', clock_timestamp(), $3, $3, $4)",
        )
        .bind(status_id)
        .bind(account_id)
        .bind(status_uri)
        .bind(visibility)
        .execute(&writer_pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO statuses (
             id, account_id, created_at, local, quote_approval_policy, reply, sensitive,
             spoiler_text, text, updated_at, visibility)
         VALUES ($1, $2, clock_timestamp(), true, 0, false, false, '',
                 'Generated Flag target fixture status', clock_timestamp(), 0)",
    )
    .bind(FLAG_GENERATED_STATUS_ID)
    .bind(MODERATOR)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO statuses (
             id, account_id, created_at, local, quote_approval_policy, reblog_of_id,
             reply, sensitive, spoiler_text, text, updated_at, visibility)
         VALUES ($1, $2, clock_timestamp(), true, 0, $3, false, false, '',
                 'Generated reblog Flag target status', clock_timestamp(), 0)",
    )
    .bind(FLAG_GENERATED_REBLOG_STATUS_ID)
    .bind(MODERATOR)
    .bind(FLAG_STATUS_ID)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO statuses (
             id, account_id, created_at, local, quote_approval_policy, reply, sensitive,
             spoiler_text, text, updated_at, visibility)
         VALUES ($1, $2, clock_timestamp(), true, 0, false, false, '',
                 'Tag Flag target fixture status', clock_timestamp(), 0)",
    )
    .bind(FLAG_TAG_STATUS_ID)
    .bind(MODERATOR)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO mentions (id, account_id, created_at, silent, status_id, updated_at)
         VALUES ($1, $2, clock_timestamp(), false, $3, clock_timestamp())",
    )
    .bind(FLAG_MENTION_ID)
    .bind(BOB)
    .bind(FLAG_MENTIONED_STATUS_ID)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO collections (
             id, account_id, created_at, discoverable, item_count, local, name,
             sensitive, updated_at)
         VALUES ($1, $2, clock_timestamp(), false, 0, true, 'Generated Flag collection',
                 false, clock_timestamp())",
    )
    .bind(FLAG_GENERATED_COLLECTION_ID)
    .bind(MODERATOR)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO collections (
             id, account_id, created_at, discoverable, item_count, local, name,
             sensitive, updated_at)
         VALUES ($1, $2, clock_timestamp(), false, 0, true, 'Tag Flag collection',
                 false, clock_timestamp())",
    )
    .bind(FLAG_TAG_COLLECTION_ID)
    .bind(MODERATOR)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO collections (
             id, account_id, created_at, discoverable, item_count, local, name,
             sensitive, updated_at, uri, url)
         VALUES ($1, $2, clock_timestamp(), false, 0, true, 'Flag collection',
                 false, clock_timestamp(), $3, $3)",
    )
    .bind(FLAG_COLLECTION_ID)
    .bind(MODERATOR)
    .bind(FLAG_COLLECTION_URI)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO collections (
             id, account_id, created_at, discoverable, item_count, local, name,
             sensitive, updated_at)
         VALUES ($1, $2, clock_timestamp(), false, 0, true, 'Web Flag collection',
                 false, clock_timestamp())",
    )
    .bind(FLAG_WEB_COLLECTION_ID)
    .bind(MODERATOR)
    .execute(&writer_pool)
    .await?;
    let result = async {
        let accepted_body = json!({
            "id": ACCEPTED_FLAG_URI,
            "type": "Flag",
            "actor": ACTOR,
            "object": [
                TARGET_URI,
                FLAG_STATUS_URI,
                FLAG_GENERATED_STATUS_URI,
                FLAG_TAG_STATUS_URI,
                FLAG_GENERATED_REBLOG_STATUS_URI,
                FLAG_PRIVATE_STATUS_URI,
                FLAG_DIRECT_STATUS_URI,
                FLAG_MENTIONED_STATUS_URI,
                FLAG_COLLECTION_URI,
                FLAG_GENERATED_COLLECTION_URI,
                FLAG_TAG_COLLECTION_URI,
                FLAG_WEB_COLLECTION_URI,
                ALICE_TARGET_URI,
                FLAG_ALICE_STATUS_URI
            ],
            "content": "Remote account report"
        })
        .to_string();
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": accepted_body,
                        "delivery_target_account_id": MODERATOR,
                        "signature_key_id": KEY_ID,
                        "remote_domain": REMOTE_DOMAIN
                    }),
                )
                .logical_key("activitypub:test-worker-flag"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "relationship-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        let job_error = sqlx::query_scalar::<_, Option<String>>(
            "SELECT last_error FROM rustodon.durable_jobs WHERE logical_key = $1",
        )
        .bind("activitypub:test-worker-flag")
        .fetch_optional(&runtime_pool)
        .await?;
        assert!(
            job_error.as_ref().is_none_or(Option::is_none),
            "Flag job should complete instead of retrying: {job_error:?}"
        );
        let report = sqlx::query_as::<_, (i64, i32, String, Vec<i64>, bool)>(
            "SELECT id, category, comment, status_ids, forwarded
               FROM reports
              WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(BOB)
        .bind(MODERATOR)
        .bind(ACCEPTED_FLAG_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(report.1, 0);
        assert_eq!(report.2, "Remote account report");
        assert_eq!(
            report.3,
            vec![
                FLAG_STATUS_ID,
                FLAG_GENERATED_STATUS_ID,
                FLAG_TAG_STATUS_ID,
                FLAG_GENERATED_REBLOG_STATUS_ID,
                FLAG_MENTIONED_STATUS_ID
            ]
        );
        assert!(!report.4);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM collection_reports
                  WHERE report_id = $1 AND collection_id = $2",
            )
            .bind(report.0)
            .bind(FLAG_COLLECTION_ID)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM collection_reports
                   WHERE report_id = $1 AND collection_id = ANY($2)",
            )
            .bind(report.0)
            .bind(vec![
                FLAG_COLLECTION_ID,
                FLAG_GENERATED_COLLECTION_ID,
                FLAG_TAG_COLLECTION_ID,
                FLAG_WEB_COLLECTION_ID,
            ])
            .fetch_one(&writer_pool)
            .await?,
            4
        );
        let alice_report = sqlx::query_as::<_, (i32, Vec<i64>)>(
            "SELECT category, status_ids
               FROM reports
              WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
        )
        .bind(BOB)
        .bind(ALICE)
        .bind(ACCEPTED_FLAG_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(alice_report.0, 0);
        assert_eq!(alice_report.1, vec![FLAG_ALICE_STATUS_ID]);
        assert!(queue.dispatch_outbox(100).await? >= 1);
        while executor
            .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
            .await?
        {}
        assert!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications
                  WHERE activity_id = $1 AND activity_type = 'Report'",
            )
            .bind(report.0)
            .fetch_one(&writer_pool)
            .await?
                > 0
        );

        sqlx::query("UPDATE accounts SET suspended_at = clock_timestamp() WHERE id = $1")
            .bind(BOB)
            .execute(&writer_pool)
            .await?;
        let suspended_body = json!({
            "id": SUSPENDED_FLAG_URI,
            "type": "Flag",
            "actor": ACTOR,
            "object": TARGET_URI,
            "content": "A suspended actor must not report"
        })
        .to_string();
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": suspended_body,
                        "delivery_target_account_id": MODERATOR,
                        "signature_key_id": KEY_ID,
                        "remote_domain": REMOTE_DOMAIN
                    }),
                )
                .logical_key("activitypub:test-worker-suspended-flag"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "relationship-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM reports
                  WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
            )
            .bind(BOB)
            .bind(MODERATOR)
            .bind(SUSPENDED_FLAG_URI)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        sqlx::query("UPDATE accounts SET suspended_at = $2 WHERE id = $1")
            .bind(BOB)
            .bind(original_source_suspended_at)
            .execute(&writer_pool)
            .await?;

        sqlx::query(
            "INSERT INTO domain_blocks (
                 id, domain, severity, reject_media, reject_reports, private_comment,
                 public_comment, obfuscate, created_at, updated_at)
             VALUES ($1, $2, 2, false, true, NULL, NULL, false,
                     clock_timestamp(), clock_timestamp())
             ON CONFLICT (domain) DO UPDATE SET
                 severity = 2, reject_media = false, reject_reports = true,
                 updated_at = clock_timestamp()",
        )
        .bind(DOMAIN_BLOCK_ID)
        .bind(REMOTE_DOMAIN)
        .execute(&writer_pool)
        .await?;
        let rejected_body = json!({
            "id": REJECTED_FLAG_URI,
            "type": "Flag",
            "actor": ACTOR,
            "object": TARGET_URI,
            "content": "This report must be rejected"
        })
        .to_string();
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": rejected_body,
                        "delivery_target_account_id": MODERATOR,
                        "signature_key_id": KEY_ID,
                        "remote_domain": REMOTE_DOMAIN
                    }),
                )
                .logical_key("activitypub:test-worker-rejected-flag"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "relationship-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM reports
                  WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
            )
            .bind(BOB)
            .bind(MODERATOR)
            .bind(REJECTED_FLAG_URI)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    let flag_uris = vec![ACCEPTED_FLAG_URI, SUSPENDED_FLAG_URI, REJECTED_FLAG_URI];
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE kind = $2
            AND payload ->> 'event' = 'notification'
            AND payload ->> 'object_id' IN (
                SELECT notification.id::text
                  FROM notifications notification
                 WHERE notification.activity_type = 'Report'
                   AND notification.activity_id IN (
                       SELECT report.id FROM reports report WHERE report.uri = ANY($1)))",
    )
    .bind(flag_uris.clone())
    .bind(STREAM_EVENT_KIND)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE (payload -> 'arguments' ->> 'report_id' IN (
                    SELECT id::text FROM reports WHERE uri = ANY($1))
             OR (kind = $2 AND payload -> 'arguments' ->> 'activity_id' IN (
                    SELECT id::text FROM reports WHERE uri = ANY($1))))",
    )
    .bind(flag_uris.clone())
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.durable_jobs
          WHERE arguments ->> 'report_id' IN (
                    SELECT id::text FROM reports WHERE uri = ANY($1))
             OR arguments ->> 'activity_id' IN (
                    SELECT id::text FROM reports WHERE uri = ANY($1))",
    )
    .bind(flag_uris.clone())
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "DELETE FROM notifications
          WHERE activity_id IN (SELECT id FROM reports WHERE uri = ANY($1))
            AND activity_type = 'Report'",
    )
    .bind(flag_uris.clone())
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "DELETE FROM collection_reports
          WHERE report_id IN (SELECT id FROM reports WHERE uri = ANY($1))",
    )
    .bind(flag_uris.clone())
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM reports WHERE uri = ANY($1)")
        .bind(flag_uris)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM collections WHERE id = $1")
        .bind(FLAG_COLLECTION_ID)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM collections WHERE id = $1")
        .bind(FLAG_GENERATED_COLLECTION_ID)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM collections WHERE id = $1")
        .bind(FLAG_TAG_COLLECTION_ID)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM collections WHERE id = $1")
        .bind(FLAG_WEB_COLLECTION_ID)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM mentions WHERE id = $1")
        .bind(FLAG_MENTION_ID)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
        .bind(vec![
            FLAG_STATUS_ID,
            FLAG_PRIVATE_STATUS_ID,
            FLAG_DIRECT_STATUS_ID,
            FLAG_MENTIONED_STATUS_ID,
            FLAG_ALICE_STATUS_ID,
            FLAG_GENERATED_STATUS_ID,
            FLAG_TAG_STATUS_ID,
            FLAG_GENERATED_REBLOG_STATUS_ID,
        ])
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM domain_blocks WHERE domain = $1")
        .bind(REMOTE_DOMAIN)
        .execute(&writer_pool)
        .await?;
    if let Some(original_domain_block) = original_domain_block {
        sqlx::query(
            "INSERT INTO domain_blocks
             SELECT * FROM jsonb_populate_record(NULL::domain_blocks, $1)",
        )
        .bind(original_domain_block)
        .execute(&writer_pool)
        .await?;
    }
    sqlx::query("UPDATE accounts SET suspended_at = $2 WHERE id = $1")
        .bind(BOB)
        .bind(original_source_suspended_at)
        .execute(&writer_pool)
        .await?;
    result?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn mastodon_suspended_authors_do_not_create_delayed_notifications()
-> Result<(), Box<dyn std::error::Error>> {
    const ALICE: i64 = 116_844_606_259_201_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const API_MODERATOR: i64 = 116_844_606_259_201_004;
    const REMOTE_MENTION: i64 = -330;

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    let queue = Queue::new(runtime_pool);
    reset().await?;

    let baseline_last_status_at = sqlx::query_scalar::<_, Option<NaiveDateTime>>(
        "SELECT last_status_at FROM account_stats WHERE account_id = $1",
    )
    .bind(ALICE)
    .fetch_one(&writer_pool)
    .await?;
    let baseline_account_suspension = sqlx::query_as::<_, (Option<NaiveDateTime>, Option<i32>)>(
        "SELECT suspended_at, suspension_origin FROM accounts WHERE id = $1",
    )
    .bind(ALICE)
    .fetch_one(&writer_pool)
    .await?;
    let baseline_notification_requests = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notification_requests \
          WHERE account_id = $1 AND from_account_id = $2",
    )
    .bind(MODERATOR)
    .bind(ALICE)
    .fetch_one(&writer_pool)
    .await?;

    let writer = WriteRepository::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&owner_url).await?);
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let direct_status = writer
        .create_status(
            &authenticated,
            "@moderator direct deletion",
            &[],
            None,
            Some(false),
            Some("direct"),
            None,
            None,
            None,
        )
        .await?;
    writer
        .delete_status(&authenticated, direct_status.status_id, false)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1 AND payload ->> 'event' = 'delete'
                AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(API_MODERATOR.to_string())
        .bind(direct_status.status_id.to_string())
        .fetch_one(&writer_pool)
        .await?,
        0,
        "a direct-status delete must not reach an unrelated follower"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1 AND payload ->> 'event' = 'delete'
                AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(MODERATOR.to_string())
        .bind(direct_status.status_id.to_string())
        .fetch_one(&writer_pool)
        .await?,
        1,
        "a direct-status delete must reach an explicitly mentioned account"
    );

    let remote_mention_status = writer
        .create_status(
            &authenticated,
            "@timeline_author@remote.fixture.invalid remote deletion",
            &[],
            None,
            Some(false),
            Some("public"),
            None,
            None,
            None,
        )
        .await?;
    writer
        .delete_status(&authenticated, remote_mention_status.status_id, false)
        .await?;
    let delete_arguments: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' FROM rustodon.outbox_events
          WHERE kind = $1 AND payload -> 'arguments' ->> 'status_id' = $2
            AND payload -> 'arguments' ->> 'activity_type' = 'Delete'",
    )
    .bind(rustodon::jobs::ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(remote_mention_status.status_id.to_string())
    .fetch_one(&writer_pool)
    .await?;
    let delete_recipient_ids = delete_arguments["recipient_account_ids"]
        .as_array()
        .ok_or("status delete recipients are not an array")?;
    assert!(
        delete_recipient_ids
            .iter()
            .any(|recipient| recipient.as_i64() == Some(REMOTE_MENTION)),
        "a remote mention must remain in local Delete reach after soft deletion"
    );

    let status = writer
        .create_status(
            &authenticated,
            "@moderator delayed suspension mention",
            &[],
            None,
            Some(false),
            Some("public"),
            None,
            None,
            None,
        )
        .await?;
    let mention_id: i64 = sqlx::query_scalar(
        "SELECT id FROM mentions WHERE status_id = $1 AND account_id = $2 ORDER BY id LIMIT 1",
    )
    .bind(status.status_id)
    .bind(MODERATOR)
    .fetch_one(&writer_pool)
    .await?;

    sqlx::query(
        "UPDATE accounts SET suspended_at = clock_timestamp(), suspension_origin = 0 \
          WHERE id = $1",
    )
    .bind(ALICE)
    .execute(&writer_pool)
    .await?;

    let handlers = infrastructure_handlers_with_writer(&queue, Some(writer_pool.clone()))?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    assert!(queue.dispatch_outbox(100).await? >= 2);
    while executor
        .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
        .await?
    {}

    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
              WHERE account_id = $1 AND activity_id = $2 AND activity_type = 'Mention'",
        )
        .bind(MODERATOR)
        .bind(mention_id)
        .fetch_one(&writer_pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notification_requests \
              WHERE account_id = $1 AND from_account_id = $2",
        )
        .bind(MODERATOR)
        .bind(ALICE)
        .fetch_one(&writer_pool)
        .await?,
        baseline_notification_requests
    );

    sqlx::query("UPDATE accounts SET suspended_at = $2, suspension_origin = $3 WHERE id = $1")
        .bind(ALICE)
        .bind(baseline_account_suspension.0)
        .bind(baseline_account_suspension.1)
        .execute(&writer_pool)
        .await?;
    writer
        .delete_status(&authenticated, status.status_id, false)
        .await?;
    sqlx::query("UPDATE account_stats SET last_status_at = $2 WHERE account_id = $1")
        .bind(ALICE)
        .bind(baseline_last_status_at)
        .execute(&writer_pool)
        .await?;
    reset().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn mastodon_notification_outbox_jobs_are_delivered_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    const ALICE: i64 = 116_844_606_259_201_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const API_MODERATOR: i64 = 116_844_606_259_201_004;

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    let queue = Queue::new(runtime_pool);
    reset().await?;
    let baseline_last_status_at = sqlx::query_scalar::<_, Option<NaiveDateTime>>(
        "SELECT last_status_at FROM account_stats WHERE account_id = $1",
    )
    .bind(ALICE)
    .fetch_one(&writer_pool)
    .await?;
    let original_notification_request = sqlx::query_as::<
        _,
        (i64, i64, Option<i64>, i64, NaiveDateTime, NaiveDateTime),
    >(
        "SELECT account_id, from_account_id, last_status_id, notifications_count, created_at, updated_at \
         FROM notification_requests WHERE id = -95",
    )
    .fetch_one(&writer_pool)
    .await?;
    let baseline_moderator_settings =
        sqlx::query_scalar::<_, Option<String>>("SELECT settings FROM users WHERE account_id = $1")
            .bind(MODERATOR)
            .fetch_one(&writer_pool)
            .await?;
    sqlx::query("UPDATE users SET settings = $2 WHERE account_id = $1")
        .bind(MODERATOR)
        .bind(r#"{"notification_emails.report":true}"#)
        .execute(&writer_pool)
        .await?;

    let writer = WriteRepository::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&owner_url).await?);
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let status = writer
        .create_status(
            &authenticated,
            "@moderator durable worker mention",
            &[],
            None,
            Some(false),
            Some("public"),
            None,
            None,
            None,
        )
        .await?;
    let mention_id: i64 = sqlx::query_scalar(
        "SELECT id FROM mentions WHERE status_id = $1 AND account_id = $2 ORDER BY id LIMIT 1",
    )
    .bind(status.status_id)
    .bind(MODERATOR)
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
              WHERE account_id = $1 AND activity_id = $2 AND activity_type = 'Mention'",
        )
        .bind(MODERATOR)
        .bind(mention_id)
        .fetch_one(&writer_pool)
        .await?,
        0
    );

    let follow_authenticated = authenticator.authenticate(&headers, WRITE_FOLLOWS).await?;
    let follow = writer
        .set_follow(&follow_authenticated, API_MODERATOR, true, None, None, None)
        .await?;
    let follow_id = follow.activity_id.expect("new follow has an activity id");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
             WHERE account_id = $1 AND activity_id = $2 AND activity_type = 'Follow'",
        )
        .bind(API_MODERATOR)
        .bind(follow_id)
        .fetch_one(&writer_pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events \
             WHERE kind = $1 \
               AND payload -> 'arguments' ->> 'recipient_account_id' = $2 \
               AND payload -> 'arguments' ->> 'activity_type' = 'follow' \
               AND payload -> 'arguments' ->> 'activity_id' = $3",
        )
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(API_MODERATOR.to_string())
        .bind(follow_id.to_string())
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    writer
        .set_follow(
            &follow_authenticated,
            API_MODERATOR,
            false,
            None,
            None,
            None,
        )
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events \
             WHERE kind = $1 \
               AND payload -> 'arguments' ->> 'recipient_account_id' = $2 \
               AND payload -> 'arguments' ->> 'activity_type' = 'follow' \
               AND payload -> 'arguments' ->> 'activity_id' = $3",
        )
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(API_MODERATOR.to_string())
        .bind(follow_id.to_string())
        .fetch_one(&writer_pool)
        .await?,
        0
    );
    let follow = writer
        .set_follow(&follow_authenticated, API_MODERATOR, true, None, None, None)
        .await?;
    let follow_id = follow
        .activity_id
        .expect("replacement follow has an activity id");
    let report_authenticated = authenticator.authenticate(&headers, WRITE_REPORTS).await?;
    let report_id = writer
        .create_report(
            &report_authenticated,
            116_844_606_259_202_002,
            "durable worker report",
            None,
            &[],
            &[],
            &[],
            None,
            None,
            "https://fixture-v4-6-5.rustodon.invalid/",
            true,
        )
        .await?;
    let report_mail_arguments: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' FROM rustodon.outbox_events \
         WHERE kind = $1 AND payload -> 'arguments' ->> 'report_id' = $2 \
           AND payload -> 'arguments' ->> 'to' = $3",
    )
    .bind(REPORT_JOB_KIND)
    .bind(report_id.to_string())
    .bind("moderator@fixture.invalid")
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(
        report_mail_arguments["target"],
        "carol@remote.fixture.invalid"
    );
    assert_eq!(report_mail_arguments["reporter"], "alice");
    let notification_authenticated = authenticator
        .authenticate(&headers, WRITE_NOTIFICATIONS)
        .await?;
    writer
        .accept_notification_request(&notification_authenticated, -95)
        .await?;
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT filtered FROM notifications WHERE id = $1")
            .bind(10021_i64)
            .fetch_one(&writer_pool)
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events \
              WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2 \
                AND payload -> 'arguments' ->> 'from_account_id' = $3",
        )
        .bind(NOTIFICATION_UNFILTER_JOB_KIND)
        .bind(ALICE.to_string())
        .bind(116_844_606_259_202_003_i64.to_string())
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    assert!(queue.notification_unfilter_pending(ALICE).await?);
    sqlx::query(
        "INSERT INTO notifications
            (id, account_id, activity_id, activity_type, from_account_id, type, filtered,
             created_at, updated_at)
         VALUES (-95002, $1, -95003, 'Mention', $2, 'mention', true,
                 clock_timestamp(), clock_timestamp())",
    )
    .bind(ALICE)
    .bind(API_MODERATOR)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO notification_requests
            (id, account_id, from_account_id, last_status_id, notifications_count,
             created_at, updated_at)
         VALUES (-95001, $1, $2, NULL, 1, clock_timestamp(), clock_timestamp())",
    )
    .bind(ALICE)
    .bind(API_MODERATOR)
    .execute(&writer_pool)
    .await?;
    writer
        .dismiss_notification_request(&notification_authenticated, -95001)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notifications WHERE id = -95002")
            .fetch_one(&writer_pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2
                AND payload -> 'arguments' ->> 'from_account_id' = $3",
        )
        .bind(NOTIFICATION_CLEANUP_JOB_KIND)
        .bind(ALICE.to_string())
        .bind(API_MODERATOR.to_string())
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    let direct_status_id = -803_i64;
    let direct_status_id_2 = -808_i64;
    let direct_conversation_id = -804_i64;
    let direct_mention_id = -805_i64;
    let direct_mention_id_2 = -809_i64;
    let direct_notification_id = -806_i64;
    let direct_notification_id_2 = -810_i64;
    let direct_request_id = -807_i64;
    sqlx::query(
        "INSERT INTO conversations (id, uri, parent_account_id, parent_status_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, clock_timestamp(), clock_timestamp())",
    )
    .bind(direct_conversation_id)
    .bind("https://fixture-v4-6-5.rustodon.invalid/conversations/-804")
    .bind(MODERATOR)
    .bind(direct_status_id)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO statuses \
         (id, account_id, conversation_id, text, spoiler_text, visibility, local, language, \
          sensitive, reply, created_at, updated_at) \
         VALUES ($1, $2, $3, 'Second filtered direct mention', '', 3, true, 'en', false, true, \
                 clock_timestamp(), clock_timestamp())",
    )
    .bind(direct_status_id_2)
    .bind(MODERATOR)
    .bind(direct_conversation_id)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO statuses \
         (id, account_id, conversation_id, text, spoiler_text, visibility, local, language, \
          sensitive, reply, created_at, updated_at) \
         VALUES ($1, $2, $3, 'Filtered direct mention', '', 3, true, 'en', false, false, \
                 clock_timestamp(), clock_timestamp())",
    )
    .bind(direct_status_id)
    .bind(MODERATOR)
    .bind(direct_conversation_id)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO mentions (id, account_id, status_id, silent, created_at, updated_at) \
         VALUES ($1, $2, $3, false, clock_timestamp(), clock_timestamp())",
    )
    .bind(direct_mention_id_2)
    .bind(ALICE)
    .bind(direct_status_id_2)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO mentions (id, account_id, status_id, silent, created_at, updated_at) \
         VALUES ($1, $2, $3, false, clock_timestamp(), clock_timestamp())",
    )
    .bind(direct_mention_id)
    .bind(ALICE)
    .bind(direct_status_id)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO notifications ( \
           id, account_id, activity_id, activity_type, from_account_id, type, filtered, \
           created_at, updated_at) \
         VALUES ($1, $2, $3, 'Mention', $4, 'mention', true, clock_timestamp(), clock_timestamp())",
    )
    .bind(direct_notification_id_2)
    .bind(ALICE)
    .bind(direct_mention_id_2)
    .bind(MODERATOR)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO notifications ( \
           id, account_id, activity_id, activity_type, from_account_id, type, filtered, \
           created_at, updated_at) \
         VALUES ($1, $2, $3, 'Mention', $4, 'mention', true, clock_timestamp(), clock_timestamp())",
    )
    .bind(direct_notification_id)
    .bind(ALICE)
    .bind(direct_mention_id)
    .bind(MODERATOR)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO notification_requests ( \
           id, account_id, from_account_id, last_status_id, notifications_count, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, 2, clock_timestamp(), clock_timestamp())",
    )
    .bind(direct_request_id)
    .bind(ALICE)
    .bind(MODERATOR)
    .bind(direct_status_id)
    .execute(&writer_pool)
    .await?;
    writer
        .accept_notification_request(&notification_authenticated, direct_request_id)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM account_conversations \
             WHERE account_id = $1 AND conversation_id = $2",
        )
        .bind(ALICE)
        .bind(direct_conversation_id)
        .fetch_one(&writer_pool)
        .await?,
        0
    );
    let original_warning_notifications: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(notification) FROM notifications notification \
         WHERE account_id = $1 AND activity_id = $2 AND activity_type = 'AccountWarning'",
    )
    .bind(ALICE)
    .bind(8401_i64)
    .fetch_all(&writer_pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Core,
                NOTIFICATION_CREATE_JOB_KIND,
                json!({
                    "recipient_account_id": ALICE,
                    "activity_type": "AccountWarning",
                    "activity_id": 8401,
                    "silenced": false
                }),
            )
            .logical_key("notification:test-account-warning"),
        )
        .await?;

    let handlers = infrastructure_handlers_with_writer(&queue, Some(writer_pool.clone()))?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    assert!(queue.dispatch_outbox(100).await? >= 3);
    assert!(queue.notification_unfilter_pending(ALICE).await?);
    while executor
        .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
        .await?
    {}
    assert!(!queue.notification_unfilter_pending(ALICE).await?);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notifications WHERE id = -95002")
            .fetch_one(&writer_pool)
            .await?,
        0
    );
    sqlx::query(
        "INSERT INTO notifications
            (id, account_id, activity_id, activity_type, from_account_id, type, filtered,
             created_at, updated_at)
         VALUES (-95005, $1, -95006, 'Mention', $2, 'mention', true,
                 clock_timestamp(), clock_timestamp())",
    )
    .bind(ALICE)
    .bind(API_MODERATOR)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO notification_requests
            (id, account_id, from_account_id, last_status_id, notifications_count,
             created_at, updated_at)
         VALUES (-95004, $1, $2, NULL, 1, clock_timestamp(), clock_timestamp())",
    )
    .bind(ALICE)
    .bind(API_MODERATOR)
    .execute(&writer_pool)
    .await?;
    writer
        .dismiss_notification_request(&notification_authenticated, -95004)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2
                AND payload -> 'arguments' ->> 'from_account_id' = $3",
        )
        .bind(NOTIFICATION_CLEANUP_JOB_KIND)
        .bind(ALICE.to_string())
        .bind(API_MODERATOR.to_string())
        .fetch_one(&writer_pool)
        .await?,
        2
    );
    assert!(queue.dispatch_outbox(100).await? >= 1);
    while executor
        .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
        .await?
    {}
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notifications WHERE id = -95005")
            .fetch_one(&writer_pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
             WHERE account_id = $1 AND activity_id = $2 AND activity_type = 'Mention'",
        )
        .bind(MODERATOR)
        .bind(mention_id)
        .fetch_one(&writer_pool)
        .await?,
        1
    );

    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
              WHERE account_id = $1 AND activity_id = $2 AND activity_type = 'Follow'",
        )
        .bind(API_MODERATOR)
        .bind(follow_id)
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    for recipient_account_id in [MODERATOR, API_MODERATOR] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications \
                 WHERE account_id = $1 AND activity_id = $2 \
                   AND activity_type = 'Report' AND type = 'admin.report'",
            )
            .bind(recipient_account_id)
            .bind(report_id)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
             WHERE account_id = $1 AND activity_id = $2 AND activity_type = 'AccountWarning' \
               AND type = 'moderation_warning'",
        )
        .bind(ALICE)
        .bind(8401_i64)
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT filtered FROM notifications WHERE id = $1")
            .bind(10021_i64)
            .fetch_one(&writer_pool)
            .await?
    );
    assert_eq!(
        sqlx::query_as::<_, (Vec<i64>, Vec<i64>, i64, bool)>(
            "SELECT participant_account_ids, status_ids, last_status_id, unread \
             FROM account_conversations WHERE account_id = $1 AND conversation_id = $2",
        )
        .bind(ALICE)
        .bind(direct_conversation_id)
        .fetch_one(&writer_pool)
        .await?,
        (
            vec![MODERATOR],
            vec![direct_status_id_2, direct_status_id],
            direct_status_id,
            true
        )
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT filtered FROM notifications WHERE id = $1",)
            .bind(direct_notification_id)
            .fetch_one(&writer_pool)
            .await?
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT filtered FROM notifications WHERE id = $1",)
            .bind(direct_notification_id_2)
            .fetch_one(&writer_pool)
            .await?
    );

    queue
        .enqueue(
            &JobSpec::new(
                Lane::Core,
                NOTIFICATION_CREATE_JOB_KIND,
                json!({
                    "recipient_account_id": MODERATOR,
                    "activity_type": "mention",
                    "activity_id": mention_id,
                    "silenced": false
                }),
            )
            .logical_key("notification:test-mention-duplicate"),
        )
        .await?;
    assert!(
        executor
            .process_one("notification-worker", &[Lane::Core], Duration::seconds(30),)
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
             WHERE account_id = $1 AND activity_id = $2 AND activity_type = 'Mention'",
        )
        .bind(MODERATOR)
        .bind(mention_id)
        .fetch_one(&writer_pool)
        .await?,
        1
    );

    let update_reblog_status_id = -700_i64;
    let quoted_update_status_id = -701_i64;
    let quoted_update_id = -702_i64;
    sqlx::query(
        "INSERT INTO statuses \
         (id, account_id, text, spoiler_text, visibility, local, language, sensitive, reply, \
          ordered_media_attachment_ids, reblog_of_id, created_at, updated_at) \
         VALUES ($1, $2, '', '', 0, true, 'en', false, false, NULL, $3, \
                 clock_timestamp(), clock_timestamp()), \
                ($4, $2, 'quoted update worker fixture', '', 0, true, 'en', false, false, NULL, NULL, \
                 clock_timestamp(), clock_timestamp())",
    )
    .bind(update_reblog_status_id)
    .bind(MODERATOR)
    .bind(status.status_id)
    .bind(quoted_update_status_id)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO quotes \
         (id, account_id, status_id, quoted_account_id, quoted_status_id, state, legacy, \
          created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, 1, false, clock_timestamp(), clock_timestamp())",
    )
    .bind(quoted_update_id)
    .bind(MODERATOR)
    .bind(quoted_update_status_id)
    .bind(116_844_606_259_201_001_i64)
    .bind(status.status_id)
    .execute(&writer_pool)
    .await?;
    for (activity_type, activity_id) in [
        ("update", status.status_id),
        ("quoted_update", quoted_update_status_id),
    ] {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Core,
                    NOTIFICATION_CREATE_JOB_KIND,
                    json!({
                        "recipient_account_id": MODERATOR,
                        "activity_type": activity_type,
                        "activity_id": activity_id,
                        "silenced": false
                    }),
                )
                .logical_key(format!("notification:test-{activity_type}-{activity_id}")),
            )
            .await?;
        assert!(
            executor
                .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
                .await?
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
              WHERE account_id = $1 AND activity_id = $2 \
                AND activity_type = 'Status' AND type = 'update'",
        )
        .bind(MODERATOR)
        .bind(status.status_id)
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
              WHERE account_id = $1 AND activity_id = $2 \
                AND activity_type = 'Status' AND type = 'quoted_update'",
        )
        .bind(MODERATOR)
        .bind(quoted_update_status_id)
        .fetch_one(&writer_pool)
        .await?,
        1
    );

    writer
        .delete_status(&authenticated, status.status_id, false)
        .await?;
    writer
        .set_follow(
            &follow_authenticated,
            API_MODERATOR,
            false,
            None,
            None,
            None,
        )
        .await?;
    sqlx::query(
        "DELETE FROM notifications WHERE activity_id = $1 AND activity_type IN ('Status', 'Mention')",
    )
        .bind(mention_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM notifications WHERE activity_id = $1 AND activity_type = 'Status'")
        .bind(status.status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "DELETE FROM notifications WHERE account_id = $1 AND activity_id = ANY($2) \
          AND activity_type = 'Status'",
    )
    .bind(MODERATOR)
    .bind(vec![quoted_update_status_id])
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM notifications WHERE activity_id = $1 AND activity_type = 'Report'")
        .bind(report_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "DELETE FROM notifications WHERE account_id = $1 AND activity_id = $2 \
         AND activity_type = 'AccountWarning'",
    )
    .bind(ALICE)
    .bind(8401_i64)
    .execute(&writer_pool)
    .await?;
    for notification in original_warning_notifications {
        sqlx::query(
            "INSERT INTO notifications \
             SELECT * FROM jsonb_populate_record(NULL::notifications, $1)",
        )
        .bind(notification)
        .execute(&writer_pool)
        .await?;
    }
    sqlx::query("DELETE FROM reports WHERE id = $1")
        .bind(report_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM quotes WHERE id = $1")
        .bind(quoted_update_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
        .bind(vec![update_reblog_status_id, quoted_update_status_id])
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM mentions WHERE status_id = $1")
        .bind(status.status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(status.status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id = $1")
        .bind(status.status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE parent_status_id = $1")
        .bind(status.status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM account_conversations WHERE conversation_id = $1")
        .bind(direct_conversation_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM notifications WHERE id = $1")
        .bind(direct_notification_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM notifications WHERE id = $1")
        .bind(direct_notification_id_2)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM mentions WHERE id = $1")
        .bind(direct_mention_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM mentions WHERE id = $1")
        .bind(direct_mention_id_2)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(direct_status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(direct_status_id_2)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(direct_conversation_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE notifications SET filtered = true WHERE id = $1")
        .bind(10021_i64)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "INSERT INTO notification_requests ( \
           id, account_id, from_account_id, last_status_id, notifications_count, created_at, updated_at) \
         VALUES (-95, $1, $2, $3, $4, $5, $6) \
         ON CONFLICT (id) DO UPDATE SET account_id = EXCLUDED.account_id, \
           from_account_id = EXCLUDED.from_account_id, last_status_id = EXCLUDED.last_status_id, \
           notifications_count = EXCLUDED.notifications_count, created_at = EXCLUDED.created_at, \
           updated_at = EXCLUDED.updated_at",
    )
    .bind(original_notification_request.0)
    .bind(original_notification_request.1)
    .bind(original_notification_request.2)
    .bind(original_notification_request.3)
    .bind(original_notification_request.4)
    .bind(original_notification_request.5)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "DELETE FROM notification_permissions \
         WHERE account_id = $1 AND from_account_id = ANY($2)",
    )
    .bind(ALICE)
    .bind(vec![MODERATOR, 116_844_606_259_202_003_i64])
    .execute(&writer_pool)
    .await?;
    sqlx::query("UPDATE account_stats SET last_status_at = $2 WHERE account_id = $1")
        .bind(ALICE)
        .bind(baseline_last_status_at)
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE users SET settings = $2 WHERE account_id = $1")
        .bind(MODERATOR)
        .bind(baseline_moderator_settings)
        .execute(&writer_pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_actor_update_and_delete_are_processed_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ACTOR_ACCOUNT_ID: i64 = 900_000_000_000_000_003;
    const ACTOR_STATS_ID: i64 = 11099;
    const SOURCE_KEY_ACCOUNT_ID: i64 = 116_844_606_259_202_001;
    const LOCAL_TARGET_ACCOUNT_ID: i64 = 116_844_606_259_201_002;
    const LIFECYCLE_KEY_URI: &str =
        "https://remote.fixture.invalid/users/exclusive_author#lifecycle-key";
    const REMOTE_FOLLOW_URI: &str =
        "https://remote.fixture.invalid/activities/lifecycle-follow-incoming";
    const LOCAL_FOLLOW_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/users/moderator#lifecycle-follow-outgoing";
    const ACTOR_STATUS_URI: &str =
        "https://remote.fixture.invalid/users/exclusive_author/statuses/lifecycle-status";
    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let media_root_path = std::env::temp_dir().join(format!(
        "rustodon-remote-actor-media-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&media_root_path);
    fs::create_dir_all(&media_root_path)?;
    let media_root = PaperclipRoot::open(&media_root_path)?;
    sqlx::query(
        "INSERT INTO accounts
            (id, actor_type, display_name, domain, note, followers_url, following_url,
             inbox_url, outbox_url, protocol, public_key, shared_inbox_url, uri, url,
             username, indexable, locked, created_at, updated_at)
         VALUES ($1, 'Person', 'Lifecycle Actor', 'remote.fixture.invalid',
                 'Lifecycle actor', $2, $3, $4, $5, 1, '', '', $6, $7,
                 'lifecycle_actor', false, false, clock_timestamp(), clock_timestamp())",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .bind("https://remote.fixture.invalid/users/lifecycle_actor/followers")
    .bind("https://remote.fixture.invalid/users/lifecycle_actor/following")
    .bind("https://remote.fixture.invalid/users/lifecycle_actor/inbox")
    .bind("https://remote.fixture.invalid/users/lifecycle_actor/outbox")
    .bind("https://remote.fixture.invalid/users/lifecycle_actor")
    .bind("https://remote.fixture.invalid/@lifecycle_actor")
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE accounts SET avatar_content_type = 'image/png', avatar_file_name = 'actor-avatar.png',
             avatar_file_size = 10, avatar_storage_schema_version = 1,
             avatar_updated_at = clock_timestamp(), header_content_type = 'image/png',
             header_file_name = 'actor-header.png', header_file_size = 20,
             header_storage_schema_version = 1, header_updated_at = clock_timestamp()
           WHERE id = $1",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO account_stats
            (id, account_id, created_at, updated_at)
         VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
    )
    .bind(ACTOR_STATS_ID)
    .bind(ACTOR_ACCOUNT_ID)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO keypairs
            (account_id, type, uri, public_key, private_key, revoked, expires_at,
             created_at, updated_at)
         SELECT $1, type, $2, public_key, NULL, false, NULL,
                clock_timestamp(), clock_timestamp()
           FROM keypairs WHERE account_id = $3 ORDER BY uri LIMIT 1
         ON CONFLICT (uri) DO NOTHING",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .bind(LIFECYCLE_KEY_URI)
    .bind(SOURCE_KEY_ACCOUNT_ID)
    .execute(&writer_pool)
    .await?;

    let actor = sqlx::query(
        "SELECT a.id, a.uri, a.domain, a.username, a.actor_type::text AS actor_type,
                a.display_name, a.note, a.url, a.inbox_url, a.outbox_url,
                a.followers_url, a.following_url, a.shared_inbox_url, a.locked,
                a.discoverable, a.indexable, a.fields, a.also_known_as,
                a.avatar_remote_url, a.header_remote_url, a.suspended_at,
                k.uri AS key_uri, s.following_count, s.followers_count, s.statuses_count
           FROM accounts a
           JOIN keypairs k ON k.account_id = a.id
           JOIN account_stats s ON s.account_id = a.id
          WHERE a.id = $1 AND a.domain IS NOT NULL
          ORDER BY a.id, k.uri
          LIMIT 1",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .fetch_optional(&writer_pool)
    .await?
    .ok_or("lifecycle actor fixture row missing")?;
    let actor_id: i64 = actor.try_get("id")?;
    let actor_uri: String = actor.try_get("uri")?;
    let remote_domain: String = actor.try_get("domain")?;
    let username: String = actor.try_get("username")?;
    let actor_type: Option<String> = actor.try_get("actor_type")?;
    let key_uri = LIFECYCLE_KEY_URI.to_owned();
    let display_name: String = actor.try_get("display_name")?;
    let note: String = actor.try_get("note")?;
    let url: Option<String> = actor.try_get("url")?;
    let inbox_url: String = actor.try_get("inbox_url")?;
    let outbox_url: String = actor.try_get("outbox_url")?;
    let followers_url: String = actor.try_get("followers_url")?;
    let following_url: String = actor.try_get("following_url")?;
    let shared_inbox_url: String = actor.try_get("shared_inbox_url")?;
    let locked: bool = actor.try_get("locked")?;
    let discoverable: Option<bool> = actor.try_get("discoverable")?;
    let indexable: bool = actor.try_get("indexable")?;
    let fields: Option<Value> = actor.try_get("fields")?;
    let also_known_as: Option<Vec<String>> = actor.try_get("also_known_as")?;
    let avatar_remote_url: Option<String> = actor.try_get("avatar_remote_url")?;
    let header_remote_url: String = actor.try_get("header_remote_url")?;
    let suspended_at: Option<NaiveDateTime> = actor.try_get("suspended_at")?;
    let following_count: i64 = actor.try_get("following_count")?;
    let followers_count: i64 = actor.try_get("followers_count")?;
    let statuses_count: i64 = actor.try_get("statuses_count")?;
    let status_state = sqlx::query_as::<_, (i64, Option<NaiveDateTime>, NaiveDateTime)>(
        "SELECT id, deleted_at, updated_at FROM statuses WHERE account_id = $1",
    )
    .bind(actor_id)
    .fetch_all(&writer_pool)
    .await?;
    let follow_state = sqlx::query_as::<
        _,
        (
            i64,
            i64,
            NaiveDateTime,
            Option<Vec<String>>,
            bool,
            bool,
            i64,
            NaiveDateTime,
            Option<String>,
        ),
    >(
        "SELECT id, account_id, created_at, languages, notify, show_reblogs,
                target_account_id, updated_at, uri
           FROM follows WHERE account_id = $1 OR target_account_id = $1",
    )
    .bind(actor_id)
    .fetch_all(&writer_pool)
    .await?;
    let follow_request_state = sqlx::query_as::<
        _,
        (
            i64,
            i64,
            NaiveDateTime,
            Option<Vec<String>>,
            bool,
            bool,
            i64,
            NaiveDateTime,
            Option<String>,
        ),
    >(
        "SELECT id, account_id, created_at, languages, notify, show_reblogs,
                target_account_id, updated_at, uri
           FROM follow_requests WHERE account_id = $1 OR target_account_id = $1",
    )
    .bind(actor_id)
    .fetch_all(&writer_pool)
    .await?;

    let baseline_target_stats = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT following_count, followers_count, statuses_count
           FROM account_stats WHERE account_id = $1",
    )
    .bind(LOCAL_TARGET_ACCOUNT_ID)
    .fetch_one(&writer_pool)
    .await?;
    let local_target_uri = format!("{ORIGIN}ap/users/{LOCAL_TARGET_ACCOUNT_ID}");
    let baseline_public_favourites_count: i64 =
        sqlx::query_scalar("SELECT favourites_count FROM status_stats WHERE status_id = $1")
            .bind(116_844_842_188_805_001_i64)
            .fetch_one(&writer_pool)
            .await?;
    let baseline_poll = sqlx::query_as::<_, (i64, Option<i64>, Vec<i64>)>(
        "SELECT votes_count, voters_count, cached_tallies FROM polls WHERE id = $1",
    )
    .bind(8201_i64)
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO favourites (id, account_id, status_id, created_at, updated_at)
         VALUES (-93301, $1, $2, clock_timestamp(), clock_timestamp())",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .bind(116_844_842_188_805_001_i64)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO bookmarks (id, account_id, status_id, created_at, updated_at)
         VALUES (-93302, $1, $2, clock_timestamp(), clock_timestamp())",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .bind(116_844_842_188_805_001_i64)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO poll_votes (id, account_id, poll_id, choice, uri, created_at, updated_at)
         VALUES (-93303, $1, 8201, 1, $2, clock_timestamp(), clock_timestamp())",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .bind("https://remote.fixture.invalid/users/exclusive_author#votes/93303")
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE status_stats SET favourites_count = favourites_count + 1 WHERE status_id = $1",
    )
    .bind(116_844_842_188_805_001_i64)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE polls SET votes_count = votes_count + 1, voters_count = voters_count + 1,
            cached_tallies[2] = cached_tallies[2] + 1 WHERE id = 8201",
    )
    .execute(&writer_pool)
    .await?;
    let actor_status_id: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (
             account_id, text, spoiler_text, visibility, local, uri, url, language,
             sensitive, reply, created_at, updated_at)
         VALUES ($1, 'lifecycle status', '', 0, false, $2, $2, 'en', false, false,
                 clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .bind(ACTOR_STATUS_URI)
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at)
         VALUES ($1, clock_timestamp(), clock_timestamp())",
    )
    .bind(actor_status_id)
    .execute(&writer_pool)
    .await?;
    let actor_reblog_id: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (
             account_id, text, spoiler_text, visibility, local, reblog_of_id,
             sensitive, reply, created_at, updated_at)
         VALUES ($1, '', '', 0, true, $2, false, false,
                 clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(LOCAL_TARGET_ACCOUNT_ID)
    .bind(actor_status_id)
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at)
         VALUES ($1, clock_timestamp(), clock_timestamp())",
    )
    .bind(actor_reblog_id)
    .execute(&writer_pool)
    .await?;
    let actor_media_id: i64 = sqlx::query_scalar(
        "INSERT INTO media_attachments (
             account_id, status_id, type, processing, remote_url, file_meta,
             created_at, updated_at)
         VALUES ($1, $2, 0, 0, 'https://media.remote.fixture.invalid/lifecycle.jpg',
                 '{}'::json, clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .bind(actor_status_id)
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE media_attachments SET file_content_type = 'image/jpeg',
             file_file_name = 'actor-media.jpg', file_file_size = 10,
             file_storage_schema_version = 1, file_updated_at = clock_timestamp(),
             thumbnail_content_type = 'image/jpeg', thumbnail_file_name = 'actor-media-thumb.jpg',
             thumbnail_file_size = 11,
             thumbnail_remote_url = 'https://media.remote.fixture.invalid/actor-media-thumb.jpg',
             thumbnail_storage_schema_version = 1, thumbnail_updated_at = clock_timestamp()
           WHERE id = $1",
    )
    .bind(actor_media_id)
    .execute(&writer_pool)
    .await?;
    let actor_media_metadata = [
        PaperclipMetadata {
            attachment: PaperclipAttachment::AccountAvatar,
            id: ACTOR_ACCOUNT_ID,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "actor-avatar.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::AccountHeader,
            id: ACTOR_ACCOUNT_ID,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "actor-header.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: actor_media_id,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "actor-media.jpg".to_owned(),
            content_type: Some("image/jpeg".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::MediaThumbnail,
            id: actor_media_id,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "actor-media-thumb.jpg".to_owned(),
            content_type: Some("image/jpeg".to_owned()),
            variant: None,
        },
    ];
    for metadata in &actor_media_metadata {
        for style in ["original", "small", "static"] {
            if let Some(path) = metadata.relative_path(style) {
                media_root.write_file(Path::new(&path), b"remote-actor-media")?;
            }
        }
    }
    sqlx::query(
        "INSERT INTO follows (
             account_id, target_account_id, show_reblogs, notify, languages, uri,
             created_at, updated_at)
         VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(LOCAL_TARGET_ACCOUNT_ID)
    .bind(ACTOR_ACCOUNT_ID)
    .bind(LOCAL_FOLLOW_URI)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO follows (
             account_id, target_account_id, show_reblogs, notify, languages, uri,
             created_at, updated_at)
         VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .bind(LOCAL_TARGET_ACCOUNT_ID)
    .bind(REMOTE_FOLLOW_URI)
    .execute(&writer_pool)
    .await?;
    let remote_follow_id: i64 = sqlx::query_scalar(
        "SELECT id FROM follows WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .bind(LOCAL_TARGET_ACCOUNT_ID)
    .bind(REMOTE_FOLLOW_URI)
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE account_stats SET following_count = following_count + 1,
            followers_count = followers_count + 1, statuses_count = statuses_count + 1,
            updated_at = clock_timestamp() WHERE account_id = $1",
    )
    .bind(ACTOR_ACCOUNT_ID)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE account_stats SET following_count = following_count + 1,
            followers_count = followers_count + 1, statuses_count = statuses_count + 1,
            updated_at = clock_timestamp() WHERE account_id = $1",
    )
    .bind(LOCAL_TARGET_ACCOUNT_ID)
    .execute(&writer_pool)
    .await?;
    sqlx::query("UPDATE status_stats SET reblogs_count = reblogs_count + 1 WHERE status_id = $1")
        .bind(actor_status_id)
        .execute(&writer_pool)
        .await?;

    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: Some(media_root.clone()),
        limited_federation: false,
        #[cfg(feature = "test-support")]
        remote_media_endpoint: None,
        #[cfg(feature = "test-support")]
        remote_delivery_endpoint: None,
        #[cfg(feature = "test-support")]
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let endpoints = if shared_inbox_url.is_empty() {
        json!({})
    } else {
        json!({"sharedInbox": shared_inbox_url})
    };
    let update_body = json!({
        "type": "Update",
        "actor": actor_uri,
        "object": {
            "id": actor_uri,
            "type": actor_type.as_deref().unwrap_or("Person"),
            "preferredUsername": username,
            "name": "Fixture Updated Actor :actor_profile_blob:",
            "summary": "Fixture updated summary #actorprofile",
            "icon": {"type": "Image", "mediaType": "image/png", "url": "https://media.fixture.invalid/updated-avatar.png"},
            "image": {"type": "Image", "mediaType": "image/jpeg", "url": "https://media.fixture.invalid/updated-header.jpg"},
            "url": url.as_deref().unwrap_or(&actor_uri),
            "inbox": inbox_url,
            "outbox": outbox_url,
            "followers": followers_url,
            "following": following_url,
            "endpoints": endpoints,
            "tag": [
                {"type": "Hashtag", "name": "#actorprofile", "href": "https://remote.fixture.invalid/tags/actorprofile"},
                {
                    "id": "https://remote.fixture.invalid/emojis/actor_profile_blob",
                    "type": "Emoji",
                    "name": ":actor_profile_blob:",
                    "updated": "2026-08-25T12:00:00Z",
                    "icon": {"type": "Image", "mediaType": "image/png", "url": "https://media.fixture.invalid/actor-profile.png"}
                }
            ]
        }
    })
    .to_string();
    let delete_body = json!({
        "type": "Delete",
        "actor": actor_uri,
        "object": actor_uri
    })
    .to_string();
    let result = async {
        let mut suspended_update: Value = serde_json::from_str(&update_body)?;
        suspended_update["object"]["suspended"] = json!(true);
        let mut unsuspended_update = suspended_update.clone();
        unsuspended_update["object"]["suspended"] = json!(false);
        for (logical_key, body) in [
            (
                "activitypub:test-actor-suspend",
                suspended_update.to_string(),
            ),
            (
                "activitypub:test-actor-unsuspend",
                unsuspended_update.to_string(),
            ),
            ("activitypub:test-actor-update", update_body),
            ("activitypub:test-actor-delete", delete_body),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": body,
                            "signature_key_id": key_uri,
                            "remote_domain": remote_domain
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one("actor-worker", &[Lane::Ingress], Duration::seconds(30))
                    .await?
            );
            if logical_key == "activitypub:test-actor-suspend" {
                let state = sqlx::query_as::<_, (Option<NaiveDateTime>, Option<i32>)>(
                    "SELECT suspended_at, suspension_origin FROM accounts WHERE id = $1",
                )
                .bind(actor_id)
                .fetch_one(&writer_pool)
                .await?;
                assert!(state.0.is_some(), "remote suspension must set suspended_at");
                assert_eq!(
                    state.1,
                    Some(1),
                    "remote actor suspension must use the remote origin"
                );
                assert_eq!(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM follows
                          WHERE account_id = $1 AND target_account_id = $2",
                    )
                    .bind(ACTOR_ACCOUNT_ID)
                    .bind(LOCAL_TARGET_ACCOUNT_ID)
                    .fetch_one(&writer_pool)
                    .await?,
                    1,
                    "remote-origin actor suspension must preserve follows"
                );
            }
            if logical_key == "activitypub:test-actor-unsuspend" {
                assert_eq!(
                    sqlx::query_as::<_, (Option<NaiveDateTime>, Option<i32>)>(
                        "SELECT suspended_at, suspension_origin FROM accounts WHERE id = $1",
                    )
                    .bind(actor_id)
                    .fetch_one(&writer_pool)
                    .await?,
                    (None, None),
                    "remote actor unsuspension must clear the remote origin"
                );
                assert_eq!(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM follows
                          WHERE account_id = $1 AND target_account_id = $2",
                    )
                    .bind(ACTOR_ACCOUNT_ID)
                    .bind(LOCAL_TARGET_ACCOUNT_ID)
                    .fetch_one(&writer_pool)
                    .await?,
                    1,
                    "remote actor unsuspension must preserve the existing follow"
                );
            }
            if logical_key == "activitypub:test-actor-update" {
                assert_eq!(
                    sqlx::query_as::<_, (String, String, Option<String>, String)>(
                        "SELECT display_name, note, avatar_remote_url, header_remote_url
                           FROM accounts WHERE id = $1",
                    )
                    .bind(actor_id)
                    .fetch_one(&writer_pool)
                    .await?,
                    (
                        "Fixture Updated Actor :actor_profile_blob:".to_owned(),
                        "Fixture updated summary #actorprofile".to_owned(),
                        Some("https://media.fixture.invalid/updated-avatar.png".to_owned()),
                        "https://media.fixture.invalid/updated-header.jpg".to_owned(),
                    ),
                    "full actor Update must persist profile text and Image.url media",
                );
                let profile_emoji = sqlx::query_as::<_, (i64, String, bool, bool)>(
                    "SELECT id, image_remote_url, disabled, visible_in_picker
                       FROM custom_emojis
                      WHERE shortcode = 'actor_profile_blob' AND domain = $1",
                )
                .bind(&remote_domain)
                .fetch_one(&writer_pool)
                .await?;
                assert_eq!(
                    profile_emoji.1,
                    "https://media.fixture.invalid/actor-profile.png"
                );
                assert!(!profile_emoji.2);
                assert!(profile_emoji.3);
                assert_eq!(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM rustodon.outbox_events
                          WHERE kind = $1 AND payload -> 'arguments' ->> 'emoji_id' = $2",
                    )
                    .bind(ACTIVITYPUB_EMOJI_FETCH_JOB_KIND)
                    .bind(profile_emoji.0.to_string())
                    .fetch_one(&writer_pool)
                    .await?,
                    1,
                    "actor emoji fetches must use the deduplicated Note emoji pipeline",
                );
                assert_eq!(
                    sqlx::query_scalar::<_, Vec<String>>(
                        "SELECT array_agg(tag.name ORDER BY tag.name)
                           FROM accounts_tags account_tag
                           JOIN tags tag ON tag.id = account_tag.tag_id
                          WHERE account_tag.account_id = $1",
                    )
                    .bind(actor_id)
                    .fetch_one(&writer_pool)
                    .await?,
                    vec!["actorprofile".to_owned()],
                    "actor emoji processing must preserve profile hashtag associations",
                );
                assert_eq!(
                    sqlx::query_scalar::<_, Option<Value>>(
                        "SELECT fields FROM accounts WHERE id = $1",
                    )
                    .bind(actor_id)
                    .fetch_one(&writer_pool)
                    .await?,
                    fields.clone(),
                    "an actor Update without attachment must preserve profile fields",
                );
            }
        }
        let updated = sqlx::query_as::<_, (String, String, Option<NaiveDateTime>)>(
            "SELECT display_name, note, suspended_at FROM accounts WHERE id = $1",
        )
        .bind(actor_id)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(updated.0, "");
        assert_eq!(updated.1, "");
        assert!(updated.2.is_some());
        assert_eq!(
            sqlx::query_as::<_, (i64, i64, i64)>(
                "SELECT following_count, followers_count, statuses_count
                   FROM account_stats WHERE account_id = $1",
            )
            .bind(actor_id)
            .fetch_one(&writer_pool)
            .await?,
            (0, 0, 0),
            "remote actor deletion must clear the deleted actor's counters"
        );
        assert_eq!(
            sqlx::query_as::<_, (i64, i64, i64)>(
                "SELECT following_count, followers_count, statuses_count
                   FROM account_stats WHERE account_id = $1",
            )
            .bind(LOCAL_TARGET_ACCOUNT_ID)
            .fetch_one(&writer_pool)
            .await?,
            baseline_target_stats,
            "remote actor deletion must restore affected local counters"
        );

        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT deleted_at IS NOT NULL FROM statuses WHERE id = $1",
            )
            .bind(actor_status_id)
            .fetch_one(&writer_pool)
            .await?
        );
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT deleted_at IS NOT NULL FROM statuses WHERE id = $1",
            )
            .bind(actor_reblog_id)
            .fetch_one(&writer_pool)
            .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_attachments WHERE id = $1")
                .bind(actor_media_id)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "remote actor deletion must purge authored media"
        );
        let profile_media_state: (Option<String>, Option<String>) =
            sqlx::query_as("SELECT avatar_file_name, header_file_name FROM accounts WHERE id = $1")
                .bind(actor_id)
                .fetch_one(&writer_pool)
                .await?;
        assert_eq!(profile_media_state, (None, None));
        for metadata in &actor_media_metadata {
            for style in ["original", "small", "static"] {
                if let Some(path) = metadata.relative_path(style) {
                    assert!(
                        media_root.open_file(Path::new(&path)).is_err(),
                        "remote actor deletion left Paperclip file {path}"
                    );
                }
            }
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM favourites WHERE account_id = $1",)
                .bind(actor_id)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "remote actor deletion must purge actor-owned favourites"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT favourites_count FROM status_stats WHERE status_id = $1",
            )
            .bind(116_844_842_188_805_001_i64)
            .fetch_one(&writer_pool)
            .await?,
            baseline_public_favourites_count,
            "remote actor deletion must restore favourite counters"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM bookmarks WHERE account_id = $1")
                .bind(actor_id)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "remote actor deletion must purge actor-owned bookmarks"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM poll_votes WHERE account_id = $1")
                .bind(actor_id)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "remote actor deletion must purge actor-owned poll votes"
        );
        assert_eq!(
            sqlx::query_as::<_, (i64, Option<i64>, Vec<i64>)>(
                "SELECT votes_count, voters_count, cached_tallies FROM polls WHERE id = $1",
            )
            .bind(8201_i64)
            .fetch_one(&writer_pool)
            .await?,
            baseline_poll,
            "remote actor deletion must restore poll counters"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'event' = 'delete'
                    AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(LOCAL_TARGET_ACCOUNT_ID.to_string())
            .bind(actor_reblog_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "remote actor deletion must remove local reblogs from user streams"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1
                    AND payload -> 'arguments' ->> 'source_account_id' = $2
                    AND payload -> 'arguments' -> 'body' ->> 'type' = 'Reject'
                    AND payload -> 'arguments' -> 'body' -> 'object' ->> 'id' = $3",
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .bind(LOCAL_TARGET_ACCOUNT_ID.to_string())
            .bind(REMOTE_FOLLOW_URI)
            .fetch_one(&writer_pool)
            .await?,
            1,
            "remote actor deletion must reject incoming remote follows"
        );
        let reject_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload -> 'arguments' ->> 'source_account_id' = $2
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Reject'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'id' = $3",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(LOCAL_TARGET_ACCOUNT_ID.to_string())
        .bind(REMOTE_FOLLOW_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(
            reject_body["id"],
            format!("{local_target_uri}#rejects/follows/{remote_follow_id}")
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1
                    AND payload -> 'arguments' ->> 'source_account_id' = $2
                    AND payload -> 'arguments' -> 'body' ->> 'type' = 'Undo'
                    AND payload -> 'arguments' -> 'body' -> 'object' ->> 'id' = $3",
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .bind(LOCAL_TARGET_ACCOUNT_ID.to_string())
            .bind(LOCAL_FOLLOW_URI)
            .fetch_one(&writer_pool)
            .await?,
            1,
            "remote actor deletion must undo local follows"
        );

        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": json!({
                            "type": "Update",
                            "actor": actor_uri,
                            "object": {
                                "id": actor_uri,
                                "type": actor_type.as_deref().unwrap_or("Person"),
                                "preferredUsername": username,
                                "name": "Should Not Resurrect",
                                "summary": "Should Not Resurrect",
                                "inbox": inbox_url,
                                "outbox": outbox_url,
                                "followers": followers_url,
                                "following": following_url
                            }
                        })
                        .to_string(),
                        "signature_key_id": key_uri,
                        "remote_domain": remote_domain
                    }),
                )
                .logical_key("activitypub:test-actor-update-after-delete"),
            )
            .await?;
        assert!(
            executor
                .process_one("actor-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT display_name FROM accounts WHERE id = $1")
                .bind(actor_id)
                .fetch_one(&writer_pool)
                .await?,
            ""
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    sqlx::query("DELETE FROM media_attachments WHERE id = $1")
        .bind(actor_media_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM favourites WHERE id = -93301")
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM bookmarks WHERE id = -93302")
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM poll_votes WHERE id = -93303")
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE status_stats SET favourites_count = $2 WHERE status_id = $1")
        .bind(116_844_842_188_805_001_i64)
        .bind(baseline_public_favourites_count)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "UPDATE polls SET votes_count = $2, voters_count = $3, cached_tallies = $4 WHERE id = 8201",
    )
    .bind(8201_i64)
    .bind(baseline_poll.0)
    .bind(baseline_poll.1)
    .bind(&baseline_poll.2)
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id = ANY($1)")
        .bind(vec![actor_status_id, actor_reblog_id])
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
        .bind(vec![actor_status_id, actor_reblog_id])
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follows WHERE uri = ANY($1)")
        .bind(vec![REMOTE_FOLLOW_URI, LOCAL_FOLLOW_URI])
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "UPDATE accounts SET display_name = $2, note = $3, url = $4,
            inbox_url = $5, outbox_url = $6, followers_url = $7, following_url = $8,
            shared_inbox_url = $9, locked = $10, discoverable = $11, indexable = $12,
            fields = $13, also_known_as = $14, avatar_remote_url = $15,
            header_remote_url = $16, suspended_at = $17, updated_at = clock_timestamp()
          WHERE id = $1",
    )
    .bind(actor_id)
    .bind(display_name)
    .bind(note)
    .bind(url)
    .bind(inbox_url)
    .bind(outbox_url)
    .bind(followers_url)
    .bind(following_url)
    .bind(shared_inbox_url)
    .bind(locked)
    .bind(discoverable)
    .bind(indexable)
    .bind(fields)
    .bind(also_known_as)
    .bind(avatar_remote_url)
    .bind(header_remote_url)
    .bind(suspended_at)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE account_stats SET following_count = $2, followers_count = $3,
            statuses_count = $4, updated_at = clock_timestamp() WHERE account_id = $1",
    )
    .bind(LOCAL_TARGET_ACCOUNT_ID)
    .bind(baseline_target_stats.0)
    .bind(baseline_target_stats.1)
    .bind(baseline_target_stats.2)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE account_stats SET following_count = $2, followers_count = $3,
            statuses_count = $4, updated_at = clock_timestamp() WHERE account_id = $1",
    )
    .bind(actor_id)
    .bind(following_count)
    .bind(followers_count)
    .bind(statuses_count)
    .execute(&writer_pool)
    .await?;
    for (status_id, deleted_at, updated_at) in status_state {
        sqlx::query("UPDATE statuses SET deleted_at = $2, updated_at = $3 WHERE id = $1")
            .bind(status_id)
            .bind(deleted_at)
            .bind(updated_at)
            .execute(&writer_pool)
            .await?;
    }
    for (
        id,
        account_id,
        created_at,
        languages,
        notify,
        show_reblogs,
        target_account_id,
        updated_at,
        uri,
    ) in follow_state
    {
        sqlx::query(
            "INSERT INTO follows
                (id, account_id, created_at, languages, notify, show_reblogs,
                 target_account_id, updated_at, uri)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (account_id, target_account_id) DO NOTHING",
        )
        .bind(id)
        .bind(account_id)
        .bind(created_at)
        .bind(languages)
        .bind(notify)
        .bind(show_reblogs)
        .bind(target_account_id)
        .bind(updated_at)
        .bind(uri)
        .execute(&writer_pool)
        .await?;
    }
    for (
        id,
        account_id,
        created_at,
        languages,
        notify,
        show_reblogs,
        target_account_id,
        updated_at,
        uri,
    ) in follow_request_state
    {
        sqlx::query(
            "INSERT INTO follow_requests
                (id, account_id, created_at, languages, notify, show_reblogs,
                 target_account_id, updated_at, uri)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (account_id, target_account_id) DO NOTHING",
        )
        .bind(id)
        .bind(account_id)
        .bind(created_at)
        .bind(languages)
        .bind(notify)
        .bind(show_reblogs)
        .bind(target_account_id)
        .bind(updated_at)
        .bind(uri)
        .execute(&writer_pool)
        .await?;
    }
    sqlx::query("DELETE FROM keypairs WHERE uri = $1")
        .bind(LIFECYCLE_KEY_URI)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(ACTOR_ACCOUNT_ID)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(ACTOR_ACCOUNT_ID)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "DELETE FROM custom_emojis
          WHERE shortcode = 'actor_profile_blob' AND domain = 'remote.fixture.invalid'",
    )
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "DELETE FROM tags WHERE lower(name) = 'actorprofile'
          AND NOT EXISTS (SELECT 1 FROM accounts_tags WHERE tag_id = tags.id)",
    )
    .execute(&writer_pool)
    .await?;
    drop(media_root);
    fs::remove_dir_all(&media_root_path)?;
    result
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_note_create_update_and_delete_are_processed_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    const ALICE: i64 = 116_844_606_259_201_001;
    const BOB: i64 = 116_844_606_259_202_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
    const KEY_ID: &str = "https://remote.fixture.invalid/users/bob#secondary-key";
    const NOTE_URI: &str = "https://remote.fixture.invalid/users/bob/statuses/rustodon-note";
    const ATOM_URI: &str = "https://remote.fixture.invalid/objects/rustodon-note";
    const LIMITED_NOTE_URI: &str =
        "https://remote.fixture.invalid/users/bob/statuses/rustodon-limited-note";
    const STALE_NOTE_URI: &str =
        "https://remote.fixture.invalid/users/bob/statuses/rustodon-stale-note";
    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let media_root_path =
        std::env::temp_dir().join(format!("rustodon-remote-note-media-{}", std::process::id()));
    let _ = fs::remove_dir_all(&media_root_path);
    fs::create_dir_all(&media_root_path)?;
    let media_root = PaperclipRoot::open(&media_root_path)?;
    let exclusive_membership: Value = sqlx::query_scalar(
        "SELECT to_jsonb(list_account) FROM list_accounts list_account WHERE id = 9004",
    )
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM list_accounts WHERE id = 9004")
        .execute(&writer_pool)
        .await?;
    let baseline_statuses_count: i64 =
        sqlx::query_scalar("SELECT statuses_count FROM account_stats WHERE account_id = $1")
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?;
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: Some(media_root.clone()),
        limited_federation: false,
        #[cfg(feature = "test-support")]
        remote_media_endpoint: None,
        #[cfg(feature = "test-support")]
        remote_delivery_endpoint: None,
        #[cfg(feature = "test-support")]
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let create_body = json!({
        "id": "https://remote.fixture.invalid/activities/rustodon-note-create",
        "type": "Create",
        "actor": ACTOR,
        "object": {
            "id": NOTE_URI,
            "type": "Note",
            "attributedTo": ACTOR,
            "published": "2026-08-25T12:00:00Z",
             "url": "https://remote.fixture.invalid/@bob/123",
             "content": "<p>Initial remote note :party_blob:</p>",
        "summary": null,
        "sensitive": null,
             "likes": {"type": "Collection", "totalItems": 7},
             "shares": {"type": "Collection", "totalItems": 3},
              "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [],
            "tag": [{
                "id": "https://remote.fixture.invalid/emojis/party_blob",
                "type": "Emoji",
                "name": ":party_blob:",
                "updated": "2026-08-25T12:00:00Z",
                "icon": {"type": "Image", "mediaType": "image/png",
                         "url": "https://media.fixture.invalid/party-v1.png"}
            }],
            "attachment": [{
                "type": "Document",
                "mediaType": "image/jpeg",
                "url": "https://media.fixture.invalid/note.jpg",
                "name": "old note image",
                "width": 320,
                "height": 240
            }]
        }
    })
    .to_string();
    let update_body = json!({
        "type": "Update",
        "actor": ACTOR,
        "object": {
            "id": NOTE_URI,
            "type": "Note",
            "attributedTo": ACTOR,
            "published": "2026-08-25T12:00:00Z",
            "updated": "2026-08-25T12:01:00Z",
            "url": "https://remote.fixture.invalid/@bob/123",
            "content": "<p>Edited remote note :party_blob:</p>",
        "summary": null,
        "sensitive": null,
             "to": ["https://remote.fixture.invalid/users/bob/followers"],
            "cc": [],
            "tag": [{
                "id": "https://remote.fixture.invalid/emojis/party_blob",
                "type": "Emoji",
                "name": "party_blob",
                "updated": "2026-08-25T12:01:00Z",
                "icon": {"type": "Image", "mediaType": "image/png",
                         "url": "https://media.fixture.invalid/party-v2.png"}
            }],
            "attachment": [{
                "type": "Document",
                "mediaType": "image/jpeg",
                "url": "https://media.fixture.invalid/note.jpg",
                "preview": "https://media.fixture.invalid/note-thumb.jpg",
                "name": "note image",
                "width": 640,
                "height": 480
            }]
        }
    })
    .to_string();
    let delete_body = json!({
        "type": "Delete",
        "actor": ACTOR,
        "object": {
            "id": NOTE_URI,
            "type": "Tombstone",
            "atomUri": ATOM_URI
        }
    })
    .to_string();
    let post_delete_update_body = json!({
        "type": "Update",
        "actor": ACTOR,
        "object": {
            "id": NOTE_URI,
            "type": "Note",
            "attributedTo": ACTOR,
            "updated": "2026-08-25T12:02:00Z",
            "content": "<p>Must not resurrect</p>",
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [],
            "tag": [],
            "attachment": []
        }
    })
    .to_string();
    let implicit_update_body = json!({
        "type": "Update",
        "actor": ACTOR,
        "object": {
            "id": NOTE_URI,
            "type": "Note",
            "attributedTo": ACTOR,
            "content": "<p>Implicit updates must not replace note text</p>",
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [],
            "tag": [],
            "attachment": []
        }
    })
    .to_string();
    let limited_note_body = json!({
        "id": "https://remote.fixture.invalid/activities/rustodon-limited-note-create",
        "type": "Create",
        "actor": ACTOR,
        "object": {
            "id": LIMITED_NOTE_URI,
            "type": "Note",
            "attributedTo": ACTOR,
            "published": "2026-08-25T12:03:00Z",
            "content": "<p>Limited remote note</p>",
            "to": [
                "https://fixture-v4-6-5.rustodon.invalid/@alice",
                "https://fixture-v4-6-5.rustodon.invalid/@moderator"
            ],
            "cc": [],
            "tag": [],
            "attachment": []
        }
    })
    .to_string();
    let limited_update_body = json!({
        "type": "Update",
        "actor": ACTOR,
        "object": {
            "id": LIMITED_NOTE_URI,
            "type": "Note",
            "attributedTo": ACTOR,
            "published": "2026-08-25T12:03:00Z",
            "updated": "2026-08-25T12:04:00Z",
            "content": "<p>Edited limited remote note</p>",
            "to": [
                "https://fixture-v4-6-5.rustodon.invalid/@alice",
                "https://fixture-v4-6-5.rustodon.invalid/@moderator"
            ],
            "cc": [],
            "tag": [],
            "attachment": []
        }
    })
    .to_string();
    let limited_delete_body = json!({
        "type": "Delete",
        "actor": ACTOR,
        "object": LIMITED_NOTE_URI
    })
    .to_string();
    let stale_update_body = json!({
        "type": "Update",
        "actor": ACTOR,
        "object": {
            "id": STALE_NOTE_URI,
            "type": "Note",
            "attributedTo": ACTOR,
            "published": (Utc::now() - Duration::days(2)).to_rfc3339(),
            "content": "<p>Stale unknown update must be ignored</p>",
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [],
            "tag": [],
            "attachment": []
        }
    })
    .to_string();
    let mut baseline_note_favourites_count = None;
    let result = async {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": stale_update_body,
                        "delivery_target_account_id": MODERATOR,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-stale-note-update"),
            )
            .await?;
        assert!(
            executor
                .process_one("note-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE uri = $1")
                .bind(STALE_NOTE_URI)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "stale unknown Note Updates must not materialize a status"
        );
        for (logical_key, body) in [
            ("activitypub:test-note-create", create_body),
            ("activitypub:test-note-create-duplicate", json!({
                "id": "https://remote.fixture.invalid/activities/rustodon-note-create-duplicate",
                "type": "Create",
                "actor": ACTOR,
                "object": {
                    "id": NOTE_URI,
                    "type": "Note",
                    "attributedTo": ACTOR,
                    "published": "2026-08-25T12:00:00Z",
                    "content": "<p>Initial remote note :party_blob:</p>",
                    "to": ["https://www.w3.org/ns/activitystreams#Public"],
                    "cc": [],
                    "tag": [{
                        "id": "https://remote.fixture.invalid/emojis/party_blob",
                        "type": "Emoji", "name": "party_blob",
                        "icon": {"mediaType": "image/png",
                                 "url": "https://media.fixture.invalid/party-v1.png"}
                    }],
                    "attachment": []
                }
            }).to_string()),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": body,
                            "delivery_target_account_id": MODERATOR,
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid"
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one("note-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
            );
            assert!(
                !sqlx::query_scalar::<_, bool>("SELECT sensitive FROM statuses WHERE uri = $1")
                    .bind(NOTE_URI)
                    .fetch_one(&writer_pool)
                    .await?,
                "nullable-sensitive Create must persist before its duplicate is processed"
            );
        }
    let note_status_id: i64 = sqlx::query_scalar("SELECT id FROM statuses WHERE uri = $1")
        .bind(NOTE_URI)
        .fetch_optional(&writer_pool)
        .await?
        .ok_or("remote Note Create did not persist the status")?;
    let emoji = sqlx::query_as::<_, (i64, String, bool, bool)>(
        "SELECT id, image_remote_url, disabled, visible_in_picker
           FROM custom_emojis WHERE shortcode = 'party_blob' AND domain = 'remote.fixture.invalid'",
    )
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(emoji.1, "https://media.fixture.invalid/party-v1.png");
    assert!(!emoji.2);
    assert!(emoji.3);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1 AND payload -> 'arguments' ->> 'emoji_id' = $2",
        )
        .bind(ACTIVITYPUB_EMOJI_FETCH_JOB_KIND)
        .bind(emoji.0.to_string())
        .fetch_one(&writer_pool)
        .await?,
        1,
        "duplicate Creates must deduplicate emoji fetches",
    );
    sqlx::query(
        "UPDATE custom_emojis SET disabled = true, visible_in_picker = false WHERE id = $1",
    )
    .bind(emoji.0)
    .execute(&writer_pool)
    .await?;
    let note_media_id: i64 = sqlx::query_scalar(
        "SELECT id FROM media_attachments WHERE status_id = $1 ORDER BY id LIMIT 1",
    )
    .bind(note_status_id)
    .fetch_optional(&writer_pool)
    .await?
    .ok_or("remote Note Create did not persist the attachment")?;
    sqlx::query(
        "UPDATE media_attachments SET file_content_type = 'image/jpeg',
             file_file_name = 'remote-note.jpg', file_file_size = 10,
             file_storage_schema_version = 1, file_updated_at = clock_timestamp(),
             thumbnail_content_type = 'image/jpeg', thumbnail_file_name = 'remote-note-thumb.jpg',
             thumbnail_file_size = 11, thumbnail_remote_url = 'https://media.fixture.invalid/note-thumb.jpg',
             thumbnail_storage_schema_version = 1, thumbnail_updated_at = clock_timestamp()
           WHERE id = $1",
    )
    .bind(note_media_id)
    .execute(&writer_pool)
    .await?;
    let note_media_metadata = [
        PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: note_media_id,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "remote-note.jpg".to_owned(),
            content_type: Some("image/jpeg".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::MediaThumbnail,
            id: note_media_id,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "remote-note-thumb.jpg".to_owned(),
            content_type: Some("image/jpeg".to_owned()),
            variant: None,
        },
    ];
    for metadata in &note_media_metadata {
        for style in ["original", "small", "static"] {
            if let Some(path) = metadata.relative_path(style) {
                media_root
                    .write_file(Path::new(&path), b"remote-note-media")
                    .map_err(|error| format!("remote Note media write {path} failed: {error}"))?;
            }
        }
    }
    let note_counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT favourites_count, reblogs_count,
                untrusted_favourites_count, untrusted_reblogs_count
           FROM status_stats WHERE status_id = $1",
    )
    .bind(note_status_id)
    .fetch_optional(&writer_pool)
    .await
    .map_err(|error| format!("remote Note status stats query failed: {error}"))?
    .ok_or("remote Note Create did not persist status stats")?;
        if (note_counts.2, note_counts.3) != (7, 3) {
            return Err(
                format!("remote Note ActivityStreams counts were not persisted: {note_counts:?}")
                    .into(),
            );
        }
        let note_favourites_count = note_counts.0;
        baseline_note_favourites_count = Some(note_counts.0);
        sqlx::query(
            "INSERT INTO favourites (id, account_id, status_id, created_at, updated_at)
             VALUES (-93701, $1, $2, clock_timestamp(), clock_timestamp())",
        )
        .bind(MODERATOR)
        .bind(note_status_id)
        .execute(&writer_pool)
        .await?;
        sqlx::query("UPDATE status_stats SET favourites_count = favourites_count + 1 WHERE status_id = $1")
            .bind(note_status_id)
            .execute(&writer_pool)
            .await?;
        sqlx::query(
            "INSERT INTO polls
                (id, account_id, status_id, options, cached_tallies, votes_count,
                 voters_count, multiple, hide_totals, expires_at, created_at, updated_at)
             VALUES (-93703, $1, $2, ARRAY['Yes', 'No'], ARRAY[0, 0]::bigint[], 0,
                     0, false, false, NULL, clock_timestamp(), clock_timestamp())",
        )
        .bind(BOB)
        .bind(note_status_id)
        .execute(&writer_pool)
        .await?;
        sqlx::query("UPDATE statuses SET poll_id = -93703 WHERE id = $1")
            .bind(note_status_id)
            .execute(&writer_pool)
            .await?;
        sqlx::query(
            "INSERT INTO notifications
                (id, account_id, activity_id, activity_type, created_at, filtered,
                 from_account_id, group_key, type, updated_at)
             VALUES (-93702, $1, -93701, 'Favourite', clock_timestamp(), false,
                     $1, NULL, 'favourite', clock_timestamp()),
                    (-93704, $1, -93703, 'Poll', clock_timestamp(), false,
                     $1, NULL, 'poll', clock_timestamp())",
        )
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'event' = 'update'
                    AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(ALICE.to_string())
            .bind(note_status_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a remote Note Create must enter a local follower's user stream"
        );
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": limited_note_body,
                        "delivery_target_account_id": MODERATOR,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-limited-note-create"),
            )
            .await
            .map_err(|error| format!("limited remote Note enqueue failed: {error}"))?;
        assert!(
            executor
                .process_one("note-worker", &[Lane::Ingress], Duration::seconds(30))
                .await
                .map_err(|error| format!("limited remote Note processing failed: {error}"))?
        );
        let limited_status_id: i64 = sqlx::query_scalar("SELECT id FROM statuses WHERE uri = $1")
            .bind(LIMITED_NOTE_URI)
            .fetch_one(&writer_pool)
            .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT visibility FROM statuses WHERE id = $1")
                .bind(limited_status_id)
                .fetch_one(&writer_pool)
                .await?,
            4,
            "a remote Note addressed to specific recipients must be limited",
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_conversations
                  WHERE account_id = $1 AND $2 = ANY(status_ids)",
            )
            .bind(MODERATOR)
            .bind(limited_status_id)
            .fetch_one(&writer_pool)
            .await?,
            0,
            "limited remote Notes must not create direct account conversations",
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'event' = 'update'
                    AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(ALICE.to_string())
            .bind(limited_status_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a silent audience mention to a follower must enter the user stream"
        );
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": limited_update_body,
                        "delivery_target_account_id": MODERATOR,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-limited-note-update"),
            )
            .await?;
        assert!(
            executor
                .process_one("note-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'event' = 'status.update'
                    AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(ALICE.to_string())
            .bind(limited_status_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a silent audience mention to a follower must receive status updates"
        );
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": limited_delete_body,
                        "delivery_target_account_id": MODERATOR,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-limited-note-delete"),
            )
            .await?;
        assert!(
            executor
                .process_one("note-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'event' = 'delete'
                    AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(ALICE.to_string())
            .bind(limited_status_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a silent audience mention to a follower must receive deletes"
        );
        let update_reblog_status_id = -703_i64;
        let quoted_update_status_id = -704_i64;
        let quoted_update_id = -705_i64;
        sqlx::query(
            "INSERT INTO statuses \
             (id, account_id, text, spoiler_text, visibility, local, language, sensitive, reply, \
              ordered_media_attachment_ids, reblog_of_id, created_at, updated_at) \
             VALUES ($1, $2, '', '', 0, true, 'en', false, false, NULL, $3, \
                     clock_timestamp(), clock_timestamp()), \
                    ($4, $2, 'quoted remote update fixture', '', 0, true, 'en', false, false, NULL, NULL, \
                     clock_timestamp(), clock_timestamp())",
        )
        .bind(update_reblog_status_id)
        .bind(MODERATOR)
        .bind(note_status_id)
        .bind(quoted_update_status_id)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO quotes \
             (id, account_id, status_id, quoted_account_id, quoted_status_id, state, legacy, \
              created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, 1, false, clock_timestamp(), clock_timestamp())",
        )
        .bind(quoted_update_id)
        .bind(MODERATOR)
        .bind(quoted_update_status_id)
        .bind(BOB)
        .bind(note_status_id)
        .execute(&writer_pool)
        .await?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": implicit_update_body,
                        "delivery_target_account_id": MODERATOR,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-note-implicit-update"),
            )
            .await?;
        assert!(
            executor
                .process_one("note-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT text FROM statuses WHERE uri = $1")
                .bind(NOTE_URI)
                .fetch_one(&writer_pool)
                .await?,
            "<p>Initial remote note :party_blob:</p>"
        );
        for (logical_key, body) in [
            ("activitypub:test-note-update", update_body),
            ("activitypub:test-note-delete", delete_body.clone()),
            ("activitypub:test-note-delete-duplicate", delete_body),
            (
                "activitypub:test-note-update-after-delete",
                post_delete_update_body,
            ),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": body,
                            "delivery_target_account_id": MODERATOR,
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid"
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one("note-worker", &[Lane::Ingress], Duration::seconds(30))
                    .await?
            );
            if logical_key == "activitypub:test-note-update" {
                assert!(
                    !sqlx::query_scalar::<_, bool>("SELECT sensitive FROM statuses WHERE uri = $1")
                        .bind(NOTE_URI)
                        .fetch_one(&writer_pool)
                        .await?,
                    "nullable-sensitive Update must retain the false default"
                );
                assert_eq!(
                    sqlx::query_as::<_, (String, bool, bool)>(
                        "SELECT image_remote_url, disabled, visible_in_picker
                           FROM custom_emojis WHERE id = $1",
                    )
                    .bind(emoji.0)
                    .fetch_one(&writer_pool)
                    .await?,
                    ("https://media.fixture.invalid/party-v2.png".to_owned(), true, false),
                    "remote emoji refreshes must preserve moderation state",
                );
                assert!(queue.dispatch_outbox(100).await? >= 2);
                while executor
                    .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
                    .await?
                {}
                assert_eq!(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM notifications \
                          WHERE account_id = $1 AND activity_id = $2 \
                            AND activity_type = 'Status' AND type = 'update'",
                    )
                    .bind(MODERATOR)
                    .bind(note_status_id)
                    .fetch_one(&writer_pool)
                    .await?,
                    1
                );
                assert_eq!(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM notifications \
                          WHERE account_id = $1 AND activity_id = $2 \
                            AND activity_type = 'Status' AND type = 'quoted_update'",
                    )
                    .bind(MODERATOR)
                    .bind(quoted_update_status_id)
                    .fetch_one(&writer_pool)
                    .await?,
                    1
                );
                assert_eq!(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM rustodon.outbox_events
                          WHERE kind = $1 AND payload ->> 'event' = 'status.update'
                            AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
                    )
                    .bind(STREAM_EVENT_KIND)
                    .bind(ALICE.to_string())
                    .bind(note_status_id.to_string())
                    .fetch_one(&writer_pool)
                    .await?,
                    1,
                    "a remote Note Update must enter the local follower's user stream"
                );
            }
        }
        let status = sqlx::query_as::<
            _,
            (
                String,
                bool,
                i32,
                Option<NaiveDateTime>,
                Option<NaiveDateTime>,
            ),
        >(
            "SELECT text, local, visibility, deleted_at, edited_at FROM statuses WHERE uri = $1",
        )
        .bind(NOTE_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(status.0, "<p>Edited remote note :party_blob:</p>");
        assert!(!status.1);
        assert_eq!(status.2, 0);
        assert!(status.3.is_some());
        assert!(status.4.is_some());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'event' = 'delete'
                    AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(ALICE.to_string())
            .bind(note_status_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a remote Note Delete must remove the status from a local follower's stream"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'event' = 'delete'
                    AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(ALICE.to_string())
            .bind(update_reblog_status_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a remote Note Delete must remove deleted reblog wrappers from a local follower's stream"
        );
        let media = sqlx::query_as::<_, (i64, Option<String>, Option<String>)>(
            "SELECT id, description, remote_url FROM media_attachments
               WHERE status_id = (SELECT id FROM statuses WHERE uri = $1)",
        )
        .bind(NOTE_URI)
        .fetch_all(&writer_pool)
        .await?;
        assert!(media.is_empty(), "remote Note Delete must purge its media rows");
        for metadata in &note_media_metadata {
            for style in ["original", "small", "static"] {
                if let Some(path) = metadata.relative_path(style) {
                    assert!(
                        media_root.open_file(Path::new(&path)).is_err(),
                        "remote Note Delete left Paperclip file {path}"
                    );
                }
            }
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM tombstones WHERE account_id = $1 AND uri = ANY($2)",
            )
            .bind(BOB)
            .bind(vec![NOTE_URI, ATOM_URI])
            .fetch_one(&writer_pool)
            .await?,
            2
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1",
            )
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            baseline_statuses_count
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications
                  WHERE account_id = $1 AND ((activity_id = $2 AND activity_type = 'Status')
                     OR (activity_id = $3 AND activity_type = 'Status'))",
            )
            .bind(MODERATOR)
            .bind(note_status_id)
            .bind(quoted_update_status_id)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM favourites WHERE status_id = $1",
            )
            .bind(note_status_id)
            .fetch_one(&writer_pool)
            .await?,
            0,
            "a remote Note Delete must remove favourites of the deleted status"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT favourites_count FROM status_stats WHERE status_id = $1",
            )
            .bind(note_status_id)
            .fetch_one(&writer_pool)
            .await?,
            note_favourites_count,
            "a remote Note Delete must restore favourite counters"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM polls WHERE id = -93703")
                .fetch_one(&writer_pool)
                .await?,
            0,
            "a remote Note Delete must remove polls owned by the deleted status"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications WHERE id = ANY($1)",
            )
            .bind(vec![-93702_i64, -93704_i64])
            .fetch_one(&writer_pool)
            .await?,
            0,
            "a remote Note Delete must remove favourite and poll notifications"
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    sqlx::query("DELETE FROM tombstones WHERE account_id = $1 AND uri = ANY($2)")
        .bind(BOB)
        .bind(vec![NOTE_URI, ATOM_URI, LIMITED_NOTE_URI])
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "DELETE FROM notifications WHERE account_id = $1 AND activity_id = ANY($2) \
          AND activity_type = 'Status'",
    )
    .bind(MODERATOR)
    .bind(vec![
        sqlx::query_scalar::<_, i64>("SELECT id FROM statuses WHERE uri = $1")
            .bind(NOTE_URI)
            .fetch_optional(&writer_pool)
            .await?
            .unwrap_or_default(),
        -704_i64,
    ])
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM notifications WHERE id = ANY($1)")
        .bind(vec![-93702_i64, -93704_i64])
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM favourites WHERE id = -93701")
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM polls WHERE id = -93703")
        .execute(&writer_pool)
        .await?;
    let note_status_id_for_cleanup: Option<i64> =
        sqlx::query_scalar("SELECT id FROM statuses WHERE uri = $1")
            .bind(NOTE_URI)
            .fetch_optional(&writer_pool)
            .await?;
    if let Some(note_status_id) = note_status_id_for_cleanup {
        sqlx::query("UPDATE statuses SET poll_id = NULL WHERE id = $1")
            .bind(note_status_id)
            .execute(&writer_pool)
            .await?;
        sqlx::query("UPDATE status_stats SET favourites_count = $2 WHERE status_id = $1")
            .bind(note_status_id)
            .bind(baseline_note_favourites_count.unwrap_or(0))
            .execute(&writer_pool)
            .await?;
    }
    sqlx::query("DELETE FROM quotes WHERE id = $1")
        .bind(-705_i64)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
        .bind(vec![-703_i64, -704_i64])
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE uri = $1")
        .bind(NOTE_URI)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "DELETE FROM custom_emojis WHERE shortcode = 'party_blob' AND domain = 'remote.fixture.invalid'",
    )
    .execute(&writer_pool)
    .await?;
    let limited_status_id =
        sqlx::query_scalar::<_, Option<i64>>("SELECT id FROM statuses WHERE uri = $1")
            .bind(LIMITED_NOTE_URI)
            .fetch_optional(&writer_pool)
            .await?;
    if let Some(limited_status_id) = limited_status_id {
        sqlx::query("DELETE FROM mentions WHERE status_id = $1")
            .bind(limited_status_id)
            .execute(&writer_pool)
            .await?;
        sqlx::query("DELETE FROM status_stats WHERE status_id = $1")
            .bind(limited_status_id)
            .execute(&writer_pool)
            .await?;
        sqlx::query("DELETE FROM conversations WHERE parent_status_id = $1")
            .bind(limited_status_id)
            .execute(&writer_pool)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id = $1")
            .bind(limited_status_id)
            .execute(&writer_pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO list_accounts
            SELECT * FROM jsonb_populate_record(NULL::list_accounts, $1)",
    )
    .bind(exclusive_membership)
    .execute(&writer_pool)
    .await?;
    sqlx::query("UPDATE account_stats SET statuses_count = $2 WHERE account_id = $1")
        .bind(BOB)
        .bind(baseline_statuses_count)
        .execute(&writer_pool)
        .await?;
    drop(media_root);
    fs::remove_dir_all(&media_root_path)?;
    result
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn uri_only_create_is_deduplicated_retried_materialized_and_replayed()
-> Result<(), Box<dyn std::error::Error>> {
    const BOB: i64 = 116_844_606_259_202_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const API_MODERATOR: i64 = 116_844_606_259_201_004;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
    const KEY_ID: &str = "https://remote.fixture.invalid/users/bob#secondary-key";
    const ACTIVITY_URI: &str = "http://relay.fixture.invalid/activities/rustodon-uri-create";
    const ALTERNATE_ACTIVITY_URI: &str =
        "http://another-relay.fixture.invalid/activities/rustodon-uri-create-copy";
    const NOTE_URI: &str = "http://remote.fixture.invalid/users/bob/statuses/rustodon-uri-create";
    const DELETED_ACTIVITY_URI: &str =
        "http://remote.fixture.invalid/activities/rustodon-uri-create-deleted";
    const DELETED_NOTE_URI: &str =
        "http://remote.fixture.invalid/users/bob/statuses/rustodon-uri-create-deleted";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let baseline_statuses_count: i64 =
        sqlx::query_scalar("SELECT statuses_count FROM account_stats WHERE account_id = $1")
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?;
    let (parent_username, parent_id_scheme) = sqlx::query_as::<_, (String, Option<i32>)>(
        "SELECT username, id_scheme FROM accounts WHERE id = $1",
    )
    .bind(MODERATOR)
    .fetch_one(&writer_pool)
    .await?;
    let parent_id: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (
             account_id, text, spoiler_text, visibility, local, sensitive, reply,
             created_at, updated_at)
         VALUES ($1, 'URI resolver forwarding target', '', 0, true, false, false,
                 clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(MODERATOR)
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at)
         VALUES ($1, clock_timestamp(), clock_timestamp())",
    )
    .bind(parent_id)
    .execute(&writer_pool)
    .await?;
    let parent_uri = if parent_id_scheme == Some(1) {
        format!(
            "{}/ap/users/{MODERATOR}/statuses/{parent_id}",
            ORIGIN.trim_end_matches('/')
        )
    } else {
        format!(
            "{}/users/{parent_username}/statuses/{parent_id}",
            ORIGIN.trim_end_matches('/')
        )
    };
    let previous_parent_follower: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(follow) FROM follows follow
          WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(-320_i64)
    .bind(MODERATOR)
    .fetch_optional(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(-320_i64)
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "INSERT INTO follows
             (account_id, target_account_id, show_reblogs, notify, languages, uri,
              created_at, updated_at)
         VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(-320_i64)
    .bind(MODERATOR)
    .bind("https://account-blocked.fixture.invalid/users/domain_viewer#follows/uri-resolver")
    .execute(&writer_pool)
    .await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let note = json!({
        "id": NOTE_URI,
        "type": "Note",
        "attributedTo": ACTOR,
        "published": "2026-08-25T12:20:00Z",
        "inReplyTo": parent_uri,
        "content": "<p>Fetched durable Note</p>",
        "summary": null,
        "to": [
            "https://fixture-v4-6-5.rustodon.invalid/users/moderator",
            "https://fixture-v4-6-5.rustodon.invalid/users/api_moderator"
        ],
        "cc": [],
        "tag": [],
        "attachment": []
    })
    .to_string()
    .into_bytes();
    let server = tokio::spawn(fixture_retry_activitypub_server(listener, note));
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            media_root_url: "/system".to_owned(),
            media_root: None,
            limited_federation: false,
            remote_media_endpoint: None,
            remote_delivery_endpoint: None,
            remote_fetch_endpoint: Some(endpoint),
        }),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let body = json!({
        "id": ACTIVITY_URI,
        "type": "Create",
        "actor": ACTOR,
        "object": NOTE_URI,
        "to": [
            "https://fixture-v4-6-5.rustodon.invalid/users/moderator",
            "https://fixture-v4-6-5.rustodon.invalid/users/api_moderator"
        ],
        "cc": [],
        "signature": {
            "type": "RsaSignature2017",
            "creator": KEY_ID,
            "created": "2026-08-25T12:20:00Z",
            "signatureValue": "fixture"
        }
    })
    .to_string();
    let duplicate_body = body.replace(ACTIVITY_URI, ALTERNATE_ACTIVITY_URI);
    let result = async {
        for (logical_key, delivery_target_account_id, delivered_body) in [
            ("activitypub:test-uri-create", MODERATOR, body.as_str()),
            (
                "activitypub:test-uri-create-duplicate",
                MODERATOR,
                duplicate_body.as_str(),
            ),
            (
                "activitypub:test-uri-create-second-recipient",
                API_MODERATOR,
                body.as_str(),
            ),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": delivered_body,
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid",
                            "delivery_target_account_id": delivery_target_account_id
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one("uri-create-ingress", &[Lane::Ingress], Duration::seconds(30))
                    .await?
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1"
            )
            .bind(ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND)
            .fetch_one(&writer_pool)
            .await?,
            2,
            "alternate activity IDs must deduplicate without suppressing another personal inbox"
        );
        assert_eq!(queue.dispatch_outbox(10).await?, 2);
        let resolution_arguments: Value = sqlx::query_scalar(
            "SELECT arguments FROM rustodon.durable_jobs
              WHERE kind = $1 AND arguments ->> 'delivery_target_account_id' = $2",
        )
        .bind(ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND)
        .bind(MODERATOR.to_string())
        .fetch_one(&runtime_pool)
        .await?;
        assert_eq!(resolution_arguments["activity_uri"], ACTIVITY_URI);
        assert_eq!(resolution_arguments["actor_uri"], ACTOR);
        assert_eq!(resolution_arguments["object_uri"], NOTE_URI);
        assert_eq!(
            resolution_arguments["delivery_target_account_id"],
            MODERATOR
        );

        assert!(
            executor
                .process_one("uri-create-pull", &[Lane::Pull], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE uri = $1")
                .bind(NOTE_URI)
                .fetch_one(&writer_pool)
                .await?,
            0
        );
        let retry = sqlx::query_as::<_, (i32, i32, Option<String>)>(
            "SELECT attempts, max_attempts, last_error FROM rustodon.durable_jobs
              WHERE kind = $1 AND arguments ->> 'delivery_target_account_id' = $2",
        )
        .bind(ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND)
        .bind(MODERATOR.to_string())
        .fetch_one(&runtime_pool)
        .await?;
        assert_eq!(retry.0, 1);
        assert_eq!(retry.1, 25);
        assert!(retry.2.is_some());
        assert!(
            executor
                .process_one("uri-create-second-recipient", &[Lane::Pull], Duration::seconds(30))
                .await?
        );
        sqlx::query(
            "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp()
              WHERE kind = $1 AND arguments ->> 'delivery_target_account_id' = $2",
        )
        .bind(ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND)
        .bind(MODERATOR.to_string())
        .execute(&runtime_pool)
        .await?;
        assert!(
            executor
                .process_one("uri-create-pull", &[Lane::Pull], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses WHERE uri = $1 AND account_id = $2 AND deleted_at IS NULL"
            )
            .bind(NOTE_URI)
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, Vec<i64>>(
                "SELECT array_agg(account_id ORDER BY account_id) FROM mentions
                  WHERE status_id = (SELECT id FROM statuses WHERE uri = $1)
                    AND account_id = ANY($2)",
            )
            .bind(NOTE_URI)
            .bind(vec![MODERATOR, API_MODERATOR])
            .fetch_one(&writer_pool)
            .await?,
            vec![MODERATOR, API_MODERATOR],
            "each personal delivery must preserve access through a silent mention"
        );

        sqlx::query("UPDATE statuses SET visibility = 0 WHERE uri = $1")
            .bind(NOTE_URI)
            .execute(&writer_pool)
            .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1 AND payload -> 'arguments' -> 'body' ->> 'id' = $2",
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .bind(ACTIVITY_URI)
            .fetch_one(&writer_pool)
            .await?,
            0,
            "the simulated crash boundary starts without forwarding work"
        );

        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Pull,
                    ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND,
                    resolution_arguments,
                )
                .logical_key("activitypub:test-uri-create-crash-replay"),
            )
            .await?;
        assert!(
            executor
                .process_one("uri-create-replay", &[Lane::Pull], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE uri = $1")
                .bind(NOTE_URI)
                .fetch_one(&writer_pool)
                .await?,
            1,
            "replay after materialization must not duplicate or refetch the Note"
        );
        let forwarding_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1 AND payload -> 'arguments' -> 'body' ->> 'id' = $2",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(ACTIVITY_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert!(
            forwarding_count > 0,
            "replay after the materialization boundary must restore forwarding work"
        );
        sqlx::query(
            "DELETE FROM rustodon.outbox_events
              WHERE kind = $1 AND payload -> 'arguments' -> 'body' ->> 'id' = $2",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(ACTIVITY_URI)
        .execute(&writer_pool)
        .await?;
        let requests = server.await??;
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| {
            String::from_utf8_lossy(request).starts_with("GET /users/bob/statuses/rustodon-uri-create")
        }));

        for (logical_key, activity) in [
            (
                "activitypub:test-uri-create-before-delete",
                json!({
                    "id": DELETED_ACTIVITY_URI,
                    "type": "Create",
                    "actor": ACTOR,
                    "object": DELETED_NOTE_URI
                }),
            ),
            (
                "activitypub:test-uri-create-delete",
                json!({
                    "type": "Delete",
                    "actor": ACTOR,
                    "object": DELETED_NOTE_URI
                }),
            ),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": activity.to_string(),
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid",
                            "delivery_target_account_id": MODERATOR
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one("uri-create-ordering", &[Lane::Ingress], Duration::seconds(30))
                    .await?
            );
        }
        assert_eq!(queue.dispatch_outbox(10).await?, 1);
        assert!(
            executor
                .process_one("uri-create-ordering", &[Lane::Pull], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE uri = $1")
                .bind(DELETED_NOTE_URI)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "Delete-before-resolution must prevent materialization"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM tombstones WHERE account_id = $1 AND uri = $2"
            )
            .bind(BOB)
            .bind(DELETED_NOTE_URI)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    sqlx::query("DELETE FROM statuses WHERE uri = $1")
        .bind(NOTE_URI)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM tombstones WHERE account_id = $1 AND uri = $2")
        .bind(BOB)
        .bind(DELETED_NOTE_URI)
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE account_stats SET statuses_count = $2 WHERE account_id = $1")
        .bind(BOB)
        .bind(baseline_statuses_count)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(-320_i64)
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;
    if let Some(previous_parent_follower) = previous_parent_follower {
        sqlx::query(
            "INSERT INTO follows
             SELECT * FROM jsonb_populate_record(NULL::follows, $1)",
        )
        .bind(previous_parent_follower)
        .execute(&writer_pool)
        .await?;
    }
    sqlx::query("DELETE FROM status_stats WHERE status_id = $1")
        .bind(parent_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(parent_id)
        .execute(&writer_pool)
        .await?;
    result
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_media_fetch_fails_closed_without_losing_the_status()
-> Result<(), Box<dyn std::error::Error>> {
    const BOB: i64 = 116_844_606_259_202_001;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const POLICY_DOMAIN: &str = "remote.fixture.invalid";
    const MEDIA_REMOTE_URL: &str = "https://127.0.0.1/rustodon-worker-media.jpg";
    const BLOCKED_MEDIA_REMOTE_URL: &str =
        "https://127.0.0.1/rustodon-worker-domain-blocked-media.jpg";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let writer_url = std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let owner_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&writer_url)
        .await?;
    reset().await?;
    let root_path =
        std::env::temp_dir().join(format!("rustodon-worker-media-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root_path);
    fs::create_dir(&root_path)?;
    let media_root = PaperclipRoot::open(&root_path)?;
    let original_domain_allow: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(domain_allow) FROM domain_allows domain_allow WHERE domain = $1",
    )
    .bind(POLICY_DOMAIN)
    .fetch_optional(&owner_pool)
    .await?;
    let original_domain_block: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(domain_block) FROM domain_blocks domain_block WHERE domain = $1",
    )
    .bind(POLICY_DOMAIN)
    .fetch_optional(&owner_pool)
    .await?;

    let result = async {
        sqlx::query("DELETE FROM domain_allows WHERE domain = $1")
            .bind(POLICY_DOMAIN)
            .execute(&owner_pool)
            .await?;
        sqlx::query("DELETE FROM domain_blocks WHERE domain = $1")
            .bind(POLICY_DOMAIN)
            .execute(&owner_pool)
            .await?;
        sqlx::query(
            "INSERT INTO domain_allows (domain, created_at, updated_at)
             VALUES ($1, clock_timestamp(), clock_timestamp())",
        )
        .bind(POLICY_DOMAIN)
        .execute(&owner_pool)
        .await?;

        let status_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses WHERE account_id = $1 AND deleted_at IS NULL ORDER BY id LIMIT 1",
        )
        .bind(BOB)
        .fetch_one(&owner_pool)
        .await?;
        let status_text =
            sqlx::query_scalar::<_, String>("SELECT text FROM statuses WHERE id = $1")
                .bind(status_id)
                .fetch_one(&owner_pool)
                .await?;
        // A lease-fenced handler can leave the attachment marked as processing.
        let media_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO media_attachments (
                 account_id, status_id, type, processing, remote_url, file_content_type, file_meta,
                 created_at, updated_at)
             VALUES ($1, $2, 0, 1, $3, 'image/jpeg', '{}'::json,
                     clock_timestamp(), clock_timestamp())
             RETURNING id",
        )
        .bind(BOB)
        .bind(status_id)
        .bind(MEDIA_REMOTE_URL)
        .fetch_one(&owner_pool)
        .await?;
        let config = ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            media_root_url: "/system".to_owned(),
            media_root: Some(media_root),
            limited_federation: true,
            #[cfg(feature = "test-support")]
            remote_media_endpoint: None,
            #[cfg(feature = "test-support")]
            remote_delivery_endpoint: None,
            #[cfg(feature = "test-support")]
            remote_fetch_endpoint: None,
        };
        let queue = Queue::new(runtime_pool.clone());
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(writer_pool.clone()),
            None,
            Some(config),
        )?;
        let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Pull,
                    ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
                    json!({"media_id": media_id}),
                )
                .logical_key("activitypub:test-media-fetch"),
            )
            .await?;
        let media_processed = executor
            .process_one("media-worker", &[Lane::Pull], Duration::seconds(30))
            .await?;
        let media_state = sqlx::query_as::<_, (Option<i32>, Option<String>)>(
            "SELECT processing, file_file_name FROM media_attachments WHERE id = $1",
        )
        .bind(media_id)
        .fetch_one(&owner_pool)
        .await?;
        let media_status_text =
            sqlx::query_scalar::<_, String>("SELECT text FROM statuses WHERE id = $1")
                .bind(status_id)
                .fetch_one(&owner_pool)
                .await?;

        sqlx::query(
            "INSERT INTO domain_blocks (
                 domain, severity, reject_media, reject_reports, private_comment,
                 public_comment, obfuscate, created_at, updated_at)
             VALUES ($1, 0, true, false, NULL, NULL, false,
                     clock_timestamp(), clock_timestamp())",
        )
        .bind(POLICY_DOMAIN)
        .execute(&owner_pool)
        .await?;
        let blocked_media_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO media_attachments (
                 account_id, status_id, type, processing, remote_url, file_content_type, file_meta,
                 created_at, updated_at)
             VALUES ($1, $2, 0, 0, $3, 'image/jpeg', '{}'::json,
                     clock_timestamp(), clock_timestamp())
             RETURNING id",
        )
        .bind(BOB)
        .bind(status_id)
        .bind(BLOCKED_MEDIA_REMOTE_URL)
        .fetch_one(&owner_pool)
        .await?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Pull,
                    ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
                    json!({"media_id": blocked_media_id}),
                )
                .logical_key("activitypub:test-media-fetch-domain-block"),
            )
            .await?;
        let blocked_media_processed = executor
            .process_one("media-worker", &[Lane::Pull], Duration::seconds(30))
            .await?;
        let blocked_media_state = sqlx::query_as::<_, (Option<i32>, Option<String>)>(
            "SELECT processing, file_file_name FROM media_attachments WHERE id = $1",
        )
        .bind(blocked_media_id)
        .fetch_one(&owner_pool)
        .await?;
        let blocked_status_text =
            sqlx::query_scalar::<_, String>("SELECT text FROM statuses WHERE id = $1")
                .bind(status_id)
                .fetch_one(&owner_pool)
                .await?;
        Ok::<_, Box<dyn std::error::Error>>(
            (media_processed, media_state, media_status_text, blocked_media_processed,
             blocked_media_state, blocked_status_text, status_text),
        )
    }
    .await;

    let cleanup_result = async {
        sqlx::query("DELETE FROM media_attachments WHERE remote_url = ANY($1)")
            .bind(vec![
                MEDIA_REMOTE_URL.to_owned(),
                BLOCKED_MEDIA_REMOTE_URL.to_owned(),
            ])
            .execute(&owner_pool)
            .await?;
        sqlx::query("DELETE FROM domain_allows WHERE domain = $1")
            .bind(POLICY_DOMAIN)
            .execute(&owner_pool)
            .await?;
        if let Some(original_domain_allow) = original_domain_allow {
            sqlx::query(
                "INSERT INTO domain_allows
                 SELECT * FROM jsonb_populate_record(NULL::domain_allows, $1)",
            )
            .bind(original_domain_allow)
            .execute(&owner_pool)
            .await?;
        }
        sqlx::query("DELETE FROM domain_blocks WHERE domain = $1")
            .bind(POLICY_DOMAIN)
            .execute(&owner_pool)
            .await?;
        if let Some(original_domain_block) = original_domain_block {
            sqlx::query(
                "INSERT INTO domain_blocks
                 SELECT * FROM jsonb_populate_record(NULL::domain_blocks, $1)",
            )
            .bind(original_domain_block)
            .execute(&owner_pool)
            .await?;
        }
        Ok::<_, sqlx::Error>(())
    }
    .await;
    let _ = fs::remove_dir_all(&root_path);
    cleanup_result?;

    let (
        media_processed,
        media_state,
        media_status_text,
        blocked_media_processed,
        blocked_media_state,
        blocked_status_text,
        status_text,
    ) = result?;
    assert!(media_processed);
    assert_eq!(media_state, (Some(3), None));
    assert_eq!(media_status_text, status_text);
    assert!(blocked_media_processed);
    assert_eq!(blocked_media_state, (Some(3), None));
    assert_eq!(blocked_status_text, status_text);
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_emoji_fetch_retries_and_installs_original_and_static_files()
-> Result<(), Box<dyn std::error::Error>> {
    const OWNER_DOMAIN: &str = "remote.fixture.invalid";
    const LOGICAL_KEY: &str = "activitypub:test-emoji-fetch-retry";
    const REPLACEMENT_KEY: &str = "activitypub:test-emoji-replacement";
    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let root_path = std::env::temp_dir().join(format!(
        "rustodon-worker-emoji-success-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root_path);
    fs::create_dir(&root_path)?;
    let media_root = PaperclipRoot::open(&root_path)?
        .with_write_fault(PaperclipWriteFault::storage_full_after(1));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let body = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.gif")?;
    let remote_url = format!("http://media.fixture.invalid:{}/emoji.gif", endpoint.port());
    let emoji_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO custom_emojis
             (shortcode, domain, image_remote_url, disabled, visible_in_picker,
              created_at, updated_at)
         VALUES ('retry_blob', $1, $2, false, true, clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .bind(OWNER_DOMAIN)
    .bind(&remote_url)
    .fetch_one(&writer_pool)
    .await?;
    let server = tokio::spawn(fixture_media_server_for_retries(listener, body.clone(), 2));
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: Some(media_root.clone()),
        limited_federation: false,
        remote_media_endpoint: Some(endpoint),
        remote_delivery_endpoint: None,
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config.clone()),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Pull,
                ACTIVITYPUB_EMOJI_FETCH_JOB_KIND,
                json!({
                    "emoji_id": emoji_id,
                    "remote_url": remote_url,
                    "media_type": "image/gif",
                    "domain": OWNER_DOMAIN
                }),
            )
            .logical_key(LOGICAL_KEY)
            .max_attempts(4),
        )
        .await?;
    assert!(
        executor
            .process_one("emoji-worker", &[Lane::Pull], Duration::seconds(30))
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT image_file_name FROM custom_emojis WHERE id = $1",
        )
        .bind(emoji_id)
        .fetch_one(&writer_pool)
        .await?,
        None,
    );
    sqlx::query(
        "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE logical_key = $1",
    )
    .bind(LOGICAL_KEY)
    .execute(&runtime_pool)
    .await?;
    assert!(
        executor
            .process_one("emoji-worker-retry", &[Lane::Pull], Duration::seconds(30))
            .await?
    );
    server.await??;
    let (file_name, content_type) = sqlx::query_as::<_, (String, String)>(
        "SELECT image_file_name, image_content_type FROM custom_emojis WHERE id = $1",
    )
    .bind(emoji_id)
    .fetch_one(&writer_pool)
    .await?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::CustomEmojiImage,
        id: emoji_id,
        remote: true,
        storage_schema_version: Some(1),
        file_name,
        content_type: Some(content_type),
        variant: None,
    };
    let original_path = metadata.relative_path("original").expect("original path");
    let static_path = metadata.relative_path("static").expect("static path");
    assert!(media_root.open_file(Path::new(&original_path)).is_ok());
    assert!(media_root.open_file(Path::new(&static_path)).is_ok());

    drop(executor);
    let replacement_listener = TcpListener::bind("127.0.0.1:0").await?;
    let replacement_endpoint = replacement_listener.local_addr()?;
    let replacement_body = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/avatar.gif")?;
    let replacement_url = format!(
        "http://media.fixture.invalid:{}/replacement.gif",
        replacement_endpoint.port()
    );
    sqlx::query("UPDATE custom_emojis SET image_remote_url = $2 WHERE id = $1")
        .bind(emoji_id)
        .bind(&replacement_url)
        .execute(&writer_pool)
        .await?;
    let faulted_root = PaperclipRoot::open(&root_path)?
        .with_commit_fault(PaperclipCommitFault::before_and_after())
        .with_remove_fault(PaperclipRemoveFault::fail_once());
    let replacement_server = tokio::spawn(fixture_media_server_for_retries(
        replacement_listener,
        replacement_body,
        3,
    ));
    let replacement_config = ActivityPubDeliveryConfig {
        remote_media_endpoint: Some(replacement_endpoint),
        media_root: Some(faulted_root),
        ..config
    };
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(replacement_config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Pull,
                ACTIVITYPUB_EMOJI_FETCH_JOB_KIND,
                json!({
                    "emoji_id": emoji_id,
                    "remote_url": replacement_url,
                    "media_type": "image/gif",
                    "domain": OWNER_DOMAIN
                }),
            )
            .logical_key(REPLACEMENT_KEY)
            .max_attempts(4),
        )
        .await?;
    for worker in [
        "emoji-replace-before",
        "emoji-replace-after",
        "emoji-replace-reconciled",
    ] {
        assert!(
            executor
                .process_one(worker, &[Lane::Pull], Duration::seconds(30))
                .await?
        );
        sqlx::query(
            "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE logical_key = $1",
        )
        .bind(REPLACEMENT_KEY)
        .execute(&runtime_pool)
        .await?;
    }
    replacement_server.await??;
    let replacement_file_name =
        sqlx::query_scalar::<_, String>("SELECT image_file_name FROM custom_emojis WHERE id = $1")
            .bind(emoji_id)
            .fetch_one(&writer_pool)
            .await?;
    assert_ne!(replacement_file_name, metadata.file_name);
    let replacement_metadata = PaperclipMetadata {
        file_name: replacement_file_name,
        ..metadata.clone()
    };
    assert!(
        executor
            .process_one(
                "emoji-cleanup-retry",
                &[Lane::Maintenance],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.durable_jobs
              WHERE kind = $1 AND attempts = 1 AND dead_at IS NULL",
        )
        .bind(ACTIVITYPUB_EMOJI_CLEANUP_JOB_KIND)
        .fetch_one(&runtime_pool)
        .await?,
        1,
        "a failed obsolete-file unlink remains durably retryable"
    );
    sqlx::query("UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE kind = $1")
        .bind(ACTIVITYPUB_EMOJI_CLEANUP_JOB_KIND)
        .execute(&runtime_pool)
        .await?;
    while executor
        .process_one(
            "emoji-cleanup-reconciled",
            &[Lane::Maintenance],
            Duration::seconds(30),
        )
        .await?
    {}
    for path in [original_path, static_path] {
        assert!(media_root.open_file(Path::new(&path)).is_err());
    }
    for style in ["original", "static"] {
        let path = replacement_metadata
            .relative_path(style)
            .expect("replacement path");
        assert!(media_root.open_file(Path::new(&path)).is_ok());
    }
    sqlx::query("DELETE FROM custom_emojis WHERE id = $1")
        .bind(emoji_id)
        .execute(&writer_pool)
        .await?;
    drop(executor);
    let _ = fs::remove_dir_all(root_path);
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn local_media_jobs_reconcile_create_and_delete_crash_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    const ALICE: i64 = 116_844_606_259_201_001;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let root_path = std::env::temp_dir().join(format!(
        "rustodon-local-media-durability-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root_path);
    fs::create_dir(&root_path)?;
    let media_root = PaperclipRoot::open(&root_path)?;
    let writer = WriteRepository::from_pool(writer_pool.clone());
    let repository = Repository::from_pool(writer_pool.clone());
    let authenticator = BearerAuthenticator::new(repository.clone());
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_MEDIA).await?;
    let bytes = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.jpg")?;
    let prepared = prepare_media_attachment(ALICE, "local.jpg", "image/jpeg", &bytes)?;
    let create = MediaAttachmentCreate {
        file_name: prepared.file_name.clone(),
        content_type: prepared.content_type.clone(),
        file_size: prepared.file_size,
        file_meta: prepared.file_meta.clone(),
        blurhash: prepared.blurhash.clone(),
        description: Some("durable local media".to_owned()),
        focus: AccountProfileValue::Unchanged,
    };
    let queue = Queue::new(runtime_pool.clone());
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: Some(media_root.clone()),
        limited_federation: false,
        remote_media_endpoint: None,
        remote_delivery_endpoint: None,
        remote_fetch_endpoint: None,
    };
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config.clone()),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;

    let abandoned_id = writer
        .stage_media_attachment(&authenticated, &create)
        .await?;
    assert_eq!(
        repository
            .media_attachment(ALICE, abandoned_id)
            .await?
            .and_then(|media| media.file_file_name),
        None,
        "staged creation has no published expected-file metadata"
    );
    let abandoned_metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: abandoned_id,
        remote: false,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let abandoned_paths = write_prepared_media(&media_root, &abandoned_metadata, &prepared)?;
    assert_eq!(queue.dispatch_outbox(100).await?, 1);
    assert!(
        executor
            .process_one(
                "local-create-rollback",
                &[Lane::Maintenance],
                Duration::seconds(30)
            )
            .await?
    );
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM media_attachments WHERE id = $1)"
        )
        .bind(abandoned_id)
        .fetch_one(&writer_pool)
        .await?
    );
    for path in abandoned_paths {
        assert!(media_root.open_file(Path::new(&path)).is_err());
    }

    let published_id = writer
        .stage_media_attachment(&authenticated, &create)
        .await?;
    let published_metadata = PaperclipMetadata {
        id: published_id,
        ..abandoned_metadata
    };
    let published_paths = write_prepared_media(&media_root, &published_metadata, &prepared)?;
    writer
        .publish_media_attachment(&authenticated, published_id, &create)
        .await?;
    assert!(
        repository
            .media_attachment(ALICE, published_id)
            .await?
            .is_some()
    );
    assert_eq!(queue.dispatch_outbox(100).await?, 1);
    assert!(
        executor
            .process_one(
                "local-create-published",
                &[Lane::Maintenance],
                Duration::seconds(30)
            )
            .await?
    );
    for path in &published_paths {
        assert!(media_root.open_file(Path::new(path)).is_ok());
    }

    let faulted_writer = writer.clone().with_local_media_cleanup_intent_fault();
    assert!(
        cleanup_media_after_response_failure_for_test(
            &media_root,
            &faulted_writer,
            &authenticated,
            published_id,
        )
        .await
        .is_err(),
        "a failed cleanup-intent write rejects response-failure cleanup"
    );
    assert!(
        repository
            .media_attachment(ALICE, published_id)
            .await?
            .is_some(),
        "the failed transaction preserves published metadata"
    );
    for path in &published_paths {
        assert!(
            media_root.open_file(Path::new(path)).is_ok(),
            "published files remain until cleanup intent commits"
        );
    }

    let faulted_root =
        PaperclipRoot::open(&root_path)?.with_remove_fault(PaperclipRemoveFault::fail_once());
    writer
        .delete_media_attachment(&authenticated, published_id)
        .await?;
    assert!(
        repository
            .media_attachment(ALICE, published_id)
            .await?
            .is_none(),
        "metadata deletion and cleanup intent commit atomically"
    );
    for path in &published_paths {
        let _ = faulted_root.remove_file(Path::new(path));
    }
    assert!(
        published_paths
            .iter()
            .any(|path| faulted_root.open_file(Path::new(path)).is_ok())
    );

    assert_eq!(queue.dispatch_outbox(100).await?, 1);
    let worker_root =
        PaperclipRoot::open(&root_path)?.with_remove_fault(PaperclipRemoveFault::fail_once());
    let faulted_config = ActivityPubDeliveryConfig {
        media_root: Some(worker_root.clone()),
        ..config.clone()
    };
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(faulted_config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    assert!(
        executor
            .process_one(
                "local-delete-retry",
                &[Lane::Maintenance],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT attempts FROM rustodon.durable_jobs WHERE kind = $1")
            .bind(LOCAL_MEDIA_CLEANUP_JOB_KIND)
            .fetch_one(&runtime_pool)
            .await?,
        1,
        "a failed worker unlink remains durably retryable"
    );
    sqlx::query("UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE kind = $1")
        .bind(LOCAL_MEDIA_CLEANUP_JOB_KIND)
        .execute(&runtime_pool)
        .await?;
    assert!(
        executor
            .process_one(
                "local-delete-recovered",
                &[Lane::Maintenance],
                Duration::seconds(30)
            )
            .await?
    );
    for path in &published_paths {
        assert!(worker_root.open_file(Path::new(path)).is_err());
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1")
            .bind(LOCAL_MEDIA_CLEANUP_JOB_KIND)
            .fetch_one(&runtime_pool)
            .await?,
        0
    );

    let status_media_id = writer
        .stage_media_attachment(&authenticated, &create)
        .await?;
    let status_metadata = PaperclipMetadata {
        id: status_media_id,
        ..published_metadata
    };
    let mut status_paths = write_prepared_media(&media_root, &status_metadata, &prepared)?;
    writer
        .publish_media_attachment(&authenticated, status_media_id, &create)
        .await?;
    let thumbnail_metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaThumbnail,
        file_name: "status-thumbnail.jpg".to_owned(),
        content_type: Some("image/jpeg".to_owned()),
        ..status_metadata
    };
    let thumbnail_path = thumbnail_metadata
        .relative_path("original")
        .expect("status thumbnail path");
    media_root.write_file(Path::new(&thumbnail_path), b"status thumbnail")?;
    status_paths.push(thumbnail_path);
    sqlx::query(
        "UPDATE media_attachments SET thumbnail_content_type = 'image/jpeg', \
             thumbnail_file_name = 'status-thumbnail.jpg', thumbnail_file_size = 16, \
             thumbnail_storage_schema_version = 1, thumbnail_updated_at = clock_timestamp() \
           WHERE id = $1",
    )
    .bind(status_media_id)
    .execute(&writer_pool)
    .await?;
    assert_eq!(queue.dispatch_outbox(100).await?, 1);
    assert!(
        executor
            .process_one(
                "status-media-create-published",
                &[Lane::Maintenance],
                Duration::seconds(30)
            )
            .await?
    );
    let status = writer
        .create_status(
            &authenticated,
            "durable status media cleanup",
            &[status_media_id],
            None,
            Some(false),
            Some("public"),
            None,
            None,
            None,
        )
        .await?;

    let faulted_writer = writer.clone().with_local_media_cleanup_intent_fault();
    assert!(
        faulted_writer
            .delete_status(&authenticated, status.status_id, true)
            .await
            .is_err(),
        "status deletion rejects a failed cleanup-intent write"
    );
    assert!(
        sqlx::query_scalar::<_, Option<NaiveDateTime>>(
            "SELECT deleted_at FROM statuses WHERE id = $1"
        )
        .bind(status.status_id)
        .fetch_one(&writer_pool)
        .await?
        .is_none(),
        "status deletion rolls back with its cleanup intents"
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM media_attachments WHERE id = $1 AND status_id = $2)"
        )
        .bind(status_media_id)
        .bind(status.status_id)
        .fetch_one(&writer_pool)
        .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1 AND payload -> 'arguments' ->> 'media_id' = $2
                AND payload -> 'arguments' ->> 'action' = 'delete'"
        )
        .bind(LOCAL_MEDIA_CLEANUP_JOB_KIND)
        .bind(status_media_id.to_string())
        .fetch_one(&writer_pool)
        .await?,
        0,
        "a rolled-back status deletion leaves no cleanup intent"
    );

    writer
        .delete_status(&authenticated, status.status_id, true)
        .await?;
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM media_attachments WHERE id = $1)"
        )
        .bind(status_media_id)
        .fetch_one(&writer_pool)
        .await?,
        "status metadata deletion and cleanup intent commit atomically"
    );
    for path in &status_paths {
        assert!(
            media_root.open_file(Path::new(path)).is_ok(),
            "a crash after status deletion leaves files for the durable worker"
        );
    }
    assert!(
        queue.dispatch_outbox(100).await? >= 1,
        "status deletion dispatches its local-media cleanup intent"
    );
    drop(executor);
    let status_worker_root =
        PaperclipRoot::open(&root_path)?.with_remove_fault(PaperclipRemoveFault::fail_once());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(ActivityPubDeliveryConfig {
            media_root: Some(status_worker_root.clone()),
            ..config
        }),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    assert!(
        executor
            .process_one(
                "status-media-delete-retry",
                &[Lane::Maintenance],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT attempts FROM rustodon.durable_jobs WHERE kind = $1")
            .bind(LOCAL_MEDIA_CLEANUP_JOB_KIND)
            .fetch_one(&runtime_pool)
            .await?,
        1,
        "a failed status-media unlink remains durably retryable"
    );
    sqlx::query("UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE kind = $1")
        .bind(LOCAL_MEDIA_CLEANUP_JOB_KIND)
        .execute(&runtime_pool)
        .await?;
    assert!(
        executor
            .process_one(
                "status-media-delete-recovered",
                &[Lane::Maintenance],
                Duration::seconds(30)
            )
            .await?
    );
    for path in &status_paths {
        assert!(status_worker_root.open_file(Path::new(path)).is_err());
    }

    drop(executor);
    drop(status_worker_root);
    drop(worker_root);
    drop(faulted_root);
    drop(media_root);
    fs::remove_dir_all(root_path)?;
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_media_fetch_reclaims_after_lease_fence()
-> Result<(), Box<dyn std::error::Error>> {
    const BOB: i64 = 116_844_606_259_202_001;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const LOGICAL_KEY: &str = "activitypub:test-media-fetch-lease-fence";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let root_path = std::env::temp_dir().join(format!(
        "rustodon-worker-media-lease-fence-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root_path);
    fs::create_dir(&root_path)?;
    let media_root = PaperclipRoot::open(&root_path)?;
    let status_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM statuses WHERE account_id = $1 AND deleted_at IS NULL ORDER BY id LIMIT 1",
    )
    .bind(BOB)
    .fetch_one(&writer_pool)
    .await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let body = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.gif")?;
    let request_started = Arc::new(Notify::new());
    let release_request = Arc::new(Notify::new());
    let remote_url = format!(
        "http://media.fixture.invalid:{}/lease-fence.gif",
        endpoint.port()
    );
    let media_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO media_attachments (
             account_id, status_id, type, processing, remote_url, file_content_type, file_meta,
             created_at, updated_at)
         VALUES ($1, $2, 0, 0, $3, 'image/gif', '{}'::json,
                 clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .bind(BOB)
    .bind(status_id)
    .bind(&remote_url)
    .fetch_one(&writer_pool)
    .await?;
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: Some(media_root.clone()),
        limited_federation: false,
        remote_media_endpoint: Some(endpoint),
        remote_delivery_endpoint: None,
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Pull,
                ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
                json!({"media_id": media_id}),
            )
            .logical_key(LOGICAL_KEY),
        )
        .await?;
    let mut server = tokio::spawn(fixture_media_server_with_lease_barrier(
        listener,
        body.clone(),
        Arc::clone(&request_started),
        Arc::clone(&release_request),
    ));

    let first_executor = executor.clone();
    let mut first: Option<tokio::task::JoinHandle<Result<bool, WorkerError>>> =
        Some(tokio::spawn(async move {
            first_executor
                .process_one(
                    "fenced-media-worker",
                    &[Lane::Pull],
                    Duration::milliseconds(150),
                )
                .await
        }));
    let operation = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            request_started.notified(),
        )
        .await
        .map_err(|_| std::io::Error::other("media fixture did not receive the first request"))?;
        assert_eq!(
            sqlx::query(
                "UPDATE rustodon.durable_jobs
                    SET lease_expires_at = clock_timestamp() - interval '1 second'
                  WHERE logical_key = $1 AND dead_at IS NULL",
            )
            .bind(LOGICAL_KEY)
            .execute(&runtime_pool)
            .await?
            .rows_affected(),
            1,
            "the live media job must be fenced before recovery"
        );
        let first_processed = match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            first.as_mut().expect("the first worker task is present"),
        )
        .await
        {
            Ok(result) => {
                let result = result??;
                first.take();
                result
            }
            Err(error) => {
                first
                    .as_mut()
                    .expect("the first worker task is present")
                    .abort();
                let _ = first
                    .take()
                    .expect("the first worker task is present")
                    .await;
                return Err(Box::new(error) as Box<dyn std::error::Error>);
            }
        };
        assert!(first_processed);
        assert_eq!(
            sqlx::query_as::<_, (Option<i32>, Option<String>)>(
                "SELECT processing, file_file_name FROM media_attachments WHERE id = $1",
            )
            .bind(media_id)
            .fetch_one(&writer_pool)
            .await?,
            (Some(1), None),
            "a fenced handler must not acknowledge or reset the claimed attachment"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs
                  WHERE logical_key = $1 AND dead_at IS NULL",
            )
            .bind(LOGICAL_KEY)
            .fetch_one(&runtime_pool)
            .await?,
            1,
            "a fenced handler must leave the durable job recoverable"
        );

        release_request.notify_one();
        let recovered = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            executor.process_one("recovery-media-worker", &[Lane::Pull], Duration::seconds(5)),
        )
        .await??;
        assert!(recovered);

        let media_state = sqlx::query_as::<_, (Option<i32>, String, Option<i32>, Value)>(
            "SELECT processing, file_file_name, file_file_size, file_meta
               FROM media_attachments WHERE id = $1",
        )
        .bind(media_id)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(media_state.0, Some(2));
        assert!(media_state.2.is_some_and(|size| size > 0));
        assert!(media_state.3["original"]["size"].is_string());
        assert!(media_state.3["small"]["size"].is_string());
        let metadata = PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: media_id,
            remote: true,
            storage_schema_version: Some(1),
            file_name: media_state.1,
            content_type: Some("image/gif".to_owned()),
            variant: None,
        };
        let original_path = metadata.relative_path("original").expect("original path");
        let small_path = metadata.relative_path("small").expect("small path");
        let mut original = media_root.open_file(Path::new(&original_path))?;
        let mut original_bytes = Vec::new();
        std::io::Read::read_to_end(&mut original, &mut original_bytes)?;
        assert_eq!(original_bytes, body);
        assert!(media_root.open_file(Path::new(&small_path)).is_ok());
        assert!(queue.dead_letters(10).await?.is_empty());
        assert_eq!(queue.queued_count().await?, 0);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    if let Some(first) = first.take()
        && !first.is_finished()
    {
        first.abort();
        let _ = first.await;
    }
    release_request.notify_one();
    let server_result: Result<(), Box<dyn std::error::Error>> = if operation.is_ok() {
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut server).await {
            Ok(Ok(result)) => result.map_err(|error| Box::new(error) as _),
            Ok(Err(error)) => Err(Box::new(error)),
            Err(error) => {
                server.abort();
                let _ = server.await;
                Err(Box::new(error))
            }
        }
    } else {
        server.abort();
        let _ = server.await;
        Ok(())
    };
    let cleanup_result = async {
        sqlx::query("DELETE FROM media_attachments WHERE id = $1")
            .bind(media_id)
            .execute(&writer_pool)
            .await?;
        drop(executor);
        drop(media_root);
        fs::remove_dir_all(root_path)?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    operation?;
    server_result?;
    cleanup_result?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_media_fetch_caches_original_and_gif_thumbnail()
-> Result<(), Box<dyn std::error::Error>> {
    const BOB: i64 = 116_844_606_259_202_001;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const LOGICAL_KEY: &str = "activitypub:test-media-fetch-success";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let root_path = std::env::temp_dir().join(format!(
        "rustodon-worker-media-success-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root_path);
    fs::create_dir(&root_path)?;
    let media_root = PaperclipRoot::open(&root_path)?
        .with_write_fault(PaperclipWriteFault::storage_full_after(1));
    let status_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM statuses WHERE account_id = $1 AND deleted_at IS NULL ORDER BY id LIMIT 1",
    )
    .bind(BOB)
    .fetch_one(&writer_pool)
    .await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let body = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.gif")?;
    let prepared = prepare_media_attachment(BOB, "remote.gif", "image/gif", &body)?;
    let remote_url = format!(
        "http://media.fixture.invalid:{}/remote.gif",
        endpoint.port()
    );
    let media_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO media_attachments (
             account_id, status_id, type, processing, remote_url, file_content_type, file_meta,
             created_at, updated_at)
         VALUES ($1, $2, 0, 0, $3, 'image/gif', '{}'::json,
                 clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .bind(BOB)
    .bind(status_id)
    .bind(&remote_url)
    .fetch_one(&writer_pool)
    .await?;
    let media_metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: media_id,
        remote: true,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let server = tokio::spawn(fixture_media_server_for_retries(listener, body.clone(), 2));
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: Some(media_root.clone()),
        limited_federation: false,
        remote_media_endpoint: Some(endpoint),
        remote_delivery_endpoint: None,
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Pull,
                ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
                json!({"media_id": media_id}),
            )
            .logical_key(LOGICAL_KEY),
        )
        .await?;
    assert!(
        executor
            .process_one("media-worker", &[Lane::Pull], Duration::seconds(30))
            .await?
    );
    let failed_media_state = sqlx::query_as::<_, (Option<i32>, Option<String>)>(
        "SELECT processing, file_file_name FROM media_attachments WHERE id = $1",
    )
    .bind(media_id)
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(failed_media_state, (Some(0), None));
    let original_path = media_metadata
        .relative_path("original")
        .expect("original path");
    let small_path = media_metadata.relative_path("small").expect("small path");
    assert!(media_root.open_file(Path::new(&original_path)).is_err());
    assert!(media_root.open_file(Path::new(&small_path)).is_err());
    sqlx::query(
        "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE logical_key = $1",
    )
    .bind(LOGICAL_KEY)
    .execute(&runtime_pool)
    .await?;
    assert!(
        executor
            .process_one("media-worker-retry", &[Lane::Pull], Duration::seconds(30))
            .await?
    );
    server.await??;

    let media_state = sqlx::query_as::<
        _,
        (
            Option<i32>,
            Option<String>,
            Option<i32>,
            Value,
            Option<String>,
        ),
    >(
        "SELECT processing, file_file_name, file_file_size, file_meta, blurhash
           FROM media_attachments WHERE id = $1",
    )
    .bind(media_id)
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(media_state.0, Some(2));
    let file_name = media_state.1.clone().expect("processed media has a name");
    assert!(
        Path::new(&file_name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
    );
    assert!(media_state.2.is_some_and(|size| size > 0));
    assert!(media_state.3["original"]["size"].is_string());
    assert!(media_state.3["small"]["size"].is_string());
    assert!(media_state.4.is_some());

    let metadata = PaperclipMetadata {
        id: media_id,
        file_name,
        ..media_metadata
    };
    let original_path = metadata.relative_path("original").expect("original path");
    let small_path = metadata.relative_path("small").expect("small path");
    assert!(original_path.starts_with("cache/"));
    assert!(small_path.starts_with("cache/"));
    assert!(
        Path::new(&small_path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    );
    let mut original = media_root.open_file(Path::new(&original_path))?;
    let mut original_bytes = Vec::new();
    std::io::Read::read_to_end(&mut original, &mut original_bytes)?;
    assert_eq!(original_bytes, body);
    let mut small = media_root.open_file(Path::new(&small_path))?;
    let mut small_bytes = Vec::new();
    std::io::Read::read_to_end(&mut small, &mut small_bytes)?;
    assert!(!small_bytes.is_empty());

    sqlx::query("DELETE FROM media_attachments WHERE id = $1")
        .bind(media_id)
        .execute(&writer_pool)
        .await?;
    drop(executor);
    let _ = fs::remove_dir_all(root_path);
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_media_fetch_reconciles_after_ambiguous_metadata_commit()
-> Result<(), Box<dyn std::error::Error>> {
    const BOB: i64 = 116_844_606_259_202_001;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const BEFORE_LOGICAL_KEY: &str = "activitypub:test-media-ambiguous-before";
    const AFTER_LOGICAL_KEY: &str = "activitypub:test-media-ambiguous-after";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let root_path = std::env::temp_dir().join(format!(
        "rustodon-worker-media-ambiguous-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root_path);
    fs::create_dir(&root_path)?;
    let media_root = PaperclipRoot::open(&root_path)?
        .with_commit_fault(PaperclipCommitFault::before_and_after());
    let status_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM statuses WHERE account_id = $1 AND deleted_at IS NULL ORDER BY id LIMIT 1",
    )
    .bind(BOB)
    .fetch_one(&writer_pool)
    .await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let body = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.gif")?;
    let prepared = prepare_media_attachment(BOB, "remote.gif", "image/gif", &body)?;
    let before_url = format!(
        "http://media.fixture.invalid:{}/ambiguous-before.gif",
        endpoint.port()
    );
    let after_url = format!(
        "http://media.fixture.invalid:{}/ambiguous-after.gif",
        endpoint.port()
    );
    let mut media_ids = Vec::new();
    for remote_url in [&before_url, &after_url] {
        media_ids.push(
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO media_attachments (
                     account_id, status_id, type, processing, remote_url, file_content_type, file_meta,
                     created_at, updated_at)
                 VALUES ($1, $2, 0, 0, $3, 'image/gif', '{}'::json,
                         clock_timestamp(), clock_timestamp())
                 RETURNING id",
            )
            .bind(BOB)
            .bind(status_id)
            .bind(remote_url)
            .fetch_one(&writer_pool)
            .await?,
        );
    }
    let mut server = tokio::spawn(fixture_media_server_for_retries(listener, body.clone(), 3));
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: Some(media_root.clone()),
        limited_federation: false,
        remote_media_endpoint: Some(endpoint),
        remote_delivery_endpoint: None,
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let operation = async {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Pull,
                    ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
                    json!({"media_id": media_ids[0]}),
                )
                .logical_key(BEFORE_LOGICAL_KEY),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "ambiguous-before-worker",
                    &[Lane::Pull],
                    Duration::seconds(30)
                )
                .await?
        );
        let before_state = sqlx::query_as::<_, (Option<i32>, Option<String>)>(
            "SELECT processing, file_file_name FROM media_attachments WHERE id = $1",
        )
        .bind(media_ids[0])
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(before_state, (Some(0), None));
        let before_metadata = PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: media_ids[0],
            remote: true,
            storage_schema_version: Some(1),
            file_name: prepared.file_name.clone(),
            content_type: Some(prepared.content_type.clone()),
            variant: None,
        };
        for style in ["original", "small"] {
            let path = before_metadata
                .relative_path(style)
                .expect("ambiguous pre-commit path");
            assert!(
                media_root.open_file(Path::new(&path)).is_ok(),
                "files must survive a commit failure before PostgreSQL reports success"
            );
        }
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Pull,
                    ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
                    json!({"media_id": media_ids[1]}),
                )
                .logical_key(AFTER_LOGICAL_KEY),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "ambiguous-after-worker",
                    &[Lane::Pull],
                    Duration::seconds(30)
                )
                .await?
        );
        let after_state = sqlx::query_as::<_, (Option<i32>, Option<String>)>(
            "SELECT processing, file_file_name FROM media_attachments WHERE id = $1",
        )
        .bind(media_ids[1])
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(after_state.0, Some(2));
        assert!(after_state.1.is_some());
        let after_metadata = PaperclipMetadata {
            id: media_ids[1],
            file_name: after_state.1.clone().expect("committed media name"),
            ..before_metadata.clone()
        };
        for style in ["original", "small"] {
            let path = after_metadata
                .relative_path(style)
                .expect("ambiguous post-commit path");
            assert!(
                media_root.open_file(Path::new(&path)).is_ok(),
                "files must survive an error after PostgreSQL committed metadata"
            );
        }
        sqlx::query(
            "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE logical_key = $1",
        )
        .bind(AFTER_LOGICAL_KEY)
        .execute(&runtime_pool)
        .await?;
        assert!(
            executor
                .process_one(
                    "ambiguous-after-retry",
                    &[Lane::Pull],
                    Duration::seconds(30)
                )
                .await?
        );
        sqlx::query(
            "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE logical_key = $1",
        )
        .bind(BEFORE_LOGICAL_KEY)
        .execute(&runtime_pool)
        .await?;
        assert!(
            executor
                .process_one(
                    "ambiguous-before-retry",
                    &[Lane::Pull],
                    Duration::seconds(30)
                )
                .await?
        );
        let before_reconciled = sqlx::query_as::<_, (Option<i32>, Option<String>)>(
            "SELECT processing, file_file_name FROM media_attachments WHERE id = $1",
        )
        .bind(media_ids[0])
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(before_reconciled.0, Some(2));
        assert!(before_reconciled.1.is_some());
        assert_eq!(queue.queued_count().await?, 0);
        assert!(queue.dead_letters(10).await?.is_empty());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM rustodon.remote_fetch_leases",)
                .fetch_one(&runtime_pool)
                .await?,
            0
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    let server_result: Result<(), Box<dyn std::error::Error>> = if operation.is_ok() {
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut server).await {
            Ok(Ok(result)) => result.map_err(|error| Box::new(error) as _),
            Ok(Err(error)) => Err(Box::new(error)),
            Err(error) => {
                server.abort();
                let _ = server.await;
                Err(Box::new(error))
            }
        }
    } else {
        server.abort();
        let _ = server.await;
        Ok(())
    };
    let cleanup_result = async {
        sqlx::query("DELETE FROM media_attachments WHERE id = ANY($1)")
            .bind(&media_ids)
            .execute(&writer_pool)
            .await?;
        sqlx::query("DELETE FROM rustodon.durable_jobs WHERE logical_key = ANY($1)")
            .bind(vec![
                BEFORE_LOGICAL_KEY.to_owned(),
                AFTER_LOGICAL_KEY.to_owned(),
            ])
            .execute(&runtime_pool)
            .await?;
        drop(executor);
        drop(media_root);
        fs::remove_dir_all(root_path)?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    operation?;
    server_result?;
    cleanup_result?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_status_update_and_delete_distribution_are_durable()
-> Result<(), Box<dyn std::error::Error>> {
    const AUTHOR: i64 = 116_844_606_259_201_002;
    const PARENT_AUTHOR: i64 = 116_844_606_259_201_001;
    const PARENT_STATUS: i64 = 116_844_842_188_805_001;
    const PARENT_REMOTE_FOLLOWER: i64 = -330;
    const REMOTE_FOLLOWER: i64 = -331;
    const REMOTE_QUOTER: i64 = -330;
    const PENDING_QUOTER: i64 = -332;
    const QUOTING_STATUS: i64 = -400;
    const QUOTE_ID: i64 = -99001;
    const PENDING_QUOTE_ID: i64 = -99002;
    const SILENT_MENTION_ID: i64 = -99003;
    const POLL_ID: i64 = -99004;
    const QUOTE_INBOX: &str = "https://quote.remote.fixture.invalid/inbox";
    const PENDING_QUOTE_INBOX: &str = "https://pending-quote.remote.fixture.invalid/inbox";
    const SILENT_MENTION_INBOX: &str = "https://silent-mention.remote.fixture.invalid/inbox";
    const RELAY_ID: i64 = 99001;
    const RELAY_INBOX: &str = "https://relay.fixture.invalid/inbox";
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const STATUS_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/users/moderator/statuses/worker-outbound";
    const REPLY_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/users/moderator/statuses/worker-reply";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let previous_relay: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(relay) FROM relays relay WHERE id = $1")
            .bind(RELAY_ID)
            .fetch_optional(&writer_pool)
            .await?;
    sqlx::query("DELETE FROM relays WHERE id = $1")
        .bind(RELAY_ID)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "INSERT INTO relays
             (id, created_at, follow_activity_id, inbox_url, state, updated_at)
         VALUES ($1, clock_timestamp(), NULL, $2, 2, clock_timestamp())",
    )
    .bind(RELAY_ID)
    .bind(RELAY_INBOX)
    .execute(&writer_pool)
    .await?;
    let previous_author_remote_follows: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(follow) FROM follows follow
          JOIN accounts follower ON follower.id = follow.account_id
         WHERE follow.target_account_id = $1 AND follower.domain IS NOT NULL",
    )
    .bind(AUTHOR)
    .fetch_all(&writer_pool)
    .await?;
    sqlx::query(
        "DELETE FROM follows follow USING accounts follower
          WHERE follower.id = follow.account_id
            AND follow.target_account_id = $1 AND follower.domain IS NOT NULL",
    )
    .bind(AUTHOR)
    .execute(&writer_pool)
    .await?;
    let previous_parent_follow: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(follow) FROM follows follow
          WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(PARENT_REMOTE_FOLLOWER)
    .bind(PARENT_AUTHOR)
    .fetch_optional(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(PARENT_REMOTE_FOLLOWER)
        .bind(PARENT_AUTHOR)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "INSERT INTO follows
             (account_id, created_at, languages, notify, show_reblogs,
              target_account_id, updated_at, uri)
         VALUES ($1, clock_timestamp(), NULL, false, true, $2, clock_timestamp(), $3)",
    )
    .bind(PARENT_REMOTE_FOLLOWER)
    .bind(PARENT_AUTHOR)
    .bind("https://remote.fixture.invalid/users/timeline_author#follows/worker-parent")
    .execute(&writer_pool)
    .await?;
    let status_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO statuses (
             account_id, text, spoiler_text, visibility, local, uri, url, language,
             sensitive, reply, created_at, updated_at)
         VALUES ($1, 'worker original', '', 0, true, $2, $2, 'en', false, false,
                 clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .bind(AUTHOR)
    .bind(STATUS_URI)
    .fetch_one(&writer_pool)
    .await?;
    let emoji_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO custom_emojis (
             shortcode, domain, image_content_type, image_file_name, image_file_size,
             image_storage_schema_version, disabled, visible_in_picker, created_at, updated_at)
         VALUES ('poll_blob', NULL, 'image/png', 'poll-blob.png', 10, 1, false, true,
                 clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO polls (
             id, account_id, status_id, options, cached_tallies, votes_count, voters_count,
             multiple, hide_totals, expires_at, created_at, updated_at)
         VALUES ($1, $2, $3, ARRAY['Vote :poll_blob:', 'No'], ARRAY[0, 0]::bigint[], 0, 0,
                 false, false, NULL, clock_timestamp(), clock_timestamp())",
    )
    .bind(POLL_ID)
    .bind(AUTHOR)
    .bind(status_id)
    .execute(&writer_pool)
    .await?;
    sqlx::query("UPDATE statuses SET poll_id = $2 WHERE id = $1")
        .bind(status_id)
        .bind(POLL_ID)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "INSERT INTO follows
             (account_id, created_at, languages, notify, show_reblogs,
              target_account_id, updated_at, uri)
         VALUES ($1, clock_timestamp(), NULL, false, true, $2, clock_timestamp(), NULL)",
    )
    .bind(REMOTE_FOLLOWER)
    .bind(AUTHOR)
    .execute(&writer_pool)
    .await?;
    let previous_quoter_state = sqlx::query_as::<_, (String, String, Option<NaiveDateTime>)>(
        "SELECT shared_inbox_url, inbox_url, suspended_at FROM accounts WHERE id = $1",
    )
    .bind(REMOTE_QUOTER)
    .fetch_one(&writer_pool)
    .await?;
    let previous_pending_quoter_inboxes = sqlx::query_as::<_, (String, String)>(
        "SELECT shared_inbox_url, inbox_url FROM accounts WHERE id = $1",
    )
    .bind(PENDING_QUOTER)
    .fetch_one(&writer_pool)
    .await?;
    let previous_follower_inboxes = sqlx::query_as::<_, (String, String)>(
        "SELECT shared_inbox_url, inbox_url FROM accounts WHERE id = $1",
    )
    .bind(REMOTE_FOLLOWER)
    .fetch_one(&writer_pool)
    .await?;
    for quote_id in [QUOTE_ID, PENDING_QUOTE_ID] {
        sqlx::query("DELETE FROM quotes WHERE id = $1")
            .bind(quote_id)
            .execute(&writer_pool)
            .await?;
    }
    sqlx::query("DELETE FROM mentions WHERE id = $1")
        .bind(SILENT_MENTION_ID)
        .execute(&writer_pool)
        .await?;
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
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
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let mut status_activity_uri = None;

    for (activity_type, logical_key) in [
        ("Create", "activitypub:test-status-create"),
        ("Update", "activitypub:test-status-update"),
    ] {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Push,
                    ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                    json!({"status_id": status_id, "activity_type": activity_type}),
                )
                .logical_key(logical_key),
            )
            .await?;
        assert!(
            executor
                .process_one("status-worker", &[Lane::Push], Duration::seconds(30))
                .await
                .map_err(|error| {
                    std::io::Error::other(format!(
                        "{activity_type} status distribution failed: {error}"
                    ))
                })?
        );
        if activity_type == "Update" {
            let create_body: Value = sqlx::query_scalar(
                "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
                  WHERE kind = 'rustodon.activitypub.deliver'
                    AND payload -> 'arguments' ->> 'status_id' = $1
                    AND payload -> 'arguments' -> 'body' ->> 'type' = 'Create'",
            )
            .bind(status_id.to_string())
            .fetch_optional(&writer_pool)
            .await?
            .ok_or_else(|| std::io::Error::other("status create delivery was not recorded"))?;
            assert_eq!(create_body["type"], "Create");
            assert!(create_body["object"]["tag"].as_array().is_some_and(|tags| {
                tags.iter()
                    .any(|tag| tag["type"] == "Emoji" && tag["name"] == ":poll_blob:")
            }));
            status_activity_uri = Some(
                create_body["object"]["id"]
                    .as_str()
                    .ok_or("Create Note has no status URI")?
                    .to_owned(),
            );
        } else {
            sqlx::query(
                "UPDATE statuses
                    SET text = 'worker updated', edited_at = clock_timestamp(),
                        updated_at = clock_timestamp()
                  WHERE id = $1",
            )
            .bind(status_id)
            .execute(&writer_pool)
            .await?;
            sqlx::query("UPDATE accounts SET shared_inbox_url = $1 WHERE id = $2")
                .bind(SILENT_MENTION_INBOX)
                .bind(REMOTE_FOLLOWER)
                .execute(&writer_pool)
                .await?;
            sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
                .bind(REMOTE_FOLLOWER)
                .bind(AUTHOR)
                .execute(&writer_pool)
                .await?;
            sqlx::query(
                "INSERT INTO mentions (id, account_id, created_at, silent, status_id, updated_at)
                 VALUES ($1, $2, clock_timestamp(), true, $3, clock_timestamp())",
            )
            .bind(SILENT_MENTION_ID)
            .bind(REMOTE_FOLLOWER)
            .bind(status_id)
            .execute(&writer_pool)
            .await?;
            sqlx::query("UPDATE accounts SET shared_inbox_url = $1 WHERE id = $2")
                .bind(QUOTE_INBOX)
                .bind(REMOTE_QUOTER)
                .execute(&writer_pool)
                .await?;
            sqlx::query("UPDATE accounts SET shared_inbox_url = $1 WHERE id = $2")
                .bind(PENDING_QUOTE_INBOX)
                .bind(PENDING_QUOTER)
                .execute(&writer_pool)
                .await?;
            sqlx::query(
                "INSERT INTO quotes
                     (id, account_id, activity_uri, approval_uri, created_at, legacy,
                      quoted_account_id, quoted_status_id, state, status_id, updated_at)
                 VALUES ($1, $2, NULL, NULL, clock_timestamp(), false, $3, $4, 1, $5,
                         clock_timestamp())",
            )
            .bind(QUOTE_ID)
            .bind(REMOTE_QUOTER)
            .bind(AUTHOR)
            .bind(status_id)
            .bind(QUOTING_STATUS)
            .execute(&writer_pool)
            .await?;
            sqlx::query(
                "INSERT INTO quotes
                     (id, account_id, activity_uri, approval_uri, created_at, legacy,
                      quoted_account_id, quoted_status_id, state, status_id, updated_at)
                 VALUES ($1, $2, NULL, NULL, clock_timestamp(), false, $3, $4, 0, $5,
                         clock_timestamp())",
            )
            .bind(PENDING_QUOTE_ID)
            .bind(AUTHOR)
            .bind(PENDING_QUOTER)
            .bind(QUOTING_STATUS)
            .bind(status_id)
            .execute(&writer_pool)
            .await?;
        }
    }
    let status_activity_uri = status_activity_uri.ok_or("Create delivery was not recorded")?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' ->> 'status_id' = $1
                AND payload -> 'arguments' ->> 'inbox_url' = $2
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Create'",
        )
        .bind(status_id.to_string())
        .bind(RELAY_INBOX)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "public status delivery must include enabled relays"
    );
    let edited_at =
        sqlx::query_scalar::<_, NaiveDateTime>("SELECT edited_at FROM statuses WHERE id = $1")
            .bind(status_id)
            .fetch_optional(&writer_pool)
            .await?
            .ok_or_else(|| std::io::Error::other("status edit timestamp was not recorded"))?;
    let update_body: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
          WHERE kind = 'rustodon.activitypub.deliver'
            AND payload -> 'arguments' ->> 'status_id' = $1
            AND payload -> 'arguments' -> 'body' ->> 'type' = 'Update'",
    )
    .bind(status_id.to_string())
    .fetch_optional(&writer_pool)
    .await?
    .ok_or_else(|| std::io::Error::other("status update delivery was not recorded"))?;
    assert_eq!(update_body["type"], "Update");
    assert!(update_body["object"]["tag"].as_array().is_some_and(|tags| {
        tags.iter()
            .any(|tag| tag["type"] == "Emoji" && tag["name"] == ":poll_blob:")
    }));
    assert_eq!(
        update_body["id"],
        format!(
            "{status_activity_uri}#updates/{}",
            edited_at.and_utc().timestamp_micros()
        )
    );
    assert_eq!(update_body["object"]["content"], "<p>worker updated</p>");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' ->> 'status_id' = $1
                AND payload -> 'arguments' ->> 'inbox_url' = $2
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Update'",
        )
        .bind(status_id.to_string())
        .bind(QUOTE_INBOX)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "a status update must reach remote accounts that quoted it"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' ->> 'status_id' = $1
                AND payload -> 'arguments' ->> 'inbox_url' = $2
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Update'",
        )
        .bind(status_id.to_string())
        .bind(PENDING_QUOTE_INBOX)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "a pending quote must reach its remote target author"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' ->> 'status_id' = $1
                AND payload -> 'arguments' ->> 'inbox_url' = $2
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Update'",
        )
        .bind(status_id.to_string())
        .bind(SILENT_MENTION_INBOX)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "a silent historical mention must receive status updates"
    );
    sqlx::query("UPDATE accounts SET shared_inbox_url = $1 WHERE id = $2")
        .bind(&previous_pending_quoter_inboxes.0)
        .bind(PENDING_QUOTER)
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE accounts SET shared_inbox_url = $1, inbox_url = $2 WHERE id = $3")
        .bind(&previous_follower_inboxes.0)
        .bind(&previous_follower_inboxes.1)
        .bind(REMOTE_FOLLOWER)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM mentions WHERE id = $1")
        .bind(SILENT_MENTION_ID)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "INSERT INTO follows
             (account_id, created_at, languages, notify, show_reblogs,
              target_account_id, updated_at, uri)
         VALUES ($1, clock_timestamp(), NULL, false, true, $2, clock_timestamp(), NULL)",
    )
    .bind(REMOTE_FOLLOWER)
    .bind(AUTHOR)
    .execute(&writer_pool)
    .await?;

    let remote_boost_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO statuses (
             account_id, text, spoiler_text, visibility, local, uri, url, language,
             sensitive, reply, reblog_of_id, created_at, updated_at)
         VALUES ($1, 'worker remote boost', '', 0, true, $2, $2, 'en', false, false, $3,
                 clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .bind(AUTHOR)
    .bind("https://fixture-v4-6-5.rustodon.invalid/users/moderator/statuses/worker-remote-boost")
    .bind(QUOTING_STATUS)
    .fetch_one(&writer_pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                json!({"status_id": remote_boost_id, "activity_type": "Create"}),
            )
            .logical_key("activitypub:test-remote-target-boost-create"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "remote-target-boost-worker",
                &[Lane::Push],
                Duration::seconds(30)
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' ->> 'status_id' = $1
                AND payload -> 'arguments' ->> 'inbox_url' = $2
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Announce'",
        )
        .bind(remote_boost_id.to_string())
        .bind(QUOTE_INBOX)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "a local Announce must reach the original remote author"
    );

    sqlx::query("UPDATE statuses SET visibility = 2 WHERE id = $1")
        .bind(status_id)
        .execute(&writer_pool)
        .await?;

    let boost_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO statuses (
             account_id, text, spoiler_text, visibility, local, uri, url, language,
             sensitive, reply, reblog_of_id, created_at, updated_at)
         VALUES ($1, '', '', 2, true, NULL, NULL, NULL, false, false, $2,
                 clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .bind(AUTHOR)
    .bind(status_id)
    .fetch_one(&writer_pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                json!({"status_id": boost_id, "activity_type": "Create"}),
            )
            .logical_key("activitypub:test-boost-create"),
        )
        .await?;
    assert!(
        executor
            .process_one("status-worker", &[Lane::Push], Duration::seconds(30))
            .await?
    );
    let announce_body: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
          WHERE kind = 'rustodon.activitypub.deliver'
            AND payload -> 'arguments' ->> 'status_id' = $1
            AND payload -> 'arguments' -> 'body' ->> 'type' = 'Announce'",
    )
    .bind(boost_id.to_string())
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(announce_body["type"], "Announce");
    assert_eq!(announce_body["object"]["type"], "Note");
    assert_eq!(announce_body["object"]["id"], status_activity_uri);
    assert!(announce_body["cc"].as_array().is_some_and(|values| {
        values
            .iter()
            .any(|value| value == &format!("{ORIGIN}ap/users/{AUTHOR}"))
    }));

    sqlx::query(
        "UPDATE statuses
            SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
          WHERE id = $1",
    )
    .bind(boost_id)
    .execute(&writer_pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                json!({"status_id": boost_id, "activity_type": "Delete"}),
            )
            .logical_key("activitypub:test-boost-delete"),
        )
        .await?;
    assert!(
        executor
            .process_one("status-worker", &[Lane::Push], Duration::seconds(30))
            .await?
    );
    let undo_announce_body: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
          WHERE kind = 'rustodon.activitypub.deliver'
            AND payload -> 'arguments' ->> 'status_id' = $1
            AND payload -> 'arguments' -> 'body' ->> 'type' = 'Undo'",
    )
    .bind(boost_id.to_string())
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(undo_announce_body["type"], "Undo");
    assert_eq!(undo_announce_body["object"]["type"], "Announce");
    assert_eq!(undo_announce_body["object"]["object"], status_activity_uri);

    sqlx::query("UPDATE statuses SET visibility = 0 WHERE id = $1")
        .bind(status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE accounts SET suspended_at = clock_timestamp() WHERE id = $1")
        .bind(REMOTE_QUOTER)
        .execute(&writer_pool)
        .await?;

    sqlx::query(
        "UPDATE statuses
            SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
          WHERE id = $1",
    )
    .bind(status_id)
    .execute(&writer_pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                json!({
                    "status_id": status_id,
                    "activity_type": "Delete",
                    "recipient_account_ids": [-332]
                }),
            )
            .logical_key("activitypub:test-status-delete"),
        )
        .await?;
    assert!(
        executor
            .process_one("status-worker", &[Lane::Push], Duration::seconds(30))
            .await?
    );
    let delete_body: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
          WHERE kind = 'rustodon.activitypub.deliver'
            AND payload -> 'arguments' ->> 'status_id' = $1
            AND payload -> 'arguments' -> 'body' ->> 'type' = 'Delete'",
    )
    .bind(status_id.to_string())
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(delete_body["type"], "Delete");
    assert_eq!(delete_body["object"]["type"], "Tombstone");
    assert_eq!(delete_body["object"]["id"], status_activity_uri);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' ->> 'status_id' = $1
                AND payload -> 'arguments' ->> 'inbox_url' = $2
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Delete'",
        )
        .bind(status_id.to_string())
        .bind(RELAY_INBOX)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "public status deletion must reach enabled relays"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' ->> 'status_id' = $1
                AND payload -> 'arguments' ->> 'inbox_url' = $2
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Delete'",
        )
        .bind(status_id.to_string())
        .bind(QUOTE_INBOX)
        .fetch_one(&writer_pool)
        .await?,
        1,
        "status deletion must reach suspended remote quoters"
    );
    sqlx::query("UPDATE accounts SET shared_inbox_url = $1, inbox_url = $2, suspended_at = $3 WHERE id = $4")
        .bind(&previous_quoter_state.0)
        .bind(&previous_quoter_state.1)
        .bind(previous_quoter_state.2)
        .bind(REMOTE_QUOTER)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(REMOTE_FOLLOWER)
        .bind(AUTHOR)
        .execute(&writer_pool)
        .await?;

    let reply_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO statuses (
             account_id, text, spoiler_text, visibility, local, uri, url, language,
             sensitive, reply, in_reply_to_id, in_reply_to_account_id,
             created_at, updated_at)
         VALUES ($1, 'worker reply', '', 0, true, $2, $2, 'en', false, true, $3, $4,
                 clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .bind(AUTHOR)
    .bind(REPLY_URI)
    .bind(PARENT_STATUS)
    .bind(PARENT_AUTHOR)
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows
              WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(PARENT_REMOTE_FOLLOWER)
        .bind(PARENT_AUTHOR)
        .fetch_one(&writer_pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows
              WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(PARENT_REMOTE_FOLLOWER)
        .bind(AUTHOR)
        .fetch_one(&writer_pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows follow
               JOIN accounts follower ON follower.id = follow.account_id
              WHERE follow.target_account_id = $1 AND follower.domain IS NOT NULL",
        )
        .bind(AUTHOR)
        .fetch_one(&writer_pool)
        .await?,
        0
    );
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                json!({"status_id": reply_id, "activity_type": "Create"}),
            )
            .logical_key("activitypub:test-reply-to-local-parent-create"),
        )
        .await?;
    assert!(
        executor
            .process_one("reply-worker", &[Lane::Push], Duration::seconds(30))
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' ->> 'status_id' = $1
                AND payload -> 'arguments' ->> 'inbox_url' = $2
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Create'",
        )
        .bind(reply_id.to_string())
        .bind("https://remote.fixture.invalid/inbox")
        .fetch_one(&writer_pool)
        .await?,
        1,
        "a reply to a local parent must reach the parent's remote followers"
    );

    for cleanup_status_id in [status_id, boost_id, remote_boost_id] {
        sqlx::query(
            "DELETE FROM rustodon.outbox_events WHERE payload -> 'arguments' ->> 'status_id' = $1",
        )
        .bind(cleanup_status_id.to_string())
        .execute(&writer_pool)
        .await?;
        sqlx::query("DELETE FROM polls WHERE status_id = $1")
            .bind(cleanup_status_id)
            .execute(&writer_pool)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id = $1")
            .bind(cleanup_status_id)
            .execute(&writer_pool)
            .await?;
    }
    sqlx::query("DELETE FROM custom_emojis WHERE id = $1")
        .bind(emoji_id)
        .execute(&writer_pool)
        .await?;
    for quote_id in [QUOTE_ID, PENDING_QUOTE_ID] {
        sqlx::query("DELETE FROM quotes WHERE id = $1")
            .bind(quote_id)
            .execute(&writer_pool)
            .await?;
    }
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(REMOTE_FOLLOWER)
        .bind(AUTHOR)
        .execute(&writer_pool)
        .await?;
    for previous_follow in previous_author_remote_follows {
        sqlx::query("INSERT INTO follows SELECT * FROM jsonb_populate_record(NULL::follows, $1)")
            .bind(previous_follow)
            .execute(&writer_pool)
            .await?;
    }
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(PARENT_REMOTE_FOLLOWER)
        .bind(PARENT_AUTHOR)
        .execute(&writer_pool)
        .await?;
    if let Some(previous_parent_follow) = previous_parent_follow {
        sqlx::query("INSERT INTO follows SELECT * FROM jsonb_populate_record(NULL::follows, $1)")
            .bind(previous_parent_follow)
            .execute(&writer_pool)
            .await?;
    }
    sqlx::query(
        "DELETE FROM rustodon.outbox_events WHERE payload -> 'arguments' ->> 'status_id' = $1",
    )
    .bind(reply_id.to_string())
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(reply_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM relays WHERE id = $1")
        .bind(RELAY_ID)
        .execute(&writer_pool)
        .await?;
    if let Some(previous_relay) = previous_relay {
        sqlx::query(
            "INSERT INTO relays
                 SELECT * FROM jsonb_populate_record(NULL::relays, $1)",
        )
        .bind(previous_relay)
        .execute(&writer_pool)
        .await?;
    }
    drop(executor);
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_account_updates_are_fanned_out_and_stale_jobs_are_fenced()
-> Result<(), Box<dyn std::error::Error>> {
    const ACCOUNT: i64 = 116_844_606_259_201_002;
    const REMOTE_FOLLOWER: i64 = -331;
    const RECENT_FOLLOW_TARGET: i64 = -330;
    const RECENT_FOLLOW_INBOX: &str =
        "https://recent-follow-account-update.remote.fixture.invalid/inbox";
    const SUSPENDED_FOLLOWER_INBOX: &str =
        "https://suspended-account-update.remote.fixture.invalid/inbox";
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let previous_account: (String, NaiveDateTime, Option<NaiveDateTime>, Option<i32>) =
        sqlx::query_as(
            "SELECT display_name, updated_at, suspended_at, suspension_origin
               FROM accounts WHERE id = $1",
        )
        .bind(ACCOUNT)
        .fetch_one(&writer_pool)
        .await?;
    let previous_remote_follower_state =
        sqlx::query_as::<_, (String, String, Option<NaiveDateTime>)>(
            "SELECT shared_inbox_url, inbox_url, suspended_at FROM accounts WHERE id = $1",
        )
        .bind(REMOTE_FOLLOWER)
        .fetch_one(&writer_pool)
        .await?;
    let previous_recent_target_state = sqlx::query_as::<_, (String, String, i32)>(
        "SELECT shared_inbox_url, inbox_url, protocol FROM accounts WHERE id = $1",
    )
    .bind(RECENT_FOLLOW_TARGET)
    .fetch_one(&writer_pool)
    .await?;
    let previous_recent_follow: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(follow) FROM follows follow
          WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(ACCOUNT)
    .bind(RECENT_FOLLOW_TARGET)
    .fetch_optional(&writer_pool)
    .await?;
    let previous_follow = sqlx::query_as::<
        _,
        (
            i64,
            NaiveDateTime,
            Option<Vec<String>>,
            bool,
            bool,
            NaiveDateTime,
            Option<String>,
        ),
    >(
        "SELECT id, created_at, languages, notify, show_reblogs, updated_at, uri
           FROM follows WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(REMOTE_FOLLOWER)
    .bind(ACCOUNT)
    .fetch_optional(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM custom_emojis WHERE id = 12992")
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "INSERT INTO custom_emojis
             (id, shortcode, domain, image_content_type, image_file_name, image_file_size,
              image_storage_schema_version, disabled, visible_in_picker, created_at, updated_at)
         VALUES (12992, 'actorupdateblob', NULL, 'image/png', 'actor-update.png', 68,
                 1, false, false, clock_timestamp(), clock_timestamp())",
    )
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(REMOTE_FOLLOWER)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "INSERT INTO follows
             (account_id, created_at, languages, notify, show_reblogs,
              target_account_id, updated_at, uri)
         VALUES ($1, clock_timestamp(), NULL, false, true, $2, clock_timestamp(), NULL)",
    )
    .bind(REMOTE_FOLLOWER)
    .bind(ACCOUNT)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE accounts SET shared_inbox_url = $1, suspended_at = clock_timestamp() WHERE id = $2",
    )
    .bind(SUSPENDED_FOLLOWER_INBOX)
    .bind(REMOTE_FOLLOWER)
    .execute(&writer_pool)
    .await?;
    let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
        "UPDATE accounts SET display_name = 'Worker actor update :actorupdateblob:', updated_at = clock_timestamp()
          WHERE id = $1 RETURNING updated_at",
    )
    .bind(ACCOUNT)
    .fetch_one(&writer_pool)
    .await?;
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
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
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
                json!({
                    "account_id": ACCOUNT,
                    "updated_at_micros": updated_at.and_utc().timestamp_micros()
                }),
            )
            .logical_key("activitypub:test-account-update"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "account-update-worker",
                &[Lane::Push],
                Duration::seconds(30)
            )
            .await?
    );
    let update_body: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
         WHERE kind = $1
            AND payload -> 'arguments' ->> 'source_account_id' = $2
            AND payload -> 'arguments' ->> 'inbox_url' = $3
            AND payload -> 'arguments' -> 'body' ->> 'type' = 'Update'",
    )
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(ACCOUNT.to_string())
    .bind(SUSPENDED_FOLLOWER_INBOX)
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(update_body["type"], "Update");
    assert_eq!(
        update_body["to"][0],
        "https://www.w3.org/ns/activitystreams#Public"
    );
    assert_eq!(
        update_body["object"]["name"],
        "Worker actor update :actorupdateblob:"
    );
    let profile_emoji = update_body["object"]["tag"]
        .as_array()
        .and_then(|tags| tags.iter().find(|tag| tag["type"] == "Emoji"))
        .ok_or("outbound actor Update omitted its profile emoji")?;
    assert_eq!(profile_emoji["name"], ":actorupdateblob:");
    assert_eq!(
        profile_emoji["id"],
        "https://fixture-v4-6-5.rustodon.invalid/emojis/12992"
    );
    assert_eq!(
        update_body["id"],
        format!(
            "https://fixture-v4-6-5.rustodon.invalid/ap/users/{ACCOUNT}#updates/{}",
            updated_at.and_utc().timestamp_micros()
        )
    );

    sqlx::query(
        "UPDATE accounts SET shared_inbox_url = $1, inbox_url = $2, suspended_at = $3
          WHERE id = $4",
    )
    .bind(&previous_remote_follower_state.0)
    .bind(&previous_remote_follower_state.1)
    .bind(previous_remote_follower_state.2)
    .bind(REMOTE_FOLLOWER)
    .execute(&writer_pool)
    .await?;

    sqlx::query(
        "UPDATE accounts SET display_name = 'Worker actor newer', updated_at = clock_timestamp()
          WHERE id = $1",
    )
    .bind(ACCOUNT)
    .execute(&writer_pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
                json!({
                    "account_id": ACCOUNT,
                    "updated_at_micros": updated_at.and_utc().timestamp_micros()
                }),
            )
            .logical_key("activitypub:test-account-update-stale"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "account-update-worker",
                &[Lane::Push],
                Duration::seconds(30)
            )
            .await?
    );
    let update_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events
          WHERE kind = $1
            AND payload -> 'arguments' ->> 'source_account_id' = $2
            AND payload -> 'arguments' -> 'body' ->> 'type' = 'Update'",
    )
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(ACCOUNT.to_string())
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(update_count, 1);

    sqlx::query("UPDATE accounts SET shared_inbox_url = $1 WHERE id = $2")
        .bind(SUSPENDED_FOLLOWER_INBOX)
        .bind(REMOTE_FOLLOWER)
        .execute(&writer_pool)
        .await?;
    let suspended_updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
        "UPDATE accounts SET suspended_at = clock_timestamp(), suspension_origin = 0,
          updated_at = clock_timestamp() WHERE id = $1 RETURNING updated_at",
    )
    .bind(ACCOUNT)
    .fetch_one(&writer_pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
                json!({
                    "account_id": ACCOUNT,
                    "updated_at_micros": suspended_updated_at.and_utc().timestamp_micros()
                }),
            )
            .logical_key("activitypub:test-account-suspension-update"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "account-suspension-update-worker",
                &[Lane::Push],
                Duration::seconds(30)
            )
            .await?
    );
    let suspension_update_body: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
          WHERE kind = $1
            AND payload -> 'arguments' ->> 'source_account_id' = $2
            AND payload -> 'arguments' ->> 'inbox_url' = $3
            AND payload -> 'arguments' ->> 'updated_at_micros' = $4
            AND payload -> 'arguments' -> 'body' ->> 'type' = 'Update'",
    )
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(ACCOUNT.to_string())
    .bind(SUSPENDED_FOLLOWER_INBOX)
    .bind(
        suspended_updated_at
            .and_utc()
            .timestamp_micros()
            .to_string(),
    )
    .fetch_one(&writer_pool)
    .await?;
    assert_eq!(suspension_update_body["type"], "Update");
    assert_eq!(suspension_update_body["object"]["suspended"], true);

    sqlx::query(
        "UPDATE accounts SET shared_inbox_url = $1, inbox_url = $1, protocol = 1
          WHERE id = $2",
    )
    .bind(RECENT_FOLLOW_INBOX)
    .bind(RECENT_FOLLOW_TARGET)
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(ACCOUNT)
        .bind(RECENT_FOLLOW_TARGET)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "INSERT INTO follows
             (account_id, created_at, languages, notify, show_reblogs,
              target_account_id, updated_at, uri)
         VALUES ($1, clock_timestamp() - interval '3 days', NULL, false, true,
                 $2, clock_timestamp() - interval '3 days',
                 'https://fixture-v4-6-5.rustodon.invalid/users/moderator#follows/recent-cutoff')",
    )
    .bind(ACCOUNT)
    .bind(RECENT_FOLLOW_TARGET)
    .execute(&writer_pool)
    .await?;
    let delayed_suspension_updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
        "UPDATE accounts SET suspended_at = clock_timestamp() - interval '3 days',
          suspension_origin = 0, updated_at = clock_timestamp() WHERE id = $1
          RETURNING updated_at",
    )
    .bind(ACCOUNT)
    .fetch_one(&writer_pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
                json!({
                    "account_id": ACCOUNT,
                    "updated_at_micros": delayed_suspension_updated_at
                        .and_utc()
                        .timestamp_micros()
                }),
            )
            .logical_key("activitypub:test-account-suspension-recent-follow"),
        )
        .await?;
    assert!(
        executor
            .process_one(
                "account-suspension-recent-follow-worker",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload -> 'arguments' ->> 'source_account_id' = $2
                AND payload -> 'arguments' ->> 'inbox_url' = $3
                AND payload -> 'arguments' ->> 'updated_at_micros' = $4
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Update'",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(ACCOUNT.to_string())
        .bind(RECENT_FOLLOW_INBOX)
        .bind(
            delayed_suspension_updated_at
                .and_utc()
                .timestamp_micros()
                .to_string(),
        )
        .fetch_one(&writer_pool)
        .await?,
        1,
        "a recent follow relative to local suspension must receive the delayed actor update",
    );
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE payload -> 'arguments' ->> 'source_account_id' = $1",
    )
    .bind(ACCOUNT.to_string())
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM custom_emojis WHERE id = 12992")
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "UPDATE accounts SET shared_inbox_url = $1, inbox_url = $2, protocol = $3
          WHERE id = $4",
    )
    .bind(&previous_recent_target_state.0)
    .bind(&previous_recent_target_state.1)
    .bind(previous_recent_target_state.2)
    .bind(RECENT_FOLLOW_TARGET)
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(ACCOUNT)
        .bind(RECENT_FOLLOW_TARGET)
        .execute(&writer_pool)
        .await?;
    if let Some(previous_follow) = previous_recent_follow {
        sqlx::query("INSERT INTO follows SELECT * FROM jsonb_populate_record(NULL::follows, $1)")
            .bind(previous_follow)
            .execute(&writer_pool)
            .await?;
    }

    sqlx::query(
        "UPDATE accounts SET shared_inbox_url = $1, inbox_url = $2, suspended_at = $3
          WHERE id = $4",
    )
    .bind(&previous_remote_follower_state.0)
    .bind(&previous_remote_follower_state.1)
    .bind(previous_remote_follower_state.2)
    .bind(REMOTE_FOLLOWER)
    .execute(&writer_pool)
    .await?;

    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE payload -> 'arguments' ->> 'source_account_id' = $1",
    )
    .bind(ACCOUNT.to_string())
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE accounts SET display_name = $1, updated_at = $2, suspended_at = $3,
          suspension_origin = $4 WHERE id = $5",
    )
    .bind(&previous_account.0)
    .bind(previous_account.1)
    .bind(previous_account.2)
    .bind(previous_account.3)
    .bind(ACCOUNT)
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(REMOTE_FOLLOWER)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    if let Some((id, created_at, languages, notify, show_reblogs, updated_at, uri)) =
        previous_follow
    {
        sqlx::query(
            "INSERT INTO follows
                 (id, account_id, created_at, languages, notify, show_reblogs,
                  target_account_id, updated_at, uri)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(REMOTE_FOLLOWER)
        .bind(created_at)
        .bind(languages)
        .bind(notify)
        .bind(show_reblogs)
        .bind(ACCOUNT)
        .bind(updated_at)
        .bind(uri)
        .execute(&writer_pool)
        .await?;
    }
    drop(executor);
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn self_service_account_deletion_queues_actor_delete_delivery()
-> Result<(), Box<dyn std::error::Error>> {
    const ACCOUNT: i64 = 116_844_606_259_201_002;
    const ACTOR_URI: &str = "https://fixture-v4-6-5.rustodon.invalid/ap/users/116844606259201002";
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let previous_account: (Option<NaiveDateTime>, Option<i32>, NaiveDateTime) = sqlx::query_as(
        "SELECT suspended_at, suspension_origin, updated_at FROM accounts WHERE id = $1",
    )
    .bind(ACCOUNT)
    .fetch_one(&writer_pool)
    .await?;
    let previous_deletion_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_deletion_requests WHERE account_id = $1")
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
            .await?;
    let operation = async {
        WriteRepository::from_pool(writer_pool.clone())
            .request_account_deletion(ACCOUNT, ACTOR_URI)
            .await?;
        let account_state: (Option<NaiveDateTime>, Option<i32>) =
            sqlx::query_as("SELECT suspended_at, suspension_origin FROM accounts WHERE id = $1")
                .bind(ACCOUNT)
                .fetch_one(&writer_pool)
                .await?;
        assert!(account_state.0.is_some());
        assert_eq!(account_state.1, Some(0));
        let deletion_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM account_deletion_requests WHERE account_id = $1",
        )
        .bind(ACCOUNT)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(deletion_count, previous_deletion_count + 1);
        let config = ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
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
        };
        let queue = Queue::new(runtime_pool.clone());
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(writer_pool.clone()),
            None,
            Some(config),
        )?;
        let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
        assert_eq!(queue.dispatch_outbox(100).await?, 2);
        let (deletion_created_at, purge_run_at): (NaiveDateTime, DateTime<Utc>) = sqlx::query_as(
            "SELECT request.created_at, job.run_at
               FROM account_deletion_requests request
               JOIN rustodon.outbox_events event
                 ON event.kind = $3
                AND event.payload -> 'arguments' ->> 'account_id' = $2
               JOIN rustodon.durable_jobs job
                 ON job.kind = $3
                AND job.logical_key = event.logical_key
              WHERE request.account_id = $1",
        )
        .bind(ACCOUNT)
        .bind(ACCOUNT.to_string())
        .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
        .fetch_one(&writer_pool)
        .await?;
        let expected_purge_at = deletion_created_at.and_utc() + Duration::days(30);
        assert!(purge_run_at >= expected_purge_at);
        assert!(purge_run_at <= expected_purge_at + Duration::seconds(1));
        assert!(
            executor
                .process_one(
                    "account-delete-worker",
                    &[Lane::Push],
                    Duration::seconds(30)
                )
                .await?
        );
        let delivery_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
               WHERE kind = $1
                 AND payload -> 'arguments' ->> 'source_account_id' = $2
                 AND payload -> 'arguments' -> 'body' ->> 'type' = 'Delete'",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(ACCOUNT.to_string())
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(delivery_body["id"], format!("{ACTOR_URI}#delete"));
        assert_eq!(delivery_body["actor"], ACTOR_URI);
        assert_eq!(delivery_body["object"], ACTOR_URI);
        assert_eq!(delivery_body["to"][0], activitypub::PUBLIC_ADDRESS);
        drop(executor);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    sqlx::query(
        "UPDATE accounts SET suspended_at = $1, suspension_origin = $2, updated_at = $3 WHERE id = $4",
    )
    .bind(previous_account.0)
    .bind(previous_account.1)
    .bind(previous_account.2)
    .bind(ACCOUNT)
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM account_deletion_requests WHERE account_id = $1")
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    reset().await?;
    operation?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines, clippy::type_complexity)]
async fn domain_block_job_purges_newly_suspended_remote_accounts()
-> Result<(), Box<dyn std::error::Error>> {
    const ACCOUNT: i64 = 900_000_000_000_000_031;
    const ACCOUNT_STATS: i64 = 900_000_000_000_000_032;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const MODERATOR_ROLE: i64 = 92;
    const LOCAL_ACCOUNT: i64 = 116_844_606_259_201_001;
    const ACTIVE_FOLLOW: i64 = 900_000_000_000_000_033;
    const PASSIVE_FOLLOW: i64 = 900_000_000_000_000_034;
    const MEDIA: i64 = 900_000_000_000_000_035;
    const DOMAIN: &str = "domain-job.fixture.invalid";
    const ACTOR_URI: &str = "https://domain-job.fixture.invalid/users/domain-job";
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let original_permissions: i64 =
        sqlx::query_scalar("SELECT permissions FROM user_roles WHERE id = $1")
            .bind(MODERATOR_ROLE)
            .fetch_one(&writer_pool)
            .await?;
    sqlx::query("UPDATE user_roles SET permissions = permissions | $1 WHERE id = $2")
        .bind(1_i64 << 5)
        .bind(MODERATOR_ROLE)
        .execute(&writer_pool)
        .await?;
    let original_local_stats: (i64, i64) = sqlx::query_as(
        "SELECT followers_count, following_count FROM account_stats WHERE account_id = $1",
    )
    .bind(LOCAL_ACCOUNT)
    .fetch_one(&writer_pool)
    .await?;
    let media_root_path = std::env::temp_dir().join(format!(
        "rustodon-domain-block-media-{}",
        std::process::id()
    ));
    fs::create_dir_all(&media_root_path)?;
    let media_root = PaperclipRoot::open(&media_root_path)?;
    let media_metadata = [
        PaperclipMetadata {
            attachment: PaperclipAttachment::AccountAvatar,
            id: ACCOUNT,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "domain-avatar.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::AccountHeader,
            id: ACCOUNT,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "domain-header.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: MEDIA,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "domain-media.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::MediaThumbnail,
            id: MEDIA,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "domain-media-thumb.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::CustomEmojiImage,
            id: MEDIA,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "domain-emoji.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
    ];
    for metadata in &media_metadata {
        for style in ["original", "small", "static"] {
            if let Some(path) = metadata.relative_path(style) {
                media_root.write_file(Path::new(&path), b"domain-block-media")?;
            }
        }
    }
    let operation = async {
        sqlx::query(
            "INSERT INTO accounts
                 (id, actor_type, domain, username, uri, created_at, updated_at)
             VALUES ($1, 'Person', $2, 'domain-job', $3, clock_timestamp(), clock_timestamp())",
        )
        .bind(ACCOUNT)
        .bind(DOMAIN)
        .bind(ACTOR_URI)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO account_stats (id, account_id, created_at, updated_at)
             VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
        )
        .bind(ACCOUNT_STATS)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE accounts SET
                 avatar_content_type = 'image/png', avatar_file_name = 'domain-avatar.png',
                 avatar_file_size = 12, avatar_storage_schema_version = 1,
                 avatar_updated_at = clock_timestamp(), header_content_type = 'image/png',
                 header_file_name = 'domain-header.png', header_file_size = 24,
                 header_storage_schema_version = 1, header_updated_at = clock_timestamp()
               WHERE id = $1",
        )
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO media_attachments
                 (id, account_id, type, processing, file_content_type, file_file_name,
                  file_file_size, file_storage_schema_version, file_updated_at,
                  thumbnail_content_type, thumbnail_file_name, thumbnail_file_size,
                  thumbnail_storage_schema_version, thumbnail_updated_at, remote_url,
                  created_at, updated_at)
             VALUES ($1, $2, 0, 2, 'image/png', 'domain-media.png', 36, 1,
                     clock_timestamp(), 'image/png', 'domain-media-thumb.png', 18, 1,
                     clock_timestamp(), 'https://media.example.invalid/domain-media.png',
                     clock_timestamp(), clock_timestamp())",
        )
        .bind(MEDIA)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO custom_emojis
                 (id, domain, shortcode, image_content_type, image_file_name, image_file_size,
                  image_storage_schema_version, image_remote_url, image_updated_at,
                  created_at, updated_at)
             VALUES ($1, $2, 'domain-job', 'image/png', 'domain-emoji.png', 8, 1,
                     'https://media.example.invalid/domain-emoji.png', clock_timestamp(),
                     clock_timestamp(), clock_timestamp())",
        )
        .bind(MEDIA)
        .bind(DOMAIN)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO follows
                 (id, account_id, target_account_id, uri, show_reblogs, notify, languages,
                  created_at, updated_at)
             VALUES ($1, $2, $3, $4, true, false, ARRAY['en'], clock_timestamp(), clock_timestamp()),
                    ($5, $3, $2, $6, false, true, ARRAY['de'], clock_timestamp(), clock_timestamp())",
        )
        .bind(ACTIVE_FOLLOW)
        .bind(LOCAL_ACCOUNT)
        .bind(ACCOUNT)
        .bind("https://fixture-v4-6-5.rustodon.invalid/activities/domain-active")
        .bind(PASSIVE_FOLLOW)
        .bind("https://fixture-v4-6-5.rustodon.invalid/activities/domain-passive")
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE account_stats SET followers_count = followers_count + 1,
                    following_count = following_count + 1, updated_at = clock_timestamp()
               WHERE account_id = $1",
        )
        .bind(LOCAL_ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE account_stats SET followers_count = 1, following_count = 1,
                    updated_at = clock_timestamp()
               WHERE account_id = $1",
        )
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;

        let writer = WriteRepository::from_pool(writer_pool.clone());
        let domain_block_id = writer
            .set_domain_block(MODERATOR, DOMAIN, 1, false, false, ORIGIN)
            .await?;
        let queue = Queue::new(runtime_pool.clone());
        assert_eq!(queue.dispatch_outbox(100).await?, 1);
        sqlx::query(
            "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp()
               WHERE kind = $1 AND arguments ->> 'domain_block_id' = $2",
        )
        .bind(MASTODON_DOMAIN_BLOCK_JOB_KIND)
        .bind(domain_block_id.to_string())
        .execute(&runtime_pool)
        .await?;
        let severance_event_id: i64 = sqlx::query_scalar(
            "SELECT (payload -> 'arguments' ->> 'severance_event_id')::bigint
               FROM rustodon.outbox_events
              WHERE kind = $1 AND payload -> 'arguments' ->> 'domain_block_id' = $2",
        )
        .bind(MASTODON_DOMAIN_BLOCK_JOB_KIND)
        .bind(domain_block_id.to_string())
        .fetch_one(&writer_pool)
        .await?;
        let config = ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            media_root_url: "/system".to_owned(),
            media_root: Some(media_root.clone()),
            limited_federation: false,
            #[cfg(feature = "test-support")]
            remote_media_endpoint: None,
            #[cfg(feature = "test-support")]
            remote_delivery_endpoint: None,
            #[cfg(feature = "test-support")]
            remote_fetch_endpoint: None,
        };
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(writer_pool.clone()),
            None,
            Some(config),
        )?;
        let executor = WorkerExecutor::new(queue, handlers, 1, 1)?;
        assert!(
            executor
                .process_one(
                    "domain-block-worker",
                    &[Lane::Maintenance],
                    Duration::seconds(30),
                )
                .await?
        );
        let account_state: (
            Option<NaiveDateTime>,
            Option<i32>,
            Option<NaiveDateTime>,
            String,
            String,
            String,
            String,
        ) = sqlx::query_as(
            "SELECT suspended_at, suspension_origin, silenced_at,
                    inbox_url, outbox_url, followers_url, following_url
               FROM accounts WHERE id = $1",
        )
        .bind(ACCOUNT)
        .fetch_one(&writer_pool)
        .await?;
        assert!(account_state.0.is_some());
        assert_eq!(account_state.1, None);
        assert_eq!(account_state.2, None);
        assert!(account_state.3.is_empty());
        assert!(account_state.4.is_empty());
        assert!(account_state.5.is_empty());
        assert!(account_state.6.is_empty());
        assert_eq!(
            sqlx::query_as::<_, (Option<String>, Option<String>, Option<i32>, Option<String>)>(
                "SELECT file_file_name, file_content_type, file_file_size, remote_url
                   FROM media_attachments WHERE id = $1",
            )
            .bind(MEDIA)
            .fetch_one(&writer_pool)
            .await?,
            (
                None,
                None,
                None,
                Some("https://media.example.invalid/domain-media.png".to_owned())
            )
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM custom_emojis WHERE id = $1")
                .bind(MEDIA)
                .fetch_one(&writer_pool)
                .await?,
            0
        );
        for metadata in &media_metadata {
            for style in ["original", "small", "static"] {
                if let Some(path) = metadata.relative_path(style) {
                    assert!(
                        media_root.open_file(Path::new(&path)).is_err(),
                        "domain cleanup left Paperclip file {path}"
                    );
                }
            }
        }
        let severed: Vec<(i64, i64, i32, Option<bool>, Option<bool>, Option<Vec<String>>)> =
            sqlx::query_as(
                "SELECT local_account_id, remote_account_id, direction, show_reblogs, notify,
                        languages
                   FROM severed_relationships
                  WHERE relationship_severance_event_id = $1
                  ORDER BY direction",
            )
            .bind(severance_event_id)
            .fetch_all(&writer_pool)
            .await?;
        assert_eq!(
            severed,
            vec![
                (
                    LOCAL_ACCOUNT,
                    ACCOUNT,
                    0,
                    Some(false),
                    Some(true),
                    Some(vec!["de".to_owned()])
                ),
                (
                    LOCAL_ACCOUNT,
                    ACCOUNT,
                    1,
                    Some(true),
                    Some(false),
                    Some(vec!["en".to_owned()])
                )
            ]
        );
        let account_event: (i64, i32, i32) = sqlx::query_as(
            "SELECT id, followers_count, following_count
               FROM account_relationship_severance_events
              WHERE account_id = $1 AND relationship_severance_event_id = $2",
        )
        .bind(LOCAL_ACCOUNT)
        .bind(severance_event_id)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(account_event.1, 1);
        assert_eq!(account_event.2, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications
                  WHERE account_id = $1 AND activity_id = $2
                    AND activity_type = 'AccountRelationshipSeveranceEvent'
                    AND type = 'severed_relationships'",
            )
            .bind(LOCAL_ACCOUNT)
            .bind(account_event.0)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs
                  WHERE kind = $1 AND arguments ->> 'domain_block_id' = $2",
            )
            .bind(MASTODON_DOMAIN_BLOCK_JOB_KIND)
            .bind(domain_block_id.to_string())
            .fetch_one(&runtime_pool)
            .await?,
            0
        );
        drop(executor);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    sqlx::query("DELETE FROM domain_blocks WHERE domain = $1")
        .bind(DOMAIN)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follows WHERE id IN ($1, $2)")
        .bind(ACTIVE_FOLLOW)
        .bind(PASSIVE_FOLLOW)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM media_attachments WHERE id = $1")
        .bind(MEDIA)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM custom_emojis WHERE id = $1")
        .bind(MEDIA)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "DELETE FROM notifications
          WHERE activity_type = 'AccountRelationshipSeveranceEvent'
            AND activity_id IN (
                SELECT id FROM account_relationship_severance_events
                 WHERE relationship_severance_event_id IN (
                     SELECT id FROM relationship_severance_events WHERE target_name = $1
                 )
            )",
    )
    .bind(DOMAIN)
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM relationship_severance_events WHERE target_name = $1")
        .bind(DOMAIN)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "UPDATE account_stats SET followers_count = $1, following_count = $2,
                updated_at = clock_timestamp()
           WHERE account_id = $3",
    )
    .bind(original_local_stats.0)
    .bind(original_local_stats.1)
    .bind(LOCAL_ACCOUNT)
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE user_roles SET permissions = $1 WHERE id = $2")
        .bind(original_permissions)
        .bind(MODERATOR_ROLE)
        .execute(&writer_pool)
        .await?;
    drop(media_root);
    fs::remove_dir_all(&media_root_path)?;
    reset().await?;
    operation?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines, clippy::type_complexity)]
async fn domain_purge_job_removes_remote_accounts_and_emoji()
-> Result<(), Box<dyn std::error::Error>> {
    const ACCOUNT: i64 = 900_000_000_000_000_041;
    const ACCOUNT_STATS: i64 = 900_000_000_000_000_042;
    const STATUS: i64 = 900_000_000_000_000_043;
    const MEDIA: i64 = 900_000_000_000_000_044;
    const EMOJI: i64 = 900_000_000_000_000_045;
    const REPORT: i64 = 900_000_000_000_000_046;
    const NOTIFICATION: i64 = 900_000_000_000_000_047;
    const SEVERANCE: i64 = 900_000_000_000_000_048;
    const SEVERED: i64 = 900_000_000_000_000_049;
    const ACTIVE_FOLLOW: i64 = 900_000_000_000_000_050;
    const PASSIVE_FOLLOW: i64 = 900_000_000_000_000_051;
    const ANNUAL_REPORT: i64 = 900_000_000_000_000_052;
    const FASP_RECOMMENDATION: i64 = 900_000_000_000_000_053;
    const REPORT_NOTIFICATION: i64 = 900_000_000_000_000_054;
    const WARNING: i64 = 900_000_000_000_000_055;
    const WARNING_NOTIFICATION: i64 = 900_000_000_000_000_056;
    const OTHER_SEVERANCE: i64 = 900_000_000_000_000_057;
    const SUBDOMAIN_ACCOUNT: i64 = 900_000_000_000_000_058;
    const SUBDOMAIN_EMOJI: i64 = 900_000_000_000_000_059;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const MODERATOR_ROLE: i64 = 92;
    const LOCAL_ACCOUNT: i64 = 116_844_606_259_201_001;
    const DOMAIN: &str = "domain-purge.fixture.invalid";
    const SUBDOMAIN: &str = "child.domain-purge.fixture.invalid";
    const ACTOR_URI: &str = "https://domain-purge.fixture.invalid/users/domain-purge";
    const STATUS_URI: &str = "https://domain-purge.fixture.invalid/users/domain-purge/statuses/1";
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let writer_url = std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    let purge_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&writer_url)
        .await?;
    reset().await?;
    let original_permissions: i64 =
        sqlx::query_scalar("SELECT permissions FROM user_roles WHERE id = $1")
            .bind(MODERATOR_ROLE)
            .fetch_one(&writer_pool)
            .await?;
    sqlx::query("UPDATE user_roles SET permissions = permissions | $1 WHERE id = $2")
        .bind(1_i64 << 5)
        .bind(MODERATOR_ROLE)
        .execute(&writer_pool)
        .await?;
    let original_local_stats: (i64, i64) = sqlx::query_as(
        "SELECT followers_count, following_count FROM account_stats WHERE account_id = $1",
    )
    .bind(LOCAL_ACCOUNT)
    .fetch_one(&writer_pool)
    .await?;
    let media_root_path = std::env::temp_dir().join(format!(
        "rustodon-domain-purge-media-{}",
        std::process::id()
    ));
    fs::create_dir_all(&media_root_path)?;
    let media_root = PaperclipRoot::open(&media_root_path)?;
    let media_metadata = [
        PaperclipMetadata {
            attachment: PaperclipAttachment::AccountAvatar,
            id: ACCOUNT,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "domain-purge-avatar.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::AccountHeader,
            id: ACCOUNT,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "domain-purge-header.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: MEDIA,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "domain-purge-media.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::MediaThumbnail,
            id: MEDIA,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "domain-purge-media-thumb.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::CustomEmojiImage,
            id: EMOJI,
            remote: true,
            storage_schema_version: Some(1),
            file_name: "domain-purge-emoji.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
    ];
    for metadata in &media_metadata {
        for style in ["original", "small", "static"] {
            if let Some(path) = metadata.relative_path(style) {
                media_root.write_file(Path::new(&path), b"domain-purge-media")?;
            }
        }
    }
    let operation = async {
        sqlx::query(
            "INSERT INTO accounts
                 (id, actor_type, domain, username, uri, created_at, updated_at)
             VALUES ($1, 'Person', $2, 'domain-purge', $3, clock_timestamp(), clock_timestamp())",
        )
        .bind(ACCOUNT)
        .bind(DOMAIN)
        .bind(ACTOR_URI)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO accounts
                 (id, actor_type, domain, username, uri, created_at, updated_at)
             VALUES ($1, 'Person', $2, 'child-domain-purge', $3,
                     clock_timestamp(), clock_timestamp())",
        )
        .bind(SUBDOMAIN_ACCOUNT)
        .bind(SUBDOMAIN)
        .bind("https://child.domain-purge.fixture.invalid/users/child-domain-purge")
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO account_stats (id, account_id, created_at, updated_at)
             VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
        )
        .bind(ACCOUNT_STATS)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE accounts SET
                 avatar_content_type = 'image/png', avatar_file_name = 'domain-purge-avatar.png',
                 avatar_file_size = 12, avatar_storage_schema_version = 1,
                 avatar_updated_at = clock_timestamp(), header_content_type = 'image/png',
                 header_file_name = 'domain-purge-header.png', header_file_size = 24,
                 header_storage_schema_version = 1, header_updated_at = clock_timestamp()
               WHERE id = $1",
        )
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO statuses
                 (id, account_id, created_at, local, text, updated_at, uri, url)
             VALUES ($1, $2, clock_timestamp(), false, 'domain purge status',
                     clock_timestamp(), $3, $3)",
        )
        .bind(STATUS)
        .bind(ACCOUNT)
        .bind(STATUS_URI)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO media_attachments
                 (id, account_id, type, processing, status_id, file_content_type, file_file_name,
                  file_file_size, file_storage_schema_version, file_updated_at,
                  thumbnail_content_type, thumbnail_file_name, thumbnail_file_size,
                  thumbnail_storage_schema_version, thumbnail_updated_at, remote_url,
                  created_at, updated_at)
             VALUES ($1, $2, 0, 2, $3, 'image/png', 'domain-purge-media.png', 36, 1,
                     clock_timestamp(), 'image/png', 'domain-purge-media-thumb.png', 18, 1,
                     clock_timestamp(), 'https://media.example.invalid/domain-purge-media.png',
                     clock_timestamp(), clock_timestamp())",
        )
        .bind(MEDIA)
        .bind(ACCOUNT)
        .bind(STATUS)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE accounts SET avatar_file_name = 'domain-purge-avatar.png',
                    header_file_name = 'domain-purge-header.png' WHERE id = $1",
        )
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO custom_emojis
                 (id, domain, shortcode, image_content_type, image_file_name, image_file_size,
                  image_storage_schema_version, image_remote_url, image_updated_at,
                  created_at, updated_at)
             VALUES ($1, $2, 'domain-purge', 'image/png', 'domain-purge-emoji.png', 8, 1,
                     'https://media.example.invalid/domain-purge-emoji.png', clock_timestamp(),
                     clock_timestamp(), clock_timestamp())",
        )
        .bind(EMOJI)
        .bind(DOMAIN)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO custom_emojis
                 (id, domain, shortcode, image_content_type, image_file_name, image_file_size,
                  image_storage_schema_version, image_remote_url, image_updated_at,
                  created_at, updated_at)
             VALUES ($1, $2, 'child-domain-purge', 'image/png', 'child-domain-purge.png', 8, 1,
                     'https://media.example.invalid/child-domain-purge.png', clock_timestamp(),
                     clock_timestamp(), clock_timestamp())",
        )
        .bind(SUBDOMAIN_EMOJI)
        .bind(SUBDOMAIN)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO follows
                 (id, account_id, target_account_id, uri, show_reblogs, notify, languages,
                  created_at, updated_at)
             VALUES ($1, $2, $3, 'https://fixture-v4-6-5.rustodon.invalid/activities/purge-active',
                     true, false, ARRAY['en'], clock_timestamp(), clock_timestamp()),
                    ($4, $3, $2, 'https://fixture-v4-6-5.rustodon.invalid/activities/purge-passive',
                     false, true, ARRAY['de'], clock_timestamp(), clock_timestamp())",
        )
        .bind(ACTIVE_FOLLOW)
        .bind(LOCAL_ACCOUNT)
        .bind(ACCOUNT)
        .bind(PASSIVE_FOLLOW)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE account_stats SET followers_count = followers_count + 1,
                    following_count = following_count + 1, updated_at = clock_timestamp()
               WHERE account_id = $1",
        )
        .bind(LOCAL_ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE account_stats SET followers_count = 1, following_count = 1,
                    statuses_count = 1, last_status_at = clock_timestamp(),
                    updated_at = clock_timestamp()
               WHERE account_id = $1",
        )
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO reports
                 (id, account_id, target_account_id, status_ids, created_at, updated_at)
             VALUES ($1, $2, $3, ARRAY[$4]::bigint[], clock_timestamp(), clock_timestamp())",
        )
        .bind(REPORT)
        .bind(LOCAL_ACCOUNT)
        .bind(ACCOUNT)
        .bind(STATUS)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO generated_annual_reports
                 (id, account_id, year, data, schema_version, created_at, updated_at)
             VALUES ($1, $2, 2026, '{}'::jsonb, 1, clock_timestamp(), clock_timestamp())",
        )
        .bind(ANNUAL_REPORT)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO fasp_follow_recommendations
                 (id, requesting_account_id, recommended_account_id, created_at, updated_at)
             VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp())",
        )
        .bind(FASP_RECOMMENDATION)
        .bind(LOCAL_ACCOUNT)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO notifications
                 (id, account_id, activity_id, activity_type, created_at, from_account_id,
                  type, updated_at)
             VALUES ($1, $2, $3, 'Status', clock_timestamp(), $4, 'mention', clock_timestamp())",
        )
        .bind(NOTIFICATION)
        .bind(LOCAL_ACCOUNT)
        .bind(STATUS)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO notifications
                 (id, account_id, activity_id, activity_type, created_at, from_account_id,
                  type, updated_at)
             VALUES ($1, $2, $3, 'Report', clock_timestamp(), $4, 'admin.report', clock_timestamp())",
        )
        .bind(REPORT_NOTIFICATION)
        .bind(LOCAL_ACCOUNT)
        .bind(REPORT)
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO account_warnings
                 (id, account_id, action, created_at, report_id, target_account_id, text, updated_at)
             VALUES ($1, $2, 0, clock_timestamp(), $3, $4, '', clock_timestamp())",
        )
        .bind(WARNING)
        .bind(MODERATOR)
        .bind(REPORT)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO notifications
                 (id, account_id, activity_id, activity_type, created_at, from_account_id,
                  type, updated_at)
             VALUES ($1, $2, $3, 'AccountWarning', clock_timestamp(), $4,
                     'moderation_warning', clock_timestamp())",
        )
        .bind(WARNING_NOTIFICATION)
        .bind(LOCAL_ACCOUNT)
        .bind(WARNING)
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO relationship_severance_events
                 (id, type, target_name, purged, created_at, updated_at)
             VALUES ($1, 0, $2, false, clock_timestamp(), clock_timestamp())",
        )
        .bind(SEVERANCE)
        .bind(DOMAIN)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO relationship_severance_events
                 (id, type, target_name, purged, created_at, updated_at)
             VALUES ($1, 2, $2, false, clock_timestamp(), clock_timestamp())",
        )
        .bind(OTHER_SEVERANCE)
        .bind(DOMAIN)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO severed_relationships
                 (id, created_at, direction, languages, local_account_id, notify,
                  relationship_severance_event_id, remote_account_id, show_reblogs, updated_at)
             VALUES ($1, clock_timestamp(), 0, ARRAY['en'], $2, false, $3, $4, true,
                     clock_timestamp())",
        )
        .bind(SEVERED)
        .bind(LOCAL_ACCOUNT)
        .bind(SEVERANCE)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query("REFRESH MATERIALIZED VIEW public.instances")
            .execute(&writer_pool)
            .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM instances WHERE domain = $1")
                .bind(DOMAIN)
                .fetch_one(&writer_pool)
                .await?,
            1
        );

        let writer = WriteRepository::from_pool(writer_pool.clone());
        let requested_domain = DOMAIN.to_ascii_uppercase();
        writer
            .request_domain_purge(MODERATOR, &requested_domain)
            .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND logical_key = $2
                    AND payload -> 'arguments' ->> 'domain' = $3",
            )
            .bind(MASTODON_DOMAIN_PURGE_JOB_KIND)
            .bind(format!("mastodon:domain-purge:{DOMAIN}"))
            .bind(DOMAIN)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        let queue = Queue::new(runtime_pool.clone());
        assert_eq!(queue.dispatch_outbox(100).await?, 1);
        let config = ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            media_root_url: "/system".to_owned(),
            media_root: Some(media_root.clone()),
            limited_federation: false,
            #[cfg(feature = "test-support")]
            remote_media_endpoint: None,
            #[cfg(feature = "test-support")]
            remote_delivery_endpoint: None,
            #[cfg(feature = "test-support")]
            remote_fetch_endpoint: None,
        };
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(purge_pool.clone()),
            None,
            Some(config),
        )?;
        let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
        assert!(
            executor
                .process_one(
                    "domain-purge-worker",
                    &[Lane::Maintenance],
                    Duration::seconds(30),
                )
                .await?
        );
        let job_error = sqlx::query_scalar::<_, Option<String>>(
            "SELECT last_error FROM rustodon.durable_jobs
              WHERE logical_key = $1",
        )
        .bind(format!("mastodon:domain-purge:{DOMAIN}"))
        .fetch_optional(&writer_pool)
        .await?;
        assert!(
            job_error.as_ref().is_none_or(Option::is_none),
            "differential writer domain purge failed: {job_error:?}"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounts WHERE id = $1")
                .bind(ACCOUNT)
                .fetch_one(&writer_pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounts WHERE id = $1")
                .bind(SUBDOMAIN_ACCOUNT)
                .fetch_one(&writer_pool)
                .await?,
            1,
            "domain purge must not remove subdomain accounts"
        );
        for (table, id) in [
            ("account_stats", ACCOUNT_STATS),
            ("statuses", STATUS),
            ("media_attachments", MEDIA),
            ("custom_emojis", EMOJI),
             ("reports", REPORT),
             ("notifications", NOTIFICATION),
             ("notifications", REPORT_NOTIFICATION),
             ("notifications", WARNING_NOTIFICATION),
             ("generated_annual_reports", ANNUAL_REPORT),
             ("fasp_follow_recommendations", FASP_RECOMMENDATION),
             ("account_warnings", WARNING),
             ("follows", ACTIVE_FOLLOW),
             ("follows", PASSIVE_FOLLOW),
        ] {
            let query = format!("SELECT count(*) FROM {table} WHERE id = $1");
            assert_eq!(
                sqlx::query_scalar::<_, i64>(&query)
                    .bind(id)
                    .fetch_one(&writer_pool)
                    .await?,
                0,
                "domain purge left {table} row {id}"
            );
        }
        for metadata in &media_metadata {
            for style in ["original", "small", "static"] {
                if let Some(path) = metadata.relative_path(style) {
                    assert!(
                        media_root.open_file(Path::new(&path)).is_err(),
                        "domain purge left media file {path}"
                    );
                }
            }
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM custom_emojis WHERE id = $1")
                .bind(SUBDOMAIN_EMOJI)
                .fetch_one(&writer_pool)
                .await?,
            1,
            "domain purge must not remove subdomain emoji"
        );
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT purged FROM relationship_severance_events WHERE id = $1",
            )
            .bind(SEVERANCE)
            .fetch_one(&writer_pool)
            .await?
        );
        assert!(!sqlx::query_scalar::<_, bool>(
            "SELECT purged FROM relationship_severance_events WHERE id = $1",
        )
            .bind(OTHER_SEVERANCE)
            .fetch_one(&writer_pool)
            .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'object_id' = $2",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(STATUS.to_string())
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM severed_relationships WHERE relationship_severance_event_id = $1",
            )
            .bind(SEVERANCE)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM instances WHERE domain = $1")
                .bind(DOMAIN)
                .fetch_one(&writer_pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM instances WHERE domain = $1")
                .bind(SUBDOMAIN)
                .fetch_one(&writer_pool)
                .await?,
            1,
            "domain purge must refresh but preserve subdomain instances"
        );
        assert_eq!(
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT followers_count, following_count FROM account_stats WHERE account_id = $1",
            )
            .bind(LOCAL_ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            original_local_stats
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM admin_action_logs
                  WHERE account_id = $1 AND action = 'destroy'
                    AND target_type = 'Instance' AND human_identifier = $2",
            )
            .bind(MODERATOR)
            .bind(DOMAIN)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1",
            )
            .bind(MASTODON_DOMAIN_PURGE_JOB_KIND)
            .fetch_one(&runtime_pool)
            .await?,
            0
        );
        writer
            .request_domain_purge(MODERATOR, &requested_domain)
            .await?;
        assert_eq!(queue.dispatch_outbox(100).await?, 1);
        assert!(
            executor
                .process_one(
                    "domain-purge-retry-worker",
                    &[Lane::Maintenance],
                    Duration::seconds(30),
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1",
            )
            .bind(MASTODON_DOMAIN_PURGE_JOB_KIND)
            .fetch_one(&runtime_pool)
            .await?,
            0,
            "a completed domain purge must be safely retryable"
        );
        drop(executor);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    drop(media_root);
    let _ = fs::remove_dir_all(&media_root_path);
    sqlx::query("DELETE FROM custom_emojis WHERE id = $1")
        .bind(EMOJI)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM custom_emojis WHERE id = $1")
        .bind(SUBDOMAIN_EMOJI)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM media_attachments WHERE id = $1")
        .bind(MEDIA)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(STATUS)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM reports WHERE id = $1")
        .bind(REPORT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM notifications WHERE id = $1")
        .bind(NOTIFICATION)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM notifications WHERE id IN ($1, $2)")
        .bind(REPORT_NOTIFICATION)
        .bind(WARNING_NOTIFICATION)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM account_warnings WHERE id = $1")
        .bind(WARNING)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM generated_annual_reports WHERE id = $1")
        .bind(ANNUAL_REPORT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM fasp_follow_recommendations WHERE id = $1")
        .bind(FASP_RECOMMENDATION)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follows WHERE id IN ($1, $2)")
        .bind(ACTIVE_FOLLOW)
        .bind(PASSIVE_FOLLOW)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM severed_relationships WHERE id = $1")
        .bind(SEVERED)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM relationship_severance_events WHERE id = $1")
        .bind(SEVERANCE)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM relationship_severance_events WHERE id = $1")
        .bind(OTHER_SEVERANCE)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM account_stats WHERE id = $1")
        .bind(ACCOUNT_STATS)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM accounts WHERE id IN ($1, $2)")
        .bind(ACCOUNT)
        .bind(SUBDOMAIN_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "DELETE FROM admin_action_logs
          WHERE account_id = $1 AND target_type = 'Instance' AND human_identifier = $2",
    )
    .bind(MODERATOR)
    .bind(DOMAIN)
    .execute(&writer_pool)
    .await?;
    sqlx::query("REFRESH MATERIALIZED VIEW public.instances")
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE user_roles SET permissions = $1 WHERE id = $2")
        .bind(original_permissions)
        .bind(MODERATOR_ROLE)
        .execute(&writer_pool)
        .await?;
    reset().await?;
    operation?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn unsuspending_account_cancels_pending_deletion_jobs()
-> Result<(), Box<dyn std::error::Error>> {
    const ACCOUNT: i64 = 116_844_606_259_201_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ACTOR_URI: &str = "https://fixture-v4-6-5.rustodon.invalid/ap/users/116844606259201001";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let operation = async {
        WriteRepository::from_pool(writer_pool.clone())
            .request_account_deletion(ACCOUNT, ACTOR_URI)
            .await?;
        WriteRepository::from_pool(writer_pool.clone())
            .set_account_suspension(
                MODERATOR,
                ACCOUNT,
                false,
                "https://fixture-v4-6-5.rustodon.invalid",
            )
            .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE payload -> 'arguments' ->> 'account_id' = $1
                    AND kind IN ($2, $3)",
            )
            .bind(ACCOUNT.to_string())
            .bind(ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND)
            .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        let account_state: (Option<NaiveDateTime>, Option<i32>) =
            sqlx::query_as("SELECT suspended_at, suspension_origin FROM accounts WHERE id = $1")
                .bind(ACCOUNT)
                .fetch_one(&writer_pool)
                .await?;
        assert_eq!(account_state, (None, None));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_deletion_requests WHERE account_id = $1",
            )
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        assert_eq!(
            Queue::new(runtime_pool.clone())
                .dispatch_outbox(100)
                .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs
                  WHERE arguments ->> 'account_id' = $1
                    AND kind IN ($2, $3)",
            )
            .bind(ACCOUNT.to_string())
            .bind(ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND)
            .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
            .fetch_one(&runtime_pool)
            .await?,
            0
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    reset().await?;
    operation?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn actor_delete_delivery_holds_lifecycle_lock_until_send_finishes()
-> Result<(), Box<dyn std::error::Error>> {
    const ACCOUNT: i64 = 116_844_606_259_201_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ACTOR_URI: &str = "https://fixture-v4-6-5.rustodon.invalid/ap/users/116844606259201001";
    const INBOX_URI: &str = "http://remote.fixture.invalid/inbox";
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let accepted = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let server = tokio::spawn(fixture_blocking_delivery_server(
        listener,
        accepted.clone(),
        release.clone(),
    ));
    let writer = WriteRepository::from_pool(writer_pool.clone());
    writer.request_account_deletion(ACCOUNT, ACTOR_URI).await?;
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: None,
        limited_federation: false,
        remote_media_endpoint: None,
        remote_delivery_endpoint: Some(endpoint),
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_DELIVERY_JOB_KIND,
                json!({
                    "source_account_id": ACCOUNT,
                    "inbox_url": INBOX_URI,
                    "remote_domain": "remote.fixture.invalid",
                    "body": activitypub::delete_actor_with_uris(
                        &format!("{ACTOR_URI}#delete"),
                        ACTOR_URI,
                    ),
                    "activity_type": "AccountDelete"
                }),
            )
            .logical_key("account-delete-lifecycle-lock"),
        )
        .await?;
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue, handlers, 1, 1)?;
    let worker = tokio::spawn(async move {
        executor
            .process_one(
                "account-delete-lifecycle-worker",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await
    });
    accepted.notified().await;

    let unsuspender = WriteRepository::from_pool(writer_pool.clone());
    let mut unsuspend = tokio::spawn(async move {
        unsuspender
            .set_account_suspension(MODERATOR, ACCOUNT, false, ORIGIN)
            .await
    });
    let finished_while_delivery_blocked =
        tokio::time::timeout(std::time::Duration::from_millis(250), &mut unsuspend)
            .await
            .is_ok();
    release.notify_one();
    assert!(!finished_while_delivery_blocked);
    assert!(worker.await??);
    assert!(server.await??.windows(4).any(|window| window == b"POST"));
    unsuspend.await??;
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn stale_account_purge_job_does_not_remove_media_after_unsuspend()
-> Result<(), Box<dyn std::error::Error>> {
    const ACCOUNT: i64 = 116_844_606_259_201_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ACTOR_URI: &str = "https://fixture-v4-6-5.rustodon.invalid/ap/users/116844606259201001";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let media_root_path = std::env::temp_dir().join(format!(
        "rustodon-stale-account-purge-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&media_root_path);
    fs::create_dir_all(&media_root_path)?;
    let media_root = PaperclipRoot::open(&media_root_path)?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::AccountAvatar,
        id: ACCOUNT,
        remote: false,
        storage_schema_version: Some(1),
        file_name: "restored-avatar.png".to_owned(),
        content_type: Some("image/png".to_owned()),
        variant: None,
    };
    let cleanup_path = metadata
        .relative_path("original")
        .expect("account avatar has a Paperclip path");
    media_root.write_file(Path::new(&cleanup_path), b"must survive unsuspend")?;

    let operation = async {
        WriteRepository::from_pool(writer_pool.clone())
            .request_account_deletion(ACCOUNT, ACTOR_URI)
            .await?;
        WriteRepository::from_pool(writer_pool.clone())
            .set_account_suspension(
                MODERATOR,
                ACCOUNT,
                false,
                "https://fixture-v4-6-5.rustodon.invalid",
            )
            .await?;

        let queue = Queue::new(runtime_pool.clone());
        let config = ActivityPubDeliveryConfig {
            origin: Url::parse("https://fixture-v4-6-5.rustodon.invalid")?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            media_root_url: "/system".to_owned(),
            media_root: Some(media_root.clone()),
            limited_federation: false,
            remote_media_endpoint: None,
            remote_delivery_endpoint: None,
            remote_fetch_endpoint: None,
        };
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(writer_pool.clone()),
            None,
            Some(config),
        )?;
        let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Maintenance,
                    MASTODON_ACCOUNT_PURGE_JOB_KIND,
                    json!({
                        "account_id": ACCOUNT,
                        "cleanup_paths": [cleanup_path]
                    }),
                )
                .logical_key("mastodon:account:stale-unsuspend-purge"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "stale-unsuspend-purge-worker",
                    &[Lane::Maintenance],
                    Duration::seconds(30),
                )
                .await?
        );
        assert!(
            media_root.open_file(Path::new(&cleanup_path)).is_ok(),
            "a stale purge job must not remove media after unsuspension"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_deletion_requests WHERE account_id = $1",
            )
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    drop(media_root);
    fs::remove_dir_all(&media_root_path)?;
    reset().await?;
    operation?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn stale_authenticated_account_writes_are_rejected_after_deletion_request()
-> Result<(), Box<dyn std::error::Error>> {
    const ACCOUNT: i64 = 116_844_606_259_201_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ACTOR_URI: &str = "https://fixture-v4-6-5.rustodon.invalid/ap/users/116844606259201001";
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid";

    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let writer = WriteRepository::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&owner_url).await?);
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;

    let operation = async {
        macro_rules! assert_stale_write {
            ($label:literal, $future:expr) => {
                assert!(
                    matches!(($future).await, Err(WriteError::Unauthorized)),
                    "stale authenticated write was accepted: {}",
                    $label
                );
            };
        }

        writer.request_account_deletion(ACCOUNT, ACTOR_URI).await?;
        assert_stale_write!(
            "report",
            writer.create_report(
                &authenticated,
                ACCOUNT,
                "stale report",
                None,
                &[],
                &[],
                &[],
                None,
                None,
                ORIGIN,
                false,
            )
        );
        assert_stale_write!("markers", writer.update_markers(&authenticated, &[]));
        assert_stale_write!(
            "marker",
            writer.update_marker(&authenticated, "home", 0, None)
        );
        assert_stale_write!(
            "account profile",
            writer.update_account_profile(&authenticated, &AccountProfileUpdate::default())
        );
        assert!(matches!(
            writer
                .stage_media_attachment(
                    &authenticated,
                    &MediaAttachmentCreate {
                        file_name: "stale-upload.png".to_owned(),
                        content_type: "image/png".to_owned(),
                        file_size: 3,
                        file_meta: json!({}),
                        blurhash: None,
                        description: None,
                        focus: AccountProfileValue::Unchanged,
                    },
                )
                .await,
            Err(WriteError::Unauthorized)
        ));
        assert!(matches!(
            writer
                .update_account_profile(&authenticated, &AccountProfileUpdate::default())
                .await,
            Err(WriteError::Unauthorized)
        ));
        assert_stale_write!(
            "media update",
            writer.update_media_attachment(&authenticated, -1, &MediaAttachmentUpdate::default())
        );
        assert_stale_write!(
            "media delete",
            writer.delete_media_attachment(&authenticated, -1)
        );
        assert_stale_write!(
            "conversation unread",
            writer.update_conversation_unread(&authenticated, -1, true)
        );
        assert_stale_write!(
            "conversation optimistic unread",
            writer.update_conversation_unread_with_lock_version(&authenticated, -1, true, 0)
        );
        assert_stale_write!(
            "conversation delete",
            writer.delete_conversation(&authenticated, -1)
        );
        assert_stale_write!("bookmark", writer.set_bookmark(&authenticated, -1, false));
        assert_stale_write!(
            "status mute",
            writer.set_status_mute(&authenticated, -1, false)
        );
        assert_stale_write!(
            "status pin",
            writer.set_status_pin(&authenticated, -1, false)
        );
        assert_stale_write!("favourite", writer.set_favourite(&authenticated, -1, false));
        assert_stale_write!(
            "reblog create",
            writer.set_reblog(&authenticated, -1, None, true)
        );
        assert_stale_write!(
            "reblog remove",
            writer.set_reblog(&authenticated, -1, None, false)
        );
        assert!(matches!(
            writer
                .create_status(
                    &authenticated,
                    "stale status",
                    &[],
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await,
            Err(WriteError::Unauthorized)
        ));
        assert_stale_write!(
            "status update",
            writer.update_status(&authenticated, -1, &StatusUpdate::default())
        );
        assert_stale_write!(
            "status delete",
            writer.delete_status(&authenticated, -1, false)
        );
        assert_stale_write!(
            "follow",
            writer.set_follow(&authenticated, -1, false, None, None, None)
        );
        assert_stale_write!(
            "follow request authorize",
            writer.authorize_follow_request(&authenticated, -1)
        );
        assert_stale_write!(
            "follow request reject",
            writer.reject_follow_request(&authenticated, -1)
        );
        assert_stale_write!(
            "remove follower",
            writer.remove_follower(&authenticated, -1)
        );
        assert_stale_write!("block", writer.set_block(&authenticated, -1, false));
        assert_stale_write!(
            "mute",
            writer.set_mute(&authenticated, -1, false, None, None)
        );
        assert_stale_write!(
            "marker with options",
            writer.update_marker_with_options(
                &authenticated,
                "home",
                0,
                None,
                WriteOptions::default(),
            )
        );
        assert_stale_write!(
            "clear notifications",
            writer.clear_notifications(&authenticated)
        );
        assert_stale_write!(
            "dismiss notification",
            writer.dismiss_notification(&authenticated, -1)
        );
        assert_stale_write!(
            "dismiss notification group",
            writer.dismiss_notification_group(&authenticated, "stale-group")
        );
        assert_stale_write!(
            "accept notification request",
            writer.accept_notification_request(&authenticated, -1)
        );
        assert_stale_write!(
            "dismiss notification request",
            writer.dismiss_notification_request(&authenticated, -1)
        );
        assert_stale_write!(
            "accept notification requests",
            writer.accept_notification_requests(&authenticated, &[])
        );
        assert_stale_write!(
            "dismiss notification requests",
            writer.dismiss_notification_requests(&authenticated, &[])
        );
        assert_stale_write!(
            "notification policy",
            writer.update_notification_policy(&authenticated, NotificationPolicyUpdate::default())
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    let _ = WriteRepository::from_pool(writer_pool.clone())
        .set_account_suspension(
            MODERATOR,
            ACCOUNT,
            false,
            "https://fixture-v4-6-5.rustodon.invalid",
        )
        .await;
    sqlx::query("DELETE FROM media_attachments WHERE account_id = $1")
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    reset().await?;
    operation?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn report_creation_waits_for_target_lifecycle_lock() -> Result<(), Box<dyn std::error::Error>>
{
    const TARGET: i64 = 116_844_606_259_201_003;

    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let writer = WriteRepository::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&owner_url).await?);
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_REPORTS).await?;
    let baseline: (Option<NaiveDateTime>, Option<i32>) =
        sqlx::query_as("SELECT suspended_at, suspension_origin FROM accounts WHERE id = $1")
            .bind(TARGET)
            .fetch_one(&writer_pool)
            .await?;
    let mut suspension = writer_pool.begin().await?;
    sqlx::query("SELECT id FROM accounts WHERE id = $1 FOR UPDATE")
        .bind(TARGET)
        .fetch_one(&mut *suspension)
        .await?;
    sqlx::query(
        "UPDATE accounts
            SET suspended_at = clock_timestamp(), suspension_origin = 0
          WHERE id = $1",
    )
    .bind(TARGET)
    .execute(&mut *suspension)
    .await?;

    let report_writer = writer.clone();
    let report_authenticated = authenticated.clone();
    let mut report = tokio::spawn(async move {
        report_writer
            .create_report(
                &report_authenticated,
                TARGET,
                "held lifecycle lock",
                None,
                &[],
                &[],
                &[],
                None,
                None,
                "https://fixture-v4-6-5.rustodon.invalid",
                false,
            )
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(250), &mut report)
            .await
            .is_err(),
        "report validation must wait for the target lifecycle row lock"
    );
    suspension.commit().await?;
    let report_result = report.await?;
    assert!(matches!(report_result, Err(WriteError::NotFound)));

    sqlx::query("UPDATE accounts SET suspended_at = $2, suspension_origin = $3 WHERE id = $1")
        .bind(TARGET)
        .bind(baseline.0)
        .bind(baseline.1)
        .execute(&writer_pool)
        .await?;
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn administrative_remote_purge_undoes_passive_follows()
-> Result<(), Box<dyn std::error::Error>> {
    const ACCOUNT: i64 = 9_000_000_000_000_021;
    const ACCOUNT_STATS: i64 = 9_000_000_000_000_021;
    const FOLLOW: i64 = 9_000_000_000_000_021;
    const REMOTE_FOLLOW: i64 = 9_000_000_000_000_022;
    const FOLLOW_NOTIFICATION: i64 = 9_000_000_000_000_023;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const SOURCE_ACCOUNT: i64 = 116_844_606_259_201_001;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ACTOR_URI: &str = "https://remote.fixture.invalid/users/purge-remote";
    const INBOX_URI: &str = "https://remote.fixture.invalid/inbox";
    const FOLLOW_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/activities/purge-remote-follow";
    const REMOTE_FOLLOW_URI: &str =
        "https://remote.fixture.invalid/activities/purge-remote-following-local";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let source_follow_stats: (i64, i64) = sqlx::query_as(
        "SELECT following_count, followers_count FROM account_stats WHERE account_id = $1",
    )
    .bind(SOURCE_ACCOUNT)
    .fetch_one(&writer_pool)
    .await?;
    let operation = async {
        sqlx::query(
            "INSERT INTO accounts
                 (id, actor_type, created_at, domain, inbox_url, public_key, protocol, uri,
                  username, updated_at)
             VALUES ($1, 'Person', clock_timestamp(), 'remote.fixture.invalid', $2,
                     'remote purge public key', 1, $3, 'purge-remote', clock_timestamp())",
        )
        .bind(ACCOUNT)
        .bind(INBOX_URI)
        .bind(ACTOR_URI)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO account_stats
                 (id, account_id, followers_count, following_count, statuses_count,
                  created_at, updated_at)
             VALUES ($1, $2, 1, 1, 0, clock_timestamp(), clock_timestamp())",
        )
        .bind(ACCOUNT_STATS)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO follows
                 (id, account_id, target_account_id, uri, created_at, updated_at)
             VALUES ($1, $2, $3, $4, clock_timestamp(), clock_timestamp()),
                    ($5, $3, $2, $6, clock_timestamp(), clock_timestamp())",
        )
        .bind(FOLLOW)
        .bind(SOURCE_ACCOUNT)
        .bind(ACCOUNT)
        .bind(FOLLOW_URI)
        .bind(REMOTE_FOLLOW)
        .bind(REMOTE_FOLLOW_URI)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO notifications
                (id, account_id, activity_id, activity_type, from_account_id, type, filtered,
                 created_at, updated_at)
             VALUES ($1, $2, $3, 'Follow', $4, 'follow', false,
                     clock_timestamp(), clock_timestamp())",
        )
        .bind(FOLLOW_NOTIFICATION)
        .bind(SOURCE_ACCOUNT)
        .bind(REMOTE_FOLLOW)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE account_stats SET following_count = following_count + 1,
                    followers_count = followers_count + 1
               WHERE account_id = $1",
        )
        .bind(SOURCE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
        let local_actor_uri = format!("{ORIGIN}users/alice");
        let accept_body = activitypub::accept_with_uris(
            &local_actor_uri,
            REMOTE_FOLLOW,
            REMOTE_FOLLOW_URI,
            ACTOR_URI,
        );
        let mut transaction = writer_pool.begin().await?;
        record_outbox_in(
            &mut transaction,
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_DELIVERY_JOB_KIND,
                json!({
                    "source_account_id": SOURCE_ACCOUNT,
                    "inbox_url": INBOX_URI,
                    "remote_domain": "remote.fixture.invalid",
                    "body": accept_body
                }),
            )
            .logical_key(activitypub::accept_delivery_logical_key(
                REMOTE_FOLLOW,
                REMOTE_FOLLOW_URI,
                INBOX_URI,
            )),
        )
        .await?;
        transaction.commit().await?;

        WriteRepository::from_pool(writer_pool.clone())
            .set_account_suspension(MODERATOR, ACCOUNT, true, ORIGIN)
            .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM follows
                  WHERE account_id = $1 AND target_account_id = $2",
            )
            .bind(ACCOUNT)
            .bind(SOURCE_ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            0,
            "local suspension must remove follows from a remote actor"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT following_count FROM account_stats WHERE account_id = $1",
            )
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            0,
            "local suspension must decrement the remote actor's following count"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT followers_count FROM account_stats WHERE account_id = $1",
            )
            .bind(SOURCE_ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            source_follow_stats.1,
            "local suspension must decrement the local follower count"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notifications WHERE id = $1",)
                .bind(FOLLOW_NOTIFICATION)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "local suspension must remove the follow notification"
        );
        let reject_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body'
               FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload -> 'arguments' ->> 'source_account_id' = $2
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Reject'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'id' = $3",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(SOURCE_ACCOUNT.to_string())
        .bind(REMOTE_FOLLOW_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(reject_body["type"], "Reject");
        assert_eq!(reject_body["object"]["id"], REMOTE_FOLLOW_URI);
        assert_eq!(
            reject_body["id"],
            format!("{local_actor_uri}#rejects/follows/{REMOTE_FOLLOW}")
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload -> 'arguments' -> 'body' ->> 'id' = $2",
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .bind(format!("{local_actor_uri}#accepts/follows/{REMOTE_FOLLOW}"))
            .fetch_one(&writer_pool)
            .await?,
            0,
            "local suspension must cancel a pending Accept for the removed follow"
        );
        sqlx::query(
            "UPDATE account_deletion_requests
                SET created_at = clock_timestamp() - interval '31 days',
                    updated_at = clock_timestamp()
              WHERE account_id = $1",
        )
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        let queue = Queue::new(runtime_pool.clone());
        let config = ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            media_root_url: "/system".to_owned(),
            media_root: None,
            limited_federation: false,
            remote_media_endpoint: None,
            remote_delivery_endpoint: None,
            remote_fetch_endpoint: None,
        };
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(writer_pool.clone()),
            None,
            Some(config),
        )?;
        let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
        assert_eq!(queue.dispatch_outbox(100).await?, 2);
        sqlx::query(
            "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp()
               WHERE kind = $1 AND arguments ->> 'account_id' = $2",
        )
        .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
        .bind(ACCOUNT.to_string())
        .execute(&runtime_pool)
        .await?;
        assert!(
            executor
                .process_one(
                    "administrative-remote-purge",
                    &[Lane::Maintenance],
                    Duration::seconds(30),
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM follows
                  WHERE account_id = $1 AND target_account_id = $2",
            )
            .bind(SOURCE_ACCOUNT)
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT following_count FROM account_stats WHERE account_id = $1",
            )
            .bind(SOURCE_ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            source_follow_stats.0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_deletion_requests WHERE account_id = $1",
            )
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        let account_state: (Option<String>, Option<NaiveDateTime>, Option<i32>) = sqlx::query_as(
            "SELECT domain, suspended_at, suspension_origin FROM accounts WHERE id = $1",
        )
        .bind(ACCOUNT)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(account_state.0.as_deref(), Some("remote.fixture.invalid"));
        assert!(account_state.1.is_some());
        assert_eq!(account_state.2, Some(0));
        let undo_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body'
               FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Undo'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'id' = $2",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(FOLLOW_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(undo_body["type"], "Undo");
        assert_eq!(undo_body["object"]["type"], "Follow");
        assert_eq!(undo_body["object"]["id"], FOLLOW_URI);
        drop(executor);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    sqlx::query("DELETE FROM follows WHERE id IN ($1, $2)")
        .bind(FOLLOW)
        .bind(REMOTE_FOLLOW)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM notifications WHERE id = $1")
        .bind(FOLLOW_NOTIFICATION)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM account_warnings WHERE target_account_id = $1")
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM account_deletion_requests WHERE account_id = $1")
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM admin_action_logs WHERE target_id = $1 AND target_type = 'Account'")
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
           WHERE payload -> 'arguments' ->> 'account_id' = $1
              OR payload -> 'arguments' -> 'body' ->> 'id' IN ($2, $3)",
    )
    .bind(ACCOUNT.to_string())
    .bind(FOLLOW_URI)
    .bind(REMOTE_FOLLOW_URI)
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM rustodon.durable_jobs WHERE arguments ->> 'account_id' = $1")
        .bind(ACCOUNT.to_string())
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "UPDATE account_stats SET following_count = $1, followers_count = $2
           WHERE account_id = $3",
    )
    .bind(source_follow_stats.0)
    .bind(source_follow_stats.1)
    .bind(SOURCE_ACCOUNT)
    .execute(&writer_pool)
    .await?;
    reset().await?;
    operation?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn due_account_purge_retains_reported_content_and_actor_identity()
-> Result<(), Box<dyn std::error::Error>> {
    const ACCOUNT: i64 = 900_000_000_000_000_001;
    const USER: i64 = 9_000_000_000_000_001;
    const ACCOUNT_STATS: i64 = 900_000_000_000_000_002;
    const NORMAL_STATUS: i64 = 9_000_000_000_000_001;
    const REPORTED_STATUS: i64 = 9_000_000_000_000_002;
    const NORMAL_STATUS_STATS: i64 = 9_000_000_000_000_001;
    const REPORTED_STATUS_STATS: i64 = 9_000_000_000_000_002;
    const REPORT: i64 = 9_000_000_000_000_001;
    const MENTION: i64 = 9_000_000_000_000_001;
    const FAVOURITE: i64 = 9_000_000_000_000_001;
    const FOLLOW: i64 = 9_000_000_000_000_001;
    const NORMAL_POLL: i64 = 9_000_000_000_000_003;
    const PROTECTED_POLL: i64 = 9_000_000_000_000_004;
    const ACCOUNT_POLL_VOTE: i64 = 9_000_000_000_000_005;
    const OTHER_POLL_VOTE: i64 = 9_000_000_000_000_006;
    const BOOKMARK: i64 = 9_000_000_000_000_007;
    const PROTECTED_PIN: i64 = 9_000_000_000_000_008;
    const EXTERNAL_PIN: i64 = 9_000_000_000_000_009;
    const ROLLBACK_STATUS: i64 = 9_000_000_000_000_010;
    const ROLLBACK_STATUS_STATS: i64 = 9_000_000_000_000_010;
    const ROLLBACK_CONVERSATION: i64 = 9_000_000_000_000_010;
    const QUOTE: i64 = 9_000_000_000_000_011;
    const NORMAL_MEDIA: i64 = 900_000_000_000_000_011;
    const REPORTED_MEDIA: i64 = 900_000_000_000_000_012;
    const OTHER_ACCOUNT: i64 = 116_844_606_259_201_002;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const EMAIL: &str = "purge-fixture-7001@example.invalid";
    const USERNAME: &str = "purge_fixture_7001";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let (other_status_id, other_favourites_count): (i64, i64) = sqlx::query_as(
        "SELECT status.id, stats.favourites_count
           FROM statuses status
           JOIN status_stats stats ON stats.status_id = status.id
           JOIN accounts account ON account.id = status.account_id
          WHERE account.domain IS NULL AND status.deleted_at IS NULL AND account.id <> $1
          ORDER BY status.id LIMIT 1",
    )
    .bind(ACCOUNT)
    .fetch_one(&writer_pool)
    .await?;
    let other_followers_count: i64 =
        sqlx::query_scalar("SELECT followers_count FROM account_stats WHERE account_id = $1")
            .bind(OTHER_ACCOUNT)
            .fetch_one(&writer_pool)
            .await?;
    let retained_private_key: String =
        sqlx::query_scalar("SELECT private_key FROM accounts WHERE id = $1")
            .bind(OTHER_ACCOUNT)
            .fetch_one(&writer_pool)
            .await?;
    let actor_uri = format!("{ORIGIN}ap/users/{ACCOUNT}");
    let media_root_path = std::env::temp_dir().join(format!(
        "rustodon-account-purge-media-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&media_root_path);
    fs::create_dir_all(&media_root_path)?;
    let media_root = PaperclipRoot::open(&media_root_path)?;
    let profile_media_metadata = [
        PaperclipMetadata {
            attachment: PaperclipAttachment::AccountAvatar,
            id: ACCOUNT,
            remote: false,
            storage_schema_version: Some(1),
            file_name: "old-avatar.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::AccountHeader,
            id: ACCOUNT,
            remote: false,
            storage_schema_version: Some(1),
            file_name: "old-header.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
        PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: NORMAL_MEDIA,
            remote: false,
            storage_schema_version: Some(1),
            file_name: "purge-normal.png".to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        },
    ];
    let reported_media_metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: REPORTED_MEDIA,
        remote: false,
        storage_schema_version: Some(1),
        file_name: "purge-reported.png".to_owned(),
        content_type: Some("image/png".to_owned()),
        variant: None,
    };
    for metadata in &profile_media_metadata {
        if let Some(path) = metadata.relative_path("original") {
            media_root.write_file(Path::new(&path), b"account-purge-media")?;
        }
    }
    if let Some(path) = reported_media_metadata.relative_path("original") {
        media_root.write_file(Path::new(&path), b"account-purge-media")?;
    }

    let operation = async {
        sqlx::query(
            r#"INSERT INTO accounts (
                id, actor_type, also_known_as, avatar_content_type, avatar_description,
                avatar_file_name, avatar_file_size, avatar_remote_url, display_name,
                discoverable, fields, header_content_type, header_description, header_file_name,
                header_file_size, header_remote_url, locked, memorial, note, private_key,
                public_key, protocol, requested_review_at, reviewed_at, shared_inbox_url,
                suspension_origin, trendable, uri, url, username, created_at, updated_at)
             VALUES ($1, 'Person', ARRAY['https://old.example/actor']::varchar[], 'image/png',
                 'old avatar', 'old-avatar.png', 10, 'https://old.example/avatar.png', 'Before Purge', false,
                '[{"name":"Field","value":"before"}]'::jsonb, 'image/png', 'old header',
                'old-header.png', 20, 'https://old.example/header.png', true, true, 'Before note',
                 $2, 'retained-public-key', 1, clock_timestamp(), clock_timestamp(), '', 0, true,
                 $3, 'https://fixture-v4-6-5.rustodon.invalid/@purge_fixture_7001', $4,
                 clock_timestamp(), clock_timestamp())"#,
         )
         .bind(ACCOUNT)
         .bind(&retained_private_key)
         .bind(&actor_uri)
        .bind(USERNAME)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO users (id, account_id, email, encrypted_password, approved,
                confirmed_at, created_at, updated_at)
             VALUES ($1, $2, $3, 'password', true, clock_timestamp(), clock_timestamp(),
                clock_timestamp())",
        )
        .bind(USER)
        .bind(ACCOUNT)
        .bind(EMAIL)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO account_stats (
                id, account_id, followers_count, following_count, statuses_count,
                created_at, updated_at)
             VALUES ($1, $2, 0, 1, 2, clock_timestamp(), clock_timestamp())",
        )
        .bind(ACCOUNT_STATS)
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        for (status_id, status_stats_id, text) in [
            (NORMAL_STATUS, NORMAL_STATUS_STATS, "remove this status"),
            (
                REPORTED_STATUS,
                REPORTED_STATUS_STATS,
                "retain this reported status",
            ),
        ] {
            sqlx::query(
                "INSERT INTO statuses (id, account_id, created_at, local, text, updated_at,
                    uri, url, visibility)
                 VALUES ($1, $2, clock_timestamp(), true, $3, clock_timestamp(), $4, $4, 0)",
            )
            .bind(status_id)
            .bind(ACCOUNT)
            .bind(text)
             .bind(format!("{actor_uri}/statuses/{status_id}"))
            .execute(&writer_pool)
            .await?;
            sqlx::query(
                "INSERT INTO status_stats (id, status_id, created_at, updated_at)
                 VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
            )
            .bind(status_stats_id)
            .bind(status_id)
            .execute(&writer_pool)
            .await?;
        }
        for (media_id, status_id, file_name) in [
            (NORMAL_MEDIA, NORMAL_STATUS, "purge-normal.png"),
            (REPORTED_MEDIA, REPORTED_STATUS, "purge-reported.png"),
        ] {
            sqlx::query(
                "INSERT INTO media_attachments (
                     id, account_id, status_id, type, processing, file_content_type,
                     file_file_name, file_file_size, file_storage_schema_version,
                     file_updated_at, created_at, updated_at)
                 VALUES ($1, $2, $3, 0, 2, 'image/png', $4, 20, 1,
                         clock_timestamp(), clock_timestamp(), clock_timestamp())",
            )
            .bind(media_id)
            .bind(ACCOUNT)
            .bind(status_id)
            .bind(file_name)
            .execute(&writer_pool)
             .await?;
        }
        sqlx::query(
            "INSERT INTO quotes (
                 id, account_id, status_id, quoted_account_id, quoted_status_id, state,
                 legacy, created_at, updated_at)
             VALUES ($1, $2, $3, (SELECT account_id FROM statuses WHERE id = $4), $4, 1,
                     false, clock_timestamp(), clock_timestamp())",
        )
        .bind(QUOTE)
        .bind(ACCOUNT)
        .bind(NORMAL_STATUS)
        .bind(other_status_id)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE status_stats SET quotes_count = quotes_count + 1
              WHERE status_id = $1",
        )
        .bind(other_status_id)
        .execute(&writer_pool)
        .await?;
        for (poll_id, status_id, tally, votes) in [
            (NORMAL_POLL, NORMAL_STATUS, 0_i64, 0_i64),
            (PROTECTED_POLL, REPORTED_STATUS, 2_i64, 2_i64),
        ] {
            sqlx::query(
                "INSERT INTO polls (
                    id, account_id, cached_tallies, created_at, options, status_id,
                    updated_at, voters_count, votes_count)
                 VALUES ($1, $2, ARRAY[$3]::bigint[], clock_timestamp(),
                    ARRAY['option']::varchar[], $4, clock_timestamp(), $5, $5)",
            )
            .bind(poll_id)
            .bind(ACCOUNT)
            .bind(tally)
            .bind(status_id)
            .bind(votes)
            .execute(&writer_pool)
            .await?;
            sqlx::query("UPDATE statuses SET poll_id = $2 WHERE id = $1")
                .bind(status_id)
                .bind(poll_id)
                .execute(&writer_pool)
                .await?;
        }
        for (vote_id, vote_account_id) in [
            (ACCOUNT_POLL_VOTE, ACCOUNT),
            (OTHER_POLL_VOTE, OTHER_ACCOUNT),
        ] {
            sqlx::query(
                "INSERT INTO poll_votes (id, account_id, choice, created_at, poll_id, updated_at)
                 VALUES ($1, $2, 0, clock_timestamp(), $3, clock_timestamp())",
            )
            .bind(vote_id)
            .bind(vote_account_id)
            .bind(PROTECTED_POLL)
            .execute(&writer_pool)
            .await?;
        }
        sqlx::query(
            "INSERT INTO reports (id, account_id, target_account_id, status_ids, created_at,
                updated_at)
             VALUES ($1, $2, $3, $4, clock_timestamp(), clock_timestamp())",
        )
        .bind(REPORT)
        .bind(OTHER_ACCOUNT)
        .bind(ACCOUNT)
        .bind(vec![REPORTED_STATUS])
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO mentions (id, account_id, status_id, created_at, updated_at)
             VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp())",
        )
        .bind(MENTION)
        .bind(ACCOUNT)
        .bind(other_status_id)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO favourites (id, account_id, status_id, created_at, updated_at)
             VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp())",
        )
        .bind(FAVOURITE)
        .bind(ACCOUNT)
        .bind(other_status_id)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO bookmarks (id, account_id, created_at, status_id, updated_at)
             VALUES ($1, $2, clock_timestamp(), $3, clock_timestamp())",
        )
        .bind(BOOKMARK)
        .bind(ACCOUNT)
        .bind(other_status_id)
        .execute(&writer_pool)
        .await?;
        for (pin_id, status_id) in [
            (PROTECTED_PIN, REPORTED_STATUS),
            (EXTERNAL_PIN, other_status_id),
        ] {
            sqlx::query(
                "INSERT INTO status_pins (id, account_id, created_at, status_id, updated_at)
                 VALUES ($1, $2, clock_timestamp(), $3, clock_timestamp())",
            )
            .bind(pin_id)
            .bind(ACCOUNT)
            .bind(status_id)
            .execute(&writer_pool)
            .await?;
        }
        sqlx::query(
            "UPDATE status_stats SET favourites_count = favourites_count + 1
              WHERE status_id = $1",
        )
        .bind(other_status_id)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO follows (id, account_id, target_account_id, created_at, updated_at, uri)
             VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp(), $4)",
        )
        .bind(FOLLOW)
        .bind(ACCOUNT)
        .bind(OTHER_ACCOUNT)
        .bind("https://fixture-v4-6-5.rustodon.invalid/activities/purge-follow")
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE account_stats SET followers_count = followers_count + 1
              WHERE account_id = $1",
        )
        .bind(OTHER_ACCOUNT)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "UPDATE accounts
                SET inbox_url = 'http://remote.fixture.invalid/inbox', shared_inbox_url = ''
              WHERE domain IS NOT NULL AND protocol = 1",
        )
        .execute(&writer_pool)
        .await?;

        WriteRepository::from_pool(writer_pool.clone())
            .request_account_deletion(ACCOUNT, &actor_uri)
            .await?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = listener.local_addr()?;
        let server = tokio::spawn(fixture_delivery_server(listener));
        let config = ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            media_root_url: "/system".to_owned(),
            media_root: Some(media_root.clone()),
            limited_federation: false,
            remote_media_endpoint: None,
            remote_delivery_endpoint: Some(endpoint),
            remote_fetch_endpoint: None,
        };
        let queue = Queue::new(runtime_pool.clone());
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(writer_pool.clone()),
            None,
            Some(config),
        )?;
        let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
        assert_eq!(queue.dispatch_outbox(100).await?, 2);
        sqlx::query(
            "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp()
               WHERE kind = $1 AND arguments ->> 'account_id' = $2",
        )
        .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
        .bind(ACCOUNT.to_string())
        .execute(&runtime_pool)
        .await?;
        let accounts_path = media_root_path.join("accounts");
        fs::remove_dir_all(&accounts_path)?;
        fs::write(&accounts_path, b"temporary cleanup failure")?;
        assert!(
            executor
                .process_one(
                    "account-purge-too-early",
                    &[Lane::Maintenance],
                    Duration::seconds(30),
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, Vec<i64>>(
                "SELECT array_agg(id ORDER BY id) FROM statuses WHERE account_id = $1",
            )
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            vec![NORMAL_STATUS, REPORTED_STATUS]
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_deletion_requests WHERE account_id = $1",
            )
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
        .await?,
            1
        );
        sqlx::query(
            "UPDATE account_deletion_requests
                SET created_at = clock_timestamp() - interval '31 days',
                    updated_at = clock_timestamp()
              WHERE account_id = $1",
        )
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Maintenance,
                    MASTODON_ACCOUNT_PURGE_JOB_KIND,
                    json!({"account_id": ACCOUNT}),
                )
                .logical_key(format!("mastodon:account:{ACCOUNT}:purge:due")),
            )
            .await?;
        sqlx::query(
            "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp()
               WHERE kind = $1 AND arguments ->> 'account_id' = $2",
        )
        .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
        .bind(ACCOUNT.to_string())
        .execute(&runtime_pool)
        .await?;
        assert!(
            executor
                .process_one(
                    "account-purge-worker",
                    &[Lane::Maintenance],
                    Duration::seconds(30)
                )
                .await?
        );
        let purge_state: Option<(i32, Option<String>, Value)> = sqlx::query_as(
            "SELECT attempts, last_error, arguments FROM rustodon.durable_jobs
              WHERE kind = $1 AND arguments ->> 'account_id' = $2",
        )
        .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
        .bind(ACCOUNT.to_string())
        .fetch_optional(&runtime_pool)
        .await?;
        assert!(
            purge_state.is_some(),
            "account purge should retain its durable cleanup manifest"
        );
        let (attempts, last_error, arguments) = purge_state.expect("purge retry state");
        assert_eq!(attempts, 1);
        assert_eq!(last_error.as_deref(), Some("account purge media removal failed"));
        assert!(
            arguments["cleanup_paths"]
                .as_array()
                .is_some_and(|paths| !paths.is_empty()),
            "account purge retry lost its cleanup paths"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE account_id = $1")
                .bind(ACCOUNT)
                .fetch_one(&writer_pool)
                .await?,
            1,
            "database purge commits before filesystem cleanup"
        );
        fs::remove_file(&accounts_path)?;
        fs::create_dir_all(&accounts_path)?;
        for metadata in &profile_media_metadata {
            if let Some(path) = metadata.relative_path("original") {
                media_root.write_file(Path::new(&path), b"account-purge-media")?;
            }
        }
        sqlx::query(
            "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp()
               WHERE kind = $1 AND arguments ->> 'account_id' = $2",
        )
        .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
        .bind(ACCOUNT.to_string())
        .execute(&runtime_pool)
        .await?;
        assert!(
            executor
                .process_one(
                    "account-purge-retry",
                    &[Lane::Maintenance],
                    Duration::seconds(30),
                )
                .await?
        );
        assert!(!sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM rustodon.durable_jobs
               WHERE kind = $1 AND arguments ->> 'account_id' = $2)",
        )
        .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
        .bind(ACCOUNT.to_string())
        .fetch_one(&runtime_pool)
        .await?);

        let remaining_statuses: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM statuses WHERE account_id = $1 ORDER BY id")
                .bind(ACCOUNT)
                .fetch_all(&writer_pool)
                .await?;
        assert_eq!(remaining_statuses, vec![REPORTED_STATUS]);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT quotes_count FROM status_stats WHERE status_id = $1",
            )
            .bind(other_status_id)
            .fetch_one(&writer_pool)
            .await?,
            0,
            "purging an accepted quote must decrement the quoted status counter"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM quotes WHERE id = $1")
                .bind(QUOTE)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "purging the quoted source status must remove its accepted quote row"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM reports WHERE id = $1")
                .bind(REPORT)
                .fetch_one(&writer_pool)
                .await?,
            1
        );
        let (disabled, email): (bool, String) =
            sqlx::query_as("SELECT disabled, email FROM users WHERE account_id = $1")
                .bind(ACCOUNT)
                .fetch_one(&writer_pool)
                .await?;
        assert!(disabled);
        assert_eq!(email, EMAIL);
        let (username, private_key, display_name, note, fields, avatar_file_name, moved_to): (
            String,
            Option<String>,
            String,
            String,
            Value,
            Option<String>,
            Option<i64>,
        ) = sqlx::query_as(
            "SELECT username, private_key, display_name, note, fields, avatar_file_name,
                    moved_to_account_id
               FROM accounts WHERE id = $1",
        )
        .bind(ACCOUNT)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(username, USERNAME);
        assert_eq!(private_key.as_deref(), Some(retained_private_key.as_str()));
        assert_eq!(display_name, "");
        assert_eq!(note, "");
        assert_eq!(fields, json!([]));
        assert_eq!(avatar_file_name, None);
        assert_eq!(moved_to, None);
        for metadata in &profile_media_metadata {
            if let Some(path) = metadata.relative_path("original") {
                assert!(
                    media_root.open_file(Path::new(&path)).is_err(),
                    "account purge left Paperclip file {path}"
                );
            }
        }
        assert!(
            media_root
                .open_file(Path::new(
                    &reported_media_metadata
                        .relative_path("original")
                        .expect("reported media path"),
                ))
                .is_ok(),
            "account purge removed reported media"
        );
        let stats: (i64, i64, i64) = sqlx::query_as(
            "SELECT statuses_count, following_count, followers_count
               FROM account_stats WHERE account_id = $1",
        )
        .bind(ACCOUNT)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(stats, (0, 0, 0));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM bookmarks WHERE account_id = $1")
                .bind(ACCOUNT)
                .fetch_one(&writer_pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM status_pins WHERE account_id = $1")
                .bind(ACCOUNT)
                .fetch_one(&writer_pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM polls WHERE account_id = $1")
                .bind(ACCOUNT)
                .fetch_one(&writer_pool)
                .await?,
            1
        );
        let (protected_tallies, protected_voters_total, protected_ballots):
            (Vec<i64>, Option<i64>, i64) = sqlx::query_as(
                "SELECT cached_tallies, voters_count, votes_count
                   FROM polls WHERE id = $1",
            )
            .bind(PROTECTED_POLL)
            .fetch_one(&writer_pool)
            .await?;
        assert_eq!(protected_tallies, vec![1]);
        assert_eq!(protected_voters_total, Some(1));
        assert_eq!(protected_ballots, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM poll_votes WHERE poll_id = $1")
                .bind(PROTECTED_POLL)
                .fetch_one(&writer_pool)
                .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mentions WHERE account_id = $1")
                .bind(ACCOUNT)
                .fetch_one(&writer_pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM favourites WHERE account_id = $1")
                .bind(ACCOUNT)
                .fetch_one(&writer_pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT favourites_count FROM status_stats WHERE status_id = $1"
            )
            .bind(other_status_id)
            .fetch_one(&writer_pool)
            .await?,
            other_favourites_count
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT followers_count FROM account_stats WHERE account_id = $1"
            )
            .bind(OTHER_ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            other_followers_count
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_deletion_requests WHERE account_id = $1",
            )
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs
                  WHERE kind = $1 AND arguments ->> 'account_id' = $2",
            )
            .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
            .bind(ACCOUNT.to_string())
            .fetch_one(&runtime_pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs
                  WHERE kind = $1 AND arguments ->> 'account_id' = $2",
            )
            .bind(ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND)
            .bind(ACCOUNT.to_string())
            .fetch_one(&runtime_pool)
            .await?,
            1
        );
        assert!(
            executor
                .process_one(
                    "account-delete-worker",
                    &[Lane::Push],
                    Duration::seconds(30)
                )
                .await?
        );
        assert!(queue.dispatch_outbox(100).await? > 0);
        assert!(
            executor
                .process_one(
                    "account-delete-delivery-worker",
                    &[Lane::Push],
                    Duration::seconds(30)
                )
                .await?
        );
        let request = server.await??;
        let request_text = String::from_utf8_lossy(&request);
        let request_text_lower = request_text.to_ascii_lowercase();
        assert!(request_text.starts_with("POST "));
        assert!(request_text_lower.contains("content-type: application/activity+json"));
        assert!(request_text_lower.contains("digest: sha-256="));
        assert!(request_text_lower.contains("signature: "));
        assert!(request_text.contains("\"type\":\"Delete\""));
        assert!(request_text.contains(&actor_uri));

        sqlx::query(
            "INSERT INTO statuses (id, account_id, created_at, local, text, updated_at,
                uri, url)
             VALUES ($1, $2, clock_timestamp(), true, 'rollback status', clock_timestamp(),
                 $3, $3)",
        )
        .bind(ROLLBACK_STATUS)
        .bind(ACCOUNT)
        .bind(format!("{actor_uri}/statuses/{ROLLBACK_STATUS}"))
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO status_stats (id, status_id, created_at, updated_at)
             VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
        )
        .bind(ROLLBACK_STATUS_STATS)
        .bind(ROLLBACK_STATUS)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO conversations (
                 id, created_at, parent_account_id, parent_status_id, updated_at, uri)
             VALUES ($1, clock_timestamp(), $2, $3, clock_timestamp(), $4)",
        )
        .bind(ROLLBACK_CONVERSATION)
        .bind(ACCOUNT)
        .bind(ROLLBACK_STATUS)
        .bind(format!("{actor_uri}/conversations/{ROLLBACK_CONVERSATION}"))
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO account_conversations (
                 account_id, conversation_id, last_status_id, lock_version,
                 participant_account_ids, status_ids)
             VALUES ($1, $2, $3, 2147483647, ARRAY[$1, $4]::bigint[], ARRAY[$3, $5]::bigint[])",
        )
        .bind(ACCOUNT)
        .bind(ROLLBACK_CONVERSATION)
        .bind(ROLLBACK_STATUS)
        .bind(OTHER_ACCOUNT)
        .bind(REPORTED_STATUS)
        .execute(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO account_deletion_requests (account_id, created_at, updated_at)
             VALUES ($1, clock_timestamp() - interval '31 days', clock_timestamp())",
        )
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
        let rollback_key = format!("mastodon:account:{ACCOUNT}:purge:rollback");
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Maintenance,
                    MASTODON_ACCOUNT_PURGE_JOB_KIND,
                    json!({"account_id": ACCOUNT}),
                )
                .logical_key(rollback_key.clone()),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "account-purge-rollback",
                    &[Lane::Maintenance],
                    Duration::seconds(30),
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE id = $1")
                .bind(ROLLBACK_STATUS)
                .fetch_one(&writer_pool)
                .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_conversations WHERE account_id = $1",
            )
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_deletion_requests WHERE account_id = $1",
            )
            .bind(ACCOUNT)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        let rollback_job: (i32, Option<String>) = sqlx::query_as(
            "SELECT attempts, last_error FROM rustodon.durable_jobs WHERE logical_key = $1",
        )
        .bind(&rollback_key)
        .fetch_one(&runtime_pool)
        .await?;
        assert_eq!(
            rollback_job,
            (1, Some("account purge failed".to_owned()))
        );
        queue
            .enqueue(&JobSpec::new(
                Lane::Maintenance,
                MASTODON_ACCOUNT_PURGE_JOB_KIND,
                json!({"account_id": ACCOUNT}),
            ))
            .await?;
        assert!(
            executor
                .process_one(
                    "account-purge-retry",
                    &[Lane::Maintenance],
                    Duration::seconds(30)
                )
                .await?
        );
        drop(executor);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(ROLLBACK_CONVERSATION)
        .execute(&writer_pool)
        .await?;
    drop(media_root);
    fs::remove_dir_all(&media_root_path)?;
    reset().await?;
    operation?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn activitypub_delivery_posts_signed_json_through_the_durable_worker()
-> Result<(), Box<dyn std::error::Error>> {
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const INBOX_HOST: &str = "alternate-inbox.remote.fixture.invalid";
    const REMOTE_DOMAIN: &str = "remote.fixture.invalid";
    const STATUS_ID: i64 = 116_844_842_188_805_001;
    const SOURCE_ACCOUNT_ID: i64 = 116_844_606_259_201_001;

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let server = tokio::spawn(fixture_delivery_server(listener));
    let inbox_url = format!("http://{INBOX_HOST}:{}{}", endpoint.port(), "/inbox");
    let remote_domain = format!("{REMOTE_DOMAIN}:{}", endpoint.port());
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: None,
        limited_federation: false,
        remote_media_endpoint: None,
        remote_delivery_endpoint: Some(endpoint),
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_DELIVERY_JOB_KIND,
                json!({
                    "status_id": STATUS_ID,
                    "source_account_id": SOURCE_ACCOUNT_ID,
                    "inbox_url": inbox_url.clone(),
                    "remote_domain": remote_domain.clone(),
                    "body": {
                        "@context": "https://www.w3.org/ns/activitystreams",
                        "id": "https://fixture-v4-6-5.rustodon.invalid/activities/worker-delivery",
                        "type": "Create",
                        "actor": "https://fixture-v4-6-5.rustodon.invalid/users/alice",
                        "object": "https://fixture-v4-6-5.rustodon.invalid/users/alice/statuses/116844842188805001"
                    }
                }),
            )
            .logical_key("activitypub:test-signed-delivery"),
        )
        .await?;
    assert!(
        executor
            .process_one("delivery-worker", &[Lane::Push], Duration::seconds(30))
            .await?
    );
    let request = server.await??;
    let request_text = String::from_utf8_lossy(&request);
    let request_text_lower = request_text.to_ascii_lowercase();
    assert!(request_text.starts_with("POST /inbox HTTP/1.1\r\n"));
    assert!(request_text_lower.contains("host: alternate-inbox.remote.fixture.invalid:"));
    assert!(request_text_lower.contains("content-type: application/activity+json"));
    assert!(request_text_lower.contains("digest: sha-256="));
    assert!(request_text_lower.contains("signature: "));
    assert!(request_text.contains("/activities/worker-delivery"));
    assert!(request_text.contains("\"type\":\"Create\""));
    drop(executor);
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_delivery_retries_after_transient_inbox_failure()
-> Result<(), Box<dyn std::error::Error>> {
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const INBOX_HOST: &str = "remote.fixture.invalid";
    const STATUS_ID: i64 = 116_844_842_188_805_001;
    const SOURCE_ACCOUNT_ID: i64 = 116_844_606_259_201_001;

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let server = tokio::spawn(fixture_retry_delivery_server(listener));
    let inbox_url = format!("http://{INBOX_HOST}:{}{}", endpoint.port(), "/inbox");
    let remote_domain = format!("{INBOX_HOST}:{}", endpoint.port());
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: None,
        limited_federation: false,
        remote_media_endpoint: None,
        remote_delivery_endpoint: Some(endpoint),
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_DELIVERY_JOB_KIND,
                json!({
                    "status_id": STATUS_ID,
                    "source_account_id": SOURCE_ACCOUNT_ID,
                    "inbox_url": inbox_url,
                    "remote_domain": remote_domain,
                    "body": {
                        "@context": "https://www.w3.org/ns/activitystreams",
                        "id": "https://fixture-v4-6-5.rustodon.invalid/activities/worker-retry",
                        "type": "Create",
                        "actor": "https://fixture-v4-6-5.rustodon.invalid/users/alice",
                        "object": "https://fixture-v4-6-5.rustodon.invalid/users/alice/statuses/116844842188805001"
                    }
                }),
            )
            .logical_key("activitypub:test-transient-delivery"),
        )
        .await?;
    assert!(
        executor
            .process_one("delivery-first", &[Lane::Push], Duration::seconds(30))
            .await?
    );
    assert_eq!(
        sqlx::query_as::<_, (i32, Option<String>)>(
            "SELECT attempts, last_error FROM rustodon.durable_jobs \
               WHERE logical_key = 'activitypub:test-transient-delivery'",
        )
        .fetch_one(&runtime_pool)
        .await?,
        (1, Some("remote delivery failed".to_owned()))
    );
    sqlx::query(
        "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() \
          WHERE logical_key = 'activitypub:test-transient-delivery'",
    )
    .execute(&runtime_pool)
    .await?;
    sqlx::query(
        "UPDATE rustodon.domain_health SET retry_at = clock_timestamp() \
          WHERE domain = $1",
    )
    .bind(INBOX_HOST)
    .execute(&runtime_pool)
    .await?;
    assert!(
        executor
            .process_one("delivery-second", &[Lane::Push], Duration::seconds(30))
            .await?
    );
    let requests = tokio::time::timeout(std::time::Duration::from_secs(2), server).await???;
    assert_eq!(requests.len(), 2);
    assert!(String::from_utf8_lossy(&requests[0]).contains("/activities/worker-retry"));
    assert!(String::from_utf8_lossy(&requests[1]).contains("/activities/worker-retry"));
    assert_eq!(queue.queued_count().await?, 0);
    assert_eq!(
        sqlx::query_as::<_, (i32, Option<chrono::DateTime<Utc>>)>(
            "SELECT failures, retry_at FROM rustodon.domain_health WHERE domain = $1",
        )
        .bind(INBOX_HOST)
        .fetch_one(&runtime_pool)
        .await?,
        (0, None)
    );
    drop(executor);
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_delivery_replays_after_worker_crash_before_ack()
-> Result<(), Box<dyn std::error::Error>> {
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const INBOX_HOST: &str = "remote.fixture.invalid";
    const STATUS_ID: i64 = 116_844_842_188_805_001;
    const SOURCE_ACCOUNT_ID: i64 = 116_844_606_259_201_001;

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let accepted = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let server = tokio::spawn(fixture_crashing_delivery_server(
        listener,
        Arc::clone(&accepted),
        Arc::clone(&release),
    ));
    let inbox_url = format!("http://{INBOX_HOST}:{}{}", endpoint.port(), "/inbox");
    let remote_domain = format!("{INBOX_HOST}:{}", endpoint.port());
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: None,
        limited_federation: false,
        remote_media_endpoint: None,
        remote_delivery_endpoint: Some(endpoint),
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let create_body = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": "https://fixture-v4-6-5.rustodon.invalid/activities/worker-crash-create",
        "type": "Create",
        "actor": "https://fixture-v4-6-5.rustodon.invalid/users/alice",
        "object": "https://fixture-v4-6-5.rustodon.invalid/users/alice/statuses/116844842188805001"
    });
    let delete_body = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": "https://fixture-v4-6-5.rustodon.invalid/activities/worker-crash-delete",
        "type": "Delete",
        "actor": "https://fixture-v4-6-5.rustodon.invalid/users/alice",
        "object": {
            "id": "https://fixture-v4-6-5.rustodon.invalid/users/alice/statuses/116844842188805001",
            "type": "Tombstone"
        }
    });
    let mut transaction = runtime_pool.begin().await?;
    for (logical_key, body) in [
        ("activitypub:test-crash-create", create_body),
        ("activitypub:test-crash-delete", delete_body),
    ] {
        record_outbox_once_in(
            &mut transaction,
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_DELIVERY_JOB_KIND,
                json!({
                    "status_id": STATUS_ID,
                    "source_account_id": SOURCE_ACCOUNT_ID,
                    "inbox_url": inbox_url,
                    "remote_domain": remote_domain,
                    "body": body,
                }),
            )
            .logical_key(logical_key),
        )
        .await?;
    }
    transaction.commit().await?;
    assert_eq!(queue.dispatch_outbox(10).await?, 1);

    let first_executor = executor.clone();
    let first_worker = tokio::spawn(async move {
        first_executor
            .process_one(
                "crashed-delivery",
                &[Lane::Push],
                Duration::milliseconds(100),
            )
            .await
    });
    accepted.notified().await;
    assert_eq!(queue.dispatch_outbox(10).await?, 1);
    assert_eq!(queue.queued_count().await?, 2);
    first_worker.abort();
    assert!(first_worker.await.is_err());
    release.notify_one();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    assert!(
        executor
            .process_one("recovered-delivery", &[Lane::Push], Duration::seconds(1))
            .await?
    );
    assert_eq!(queue.queued_count().await?, 1);
    assert!(
        executor
            .process_one("successor-delivery", &[Lane::Push], Duration::seconds(1))
            .await?
    );
    let requests = tokio::time::timeout(std::time::Duration::from_secs(3), server).await???;
    assert_eq!(requests.len(), 3);
    let request_texts = requests
        .iter()
        .map(|request| String::from_utf8_lossy(request).into_owned())
        .collect::<Vec<_>>();
    assert!(request_texts[0].contains("worker-crash-create"));
    assert!(request_texts[1].contains("worker-crash-create"));
    assert!(request_texts[2].contains("worker-crash-delete"));
    assert_eq!(queue.queued_count().await?, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i32>(
            "SELECT failures FROM rustodon.domain_health WHERE domain = $1",
        )
        .bind(INBOX_HOST)
        .fetch_one(&runtime_pool)
        .await?,
        0
    );
    drop(executor);
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_relationship_delivery_keeps_undo_after_live_follow()
-> Result<(), Box<dyn std::error::Error>> {
    const ALICE: i64 = 116_844_606_259_201_001;
    const REMOTE_ACCOUNT: i64 = -331;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    let original_inboxes: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT inbox_url, shared_inbox_url FROM accounts WHERE id = $1")
            .bind(REMOTE_ACCOUNT)
            .fetch_one(&writer_pool)
            .await?;

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let accepted = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut server = tokio::spawn(fixture_ordered_relationship_delivery_server(
        listener,
        Arc::clone(&accepted),
        Arc::clone(&release),
    ));
    sqlx::query("UPDATE accounts SET inbox_url = $2, shared_inbox_url = '' WHERE id = $1")
        .bind(REMOTE_ACCOUNT)
        .bind(format!(
            "http://remote.fixture.invalid:{}/inbox",
            endpoint.port()
        ))
        .execute(&writer_pool)
        .await?;

    let writer = WriteRepository::connect(&owner_url).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-follow-v4-6-5"),
    );
    let authenticated = BearerAuthenticator::new(Repository::connect(&owner_url).await?)
        .authenticate(&headers, WRITE_FOLLOWS)
        .await?;
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: None,
        limited_federation: false,
        remote_media_endpoint: None,
        remote_delivery_endpoint: Some(endpoint),
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let mut first_worker = None;
    let operation = async {
        let followed = writer
            .set_follow_with_origin(
                &authenticated,
                REMOTE_ACCOUNT,
                true,
                None,
                None,
                None,
                Some(ORIGIN),
                false,
            )
            .await?;
        let follow_uri = followed
            .activity_uri
            .ok_or("remote Follow did not receive an activity URI")?;
        assert!(queue.dispatch_outbox(100).await? >= 1);
        let first_executor = executor.clone();
        first_worker = Some(tokio::spawn(async move {
            first_executor
                .process_one(
                    "relationship-follow-worker",
                    &[Lane::Push],
                    Duration::seconds(30),
                )
                .await
        }));
        accepted.notified().await;

        let undone = writer
            .set_follow_with_origin(
                &authenticated,
                REMOTE_ACCOUNT,
                false,
                None,
                None,
                None,
                Some(ORIGIN),
                false,
            )
            .await?;
        assert_eq!(undone.activity_uri.as_deref(), Some(follow_uri.as_str()));
        assert!(queue.dispatch_outbox(100).await? >= 1);
        assert!(
            queue
                .claim(
                    "relationship-successor-probe",
                    &[Lane::Push],
                    Duration::seconds(1),
                )
                .await?
                .is_none(),
            "a live Follow delivery must fence its Undo successor"
        );

        release.notify_one();
        let first_worker = first_worker
            .take()
            .expect("the Follow worker was started")
            .await??;
        assert!(first_worker);
        assert!(
            executor
                .process_one(
                    "relationship-undo-worker",
                    &[Lane::Push],
                    Duration::seconds(30)
                )
                .await?
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    release.notify_one();
    if let Some(worker) = first_worker.take() {
        worker.abort();
        let _ = worker.await;
    }
    let requests = if let Ok(Ok(Ok(requests))) =
        tokio::time::timeout(std::time::Duration::from_secs(3), &mut server).await
    {
        Some(requests)
    } else {
        server.abort();
        let _ = server.await;
        None
    };
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE accounts SET inbox_url = $2, shared_inbox_url = $3 WHERE id = $1")
        .bind(REMOTE_ACCOUNT)
        .bind(original_inboxes.0)
        .bind(original_inboxes.1)
        .execute(&writer_pool)
        .await?;
    reset().await?;
    operation?;
    let requests = requests.ok_or("ordered relationship delivery server did not finish")?;
    assert_eq!(requests.len(), 2);
    assert!(String::from_utf8_lossy(&requests[0]).contains("\"type\":\"Follow\""));
    assert!(String::from_utf8_lossy(&requests[1]).contains("\"type\":\"Undo\""));
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_relationship_delivery_keeps_undo_after_live_block()
-> Result<(), Box<dyn std::error::Error>> {
    const ALICE: i64 = 116_844_606_259_201_001;
    const REMOTE_ACCOUNT: i64 = -331;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    let original_inboxes: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT inbox_url, shared_inbox_url FROM accounts WHERE id = $1")
            .bind(REMOTE_ACCOUNT)
            .fetch_one(&writer_pool)
            .await?;

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let accepted = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut server = tokio::spawn(fixture_ordered_relationship_delivery_server(
        listener,
        Arc::clone(&accepted),
        Arc::clone(&release),
    ));
    sqlx::query("UPDATE accounts SET inbox_url = $2, shared_inbox_url = '' WHERE id = $1")
        .bind(REMOTE_ACCOUNT)
        .bind(format!(
            "http://remote.fixture.invalid:{}/inbox",
            endpoint.port()
        ))
        .execute(&writer_pool)
        .await?;

    let writer = WriteRepository::connect(&owner_url).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-follow-v4-6-5"),
    );
    let authenticated = BearerAuthenticator::new(Repository::connect(&owner_url).await?)
        .authenticate(&headers, WRITE_BLOCKS)
        .await?;
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: None,
        limited_federation: false,
        remote_media_endpoint: None,
        remote_delivery_endpoint: Some(endpoint),
        remote_fetch_endpoint: None,
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let mut first_worker = None;
    let operation = async {
        writer
            .set_block_with_origin(&authenticated, REMOTE_ACCOUNT, true, Some(ORIGIN))
            .await?;
        let block_uri = sqlx::query_scalar::<_, String>(
            "SELECT uri FROM blocks WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .fetch_one(&writer_pool)
        .await?;
        assert!(queue.dispatch_outbox(100).await? >= 1);
        let first_executor = executor.clone();
        first_worker = Some(tokio::spawn(async move {
            first_executor
                .process_one(
                    "relationship-block-worker",
                    &[Lane::Push],
                    Duration::seconds(30),
                )
                .await
        }));
        accepted.notified().await;

        writer
            .set_block_with_origin(&authenticated, REMOTE_ACCOUNT, false, Some(ORIGIN))
            .await?;
        assert!(queue.dispatch_outbox(100).await? >= 1);
        assert!(
            queue
                .claim(
                    "relationship-block-successor-probe",
                    &[Lane::Push],
                    Duration::seconds(1),
                )
                .await?
                .is_none(),
            "a live Block delivery must fence its Undo successor"
        );

        release.notify_one();
        let first_worker = first_worker
            .take()
            .expect("the Block worker was started")
            .await??;
        assert!(first_worker);
        assert!(
            executor
                .process_one(
                    "relationship-block-undo-worker",
                    &[Lane::Push],
                    Duration::seconds(30),
                )
                .await?
        );
        assert!(block_uri.starts_with("https://fixture-v4-6-5.rustodon.invalid/payloads/block-"));
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    release.notify_one();
    if let Some(worker) = first_worker.take() {
        worker.abort();
        let _ = worker.await;
    }
    let requests = if let Ok(Ok(Ok(requests))) =
        tokio::time::timeout(std::time::Duration::from_secs(3), &mut server).await
    {
        Some(requests)
    } else {
        server.abort();
        let _ = server.await;
        None
    };
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE accounts SET inbox_url = $2, shared_inbox_url = $3 WHERE id = $1")
        .bind(REMOTE_ACCOUNT)
        .bind(original_inboxes.0)
        .bind(original_inboxes.1)
        .execute(&writer_pool)
        .await?;
    reset().await?;
    operation?;
    let requests = requests.ok_or("ordered relationship delivery server did not finish")?;
    assert_eq!(requests.len(), 2);
    assert!(String::from_utf8_lossy(&requests[0]).contains("\"type\":\"Block\""));
    assert!(String::from_utf8_lossy(&requests[1]).contains("\"type\":\"Undo\""));
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_reply_resolution_repairs_child_after_parent_arrives()
-> Result<(), Box<dyn std::error::Error>> {
    const BOB: i64 = 116_844_606_259_202_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
    const KEY_ID: &str = "https://remote.fixture.invalid/users/bob#secondary-key";
    const PARENT_URI: &str = "https://remote.fixture.invalid/users/bob/statuses/rustodon-parent";
    const CHILD_URI: &str = "https://remote.fixture.invalid/users/bob/statuses/rustodon-child";
    const LOCAL_CHILD_URI: &str =
        "https://remote.fixture.invalid/users/bob/statuses/rustodon-local-child";
    const LOCAL_CHILD_UPDATE_ACTIVITY_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-local-child-update";
    const LOCAL_CHILD_DELETE_ACTIVITY_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-local-child-delete";
    const SHARED_NOTE_URI: &str =
        "https://remote.fixture.invalid/users/bob/statuses/rustodon-shared-note";
    const SHARED_NOTE_ACTIVITY_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-shared-note-create";
    const SHARED_NOTE_DELETE_ACTIVITY_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-shared-note-delete";
    const QUOTED_NOTE_URI: &str =
        "https://remote.fixture.invalid/users/bob/statuses/rustodon-quoted-note";
    const QUOTED_NOTE_ACTIVITY_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-quoted-note-create";
    const LOCAL_QUOTE_ID: i64 = -99004;
    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let baseline_statuses_count: i64 =
        sqlx::query_scalar("SELECT statuses_count FROM account_stats WHERE account_id = $1")
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?;
    let (local_username, local_id_scheme) = sqlx::query_as::<_, (String, Option<i32>)>(
        "SELECT username, id_scheme FROM accounts WHERE id = $1",
    )
    .bind(MODERATOR)
    .fetch_one(&writer_pool)
    .await?;
    let local_parent_id: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (
            account_id, text, spoiler_text, visibility, local, sensitive, reply,
            created_at, updated_at)
         VALUES ($1, 'local reply target', '', 0, true, false, false,
                 clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(MODERATOR)
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at)
         VALUES ($1, clock_timestamp(), clock_timestamp())",
    )
    .bind(local_parent_id)
    .execute(&writer_pool)
    .await?;
    let previous_parent_follower: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(follow) FROM follows follow
          WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(-320_i64)
    .bind(MODERATOR)
    .fetch_optional(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(-320_i64)
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;
    sqlx::query(
        "INSERT INTO follows
             (account_id, target_account_id, show_reblogs, notify, languages, uri,
              created_at, updated_at)
         VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(-320_i64)
    .bind(MODERATOR)
    .bind("https://account-blocked.fixture.invalid/users/domain_viewer#follows/rustodon-parent")
    .execute(&writer_pool)
    .await?;
    let local_parent_uri = if local_id_scheme == Some(1) {
        format!(
            "{}/ap/users/{MODERATOR}/statuses/{local_parent_id}",
            ORIGIN.trim_end_matches('/')
        )
    } else {
        format!(
            "{}/users/{local_username}/statuses/{local_parent_id}",
            ORIGIN.trim_end_matches('/')
        )
    };
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
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
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let private_reply_status_id: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (
             account_id, text, spoiler_text, visibility, local, sensitive, reply,
             in_reply_to_id, in_reply_to_account_id, created_at, updated_at, deleted_at)
         VALUES ($1, 'deleted direct reply', '', 3, true, false, true, $2, $1,
                 clock_timestamp(), clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .bind(MODERATOR)
    .bind(local_parent_id)
    .fetch_one(&writer_pool)
    .await?;
    let mut transaction = writer_pool.begin().await?;
    record_outbox_once_in(
        &mut transaction,
        &JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
            json!({
                "status_id": private_reply_status_id,
                "activity_type": "Delete"
            }),
        )
        .logical_key(format!(
            "activitypub:test-private-reply-delete:{private_reply_status_id}"
        )),
    )
    .await?;
    transaction.commit().await?;
    assert_eq!(queue.dispatch_outbox(10).await?, 1);
    assert!(
        executor
            .process_one("private-reply-delete", &[Lane::Push], Duration::seconds(30),)
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload -> 'arguments' ->> 'status_id' = $2
                AND payload -> 'arguments' ->> 'inbox_url' = $3",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(private_reply_status_id.to_string())
        .bind("https://account-blocked.fixture.invalid/inbox")
        .fetch_one(&writer_pool)
        .await?,
        0,
        "a deleted direct reply must not reach the parent's remote followers",
    );
    let child_body = json!({
        "id": "https://remote.fixture.invalid/activities/rustodon-child-create",
        "type": "Create",
        "actor": ACTOR,
        "object": {
            "id": CHILD_URI,
            "type": "Note",
            "attributedTo": ACTOR,
            "published": "2026-08-25T12:01:00Z",
            "inReplyTo": PARENT_URI,
            "content": "<p>Child delivered first</p>",
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [],
            "tag": [],
            "attachment": []
        }
    })
    .to_string();
    let parent_body = json!({
        "id": "https://remote.fixture.invalid/activities/rustodon-parent-create",
        "type": "Create",
        "actor": ACTOR,
        "object": {
            "id": PARENT_URI,
            "type": "Note",
            "attributedTo": ACTOR,
            "published": "2026-08-25T12:00:00Z",
            "content": "<p>Parent delivered second</p>",
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [],
            "tag": [],
            "attachment": []
        }
    })
    .to_string();
    let result = async {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": child_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-reply-child"),
            )
            .await?;
        assert!(
            executor
                .process_one("reply-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        let child_before = sqlx::query_as::<_, (i64, bool, Option<i64>, Option<i64>, Option<i64>)>(
            "SELECT id, reply, in_reply_to_id, in_reply_to_account_id, conversation_id
               FROM statuses WHERE uri = $1",
        )
        .bind(CHILD_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert!(child_before.1);
        assert_eq!(child_before.2, None);
        assert_eq!(child_before.3, None);
        assert!(child_before.4.is_some());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND logical_key = $2",
            )
            .bind(ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND)
            .bind(format!("activitypub:thread:{}", child_before.0))
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(queue.dispatch_outbox(10).await?, 1);
        assert_eq!(queue.dispatch_outbox(10).await?, 0);

        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": parent_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-reply-parent"),
            )
            .await?;
        assert!(
            executor
                .process_one("reply-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert!(
            executor
                .process_one("reply-worker", &[Lane::Pull], Duration::seconds(30))
                .await?
        );
        assert!(
            !executor
                .process_one("reply-worker", &[Lane::Pull], Duration::seconds(30))
                .await?
        );
        let parent_id: i64 = sqlx::query_scalar("SELECT id FROM statuses WHERE uri = $1")
            .bind(PARENT_URI)
            .fetch_one(&writer_pool)
            .await?;
        let child_after = sqlx::query_as::<_, (bool, Option<i64>, Option<i64>, Option<i64>)>(
            "SELECT reply, in_reply_to_id, in_reply_to_account_id, conversation_id
               FROM statuses WHERE uri = $1",
        )
        .bind(CHILD_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert!(child_after.0);
        assert_eq!(child_after.1, Some(parent_id));
        assert_eq!(child_after.2, Some(BOB));
        assert_eq!(child_after.3, child_before.4);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT replies_count FROM status_stats WHERE status_id = $1",
            )
            .bind(parent_id)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1",
            )
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            baseline_statuses_count + 2
        );

        let local_child_body = json!({
            "id": "https://remote.fixture.invalid/activities/rustodon-local-child-create",
            "type": "Create",
            "actor": ACTOR,
            "signature": {
                "type": "RsaSignature2017",
                "creator": KEY_ID,
                "created": "2026-08-25T12:02:00Z",
                "signatureValue": "fixture"
            },
            "object": {
                "id": LOCAL_CHILD_URI,
                "type": "Note",
                "attributedTo": ACTOR,
                "published": "2026-08-25T12:02:00Z",
                "inReplyTo": local_parent_uri,
                "content": "<p>Reply to a local computed URI</p>",
                "to": ["https://www.w3.org/ns/activitystreams#Public"],
                "cc": [],
                "tag": [],
                "attachment": []
            }
        })
        .to_string();
        let local_child_update_body = json!({
            "id": LOCAL_CHILD_UPDATE_ACTIVITY_URI,
            "type": "Update",
            "actor": ACTOR,
            "signature": {
                "type": "RsaSignature2017",
                "creator": KEY_ID,
                "created": "2026-08-25T12:04:00Z",
                "signatureValue": "fixture"
            },
            "object": {
                "id": LOCAL_CHILD_URI,
                "type": "Note",
                "attributedTo": ACTOR,
                "published": "2026-08-25T12:02:00Z",
                "updated": "2026-08-25T12:04:00Z",
                "inReplyTo": local_parent_uri,
                "content": "<p>Updated reply to a local computed URI</p>",
                "to": ["https://www.w3.org/ns/activitystreams#Public"],
                "cc": [],
                "tag": [],
                "attachment": []
            }
        })
        .to_string();
        let local_child_delete_body = json!({
            "id": LOCAL_CHILD_DELETE_ACTIVITY_URI,
            "type": "Delete",
            "actor": ACTOR,
            "signature": {
                "type": "RsaSignature2017",
                "creator": KEY_ID,
                "created": "2026-08-25T12:05:00Z",
                "signatureValue": "fixture"
            },
            "object": {
                "id": LOCAL_CHILD_URI,
                "type": "Tombstone"
            }
        })
        .to_string();
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": local_child_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-reply-local-child"),
            )
            .await?;
        assert!(
            executor
                .process_one("reply-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        let local_child = sqlx::query_as::<_, (i64, Option<i64>, Option<i64>)>(
            "SELECT id, in_reply_to_id, in_reply_to_account_id
               FROM statuses WHERE uri = $1",
        )
        .bind(LOCAL_CHILD_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(local_child.1, Some(local_parent_id));
        assert_eq!(local_child.2, Some(MODERATOR));
        let local_child_activity: Value = serde_json::from_str(&local_child_body)?;
        assert!(local_child_activity.get("signature").is_some());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload -> 'arguments' ->> 'child_status_id' = $2",
            )
            .bind(ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND)
            .bind(local_child.0.to_string())
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = 'rustodon.activitypub.deliver'
                    AND payload -> 'arguments' ->> 'source_account_id' = $1
                    AND payload -> 'arguments' ->> 'inbox_url' = $2
                    AND payload -> 'arguments' -> 'body' ->> 'id' = $3",
            )
            .bind(MODERATOR.to_string())
            .bind("https://account-blocked.fixture.invalid/inbox")
            .bind("https://remote.fixture.invalid/activities/rustodon-local-child-create")
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a signed reply to a local status must be forwarded to its remote followers"
        );
        let forwarding_key = activitypub::forward_delivery_logical_key(
            MODERATOR,
            "https://remote.fixture.invalid/activities/rustodon-local-child-create",
            "https://account-blocked.fixture.invalid/inbox",
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND logical_key = $2 AND dispatched_at IS NULL",
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .bind(&forwarding_key)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(queue.dispatch_outbox(100).await?, 1);
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": local_child_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-reply-local-child-duplicate"),
            )
            .await?;
        assert!(
            executor
                .process_one("reply-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND logical_key = $2 AND dispatched_at IS NULL",
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .bind(&forwarding_key)
            .fetch_one(&writer_pool)
            .await?,
            0,
            "replaying a signed Create must not reset a dispatched forwarding delivery"
        );
        for (logical_key, body, activity_uri) in [
            (
                "activitypub:test-reply-local-child-update",
                local_child_update_body,
                LOCAL_CHILD_UPDATE_ACTIVITY_URI,
            ),
            (
                "activitypub:test-reply-local-child-delete",
                local_child_delete_body,
                LOCAL_CHILD_DELETE_ACTIVITY_URI,
            ),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": body,
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid",
                            "delivery_target_account_id": MODERATOR
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one("reply-worker", &[Lane::Ingress], Duration::seconds(30))
                    .await?
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM rustodon.outbox_events
                      WHERE kind = 'rustodon.activitypub.deliver'
                        AND payload -> 'arguments' ->> 'source_account_id' = $1
                        AND payload -> 'arguments' ->> 'inbox_url' = $2
                        AND payload -> 'arguments' -> 'body' ->> 'id' = $3",
                )
                .bind(MODERATOR.to_string())
                .bind("https://account-blocked.fixture.invalid/inbox")
                .bind(activity_uri)
                .fetch_one(&writer_pool)
                .await?,
                1,
                "signed remote status lifecycle activities must be forwarded"
            );
        }

        let shared_note_body = json!({
            "id": SHARED_NOTE_ACTIVITY_URI,
            "type": "Create",
            "actor": ACTOR,
            "signature": {
                "type": "RsaSignature2017",
                "creator": KEY_ID,
                "created": "2026-08-25T12:03:00Z",
                "signatureValue": "fixture"
            },
            "object": {
                "id": SHARED_NOTE_URI,
                "type": "Note",
                "attributedTo": ACTOR,
                "published": "2026-08-25T12:03:00Z",
                "content": "<p>Shared by a local reblog</p>",
                "to": ["https://www.w3.org/ns/activitystreams#Public"],
                "cc": [],
                "tag": [],
                "attachment": []
            }
        })
        .to_string();
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": shared_note_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-shared-remote-note"),
            )
            .await?;
        assert!(
            executor
                .process_one("reply-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        let shared_status_id: i64 = sqlx::query_scalar("SELECT id FROM statuses WHERE uri = $1")
            .bind(SHARED_NOTE_URI)
            .fetch_one(&writer_pool)
            .await?;
        let shared_boost_id: i64 = sqlx::query_scalar(
            "INSERT INTO statuses (
                 account_id, text, spoiler_text, visibility, local, sensitive, reply,
                 reblog_of_id, created_at, updated_at)
             VALUES ($1, '', '', 0, true, false, false, $2,
                     clock_timestamp(), clock_timestamp())
             RETURNING id",
        )
        .bind(MODERATOR)
        .bind(shared_status_id)
        .fetch_one(&writer_pool)
        .await?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": shared_note_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-shared-remote-note-replay"),
            )
            .await?;
        assert!(
            executor
                .process_one("reply-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = 'rustodon.activitypub.deliver'
                    AND payload -> 'arguments' ->> 'source_account_id' = $1
                    AND payload -> 'arguments' ->> 'inbox_url' = $2
                    AND payload -> 'arguments' -> 'body' ->> 'id' = $3",
            )
            .bind(MODERATOR.to_string())
            .bind("https://account-blocked.fixture.invalid/inbox")
            .bind(SHARED_NOTE_ACTIVITY_URI)
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a signed Create shared by a local reblog must be forwarded"
        );
        let shared_note_delete_body = json!({
            "id": SHARED_NOTE_DELETE_ACTIVITY_URI,
            "type": "Delete",
            "actor": ACTOR,
            "signature": {
                "type": "RsaSignature2017",
                "creator": KEY_ID,
                "created": "2026-08-25T12:07:00Z",
                "signatureValue": "fixture"
            },
            "object": {
                "id": SHARED_NOTE_URI,
                "type": "Tombstone"
            }
        })
        .to_string();
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": shared_note_delete_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-shared-remote-note-delete"),
            )
            .await?;
        assert!(
            executor
                .process_one("reply-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1
                    AND payload -> 'arguments' ->> 'status_id' = $2
                    AND payload -> 'arguments' ->> 'activity_type' = 'Delete'",
            )
            .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
            .bind(shared_boost_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "deleting a remote original must withdraw local reblogs from their remote followers"
        );
        sqlx::query("DELETE FROM statuses WHERE id = $1 OR uri = $2")
            .bind(shared_boost_id)
            .bind(SHARED_NOTE_URI)
            .execute(&writer_pool)
            .await?;

        let quoted_note_body = json!({
            "id": QUOTED_NOTE_ACTIVITY_URI,
            "type": "Create",
            "actor": ACTOR,
            "signature": {
                "type": "RsaSignature2017",
                "creator": KEY_ID,
                "created": "2026-08-25T12:06:00Z",
                "signatureValue": "fixture"
            },
            "object": {
                "id": QUOTED_NOTE_URI,
                "type": "Note",
                "attributedTo": ACTOR,
                "published": "2026-08-25T12:06:00Z",
                "content": "<p>Quoted by a local account</p>",
                "to": ["https://www.w3.org/ns/activitystreams#Public"],
                "cc": [],
                "tag": [],
                "attachment": []
            }
        })
        .to_string();
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": quoted_note_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-quoted-remote-note"),
            )
            .await?;
        assert!(
            executor
                .process_one("reply-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        let quoted_status_id: i64 = sqlx::query_scalar("SELECT id FROM statuses WHERE uri = $1")
            .bind(QUOTED_NOTE_URI)
            .fetch_one(&writer_pool)
            .await?;
        let local_quote_status_id: i64 = sqlx::query_scalar(
            "INSERT INTO statuses (
                 account_id, text, spoiler_text, visibility, local, sensitive, reply,
                 created_at, updated_at)
             VALUES ($1, 'local quote wrapper', '', 0, true, false, false,
                     clock_timestamp(), clock_timestamp())
             RETURNING id",
        )
        .bind(MODERATOR)
        .fetch_one(&writer_pool)
        .await?;
        sqlx::query(
            "INSERT INTO quotes
                 (id, account_id, activity_uri, approval_uri, created_at, legacy,
                  quoted_account_id, quoted_status_id, state, status_id, updated_at)
             VALUES ($1, $2, $3, NULL, clock_timestamp(), false, $4, $5, 1, $6,
                     clock_timestamp())",
        )
        .bind(LOCAL_QUOTE_ID)
        .bind(MODERATOR)
        .bind("https://fixture-v4-6-5.rustodon.invalid/quotes/worker-quote")
        .bind(BOB)
        .bind(quoted_status_id)
        .bind(local_quote_status_id)
        .execute(&writer_pool)
        .await?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": quoted_note_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-quoted-remote-note-replay"),
            )
            .await?;
        assert!(
            executor
                .process_one("reply-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = 'rustodon.activitypub.deliver'
                    AND payload -> 'arguments' ->> 'source_account_id' = $1
                    AND payload -> 'arguments' ->> 'inbox_url' = $2
                    AND payload -> 'arguments' -> 'body' ->> 'id' = $3",
            )
            .bind(MODERATOR.to_string())
            .bind("https://account-blocked.fixture.invalid/inbox")
            .bind(QUOTED_NOTE_ACTIVITY_URI)
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a signed Create referenced by a local quote must be forwarded"
        );
        sqlx::query("DELETE FROM quotes WHERE id = $1")
            .bind(LOCAL_QUOTE_ID)
            .execute(&writer_pool)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id = $1 OR uri = $2")
            .bind(local_quote_status_id)
            .bind(QUOTED_NOTE_URI)
            .execute(&writer_pool)
            .await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    sqlx::query("DELETE FROM statuses WHERE id = $1 OR uri = ANY($2)")
        .bind(local_parent_id)
        .bind(vec![PARENT_URI, CHILD_URI, LOCAL_CHILD_URI])
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(private_reply_status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE account_stats SET statuses_count = $2 WHERE account_id = $1")
        .bind(BOB)
        .bind(baseline_statuses_count)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(-320_i64)
        .bind(MODERATOR)
        .execute(&writer_pool)
        .await?;
    if let Some(previous_parent_follower) = previous_parent_follower {
        sqlx::query("INSERT INTO follows SELECT * FROM jsonb_populate_record(NULL::follows, $1)")
            .bind(previous_parent_follower)
            .execute(&writer_pool)
            .await?;
    }
    result
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_unknown_announce_target_is_fetched_and_embedded_self_boost_is_created()
-> Result<(), Box<dyn std::error::Error>> {
    const BOB: i64 = 116_844_606_259_202_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
    const KEY_ID: &str = "https://remote.fixture.invalid/users/bob#secondary-key";
    const ANNOUNCE_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-fetched-announce";
    const EMBEDDED_ANNOUNCE_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-embedded-announce";

    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let baseline_statuses_count: i64 =
        sqlx::query_scalar("SELECT statuses_count FROM account_stats WHERE account_id = $1")
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let target_uri = "http://remote.fixture.invalid/users/bob/statuses/rustodon-fetched-target";
    let embedded_uri = "https://remote.fixture.invalid/users/bob/statuses/rustodon-embedded-target";
    let fetched_note = json!({
        "id": target_uri,
        "type": "Note",
        "attributedTo": ACTOR,
        "published": "2026-08-25T12:10:00Z",
        "content": "<p>Fetched before the Announce</p>",
        "summary": null,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [],
        "tag": [],
        "attachment": []
    })
    .to_string();
    let server = tokio::spawn(fixture_activitypub_server(
        listener,
        fetched_note.into_bytes(),
    ));
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
        local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        media_root_url: "/system".to_owned(),
        media_root: None,
        limited_federation: false,
        remote_media_endpoint: None,
        remote_delivery_endpoint: None,
        remote_fetch_endpoint: Some(endpoint),
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let announce_body = json!({
        "id": ANNOUNCE_URI,
        "type": "Announce",
        "actor": ACTOR,
        "object": target_uri,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "published": "2026-08-25T12:11:00Z"
    })
    .to_string();
    let embedded_body = json!({
        "id": EMBEDDED_ANNOUNCE_URI,
        "type": "Announce",
        "actor": ACTOR,
        "object": {
            "id": embedded_uri,
            "type": "Note",
            "attributedTo": ACTOR,
            "published": "2026-08-25T12:12:00Z",
            "content": "<p>Embedded self-boost</p>",
        "summary": null,
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [],
            "tag": [],
            "attachment": []
        },
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "published": "2026-08-25T12:12:00Z"
    })
    .to_string();
    let result = async {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": announce_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-unknown-announce"),
            )
            .await?;
        assert!(
            executor
                .process_one("announce-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload -> 'arguments' ->> 'object_uri' = $2",
            )
            .bind(ACTIVITYPUB_ANNOUNCE_RESOLVE_JOB_KIND)
            .bind(target_uri)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(queue.dispatch_outbox(10).await?, 1);
        assert!(
            executor
                .process_one("announce-worker", &[Lane::Pull], Duration::seconds(30))
                .await?
        );
        server.await??;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses WHERE uri = $1 AND deleted_at IS NULL",
            )
            .bind(target_uri)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses
                  WHERE uri = $1 AND account_id = $2 AND deleted_at IS NULL",
            )
            .bind(ANNOUNCE_URI)
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        let target_id: i64 = sqlx::query_scalar("SELECT id FROM statuses WHERE uri = $1")
            .bind(target_uri)
            .fetch_one(&writer_pool)
            .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses
                  WHERE uri = $1 AND reblog_of_id = $2 AND deleted_at IS NULL",
            )
            .bind(ANNOUNCE_URI)
            .bind(target_id)
            .fetch_one(&writer_pool)
            .await?,
            1
        );

        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": embedded_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid",
                        "delivery_target_account_id": MODERATOR
                    }),
                )
                .logical_key("activitypub:test-embedded-announce"),
            )
            .await?;
        assert!(
            executor
                .process_one("announce-worker", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses WHERE uri = $1 AND deleted_at IS NULL",
            )
            .bind(embedded_uri)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses
                  WHERE uri = $1 AND reblog_of_id = (SELECT id FROM statuses WHERE uri = $2)
                    AND deleted_at IS NULL",
            )
            .bind(EMBEDDED_ANNOUNCE_URI)
            .bind(embedded_uri)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload -> 'arguments' ->> 'object_uri' = $2",
            )
            .bind(ACTIVITYPUB_ANNOUNCE_RESOLVE_JOB_KIND)
            .bind(embedded_uri)
            .fetch_one(&writer_pool)
            .await?,
            0,
            "embedded self-boosts should not enqueue a remote fetch"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1"
            )
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            baseline_statuses_count + 4
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    sqlx::query("DELETE FROM statuses WHERE uri = ANY($1)")
        .bind(vec![
            target_uri,
            ANNOUNCE_URI,
            embedded_uri,
            EMBEDDED_ANNOUNCE_URI,
        ])
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE account_stats SET statuses_count = $2 WHERE account_id = $1")
        .bind(BOB)
        .bind(baseline_statuses_count)
        .execute(&writer_pool)
        .await?;
    drop(executor);
    result
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn activitypub_fetched_provenance_rejects_forged_create()
-> Result<(), Box<dyn std::error::Error>> {
    fetched_provenance_scenario("Create", false, false).await
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn activitypub_fetched_provenance_dereferences_foreign_nested_note()
-> Result<(), Box<dyn std::error::Error>> {
    fetched_provenance_scenario("Announce", false, false).await
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn activitypub_fetched_provenance_rejects_forged_nested_actor()
-> Result<(), Box<dyn std::error::Error>> {
    fetched_provenance_scenario("Announce", false, true).await
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn activitypub_fetched_provenance_preserves_authoritative_cross_author_boost()
-> Result<(), Box<dyn std::error::Error>> {
    fetched_provenance_scenario("Announce", true, false).await
}

#[cfg(feature = "test-support")]
#[allow(clippy::too_many_lines)]
async fn fetched_provenance_scenario(
    wrapper_type: &str,
    canonical_exists: bool,
    spoof_nested_actor: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    const BOB: i64 = 116_844_606_259_202_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ATTACKER_ID: i64 = -90101;
    const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
    const ATTACKER: &str = "http://evil.fixture.invalid/users/mallory";
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const LOCAL_ACTOR: &str = "https://fixture-v4-6-5.rustodon.invalid/ap/users/116844606259201002";
    const WRAPPER: &str = "http://evil.fixture.invalid/activities/r01-wrapper";
    const TARGET: &str = "http://remote.fixture.invalid/users/bob/statuses/r01-target";
    const BOOST: &str = "https://remote.fixture.invalid/activities/r01-outer";
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?)
        .await?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?)
        .await?;
    reset().await?;
    // The victim is deliberately already cached; no author discovery can authenticate this Note.
    sqlx::query("INSERT INTO accounts SELECT (jsonb_populate_record(NULL::accounts,
        to_jsonb(account) || jsonb_build_object('id', $1::bigint, 'uri', $2::text,
        'username', 'r01-mallory', 'domain', 'evil.fixture.invalid'))).* FROM accounts account WHERE id = $3")
        .bind(ATTACKER_ID).bind(ATTACKER).bind(BOB).execute(&pool).await?;
    sqlx::query(
        "INSERT INTO follows (id, account_id, target_account_id, created_at, updated_at)
        VALUES ($1, $2, $1, now(), now())",
    )
    .bind(ATTACKER_ID)
    .bind(MODERATOR)
    .execute(&pool)
    .await?;
    let baseline_stats: Value =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(BOB)
            .fetch_one(&pool)
            .await?;
    let baseline: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM statuses), (SELECT count(*) FROM mentions), (SELECT count(*) FROM notifications)")
        .fetch_one(&pool).await?;
    let forged_note = json!({
        "id": TARGET, "type": "Note", "attributedTo": ACTOR,
        "content": "<p>Forged R01 mention</p>",
        "to": [activitypub::PUBLIC_ADDRESS, LOCAL_ACTOR], "cc": [],
        "tag": [{"type": "Mention", "href": LOCAL_ACTOR, "name": "@moderator"}],
        "attachment": []
    });
    let document = json!({
        "id": WRAPPER, "type": wrapper_type,
        "actor": if wrapper_type == "Create" || spoof_nested_actor { ACTOR } else { ATTACKER },
        "object": forged_note, "to": [activitypub::PUBLIC_ADDRESS]
    });
    let canonical = json!({
        "id": TARGET, "type": "Note", "attributedTo": ACTOR,
        "content": "<p>Authoritative original</p>",
        "to": [activitypub::PUBLIC_ADDRESS], "cc": [], "tag": [], "attachment": []
    });
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let canonical_fetches = Arc::new(AtomicUsize::new(0));
    let fetches = canonical_fetches.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await?;
            let request = fixture_delivery_request(&mut socket).await?;
            let request = String::from_utf8_lossy(&request);
            let (status, body) = if request.starts_with("GET /activities/r01-wrapper ") {
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("host: evil.fixture.invalid")
                );
                ("200 OK", document.to_string())
            } else {
                assert!(request.starts_with("GET /users/bob/statuses/r01-target "));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("host: remote.fixture.invalid")
                );
                fetches.fetch_add(1, Ordering::SeqCst);
                if canonical_exists {
                    ("200 OK", canonical.to_string())
                } else {
                    ("410 Gone", String::new())
                }
            };
            socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/activity+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), std::io::Error>(())
    });
    let queue = Queue::new(runtime_pool);
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(pool.clone()),
        None,
        Some(ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            media_root_url: "/system".to_owned(),
            media_root: None,
            limited_federation: false,
            remote_media_endpoint: None,
            remote_delivery_endpoint: None,
            remote_fetch_endpoint: Some(endpoint),
        }),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": json!({"id": BOOST, "type": "Announce", "actor": ACTOR,
                        "object": WRAPPER, "to": [activitypub::PUBLIC_ADDRESS]}).to_string(),
                    "signature_key_id": "https://remote.fixture.invalid/users/bob#secondary-key",
                    "remote_domain": "remote.fixture.invalid"
                }),
            )
            .logical_key("activitypub:r01-provenance"),
        )
        .await?;
    assert!(
        executor
            .process_one("r01", &[Lane::Ingress], Duration::seconds(30))
            .await?
    );
    assert_eq!(queue.dispatch_outbox(100).await?, 1);
    assert!(
        executor
            .process_one("r01", &[Lane::Pull], Duration::seconds(30))
            .await?
    );
    // Run notification/distribution work too, rather than asserting only inbox acceptance.
    queue.dispatch_outbox(100).await?;
    for _ in 0..20 {
        if !executor
            .process_one("r01", &[Lane::Core], Duration::seconds(30))
            .await?
        {
            break;
        }
        queue.dispatch_outbox(100).await?;
    }
    server.abort();
    let server_result = server.await;
    assert!(
        server_result
            .as_ref()
            .is_err_and(tokio::task::JoinError::is_cancelled),
        "fixture server failed: {server_result:?}"
    );
    let after: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM statuses), (SELECT count(*) FROM mentions), (SELECT count(*) FROM notifications)")
        .fetch_one(&pool).await?;
    let statuses: Vec<(String, i64, String, Option<i64>)> = sqlx::query_as(
        "SELECT uri, account_id, text, reblog_of_id FROM statuses WHERE uri = ANY($1) ORDER BY uri",
    )
    .bind(vec![TARGET, WRAPPER, BOOST])
    .fetch_all(&pool)
    .await?;
    let target_id: Option<i64> = sqlx::query_scalar("SELECT id FROM statuses WHERE uri = $1")
        .bind(TARGET)
        .fetch_optional(&pool)
        .await?;
    let failed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 AND dead_at IS NOT NULL",
    )
    .bind(ACTIVITYPUB_ANNOUNCE_RESOLVE_JOB_KIND)
    .fetch_one(&pool)
    .await?;
    // Clean up even when the regression assertions below fail (the harness shares its restored DB).
    sqlx::query("DELETE FROM notifications WHERE (activity_type = 'Mention' AND activity_id IN
        (SELECT id FROM mentions WHERE status_id IN (SELECT id FROM statuses WHERE uri = ANY($1))))
        OR (activity_type = 'Status' AND activity_id IN (SELECT id FROM statuses WHERE uri = ANY($1)))")
        .bind(vec![TARGET, WRAPPER, BOOST]).execute(&pool).await?;
    sqlx::query(
        "DELETE FROM mentions WHERE status_id IN (SELECT id FROM statuses WHERE uri = ANY($1))",
    )
    .bind(vec![TARGET, WRAPPER, BOOST])
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM statuses WHERE uri = ANY($1)")
        .bind(vec![BOOST, WRAPPER, TARGET])
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM follows WHERE id = $1")
        .bind(ATTACKER_ID)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
        .bind(vec![BOB, ATTACKER_ID])
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO account_stats SELECT * FROM jsonb_populate_record(NULL::account_stats, $1)",
    )
    .bind(baseline_stats)
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(ATTACKER_ID)
        .execute(&pool)
        .await?;
    assert_eq!(
        after,
        (
            baseline.0 + if canonical_exists { 3 } else { 0 },
            baseline.1,
            baseline.2
        ),
        "fetched provenance must not commit forged statuses/mentions/notifications/boosts: {statuses:?}"
    );
    if canonical_exists {
        assert_eq!(
            canonical_fetches.load(Ordering::SeqCst),
            1,
            "foreign embedded Notes must be dereferenced"
        );
        assert_eq!(failed, 0);
        assert_eq!(after.0, baseline.0 + 3);
        assert_eq!(statuses.len(), 3);
        let original = statuses
            .iter()
            .find(|status| status.0 == TARGET)
            .expect("canonical Note");
        assert_eq!(original.1, BOB);
        assert_eq!(original.2, "<p>Authoritative original</p>");
        for boost in statuses.iter().filter(|status| status.0 != TARGET) {
            assert_eq!(
                boost.3, target_id,
                "both boosts must reference the original"
            );
            assert_eq!(boost.1, if boost.0 == WRAPPER { ATTACKER_ID } else { BOB });
        }
    } else {
        assert_eq!(
            after.0, baseline.0,
            "forged statuses or boosts must not be committed: {statuses:?}"
        );
        assert!(statuses.is_empty());
        assert_eq!(failed, 1, "the invalid target must fail permanently");
        assert_eq!(
            canonical_fetches.load(Ordering::SeqCst),
            usize::from(wrapper_type == "Announce" && !spoof_nested_actor)
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn activitypub_like_and_announce_undo_are_processed_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    const ALICE: i64 = 116_844_606_259_201_001;
    const BOB: i64 = 116_844_606_259_202_001;
    const MODERATOR: i64 = 116_844_606_259_201_002;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
    const KEY_ID: &str = "https://remote.fixture.invalid/users/bob#secondary-key";
    const LIKE_URI: &str = "https://remote.fixture.invalid/activities/rustodon-like";
    const URI_ONLY_LIKE_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-uri-only-like";
    const PRIVATE_LIKE_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-private-like";
    const ANNOUNCE_URI: &str = "https://remote.fixture.invalid/activities/rustodon-announce";
    const ANNOUNCE_ALT_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-announce-alt";
    const REMOTE_TARGET_URI: &str =
        "https://remote.fixture.invalid/users/bob/statuses/rustodon-remote-target";
    const REMOTE_ANNOUNCE_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-remote-announce";
    const REMOTE_NESTED_ANNOUNCE_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-remote-nested-announce";
    const REMOTE_DELETE_ANNOUNCE_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-remote-delete-announce";
    const REMOTE_DELETE_NOTE_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-remote-delete-note";
    const REMOTE_SELF_PRIVATE_ANNOUNCE_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-remote-self-private-announce";
    const REMOTE_ANNOUNCE_NOFOLLOW_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-remote-announce-no-follow";
    const GROUP_ANNOUNCE_URI: &str =
        "https://remote.fixture.invalid/activities/rustodon-group-announce";
    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    reset().await?;
    let exclusive_membership: Value = sqlx::query_scalar(
        "SELECT to_jsonb(list_account) FROM list_accounts list_account WHERE id = 9004",
    )
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM list_accounts WHERE id = 9004")
        .execute(&writer_pool)
        .await?;
    let target_status_id: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (
            account_id, text, spoiler_text, visibility, local, sensitive, reply,
            created_at, updated_at
         ) VALUES ($1, 'interaction target', '', 0, true, false, false,
                   clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(MODERATOR)
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at)
          VALUES ($1, clock_timestamp(), clock_timestamp())",
    )
    .bind(target_status_id)
    .execute(&writer_pool)
    .await?;
    let remote_target_status_id: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (
             account_id, text, spoiler_text, visibility, local, uri, url, language,
             sensitive, reply, created_at, updated_at)
         VALUES ($1, 'known remote interaction target', '', 0, false, $2, $2, 'en',
                 false, false, clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(BOB)
    .bind(REMOTE_TARGET_URI)
    .fetch_one(&writer_pool)
    .await?;
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at)
           VALUES ($1, clock_timestamp(), clock_timestamp())",
    )
    .bind(remote_target_status_id)
    .execute(&writer_pool)
    .await?;
    sqlx::query(
        "UPDATE account_stats SET statuses_count = statuses_count + 1,
            updated_at = clock_timestamp() WHERE account_id = $1",
    )
    .bind(BOB)
    .execute(&writer_pool)
    .await?;
    let baseline_counts = sqlx::query_as::<_, (i64, i64)>(
        "SELECT favourites_count, reblogs_count FROM status_stats WHERE status_id = $1",
    )
    .bind(target_status_id)
    .fetch_one(&writer_pool)
    .await?;
    let baseline_account_statuses_count = sqlx::query_scalar::<_, i64>(
        "SELECT statuses_count FROM account_stats WHERE account_id = $1",
    )
    .bind(BOB)
    .fetch_one(&writer_pool)
    .await?;
    let baseline_remote_target_reblogs_count =
        sqlx::query_scalar::<_, i64>("SELECT reblogs_count FROM status_stats WHERE status_id = $1")
            .bind(remote_target_status_id)
            .fetch_one(&writer_pool)
            .await?;
    let alice_bob_follow: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(follow) FROM follows follow
          WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(116_844_606_259_201_001_i64)
    .bind(BOB)
    .fetch_optional(&writer_pool)
    .await?;
    let baseline_follow_stats = sqlx::query_as::<_, (i64, i64)>(
        "SELECT following_count, followers_count FROM account_stats
          WHERE account_id = ANY($1) ORDER BY account_id",
    )
    .bind(vec![116_844_606_259_201_001_i64, BOB])
    .fetch_all(&writer_pool)
    .await?;
    let baseline_actor_type: Option<String> =
        sqlx::query_scalar("SELECT actor_type::text FROM accounts WHERE id = $1")
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?;
    let moderator_bob_follow: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(follow) FROM follows follow
          WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(MODERATOR)
    .bind(BOB)
    .fetch_optional(&writer_pool)
    .await?;
    let baseline_relationship_tombstones = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM rustodon.idempotency_keys
         WHERE scope = 'rustodon.activitypub.relationship'",
    )
    .fetch_one(&writer_pool)
    .await?;
    let (target_username, target_id_scheme) = sqlx::query_as::<_, (String, Option<i32>)>(
        "SELECT username, id_scheme FROM accounts WHERE id = $1",
    )
    .bind(MODERATOR)
    .fetch_one(&writer_pool)
    .await?;
    let config = ActivityPubDeliveryConfig {
        origin: Url::parse(ORIGIN)?,
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
    };
    let queue = Queue::new(runtime_pool.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let target_uri = if target_id_scheme == Some(1) {
        format!(
            "{}/ap/users/{MODERATOR}/statuses/{target_status_id}",
            ORIGIN.trim_end_matches('/')
        )
    } else {
        format!(
            "{}/users/{target_username}/statuses/{target_status_id}",
            ORIGIN.trim_end_matches('/')
        )
    };
    let like_body = json!({
        "id": LIKE_URI,
        "type": "Like",
        "actor": ACTOR,
        "object": target_uri
    })
    .to_string();
    let undo_like_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Like",
            "id": LIKE_URI,
            "actor": ACTOR,
            "object": target_uri
        }
    })
    .to_string();
    let announce_body = json!({
        "id": ANNOUNCE_URI,
        "type": "Announce",
        "actor": ACTOR,
        "object": target_uri,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "published": "2026-08-24T12:00:00Z"
    })
    .to_string();
    let remote_announce_body = json!({
        "id": REMOTE_ANNOUNCE_URI,
        "type": "Announce",
        "actor": ACTOR,
        "object": REMOTE_TARGET_URI,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "published": "2026-08-24T12:01:00Z"
    })
    .to_string();
    let remote_nested_announce_body = json!({
        "id": REMOTE_NESTED_ANNOUNCE_URI,
        "type": "Announce",
        "actor": ACTOR,
        "object": REMOTE_ANNOUNCE_URI,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "published": "2026-08-24T12:01:30Z"
    })
    .to_string();
    let remote_delete_announce_body = json!({
        "id": REMOTE_DELETE_ANNOUNCE_URI,
        "type": "Announce",
        "actor": ACTOR,
        "object": REMOTE_TARGET_URI,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "published": "2026-08-24T12:02:00Z"
    })
    .to_string();
    let remote_delete_note_body = json!({
        "id": REMOTE_DELETE_NOTE_URI,
        "type": "Delete",
        "actor": ACTOR,
        "object": REMOTE_TARGET_URI
    })
    .to_string();
    let undo_remote_announce_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Announce",
            "id": REMOTE_ANNOUNCE_URI,
            "actor": ACTOR,
            "object": REMOTE_TARGET_URI
        }
    })
    .to_string();
    let remote_self_private_announce_body = json!({
        "id": REMOTE_SELF_PRIVATE_ANNOUNCE_URI,
        "type": "Announce",
        "actor": ACTOR,
        "object": REMOTE_TARGET_URI,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "published": "2026-08-24T12:01:45Z"
    })
    .to_string();
    let undo_remote_self_private_announce_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Announce",
            "id": REMOTE_SELF_PRIVATE_ANNOUNCE_URI,
            "actor": ACTOR,
            "object": REMOTE_TARGET_URI
        }
    })
    .to_string();
    let group_announce_body = json!({
        "id": GROUP_ANNOUNCE_URI,
        "type": "Announce",
        "actor": ACTOR,
        "object": target_uri,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "published": "2026-08-24T12:03:00Z"
    })
    .to_string();
    let undo_group_announce_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Announce",
            "id": GROUP_ANNOUNCE_URI,
            "actor": ACTOR,
            "object": target_uri
        }
    })
    .to_string();
    let undo_announce_uri_only_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": ANNOUNCE_URI
    })
    .to_string();
    let undo_announce_body = json!({
        "type": "Undo",
        "actor": ACTOR,
        "object": {
            "type": "Announce",
            "id": ANNOUNCE_URI,
            "actor": ACTOR,
            "object": target_uri
        }
    })
    .to_string();
    let result = async {
        for (logical_key, body) in [
            ("activitypub:test-like", like_body),
            (
                "activitypub:test-like-duplicate",
                json!({
                    "id": LIKE_URI,
                    "type": "Like",
                    "actor": ACTOR,
                    "object": target_uri
                })
                .to_string(),
            ),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": body,
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid"
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one(
                        "interaction-worker",
                        &[Lane::Ingress],
                        Duration::seconds(30)
                    )
                    .await?
            );
        }
        assert!(queue.dispatch_outbox(100).await? >= 1);
        while executor
            .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
            .await?
        {}
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM favourites WHERE account_id = $1 AND status_id = $2",
            )
            .bind(BOB)
            .bind(target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        let favourite_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM favourites WHERE account_id = $1 AND status_id = $2",
        )
        .bind(BOB)
        .bind(target_status_id)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications
                 WHERE account_id = $1 AND activity_type = 'Favourite' AND activity_id = $2",
            )
            .bind(MODERATOR)
            .bind(favourite_id)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT favourites_count, reblogs_count FROM status_stats WHERE status_id = $1",
            )
            .bind(target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            (baseline_counts.0 + 1, baseline_counts.1)
        );
        for (logical_key, body) in [
            ("activitypub:test-undo-like", undo_like_body),
            (
                "activitypub:test-undo-like-duplicate",
                json!({
                    "type": "Undo",
                    "actor": ACTOR,
                    "object": {"type": "Like", "id": LIKE_URI, "object": target_uri}
                })
                .to_string(),
            ),
            (
                "activitypub:test-like-after-undo",
                json!({
                    "id": LIKE_URI,
                    "type": "Like",
                    "actor": ACTOR,
                    "object": target_uri
                })
                .to_string(),
            ),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": body,
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid"
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one(
                        "interaction-worker",
                        &[Lane::Ingress],
                        Duration::seconds(30)
                    )
                    .await?
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM favourites WHERE account_id = $1 AND status_id = $2",
            )
            .bind(BOB)
            .bind(target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications
                 WHERE account_id = $1 AND activity_type = 'Favourite' AND activity_id = $2",
            )
            .bind(MODERATOR)
            .bind(favourite_id)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        sqlx::query("UPDATE statuses SET visibility = 3 WHERE id = $1")
            .bind(target_status_id)
            .execute(&writer_pool)
            .await?;
        let private_like_body = json!({
            "id": PRIVATE_LIKE_URI,
            "type": "Like",
            "actor": ACTOR,
            "object": target_uri
        })
        .to_string();
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": private_like_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-private-like"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM favourites WHERE account_id = $1 AND status_id = $2",
            )
            .bind(BOB)
            .bind(target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": json!({
                            "type": "Undo",
                            "actor": ACTOR,
                            "object": {
                                "type": "Like",
                                "id": PRIVATE_LIKE_URI,
                                "object": target_uri
                            }
                        })
                        .to_string(),
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-private-like-undo"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM favourites WHERE account_id = $1 AND status_id = $2",
            )
            .bind(BOB)
            .bind(target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        sqlx::query("UPDATE statuses SET visibility = 0 WHERE id = $1")
            .bind(target_status_id)
            .execute(&writer_pool)
            .await?;
        for (logical_key, body) in [
            (
                "activitypub:test-uri-only-like-undo",
                json!({
                    "type": "Undo",
                    "actor": ACTOR,
                    "object": URI_ONLY_LIKE_URI
                })
                .to_string(),
            ),
            (
                "activitypub:test-uri-only-like-after-undo",
                json!({
                    "id": URI_ONLY_LIKE_URI,
                    "type": "Like",
                    "actor": ACTOR,
                    "object": target_uri
                })
                .to_string(),
            ),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": body,
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid"
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one(
                        "interaction-worker",
                        &[Lane::Ingress],
                        Duration::seconds(30)
                    )
                    .await?
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM favourites WHERE account_id = $1 AND status_id = $2",
            )
            .bind(BOB)
            .bind(target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        for (logical_key, body) in [
            ("activitypub:test-announce", announce_body.clone()),
            ("activitypub:test-announce-duplicate", announce_body),
            (
                "activitypub:test-announce-invalid-replay",
                json!({
                    "id": ANNOUNCE_URI,
                    "type": "Announce",
                    "actor": ACTOR,
                    "object": target_uri,
                    "published": "not-a-date"
                })
                .to_string(),
            ),
            (
                "activitypub:test-announce-alternate-uri",
                json!({
                    "id": ANNOUNCE_ALT_URI,
                    "type": "Announce",
                    "actor": ACTOR,
                    "object": target_uri,
                    "to": ["https://www.w3.org/ns/activitystreams#Public"]
                })
                .to_string(),
            ),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": body,
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid"
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one(
                        "interaction-worker",
                        &[Lane::Ingress],
                        Duration::seconds(30)
                    )
                    .await?
            );
        }
        assert!(queue.dispatch_outbox(100).await? >= 1);
        while executor
            .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
            .await?
        {}
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses WHERE uri = $1 AND deleted_at IS NULL",
            )
            .bind(ANNOUNCE_URI)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses
                 WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL",
            )
            .bind(BOB)
            .bind(target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i32>(
                "SELECT visibility FROM statuses WHERE uri = $1 AND deleted_at IS NULL",
            )
            .bind(ANNOUNCE_URI)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, NaiveDateTime>(
                "SELECT created_at FROM statuses WHERE uri = $1 AND deleted_at IS NULL",
            )
            .bind(ANNOUNCE_URI)
            .fetch_one(&writer_pool)
            .await?,
            NaiveDateTime::parse_from_str("2026-08-24 12:00:00", "%Y-%m-%d %H:%M:%S")?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications
                 WHERE account_id = $1 AND activity_type = 'Status'
                   AND activity_id IN (SELECT id FROM statuses WHERE uri = $2)",
            )
            .bind(MODERATOR)
            .bind(ANNOUNCE_URI)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1",
            )
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            baseline_account_statuses_count + 1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT reblogs_count FROM status_stats WHERE status_id = $1",
            )
            .bind(target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            baseline_counts.1 + 1
        );
        for (logical_key, body) in [
            (
                "activitypub:test-remote-announce",
                remote_announce_body.clone(),
            ),
            (
                "activitypub:test-remote-announce-duplicate",
                remote_announce_body,
            ),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": body,
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid"
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one(
                        "interaction-worker",
                        &[Lane::Ingress],
                        Duration::seconds(30)
                    )
                    .await?
            );
        }
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": remote_nested_announce_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-remote-nested-announce"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE uri = $1")
                .bind(REMOTE_NESTED_ANNOUNCE_URI)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "an Announce of a boost must resolve to the original status"
        );
        let remote_boost_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses WHERE uri = $1 AND deleted_at IS NULL",
        )
        .bind(REMOTE_ANNOUNCE_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'event' = 'update'
                    AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(ALICE.to_string())
            .bind(remote_boost_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a remote Announce must enter a local follower's user stream"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses
                  WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL",
            )
            .bind(BOB)
            .bind(remote_target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT reblogs_count FROM status_stats WHERE status_id = $1",
            )
            .bind(remote_target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            baseline_remote_target_reblogs_count + 1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications
                  WHERE account_id = $1 AND activity_type = 'Status' AND activity_id = $2",
            )
            .bind(BOB)
            .bind(remote_boost_id)
            .fetch_one(&writer_pool)
            .await?,
            0,
            "a remote original author must not receive a local notification"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1",
            )
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            baseline_account_statuses_count + 2
        );
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": undo_remote_announce_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-undo-remote-announce"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT deleted_at IS NOT NULL FROM statuses WHERE uri = $1",
            )
            .bind(REMOTE_ANNOUNCE_URI)
            .fetch_one(&writer_pool)
            .await?,
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT reblogs_count FROM status_stats WHERE status_id = $1",
            )
            .bind(remote_target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            baseline_remote_target_reblogs_count
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1",
            )
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            baseline_account_statuses_count + 1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'event' = 'delete'
                    AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(ALICE.to_string())
            .bind(remote_boost_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a remote Undo Announce must remove the boost from a local follower's stream"
        );
        sqlx::query("UPDATE statuses SET visibility = 3 WHERE id = $1")
            .bind(remote_target_status_id)
            .execute(&writer_pool)
            .await?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": remote_self_private_announce_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-remote-self-private-announce"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        let self_private_boost_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses WHERE uri = $1 AND deleted_at IS NULL",
        )
        .bind(REMOTE_SELF_PRIVATE_ANNOUNCE_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT visibility FROM statuses WHERE id = $1")
                .bind(self_private_boost_id)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "a remote author may Announce its own private status"
        );
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": undo_remote_self_private_announce_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-remote-self-private-announce-undo"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT deleted_at IS NOT NULL FROM statuses WHERE id = $1",
            )
            .bind(self_private_boost_id)
            .fetch_one(&writer_pool)
            .await?
        );
        sqlx::query("UPDATE statuses SET visibility = 0 WHERE id = $1")
            .bind(remote_target_status_id)
            .execute(&writer_pool)
            .await?;
        sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
            .bind(116_844_606_259_201_001_i64)
            .bind(BOB)
            .execute(&writer_pool)
            .await?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": json!({
                            "id": REMOTE_ANNOUNCE_NOFOLLOW_URI,
                            "type": "Announce",
                            "actor": ACTOR,
                            "object": REMOTE_TARGET_URI,
                            "to": ["https://www.w3.org/ns/activitystreams#Public"]
                        })
                        .to_string(),
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-remote-announce-without-follow"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE uri = $1",)
                .bind(REMOTE_ANNOUNCE_NOFOLLOW_URI)
                .fetch_one(&writer_pool)
                .await?,
            0,
            "an Announce from an unfollowed remote actor must be ignored"
        );
        if let Some(alice_bob_follow) = &alice_bob_follow {
            sqlx::query(
                "INSERT INTO follows SELECT * FROM jsonb_populate_record(NULL::follows, $1)",
            )
            .bind(alice_bob_follow)
            .execute(&writer_pool)
            .await?;
        }
        for (account_id, (following_count, followers_count)) in [
            (116_844_606_259_201_001_i64, baseline_follow_stats[0]),
            (BOB, baseline_follow_stats[1]),
        ] {
            sqlx::query(
                "UPDATE account_stats SET following_count = $2, followers_count = $3
                  WHERE account_id = $1",
            )
            .bind(account_id)
            .bind(following_count)
            .bind(followers_count)
            .execute(&writer_pool)
            .await?;
        }
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": remote_delete_announce_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-remote-delete-target-announce"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses
                  WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL",
            )
            .bind(BOB)
            .bind(remote_target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1",
            )
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            baseline_account_statuses_count + 2
        );
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": remote_delete_note_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-remote-delete-target"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT deleted_at IS NOT NULL FROM statuses WHERE uri = $1",
            )
            .bind(REMOTE_TARGET_URI)
            .fetch_one(&writer_pool)
            .await?
        );
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT deleted_at IS NOT NULL FROM statuses WHERE uri = $1",
            )
            .bind(REMOTE_DELETE_ANNOUNCE_URI)
            .fetch_one(&writer_pool)
            .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT reblogs_count FROM status_stats WHERE status_id = $1",
            )
            .bind(remote_target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            baseline_remote_target_reblogs_count
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1",
            )
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            baseline_account_statuses_count
        );
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": undo_announce_uri_only_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-undo-announce-uri-only"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT deleted_at IS NOT NULL FROM statuses WHERE uri = $1",
            )
            .bind(ANNOUNCE_URI)
            .fetch_one(&writer_pool)
            .await?
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT reblogs_count FROM status_stats WHERE status_id = $1",
            )
            .bind(target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            baseline_counts.1
        );
        let announce_status_id =
            sqlx::query_scalar::<_, i64>("SELECT id FROM statuses WHERE uri = $1")
                .bind(ANNOUNCE_URI)
                .fetch_one(&writer_pool)
                .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload ->> 'event' = 'delete'
                    AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(ALICE.to_string())
            .bind(announce_status_id.to_string())
            .fetch_one(&writer_pool)
            .await?,
            1,
            "a URI-only remote Undo Announce must remove the boost from a local follower's stream"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.idempotency_keys
                 WHERE scope = 'rustodon.activitypub.relationship'",
            )
            .fetch_one(&writer_pool)
            .await?,
            baseline_relationship_tombstones
        );
        for (logical_key, body) in [
            (
                "activitypub:test-undo-announce-duplicate",
                undo_announce_body,
            ),
            (
                "activitypub:test-announce-after-undo",
                json!({
                    "id": ANNOUNCE_URI,
                    "type": "Announce",
                    "actor": ACTOR,
                    "object": target_uri
                })
                .to_string(),
            ),
        ] {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Ingress,
                        ACTIVITYPUB_INBOX_JOB_KIND,
                        json!({
                            "body": body,
                            "signature_key_id": KEY_ID,
                            "remote_domain": "remote.fixture.invalid"
                        }),
                    )
                    .logical_key(logical_key),
                )
                .await?;
            assert!(
                executor
                    .process_one(
                        "interaction-worker",
                        &[Lane::Ingress],
                        Duration::seconds(30)
                    )
                    .await?
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT reblogs_count FROM status_stats WHERE status_id = $1",
            )
            .bind(target_status_id)
            .fetch_one(&writer_pool)
            .await?,
            baseline_counts.1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications
                 WHERE account_id = $1 AND activity_type = 'Status'
                   AND activity_id IN (SELECT id FROM statuses WHERE uri = $2)",
            )
            .bind(MODERATOR)
            .bind(ANNOUNCE_URI)
            .fetch_one(&writer_pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1",
            )
            .bind(BOB)
            .fetch_one(&writer_pool)
            .await?,
            baseline_account_statuses_count - 1
        );
        sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
            .bind(MODERATOR)
            .bind(BOB)
            .execute(&writer_pool)
            .await?;
        sqlx::query(
            "INSERT INTO follows
                (account_id, target_account_id, show_reblogs, notify, languages, uri,
                 created_at, updated_at)
             VALUES ($1, $2, true, false, NULL, NULL, clock_timestamp(), clock_timestamp())",
        )
        .bind(MODERATOR)
        .bind(BOB)
        .execute(&writer_pool)
        .await?;
        sqlx::query("UPDATE accounts SET actor_type = 'Group' WHERE id = $1")
            .bind(BOB)
            .execute(&writer_pool)
            .await?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": group_announce_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-group-announce"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        queue.dispatch_outbox(100).await?;
        while executor
            .process_one("notification-worker", &[Lane::Core], Duration::seconds(30))
            .await?
        {}
        let group_boost_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses WHERE uri = $1 AND deleted_at IS NULL",
        )
        .bind(GROUP_ANNOUNCE_URI)
        .fetch_one(&writer_pool)
        .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications
                  WHERE account_id = $1 AND activity_type = 'Status' AND activity_id = $2",
            )
            .bind(MODERATOR)
            .bind(group_boost_id)
            .fetch_one(&writer_pool)
            .await?,
            0,
            "a group followed by the original author must not create a reblog notification"
        );
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Ingress,
                    ACTIVITYPUB_INBOX_JOB_KIND,
                    json!({
                        "body": undo_group_announce_body,
                        "signature_key_id": KEY_ID,
                        "remote_domain": "remote.fixture.invalid"
                    }),
                )
                .logical_key("activitypub:test-group-announce-undo"),
            )
            .await?;
        assert!(
            executor
                .process_one(
                    "interaction-worker",
                    &[Lane::Ingress],
                    Duration::seconds(30)
                )
                .await?
        );
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT deleted_at IS NOT NULL FROM statuses WHERE id = $1",
            )
            .bind(group_boost_id)
            .fetch_one(&writer_pool)
            .await?
        );
        sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
            .bind(MODERATOR)
            .bind(BOB)
            .execute(&writer_pool)
            .await?;
        if let Some(moderator_bob_follow) = &moderator_bob_follow {
            sqlx::query(
                "INSERT INTO follows SELECT * FROM jsonb_populate_record(NULL::follows, $1)",
            )
            .bind(moderator_bob_follow)
            .execute(&writer_pool)
            .await?;
        }
        sqlx::query("UPDATE accounts SET actor_type = $2 WHERE id = $1")
            .bind(BOB)
            .bind(baseline_actor_type.as_deref())
            .execute(&writer_pool)
            .await?;
        sqlx::query(
            "INSERT INTO list_accounts
                SELECT * FROM jsonb_populate_record(NULL::list_accounts, $1)",
        )
        .bind(&exclusive_membership)
        .execute(&writer_pool)
        .await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    sqlx::query(
        "DELETE FROM notifications WHERE activity_id IN (
             SELECT id FROM favourites WHERE account_id = $1 AND status_id = $2
             UNION ALL
             SELECT id FROM statuses WHERE uri = ANY($3)
          )",
    )
    .bind(BOB)
    .bind(target_status_id)
    .bind(vec![
        ANNOUNCE_URI,
        REMOTE_ANNOUNCE_URI,
        REMOTE_NESTED_ANNOUNCE_URI,
        REMOTE_DELETE_ANNOUNCE_URI,
        REMOTE_SELF_PRIVATE_ANNOUNCE_URI,
        GROUP_ANNOUNCE_URI,
    ])
    .execute(&writer_pool)
    .await?;
    sqlx::query("DELETE FROM favourites WHERE account_id = $1 AND status_id = $2")
        .bind(BOB)
        .bind(target_status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM tombstones WHERE account_id = $1 AND uri = ANY($2)")
        .bind(BOB)
        .bind(vec![
            LIKE_URI,
            PRIVATE_LIKE_URI,
            URI_ONLY_LIKE_URI,
            ANNOUNCE_URI,
            REMOTE_ANNOUNCE_URI,
            REMOTE_SELF_PRIVATE_ANNOUNCE_URI,
            REMOTE_TARGET_URI,
            GROUP_ANNOUNCE_URI,
        ])
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE uri = ANY($1)")
        .bind(vec![
            ANNOUNCE_URI,
            REMOTE_ANNOUNCE_URI,
            REMOTE_NESTED_ANNOUNCE_URI,
            REMOTE_DELETE_ANNOUNCE_URI,
            REMOTE_SELF_PRIVATE_ANNOUNCE_URI,
            GROUP_ANNOUNCE_URI,
        ])
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(remote_target_status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(target_status_id)
        .execute(&writer_pool)
        .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(MODERATOR)
        .bind(BOB)
        .execute(&writer_pool)
        .await?;
    if let Some(moderator_bob_follow) = &moderator_bob_follow {
        sqlx::query("INSERT INTO follows SELECT * FROM jsonb_populate_record(NULL::follows, $1)")
            .bind(moderator_bob_follow)
            .execute(&writer_pool)
            .await?;
    }
    sqlx::query("UPDATE accounts SET actor_type = $2 WHERE id = $1")
        .bind(BOB)
        .bind(baseline_actor_type.as_deref())
        .execute(&writer_pool)
        .await?;
    sqlx::query("UPDATE account_stats SET statuses_count = $2 WHERE account_id = $1")
        .bind(BOB)
        .bind(baseline_account_statuses_count - 1)
        .execute(&writer_pool)
        .await?;
    result
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
    assert!(
        record_outbox_once_in(
            &mut transaction,
            &JobSpec::new(Lane::Push, "immutable.delivery", json!({"id": "first"}))
                .logical_key("immutable:delivery"),
        )
        .await?
    );
    transaction.commit().await?;
    assert_eq!(queue.dispatch_outbox(10).await?, 1);
    let immutable_job = queue
        .claim("immutable", &[Lane::Push], Duration::seconds(30))
        .await?
        .expect("the immutable event becomes a durable job");
    assert!(
        queue
            .complete(
                immutable_job.id,
                &immutable_job.lease_owner,
                immutable_job.generation,
            )
            .await?
    );
    let mut transaction = pool.begin().await?;
    assert!(
        !record_outbox_once_in(
            &mut transaction,
            &JobSpec::new(Lane::Push, "immutable.delivery", json!({"id": "second"}))
                .logical_key("immutable:delivery"),
        )
        .await?
    );
    transaction.commit().await?;
    assert_eq!(
        queue.dispatch_outbox(10).await?,
        0,
        "retrying immutable fan-out must not reset an already-dispatched delivery"
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
async fn activitypub_delivery_outbox_serializes_same_inbox_across_workers()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let queue = Queue::new(pool.clone());
    reset().await?;

    let mut transaction = pool.begin().await?;
    for (logical_key, activity_id) in [
        ("delivery:first", "https://fixture.invalid/activities/first"),
        (
            "delivery:second",
            "https://fixture.invalid/activities/second",
        ),
    ] {
        record_outbox_once_in(
            &mut transaction,
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_DELIVERY_JOB_KIND,
                json!({
                    "source_account_id": 42,
                    "inbox_url": "https://remote.invalid/inbox",
                    "remote_domain": "remote.invalid",
                    "body": {"type": "Create", "id": activity_id},
                }),
            )
            .logical_key(logical_key),
        )
        .await?;
    }
    transaction.commit().await?;

    assert_eq!(
        queue.dispatch_outbox(10).await?,
        1,
        "a later delivery for one inbox must wait for the earlier outbox event"
    );
    let first = queue
        .claim("push-first", &[Lane::Push], Duration::milliseconds(25))
        .await?
        .expect("the first delivery is claimed");
    assert_eq!(
        first.arguments["body"]["id"],
        "https://fixture.invalid/activities/first"
    );

    sqlx::query(
        "UPDATE rustodon.ordering_markers
            SET created_at = clock_timestamp() - interval '2 seconds',
                expires_at = clock_timestamp() - interval '1 second'",
    )
    .execute(&pool)
    .await?;
    assert_eq!(queue.dispatch_outbox(10).await?, 1);
    tokio::time::sleep(std::time::Duration::from_millis(35)).await;
    let recovered = queue
        .claim("push-recovery", &[Lane::Push], Duration::seconds(1))
        .await?
        .expect("a second worker recovers the abandoned first delivery");
    assert_eq!(recovered.id, first.id);
    assert!(
        queue
            .claim("push-second", &[Lane::Push], Duration::seconds(1))
            .await?
            .is_none(),
        "the successor must remain fenced until recovery completes"
    );
    assert!(
        queue
            .complete(recovered.id, &recovered.lease_owner, recovered.generation)
            .await?
    );

    let second = queue
        .claim("push-second", &[Lane::Push], Duration::seconds(1))
        .await?
        .expect("the successor is claimable after recovery completes");
    assert_eq!(
        second.arguments["body"]["id"],
        "https://fixture.invalid/activities/second"
    );
    assert!(
        queue
            .complete(second.id, &second.lease_owner, second.generation)
            .await?
    );
    assert_eq!(queue.queued_count().await?, 0);
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
    for index in 0..20 {
        queue
            .enqueue(
                &JobSpec::new(Lane::Pull, "fixture.http", json!({}))
                    .logical_key(format!("effect-{index}"))
                    .max_attempts(2),
            )
            .await?;
    }
    let executor = WorkerExecutor::new(queue.clone(), registry, 4, 1)?;
    for round in 0..2 {
        let attempts = (0..20)
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
    assert!(maximum.load(Ordering::SeqCst) <= 4);
    assert_eq!(queue.queued_count().await?, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.idempotency_keys WHERE scope = 'worker-test'",
        )
        .fetch_one(&pool)
        .await?,
        20
    );
    Ok(())
}

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn executor_processes_twenty_user_waves_without_duplicate_effects()
-> Result<(), Box<dyn std::error::Error>> {
    const USER_COUNT: usize = 20;
    const WAVES: usize = 8;
    const PERMITS: usize = 4;

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
    registry.register(
        "fixture.sustained",
        Lane::Pull,
        ResourceClass::RemoteHttp,
        {
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
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    sqlx::query(
                        "INSERT INTO rustodon.idempotency_keys
                           (scope, key, fingerprint, result, expires_at)
                         VALUES ('worker-sustained', $1, $2, '{}'::jsonb,
                                 clock_timestamp() + interval '1 hour')
                         ON CONFLICT (scope, key) DO NOTHING",
                    )
                    .bind(job.logical_key.as_deref().unwrap_or("missing"))
                    .bind([0_u8; 32].as_slice())
                    .execute(&pool)
                    .await
                    .map_err(|_| HandlerFailure::retry("sustained effect write failed"))?;
                    active.fetch_sub(1, Ordering::SeqCst);
                    if job.attempt == 1 {
                        Err(HandlerFailure::retry("retry after sustained effect commit"))
                    } else {
                        Ok(())
                    }
                })
            }
        },
    )?;

    let executor = WorkerExecutor::new(queue.clone(), registry, PERMITS, 1)?;
    for wave in 0..WAVES {
        for user in 0..USER_COUNT {
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Pull,
                        "fixture.sustained",
                        json!({"user": user, "wave": wave}),
                    )
                    .logical_key(format!("user-{user}-wave-{wave}"))
                    .max_attempts(2),
                )
                .await?;
        }

        let first_attempts = (0..USER_COUNT)
            .map(|slot| {
                let executor = executor.clone();
                async move {
                    executor
                        .process_one(
                            &format!("sustained-{wave}-first-{slot}"),
                            &[Lane::Pull],
                            Duration::seconds(1),
                        )
                        .await
                }
            })
            .collect::<Vec<_>>();
        for result in futures_util::future::join_all(first_attempts).await {
            assert!(result?);
        }

        sqlx::query(
            "UPDATE rustodon.durable_jobs
                SET run_at = clock_timestamp()
              WHERE kind = 'fixture.sustained' AND dead_at IS NULL",
        )
        .execute(&pool)
        .await?;

        let retry_attempts = (0..USER_COUNT)
            .map(|slot| {
                let executor = executor.clone();
                async move {
                    executor
                        .process_one(
                            &format!("sustained-{wave}-retry-{slot}"),
                            &[Lane::Pull],
                            Duration::seconds(1),
                        )
                        .await
                }
            })
            .collect::<Vec<_>>();
        for result in futures_util::future::join_all(retry_attempts).await {
            assert!(result?);
        }
    }

    assert_eq!(maximum.load(Ordering::SeqCst), PERMITS);
    assert_eq!(queue.queued_count().await?, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.durable_jobs
              WHERE kind = 'fixture.sustained' AND dead_at IS NOT NULL",
        )
        .fetch_one(&pool)
        .await?,
        0,
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.idempotency_keys
              WHERE scope = 'worker-sustained'",
        )
        .fetch_one(&pool)
        .await?,
        i64::try_from(USER_COUNT * WAVES)?,
    );
    let expected_keys = (0..WAVES)
        .flat_map(|wave| (0..USER_COUNT).map(move |user| format!("user-{user}-wave-{wave}")))
        .collect::<BTreeSet<_>>();
    let actual_keys = sqlx::query_scalar::<_, String>(
        "SELECT key FROM rustodon.idempotency_keys
          WHERE scope = 'worker-sustained' ORDER BY key",
    )
    .fetch_all(&pool)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(actual_keys, expected_keys);
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
#[allow(clippy::too_many_lines)]
async fn runtime_publishes_readiness_and_removes_it_on_graceful_shutdown()
-> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let owner = PgPoolOptions::new()
        .max_connections(2)
        .connect(&owner_url)
        .await?;
    let queue = Queue::new(pool.clone());
    reset().await?;
    let mute_account_id = 116_844_606_259_201_001_i64;
    let mute_target_account_id = -323_i64;
    let previous_mute: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(mute) FROM public.mutes mute \
         WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(mute_account_id)
    .bind(mute_target_account_id)
    .fetch_optional(&owner)
    .await?;
    sqlx::query("DELETE FROM public.mutes WHERE account_id = $1 AND target_account_id = $2")
        .bind(mute_account_id)
        .bind(mute_target_account_id)
        .execute(&owner)
        .await?;
    let expired_mute_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO public.mutes (account_id, created_at, expires_at, hide_notifications, \
             target_account_id, updated_at) \
         VALUES ($1, clock_timestamp() - interval '2 seconds', \
             clock_timestamp() - interval '1 second', false, $2, clock_timestamp()) \
         RETURNING id",
    )
    .bind(mute_account_id)
    .bind(mute_target_account_id)
    .fetch_one(&owner)
    .await?;
    let expired_rate_limit_key = format!("test:maintenance-rate-limit:{}", std::process::id());
    sqlx::query(
        "INSERT INTO rustodon.rate_limit_windows \
           (window_key, bucket, attempts, expires_at) \
         VALUES ($1, 0, 1, clock_timestamp() - interval '1 second')",
    )
    .bind(&expired_rate_limit_key)
    .execute(&pool)
    .await?;
    let expired_remote_fetch_host =
        format!("remote-fetch-maintenance-{}.example", std::process::id());
    sqlx::query(
        "INSERT INTO rustodon.remote_fetch_leases (host, lease_id, expires_at) \
         VALUES ($1, 'expired', clock_timestamp() - interval '1 second')",
    )
    .bind(&expired_remote_fetch_host)
    .execute(&pool)
    .await?;
    let handlers = infrastructure_handlers_with_writer(&queue, Some(owner.clone()))?;
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
    let _mute_outbox_id = record_outbox_in(
        &mut transaction,
        &JobSpec::new(
            Lane::Maintenance,
            "rustodon.mastodon.delete_mute",
            json!({"mute_id": expired_mute_id}),
        )
        .logical_key("expired-mute-test"),
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
    tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if !sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM rustodon.remote_fetch_leases WHERE host = $1)",
            )
            .bind(&expired_remote_fetch_host)
            .fetch_one(&pool)
            .await
            .expect("remote-fetch lease cleanup inspection succeeds")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if !sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM rustodon.rate_limit_windows WHERE window_key = $1)",
            )
            .bind(&expired_rate_limit_key)
            .fetch_one(&pool)
            .await
            .expect("rate-limit cleanup inspection succeeds")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS (SELECT 1 FROM public.mutes WHERE id = $1)",
            )
            .bind(expired_mute_id)
            .fetch_one(&owner)
            .await
            .expect("mute expiry inspection succeeds")
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
    sqlx::query("DELETE FROM public.mutes WHERE id = $1")
        .bind(expired_mute_id)
        .execute(&owner)
        .await?;
    if let Some(previous_mute) = previous_mute {
        sqlx::query(
            "INSERT INTO public.mutes \
             SELECT * FROM jsonb_populate_record(NULL::public.mutes, $1)",
        )
        .bind(previous_mute)
        .execute(&owner)
        .await?;
    }
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

#[cfg(feature = "test-support")]
async fn fixture_delivery_server(listener: TcpListener) -> Result<Vec<u8>, std::io::Error> {
    let (mut socket, _) = listener.accept().await?;
    let request = fixture_delivery_request(&mut socket).await?;
    socket
        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await?;
    Ok(request)
}

#[cfg(feature = "test-support")]
async fn fixture_blocking_delivery_server(
    listener: TcpListener,
    accepted: Arc<Notify>,
    release: Arc<Notify>,
) -> Result<Vec<u8>, std::io::Error> {
    let (mut socket, _) = listener.accept().await?;
    let request = fixture_delivery_request(&mut socket).await?;
    accepted.notify_one();
    release.notified().await;
    socket
        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await?;
    Ok(request)
}

#[cfg(feature = "test-support")]
async fn fixture_ordered_relationship_delivery_server(
    listener: TcpListener,
    accepted: Arc<Notify>,
    release: Arc<Notify>,
) -> Result<Vec<Vec<u8>>, std::io::Error> {
    let (mut first_socket, _) = listener.accept().await?;
    let first_request = fixture_delivery_request(&mut first_socket).await?;
    accepted.notify_one();
    release.notified().await;
    first_socket
        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await?;
    drop(first_socket);

    let (mut second_socket, _) = listener.accept().await?;
    let second_request = fixture_delivery_request(&mut second_socket).await?;
    second_socket
        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await?;
    Ok(vec![first_request, second_request])
}

#[cfg(feature = "test-support")]
async fn fixture_retry_delivery_server(
    listener: TcpListener,
) -> Result<Vec<Vec<u8>>, std::io::Error> {
    let mut requests = Vec::with_capacity(2);
    for response in [
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ] {
        let (mut socket, _) = listener.accept().await?;
        requests.push(fixture_delivery_request(&mut socket).await?);
        socket.write_all(response.as_bytes()).await?;
    }
    Ok(requests)
}

#[cfg(feature = "test-support")]
async fn fixture_crashing_delivery_server(
    listener: TcpListener,
    accepted: Arc<Notify>,
    release: Arc<Notify>,
) -> Result<Vec<Vec<u8>>, std::io::Error> {
    let (mut socket, _) = listener.accept().await?;
    let mut requests = vec![fixture_delivery_request(&mut socket).await?];
    socket
        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 1\r\nConnection: close\r\n\r\n")
        .await?;
    accepted.notify_one();
    release.notified().await;
    let _ = socket.write_all(b"x").await;
    for _ in 0..2 {
        let (mut socket, _) = listener.accept().await?;
        requests.push(fixture_delivery_request(&mut socket).await?);
        socket
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
    }
    Ok(requests)
}

#[cfg(feature = "test-support")]
async fn fixture_delivery_request(
    socket: &mut tokio::net::TcpStream,
) -> Result<Vec<u8>, std::io::Error> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("Content-Length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or_default();
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    Ok(request)
}

async fn fixture_retrying_smtp_server(
    listener: TcpListener,
    first_accepted: Arc<Notify>,
) -> Result<Vec<Vec<u8>>, std::io::Error> {
    let mut messages = Vec::with_capacity(3);
    for attempt in 0..3 {
        let (socket, _) = listener.accept().await?;
        let mut reader = BufReader::new(socket);
        reader
            .get_mut()
            .write_all(b"220 fixture.test ESMTP\r\n")
            .await?;

        let mut line = String::new();
        reader.read_line(&mut line).await?;
        assert!(line.to_ascii_uppercase().starts_with("EHLO "), "{line:?}");
        reader
            .get_mut()
            .write_all(b"250-fixture.test\r\n250-8BITMIME\r\n250 OK\r\n")
            .await?;

        line.clear();
        reader.read_line(&mut line).await?;
        assert!(
            line.to_ascii_uppercase().starts_with("MAIL FROM:"),
            "{line:?}"
        );
        reader
            .get_mut()
            .write_all(b"250 2.1.0 accepted\r\n")
            .await?;

        line.clear();
        reader.read_line(&mut line).await?;
        assert!(
            line.to_ascii_uppercase().starts_with("RCPT TO:"),
            "{line:?}"
        );
        reader
            .get_mut()
            .write_all(b"250 2.1.5 accepted\r\n")
            .await?;

        line.clear();
        reader.read_line(&mut line).await?;
        assert!(line.eq_ignore_ascii_case("DATA\r\n"), "{line:?}");
        reader
            .get_mut()
            .write_all(b"354 end with <CRLF>.<CRLF>\r\n")
            .await?;

        let mut message = Vec::new();
        loop {
            line.clear();
            reader.read_line(&mut line).await?;
            if line == ".\r\n" {
                break;
            }
            message.extend_from_slice(line.as_bytes());
        }
        reader.get_mut().write_all(b"250 2.0.0 queued\r\n").await?;
        if attempt == 0 {
            first_accepted.notify_one();
        }
        messages.push(message);
    }
    Ok(messages)
}

fn worker_mail_config(port: u16, smtp_domain: &str) -> MailConfig {
    MailConfig::new(
        SmtpConfig::Enabled(Box::new(SmtpSettings {
            delivery_method: SmtpDeliveryMethod::Smtp,
            server: "127.0.0.1".to_owned(),
            port,
            login: None,
            password: None,
            from: Mailbox {
                display_name: Some("Fixture Notifications".to_owned()),
                address: "notifications@example.invalid".to_owned(),
            },
            reply_to: None,
            return_path: None,
            domain: smtp_domain.to_owned(),
            authentication: SmtpAuthentication::None,
            transport: SmtpTransport::Plain,
            verify_mode: None,
            ca_file: PathBuf::from("/etc/ssl/certs/ca-certificates.crt"),
        })),
        Url::parse("https://example.invalid/").unwrap(),
        SecretString::new("worker-mail-secret".to_owned()),
    )
}

fn message_id_header(message: &[u8]) -> Option<String> {
    String::from_utf8_lossy(message)
        .lines()
        .find(|line| line.starts_with("Message-ID:"))
        .map(str::to_owned)
}

#[cfg(feature = "test-support")]
async fn fixture_media_server_for_retries(
    listener: TcpListener,
    body: Vec<u8>,
    responses: usize,
) -> Result<(), std::io::Error> {
    for _ in 0..responses {
        let (mut socket, _) = listener.accept().await?;
        let mut request = vec![0; 4096];
        let _ = socket.read(&mut request).await?;
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/gif\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(headers.as_bytes()).await?;
        socket.write_all(&body).await?;
    }
    Ok(())
}

#[cfg(feature = "test-support")]
async fn fixture_media_server_with_lease_barrier(
    listener: TcpListener,
    body: Vec<u8>,
    request_started: Arc<Notify>,
    release_request: Arc<Notify>,
) -> Result<(), std::io::Error> {
    let (mut socket, _) = listener.accept().await?;
    fixture_delivery_request(&mut socket).await?;
    request_started.notify_one();
    release_request.notified().await;
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: image/gif\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(headers.as_bytes()).await;
    let _ = socket.write_all(&body).await;
    drop(socket);

    let (mut socket, _) = listener.accept().await?;
    fixture_delivery_request(&mut socket).await?;
    socket.write_all(headers.as_bytes()).await?;
    socket.write_all(&body).await
}

#[cfg(feature = "test-support")]
async fn fixture_activitypub_server(
    listener: TcpListener,
    body: Vec<u8>,
) -> Result<(), std::io::Error> {
    let (mut socket, _) = listener.accept().await?;
    let mut request = [0; 4096];
    let _ = socket.read(&mut request).await?;
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/activity+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(headers.as_bytes()).await?;
    socket.write_all(&body).await
}

#[cfg(feature = "test-support")]
async fn fixture_retry_activitypub_server(
    listener: TcpListener,
    body: Vec<u8>,
) -> Result<Vec<Vec<u8>>, std::io::Error> {
    let mut requests = Vec::with_capacity(2);
    for status in ["503 Service Unavailable", "200 OK"] {
        let (mut socket, _) = listener.accept().await?;
        requests.push(fixture_delivery_request(&mut socket).await?);
        let response_body = if status == "200 OK" {
            body.as_slice()
        } else {
            &[]
        };
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/activity+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        );
        socket.write_all(headers.as_bytes()).await?;
        socket.write_all(response_body).await?;
    }
    Ok(requests)
}

async fn reset() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("RUSTODON_WORKER_ADMIN_DATABASE_URL")?;
    let mut connection = PgConnection::connect(&url).await?;
    sqlx::raw_sql(
        "TRUNCATE rustodon.durable_jobs, rustodon.outbox_events, rustodon.heartbeats, \
                  rustodon.idempotency_keys, rustodon.ordering_markers, \
                  rustodon.rate_limit_windows, rustodon.remote_fetch_leases, \
                  rustodon.domain_health \
         RESTART IDENTITY CASCADE",
    )
    .execute(&mut connection)
    .await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn discovered_actor_follow_accept_allows_shared_inbox_private_note()
-> Result<(), Box<dyn std::error::Error>> {
    const ALICE: i64 = 116_844_606_259_201_001;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ACTOR: &str = "http://fresh-audience.fixture.invalid/users/fresh";
    // Deliberately not actor + /followers: retain the collection actually advertised.
    const FOLLOWERS: &str = "http://fresh-audience.fixture.invalid/collections/subscribers";
    const FOLLOWING: &str = "http://fresh-audience.fixture.invalid/collections/subscriptions";
    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    let runtime = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    reset().await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM accounts WHERE uri = $1")
            .bind(ACTOR)
            .fetch_one(&pool)
            .await?,
        0
    );
    let repository = Repository::connect(&owner_url).await?;
    let local = repository
        .account(ALICE)
        .await?
        .ok_or("local actor missing")?;
    let local_uri = activitypub::actor_url(&Url::parse(ORIGIN)?, &local);
    let baseline_following: i64 =
        sqlx::query_scalar("SELECT following_count FROM account_stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    let key_id = format!("{ACTOR}#main-key");
    let actor_document = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": ACTOR, "type": "Person", "preferredUsername": "fresh",
        "summary": "Fresh discovery", "inbox": format!("{ACTOR}/inbox"),
        "followers": FOLLOWERS, "following": FOLLOWING,
        "endpoints": {"sharedInbox": "http://fresh-audience.fixture.invalid/inbox"},
        "publicKey": {"id": key_id, "owner": ACTOR, "publicKeyPem": local.public_key}
    });
    let webfinger = json!({
        "subject": "acct:fresh@fresh-audience.fixture.invalid",
        "links": [{"rel": "self", "type": "application/activity+json", "href": ACTOR}]
    });
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (content_type, body) in [
            ("application/activity+json", actor_document),
            ("application/jrd+json", webfinger),
        ] {
            let (mut socket, _) = listener.accept().await?;
            requests.push(fixture_delivery_request(&mut socket).await?);
            let body = body.to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
        }
        Ok::<_, std::io::Error>(requests)
    });
    let delivery_listener = TcpListener::bind("127.0.0.1:0").await?;
    let delivery_endpoint = delivery_listener.local_addr()?;
    let delivery_server = tokio::spawn(fixture_delivery_server(delivery_listener));
    let queue = Queue::new(runtime);
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(pool.clone()),
        None,
        Some(ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            media_root_url: "/system".to_owned(),
            media_root: None,
            limited_federation: false,
            remote_media_endpoint: None,
            remote_delivery_endpoint: Some(delivery_endpoint),
            remote_fetch_endpoint: Some(endpoint),
        }),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    // An inbox job starts after signature verification, just as for /inbox in production.
    let first_note = format!("{ACTOR}/statuses/discovery");
    queue.enqueue(&JobSpec::new(Lane::Ingress, ACTIVITYPUB_INBOX_JOB_KIND, json!({
        "signature_key_id": key_id, "remote_domain": "fresh-audience.fixture.invalid",
        "delivery_target_account_id": ALICE,
        "body": json!({"id": format!("{first_note}/activity"), "type": "Create", "actor": ACTOR,
            "object": {"id": first_note, "type": "Note", "attributedTo": ACTOR,
                "content": "<p>Discovered</p>", "summary": "", "to": [local_uri]}}).to_string()
    }))).await?;
    assert!(
        executor
            .process_one(
                "audience-discovery",
                &[Lane::Ingress],
                Duration::seconds(30)
            )
            .await?
    );
    let remote_id: i64 = sqlx::query_scalar("SELECT id FROM accounts WHERE uri = $1")
        .bind(ACTOR)
        .fetch_one(&pool)
        .await?;
    let requests = tokio::time::timeout(std::time::Duration::from_secs(5), server).await???;
    assert!(String::from_utf8_lossy(&requests[0]).starts_with("GET /users/fresh "));
    assert!(String::from_utf8_lossy(&requests[1]).starts_with("GET /.well-known/webfinger?"));
    let writer = WriteRepository::connect(&owner_url).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-follow-v4-6-5"),
    );
    let authenticated = BearerAuthenticator::new(repository)
        .authenticate(&headers, WRITE_FOLLOWS)
        .await?;
    let follow = writer
        .set_follow_with_origin(
            &authenticated,
            remote_id,
            true,
            None,
            None,
            None,
            Some(ORIGIN),
            false,
        )
        .await?;
    let follow_uri = follow.activity_uri.ok_or("Follow URI missing")?;
    assert!(queue.dispatch_outbox(100).await? >= 1);
    assert!(
        executor
            .process_one("audience-follow", &[Lane::Push], Duration::seconds(30))
            .await?
    );
    let delivered =
        tokio::time::timeout(std::time::Duration::from_secs(5), delivery_server).await???;
    let body_start = delivered
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .ok_or("missing HTTP headers")?
        + 4;
    let delivered: Value = serde_json::from_slice(&delivered[body_start..])?;
    assert_eq!(delivered["type"], "Follow");
    assert_eq!(delivered["id"], follow_uri);
    assert_eq!(delivered["object"], ACTOR);
    let private_note = format!("{ACTOR}/statuses/private");
    for body in [
        json!({"type": "Accept", "actor": ACTOR, "object": follow_uri}),
        json!({"id": format!("{private_note}/activity"), "type": "Create", "actor": ACTOR,
            "object": {"id": private_note, "type": "Note", "attributedTo": ACTOR,
                "content": "<p>Followers only</p>", "summary": "", "to": [FOLLOWERS], "cc": []}}),
    ] {
        // No delivery_target_account_id: exercise the shared inbox relevance decision.
        queue.enqueue(&JobSpec::new(Lane::Ingress, ACTIVITYPUB_INBOX_JOB_KIND, json!({
            "signature_key_id": key_id, "remote_domain": "fresh-audience.fixture.invalid", "body": body.to_string()
        }))).await?;
        assert!(
            executor
                .process_one("audience-private", &[Lane::Ingress], Duration::seconds(30))
                .await?
        );
    }
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2 AND uri = $3")
        .bind(ALICE).bind(remote_id).bind(&follow_uri).fetch_one(&pool).await?, 1);
    let stored: Option<(i64, i32)> =
        sqlx::query_as("SELECT account_id, visibility FROM statuses WHERE uri = $1")
            .bind(&private_note)
            .fetch_optional(&pool)
            .await?;
    assert_eq!(
        stored,
        Some((remote_id, 2)),
        "freshly discovered followers-only Note must survive shared-inbox ingestion"
    );
    let collections: (String, String) =
        sqlx::query_as("SELECT followers_url, following_url FROM accounts WHERE id = $1")
            .bind(remote_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(collections, (FOLLOWERS.to_owned(), FOLLOWING.to_owned()));
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(remote_id)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE account_stats SET following_count = $2 WHERE account_id = $1")
        .bind(ALICE)
        .bind(baseline_following)
        .execute(&pool)
        .await?;
    reset().await?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn private_announce_writer_uses_fresh_local_followers_audience()
-> Result<(), Box<dyn std::error::Error>> {
    private_announce_local_audience(false).await
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
async fn private_announce_worker_uses_fresh_local_followers_audience()
-> Result<(), Box<dyn std::error::Error>> {
    private_announce_local_audience(true).await
}

#[cfg(feature = "test-support")]
#[allow(clippy::too_many_lines)]
async fn private_announce_local_audience(worker: bool) -> Result<(), Box<dyn std::error::Error>> {
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const TARGET: i64 = -331;
    const FOLLOWER_INBOX: &str = "http://announce-follower.fixture.invalid/inbox";
    let owner_url = std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?;
    let runtime_url = std::env::var("RUSTODON_WORKER_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&owner_url)
        .await?;
    let runtime = PgPoolOptions::new()
        .max_connections(8)
        .connect(&runtime_url)
        .await?;
    let writer = WriteRepository::connect(&owner_url).await?;
    let repository = Repository::connect(&owner_url).await?;
    for id_scheme in [None, Some(0_i32)] {
        reset().await?;
        let username = format!(
            "announce_{}_{}",
            if worker { "worker" } else { "writer" },
            id_scheme.unwrap_or(1)
        );
        let created = writer
            .create_local_user(
                &format!("{username}@fixture.invalid"),
                &username,
                "fixture-audience-password",
            )
            .await?;
        let local = repository
            .account(created.account_id)
            .await?
            .ok_or("new local account missing")?;
        assert!(
            local.followers_url.is_empty(),
            "ordinary local creation must exercise the empty schema default"
        );
        if let Some(id_scheme) = id_scheme {
            // Restored local collection values are not authoritative either.
            sqlx::query("UPDATE accounts SET id_scheme = $2, followers_url = 'https://stale.fixture.invalid/wrong-collection' WHERE id = $1")
                .bind(created.account_id).bind(id_scheme).execute(&pool).await?;
        }
        let local = repository
            .account(created.account_id)
            .await?
            .ok_or("new local account missing")?;
        let actor_uri = activitypub::actor_url(&Url::parse(ORIGIN)?, &local);
        let followers_uri = format!("{actor_uri}/followers");
        let token = format!("fixture-audience-{username}");
        sqlx::query("INSERT INTO oauth_access_tokens (resource_owner_id, token, scopes, created_at) VALUES ($1, $2, 'write:statuses', clock_timestamp())")
            .bind(created.user_id).bind(&token).execute(&pool).await?;
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
        let authenticated = BearerAuthenticator::new(repository.clone())
            .authenticate(&headers, WRITE_STATUSES)
            .await?;
        let follower: i64 = sqlx::query_scalar("INSERT INTO accounts (username, domain, uri, inbox_url, shared_inbox_url, protocol, created_at, updated_at) VALUES ($1, 'announce-follower.fixture.invalid', $2, $3, $3, 1, clock_timestamp(), clock_timestamp()) RETURNING id")
            .bind(&username).bind(format!("http://announce-follower.fixture.invalid/users/{username}")).bind(FOLLOWER_INBOX).fetch_one(&pool).await?;
        sqlx::query("INSERT INTO follows (account_id, target_account_id, show_reblogs, notify, created_at, updated_at) VALUES ($1, $2, true, false, clock_timestamp(), clock_timestamp())")
            .bind(follower).bind(created.account_id).execute(&pool).await?;
        let target_uri =
            format!("https://remote.fixture.invalid/users/exclusive_author/statuses/{username}");
        let target_id: i64 = sqlx::query_scalar("INSERT INTO statuses (account_id, text, spoiler_text, visibility, local, uri, sensitive, reply, created_at, updated_at) VALUES ($1, 'Audience target', '', 0, false, $2, false, false, clock_timestamp(), clock_timestamp()) RETURNING id")
            .bind(TARGET).bind(&target_uri).fetch_one(&pool).await?;
        sqlx::query("INSERT INTO status_stats (status_id, created_at, updated_at) VALUES ($1, clock_timestamp(), clock_timestamp())")
            .bind(target_id).execute(&pool).await?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = listener.local_addr()?;
        let queue = Queue::new(runtime.clone());
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(pool.clone()),
            None,
            Some(ActivityPubDeliveryConfig {
                origin: Url::parse(ORIGIN)?,
                local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
                media_root_url: "/system".to_owned(),
                media_root: None,
                limited_federation: false,
                remote_media_endpoint: None,
                remote_delivery_endpoint: Some(endpoint),
                remote_fetch_endpoint: None,
            }),
        )?;
        let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
        let boost = writer
            .set_reblog_with_origin(
                &authenticated,
                target_id,
                Some("private"),
                true,
                Some(ORIGIN),
                false,
            )
            .await?;
        assert!(boost.created);
        let writer_body: Value = sqlx::query_scalar("SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events WHERE kind = $1 AND payload -> 'arguments' -> 'body' ->> 'type' = 'Announce'")
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND).fetch_one(&pool).await?;
        assert!(queue.dispatch_outbox(100).await? >= 1);
        // Distribution is recorded before the writer's author delivery.
        assert!(
            executor
                .process_one(
                    "announce-audience-distribution",
                    &[Lane::Push],
                    Duration::seconds(30)
                )
                .await?
        );
        let worker_body: Value = sqlx::query_scalar("SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events WHERE kind = $1 AND payload -> 'arguments' ->> 'inbox_url' = $2")
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND).bind(FOLLOWER_INBOX).fetch_one(&pool).await?;
        let body = if worker { &worker_body } else { &writer_body };
        assert_eq!(body["type"], "Announce");
        assert_eq!(body["actor"], actor_uri);
        assert_eq!(body["object"], target_uri);
        assert_eq!(
            body["to"],
            json!([followers_uri]),
            "private Announce must address the canonical LOCAL followers collection"
        );
        assert_eq!(
            body["cc"],
            json!(["https://remote.fixture.invalid/users/exclusive_author"])
        );
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await?;
                requests.push(fixture_delivery_request(&mut socket).await?);
                socket
                    .write_all(
                        b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await?;
            }
            Ok::<_, std::io::Error>(requests)
        });
        queue.dispatch_outbox(100).await?;
        for _ in 0..2 {
            assert!(
                executor
                    .process_one(
                        "announce-audience-delivery",
                        &[Lane::Push],
                        Duration::seconds(30)
                    )
                    .await?
            );
        }
        let requests = tokio::time::timeout(std::time::Duration::from_secs(5), server).await???;
        let follower_request = requests
            .iter()
            .find(|request| {
                String::from_utf8_lossy(request)
                    .to_ascii_lowercase()
                    .contains("host: announce-follower.fixture.invalid")
            })
            .ok_or("shared-inbox delivery missing")?;
        let body_start = follower_request
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .ok_or("missing HTTP headers")?
            + 4;
        let delivered: Value = serde_json::from_slice(&follower_request[body_start..])?;
        assert_eq!(
            delivered, worker_body,
            "actual shared-inbox wire body must match distribution"
        );
        sqlx::query("DELETE FROM accounts WHERE id = ANY($1)")
            .bind(vec![created.account_id, follower])
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id = $1")
            .bind(target_id)
            .execute(&pool)
            .await?;
        reset().await?;
    }
    Ok(())
}
