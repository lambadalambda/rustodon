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
    media_session(&pool, "media-upload-owner", OWNER_TOKEN).await?;
    let owner_bearer = format!("Bearer {OWNER_TOKEN}");
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
                assert!(resources.root.join(&relative).is_file());
                let path = format!("/system/{relative}");
                for credentials in [
                    (Some(owner_bearer.as_str()), None),
                    (None, Some("media-upload-owner")),
                ] {
                    for (method, extra, expected) in [
                        (Method::GET, None, 200),
                        (Method::HEAD, None, 200),
                        (Method::GET, Some(("range", "bytes=0-15")), 206),
                    ] {
                        media_read(&client, &base, &path, method, credentials, extra, expected)
                            .await?;
                    }
                }
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

fn media_path(id: i64, style: &str) -> String {
    let partition = format!("{id:09}");
    format!(
        "/system/media_attachments/files/{}/{}/{}/{style}/state.jpg",
        &partition[..3],
        &partition[3..6],
        &partition[6..]
    )
}

async fn media_read(
    client: &Client,
    base: &str,
    path: &str,
    method: Method,
    credentials: (Option<&str>, Option<&str>),
    extra: Option<(&str, &str)>,
    expected: u16,
) -> TestResult<reqwest::Response> {
    let mut request = client
        .request(method, format!("{base}{path}"))
        .header("host", DOMAIN);
    if let Some(token) = credentials.0 {
        // Empty/malformed Authorization is deliberately not omitted.
        request = request.header("authorization", token);
    }
    if let Some(session) = credentials.1 {
        request = request.header("cookie", format!("_mastodon_session={session}"));
    }
    if let Some((name, value)) = extra {
        request = request.header(name, value);
    }
    let response = request.send().await?;
    assert_eq!(
        response.status().as_u16(),
        expected,
        "{path}, credentials={credentials:?}, extra={extra:?}"
    );
    assert!(!response.headers().contains_key("set-cookie"));
    let private = credentials.0.is_some() || credentials.1.is_some() || expected >= 400;
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some(if private {
            "private, no-store"
        } else {
            "public, max-age=2419200, immutable"
        }),
        "{path}: media denial/success caching"
    );
    assert_eq!(
        response.headers().get("vary").and_then(|v| v.to_str().ok()),
        Some("Authorization, Cookie, Signature")
    );
    Ok(response)
}

async fn media_session(pool: &PgPool, session: &str, token: &str) -> TestResult {
    sqlx::query("INSERT INTO session_activations (access_token_id, user_id, session_id, created_at, updated_at, ip, user_agent) SELECT id, resource_owner_id, $1, clock_timestamp(), clock_timestamp(), '192.0.2.1', 'media fixture' FROM oauth_access_tokens WHERE token=$2")
        .bind(session).bind(token).execute(pool).await?;
    Ok(())
}

async fn session_snapshot(pool: &PgPool) -> TestResult<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('sessions', (SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM session_activations s), 'tokens', (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM oauth_access_tokens t), 'sign_ins', (SELECT jsonb_agg(jsonb_build_array(id, current_sign_in_at, last_sign_in_at, sign_in_count) ORDER BY id) FROM users), 'activity_buckets', (SELECT jsonb_agg(to_jsonb(b) ORDER BY day) FROM rustodon.activity_buckets b), 'activity_members', (SELECT jsonb_agg(to_jsonb(m) ORDER BY day, user_id) FROM rustodon.activity_members m))")
        .fetch_one(pool).await?)
}

