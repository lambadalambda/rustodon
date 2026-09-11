#![cfg(feature = "test-support")]

use super::*;
use rustodon::jobs::ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND;
use rustodon::mastodon::rest::{RestProjectionLoader, RestSerializer};
use rustodon::worker::refresh_remote_account;
use sqlx::PgPool;
use std::io::Cursor;
use std::sync::atomic::AtomicBool;
use tokio::sync::Mutex;

const LOCAL: i64 = 116_844_606_259_201_001;
const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
const DOMAIN: &str = "profile-media.fixture.invalid";
const ACTOR: &str = "http://profile-media.fixture.invalid/users/profile";
const AVATAR: &str = "http://profile-images.fixture.invalid/avatar.blob";
const HEADER: &str = "http://profile-images.fixture.invalid/header.blob";
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

// Both transports are pinned to loopback; neither actor discovery nor media may
// contact the advertised synthetic origins. The media barrier fences installation,
// rather than merely testing policy before the network request starts.
struct Fixture {
    owner: PgPool,
    writer: PgPool,
    runtime: PgPool,
    queue: Queue,
    executor: Arc<WorkerExecutor>,
    config: ActivityPubDeliveryConfig,
    id: i64,
    document: Arc<Mutex<Value>>,
    actor_requests: Arc<Mutex<Vec<Vec<u8>>>>,
    media_requests: Arc<AtomicUsize>,
    media_body: Arc<Mutex<Vec<u8>>>,
    pause_media: Arc<AtomicBool>,
    media_started: Arc<Notify>,
    media_release: Arc<Notify>,
    servers: Vec<tokio::task::JoinHandle<std::io::Result<()>>>,
    root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for server in &self.servers {
            server.abort();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    async fn discover() -> TestResult<Self> {
        reset().await?;
        let owner = PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
        let writer = PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
        let runtime = PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
        // Only fixture setup/teardown uses the owner. Discovery, Update, refresh,
        // and media installation all exercise the restricted WRITE_DATABASE_URL.
        sqlx::query("DELETE FROM accounts WHERE domain = $1")
            .bind(DOMAIN)
            .execute(&owner)
            .await?;
        sqlx::query("DELETE FROM domain_blocks WHERE domain = $1")
            .bind(DOMAIN)
            .execute(&owner)
            .await?;
        let local = Repository::from_pool(runtime.clone())
            .account(LOCAL)
            .await?
            .ok_or("fixture local account missing")?;
        let local_uri = activitypub::actor_url(&Url::parse(ORIGIN)?, &local);
        let document = Arc::new(Mutex::new(json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": ACTOR, "type": "Person", "preferredUsername": "profile",
            "name": "Discovered profile", "summary": "Synthetic profile",
            "inbox": format!("{ACTOR}/inbox"),
            "followers": format!("{ACTOR}/followers"),
            "following": format!("{ACTOR}/following"),
            "icon": {"type": "Image", "url": AVATAR},
            "image": {"type": "Image", "url": HEADER},
            "publicKey": {"id": format!("{ACTOR}#main-key"), "owner": ACTOR,
                          "publicKeyPem": local.public_key}
        })));
        let actor_listener = TcpListener::bind("127.0.0.1:0").await?;
        let actor_endpoint = actor_listener.local_addr()?;
        let actor_requests = Arc::new(Mutex::new(Vec::new()));
        let actor_server = tokio::spawn({
            let document = document.clone();
            let requests = actor_requests.clone();
            async move {
                loop {
                    let (mut socket, _) = actor_listener.accept().await?;
                    let request = fixture_delivery_request(&mut socket).await?;
                    let webfinger = String::from_utf8_lossy(&request)
                        .starts_with("GET /.well-known/webfinger?");
                    requests.lock().await.push(request);
                    let body = if webfinger {
                        json!({"subject": format!("acct:profile@{DOMAIN}"), "links": [
                            {"rel": "self", "type": "application/activity+json", "href": ACTOR}
                        ]})
                    } else {
                        document.lock().await.clone()
                    };
                    reply(
                        &mut socket,
                        if webfinger {
                            "application/jrd+json"
                        } else {
                            "application/activity+json"
                        },
                        body.to_string().as_bytes(),
                    )
                    .await?;
                }
            }
        });
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            32,
            24,
            image::Rgba([43, 87, 131, 255]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)?;
        let media_body = Arc::new(Mutex::new(png.into_inner()));
        let media_listener = TcpListener::bind("127.0.0.1:0").await?;
        let media_endpoint = media_listener.local_addr()?;
        let media_requests = Arc::new(AtomicUsize::new(0));
        let pause_media = Arc::new(AtomicBool::new(false));
        let media_started = Arc::new(Notify::new());
        let media_release = Arc::new(Notify::new());
        let media_server = tokio::spawn({
            let body = media_body.clone();
            let count = media_requests.clone();
            let pause = pause_media.clone();
            let started = media_started.clone();
            let release = media_release.clone();
            async move {
                loop {
                    let (mut socket, _) = media_listener.accept().await?;
                    let request = fixture_delivery_request(&mut socket).await?;
                    assert!(String::from_utf8_lossy(&request).starts_with("GET /"));
                    count.fetch_add(1, Ordering::SeqCst);
                    if pause.load(Ordering::SeqCst) {
                        started.notify_one();
                        release.notified().await;
                    }
                    // Deliberately no extension or trusted HTTP image MIME.
                    reply(
                        &mut socket,
                        "application/octet-stream",
                        &body.lock().await.clone(),
                    )
                    .await?;
                }
            }
        });
        let root =
            std::env::temp_dir().join(format!("rustodon-profile-media-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root)?;
        let config = ActivityPubDeliveryConfig {
            origin: Url::parse(ORIGIN)?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
            media_root_url: "/system".to_owned(),
            media_root: Some(PaperclipRoot::open(&root)?),
            limited_federation: false,
            remote_media_endpoint: Some(media_endpoint),
            remote_fetch_endpoint: Some(actor_endpoint),
            remote_delivery_endpoint: Some(actor_endpoint),
        };
        let queue = Queue::new(runtime.clone());
        let executor = Arc::new(WorkerExecutor::new(
            queue.clone(),
            infrastructure_handlers_with_writer_and_mail_and_federation(
                &queue,
                Some(writer.clone()),
                None,
                Some(config.clone()),
            )?,
            1,
            1,
        )?);
        let mut fixture = Self {
            owner,
            writer,
            runtime,
            queue,
            executor,
            config,
            id: 0,
            document,
            actor_requests,
            media_requests,
            media_body,
            pause_media,
            media_started,
            media_release,
            servers: vec![actor_server, media_server],
            root,
        };
        // As with the existing audience discovery test, start after inbox signature
        // verification and let the real ingress worker discover the unknown actor.
        fixture.ingress(json!({
            "id": format!("{ACTOR}/statuses/discovery/activity"), "type": "Create", "actor": ACTOR,
            "object": {"id": format!("{ACTOR}/statuses/discovery"), "type": "Note",
                "attributedTo": ACTOR, "content": "<p>Profile discovery</p>",
                "summary": "", "to": [local_uri]}
        })).await?;
        fixture.id = sqlx::query_scalar("SELECT id FROM accounts WHERE uri = $1")
            .bind(ACTOR)
            .fetch_one(&fixture.writer)
            .await?;
        Ok(fixture)
    }

    fn rebuild_executor(&mut self) -> TestResult {
        self.executor = Arc::new(WorkerExecutor::new(
            self.queue.clone(),
            infrastructure_handlers_with_writer_and_mail_and_federation(
                &self.queue,
                Some(self.writer.clone()),
                None,
                Some(self.config.clone()),
            )?,
            1,
            1,
        )?);
        Ok(())
    }

    async fn ingress(&self, body: Value) -> TestResult {
        self.queue
            .enqueue(&JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "signature_key_id": format!("{ACTOR}#main-key"), "remote_domain": DOMAIN,
                    "delivery_target_account_id": LOCAL, "body": body.to_string()
                }),
            ))
            .await?;
        self.process(Lane::Ingress).await
    }

    async fn update(&self, patch: Value) -> TestResult {
        let mut actor = self.document.lock().await.clone();
        let object = actor.as_object_mut().ok_or("actor object")?;
        object.remove("icon");
        object.remove("image");
        object.extend(patch.as_object().ok_or("patch object")?.clone());
        self.ingress(json!({"type": "Update", "actor": ACTOR, "object": actor}))
            .await
    }

    async fn process(&self, lane: Lane) -> TestResult {
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.executor
                    .process_one("profile-media-test", &[lane], Duration::seconds(30)),
            )
            .await??
        );
        self.assert_acknowledged().await
    }

    async fn assert_acknowledged(&self) -> TestResult {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs WHERE attempts > 0"
            )
            .fetch_one(&self.runtime)
            .await?,
            0,
            "handled jobs must not be retried or dead-lettered"
        );
        Ok(())
    }

    async fn jobs(&self) -> TestResult<Vec<Value>> {
        flush_outbox(&self.queue).await?;
        Ok(sqlx::query_scalar(
            "SELECT arguments FROM rustodon.durable_jobs WHERE kind = $1 ORDER BY id",
        )
        .bind(ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND)
        .fetch_all(&self.runtime)
        .await?)
    }

    async fn state(&self) -> TestResult<Value> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object(
            'avatar_url', avatar_remote_url, 'header_url', header_remote_url,
            'avatar_version', floor(extract(epoch FROM avatar_updated_at))::bigint,
            'header_version', floor(extract(epoch FROM header_updated_at))::bigint,
            'avatar_file', avatar_file_name, 'header_file', header_file_name,
            'avatar_type', avatar_content_type, 'header_type', header_content_type,
            'avatar_size', avatar_file_size, 'header_size', header_file_size,
            'avatar_schema', avatar_storage_schema_version, 'header_schema', header_storage_schema_version
        ) FROM accounts WHERE id = $1").bind(self.id).fetch_one(&self.writer).await?)
    }

    async fn serialized(&self) -> TestResult<Value> {
        let loader = RestProjectionLoader::new(
            Repository::from_pool(self.runtime.clone()),
            None,
            "fixture-v4-6-5.rustodon.invalid",
        );
        let account = loader.account(self.id).await?.ok_or("projection missing")?;
        let origin = Url::parse(ORIGIN)?;
        let serializer = RestSerializer::new(
            &origin,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            Utc::now().naive_utc(),
        );
        Ok(serde_json::to_value(serializer.account(&account)?)?)
    }

    async fn cleanup(&self) -> TestResult {
        sqlx::query("DELETE FROM accounts WHERE id = $1")
            .bind(self.id)
            .execute(&self.owner)
            .await?;
        sqlx::query("DELETE FROM domain_blocks WHERE domain = $1")
            .bind(DOMAIN)
            .execute(&self.owner)
            .await?;
        reset().await
    }
}

