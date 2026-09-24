use super::*;

const ALICE: i64 = 116_844_606_259_201_001;
const BOB: i64 = 116_844_606_259_202_001;
const REMOTE_TARGET: i64 = 116_845_093_847_045_103;
const ACCEPT_STATUS: i64 = -9_401;
const REJECT_STATUS: i64 = -9_402;
const ACCEPT_QUOTE: i64 = -9_411;
const REJECT_QUOTE: i64 = -9_412;
const LEGACY_STATUS: i64 = -9_405;
const LEGACY_QUOTE: i64 = -9_415;
const REMOTE_QUOTING_STATUS: i64 = -9_406;
const REMOTE_QUOTE: i64 = -9_416;
const INBOUND_ALLOW_TARGET: i64 = -9_407;
const INBOUND_DENY_TARGET: i64 = -9_408;
const INBOUND_ALLOWED_URI: &str = "https://remote.fixture.invalid/statuses/worker-inbound-allowed";
const INBOUND_DENIED_URI: &str = "https://remote.fixture.invalid/statuses/worker-inbound-denied";
const INBOUND_SCALAR_URI: &str = "http://remote.fixture.invalid/statuses/worker-inbound-scalar";
const INBOUND_ALLOWED_REQUEST: &str =
    "https://remote.fixture.invalid/activities/worker-inbound-allowed-request";
const INBOUND_DENIED_REQUEST: &str =
    "https://remote.fixture.invalid/activities/worker-inbound-denied-request";
const INBOUND_SCALAR_REQUEST: &str =
    "https://remote.fixture.invalid/activities/worker-inbound-scalar-request";
const REMOTE_QUOTING_URI: &str = "https://remote.fixture.invalid/statuses/worker-quoting-note";
const REMOTE_QUOTE_REQUEST: &str =
    "https://remote.fixture.invalid/activities/worker-quoting-note-request";
const LEGACY_APPROVAL: &str = "https://remote.fixture.invalid/activities/worker-legacy-approval";
const FORWARD_REBLOG: i64 = -9_403;
const FORWARD_FOLLOW: i64 = -9_404;
const FORWARD_FOLLOWER: i64 = -320;
const ACCEPT_REQUEST: &str =
    "https://fixture-v4-6-5.rustodon.invalid/users/alice/quote_requests/worker-accept";
const REJECT_REQUEST: &str =
    "https://fixture-v4-6-5.rustodon.invalid/users/alice/quote_requests/worker-reject";
const APPROVAL: &str = "https://remote.fixture.invalid/activities/worker-quote-approval";
const STALE_APPROVAL: &str =
    "https://remote.fixture.invalid/activities/worker-stale-quote-approval";

fn federation_config() -> ActivityPubDeliveryConfig {
    ActivityPubDeliveryConfig {
        origin: Url::parse("https://fixture-v4-6-5.rustodon.invalid/").unwrap(),
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
    }
}

async fn process_activity(
    queue: &Queue,
    executor: &WorkerExecutor,
    body: Value,
    suffix: &str,
    expect_failure: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({
                    "body": body.to_string(),
                    "delivery_target_account_id": ALICE,
                    "signature_key_id": "https://remote.fixture.invalid/users/bob#secondary-key",
                    "remote_domain": "remote.fixture.invalid",
                }),
            )
            .logical_key(format!("quote-lifecycle:{suffix}")),
        )
        .await?;
    if !executor
        .process_one(
            "quote-lifecycle-worker",
            &[Lane::Ingress],
            Duration::seconds(30),
        )
        .await?
    {
        return Err(format!("quote lifecycle activity was not processed: {suffix}").into());
    }
    let failure: Option<String> =
        sqlx::query_scalar("SELECT last_error FROM rustodon.durable_jobs WHERE logical_key = $1")
            .bind(format!("quote-lifecycle:{suffix}"))
            .fetch_optional(queue.pool())
            .await?
            .flatten();
    match (expect_failure, failure) {
        (false, Some(failure)) => Err(format!("quote lifecycle activity failed: {failure}").into()),
        (true, None) => Err("invalid quote lifecycle activity was not rejected".into()),
        _ => Ok(()),
    }
}

