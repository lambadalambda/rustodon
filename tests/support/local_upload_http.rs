//! Real local HTTP -> durable acceptance -> held/released worker contract.
//! Runtime/writer URLs are mandatory; owner credentials are setup/assertion-only.
use super::*;
use rustodon::jobs::{Lane, Queue};
use rustodon::mastodon::local_uploads::UploadIdentity;
use rustodon::paperclip::PaperclipRoot;
use rustodon::worker::{
    ActivityPubDeliveryConfig, WorkerExecutor,
    infrastructure_handlers_with_writer_and_mail_and_federation, local_uploads::processing_job,
};

fn upload_body(mime: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = format!("--local-upload-test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload\"\r\nContent-Type: {mime}\r\n\r\n").into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--local-upload-test\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\ninitial description\r\n--local-upload-test\r\nContent-Disposition: form-data; name=\"focus\"\r\n\r\n0.1,0.2\r\n--local-upload-test--\r\n");
    body
}

async fn upload(
    client: &Client,
    base: &str,
    version: u8,
    mime: &str,
    bytes: &[u8],
) -> TestResult<(u16, Value)> {
    let response = client
        .post(format!("{base}/api/v{version}/media"))
        .header("host", DOMAIN)
        .bearer_auth(OWNER_TOKEN)
        .header(
            "content-type",
            "multipart/form-data; boundary=local-upload-test",
        )
        .body(upload_body(mime, bytes))
        .send()
        .await?;
    let status = response.status().as_u16();
    Ok((status, serde_json::from_slice(&response.bytes().await?)?))
}

async fn request(
    client: &Client,
    base: &str,
    method: Method,
    id: i64,
    token: &str,
    body: Option<Value>,
) -> TestResult<(u16, Value)> {
    let mut request = client
        .request(method, format!("{base}/api/v1/media/{id}"))
        .header("host", DOMAIN)
        .bearer_auth(token);
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(body.to_string());
    }
    let response = request.send().await?;
    Ok((
        response.status().as_u16(),
        serde_json::from_slice(&response.bytes().await?)?,
    ))
}

async fn process(queue: &Queue, executor: &WorkerExecutor) -> TestResult {
    queue.dispatch_outbox(100).await?;
    // Only this task's database, with no live scheduler. Drain bounded maintenance
    // work (including legacy rollback intents); no federation lane is executed.
    for _ in 0..100 {
        if !executor
            .process_one(
                "http-local-upload",
                &[Lane::Maintenance],
                chrono::Duration::minutes(5),
            )
            .await?
        {
            return Ok(());
        }
    }
    Err("maintenance drain exceeded fixture bound".into())
}

