use super::*;
use crate::mastodon::local_uploads::{RawInput, accept_in, stage_in};
use crate::paperclip::{PaperclipCommitFault, PaperclipWriteFault};
use chrono::Utc;
use std::path::PathBuf;
use tokio::sync::oneshot;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Fixture {
    pool: PgPool,
    root: PaperclipRoot,
    directory: PathBuf,
    account: i64,
    ids: Vec<UploadIdentity>,
}

impl Fixture {
    async fn new() -> Self {
        let pool = PgPool::connect(&std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let mut connection = pool.acquire().await.unwrap();
        crate::operational_schema::migrate(&mut connection)
            .await
            .unwrap();
        let account = sqlx::query_scalar("SELECT a.id FROM accounts a JOIN users u ON u.account_id = a.id WHERE a.id > 0 AND a.domain IS NULL AND a.suspended_at IS NULL AND u.confirmed_at IS NOT NULL AND u.approved AND NOT u.disabled ORDER BY a.id LIMIT 1")
            .fetch_one(&pool).await.unwrap();
        let directory = std::env::temp_dir().join(format!(
            "rustodon-local-upload-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let root = PaperclipRoot::open(&directory).unwrap();
        Self {
            pool,
            root,
            directory,
            account,
            ids: Vec::new(),
        }
    }

    async fn stage(&mut self, mime: &str, bytes: &[u8], accepted: bool) -> UploadIdentity {
        let mut tx = self.pool.begin().await.unwrap();
        let id = stage_in(
            &mut tx,
            self.account,
            1,
            &RawInput {
                mime,
                size: bytes.len().try_into().unwrap(),
                sha256: &Sha256::digest(bytes).into(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        self.ids.push(id);
        self.root
            .private_upload_root()
            .unwrap()
            .write_file(Path::new(&raw_path(id)), bytes)
            .unwrap();
        if accepted {
            let mut tx = self.pool.begin().await.unwrap();
            accept_in(&mut tx, id, &processing_job(id)).await.unwrap();
            tx.commit().await.unwrap();
        }
        id
    }

    async fn state(&self, id: UploadIdentity) -> Option<UploadState> {
        let mut tx = self.pool.begin().await.unwrap();
        load_in(&mut tx, id).await.unwrap()
    }

    async fn age(&self, id: UploadIdentity) {
        sqlx::query("UPDATE rustodon.local_uploads SET created_at = clock_timestamp() - interval '2 hours' WHERE media_id = $1")
            .bind(id.media_id).execute(&self.pool).await.unwrap();
    }

    async fn cleanup(self) {
        for id in &self.ids {
            sqlx::query("DELETE FROM media_attachments WHERE id = $1")
                .bind(id.media_id)
                .execute(&self.pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM rustodon.local_uploads WHERE media_id = $1")
                .bind(id.media_id)
                .execute(&self.pool)
                .await
                .unwrap();
            for table in ["outbox_events", "durable_jobs"] {
                sqlx::query(&format!(
                    "DELETE FROM rustodon.{table} WHERE kind = $1 AND logical_key = $2"
                ))
                .bind(PROCESS_KIND)
                .bind(processing_job(*id).logical_key_value())
                .execute(&self.pool)
                .await
                .unwrap();
            }
        }
        std::fs::remove_dir_all(&self.directory).unwrap();
        self.pool.close().await;
    }
}

fn job(id: UploadIdentity) -> ClaimedJob {
    ClaimedJob {
        id: 1,
        lane: Lane::Maintenance,
        kind: PROCESS_KIND.into(),
        arguments: processing_job(id).arguments().clone(),
        logical_key: processing_job(id).logical_key_value().map(str::to_owned),
        run_at: Utc::now(),
        attempt: 1,
        max_attempts: 4,
        generation: 1,
        lease_owner: "upload-test".into(),
        lease_expires_at: Utc::now() + chrono::Duration::minutes(5),
    }
}

fn image() -> Vec<u8> {
    include_bytes!("../../../tests/fixtures/media/emojo.png").to_vec()
}

async fn scan(f: &Fixture) {
    recover(
        f.pool.clone(),
        Queue::new(f.pool.clone()),
        f.root.clone(),
        &json!({}),
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and real ffmpeg/ffprobe"]
async fn registered_worker_real_image_video_audio_retire_raw_only() -> TestResult {
    let mut f = Fixture::new().await;
    let handlers = HandlerRegistry::new();
    register(
        &handlers,
        f.pool.clone(),
        Queue::new(f.pool.clone()),
        f.root.clone(),
    )?;
    let handler = handlers.get(PROCESS_KIND)?.unwrap();
    assert!(matches!(handler.resource, ResourceClass::Media));
    assert_eq!(handler.lane, Lane::Maintenance);
    let queue = Queue::new(f.pool.clone());
    let executor = super::super::WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    for (name, mime, expected_type, count) in [
        ("emojo.png", "image/png", 0, 2),
        ("attachment.webm", "video/webm", 2, 2),
        ("boop.ogg", "audio/ogg", 4, 1),
    ] {
        let bytes = std::fs::read(format!("tests/fixtures/media/{name}"))?;
        let id = f.stage(mime, &bytes, true).await;
        sqlx::query("UPDATE media_attachments SET description = 'latest', file_meta = '{\"focus\":{\"x\":0.25,\"y\":-0.5}}'::json WHERE id = $1")
            .bind(id.media_id).execute(&f.pool).await?;
        assert_eq!(queue.dispatch_outbox(100).await?, 1);
        assert!(
            executor
                .process_one(
                    "upload-real",
                    &[Lane::Maintenance],
                    chrono::Duration::minutes(5)
                )
                .await?
        );
        queue.enqueue(&processing_job(id)).await?;
        assert!(
            executor
                .process_one(
                    "upload-replay",
                    &[Lane::Maintenance],
                    chrono::Duration::minutes(5)
                )
                .await?
        );
        assert!(f.state(id).await.is_none());
        assert!(
            f.root
                .private_upload_root()?
                .open_file(Path::new(&raw_path(id)))
                .is_err()
        );
        let row: (i32, String, String, String, serde_json::Value) = sqlx::query_as("SELECT type, file_file_name, file_content_type, description, file_meta FROM media_attachments WHERE id = $1 AND processing = 2")
            .bind(id.media_id).fetch_one(&f.pool).await?;
        assert_eq!(row.0, expected_type);
        assert_eq!(row.3, "latest");
        assert_eq!(row.4["focus"], json!({"x":0.25,"y":-0.5}));
        let metadata = PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: id.media_id,
            remote: false,
            storage_schema_version: Some(1),
            file_name: row.1,
            content_type: Some(row.2),
            variant: None,
        };
        let paths: Vec<_> = ["original", "small"]
            .into_iter()
            .filter_map(|s| metadata.relative_path(s))
            .collect();
        assert_eq!(paths.len(), count);
        for path in paths {
            assert!(f.root.open_file(Path::new(&path))?.metadata()?.len() > 0);
        }
    }
    f.cleanup().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn commit_ambiguity_and_partial_output_replay_preserve_published_bytes() -> TestResult {
    let mut f = Fixture::new().await;
    let id = f.stage("image/png", &image(), true).await;
    let failing = f
        .root
        .clone()
        .with_write_fault(PaperclipWriteFault::storage_full_after(1));
    assert!(process(f.pool.clone(), failing, job(id)).await.is_err());
    let state = f.state(id).await.unwrap();
    assert_eq!(
        state.output_paths.len(),
        2,
        "manifest committed before attempted writes"
    );
    assert_eq!(public_state(&f.pool, id).await?.unwrap().0, Some(1));
    let faults = f
        .root
        .clone()
        .with_commit_fault(PaperclipCommitFault::before_and_after());
    assert!(
        process(f.pool.clone(), faults.clone(), job(id))
            .await
            .is_err()
    );
    assert_eq!(public_state(&f.pool, id).await?.unwrap().0, Some(1));
    assert!(process(f.pool.clone(), faults, job(id)).await.is_err());
    assert_eq!(public_state(&f.pool, id).await?.unwrap().0, Some(2));
    let original: Vec<_> = state
        .output_paths
        .iter()
        .map(|p| std::fs::read(f.directory.join(p)).unwrap())
        .collect();
    process_with(f.pool.clone(), f.root.clone(), job(id), |_, _, _| async {
        panic!("ready replay must not process or rewrite output")
    })
    .await
    .unwrap();
    for (path, expected) in state.output_paths.iter().zip(original) {
        assert_eq!(std::fs::read(f.directory.join(path))?, expected);
    }
    assert!(f.state(id).await.is_none());
    f.cleanup().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn recovery_exact_ownership_raw_validation_and_legacy_staging_guard() -> TestResult {
    let mut f = Fixture::new().await;
    let id = f.stage("image/png", &image(), false).await;
    f.age(id).await;
    let accepted = f.stage("image/png", &image(), true).await;
    process_with(
        f.pool.clone(),
        f.root.clone(),
        job(UploadIdentity {
            generation: 2,
            ..accepted
        }),
        |_, _, _| async { panic!("stale generation must not process") },
    )
    .await
    .unwrap();
    let mut wrong_hash = f.state(accepted).await.unwrap();
    wrong_hash.raw_sha256 = vec![0; 32];
    assert!(read_raw(&f.root.private_upload_root()?, &wrong_hash).is_err());
    // Integrity mismatch fails before any processor/output work.
    let raw = f.root.private_upload_root()?;
    raw.remove_file(Path::new(&raw_path(accepted)))?;
    raw.write_file(Path::new(&raw_path(accepted)), b"wrong")?;
    assert!(
        process_with(
            f.pool.clone(),
            f.root.clone(),
            job(accepted),
            |_, _, _| async { panic!("corrupt input must not reach processor") }
        )
        .await
        .is_err()
    );
    assert!(f.state(accepted).await.unwrap().output_paths.is_empty());
    f.age(accepted).await;
    scan(&f).await;
    assert!(f.state(id).await.is_none());
    assert!(
        f.state(accepted).await.is_some(),
        "undispatched intent must survive recovery"
    );
    sqlx::query("UPDATE rustodon.outbox_events SET dispatched_at = clock_timestamp() WHERE kind = $1 AND logical_key = $2")
        .bind(PROCESS_KIND).bind(processing_job(accepted).logical_key_value()).execute(&f.pool).await?;
    scan(&f).await;
    assert!(
        f.state(accepted).await.is_none(),
        "exhausted/lost job cleanup"
    );
    assert!(raw.open_file(Path::new(&raw_path(accepted))).is_err());
    let guarded = f.stage("image/png", &image(), false).await;
    // Use a real metadata-derived path, not the private raw path.
    let meta = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: guarded.media_id,
        remote: false,
        storage_schema_version: Some(1),
        file_name: "test.png".into(),
        content_type: Some("image/png".into()),
        variant: None,
    };
    let path = meta.relative_path("original").unwrap();
    super::super::process_local_media_cleanup_job(f.pool.clone(), f.root.clone(), &json!({"account_id": f.account,"media_id":guarded.media_id,"action":"rollback_create","paths":[path]})).await.unwrap();
    assert!(public_state(&f.pool, guarded).await?.is_some());
    assert!(parse_paperclip_path(&format!(".local-upload-input/{}", raw_path(guarded))).is_none());
    use std::os::unix::fs::{PermissionsExt, symlink};
    assert_eq!(
        std::fs::metadata(f.directory.join(".local-upload-input"))?
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    raw.remove_file(Path::new(&raw_path(guarded)))?;
    symlink(
        "/etc/passwd",
        f.directory
            .join(".local-upload-input")
            .join(raw_path(guarded)),
    )?;
    assert!(read_raw(&raw, &f.state(guarded).await.unwrap()).is_err());
    f.cleanup().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn processing_releases_lock_and_rejects_stale_deleted_disabled_and_cancelled_work()
-> TestResult {
    let mut f = Fixture::new().await;
    for action in ["stale", "deleted", "disabled", "cancelled", "edit"] {
        let id = f.stage("image/png", &image(), true).await;
        let (started, start) = oneshot::channel();
        let (finish, finished) = oneshot::channel();
        let pool = f.pool.clone();
        let root = f.root.clone();
        let task = tokio::spawn(async move {
            process_with(pool, root, job(id), |id, mime, bytes| async move {
                started.send(()).unwrap();
                finished.await.unwrap();
                prepare_rich_media_attachment(id.account_id, "upload", &mime, &bytes).await
            })
            .await
        });
        start.await?;
        let writer = WriteRepository::from_pool(f.pool.clone());
        tokio::time::timeout(std::time::Duration::from_secs(3), writer.with_account_lock(id.account_id, || async {
            let mut tx = f.pool.begin().await?;
            match action {
                "stale" => { claim_in(&mut tx, id).await?; }
                "deleted" => { sqlx::query("DELETE FROM media_attachments WHERE id = $1").bind(id.media_id).execute(&mut *tx).await?; }
                "disabled" => { sqlx::query("UPDATE users SET disabled = true WHERE account_id = $1").bind(id.account_id).execute(&mut *tx).await?; }
                "edit" => { sqlx::query("UPDATE media_attachments SET description = 'edited during processing', file_meta = '{\"focus\":{\"x\":1,\"y\":0}}'::json WHERE id = $1").bind(id.media_id).execute(&mut *tx).await?; }
                _ => {}
            }
            tx.commit().await?;
            Ok(())
        })).await??;
        if action == "cancelled" {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert!(f.state(id).await.unwrap().output_paths.is_empty());
            process(f.pool.clone(), f.root.clone(), job(id))
                .await
                .unwrap();
        } else {
            finish.send(()).unwrap();
            let result = task.await?;
            if matches!(action, "stale" | "disabled") {
                assert!(result.is_err());
                assert!(f.state(id).await.unwrap().output_paths.is_empty());
            } else {
                result.unwrap();
            }
            if action == "disabled" {
                sqlx::query("UPDATE users SET disabled = false WHERE account_id = $1")
                    .bind(id.account_id)
                    .execute(&f.pool)
                    .await?;
            }
            if action == "edit" {
                let row: (String, serde_json::Value) = sqlx::query_as(
                    "SELECT description, file_meta FROM media_attachments WHERE id = $1",
                )
                .bind(id.media_id)
                .fetch_one(&f.pool)
                .await?;
                assert_eq!(row.0, "edited during processing");
                assert_eq!(row.1["focus"], json!({"x":1,"y":0}));
            }
        }
    }
    f.cleanup().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn durable_queue_retry_exhaustion_and_raw_retirement_failure() -> TestResult {
    use crate::jobs::RetryResult;
    use crate::paperclip::{MediaAttachmentError, PaperclipRemoveFault};
    let mut f = Fixture::new().await;
    let id = f.stage("image/png", &image(), true).await;
    f.age(id).await;
    let queue = Queue::new(f.pool.clone());
    queue.dispatch_outbox(100).await?;
    for attempt in 1..=4 {
        let claimed = queue
            .claim(
                "upload-test",
                &[Lane::Maintenance],
                chrono::Duration::minutes(5),
            )
            .await?
            .unwrap();
        assert_eq!(claimed.arguments["media_id"], id.media_id);
        assert_eq!(claimed.attempt, attempt);
        assert!(
            process_with(
                f.pool.clone(),
                f.root.clone(),
                claimed.clone(),
                |_, _, _| async { Err(MediaAttachmentError::ProcessingUnavailable) }
            )
            .await
            .is_err()
        );
        scan(&f).await;
        assert!(f.state(id).await.is_some(), "active job retained");
        let result = queue
            .retry(&claimed, Utc::now(), "injected unavailable processor")
            .await?;
        assert_eq!(
            result,
            if attempt == 4 {
                RetryResult::Dead
            } else {
                RetryResult::Scheduled
            }
        );
    }
    scan(&f).await;
    assert!(f.state(id).await.is_none());
    assert_eq!(public_state(&f.pool, id).await?, Some((Some(3), None)));
    let ready = f.stage("image/png", &image(), true).await;
    // Two pending output removals occur before installation, then raw unlink fails.
    // Exercise raw retirement in isolation after an ambiguous acknowledged publication.
    let faults = f
        .root
        .clone()
        .with_commit_fault(PaperclipCommitFault::before_and_after());
    assert!(
        process(f.pool.clone(), faults.clone(), job(ready))
            .await
            .is_err()
    );
    assert!(process(f.pool.clone(), faults, job(ready)).await.is_err());
    let state = f.state(ready).await.unwrap();
    assert_eq!(public_state(&f.pool, ready).await?.unwrap().0, Some(2));
    let root = f
        .root
        .clone()
        .with_remove_fault(PaperclipRemoveFault::fail_once());
    assert!(process(f.pool.clone(), root, job(ready)).await.is_err());
    assert!(f.state(ready).await.is_some());
    for path in &state.output_paths {
        assert!(f.root.open_file(Path::new(path)).is_ok());
    }
    scan(&f).await;
    assert!(f.state(ready).await.is_none());
    for path in &state.output_paths {
        assert!(f.root.open_file(Path::new(path)).is_ok());
    }
    f.cleanup().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn recovery_batch_bound_and_failed_owner_do_not_starve_later_rows() -> TestResult {
    use crate::paperclip::PaperclipRemoveFault;
    let mut f = Fixture::new().await;
    for _ in 0..101 {
        let id = f.stage("image/png", &image(), false).await;
        f.age(id).await;
    }
    let first = f.ids[0];
    let cursor = f.ids[99].media_id;
    let last = f.ids[100];
    let queue = Queue::new(f.pool.clone());
    assert!(
        recover(
            f.pool.clone(),
            queue.clone(),
            f.root
                .clone()
                .with_remove_fault(PaperclipRemoveFault::fail_once()),
            &json!({})
        )
        .await
        .is_err()
    );
    assert!(
        f.state(first).await.is_some(),
        "failed unlink retains exact orphan owner"
    );
    assert!(f.state(f.ids[99]).await.is_none());
    assert!(
        f.state(last).await.is_some(),
        "first scan is bounded to 100 rows"
    );
    let continuation: serde_json::Value = sqlx::query_scalar(
        "SELECT arguments FROM rustodon.durable_jobs WHERE kind = $1 AND arguments->>'after' = $2",
    )
    .bind(RECOVER_KIND)
    .bind(cursor.to_string())
    .fetch_one(&f.pool)
    .await?;
    recover(f.pool.clone(), queue, f.root.clone(), &continuation)
        .await
        .unwrap();
    assert!(f.state(last).await.is_none());
    scan(&f).await;
    assert!(f.state(first).await.is_none());
    sqlx::query("DELETE FROM rustodon.durable_jobs WHERE kind = $1 AND arguments->>'after' = $2")
        .bind(RECOVER_KIND)
        .bind(cursor.to_string())
        .execute(&f.pool)
        .await?;
    f.cleanup().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn recovery_scheduler_second_tick_reuses_live_root() -> TestResult {
    use crate::jobs::KindScheduleOwnership;
    let f = Fixture::new().await;
    let queue = Queue::new(f.pool.clone());
    let first = schedule_recovery(&queue).await?;
    assert!(matches!(first, KindScheduleOwnership::Acquired { .. }));
    // Call the same path as the maintenance tick again, without completing root.
    let second = schedule_recovery(&queue).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 AND dead_at IS NULL",
    )
    .bind(RECOVER_KIND)
    .fetch_one(&f.pool)
    .await?;
    sqlx::query("DELETE FROM rustodon.durable_jobs WHERE id = $1")
        .bind(first.job_id())
        .execute(&f.pool)
        .await?;
    f.cleanup().await;
    assert_eq!(count, 1);
    assert_eq!(
        second?,
        KindScheduleOwnership::Existing {
            job_id: first.job_id(),
            logical_key: "local-upload-recovery:root".into(),
            arguments: first.arguments().clone(),
        }
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable restored PostgreSQL fixture"]
async fn local_upload_readiness_requires_registered_maintenance_worker() -> TestResult {
    use crate::jobs::WorkerHeartbeat;
    let f = Fixture::new().await;
    let queue = Queue::new(f.pool.clone());
    let id = "local-upload-readiness-test";
    let freshness = chrono::Duration::seconds(30);
    queue
        .heartbeat(&WorkerHeartbeat::worker(
            id,
            [Lane::Maintenance],
            json!({"concurrency":1}),
        ))
        .await?;
    assert!(
        !ready(&queue, freshness).await?,
        "generic Maintenance cannot process uploads"
    );
    queue
        .heartbeat(&WorkerHeartbeat::worker(
            id,
            [Lane::Core],
            json!({"local_uploads":true}),
        ))
        .await?;
    assert!(
        !ready(&queue, freshness).await?,
        "handler outside the selected lane is insufficient"
    );
    queue
        .heartbeat(&WorkerHeartbeat::worker(
            id,
            [Lane::Maintenance],
            json!({"local_uploads":true}),
        ))
        .await?;
    assert!(ready(&queue, freshness).await?);
    sqlx::query("UPDATE rustodon.heartbeats SET started_at = clock_timestamp() - interval '2 minutes', heartbeat_at = clock_timestamp() - interval '1 minute' WHERE process_id = $1").bind(id).execute(&f.pool).await?;
    assert!(!ready(&queue, freshness).await?);
    queue.remove_heartbeat(id).await?;
    f.cleanup().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable restored PostgreSQL fixture"]
async fn terminal_failure_unlink_retry_retains_pollable_error_and_fences_replay() -> TestResult {
    use crate::paperclip::{MediaAttachmentError, PaperclipRemoveFault};
    let mut f = Fixture::new().await;
    let id = f.stage("image/avif", b"invalid bytes", true).await;
    let root = f
        .root
        .clone()
        .with_remove_fault(PaperclipRemoveFault::fail_once());
    assert!(
        process_with(f.pool.clone(), root, job(id), |_, _, _| async {
            Err(MediaAttachmentError::InvalidMedia)
        })
        .await
        .is_err()
    );
    assert_eq!(public_state(&f.pool, id).await?, Some((Some(3), None)));
    assert!(
        f.state(id).await.is_some(),
        "unlink error must retain cleanup ownership"
    );
    process_with(f.pool.clone(), f.root.clone(), job(id), |_, _, _| async {
        panic!("failed replay must not decode or publish")
    })
    .await
    .unwrap();
    assert_eq!(public_state(&f.pool, id).await?, Some((Some(3), None)));
    assert!(f.state(id).await.is_none());
    assert!(
        f.root
            .private_upload_root()?
            .open_file(Path::new(&raw_path(id)))
            .is_err()
    );
    f.cleanup().await;
    Ok(())
}
