use super::*;
use futures_util::FutureExt;

const SENDER: i64 = -99110;
const REMOTE_SENDER: i64 = -99111;
const FOLLOWER: i64 = -331;
const SENDER_ORIGIN: &str = "https://sender.fixture.invalid/";
const ACTOR: &str = "https://sender.fixture.invalid/users/update_sender";

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn delivered_same_second_status_updates_have_stable_distinct_ids() -> TestResult {
    delivered_updates(false).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
async fn delivered_same_second_profile_updates_have_stable_distinct_ids() -> TestResult {
    delivered_updates(true).await
}

// Sender and receiver use separate disposable fixture databases and origins. Wire bodies
// pass unchanged through signed delivery, the real inbox router, and ingress worker.
#[allow(clippy::too_many_lines)]
async fn delivered_updates(profile: bool) -> TestResult {
    let pool = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    reset().await?;
    let queue = Queue::new(runtime);
    let remote_pool = sqlx::PgPool::connect(&std::env::var(
        "RUSTODON_WORKER_RECEIVER_OWNER_DATABASE_URL",
    )?)
    .await?;
    let remote_runtime =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_RECEIVER_DATABASE_URL")?).await?;
    let receiver_queue = Queue::new(remote_runtime.clone());
    for (id, domain, uri) in [
        (SENDER, None, ""),
        (REMOTE_SENDER, Some("sender.fixture.invalid"), ACTOR),
    ] {
        let pool = if id == SENDER { &pool } else { &remote_pool };
        sqlx::query("INSERT INTO accounts (id, username, domain, uri, public_key, private_key, protocol, id_scheme, created_at, updated_at) SELECT $1, 'update_sender', $2, $3, public_key, CASE WHEN $2::varchar IS NULL THEN private_key ELSE NULL END, 1, 0, '2026-07-01'::timestamp, '2026-07-01'::timestamp FROM accounts WHERE id = $4")
            .bind(id).bind(domain).bind(uri).bind(ALICE).execute(pool).await?;
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let host = format!("remote.fixture.invalid:{}", endpoint.port());
    let inbox = format!("http://{host}/inbox");
    let previous_inboxes: (String, String) =
        sqlx::query_as("SELECT inbox_url, shared_inbox_url FROM accounts WHERE id = $1")
            .bind(FOLLOWER)
            .fetch_one(&pool)
            .await?;
    sqlx::query("UPDATE accounts SET inbox_url = $2, shared_inbox_url = $2 WHERE id = $1")
        .bind(FOLLOWER)
        .bind(&inbox)
        .execute(&pool)
        .await?;
    for (source, target) in [(FOLLOWER, SENDER), (ALICE, REMOTE_SENDER)] {
        let pool = if target == SENDER {
            &pool
        } else {
            &remote_pool
        };
        sqlx::query("INSERT INTO follows (account_id, target_account_id, created_at, updated_at) VALUES ($1, $2, clock_timestamp(), clock_timestamp())")
            .bind(source).bind(target).execute(pool).await?;
    }
    let media_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("update-versions-{}", std::process::id()));
    fs::create_dir_all(&media_root)?;
    let state = rustodon::web::WebState::new(
        Repository::from_pool(remote_runtime),
        Url::parse(ORIGIN)?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        media_root.clone(),
        InstanceRuntimeConfig {
            domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
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
            translation_enabled: false,
            limited_federation: false,
            single_user_mode: false,
            terms_of_service_url: None,
            sso_signup_url: None,
            wrapstodon: None,
        },
        Vec::new(),
        vec![host],
    )?
    .with_queue(receiver_queue.clone());
    let requests = Arc::new(tokio::sync::Mutex::new(
        Vec::<(Vec<u8>, http::StatusCode)>::new(),
    ));
    let fail_once = Arc::new(AtomicUsize::new(0));
    let app = rustodon::web::router(state).layer(axum::middleware::from_fn({
        let requests = requests.clone();
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let requests = requests.clone();
            let fail_once = fail_once.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 1_048_576).await.unwrap();
                let is_update =
                    serde_json::from_slice::<Value>(&bytes).unwrap()["type"] == "Update";
                let mut response = next
                    .run(http::Request::from_parts(
                        parts,
                        axum::body::Body::from(bytes.clone()),
                    ))
                    .await;
                requests
                    .lock()
                    .await
                    .push((bytes.to_vec(), response.status()));
                // The receiver commits acceptance, but the sender sees a transient failure.
                // Its retry must replay exactly the stored body/identity without extra ingress.
                if is_update
                    && response.status() == http::StatusCode::ACCEPTED
                    && fail_once.fetch_add(1, Ordering::SeqCst) == 0
                {
                    *response.status_mut() = http::StatusCode::SERVICE_UNAVAILABLE;
                }
                response
            }
        }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let mut sender_config = federation_config();
    sender_config.origin = Url::parse(SENDER_ORIGIN)?;
    "account-domain.invalid".clone_into(&mut sender_config.local_domain);
    sender_config.remote_delivery_endpoint = Some(endpoint);
    let sender = WorkerExecutor::new(
        queue.clone(),
        infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(pool.clone()),
            None,
            Some(sender_config),
        )?,
        1,
        1,
    )?;
    let receiver = WorkerExecutor::new(
        receiver_queue.clone(),
        infrastructure_handlers_with_writer_and_mail_and_federation(
            &receiver_queue,
            Some(remote_pool.clone()),
            None,
            Some(federation_config()),
        )?,
        1,
        1,
    )?;
    let status_id: i64 = sqlx::query_scalar("INSERT INTO statuses (account_id, text, spoiler_text, visibility, local, language, created_at, updated_at) VALUES ($1, 'original', 'version test', 0, true, 'en', '2026-07-01'::timestamp, '2026-07-01'::timestamp) RETURNING id")
        .bind(SENDER).fetch_one(&pool).await?;
    // Legacy local rows serialize a tag atomUri even though their AP ID is HTTPS.
    // Its tagging authority need not equal WEB_DOMAIN and must not grant alias authority.
    let atom_uri =
        format!("tag:account-domain.invalid,2026-07-01:objectId={status_id}:objectType=Status");
    sqlx::query("UPDATE statuses SET uri = $2 WHERE id = $1")
        .bind(status_id)
        .bind(&atom_uri)
        .execute(&pool)
        .await?;
    let victim_status_id: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (account_id, text, spoiler_text, visibility, local, uri,
          created_at, updated_at) VALUES ($1, 'untouched legacy victim', '', 0, false, $2,
          clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(BOB)
    .bind(&atom_uri)
    .fetch_one(&remote_pool)
    .await?;
    let result = std::panic::AssertUnwindSafe(async {
    if !profile {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Push,
                    ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                    json!({"status_id": status_id, "activity_type": "Create"}),
                )
                .logical_key("version:create"),
            )
            .await?;
        distribute_and_deliver(&queue, &sender).await?;
        apply_ingress(&receiver_queue, &receiver).await?;
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT text FROM statuses WHERE account_id = $1")
                .bind(REMOTE_SENDER)
                .fetch_one(&remote_pool)
                .await?,
            "<p>original</p>"
        );
    }
    let base = DateTime::parse_from_rfc3339("2026-07-02T12:00:00.123456Z")?.with_timezone(&Utc);
    let mut delivered = Vec::new();
    for (version, text) in [
        (base, "version A"),
        (base + Duration::microseconds(1), "version B"),
    ] {
        assert_eq!(
            version.timestamp(),
            base.timestamp(),
            "deterministic same-second versions"
        );
        let micros = version.timestamp_micros();
        if profile {
            sqlx::query(
                "UPDATE accounts SET display_name = $2, note = $2, updated_at = $3 WHERE id = $1",
            )
            .bind(SENDER)
            .bind(text)
            .bind(version.naive_utc())
            .execute(&pool)
            .await?;
            queue
                .enqueue(
                    &JobSpec::new(
                        Lane::Push,
                        ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
                        json!({"account_id": SENDER, "updated_at_micros": micros}),
                    )
                    .logical_key(format!("version:profile:{micros}")),
                )
                .await?;
        } else {
            sqlx::query(
                "UPDATE statuses SET text = $2, edited_at = $3, updated_at = $3 WHERE id = $1",
            )
            .bind(status_id)
            .bind(text)
            .bind(version.naive_utc())
            .execute(&pool)
            .await?;
            queue.enqueue(&JobSpec::new(Lane::Push, ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND, json!({"status_id": status_id, "activity_type": "Update", "edited_at_micros": micros})).logical_key(format!("version:status:{micros}"))).await?;
        }
        distribute_and_deliver(&queue, &sender).await?;
        let (body, response) = requests
            .lock()
            .await
            .last()
            .cloned()
            .ok_or("Update never reached receiver")?;
        assert_eq!(
            response,
            http::StatusCode::ACCEPTED,
            "distinct delivered same-second versions must not conflict at the receiver"
        );
        delivered.push(serde_json::from_slice::<Value>(&body)?);
        apply_ingress(&receiver_queue, &receiver).await?;
        if profile {
            let remote: (String, String) =
                sqlx::query_as("SELECT display_name, note FROM accounts WHERE id = $1")
                    .bind(REMOTE_SENDER)
                    .fetch_one(&remote_pool)
                    .await?;
            assert_eq!(remote, (text.to_owned(), format!("<p>{text}</p>")));
        } else {
            let remote: (String, NaiveDateTime) =
                sqlx::query_as("SELECT text, edited_at FROM statuses WHERE account_id = $1")
                    .bind(REMOTE_SENDER)
                    .fetch_one(&remote_pool)
                    .await?;
            assert_eq!(remote, (format!("<p>{text}</p>"), version.naive_utc()));
        }
        if version == base {
            // Retry after the receiver has already applied A, before B exists.
            sqlx::query(
                "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE kind = $1",
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .execute(&pool)
            .await?;
            sqlx::query("UPDATE rustodon.domain_health SET retry_at = clock_timestamp() - interval '1 second'")
                .execute(&pool).await?;
            assert!(
                sender
                    .process_one("version-retry", &[Lane::Push], Duration::seconds(30))
                    .await?
            );
            let (retry_body, retry_response) = requests.lock().await.last().cloned().unwrap();
            assert_eq!(retry_response, http::StatusCode::ACCEPTED);
            assert_eq!(
                retry_body, body,
                "retry must preserve exact JSON bytes and wire ID"
            );
            assert!(
                !receiver
                    .process_one("duplicate-ingress", &[Lane::Ingress], Duration::seconds(30))
                    .await?,
                "identical retry must not enqueue another ingress application"
            );
        }
        for queue in [&queue, &receiver_queue] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 OR kind = $2"
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .bind(ACTIVITYPUB_INBOX_JOB_KIND)
            .fetch_one(queue.pool())
            .await?,
            0,
            "both sides must finish, without retrying or dead-letter jobs"
        );
        }
    }
    assert_ne!(delivered[0]["id"], delivered[1]["id"]);
    assert_eq!(delivered[0]["object"]["id"], delivered[1]["object"]["id"]);
    assert_ne!(delivered[0]["object"], delivered[1]["object"]);
    assert_eq!(requests.lock().await.len(), if profile { 3 } else { 4 });
    if !profile {
        assert_eq!(delivered[0]["object"]["atomUri"], atom_uri);
        sqlx::query("UPDATE statuses SET deleted_at = clock_timestamp() WHERE id = $1")
            .bind(status_id).execute(&pool).await?;
        queue.enqueue(&JobSpec::new(
            Lane::Push, ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
            json!({"status_id": status_id, "activity_type": "Delete"}),
        ).logical_key("version:delete")).await?;
        distribute_and_deliver(&queue, &sender).await?;
        let (body, response) = requests.lock().await.last().cloned().unwrap();
        assert_eq!(response, http::StatusCode::ACCEPTED);
        let delete: Value = serde_json::from_slice(&body)?;
        assert_eq!(delete["object"]["atomUri"], atom_uri);
        apply_ingress(&receiver_queue, &receiver).await?;
        assert!(sqlx::query_scalar::<_, bool>(
            "SELECT deleted_at IS NOT NULL FROM statuses WHERE account_id = $1",
        ).bind(REMOTE_SENDER).fetch_one(&remote_pool).await?);
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM tombstones WHERE uri = $1",
        ).bind(&atom_uri).fetch_one(&remote_pool).await?, 0,
        "opaque tags must never acquire tombstone authority");
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM tombstones WHERE account_id = $1 AND uri = $2",
        ).bind(REMOTE_SENDER).bind(delivered[0]["object"]["id"].as_str())
            .fetch_one(&remote_pool).await?, 1,
        "the canonical ActivityPub ID must retain its deletion tombstone");
    }
    assert_eq!(sqlx::query_as::<_, (String, Option<NaiveDateTime>)>(
        "SELECT text, deleted_at FROM statuses WHERE id = $1",
    ).bind(victim_status_id).fetch_one(&remote_pool).await?,
    ("untouched legacy victim".to_owned(), None),
    "a tag matching another account's legacy URI grants no mutation authority");
    Ok::<(), Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
    server.abort();
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(victim_status_id)
        .execute(&remote_pool)
        .await?;
    for pool in [&pool, &remote_pool] {
        sqlx::query("DELETE FROM tombstones WHERE account_id = ANY($1)")
            .bind(vec![SENDER, REMOTE_SENDER])
            .execute(pool)
            .await?;
        sqlx::query("DELETE FROM status_edits WHERE status_id IN (SELECT id FROM statuses WHERE account_id = ANY($1))").bind(vec![SENDER, REMOTE_SENDER]).execute(pool).await?;
        sqlx::query("DELETE FROM status_stats WHERE status_id IN (SELECT id FROM statuses WHERE account_id = ANY($1))").bind(vec![SENDER, REMOTE_SENDER]).execute(pool).await?;
        sqlx::query("DELETE FROM statuses WHERE account_id = ANY($1)")
            .bind(vec![SENDER, REMOTE_SENDER])
            .execute(pool)
            .await?;
        sqlx::query(
            "DELETE FROM follows WHERE account_id = ANY($1) OR target_account_id = ANY($1)",
        )
        .bind(vec![SENDER, REMOTE_SENDER])
        .execute(pool)
        .await?;
        for table in ["keypairs", "account_stats"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE account_id = ANY($1)"))
                .bind(vec![SENDER, REMOTE_SENDER])
                .execute(pool)
                .await?;
        }
        sqlx::query("DELETE FROM accounts WHERE id = ANY($1)")
            .bind(vec![SENDER, REMOTE_SENDER])
            .execute(pool)
            .await?;
        sqlx::raw_sql("TRUNCATE rustodon.durable_jobs, rustodon.outbox_events, rustodon.idempotency_keys, rustodon.ordering_markers, rustodon.domain_health, rustodon.rate_limit_windows")
        .execute(pool).await?;
    }
    sqlx::query("UPDATE accounts SET inbox_url = $2, shared_inbox_url = $3 WHERE id = $1")
        .bind(FOLLOWER)
        .bind(previous_inboxes.0)
        .bind(previous_inboxes.1)
        .execute(&pool)
        .await?;
    fs::remove_dir_all(media_root)?;
    reset().await?;
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

async fn distribute_and_deliver(queue: &Queue, sender: &WorkerExecutor) -> TestResult {
    assert!(
        sender
            .process_one("version-fanout", &[Lane::Push], Duration::seconds(30))
            .await?
    );
    while queue.dispatch_outbox(100).await? > 0 {}
    assert!(
        sender
            .process_one("version-delivery", &[Lane::Push], Duration::seconds(30))
            .await?
    );
    Ok(())
}

async fn apply_ingress(queue: &Queue, receiver: &WorkerExecutor) -> TestResult {
    assert!(
        receiver
            .process_one("version-ingress", &[Lane::Ingress], Duration::seconds(30))
            .await?
    );
    let remaining: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT logical_key, last_error FROM rustodon.durable_jobs WHERE kind = $1")
            .bind(ACTIVITYPUB_INBOX_JOB_KIND)
            .fetch_all(queue.pool())
            .await?;
    assert!(
        remaining.is_empty(),
        "receiver must apply the Update: {remaining:?}"
    );
    Ok(())
}