fn fixture(mime: &str) -> &'static str {
    match mime {
        "image/heic" | "image/heif" => "600x400.heic",
        "image/avif" => "600x400.avif",
        "video/webm" => "attachment.webm",
        "video/mp4" => "capability.mp4",
        "video/quicktime" => "capability.mov",
        "video/ogg" => "capability-video.ogg",
        "audio/wave" | "audio/wav" | "audio/x-wav" | "audio/x-pn-wave" | "audio/vnd.wave" => {
            "capability.wav"
        }
        "audio/ogg" | "audio/vorbis" => "boop.ogg",
        "audio/mpeg" | "audio/mp3" => "capability.mp3",
        "audio/webm" => "capability-audio.webm",
        "audio/flac" => "capability.flac",
        "audio/aac" => "capability.aac",
        "audio/m4a" | "audio/x-m4a" | "audio/mp4" => "capability.m4a",
        "audio/3gpp" => "capability.3gp",
        "video/x-ms-asf" => "capability.asf",
        _ => panic!("missing advertised MIME fixture: {mime}"),
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL fixture and real codecs"]
#[allow(clippy::too_many_lines)]
async fn local_rich_upload_http_lifecycle() -> TestResult {
    let url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
    let pool = PgPool::connect(&url).await?;
    let runtime_pool = PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer_pool =
        PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let writer_name: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&writer_pool)
        .await?;
    let mut connection = pool.acquire().await?;
    rustodon::operational_schema::migrate_with_writer_role(&mut connection, Some(&writer_name))
        .await?;
    drop(connection);
    assert_restricted_roles(&pool, &runtime_pool, &writer_pool).await?;
    let queue = Queue::new(runtime_pool.clone());
    let old_scope: String =
        sqlx::query_scalar("SELECT scopes FROM oauth_access_tokens WHERE token = $1")
            .bind(OTHER_TOKEN)
            .fetch_one(&pool)
            .await?;
    sqlx::query("UPDATE oauth_access_tokens SET scopes = 'write:media' WHERE token = $1")
        .bind(OTHER_TOKEN)
        .execute(&pool)
        .await?;
    let root = std::env::temp_dir().join(format!("rustodon-upload-http-{}", std::process::id()));
    fs::create_dir(&root)?;
    let mut resources = Resources {
        root: root.canonicalize()?,
        server: None,
    };
    let state = WebState::new(
        Repository::from_pool(runtime_pool.clone()),
        Url::parse(&format!("https://{DOMAIN}/"))?,
        DOMAIN,
        "/system",
        &resources.root,
        runtime(),
        Vec::new(),
        vec![DOMAIN.to_owned()],
    )?
    .with_queue(queue.clone())
    .with_write_repository(WriteRepository::from_pool(writer_pool.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    resources.server = Some(tokio::spawn(async move {
        axum::serve(listener, router(state)).await
    }));
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let root = PaperclipRoot::open(&resources.root)?;
    // Same registration as the worker binary, including a local media root and
    // writer, even with federation limited. No remote handler is executed.
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer_pool.clone()),
        None,
        Some(ActivityPubDeliveryConfig {
            origin: Url::parse(&format!("https://{DOMAIN}/"))?,
            local_domain: DOMAIN.into(),
            media_root_url: "/system".into(),
            media_root: Some(root.clone()),
            limited_federation: true,
            remote_media_endpoint: None,
            remote_delivery_endpoint: None,
            remote_fetch_endpoint: None,
        }),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let mut ids = Vec::new();
    // Before the executor runs, acceptance must be externally pollable (not 404).
    for &mime in rustodon::media::ALL_MEDIA_MIME_TYPES {
        let format = rustodon::media::media_format(mime).unwrap();
        if !format.external_processing {
            continue;
        }
        let bytes = fs::read(Path::new("tests/fixtures/media").join(fixture(mime)))?;
        let (status, body) = upload(&client, &base, 2, mime, &bytes).await?;
        assert_eq!(status, 202, "{mime}: {body}");
        let id: i64 = body["id"].as_str().ok_or("missing id")?.parse()?;
        ids.push(id);
        assert!(body["url"].is_null());
        assert!(body["preview_url"].is_null(), "no artifact exists yet");
        let row: (i32, Option<String>, bool) = sqlx::query_as("SELECT m.processing, m.file_file_name, u.accepted FROM media_attachments m JOIN rustodon.local_uploads u ON u.media_id = m.id WHERE m.id = $1").bind(id).fetch_one(&pool).await?;
        assert_eq!(row, (1, None, true));
        let (status, pending) = request(&client, &base, Method::GET, id, OWNER_TOKEN, None).await?;
        assert_eq!(status, 206);
        assert_eq!(pending["id"], body["id"]);
        assert_eq!(pending["description"], "initial description");
        assert_eq!(pending["meta"]["focus"], json!({"x":0.1,"y":0.2}));
        let private = client
            .get(format!(
                "{base}/system/.local-upload-input/local_uploads/{id}/1/input"
            ))
            .header("host", DOMAIN)
            .bearer_auth(OWNER_TOKEN)
            .send()
            .await?;
        assert_eq!(private.status().as_u16(), 404, "raw input is never public");
        for method in [Method::GET, Method::PUT, Method::DELETE] {
            assert_eq!(
                request(
                    &client,
                    &base,
                    method,
                    id,
                    OTHER_TOKEN,
                    Some(json!({"description":"foreign"}))
                )
                .await?
                .0,
                404
            );
        }
        let edit = json!({"description":"latest pending edit", "focus":"0.25,-0.5"});
        assert_eq!(
            request(&client, &base, Method::PUT, id, OWNER_TOKEN, Some(edit))
                .await?
                .0,
            206
        );
        let attach = client
            .post(format!("{base}/api/v1/statuses"))
            .header("host", DOMAIN)
            .bearer_auth(OWNER_TOKEN)
            .header("content-type", "application/json")
            .body(json!({"status":"not ready", "media_ids":[id.to_string()]}).to_string())
            .send()
            .await?;
        assert_eq!(attach.status().as_u16(), 422, "pending cannot attach");
        process(&queue, &executor).await?;
        let (status, ready) = request(&client, &base, Method::GET, id, OWNER_TOKEN, None).await?;
        assert_eq!(status, 200, "{mime}: {ready}");
        assert_eq!(ready["id"], body["id"]);
        assert!(ready["url"].is_string());
        assert_eq!(ready["description"], "latest pending edit");
        assert_eq!(ready["meta"]["focus"], json!({"x":0.25,"y":-0.5}));
        assert_eq!(
            ready["preview_url"].is_string(),
            format.preview_content_type.is_some()
        );
        // URLs must name actual installed artifacts; raw is private and retired.
        for field in ["url", "preview_url"] {
            if let Some(url) = ready[field].as_str() {
                let relative = Url::parse(url)?
                    .path()
                    .trim_start_matches("/system/")
                    .to_owned();
                assert!(resources.root.join(relative).is_file());
            }
        }
        assert!(
            !resources
                .root
                .join(format!(".local-upload-input/local_uploads/{id}/1/input"))
                .exists()
        );
        queue
            .enqueue(&processing_job(UploadIdentity {
                media_id: id,
                account_id: OWNER,
                generation: 1,
            }))
            .await?;
        process(&queue, &executor).await?;
        assert_eq!(
            request(&client, &base, Method::GET, id, OWNER_TOKEN, None)
                .await?
                .1,
            ready,
            "ready replay must not change response"
        );
    }
    // A ready converted image can attach; the same pending ID was refused above.
    let response = client
        .post(format!("{base}/api/v1/statuses"))
        .header("host", DOMAIN)
        .bearer_auth(OWNER_TOKEN)
        .header("content-type", "application/json")
        .body(
            json!({"status":"ready local attachment", "media_ids":[ids[0].to_string()]})
                .to_string(),
        )
        .send()
        .await?;
    assert_eq!(response.status().as_u16(), 200);
    let status: Value = serde_json::from_slice(&response.bytes().await?)?;
    assert_eq!(status["media_attachments"][0]["id"], ids[0].to_string());
    let response = client
        .delete(format!(
            "{base}/api/v1/statuses/{}",
            status["id"].as_str().unwrap()
        ))
        .header("host", DOMAIN)
        .bearer_auth(OWNER_TOKEN)
        .send()
        .await?;
    assert_eq!(response.status().as_u16(), 200);
    // Incomplete staging has not been accepted and must remain invisible.
    let mut tx = pool.begin().await?;
    let staged = rustodon::mastodon::local_uploads::stage_in(
        &mut tx,
        OWNER,
        1,
        &rustodon::mastodon::local_uploads::RawInput {
            mime: "audio/ogg",
            size: 10,
            sha256: &[0; 32],
        },
    )
    .await?;
    tx.commit().await?;
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        assert_eq!(
            request(&client, &base, method, staged.media_id, OWNER_TOKEN, None)
                .await?
                .0,
            404
        );
    }
    let mut tx = pool.begin().await?;
    rustodon::mastodon::local_uploads::discard_staging_in(&mut tx, staged).await?;
    rustodon::mastodon::local_uploads::forget_orphan_in(&mut tx, staged).await?;
    tx.commit().await?;
    // Scope authorization still precedes owner lookup, including failed rows.
    sqlx::query("UPDATE oauth_access_tokens SET scopes = 'read' WHERE token = $1")
        .bind(OTHER_TOKEN)
        .execute(&pool)
        .await?;
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        assert_eq!(
            request(&client, &base, method, ids[0], OTHER_TOKEN, None)
                .await?
                .0,
            403
        );
    }
    let denied = client
        .post(format!("{base}/api/v2/media"))
        .header("host", DOMAIN)
        .bearer_auth(OTHER_TOKEN)
        .header(
            "content-type",
            "multipart/form-data; boundary=local-upload-test",
        )
        .body(upload_body("audio/ogg", b"denied"))
        .send()
        .await?;
    assert_eq!(denied.status().as_u16(), 403);
    sqlx::query("UPDATE oauth_access_tokens SET scopes = 'write:media' WHERE token = $1")
        .bind(OTHER_TOKEN)
        .execute(&pool)
        .await?;
    // A bytes/MIME mismatch is a retained asynchronous terminal error, not 404.
    let (status, failed) = upload(&client, &base, 2, "image/heic", b"not heic").await?;
    assert_eq!(status, 202);
    let failed: i64 = failed["id"].as_str().unwrap().parse()?;
    ids.push(failed);
    process(&queue, &executor).await?;
    let before: Value =
        sqlx::query_scalar("SELECT to_jsonb(m) FROM media_attachments m WHERE id=$1")
            .bind(failed)
            .fetch_one(&pool)
            .await?;
    assert_eq!(before["processing"], 3);
    assert!(before["file_file_name"].is_null());
    for method in [Method::GET, Method::PUT] {
        let (status, body) = request(
            &client,
            &base,
            method.clone(),
            failed,
            OWNER_TOKEN,
            Some(json!({"description":"must not change"})),
        )
        .await?;
        assert_eq!(status, 422);
        assert_eq!(body["error"], PROCESSING_ERROR);
        assert_eq!(
            request(&client, &base, method, failed, OTHER_TOKEN, None)
                .await?
                .0,
            404
        );
    }
    let after: Value =
        sqlx::query_scalar("SELECT to_jsonb(m) FROM media_attachments m WHERE id=$1")
            .bind(failed)
            .fetch_one(&pool)
            .await?;
    assert_eq!(before, after);
    assert_eq!(
        request(&client, &base, Method::DELETE, failed, OWNER_TOKEN, None)
            .await?
            .0,
        200
    );
    // Explicit pending deletion followed by delivery/replay cannot resurrect rows.
    let (_, pending) = upload(
        &client,
        &base,
        2,
        "audio/ogg",
        &fs::read("tests/fixtures/media/boop.ogg")?,
    )
    .await?;
    let pending: i64 = pending["id"].as_str().unwrap().parse()?;
    ids.push(pending);
    assert_eq!(
        request(&client, &base, Method::DELETE, pending, OWNER_TOKEN, None)
            .await?
            .0,
        200
    );
    process(&queue, &executor).await?;
    for id in [failed, pending] {
        queue
            .enqueue(&processing_job(UploadIdentity {
                media_id: id,
                account_id: OWNER,
                generation: 1,
            }))
            .await?;
        process(&queue, &executor).await?;
        assert_eq!(
            request(&client, &base, Method::GET, id, OWNER_TOKEN, None)
                .await?
                .0,
            404
        );
        let retained: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM rustodon.local_uploads WHERE media_id=$1)",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
        assert!(!retained);
        assert!(
            !resources
                .root
                .join(format!(".local-upload-input/local_uploads/{id}/1/input"))
                .exists()
        );
    }
    // Cheap invalid MIME/size rejection creates no public row or private owner.
    let owners_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rustodon.local_uploads WHERE account_id=$1")
            .bind(OWNER)
            .fetch_one(&pool)
            .await?;
    let files_before = files(&resources.root, &resources.root)?;
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM media_attachments WHERE account_id=$1")
            .bind(OWNER)
            .fetch_one(&pool)
            .await?;
    for (mime, bytes) in [
        ("application/pdf", vec![0; 16]),
        ("image/avif", vec![0; rustodon::media::IMAGE_SIZE_LIMIT]),
    ] {
        assert_eq!(upload(&client, &base, 2, mime, &bytes).await?.0, 422);
    }
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM media_attachments WHERE account_id=$1")
            .bind(OWNER)
            .fetch_one(&pool)
            .await?;
    assert_eq!(before, after);
    let owners_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rustodon.local_uploads WHERE account_id=$1")
            .bind(OWNER)
            .fetch_one(&pool)
            .await?;
    assert_eq!(owners_before, owners_after);
    assert_eq!(files_before, files(&resources.root, &resources.root)?);
    for version in [1, 2] {
        let (status, ready) = upload(&client, &base, version, "image/jpeg", &jpeg(32, 24)?).await?;
        assert_eq!(status, 200, "ordinary JPEG remains synchronous");
        assert!(ready["url"].is_string());
        ids.push(ready["id"].as_str().unwrap().parse()?);
    }
    for id in ids {
        let _ = request(&client, &base, Method::DELETE, id, OWNER_TOKEN, None).await?;
    }
    process(&queue, &executor).await?;
    sqlx::query("UPDATE oauth_access_tokens SET scopes=$1 WHERE token=$2")
        .bind(old_scope)
        .bind(OTHER_TOKEN)
        .execute(&pool)
        .await?;
    assert!(
        files(&resources.root, &resources.root)?.is_empty(),
        "all artifacts durably removed"
    );
    Ok(())
}