async fn reply(socket: &mut tokio::net::TcpStream, mime: &str, body: &[u8]) -> std::io::Result<()> {
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await?;
    socket.write_all(body).await
}

async fn flush_outbox(queue: &Queue) -> TestResult {
    while queue.dispatch_outbox(100).await? != 0 {}
    Ok(())
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn discovered_profile_images_install_decoded_png_and_serialize_cached_urls() -> TestResult {
    assert_eq!(
        ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND,
        "rustodon.activitypub.fetch_profile_media"
    );
    let fixture = Fixture::discover().await?;
    let placeholders = fixture.serialized().await?;
    let jobs = fixture.jobs().await?;
    assert_eq!(jobs.len(), 2);
    let state = fixture.state().await?;
    for (slot, url) in [("avatar", AVATAR), ("header", HEADER)] {
        let job = jobs
            .iter()
            .find(|job| job["slot"] == slot)
            .ok_or("slot job missing")?;
        assert_eq!(job["account_id"], fixture.id);
        assert_eq!(job["actor_uri"], ACTOR);
        assert_eq!(job["domain"], DOMAIN);
        assert_eq!(job["remote_url"], url);
        assert_eq!(job["version"], state[format!("{slot}_version")]);
        assert!(job["version"].as_i64().is_some());
        fixture.process(Lane::Pull).await?;
    }
    assert!(
        fixture.jobs().await?.is_empty(),
        "successful media jobs must be acknowledged"
    );
    assert_eq!(fixture.media_requests.load(Ordering::SeqCst), 2);
    let state = fixture.state().await?;
    let serialized = fixture.serialized().await?;
    for (slot, attachment) in [
        ("avatar", PaperclipAttachment::AccountAvatar),
        ("header", PaperclipAttachment::AccountHeader),
    ] {
        assert_eq!(state[format!("{slot}_type")], "image/png");
        assert!(state[format!("{slot}_size")].as_i64().unwrap_or_default() > 0);
        assert_eq!(state[format!("{slot}_schema")], 1);
        let metadata = PaperclipMetadata {
            attachment,
            id: fixture.id,
            remote: true,
            storage_schema_version: Some(1),
            file_name: state[format!("{slot}_file")]
                .as_str()
                .ok_or("filename missing")?
                .to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        };
        let path = metadata
            .relative_path("original")
            .ok_or("Paperclip path missing")?;
        let bytes = fs::read(fixture.root.join(&path))?;
        assert_eq!(image::guess_format(&bytes)?, image::ImageFormat::Png);
        // Paperclip aliases nonanimated profile images' static URLs to original.
        assert_eq!(serialized[format!("{slot}_static")], serialized[slot]);
        for field in [slot.to_owned(), format!("{slot}_static")] {
            assert_ne!(serialized[&field], placeholders[&field]);
            assert!(
                serialized[&field]
                    .as_str()
                    .ok_or("serialized URL missing")?
                    .contains(&path)
            );
        }
    }
    fixture.cleanup().await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
#[allow(clippy::too_many_lines)]
async fn profile_media_missing_null_and_readd_fence_stale_same_url_jobs() -> TestResult {
    let fixture = Fixture::discover().await?;
    let jobs = fixture.jobs().await?;
    let old_avatar = jobs
        .iter()
        .find(|job| job["slot"] == "avatar")
        .ok_or("avatar job")?
        .clone();
    for _ in 0..2 {
        fixture.process(Lane::Pull).await?;
    }
    let installed = fixture.state().await?;
    let installed_urls = fixture.serialized().await?;
    fixture.update(json!({"name": "Only text changed"})).await?;
    assert_eq!(
        fixture.state().await?,
        installed,
        "missing images must preserve URLs, metadata and versions"
    );
    assert_eq!(
        fixture.serialized().await?["avatar"],
        installed_urls["avatar"]
    );
    assert!(fixture.jobs().await?.is_empty());
    fixture
        .update(json!({"icon": {"type": "Image", "url": AVATAR},
                          "image": {"type": "Image", "url": HEADER}}))
        .await?;
    assert_eq!(
        fixture.state().await?,
        installed,
        "unchanged URLs must not churn cache versions"
    );
    assert!(fixture.jobs().await?.is_empty());

    // Pin into the future: remove and re-add cannot rely on wall-clock seconds
    // advancing, even on a slow test host. This is also a clock-regression case.
    let future = Utc::now().timestamp() + 3600;
    sqlx::query(
        "UPDATE accounts SET avatar_updated_at = to_timestamp($2::bigint)::timestamp WHERE id = $1",
    )
    .bind(fixture.id)
    .bind(future)
    .execute(&fixture.owner)
    .await?;
    let mut old_avatar = old_avatar;
    old_avatar["version"] = json!(future);
    fixture.update(json!({"icon": null, "image": null})).await?;
    let removed = fixture.state().await?;
    for slot in ["avatar", "header"] {
        assert!(
            removed[format!("{slot}_url")]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        );
        for field in ["file", "type", "size"] {
            assert!(
                removed[format!("{slot}_{field}")].is_null(),
                "null must clear {slot} {field}"
            );
        }
    }
    assert!(
        removed["avatar_version"]
            .as_i64()
            .ok_or("removal version missing")?
            > future
    );
    let calls = fixture.media_requests.load(Ordering::SeqCst);
    fixture
        .queue
        .enqueue(&JobSpec::new(
            Lane::Pull,
            ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND,
            old_avatar.clone(),
        ))
        .await?;
    fixture.process(Lane::Pull).await?;
    assert_eq!(fixture.state().await?, removed);
    assert_eq!(fixture.media_requests.load(Ordering::SeqCst), calls);

    // Put old work first, then re-add the exact URL. URL equality alone must not
    // authorize this job, and the new generation must remain installable.
    fixture
        .queue
        .enqueue(&JobSpec::new(
            Lane::Pull,
            ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND,
            old_avatar,
        ))
        .await?;
    fixture
        .update(json!({"icon": {"type": "Image", "url": AVATAR}}))
        .await?;
    let readded = fixture.state().await?;
    assert!(
        readded["avatar_version"].as_i64().ok_or("re-add version")?
            > removed["avatar_version"]
                .as_i64()
                .ok_or("removal version")?
    );
    fixture.jobs().await?;
    fixture.process(Lane::Pull).await?;
    assert_eq!(fixture.state().await?, readded);
    assert_eq!(fixture.media_requests.load(Ordering::SeqCst), calls);
    fixture.process(Lane::Pull).await?;
    assert!(fixture.state().await?["avatar_file"].is_string());
    assert_eq!(fixture.media_requests.load(Ordering::SeqCst), calls + 1);
    fixture.cleanup().await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn profile_media_installation_rechecks_identity_suspension_and_domain_policy() -> TestResult {
    for policy in ["identity", "suspension", "reject_media"] {
        let fixture = Fixture::discover().await?;
        fixture.jobs().await?;
        fixture.pause_media.store(true, Ordering::SeqCst);
        let executor = fixture.executor.clone();
        let worker = tokio::spawn(async move {
            executor
                .process_one("profile-media-fence", &[Lane::Pull], Duration::seconds(30))
                .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fixture.media_started.notified(),
        )
        .await?;
        match policy {
            "identity" => {
                sqlx::query("UPDATE accounts SET uri = uri || '/replaced' WHERE id = $1")
                    .bind(fixture.id)
                    .execute(&fixture.owner)
                    .await?;
            }
            "suspension" => {
                sqlx::query("UPDATE accounts SET suspended_at = clock_timestamp(), suspension_origin = 0 WHERE id = $1")
                    .bind(fixture.id).execute(&fixture.owner).await?;
            }
            _ => {
                sqlx::query(
                    "INSERT INTO domain_blocks (domain, severity, reject_media, reject_reports,
                    obfuscate, created_at, updated_at) VALUES ($1, 0, true, false, false,
                    clock_timestamp(), clock_timestamp())",
                )
                .bind(DOMAIN)
                .execute(&fixture.owner)
                .await?;
            }
        }
        fixture.media_release.notify_one();
        assert!(tokio::time::timeout(std::time::Duration::from_secs(10), worker).await???);
        fixture.assert_acknowledged().await?;
        let state = fixture.state().await?;
        assert!(
            state["avatar_file"].is_null(),
            "{policy} must fence avatar installation"
        );
        assert!(
            state["header_file"].is_null(),
            "{policy} must fence header installation"
        );
        // The second slot starts after the policy change and must not fetch at all.
        fixture.pause_media.store(false, Ordering::SeqCst);
        fixture.process(Lane::Pull).await?;
        assert_eq!(
            fixture.media_requests.load(Ordering::SeqCst),
            1,
            "{policy} pre-fetch fence"
        );
        fixture.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn operator_profile_refresh_fetches_signed_canonical_actor_and_preserves_local_state()
-> TestResult {
    let fixture = Fixture::discover().await?;
    fixture.jobs().await?;
    for _ in 0..2 {
        fixture.process(Lane::Pull).await?;
    }
    sqlx::query(
        "UPDATE accounts SET url = 'http://wrong-profile.fixture.invalid/not-an-actor',
        public_key = (SELECT public_key FROM keypairs WHERE account_id = $1 ORDER BY id LIMIT 1),
        suspended_at = '2026-01-02'::timestamp, suspension_origin = 0 WHERE id = $1",
    )
    .bind(fixture.id)
    .execute(&fixture.owner)
    .await?;
    sqlx::query(
        "INSERT INTO follows (account_id, target_account_id, created_at, updated_at)
        VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
    )
    .bind(LOCAL)
    .bind(fixture.id)
    .execute(&fixture.owner)
    .await?;
    let keys: Value = sqlx::query_scalar(
        "SELECT coalesce(jsonb_agg(to_jsonb(k) ORDER BY k.id), '[]'::jsonb)
        FROM keypairs k WHERE account_id = $1",
    )
    .bind(fixture.id)
    .fetch_one(&fixture.writer)
    .await?;
    assert!(!keys.as_array().ok_or("key array")?.is_empty());
    let public_key: String = sqlx::query_scalar("SELECT public_key FROM accounts WHERE id = $1")
        .bind(fixture.id)
        .fetch_one(&fixture.writer)
        .await?;
    assert!(public_key.contains("BEGIN PUBLIC KEY"));
    fixture.actor_requests.lock().await.clear();
    {
        let mut document = fixture.document.lock().await;
        document.as_object_mut().ok_or("actor")?.remove("publicKey");
        document["name"] = json!("Operator refreshed");
        document["suspended"] = json!(false);
        document["icon"]["url"] = json!("http://profile-images.fixture.invalid/refreshed.blob");
    }
    refresh_remote_account(fixture.writer.clone(), &fixture.config, fixture.id)
        .await
        .map_err(|error| format!("refresh failed: {error:?}"))?;
    let requests = fixture.actor_requests.lock().await;
    let first = String::from_utf8_lossy(requests.first().ok_or("canonical actor fetch missing")?);
    assert!(first.starts_with("GET /users/profile "), "{first}");
    let headers = first.to_ascii_lowercase();
    assert!(headers.contains("\r\nhost: profile-media.fixture.invalid\r\n"));
    assert!(
        headers.contains("\r\nsignature: "),
        "operator fetch must be signed"
    );
    drop(requests);
    let state: (String, String, Option<NaiveDateTime>, Option<i32>) = sqlx::query_as(
        "SELECT display_name, public_key, suspended_at, suspension_origin FROM accounts WHERE id = $1")
        .bind(fixture.id).fetch_one(&fixture.writer).await?;
    assert_eq!(state.0, "Operator refreshed");
    assert_eq!(state.1, public_key);
    assert_eq!(
        state.2,
        Some(DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")?.naive_utc())
    );
    assert_eq!(state.3, Some(0));
    assert_eq!(
        sqlx::query_scalar::<_, Value>(
            "SELECT coalesce(jsonb_agg(to_jsonb(k) ORDER BY k.id), '[]'::jsonb)
        FROM keypairs k WHERE account_id = $1"
        )
        .bind(fixture.id)
        .fetch_one(&fixture.writer)
        .await?,
        keys
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2"
        )
        .bind(LOCAL)
        .bind(fixture.id)
        .fetch_one(&fixture.writer)
        .await?,
        1
    );
    // A local suspension cannot be undone by the actor document or install images.
    for _ in fixture.jobs().await? {
        fixture.process(Lane::Pull).await?;
    }
    assert_eq!(fixture.media_requests.load(Ordering::SeqCst), 2);
    fixture.cleanup().await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn operator_refresh_repairs_missing_and_truncated_files_and_missing_metadata() -> TestResult {
    let fixture = Fixture::discover().await?;
    fixture.jobs().await?;
    for _ in 0..2 {
        fixture.process(Lane::Pull).await?;
    }
    for corrupt in [false, true] {
        let state = fixture.state().await?;
        let metadata = PaperclipMetadata {
            attachment: PaperclipAttachment::AccountAvatar,
            id: fixture.id,
            remote: true,
            storage_schema_version: Some(1),
            file_name: state["avatar_file"]
                .as_str()
                .ok_or("avatar file")?
                .to_owned(),
            content_type: Some("image/png".to_owned()),
            variant: None,
        };
        let path = fixture
            .root
            .join(metadata.relative_path("original").ok_or("path")?);
        if corrupt {
            fs::write(&path, b"partial")?;
        } else {
            fs::remove_file(&path)?;
        }
        refresh_remote_account(fixture.writer.clone(), &fixture.config, fixture.id).await?;
        assert_eq!(fixture.jobs().await?.len(), 2);
        for _ in 0..2 {
            fixture.process(Lane::Pull).await?;
        }
        assert_eq!(
            image::guess_format(&fs::read(path)?)?,
            image::ImageFormat::Png
        );
    }
    let requests = fixture.media_requests.load(Ordering::SeqCst);
    sqlx::query(
        "UPDATE accounts SET header_content_type = NULL, header_file_size = NULL WHERE id = $1",
    )
    .bind(fixture.id)
    .execute(&fixture.owner)
    .await?;
    fixture.update(json!({"image": {"url": HEADER}})).await?;
    assert_eq!(fixture.jobs().await?.len(), 1);
    fixture.process(Lane::Pull).await?;
    assert_eq!(fixture.media_requests.load(Ordering::SeqCst), requests + 1);
    assert_eq!(fixture.state().await?["header_type"], "image/png");
    fixture.cleanup().await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn profile_refresh_repairs_truncated_gif_static_derivative() -> TestResult {
    let fixture = Fixture::discover().await?;
    let mut gif = Vec::new();
    let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
    for _ in 0..2 {
        encoder.encode_frame(image::Frame::new(image::RgbaImage::new(2, 2)))?;
    }
    drop(encoder);
    *fixture.media_body.lock().await = gif;
    fixture.jobs().await?;
    for _ in 0..2 {
        fixture.process(Lane::Pull).await?;
    }
    let state = fixture.state().await?;
    assert_eq!(state["avatar_type"], "image/gif");
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::AccountAvatar,
        id: fixture.id,
        remote: true,
        storage_schema_version: Some(1),
        file_name: state["avatar_file"]
            .as_str()
            .ok_or("avatar file")?
            .to_owned(),
        content_type: Some("image/gif".to_owned()),
        variant: None,
    };
    let path = fixture
        .root
        .join(metadata.relative_path("static").ok_or("static path")?);
    fs::write(&path, b"partial")?;
    refresh_remote_account(fixture.writer.clone(), &fixture.config, fixture.id).await?;
    fixture.jobs().await?;
    for _ in 0..2 {
        fixture.process(Lane::Pull).await?;
    }
    assert_eq!(
        image::guess_format(&fs::read(path)?)?,
        image::ImageFormat::Png
    );
    fixture.cleanup().await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn profile_media_limited_federation_requires_actor_and_cdn_allow() -> TestResult {
    let mut fixture = Fixture::discover().await?;
    fixture.config.limited_federation = true;
    fixture.rebuild_executor()?;
    sqlx::query("INSERT INTO domain_allows (domain, created_at, updated_at) VALUES ($1, clock_timestamp(), clock_timestamp())")
        .bind(DOMAIN).execute(&fixture.owner).await?;
    fixture.jobs().await?;
    for _ in 0..2 {
        fixture.process(Lane::Pull).await?;
    }
    assert_eq!(fixture.media_requests.load(Ordering::SeqCst), 0);
    sqlx::query("INSERT INTO domain_allows (domain, created_at, updated_at) VALUES ('profile-images.fixture.invalid', clock_timestamp(), clock_timestamp())")
        .execute(&fixture.owner).await?;
    refresh_remote_account(fixture.writer.clone(), &fixture.config, fixture.id).await?;
    fixture.jobs().await?;
    for _ in 0..2 {
        fixture.process(Lane::Pull).await?;
    }
    assert_eq!(fixture.media_requests.load(Ordering::SeqCst), 2);
    sqlx::query("DELETE FROM domain_allows WHERE domain IN ($1, 'profile-images.fixture.invalid')")
        .bind(DOMAIN)
        .execute(&fixture.owner)
        .await?;
    fixture.cleanup().await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn profile_media_reconciliation_survives_ambiguous_commit_and_removal() -> TestResult {
    let mut fixture = Fixture::discover().await?;
    fixture.config.media_root = Some(
        PaperclipRoot::open(&fixture.root)?
            .with_commit_fault(PaperclipCommitFault::before_and_after()),
    );
    fixture.rebuild_executor()?;
    fixture.jobs().await?;
    // Restrict this fault test to the avatar so retries run in a predictable order.
    sqlx::query(
        "DELETE FROM rustodon.durable_jobs WHERE kind = $1 AND arguments->>'slot' = 'header'",
    )
    .bind(ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND)
    .execute(&fixture.runtime)
    .await?;
    for _ in 0..2 {
        assert!(
            fixture
                .executor
                .process_one("profile-commit-fault", &[Lane::Pull], Duration::seconds(30))
                .await?
        );
        assert!(
            fixture
                .executor
                .process_one(
                    "profile-reconcile",
                    &[Lane::Maintenance],
                    Duration::seconds(30)
                )
                .await?
        );
        sqlx::query("UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE kind = $1")
            .bind(ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND)
            .execute(&fixture.runtime)
            .await?;
    }
    fixture.process(Lane::Pull).await?;
    let state = fixture.state().await?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::AccountAvatar,
        id: fixture.id,
        remote: true,
        storage_schema_version: Some(1),
        file_name: state["avatar_file"]
            .as_str()
            .ok_or("avatar file")?
            .to_owned(),
        content_type: Some("image/png".to_owned()),
        variant: None,
    };
    let path = fixture
        .root
        .join(metadata.relative_path("original").ok_or("path")?);
    assert!(path.exists());
    fixture.update(json!({"icon": null, "image": null})).await?;
    fixture.jobs().await?;
    fixture.process(Lane::Maintenance).await?;
    assert!(!path.exists());
    fixture.cleanup().await
}
