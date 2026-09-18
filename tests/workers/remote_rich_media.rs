//! Focused adversarial remote-worker cases; owner is fixture/assertion-only.
use super::*;

#[tokio::test]
#[ignore = "requires disposable restored PG14 and native ffmpeg/ffprobe"]
async fn activitypub_media_fetch_rich_rejections_and_install_fences()
-> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(
        std::time::Duration::from_mins(1),
        rich_rejections(&["mismatch", "malformed", "retry", "deleted", "policy"]),
    )
    .await?
}

#[tokio::test]
#[ignore = "requires disposable restored PG14 and native ffmpeg/ffprobe"]
async fn activitypub_media_fetch_current_focus_during_fetch()
-> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(
        std::time::Duration::from_mins(1),
        rich_rejections(&["focus-edit", "focus-clear"]),
    )
    .await?
}

#[tokio::test]
#[ignore = "requires disposable restored PG14 and native ffmpeg/ffprobe"]
async fn activitypub_media_fetch_cancel_during_stream_flush()
-> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(
        std::time::Duration::from_mins(1),
        rich_rejections(&["cancel-flush"]),
    )
    .await?
}

#[allow(clippy::too_many_lines)]
async fn rich_rejections(cases: &[&'static str]) -> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let mut roles = BTreeSet::new();
    for pool in [&owner, &runtime, &writer] {
        roles.insert(
            sqlx::query_scalar::<_, String>("SELECT current_user::text")
                .fetch_one(pool)
                .await?,
        );
    }
    assert_eq!(roles.len(), 3);
    for pool in [&runtime, &writer] {
        assert!(!sqlx::query_scalar::<_, bool>("SELECT rolsuper OR rolcreaterole OR rolcreatedb OR rolbypassrls OR EXISTS(SELECT 1 FROM pg_auth_members WHERE member=oid) FROM pg_roles WHERE rolname=current_user").fetch_one(pool).await?);
    }
    let bytes = fs::read("tests/fixtures/media/capability.mp4")?;
    for &case in cases {
        reset().await?;
        let status: i64 = sqlx::query_scalar("INSERT INTO statuses (account_id,text,spoiler_text,visibility,local,sensitive,reply,created_at,updated_at) VALUES (116844606259202001,'rich fence','',0,false,false,false,clock_timestamp(),clock_timestamp()) RETURNING id").fetch_one(&owner).await?;
        let root_path =
            std::env::temp_dir().join(format!("remote-rich-{case}-{}", std::process::id()));
        fs::create_dir(&root_path)?;
        let root = PaperclipRoot::open(&root_path)?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = listener.local_addr()?;
        let remote_url = format!("http://media.fixture.invalid:{}/{case}", endpoint.port());
        let media: i64 = sqlx::query_scalar("INSERT INTO media_attachments (account_id,status_id,type,processing,remote_url,file_content_type,file_meta,created_at,updated_at) VALUES (116844606259202001,$1,0,0,$2,'video/mp4','{\"focus\":{\"x\":0.1,\"y\":0.2}}',clock_timestamp(),clock_timestamp()) RETURNING id")
            .bind(status).bind(&remote_url).fetch_one(&owner).await?;
        let queue = Queue::new(runtime.clone());
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(writer.clone()),
            None,
            Some(ActivityPubDeliveryConfig {
                origin: Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
                local_domain: "fixture-v4-6-5.rustodon.invalid".into(),
                media_root_url: "/system".into(),
                media_root: Some(root.clone()),
                limited_federation: false,
                remote_media_endpoint: Some(endpoint),
                remote_delivery_endpoint: None,
                remote_fetch_endpoint: None,
            }),
        )?;
        let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Pull,
                    ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
                    json!({"media_id":media}),
                )
                .logical_key(format!("rich:{case}")),
            )
            .await?;
        let started = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let server_started = started.clone();
        let server_released = released.clone();
        let body = if case == "malformed" {
            b"not an mp4".to_vec()
        } else {
            bytes.clone()
        };
        let server = tokio::spawn(async move {
            for attempt in 0..if case == "retry" { 2 } else { 1 } {
                let (mut socket, _) = listener.accept().await?;
                fixture_delivery_request(&mut socket).await?;
                if attempt == 0 {
                    server_started.notify_one();
                    server_released.notified().await;
                }
                let status = if case == "retry" && attempt == 0 {
                    "503 Service Unavailable"
                } else {
                    "200 OK"
                };
                let mime = if case == "mismatch" {
                    "image/png"
                } else {
                    "video/mp4"
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(header.as_bytes()).await?;
                // Mismatch is rejected before body download.
                let _ = socket.write_all(&body).await;
            }
            Ok::<(), std::io::Error>(())
        });
        // Hold only the existing stream-order lock: the worker must reach the
        // awaited flush with originals/poster written, but without issuing COMMIT.
        let flush_barrier = if case == "cancel-flush" {
            let mut transaction = owner.begin().await?;
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *transaction)
                .await?;
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('rustodon.mastodon.stream_event.commit_order', 0))")
                .execute(&mut *transaction).await?;
            Some((transaction, pid))
        } else {
            None
        };
        let first = executor.clone();
        let work = tokio::spawn(async move {
            first
                .process_one("rich-fence", &[Lane::Pull], Duration::seconds(30))
                .await
        });
        started.notified().await;
        match case {
            "focus-edit" | "focus-clear" => {
                let mut meta =
                    json!({"original":{"width":9999,"height":1},"small":{"width":9999,"height":1}});
                if case == "focus-edit" {
                    meta["focus"] = json!({"x":0.7,"y":-0.4});
                }
                // Same row mutation as an in-flight Note update, using the narrow writer.
                sqlx::query("UPDATE media_attachments SET file_meta=$2::json WHERE id=$1")
                    .bind(media)
                    .bind(meta)
                    .execute(&writer)
                    .await?;
            }
            "deleted" => {
                sqlx::query("UPDATE statuses SET deleted_at=clock_timestamp() WHERE id=$1")
                    .bind(status)
                    .execute(&owner)
                    .await?;
            }
            "policy" => {
                sqlx::query("INSERT INTO domain_blocks (domain,severity,reject_media,reject_reports,obfuscate,created_at,updated_at) VALUES ('media.fixture.invalid',0,true,false,false,clock_timestamp(),clock_timestamp())").execute(&owner).await?;
            }
            _ => {}
        }
        released.notify_one();
        if let Some((barrier, pid)) = flush_barrier {
            let prepared = rustodon::paperclip::prepare_rich_media_attachment(
                116_844_606_259_202_001,
                case,
                "video/mp4",
                &bytes,
            )
            .await?;
            let metadata = PaperclipMetadata {
                attachment: PaperclipAttachment::MediaFile,
                id: media,
                remote: true,
                storage_schema_version: Some(1),
                file_name: prepared.file_name,
                content_type: Some(prepared.content_type),
                variant: None,
            };
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                while !sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) AND wait_event='advisory')")
                    .bind(pid).fetch_one(&owner).await? {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Ok::<(),sqlx::Error>(())
            }).await??;
            for style in ["original", "small"] {
                assert!(
                    root.open_file(Path::new(&metadata.relative_path(style).unwrap()))
                        .is_ok(),
                    "installed output exists while flush waits"
                );
            }
            work.abort();
            assert!(work.await.unwrap_err().is_cancelled());
            for style in ["original", "small"] {
                assert!(
                    root.open_file(Path::new(&metadata.relative_path(style).unwrap()))
                        .is_err(),
                    "cancel before COMMIT must clean {style}"
                );
            }
            barrier.rollback().await?;
            // Synchronize with the cancelled connection's rollback before reading state.
            let mut probe = owner.begin().await?;
            sqlx::query("SELECT id FROM media_attachments WHERE id=$1 FOR UPDATE")
                .bind(media)
                .fetch_one(&mut *probe)
                .await?;
            probe.rollback().await?;
        } else {
            assert!(work.await??);
        }
        if case == "retry" {
            assert_eq!(
                sqlx::query_scalar::<_, i32>(
                    "SELECT processing FROM media_attachments WHERE id=$1"
                )
                .bind(media)
                .fetch_one(&owner)
                .await?,
                0
            );
            sqlx::query("UPDATE rustodon.durable_jobs SET run_at=clock_timestamp() WHERE logical_key='rich:retry'").execute(&runtime).await?;
            assert!(
                executor
                    .process_one("rich-retry", &[Lane::Pull], Duration::seconds(30))
                    .await?
            );
        }
        server.await??;
        let state: (Option<String>, i32) =
            sqlx::query_as("SELECT file_file_name,processing FROM media_attachments WHERE id=$1")
                .bind(media)
                .fetch_one(&owner)
                .await?;
        let events: i64=sqlx::query_scalar("SELECT count(*) FROM rustodon.outbox_events WHERE kind=$1 AND payload->>'object_id'=$2 AND payload->>'event'='status.update'").bind(STREAM_EVENT_KIND).bind(status.to_string()).fetch_one(&owner).await?;
        if matches!(case, "retry" | "focus-edit" | "focus-clear") {
            assert!(state.0.is_some());
            assert_eq!(state.1, 2);
            assert!(events > 0);
            if matches!(case, "focus-edit" | "focus-clear") {
                let meta: Value =
                    sqlx::query_scalar("SELECT file_meta FROM media_attachments WHERE id=$1")
                        .bind(media)
                        .fetch_one(&owner)
                        .await?;
                let prepared = rustodon::paperclip::prepare_rich_media_attachment(
                    116_844_606_259_202_001,
                    case,
                    "video/mp4",
                    &bytes,
                )
                .await?;
                assert_eq!(meta["original"], prepared.file_meta["original"]);
                assert_eq!(meta["small"], prepared.file_meta["small"]);
                if case == "focus-edit" {
                    assert_eq!(meta["focus"], json!({"x":0.7,"y":-0.4}));
                } else {
                    assert!(meta.get("focus").is_none(), "cleared focus must not return");
                }
            }
        } else if case == "cancel-flush" {
            assert!(state.0.is_none());
            assert_eq!(
                state.1, 1,
                "claim remains reclaimable, installation rolled back"
            );
            assert_eq!(events, 0, "cancelled flush must not publish streams");
        } else {
            assert!(state.0.is_none());
            assert_eq!(events, 0);
            assert_eq!(
                fs::read_dir(&root_path)?.count(),
                0,
                "no writes before rejected install"
            );
            if case != "deleted" {
                assert_eq!(state.1, 3);
            }
        }
        if case == "policy" {
            sqlx::query("DELETE FROM domain_blocks WHERE domain='media.fixture.invalid'")
                .execute(&owner)
                .await?;
        }
        sqlx::query("DELETE FROM media_attachments WHERE id=$1")
            .bind(media)
            .execute(&owner)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id=$1")
            .bind(status)
            .execute(&owner)
            .await?;
        drop(executor);
        drop(root);
        fs::remove_dir_all(root_path)?;
    }
    Ok(())
}