/// Owner-only setup, before applying the unchanged documented role grants.
#[tokio::test]
#[ignore = "prepares only a disposable restored PostgreSQL fixture"]
async fn local_rich_upload_schema_setup() -> TestResult {
    let pool = PgPool::connect(&std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?).await?;
    let mut connection = pool.acquire().await?;
    rustodon::operational_schema::migrate(&mut connection).await?;
    Ok(())
}

async fn assert_restricted_roles(owner: &PgPool, runtime: &PgPool, writer: &PgPool) -> TestResult {
    let owner_name: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(owner)
        .await?;
    let mut names = vec![owner_name];
    for (label, pool) in [("runtime", runtime), ("writer", writer)] {
        let (name, privileged): (String, bool) = sqlx::query_as(
            "SELECT current_user::text, r.rolsuper OR r.rolcreatedb OR r.rolcreaterole OR r.rolreplication OR r.rolbypassrls
               OR EXISTS (SELECT 1 FROM pg_auth_members WHERE member=r.oid)
               OR EXISTS (SELECT 1 FROM pg_database WHERE datname=current_database() AND datdba=r.oid)
               OR EXISTS (SELECT 1 FROM pg_namespace WHERE nspname IN ('public','rustodon') AND nspowner=r.oid)
               OR EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname IN ('public','rustodon') AND c.relowner=r.oid)
             FROM pg_roles r WHERE rolname=current_user"
        ).fetch_one(pool).await?;
        assert!(
            !privileged,
            "{label} must not inherit or own application objects"
        );
        assert!(
            !names.contains(&name),
            "owner/runtime/writer must be distinct logins"
        );
        eprintln!("restricted lifecycle {label} current_user={name}");
        names.push(name);
    }
    // Actual denied SQL, not just ACL predicates: no accidental owner fallback.
    for (label, pool, statement) in [
        (
            "runtime public write",
            runtime,
            "UPDATE public.media_attachments SET description=NULL WHERE false",
        ),
        (
            "runtime private upload owner",
            runtime,
            "SELECT media_id FROM rustodon.local_uploads LIMIT 0",
        ),
        (
            "writer heartbeat",
            writer,
            "SELECT process_id FROM rustodon.heartbeats LIMIT 0",
        ),
        (
            "writer runtime enqueue",
            writer,
            "INSERT INTO rustodon.durable_jobs (kind) SELECT 'forbidden' WHERE false",
        ),
        (
            "writer identity secret",
            writer,
            "UPDATE public.accounts SET private_key=NULL WHERE false",
        ),
    ] {
        let error = sqlx::query(statement).execute(pool).await.expect_err(label);
        assert_eq!(
            error
                .as_database_error()
                .and_then(sqlx::error::DatabaseError::code)
                .as_deref(),
            Some("42501"),
            "{label}: {error}"
        );
        eprintln!("expected ACL rejection ({label}, SQLSTATE 42501): {error}");
    }
    Ok(())
}