/// Persisted native media reads. Reuse the existing JPEG/state fixture (including
/// stale pending/failed bytes), not a second codec or browser harness.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL with the documented runtime/writer roles"]
#[allow(clippy::too_many_lines)]
async fn local_upload_browser_media_access() -> TestResult {
    const UNRELATED: &str = "fixture-bearer-matrix-viewer-v4-6-5";
    let pool = PgPool::connect(&std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?).await?;
    let runtime_pool = PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer_pool =
        PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    assert_restricted_roles(&pool, &runtime_pool, &writer_pool).await?;
    let root = std::env::temp_dir().join(format!("rustodon-media-viewer-{}", std::process::id()));
    fs::create_dir(&root)?;
    let mut resources = Resources {
        root: root.canonicalize()?,
        server: None,
    };
    seed(&pool, &resources.root).await?;
    sqlx::query("UPDATE oauth_access_tokens SET scopes='read' WHERE token=$1")
        .bind(OTHER_TOKEN)
        .execute(&pool)
        .await?;
    for (session, token) in [
        ("media-owner", OWNER_TOKEN),
        ("media-follower", OTHER_TOKEN),
        ("media-unrelated", UNRELATED),
        ("media-expired-session", OWNER_TOKEN),
        ("media-revoked", "fixture-bearer-revoked-v4-6-5"),
        ("media-expired-token", "fixture-bearer-expired-v4-6-5"),
        ("media-disabled", "fixture-bearer-disabled-user-v4-6-5"),
        ("media-2fa", "fixture-bearer-missing-2fa-v4-6-5"),
        ("media-scope", "fixture-bearer-insufficient-v4-6-5"),
        ("media-mismatch", OTHER_TOKEN),
        ("media-logout", OWNER_TOKEN),
    ] {
        media_session(&pool, session, token).await?;
    }
    sqlx::query("UPDATE session_activations SET updated_at=clock_timestamp()-interval '31 days' WHERE session_id='media-expired-session'").execute(&pool).await?;
    sqlx::query("UPDATE session_activations SET user_id=101 WHERE session_id='media-mismatch'")
        .execute(&pool)
        .await?;
    // The real logout persistence operation removes the session and revokes its
    // backing token; use a dedicated token so the owner's other tests stay valid.
    sqlx::query("INSERT INTO oauth_access_tokens (application_id, resource_owner_id, token, scopes, created_at) VALUES (301, 101, 'media-logout-token', 'read', clock_timestamp())").execute(&pool).await?;
    sqlx::query("UPDATE session_activations SET access_token_id=(SELECT id FROM oauth_access_tokens WHERE token='media-logout-token') WHERE session_id='media-logout'").execute(&pool).await?;
    WriteRepository::from_pool(writer_pool.clone())
        .delete_browser_session("media-logout")
        .await?;
    // A regression that globally tracks cookie reads must not hide behind the
    // throttle: make this retained, functional owner due before media GET/HEAD.
    sqlx::query(
        "UPDATE users SET current_sign_in_at=clock_timestamp()-interval '25 hours' WHERE id=101",
    )
    .execute(&pool)
    .await?;
    let snapshot = session_snapshot(&pool).await?;
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
    .with_write_repository(WriteRepository::from_pool(writer_pool.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    resources.server = Some(tokio::spawn(async move {
        axum::serve(listener, router(state)).await
    }));
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let bearer = format!("Bearer {OWNER_TOKEN}");
    let follower = format!("Bearer {OTHER_TOKEN}");
    let unrelated = format!("Bearer {UNRELATED}");
    let owner_credentials = [(Some(bearer.as_str()), None), (None, Some("media-owner"))];
    // Anonymous recognized media denials are not shared, either.
    media_read(
        &client,
        &base,
        &media_path(READY, "original"),
        Method::GET,
        (None, None),
        None,
        404,
    )
    .await?;
    // Preserve the pre-existing optional bearer contract on public attached media.
    for (token, expected) in [
        ("", 200),
        ("malformed", 200),
        ("Bearer unknown", 200),
        ("Bearer fixture-bearer-application-only-v4-6-5", 200),
        ("Bearer fixture-bearer-disabled-user-v4-6-5", 200),
        ("Bearer fixture-bearer-revoked-v4-6-5", 401),
        ("Bearer fixture-bearer-expired-v4-6-5", 401),
        ("Bearer fixture-bearer-insufficient-v4-6-5", 403),
    ] {
        media_read(
            &client,
            &base,
            &media_path(ATTACHED, "original"),
            Method::GET,
            (Some(token), Some("media-owner")),
            None,
            expected,
        )
        .await?;
    }
    // Reproduce native private attached image reads independently of uploads.
    sqlx::query("UPDATE statuses SET visibility=2 WHERE id=$1")
        .bind(STATUS)
        .execute(&pool)
        .await?;
    media_read(
        &client,
        &base,
        &media_path(ATTACHED, "original"),
        Method::GET,
        owner_credentials[1],
        None,
        200,
    )
    .await?;
    sqlx::query("UPDATE statuses SET visibility=0 WHERE id=$1")
        .bind(STATUS)
        .execute(&pool)
        .await?;
    for credentials in owner_credentials {
        for style in ["original", "small"] {
            let path = media_path(READY, style);
            let response =
                media_read(&client, &base, &path, Method::GET, credentials, None, 200).await?;
            let modified = response.headers()["last-modified"].to_str()?.to_owned();
            let bytes = response.bytes().await?;
            assert_eq!(
                bytes.as_ref(),
                fs::read(resources.root.join(path.trim_start_matches("/system/")))?
            );
            let response =
                media_read(&client, &base, &path, Method::HEAD, credentials, None, 200).await?;
            assert_eq!(
                response.headers()["content-length"]
                    .to_str()?
                    .parse::<usize>()?,
                bytes.len()
            );
            assert!(response.bytes().await?.is_empty());
            let response = media_read(
                &client,
                &base,
                &path,
                Method::GET,
                credentials,
                Some(("range", "bytes=0-15")),
                206,
            )
            .await?;
            assert_eq!(
                response.headers()["content-range"],
                format!("bytes 0-15/{}", bytes.len())
            );
            assert_eq!(response.bytes().await?.as_ref(), &bytes[..16]);
            media_read(
                &client,
                &base,
                &path,
                Method::GET,
                credentials,
                Some(("if-modified-since", &modified)),
                304,
            )
            .await?;
            media_read(
                &client,
                &base,
                &path,
                Method::GET,
                credentials,
                Some(("range", "bytes=999999-")),
                404, // Existing Rack/Rails unsatisfiable-range convention.
            )
            .await?;
        }
    }
    // Even stale artifacts and conditionals/ranges must not bypass state/owner.
    for id in [PENDING, IN_PROGRESS, FAILED, READY, 940_799] {
        for credentials in [
            owner_credentials[0],
            owner_credentials[1],
            (None, None),
            (Some(unrelated.as_str()), Some("media-owner")),
            (None, Some("media-unrelated")),
        ] {
            if id == READY && owner_credentials.contains(&credentials) {
                continue;
            }
            for style in ["original", "small"] {
                for method in [Method::GET, Method::HEAD] {
                    for extra in [
                        None,
                        Some(("range", "bytes=0-15")),
                        Some(("range", "bytes=999999-")),
                        Some(("if-modified-since", "Wed, 01 Jan 2100 00:00:00 GMT")),
                    ] {
                        media_read(
                            &client,
                            &base,
                            &media_path(id, style),
                            method.clone(),
                            credentials,
                            extra,
                            404,
                        )
                        .await?;
                    }
                }
            }
        }
    }
    for (session, expected) in [
        ("media-expired-session", 404),
        ("media-revoked", 404),
        ("media-expired-token", 404),
        ("media-disabled", 403),
        ("media-2fa", 403),
        ("media-scope", 403),
        ("media-mismatch", 404),
        ("media-logout", 404),
        ("missing-session", 404),
    ] {
        media_read(
            &client,
            &base,
            &media_path(READY, "original"),
            Method::GET,
            (None, Some(session)),
            Some(("range", "bytes=0-15")),
            expected,
        )
        .await?;
    }
    for (token, expected) in [
        ("", 401),
        ("malformed", 401),
        ("Bearer unknown", 401),
        ("Bearer fixture-bearer-revoked-v4-6-5", 401),
        ("Bearer fixture-bearer-expired-v4-6-5", 401),
        ("Bearer fixture-bearer-insufficient-v4-6-5", 403),
        ("Bearer fixture-bearer-application-only-v4-6-5", 422),
        ("Bearer fixture-bearer-disabled-user-v4-6-5", 403),
    ] {
        media_read(
            &client,
            &base,
            &media_path(READY, "original"),
            Method::GET,
            (Some(token), Some("media-owner")),
            None,
            expected,
        )
        .await?;
    }
    media_read(
        &client,
        &base,
        &media_path(READY, "original"),
        Method::GET,
        (Some(""), None),
        None,
        401,
    )
    .await?;
    // An explicit valid bearer also wins over an invalid/different cookie.
    media_read(
        &client,
        &base,
        &media_path(READY, "original"),
        Method::GET,
        (Some(&bearer), Some("media-unrelated")),
        None,
        200,
    )
    .await?;
    assert_eq!(
        snapshot,
        session_snapshot(&pool).await?,
        "media reads must not touch sessions/tokens or mint replacements"
    );
    // Attached historical processing values are not reinterpreted as local-upload readiness.
    sqlx::query("UPDATE media_attachments SET processing=0 WHERE id=$1")
        .bind(ATTACHED)
        .execute(&pool)
        .await?;
    let path = media_path(ATTACHED, "original");
    let public = media_read(&client, &base, &path, Method::GET, (None, None), None, 200).await?;
    assert_eq!(
        public.headers()["cache-control"],
        "public, max-age=2419200, immutable"
    );
    let modified = public.headers()["last-modified"].to_str()?;
    for (extra, expected) in [
        (Some(("range", "bytes=0-15")), 206),
        (Some(("if-modified-since", modified)), 304),
        (Some(("range", "bytes=999999-")), 404),
    ] {
        media_read(
            &client,
            &base,
            &path,
            Method::GET,
            (None, None),
            extra,
            expected,
        )
        .await?;
    }
    media_read(
        &client,
        &base,
        &path.replace("state.jpg", "wrong.jpg"),
        Method::GET,
        (None, None),
        None,
        404,
    )
    .await?;
    sqlx::query("UPDATE statuses SET visibility=2 WHERE id=$1")
        .bind(STATUS)
        .execute(&pool)
        .await?;
    for credentials in [
        owner_credentials[0],
        owner_credentials[1],
        (Some(follower.as_str()), None),
        (None, Some("media-follower")),
    ] {
        for method in [Method::GET, Method::HEAD] {
            media_read(
                &client,
                &base,
                &path,
                method,
                credentials,
                Some(("range", "bytes=0-15")),
                206,
            )
            .await?;
        }
    }
    for credentials in [
        (None, None),
        (Some(unrelated.as_str()), None),
        (None, Some("media-unrelated")),
    ] {
        media_read(
            &client,
            &base,
            &path,
            Method::GET,
            credentials,
            Some(("if-modified-since", "Wed, 01 Jan 2100 00:00:00 GMT")),
            404,
        )
        .await?;
    }
    // Optional bearer anonymity must never become an owner-cookie fallback.
    for token in [
        "malformed",
        "Bearer unknown",
        "Bearer fixture-bearer-application-only-v4-6-5",
    ] {
        media_read(
            &client,
            &base,
            &path,
            Method::GET,
            (Some(token), Some("media-owner")),
            None,
            404,
        )
        .await?;
    }
    // The fixture follower is a report manager; retain its discarded exception.
    for dangling in [false, true] {
        if dangling {
            // The ordinary FK nulls status_id on DELETE. Model an imported
            // dangling reference explicitly, owner-only in this disposable DB.
            let mut tx = pool.begin().await?;
            sqlx::query("SET LOCAL session_replication_role = replica")
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE media_attachments SET status_id=$1+1, processing=2 WHERE id=$2")
                .bind(STATUS)
                .bind(ATTACHED)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        } else {
            sqlx::query("UPDATE statuses SET deleted_at=clock_timestamp() WHERE id=$1")
                .bind(STATUS)
                .execute(&pool)
                .await?;
            sqlx::query("UPDATE media_attachments SET processing=2 WHERE id=$1")
                .bind(ATTACHED)
                .execute(&pool)
                .await?;
        }
        for credentials in owner_credentials {
            media_read(
                &client,
                &base,
                &path,
                Method::GET,
                credentials,
                Some(("range", "bytes=0-15")),
                404,
            )
            .await?;
        }
        media_read(&client, &base, &path, Method::GET, (None, None), None, 404).await?;
        media_read(
            &client,
            &base,
            &path,
            Method::GET,
            (None, Some("media-follower")),
            None,
            200,
        )
        .await?;
    }
    // Exact metadata/path and on-disk lookup denials receive the same private policy.
    let bad_path = media_path(READY, "original").replace("state.jpg", "wrong.jpg");
    media_read(
        &client,
        &base,
        &bad_path,
        Method::GET,
        owner_credentials[1],
        None,
        404,
    )
    .await?;
    sqlx::query(
        "UPDATE media_attachments SET remote_url='https://remote.invalid/media.jpg' WHERE id=$1",
    )
    .bind(READY)
    .execute(&pool)
    .await?;
    media_read(
        &client,
        &base,
        &media_path(READY, "original"),
        Method::GET,
        owner_credentials[1],
        None,
        404,
    )
    .await?;
    sqlx::query("UPDATE media_attachments SET remote_url='' WHERE id=$1")
        .bind(READY)
        .execute(&pool)
        .await?;
    fs::remove_file(
        resources
            .root
            .join(media_path(READY, "original").trim_start_matches("/system/")),
    )?;
    media_read(
        &client,
        &base,
        &media_path(READY, "original"),
        Method::GET,
        owner_credentials[1],
        None,
        404,
    )
    .await?;
    let raw = client
        .get(format!(
            "{base}/system/.local-upload-input/local_uploads/{READY}/1/input"
        ))
        .header("host", DOMAIN)
        .header("cookie", "_mastodon_session=media-owner")
        .send()
        .await?;
    assert_eq!(raw.status().as_u16(), 404);
    // Run the same public attached route in limited federation, sequentially.
    resources.server.take().unwrap().abort();
    sqlx::query("UPDATE statuses SET visibility=0, deleted_at=NULL WHERE id=$1")
        .bind(STATUS)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE media_attachments SET status_id=$1 WHERE id=$2")
        .bind(STATUS)
        .bind(ATTACHED)
        .execute(&pool)
        .await?;
    let state = WebState::new(
        Repository::from_pool(runtime_pool),
        Url::parse(&format!("https://{DOMAIN}/"))?,
        DOMAIN,
        "/system",
        &resources.root,
        InstanceRuntimeConfig {
            limited_federation: true,
            ..runtime()
        },
        Vec::new(),
        vec![DOMAIN.to_owned()],
    )?
    .with_write_repository(WriteRepository::from_pool(writer_pool));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    resources.server = Some(tokio::spawn(async move {
        axum::serve(listener, router(state)).await
    }));
    for (credentials, expected) in [
        ((None, None), 401),
        ((None, Some("missing-session")), 401),
        ((Some("malformed"), Some("media-owner")), 401),
        ((Some("Bearer unknown"), Some("media-owner")), 401),
        (
            (
                Some("Bearer fixture-bearer-application-only-v4-6-5"),
                Some("media-owner"),
            ),
            422,
        ),
        (
            (
                Some("Bearer fixture-bearer-revoked-v4-6-5"),
                Some("media-owner"),
            ),
            401,
        ),
        (owner_credentials[0], 200),
        (owner_credentials[1], 200),
    ] {
        media_read(
            &client,
            &base,
            &path,
            Method::GET,
            credentials,
            None,
            expected,
        )
        .await?;
    }
    assert_eq!(snapshot, session_snapshot(&pool).await?);
    assert_interactive_activity_subset(&pool, &client, &base).await?;
    Ok(())
}

// Reuse the persisted browser/media fixture to prove explicit tracking call sites,
// not broad middleware coverage and not an executable-browser gate.
#[allow(clippy::too_many_lines)]
async fn assert_interactive_activity_subset(
    pool: &PgPool,
    client: &Client,
    base: &str,
) -> TestResult {
    sqlx::query("DELETE FROM rustodon.activity_members WHERE user_id=101")
        .execute(pool)
        .await?;
    sqlx::query("UPDATE users SET current_sign_in_at=NULL WHERE id=101")
        .execute(pool)
        .await?;
    let count_before: i64 =
        sqlx::query_scalar("SELECT sign_in_count::bigint FROM users WHERE id=101")
            .fetch_one(pool)
            .await?;
    let ordinary = client
        .get(format!("{base}/api/v1/preferences"))
        .header("host", DOMAIN)
        .bearer_auth(OWNER_TOKEN)
        .send()
        .await?;
    assert_eq!(ordinary.status().as_u16(), 200);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.activity_members WHERE user_id=101"
        )
        .fetch_one(pool)
        .await?,
        0,
        "generic bearer reads do not track"
    );
    for _ in 0..2 {
        let response = client
            .get(format!("{base}/api/v1/accounts/verify_credentials"))
            .header("host", DOMAIN)
            .bearer_auth(OWNER_TOKEN)
            .send()
            .await?;
        assert_eq!(response.status().as_u16(), 200);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.activity_members WHERE user_id=101"
        )
        .fetch_one(pool)
        .await?,
        1
    );
    let previous: chrono::NaiveDateTime =
        sqlx::query_scalar("SELECT current_sign_in_at FROM users WHERE id=101")
            .fetch_one(pool)
            .await?;
    // Same user, retained browser: no second update while not due.
    let response = client
        .get(format!("{base}/home"))
        .header("host", DOMAIN)
        .header("cookie", "_mastodon_session=media-owner")
        .send()
        .await?;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        sqlx::query_scalar::<_, chrono::NaiveDateTime>(
            "SELECT current_sign_in_at FROM users WHERE id=101"
        )
        .fetch_one(pool)
        .await?,
        previous
    );
    for path in ["/home", "/settings/profile", "/auth/session"] {
        sqlx::query("DELETE FROM rustodon.activity_members WHERE user_id=101")
            .execute(pool)
            .await?;
        sqlx::query(
            "UPDATE users SET current_sign_in_at=clock_timestamp()-interval '25 hours' WHERE id=101",
        )
        .execute(pool)
        .await?;
        let response = client
            .get(format!("{base}{path}"))
            .header("host", DOMAIN)
            .header("cookie", "_mastodon_session=media-owner")
            .send()
            .await?;
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.activity_members WHERE user_id=101"
            )
            .fetch_one(pool)
            .await?,
            1,
            "{path} tracks a due retained browser"
        );
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT current_sign_in_at > clock_timestamp()-interval '1 minute' \
                 AND last_sign_in_at < clock_timestamp()-interval '24 hours' FROM users WHERE id=101",
            )
            .fetch_one(pool)
            .await?,
            "{path} claims the due sign-in timestamp"
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sign_in_count::bigint FROM users WHERE id=101")
            .fetch_one(pool)
            .await?,
        count_before
    );
    Ok(())
}