async fn quote_update_effect_counts(
    pool: &sqlx::PgPool,
    status_id: i64,
) -> Result<(i64, i64), sqlx::Error> {
    sqlx::query_as(
        "SELECT
           -- One update fans out to a global event plus one per recipient; they
           -- share the version suffix of their logical keys.
           (SELECT count(DISTINCT regexp_replace(logical_key, '^.*:', ''))
              FROM rustodon.outbox_events
             WHERE kind = 'rustodon.mastodon.stream_event'
               AND (payload ->> 'object_id')::bigint = $1
               AND payload ->> 'event' = 'status.update'),
           (SELECT count(*) FROM rustodon.outbox_events
             WHERE kind = $2 AND (payload #>> '{arguments,status_id}')::bigint = $1
               AND payload #>> '{arguments,update_kind}' = 'quote')
           +
           (SELECT count(*) FROM rustodon.durable_jobs
             WHERE kind = $2 AND (arguments ->> 'status_id')::bigint = $1
               AND arguments ->> 'update_kind' = 'quote')",
    )
    .bind(status_id)
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .fetch_one(pool)
    .await
}

async fn queue_quote_delivery(
    queue: &Queue,
    pool: &sqlx::PgPool,
    request_uri: &str,
    delivery_kind: &str,
) -> Result<(String, Value), Box<dyn std::error::Error>> {
    let (event_id, logical_key, arguments): (i64, String, Value) = sqlx::query_as(
        "SELECT id, logical_key, payload -> 'arguments' \
           FROM rustodon.outbox_events \
          WHERE dispatched_at IS NULL AND kind = $1 \
            AND payload #>> '{arguments,quote_request_uri}' = $2 \
            AND payload #>> '{arguments,quote_delivery_kind}' = $3 \
          ORDER BY id LIMIT 1",
    )
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(request_uri)
    .bind(delivery_kind)
    .fetch_one(pool)
    .await?;
    queue
        .enqueue(
            &JobSpec::new(Lane::Push, ACTIVITYPUB_DELIVERY_JOB_KIND, arguments.clone())
                .logical_key(logical_key.clone()),
        )
        .await?;
    sqlx::query(
        "UPDATE rustodon.outbox_events SET dispatched_at = clock_timestamp() \
          WHERE id = $1 AND dispatched_at IS NULL",
    )
    .bind(event_id)
    .execute(pool)
    .await?;
    Ok((logical_key, arguments))
}

async fn expire_quote_delivery_lease(
    queue: &Queue,
    job: &rustodon::jobs::ClaimedJob,
) -> Result<(), Box<dyn std::error::Error>> {
    let changed = sqlx::query(
        "UPDATE rustodon.durable_jobs SET lease_expires_at = clock_timestamp() - interval '1 second' \
          WHERE id = $1 AND lease_owner = $2 AND lease_generation = $3 AND dead_at IS NULL",
    )
    .bind(job.id)
    .bind(&job.lease_owner)
    .bind(job.generation)
    .execute(queue.pool())
    .await?
    .rows_affected();
    if changed != 1 {
        return Err("quote delivery lease could not be expired deterministically".into());
    }
    Ok(())
}

/// Records each delivery in `seen` as it arrives, so a timeout can report them.
async fn quote_delivery_server(
    listener: TcpListener,
    count: usize,
    seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) -> Result<Vec<Vec<u8>>, std::io::Error> {
    let mut requests = Vec::with_capacity(count);
    for _ in 0..count {
        let (mut socket, _) = listener.accept().await?;
        let request = fixture_delivery_request(&mut socket).await?;
        let text = String::from_utf8_lossy(&request);
        let first_line = text.lines().next().unwrap_or_default().to_owned();
        let body = text.split("\r\n\r\n").nth(1).unwrap_or_default();
        let summary = serde_json::from_str::<Value>(body).map_or_else(
            |_| {
                format!(
                    "{first_line} (unparsed body: {})",
                    body.chars().take(120).collect::<String>()
                )
            },
            |body| format!("{first_line} {} {}", body["type"], body["id"]),
        );
        seen.lock().expect("delivery log").push(summary);
        requests.push(request);
        socket
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
    }
    Ok(requests)
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture"]
#[allow(clippy::too_many_lines)]
async fn quote_federation_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    reset().await?;
    // Mastodon creates status_stats rows lazily; a missing row means zero quotes.
    let baseline: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT quotes_count FROM status_stats WHERE status_id = $1), 0)",
    )
    .bind(REMOTE_TARGET)
    .fetch_one(&owner)
    .await?;
    let (target_ap_id, target_web_link): (String, Option<String>) =
        sqlx::query_as("SELECT uri, url FROM statuses WHERE id = $1")
            .bind(REMOTE_TARGET)
            .fetch_one(&owner)
            .await?;
    let target_web_link = target_web_link.unwrap_or_else(|| target_ap_id.clone());
    let bob_inboxes: (String, String, i32) =
        sqlx::query_as("SELECT inbox_url, shared_inbox_url, protocol FROM accounts WHERE id = $1")
            .bind(BOB)
            .fetch_one(&owner)
            .await?;
    let delivery_inbox = "http://remote.fixture.invalid/inbox";

    for (status_id, text) in [
        (ACCEPT_STATUS, "Worker quote awaiting approval"),
        (REJECT_STATUS, "Worker quote awaiting denial"),
        (LEGACY_STATUS, "Worker legacy quote already accepted"),
        (INBOUND_ALLOW_TARGET, "Worker inbound quote target"),
        (INBOUND_DENY_TARGET, "Worker denied inbound quote target"),
    ] {
        sqlx::query(
            "INSERT INTO statuses (
                 id, account_id, created_at, local, quote_approval_policy, reply, sensitive,
                 spoiler_text, text, updated_at, visibility)
             VALUES ($1, $2, clock_timestamp(), true, 0, false, false, '', $3,
                     clock_timestamp(), 0)",
        )
        .bind(status_id)
        .bind(ALICE)
        .bind(text)
        .execute(&owner)
        .await?;
    }
    sqlx::query("UPDATE statuses SET quote_approval_policy = $2 WHERE id = $1")
        .bind(INBOUND_ALLOW_TARGET)
        .bind(2_i32 << 16)
        .execute(&owner)
        .await?;
    for status_id in [INBOUND_ALLOW_TARGET, INBOUND_DENY_TARGET] {
        sqlx::query(
            "INSERT INTO status_stats (status_id, created_at, updated_at, quotes_count) \
             VALUES ($1, clock_timestamp(), clock_timestamp(), 0)",
        )
        .bind(status_id)
        .execute(&owner)
        .await?;
    }
    for (quote_id, status_id, request) in [
        (ACCEPT_QUOTE, ACCEPT_STATUS, ACCEPT_REQUEST),
        (REJECT_QUOTE, REJECT_STATUS, REJECT_REQUEST),
    ] {
        sqlx::query(
            "INSERT INTO quotes (
                 id, account_id, activity_uri, approval_uri, created_at, legacy,
                 quoted_account_id, quoted_status_id, state, status_id, updated_at)
             VALUES ($1, $2, $3, NULL, clock_timestamp(), false, $4, $5, 0, $6,
                     clock_timestamp())",
        )
        .bind(quote_id)
        .bind(ALICE)
        .bind(request)
        .bind(BOB)
        .bind(REMOTE_TARGET)
        .bind(status_id)
        .execute(&owner)
        .await?;
    }
    sqlx::query(
        "INSERT INTO quotes (
             id, account_id, activity_uri, approval_uri, created_at, legacy,
             quoted_account_id, quoted_status_id, state, status_id, updated_at)
         VALUES ($1, $2, NULL, $3, clock_timestamp(), true, $4, $5, 1, $6,
                 clock_timestamp())",
    )
    .bind(LEGACY_QUOTE)
    .bind(ALICE)
    .bind(LEGACY_APPROVAL)
    .bind(BOB)
    .bind(REMOTE_TARGET)
    .bind(LEGACY_STATUS)
    .execute(&owner)
    .await?;
    sqlx::query(
        "INSERT INTO statuses (
             id, account_id, created_at, local, uri, url, quote_approval_policy, reply,
             sensitive, spoiler_text, text, updated_at, visibility)
         VALUES ($1, $2, clock_timestamp(), false, $3, $3, 0, false, false, '',
                 'Remote worker quote of a local status', clock_timestamp(), 3)",
    )
    .bind(REMOTE_QUOTING_STATUS)
    .bind(BOB)
    .bind(REMOTE_QUOTING_URI)
    .execute(&owner)
    .await?;
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at, quotes_count)
         VALUES ($1, clock_timestamp(), clock_timestamp(), 1)",
    )
    .bind(ACCEPT_STATUS)
    .execute(&owner)
    .await?;
    sqlx::query(
        "INSERT INTO quotes (
             id, account_id, activity_uri, approval_uri, created_at, legacy,
             quoted_account_id, quoted_status_id, state, status_id, updated_at)
         VALUES ($1, $2, $3, $4, clock_timestamp(), false, $5, $6, 1, $7,
                 clock_timestamp())",
    )
    .bind(REMOTE_QUOTE)
    .bind(BOB)
    .bind(REMOTE_QUOTE_REQUEST)
    .bind("https://fixture-v4-6-5.rustodon.invalid/users/alice/quote_authorizations/worker")
    .bind(ALICE)
    .bind(ACCEPT_STATUS)
    .bind(REMOTE_QUOTING_STATUS)
    .execute(&owner)
    .await?;
    sqlx::query(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload)
         VALUES ('rustodon.mastodon.notify_activity', $1,
                 jsonb_build_object('arguments', jsonb_build_object(
                   'recipient_account_id', $2::bigint, 'activity_type', 'quote',
                   'activity_id', $3::bigint))),
                ($4, $5, jsonb_build_object('arguments', jsonb_build_object(
                   'status_id', $6::bigint)))",
    )
    .bind(format!("notification:quote:{ALICE}:{REMOTE_QUOTE}"))
    .bind(ALICE)
    .bind(REMOTE_QUOTE)
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(format!("activitypub:quote-request:{REMOTE_QUOTE}"))
    .bind(REMOTE_QUOTING_STATUS)
    .execute(&owner)
    .await?;
    sqlx::query(
        "INSERT INTO statuses (
             id, account_id, created_at, local, quote_approval_policy, reblog_of_id, reply,
             sensitive, spoiler_text, text, updated_at, visibility)
         VALUES ($1, $2, clock_timestamp(), true, 0, $3, false, false, '', '',
                 clock_timestamp(), 0)",
    )
    .bind(FORWARD_REBLOG)
    .bind(ALICE)
    .bind(ACCEPT_STATUS)
    .execute(&owner)
    .await?;
    sqlx::query(
        "INSERT INTO follows (
             id, account_id, created_at, languages, notify, show_reblogs,
             target_account_id, updated_at, uri)
         VALUES ($1, $2, clock_timestamp(), NULL, false, true, $3,
                 clock_timestamp(), $4)",
    )
    .bind(FORWARD_FOLLOW)
    .bind(FORWARD_FOLLOWER)
    .bind(ALICE)
    .bind("https://account-blocked.fixture.invalid/users/domain_viewer#quote-forwarding")
    .execute(&owner)
    .await?;

    let scalar_target_uri =
        format!("https://fixture-v4-6-5.rustodon.invalid/@alice/{INBOUND_ALLOW_TARGET}");
    let scalar_document = json!({
        "id": INBOUND_SCALAR_URI,
        "type": "Note",
        "attributedTo": "https://remote.fixture.invalid/users/bob",
        "content": "A scalar QuoteRequest instrument",
        "quoteUrl": scalar_target_uri,
        "to": [activitypub::PUBLIC_ADDRESS],
        "cc": []
    });
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let scalar_endpoint = listener.local_addr()?;
    let scalar_server = tokio::spawn(fixture_retry_activitypub_server(
        listener,
        scalar_document.to_string().into_bytes(),
    ));
    let alice_id_scheme: Option<i32> =
        sqlx::query_scalar("SELECT id_scheme FROM accounts WHERE id = $1")
            .bind(ALICE)
            .fetch_one(&owner)
            .await?;
    let expected_alice_key_id = if alice_id_scheme == Some(1) {
        format!("https://fixture-v4-6-5.rustodon.invalid/ap/users/{ALICE}#main-key")
    } else {
        "https://fixture-v4-6-5.rustodon.invalid/users/alice#main-key".to_owned()
    };

    let queue = Queue::new(runtime);
    queue
        .enqueue(
            &JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                json!({
                    "status_id": REJECT_STATUS,
                    "activity_type": "QuoteRequest",
                    "quote_id": REJECT_QUOTE,
                    "quote_request_uri": REJECT_REQUEST,
                    "quoting_status_id": REJECT_STATUS,
                    "quoted_status_id": REMOTE_TARGET,
                    "quoted_status_uri": target_ap_id,
                    "quoted_status_url": target_web_link,
                    "quoted_account_id": BOB,
                }),
            )
            .logical_key(format!("activitypub:quote-request:{REJECT_QUOTE}")),
        )
        .await?;
    let delivery_listener = TcpListener::bind("127.0.0.1:0").await?;
    let delivery_endpoint = delivery_listener.local_addr()?;
    let mut config = federation_config();
    config.remote_fetch_endpoint = Some(scalar_endpoint);
    config.remote_delivery_endpoint = Some(delivery_endpoint);
    let write_repository = WriteRepository::from_pool(writer.clone());
    let mut auth_headers = HeaderMap::new();
    auth_headers.insert(
        reqwest::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let alice = BearerAuthenticator::new(Repository::from_pool(owner.clone()))
        .authenticate(&auth_headers, WRITE_STATUSES)
        .await?;
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer),
        None,
        Some(config),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    // Delivery only reaches ActivityPub accounts; the fixture's Bob is protocol 0.
    sqlx::query(
        "UPDATE accounts SET inbox_url = $2, shared_inbox_url = $2, protocol = 1 WHERE id = $1",
    )
    .bind(BOB)
    .bind(delivery_inbox)
    .execute(&owner)
    .await?;
    let delivery_log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let delivery_server = tokio::spawn(quote_delivery_server(
        delivery_listener,
        3,
        delivery_log.clone(),
    ));

    let operation = async {
        if !executor
            .process_one(
                "quote-lifecycle-distribution",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
        {
            return Err("outbound QuoteRequest distribution was not processed".into());
        }
        queue_quote_delivery(&queue, &owner, REJECT_REQUEST, "request").await?;
        if !executor
            .process_one(
                "quote-lifecycle-delivery",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
        {
            return Err("outbound QuoteRequest delivery was not processed".into());
        }

        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Push,
                    ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                    json!({
                        "status_id": ACCEPT_STATUS,
                        "activity_type": "QuoteRequest",
                        "quote_id": ACCEPT_QUOTE,
                        "quote_request_uri": ACCEPT_REQUEST,
                        "quoting_status_id": ACCEPT_STATUS,
                        "quoted_status_id": REMOTE_TARGET,
                        "quoted_status_uri": target_ap_id,
                        "quoted_status_url": target_web_link,
                        "quoted_account_id": BOB,
                    }),
                )
                .logical_key(format!("activitypub:quote-request:{ACCEPT_QUOTE}")),
            )
            .await?;
        if !executor
            .process_one(
                "quote-lifecycle-stale-distribution",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
        {
            return Err("stale QuoteRequest distribution was not processed".into());
        }
        let (_, blocked_request_arguments) =
            queue_quote_delivery(&queue, &owner, ACCEPT_REQUEST, "request").await?;
        let blocked_request = queue
            .claim(
                "quote-lifecycle-blocked-request",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
            .ok_or("outbound QuoteRequest was not claimable")?;
        sqlx::query(
            "INSERT INTO blocks (account_id, created_at, target_account_id, updated_at, uri) \
             VALUES ($1, clock_timestamp(), $2, clock_timestamp(), $3) \
             ON CONFLICT (account_id, target_account_id) DO NOTHING",
        )
        .bind(ALICE)
        .bind(BOB)
        .bind("https://fixture-v4-6-5.rustodon.invalid/blocks/quote-delivery-fence")
        .execute(&owner)
        .await?;
        expire_quote_delivery_lease(&queue, &blocked_request).await?;
        if !executor
            .process_one(
                "quote-lifecycle-blocked-request-retry",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
        {
            return Err("blocked QuoteRequest retry was not processed".into());
        }
        sqlx::query("DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2 AND uri = $3")
            .bind(ALICE)
            .bind(BOB)
            .bind("https://fixture-v4-6-5.rustodon.invalid/blocks/quote-delivery-fence")
            .execute(&owner)
            .await?;
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Push,
                    ACTIVITYPUB_DELIVERY_JOB_KIND,
                    blocked_request_arguments,
                )
                .logical_key("quote-lifecycle:terminal-request-retry"),
            )
            .await?;
        let stale_request = queue
            .claim(
                "quote-lifecycle-stale-request",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
            .ok_or("terminal outbound QuoteRequest was not claimable")?;
        process_activity(
            &queue,
            &executor,
            json!({
                "type": "Accept",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": ACCEPT_REQUEST,
                "result": APPROVAL,
            }),
            "accept",
            false,
        )
        .await?;
        expire_quote_delivery_lease(&queue, &stale_request).await?;
        if !executor
            .process_one(
                "quote-lifecycle-stale-request-retry",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
        {
            return Err("stale QuoteRequest retry was not processed".into());
        }

        let allowed_target_uri = format!(
            "https://fixture-v4-6-5.rustodon.invalid/@alice/{INBOUND_ALLOW_TARGET}"
        );
        let denied_target_uri = format!(
            "https://fixture-v4-6-5.rustodon.invalid/@alice/{INBOUND_DENY_TARGET}"
        );
        let allowed_request = json!({
            "id": INBOUND_ALLOWED_REQUEST,
            "type": "QuoteRequest",
            "actor": "https://remote.fixture.invalid/users/bob",
            "object": allowed_target_uri,
            "instrument": {
                "id": INBOUND_ALLOWED_URI,
                "type": "Note",
                "attributedTo": "https://remote.fixture.invalid/users/bob",
                "content": "An allowed embedded quote",
                "quote": allowed_target_uri,
                "to": [activitypub::PUBLIC_ADDRESS],
                "cc": []
            }
        });
        process_activity(
            &queue,
            &executor,
            allowed_request.clone(),
            "inbound-allowed",
            false,
        )
        .await?;
        let allowed: (i64, i32, i64, i64) = sqlx::query_as(
            "SELECT status.id, quote.state, quote.quoted_status_id, stats.quotes_count \
               FROM quotes quote \
               JOIN statuses status ON status.id = quote.status_id \
               JOIN status_stats stats ON stats.status_id = quote.quoted_status_id \
              WHERE status.uri = $1",
        )
        .bind(INBOUND_ALLOWED_URI)
        .fetch_one(&owner)
        .await?;
        if allowed.1 != 1 || allowed.2 != INBOUND_ALLOW_TARGET || allowed.3 != 1 {
            return Err(format!("allowed inbound QuoteRequest differs: {allowed:?}").into());
        }
        let allowed_decision: (String, String, String) = sqlx::query_as(
            "SELECT payload #>> '{arguments,quote_delivery_kind}', \
                    payload #>> '{arguments,quote_request_uri}', \
                    payload #>> '{arguments,body,type}' \
               FROM rustodon.outbox_events \
              WHERE kind = $1 AND logical_key = $2",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(activitypub::quote_request_decision_logical_key(
            INBOUND_ALLOWED_REQUEST,
        ))
        .fetch_one(&owner)
        .await?;
        if allowed_decision
            != (
                "accept".to_owned(),
                INBOUND_ALLOWED_REQUEST.to_owned(),
                "Accept".to_owned(),
            )
        {
            return Err(format!("allowed QuoteRequest decision differs: {allowed_decision:?}").into());
        }
        process_activity(
            &queue,
            &executor,
            allowed_request,
            "inbound-allowed-replay",
            false,
        )
        .await?;
        let allowed_replay_counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT \
               (SELECT count(*) FROM statuses WHERE uri = $1), \
               (SELECT count(*) FROM quotes quote JOIN statuses status ON status.id = quote.status_id \
                 WHERE status.uri = $1), \
               (SELECT quotes_count FROM status_stats WHERE status_id = $2)",
        )
        .bind(INBOUND_ALLOWED_URI)
        .bind(INBOUND_ALLOW_TARGET)
        .fetch_one(&owner)
        .await?;
        if allowed_replay_counts != (1, 1, 1) {
            return Err(format!(
                "allowed QuoteRequest replay duplicated effects: {allowed_replay_counts:?}"
            )
            .into());
        }
        queue_quote_delivery(&queue, &owner, INBOUND_ALLOWED_REQUEST, "accept").await?;
        let stale_accept = queue
            .claim(
                "quote-lifecycle-stale-accept",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
            .ok_or("outbound Accept was not claimable")?;
        write_repository
            .revoke_quote(
                &alice,
                INBOUND_ALLOW_TARGET,
                allowed.0,
                "https://fixture-v4-6-5.rustodon.invalid/",
            )
            .await?;
        expire_quote_delivery_lease(&queue, &stale_accept).await?;
        if !executor
            .process_one(
                "quote-lifecycle-stale-accept-retry",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
        {
            return Err("stale Accept retry was not processed".into());
        }

        process_activity(
            &queue,
            &executor,
            json!({
                "id": "https://remote.fixture.invalid/activities/worker-inbound-denied-create",
                "type": "Create",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": {
                    "id": INBOUND_DENIED_URI,
                    "type": "Note",
                    "attributedTo": "https://remote.fixture.invalid/users/bob",
                    "content": "A quote Note received before its denied request",
                    "quote": denied_target_uri,
                    "to": [activitypub::PUBLIC_ADDRESS],
                    "cc": []
                }
            }),
            "inbound-denied-note-first",
            false,
        )
        .await?;
        process_activity(
            &queue,
            &executor,
            json!({
                "id": INBOUND_DENIED_REQUEST,
                "type": "QuoteRequest",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": denied_target_uri,
                "instrument": {
                    "id": INBOUND_DENIED_URI,
                    "type": "Note",
                    "attributedTo": "https://remote.fixture.invalid/users/bob",
                    "content": "A denied embedded quote",
                    "quote": denied_target_uri,
                    "to": [activitypub::PUBLIC_ADDRESS],
                    "cc": []
                }
            }),
            "inbound-denied",
            false,
        )
        .await?;
        let denied_state: (i64, i32, i64) = sqlx::query_as(
            "SELECT count(*) OVER (), quote.state, stats.quotes_count \
               FROM statuses status \
               JOIN quotes quote ON quote.status_id = status.id \
               JOIN status_stats stats ON stats.status_id = quote.quoted_status_id \
              WHERE status.uri = $1",
        )
        .bind(INBOUND_DENIED_URI)
        .fetch_one(&owner)
        .await?;
        let denied_decision: String = sqlx::query_scalar(
            "SELECT payload #>> '{arguments,body,type}' FROM rustodon.outbox_events \
              WHERE kind = $1 AND logical_key = $2",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(activitypub::quote_request_decision_logical_key(
            INBOUND_DENIED_REQUEST,
        ))
        .fetch_one(&owner)
        .await?;
        if denied_state != (1, 2, 0) || denied_decision != "Reject" {
            return Err(format!(
                "Note-before-QuoteRequest denial differs: state={denied_state:?}, decision={denied_decision}"
            )
            .into());
        }
        queue_quote_delivery(&queue, &owner, INBOUND_DENIED_REQUEST, "reject").await?;
        if !executor
            .process_one(
                "quote-lifecycle-reject-delivery",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
        {
            return Err("outbound Reject was not processed".into());
        }
        process_activity(
            &queue,
            &executor,
            json!({
                "id": "https://remote.fixture.invalid/activities/worker-inbound-denied-update",
                "type": "Update",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": {
                    "id": INBOUND_DENIED_URI,
                    "type": "Note",
                    "attributedTo": "https://remote.fixture.invalid/users/bob",
                    "content": "The quote target was removed",
                    "quote": {"type": "Tombstone"},
                    "to": [activitypub::PUBLIC_ADDRESS],
                    "cc": []
                }
            }),
            "inbound-idless-tombstone",
            false,
        )
        .await?;
        let tombstoned_quote: (i32, Option<i64>, Option<i64>, Option<String>, i64) =
            sqlx::query_as(
                "SELECT quote.state, quote.quoted_status_id, quote.quoted_account_id, \
                        quote.approval_uri, stats.quotes_count \
                   FROM quotes quote \
                   JOIN statuses status ON status.id = quote.status_id \
                   JOIN status_stats stats ON stats.status_id = $2 \
                  WHERE status.uri = $1",
            )
            .bind(INBOUND_DENIED_URI)
            .bind(INBOUND_DENY_TARGET)
            .fetch_one(&owner)
            .await?;
        if tombstoned_quote != (4, None, None, None, 0) {
            return Err(format!("ID-less Tombstone reconciliation differs: {tombstoned_quote:?}").into());
        }

        process_activity(
            &queue,
            &executor,
            json!({
                "id": INBOUND_SCALAR_REQUEST,
                "type": "QuoteRequest",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": scalar_target_uri,
                "instrument": INBOUND_SCALAR_URI
            }),
            "inbound-scalar",
            true,
        )
        .await?;
        let scalar_retry: (i32, String) = sqlx::query_as(
            "SELECT attempts, arguments ->> 'body' FROM rustodon.durable_jobs \
              WHERE logical_key = 'quote-lifecycle:inbound-scalar'",
        )
        .fetch_one(queue.pool())
        .await?;
        if scalar_retry.0 != 1 || !scalar_retry.1.contains(INBOUND_SCALAR_REQUEST) {
            return Err(format!("scalar QuoteRequest retry changed its job: {scalar_retry:?}").into());
        }
        let scalar_before_retry: i64 =
            sqlx::query_scalar("SELECT count(*) FROM statuses WHERE uri = $1")
                .bind(INBOUND_SCALAR_URI)
                .fetch_one(&owner)
                .await?;
        if scalar_before_retry != 0 {
            return Err("transient scalar fetch imported a partial instrument".into());
        }
        sqlx::query(
            "UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() \
              WHERE logical_key = 'quote-lifecycle:inbound-scalar'",
        )
        .execute(queue.pool())
        .await?;
        if !executor
            .process_one(
                "quote-lifecycle-worker",
                &[Lane::Ingress],
                Duration::seconds(30),
            )
            .await?
        {
            return Err("scalar QuoteRequest retry was not processed".into());
        }
        let scalar_result: (i32, i64) = sqlx::query_as(
            "SELECT quote.state, stats.quotes_count FROM quotes quote \
               JOIN statuses status ON status.id = quote.status_id \
               JOIN status_stats stats ON stats.status_id = quote.quoted_status_id \
              WHERE status.uri = $1",
        )
        .bind(INBOUND_SCALAR_URI)
        .fetch_one(&owner)
        .await?;
        // Mastodon 4.6.5: the allowed quote was revoked above (count back to 0), and a
        // quoteUrl-only instrument is a legacy quote, whose acceptance is never counted
        // (Quote#update_counter_caches! returns on legacy?).
        if scalar_result != (1, 0) {
            let target_quotes: Vec<Value> = sqlx::query_scalar(
                "SELECT jsonb_build_object('id', quote.id, 'state', quote.state, 'legacy', quote.legacy, \
                        'status_uri', status.uri) \
                   FROM quotes quote JOIN statuses status ON status.id = quote.status_id \
                  WHERE quote.quoted_status_id = $1 ORDER BY quote.id",
            )
            .bind(INBOUND_ALLOW_TARGET)
            .fetch_all(&owner)
            .await?;
            return Err(format!(
                "scalar QuoteRequest result differs: {scalar_result:?}; target quotes {target_quotes:?}"
            )
            .into());
        }
        queue_quote_delivery(&queue, &owner, INBOUND_SCALAR_REQUEST, "accept").await?;
        if !executor
            .process_one(
                "quote-lifecycle-accept-delivery",
                &[Lane::Push],
                Duration::seconds(30),
            )
            .await?
        {
            return Err("outbound Accept was not processed".into());
        }
        let scalar_requests = scalar_server.await??;
        if scalar_requests.len() != 2
            || scalar_requests.iter().any(|request| {
                let request = String::from_utf8_lossy(request);
                let request_lower = request.to_ascii_lowercase();
                !request.starts_with("GET /statuses/worker-inbound-scalar ")
                    || !request_lower.contains("signature:")
                    || !request.contains(&expected_alice_key_id)
            })
        {
            return Err("scalar QuoteRequest fetch was not retried with Alice's signature".into());
        }
        let Ok(delivery_requests) =
            tokio::time::timeout(std::time::Duration::from_secs(5), delivery_server).await
        else {
            let jobs: Vec<Value> = sqlx::query_scalar(
                "SELECT jsonb_build_object('kind', kind, 'attempts', attempts, 'dead', \
                        dead_at IS NOT NULL, 'error', last_error, \
                        'type', arguments #>> '{body,type}', 'inbox', arguments ->> 'inbox_url') \
                   FROM rustodon.durable_jobs WHERE lane = 'push'",
            )
            .fetch_all(&owner)
            .await?;
            return Err(format!(
                "timed out waiting for quote deliveries; received {:?}; push jobs {jobs:?}",
                delivery_log.lock().expect("delivery log")
            )
            .into());
        };
        let delivery_requests = delivery_requests??;
        let delivered_bodies = delivery_requests
            .iter()
            .map(|request| {
                let text = String::from_utf8_lossy(request);
                let lower = text.to_ascii_lowercase();
                if !text.starts_with("POST /inbox HTTP/1.1\r\n")
                    || !lower.contains("content-type: application/activity+json")
                    || !lower.contains("digest: sha-256=")
                    || !lower.contains("signature:")
                {
                    return Err("quote delivery was not a signed ActivityPub POST");
                }
                let body = text
                    .split_once("\r\n\r\n")
                    .map(|(_, body)| body)
                    .ok_or("quote delivery had no HTTP body")?;
                serde_json::from_str::<Value>(body)
                    .map_err(|_| "quote delivery body was not JSON")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let delivered_types = delivered_bodies
            .iter()
            .map(|body| body.get("type").and_then(Value::as_str))
            .collect::<Vec<_>>();
        if delivered_types != [Some("QuoteRequest"), Some("Reject"), Some("Accept")]
            || delivered_bodies[0].get("id").and_then(Value::as_str) != Some(REJECT_REQUEST)
            || delivered_bodies[1]["object"]["id"] != INBOUND_DENIED_REQUEST
            || delivered_bodies[2]["object"]["id"] != INBOUND_SCALAR_REQUEST
        {
            return Err(format!("quote delivery bodies differ: {delivered_bodies:?}").into());
        }
        let queued_quote_deliveries: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1 \
               AND arguments ->> 'quote_delivery_kind' IS NOT NULL",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .fetch_one(queue.pool())
        .await?;
        if queued_quote_deliveries != 0 {
            return Err("terminal quote transitions retained a delivery retry".into());
        }

        for (suffix, status_uri, target_uri) in [
            (
                "inbound-cached-tombstone",
                INBOUND_ALLOWED_URI,
                allowed_target_uri.as_str(),
            ),
            (
                "inbound-uncached-tombstone",
                INBOUND_SCALAR_URI,
                "http://remote.fixture.invalid/statuses/deleted-uncached-target",
            ),
        ] {
            process_activity(
                &queue,
                &executor,
                json!({
                    "id": format!("https://remote.fixture.invalid/activities/{suffix}"),
                    "type": "Update",
                    "actor": "https://remote.fixture.invalid/users/bob",
                    "object": {
                        "id": status_uri,
                        "type": "Note",
                        "attributedTo": "https://remote.fixture.invalid/users/bob",
                        "content": "The quote target was tombstoned",
                        "quote": {"id": target_uri, "type": "Tombstone"},
                        "to": [activitypub::PUBLIC_ADDRESS],
                        "cc": []
                    }
                }),
                suffix,
                false,
            )
            .await?;
            let removed: (i32, Option<i64>, Option<i64>, Option<String>) = sqlx::query_as(
                "SELECT quote.state, quote.quoted_status_id, quote.quoted_account_id, \
                        quote.approval_uri \
                   FROM quotes quote JOIN statuses status ON status.id = quote.status_id \
                  WHERE status.uri = $1",
            )
            .bind(status_uri)
            .fetch_one(&owner)
            .await?;
            if removed != (4, None, None, None) {
                return Err(format!("{suffix} retained stale quote disclosure: {removed:?}").into());
            }
        }
        let accepted_target_count: i64 =
            sqlx::query_scalar("SELECT quotes_count FROM status_stats WHERE status_id = $1")
                .bind(INBOUND_ALLOW_TARGET)
                .fetch_one(&owner)
                .await?;
        if accepted_target_count != 0 {
            return Err("cached/uncached Tombstone removals did not restore the target counter".into());
        }

        let accepted: (i32, Option<String>) =
            sqlx::query_as("SELECT state, approval_uri FROM quotes WHERE id = $1")
                .bind(ACCEPT_QUOTE)
                .fetch_one(&owner)
                .await?;
        if accepted != (1, Some(APPROVAL.to_owned())) {
            return Err(format!("remote quote approval was not persisted: {accepted:?}").into());
        }
        let count: i64 =
            sqlx::query_scalar("SELECT quotes_count FROM status_stats WHERE status_id = $1")
                .bind(REMOTE_TARGET)
                .fetch_one(&owner)
                .await?;
        if count != baseline + 1 {
            return Err("accepted quote did not increment the exact target counter".into());
        }
        let effects = quote_update_effect_counts(&owner, ACCEPT_STATUS).await?;
        if effects != (1, 1) {
            let events: Vec<Value> = sqlx::query_scalar(
                "SELECT jsonb_build_object('key', logical_key, 'account', payload ->> 'account_id') \
                   FROM rustodon.outbox_events WHERE kind = 'rustodon.mastodon.stream_event' \
                    AND (payload ->> 'object_id')::bigint = $1 AND payload ->> 'event' = 'status.update'",
            )
            .bind(ACCEPT_STATUS)
            .fetch_all(&owner)
            .await?;
            return Err(format!(
                "quote acceptance did not record exactly one stream and distribution update: \
                 {effects:?}; stream events {events:?}"
            )
            .into());
        }
        process_activity(
            &queue,
            &executor,
            json!({
                "type": "Accept",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": ACCEPT_REQUEST,
                "result": "https://remote.fixture.invalid/activities/conflicting-approval",
            }),
            "conflicting-approval",
            true,
        )
        .await?;
        let stable: (i32, Option<String>) =
            sqlx::query_as("SELECT state, approval_uri FROM quotes WHERE id = $1")
                .bind(ACCEPT_QUOTE)
                .fetch_one(&owner)
                .await?;
        if stable != (1, Some(APPROVAL.to_owned())) {
            return Err("conflicting approval replay mutated the accepted quote".into());
        }
        if quote_update_effect_counts(&owner, ACCEPT_STATUS).await? != (1, 1) {
            return Err("conflicting approval replay duplicated quote update effects".into());
        }

        process_activity(
            &queue,
            &executor,
            json!({
                "id": "https://remote.fixture.invalid/activities/worker-stale-approval-delete",
                "type": "Delete",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": {"id": STALE_APPROVAL, "type": "QuoteAuthorization"}
            }),
            "delete-before-approval",
            false,
        )
        .await?;
        let marker: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM tombstones WHERE account_id = $1 AND uri = $2)",
        )
        .bind(BOB)
        .bind(STALE_APPROVAL)
        .fetch_one(&owner)
        .await?;
        if !marker {
            return Err("Delete-before-attachment did not persist its tombstone".into());
        }
        process_activity(
            &queue,
            &executor,
            json!({
                "type": "Accept",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": REJECT_REQUEST,
                "result": STALE_APPROVAL,
            }),
            "stale-approval-after-delete",
            false,
        )
        .await?;
        let stale_state: (i32, Option<String>) =
            sqlx::query_as("SELECT state, approval_uri FROM quotes WHERE id = $1")
                .bind(REJECT_QUOTE)
                .fetch_one(&owner)
                .await?;
        if stale_state != (2, None) {
            return Err(format!("stale approval did not terminally reject the quote: {stale_state:?}").into());
        }

        process_activity(
            &queue,
            &executor,
            json!({
                "type": "Reject",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": REJECT_REQUEST,
            }),
            "reject",
            false,
        )
        .await?;
        let rejected: i32 = sqlx::query_scalar("SELECT state FROM quotes WHERE id = $1")
            .bind(REJECT_QUOTE)
            .fetch_one(&owner)
            .await?;
        if rejected != 2 {
            return Err(format!("remote quote denial was not persisted: state={rejected}").into());
        }
        let denied_count: i64 =
            sqlx::query_scalar("SELECT quotes_count FROM status_stats WHERE status_id = $1")
                .bind(REMOTE_TARGET)
                .fetch_one(&owner)
                .await?;
        if denied_count != baseline + 1
            || quote_update_effect_counts(&owner, REJECT_STATUS).await? != (1, 1)
        {
            return Err("quote denial changed counters or did not record exact update effects".into());
        }

        process_activity(
            &queue,
            &executor,
            json!({
                "id": "https://remote.fixture.invalid/activities/worker-quote-revoke",
                "type": "Delete",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": {"id": APPROVAL, "type": "QuoteAuthorization"},
                "signature": {"type": "RsaSignature2017", "signatureValue": "fixture"},
            }),
            "revoke",
            false,
        )
        .await?;
        let revoked: (i32, Option<String>) =
            sqlx::query_as("SELECT state, approval_uri FROM quotes WHERE id = $1")
                .bind(ACCEPT_QUOTE)
                .fetch_one(&owner)
                .await?;
        if revoked != (3, None) {
            return Err(
                format!("quote authorization deletion was not persisted: {revoked:?}").into(),
            );
        }
        let count: i64 =
            sqlx::query_scalar("SELECT quotes_count FROM status_stats WHERE status_id = $1")
                .bind(REMOTE_TARGET)
                .fetch_one(&owner)
                .await?;
        if count != baseline {
            return Err("quote revocation did not restore the exact target counter".into());
        }
        if quote_update_effect_counts(&owner, ACCEPT_STATUS).await? != (2, 2)
            || quote_update_effect_counts(&owner, REJECT_STATUS).await? != (1, 1)
        {
            return Err("accept, reject, and revoke update effects were not transition-specific".into());
        }
        process_activity(
            &queue,
            &executor,
            json!({
                "type": "Delete",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": LEGACY_APPROVAL,
            }),
            "legacy-revoke",
            false,
        )
        .await?;
        let legacy_state: i32 = sqlx::query_scalar("SELECT state FROM quotes WHERE id = $1")
            .bind(LEGACY_QUOTE)
            .fetch_one(&owner)
            .await?;
        let legacy_count: i64 =
            sqlx::query_scalar("SELECT quotes_count FROM status_stats WHERE status_id = $1")
                .bind(REMOTE_TARGET)
                .fetch_one(&owner)
                .await?;
        if legacy_state != 3 || legacy_count != baseline {
            return Err("legacy quote revocation changed its target counter".into());
        }
        let forwarded: Vec<(String, Value)> = sqlx::query_as(
            "SELECT payload #>> '{arguments,inbox_url}', payload #> '{arguments,body}'
               FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload #>> '{arguments,body,id}' =
                    'https://remote.fixture.invalid/activities/worker-quote-revoke'",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .fetch_all(&owner)
        .await?;
        if forwarded
            != vec![(
                "https://account-blocked.fixture.invalid/inbox".to_owned(),
                json!({
                    "id": "https://remote.fixture.invalid/activities/worker-quote-revoke",
                    "type": "Delete",
                    "actor": "https://remote.fixture.invalid/users/bob",
                    "object": {"id": APPROVAL, "type": "QuoteAuthorization"},
                    "signature": {"type": "RsaSignature2017", "signatureValue": "fixture"},
                }),
            )]
        {
            return Err(
                format!("signed quote revocation forwarding differs: {forwarded:?}").into(),
            );
        }

        // Deleting a remote quoting Note retires the accepted relationship and
        // all pending quote side effects without requiring DELETE on quotes.
        process_activity(
            &queue,
            &executor,
            json!({
                "type": "Delete",
                "actor": "https://remote.fixture.invalid/users/bob",
                "object": REMOTE_QUOTING_URI,
            }),
            "delete-remote-quoting-note",
            false,
        )
        .await?;
        let deleted_quote: (bool, bool, i64) = sqlx::query_as(
            "SELECT status.deleted_at IS NOT NULL,
                    EXISTS (SELECT 1 FROM quotes active
                             JOIN statuses quoting ON quoting.id = active.status_id
                            WHERE active.id = $1 AND quoting.deleted_at IS NULL),
                    (SELECT quotes_count FROM status_stats WHERE status_id = $2)
               FROM statuses status WHERE status.id = $3",
        )
        .bind(REMOTE_QUOTE)
        .bind(ACCEPT_STATUS)
        .bind(REMOTE_QUOTING_STATUS)
        .fetch_one(&owner)
        .await?;
        if deleted_quote != (true, false, 0) {
            return Err(format!("remote quoting Note cleanup differs: {deleted_quote:?}").into());
        }
        let stale_quote_jobs: i64 = sqlx::query_scalar(
            "SELECT
               (SELECT count(*) FROM rustodon.outbox_events
                 WHERE logical_key IN ($1, $2))
               +
               (SELECT count(*) FROM rustodon.durable_jobs
                 WHERE logical_key = $2)",
        )
        .bind(format!("notification:quote:{ALICE}:{REMOTE_QUOTE}"))
        .bind(format!("activitypub:quote-request:{REMOTE_QUOTE}"))
        .fetch_one(&owner)
        .await?;
        if stale_quote_jobs != 0 {
            return Err("remote quoting Note deletion retained pending quote jobs".into());
        }
        let authorization_delete: (String, String) = sqlx::query_as(
            // Mastodon embeds the QuoteAuthorization object in its Delete.
            "SELECT payload #>> '{arguments,body,type}',
                    payload #>> '{arguments,body,object,id}'
               FROM rustodon.outbox_events
              WHERE kind = $1 AND logical_key LIKE $2
              ORDER BY id LIMIT 1",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(format!(
            "activitypub:quote-authorization-delete:{REMOTE_QUOTE}:%"
        ))
        .fetch_one(&owner)
        .await?;
        if authorization_delete.0 != "Delete"
            || !authorization_delete
                .1
                .ends_with(&format!("/quote_authorizations/{REMOTE_QUOTE}"))
        {
            return Err(format!(
                "remote quoting Note deletion did not emit authorization Delete: {authorization_delete:?}"
            )
            .into());
        }

        // A mismatched actor must fail closed without changing the denied quote.
        process_activity(
            &queue,
            &executor,
            json!({
                "type": "Accept",
                "actor": "https://remote.fixture.invalid/users/carol",
                "object": REJECT_REQUEST,
                "result": "https://remote.fixture.invalid/activities/wrong-actor-approval",
            }),
            "wrong-actor",
            true,
        )
        .await?;
        let state: i32 = sqlx::query_scalar("SELECT state FROM quotes WHERE id = $1")
            .bind(REJECT_QUOTE)
            .fetch_one(&owner)
            .await?;
        if state != 2 {
            return Err("wrong-actor approval mutated denied quote state".into());
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    sqlx::query(
        "UPDATE accounts SET inbox_url = $2, shared_inbox_url = $3, protocol = $4 WHERE id = $1",
    )
    .bind(BOB)
    .bind(&bob_inboxes.0)
    .bind(&bob_inboxes.1)
    .bind(bob_inboxes.2)
    .execute(&owner)
    .await?;
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2 AND uri = $3")
        .bind(ALICE)
        .bind(BOB)
        .bind("https://fixture-v4-6-5.rustodon.invalid/blocks/quote-delivery-fence")
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM follows WHERE id = $1")
        .bind(FORWARD_FOLLOW)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(FORWARD_REBLOG)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM tombstones WHERE account_id = $1 AND uri = ANY($2)")
        .bind(BOB)
        .bind(vec![
            REMOTE_QUOTING_URI,
            STALE_APPROVAL,
            APPROVAL,
            LEGACY_APPROVAL,
        ])
        .execute(&owner)
        .await?;
    sqlx::query(
        "DELETE FROM quotes WHERE status_id IN (SELECT id FROM statuses WHERE uri = ANY($1))",
    )
    .bind(vec![
        INBOUND_ALLOWED_URI,
        INBOUND_DENIED_URI,
        INBOUND_SCALAR_URI,
    ])
    .execute(&owner)
    .await?;
    sqlx::query("DELETE FROM statuses WHERE uri = ANY($1)")
        .bind(vec![
            INBOUND_ALLOWED_URI,
            INBOUND_DENIED_URI,
            INBOUND_SCALAR_URI,
        ])
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM quotes WHERE id = ANY($1)")
        .bind(vec![ACCEPT_QUOTE, REJECT_QUOTE, LEGACY_QUOTE, REMOTE_QUOTE])
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
        .bind(vec![
            ACCEPT_STATUS,
            REJECT_STATUS,
            LEGACY_STATUS,
            REMOTE_QUOTING_STATUS,
            INBOUND_ALLOW_TARGET,
            INBOUND_DENY_TARGET,
        ])
        .execute(&owner)
        .await?;
    sqlx::query("UPDATE status_stats SET quotes_count = $2 WHERE status_id = $1")
        .bind(REMOTE_TARGET)
        .bind(baseline)
        .execute(&owner)
        .await?;
    reset().await?;
    operation
}