/// Exercise the real heartbeat producer and the same capability predicate used
/// by writable `admin worker-readiness`, not a hand-forged positive heartbeat.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL with the documented runtime/writer roles"]
#[allow(clippy::too_many_lines)]
async fn local_rich_upload_restricted_worker_readiness() -> TestResult {
    use rustodon::config::WorkerConfig;
    use rustodon::worker::{local_uploads, run_until_shutdown};
    use std::time::Duration;
    let owner = PgPool::connect(&std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?).await?;
    let runtime_pool = PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer = PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    assert_restricted_roles(&owner, &runtime_pool, &writer).await?;
    let queue = Queue::new(runtime_pool);
    let directory =
        std::env::temp_dir().join(format!("rustodon-upload-readiness-{}", std::process::id()));
    fs::create_dir(&directory)?;
    let resources = Resources {
        root: directory,
        server: None,
    };
    let freshness = chrono::Duration::seconds(5);
    for (name, writer_enabled, root_enabled, lane, expected) in [
        (
            "generic-maintenance",
            false,
            false,
            Lane::Maintenance,
            false,
        ),
        ("writer-without-root", true, false, Lane::Maintenance, false),
        ("local-wrong-lane", true, true, Lane::Pull, false),
        ("local-maintenance", true, true, Lane::Maintenance, true),
    ] {
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            writer_enabled.then(|| writer.clone()),
            None,
            Some(ActivityPubDeliveryConfig {
                origin: Url::parse(&format!("https://{DOMAIN}/"))?,
                local_domain: DOMAIN.into(),
                media_root_url: "/system".into(),
                media_root: if root_enabled {
                    Some(PaperclipRoot::open(&resources.root)?)
                } else {
                    None
                },
                limited_federation: true,
                remote_media_endpoint: None,
                remote_delivery_endpoint: None,
                remote_fetch_endpoint: None,
            }),
        )?;
        let config = WorkerConfig {
            lanes: [lane].into_iter().collect(),
            concurrency: 1,
            remote_http_concurrency: 1,
            media_concurrency: 1,
            lease_seconds: 30,
            poll_milliseconds: 100,
            heartbeat_seconds: 1,
            shutdown_seconds: 5,
        };
        let required = config.lanes.clone();
        let process_id = format!("upload-role-readiness-{name}");
        let heartbeat_id = format!("{process_id}:worker");
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(run_until_shutdown(
            queue.clone(),
            handlers,
            config,
            process_id,
            writer_enabled.then(|| writer.clone()),
            async {
                let _ = stopped.await;
            },
        ));
        let observed = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let info: Option<Value> = sqlx::query_scalar("SELECT info FROM rustodon.heartbeats WHERE process_id=$1")
                    .bind(&heartbeat_id).fetch_optional(&owner).await?;
                if let Some(info) = info {
                    assert_eq!(info["local_uploads"], expected, "{name}");
                    // Worker and scheduler heartbeats are separate writes. Observe
                    // both before asserting the capability extension to readiness.
                    if !queue.readiness(&required, freshness).await?.ready() {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                    assert_eq!(local_uploads::ready(&queue, freshness).await?, expected, "writable local capability: {name}");
                    // Wait for a second emitted heartbeat, proving refresh keeps the flag.
                    let first: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT heartbeat_at FROM rustodon.heartbeats WHERE process_id=$1")
                        .bind(&heartbeat_id).fetch_one(&owner).await?;
                    loop {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        let refreshed: bool = sqlx::query_scalar("SELECT heartbeat_at > $2 AND info->'local_uploads' = $3 FROM rustodon.heartbeats WHERE process_id=$1")
                            .bind(&heartbeat_id).bind(first).bind(json!(expected)).fetch_one(&owner).await?;
                        if refreshed { break; }
                    }
                    return Ok::<(), Box<dyn Error>>(());
                }
                if worker.is_finished() { return Err("worker exited before readiness heartbeat".into()); }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await;
        let _ = stop.send(());
        tokio::time::timeout(Duration::from_secs(10), worker).await???;
        observed??;
        assert!(
            !local_uploads::ready(&queue, freshness).await?,
            "shutdown removes capability: {name}"
        );
        eprintln!("restricted worker readiness passed: {name}, local_uploads={expected}");
    }
    Ok(())
}
