use super::*;
use futures_util::FutureExt;
use rustodon::mastodon::rest::{RestProjectionLoader, TimelineOptions};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const ALICE: i64 = 116_844_606_259_201_001;
const OUTSIDER: i64 = 116_844_606_259_201_004;
const BOB: i64 = 116_844_606_259_202_001;
const PARENT_AUTHOR: i64 = -330;
const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
const PARENT_ACTOR: &str = "https://remote.fixture.invalid/users/timeline_author";
const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
const PUBLIC: &str = "https://www.w3.org/ns/activitystreams#Public";

// Pinned 4.6.5 Create#resolve_thread schedules unresolved replies independently of
// Create#distribute (home feeds and mention notifications). FetchRemoteStatusService
// retains fetched authorship. See the issue for exact read-only oracle provenance.
// The HTTP 503 -> durable retry policy is Rustodon's worker contract, not a claim
// that Mastodon's fetch service itself raises/retries every HTTP 503.
#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn public_reply_recovers_after_parent_fetch_503() -> TestResult {
    check_recovery("public", 0, 1).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn followers_reply_keeps_privacy_after_parent_fetch_503() -> TestResult {
    check_recovery("followers", 2, 0).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn direct_reply_keeps_privacy_after_parent_fetch_503() -> TestResult {
    check_recovery("direct", 3, 0).await
}

fn note(uri: &str, actor: &str, recipient: &str, visibility: i32) -> Value {
    let (to, cc) = match visibility {
        0 => (vec![PUBLIC.to_owned()], vec![recipient.to_owned()]),
        1 => (vec![recipient.to_owned()], vec![PUBLIC.to_owned()]),
        2 => (
            vec![format!("{actor}/followers")],
            vec![recipient.to_owned()],
        ),
        3 => (vec![recipient.to_owned()], vec![]),
        _ => unreachable!("only the selected recovery matrix audiences"),
    };
    json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": uri, "type": "Note", "attributedTo": actor,
        "published": Utc::now().to_rfc3339(), "content": "<p>Parent fetch recovery</p>",
        "to": to, "cc": cc, "tag": [{"type": "Mention", "href": recipient, "name": "@alice"}]
    })
}

#[allow(clippy::too_many_lines)]
async fn check_recovery(label: &str, child_visibility: i32, parent_visibility: i32) -> TestResult {
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    reset().await?;
    assert_eq!(
        sqlx::query_as::<_, (String, bool)>(
            "SELECT current_user::text, rolsuper FROM pg_roles WHERE rolname = current_user"
        )
        .fetch_one(&writer)
        .await?,
        ("rustodon_differential_writer".into(), false)
    );
    // Stats self-heal in earlier worker tests may create the parent author's row;
    // this scenario needs it missing, as in the restored fixture.
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(PARENT_AUTHOR)
        .execute(&owner)
        .await?;
    let baseline: Vec<(i64, i64, Option<NaiveDateTime>)> = sqlx::query_as(
        "SELECT account_id, statuses_count, last_status_at FROM account_stats
         WHERE account_id IN ($1, $2) ORDER BY account_id",
    )
    .bind(BOB)
    .bind(PARENT_AUTHOR)
    .fetch_all(&owner)
    .await?;
    assert_eq!(
        baseline.iter().map(|row| row.0).collect::<Vec<_>>(),
        vec![BOB],
        "the fixture has Bob's statistics but no parent-author statistics row"
    );
    // Relationship prerequisites are established inside the guarded fixture setup.
    // Combined worker runs may legitimately have removed a seed follow already.
    let repository = Repository::from_pool(runtime.clone());
    let alice = repository.account(ALICE).await?.ok_or("Alice missing")?;
    let recipient = activitypub::actor_url(&Url::parse(ORIGIN)?, &alice);
    // Plain HTTP is intentional: the test-only endpoint is a loopback HTTP fixture.
    let parent_uri = format!(
        "http://remote.fixture.invalid/users/timeline_author/statuses/parent-fetch-recovery-{label}"
    );
    let child_uri = format!("{ACTOR}/statuses/parent-fetch-recovery-{label}");
    let successor_uri = format!("{child_uri}/successor");
    let uris = vec![child_uri.clone(), parent_uri.clone(), successor_uri.clone()];
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE uri = ANY($1)")
            .bind(&uris)
            .fetch_one(&runtime)
            .await?,
        0,
        "use a clean disposable fixture"
    );
    let parent = note(&parent_uri, PARENT_ACTOR, &recipient, parent_visibility);
    let mut child = note(&child_uri, ACTOR, &recipient, child_visibility);
    child["inReplyTo"] = json!(parent_uri);
    let body = json!({
        "id": format!("{child_uri}/activity"), "type": "Create", "actor": ACTOR, "object": child
    });
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    // Reuse the existing two-response fixture, rather than duplicating an HTTP server.
    let mut server = tokio::spawn(fixture_retry_activitypub_server(
        listener,
        serde_json::to_vec(&parent)?,
    ));
    let queue = Queue::new(runtime.clone());
    let executor = WorkerExecutor::new(
        queue.clone(),
        infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(writer),
            None,
            Some(ActivityPubDeliveryConfig {
                origin: Url::parse(ORIGIN)?,
                local_domain: DOMAIN.into(),
                media_root_url: "/system".into(),
                media_root: None,
                limited_federation: false,
                remote_fetch_endpoint: Some(endpoint),
                remote_media_endpoint: None,
                remote_delivery_endpoint: None,
            }),
        )?,
        1,
        1,
    )?;
    let loader = RestProjectionLoader::new(repository.clone(), Some(ALICE), DOMAIN);
    let mut exclusive_memberships: Vec<Value> = Vec::new();
    let mut viewer_mutes: Vec<Value> = Vec::new();
    let mut created_follows: Vec<i64> = Vec::new();
    // Clean up even when an assertion fails in the parent's baseline/mutation runs.
    let result = std::panic::AssertUnwindSafe(async {
        // Preserve existing follows byte-for-byte; install only missing prerequisites.
        // Record inserted IDs so cleanup also restores an originally absent follow.
        created_follows = sqlx::query_scalar(
            "INSERT INTO follows (account_id, target_account_id, show_reblogs, notify,
                                  languages, uri, created_at, updated_at)
             SELECT $1, target, true, false, NULL, $3 || '/' || target::text,
                    clock_timestamp(), clock_timestamp()
             FROM unnest($2::bigint[]) AS targets(target)
             ON CONFLICT (account_id, target_account_id) DO NOTHING RETURNING id",
        ).bind(ALICE).bind(vec![BOB, PARENT_AUTHOR])
            .bind(format!("{ORIGIN}parent-fetch-recovery/{label}/follows"))
            .fetch_all(&owner).await?;
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id IN ($2, $3)",
        ).bind(ALICE).bind(BOB).bind(PARENT_AUTHOR).fetch_one(&runtime).await?, 2,
            "recovery requires Alice to follow both child and parent authors");
        // Otherwise Bob's exclusive list independently hides the child, masking the
        // unresolved -> resolved feed transition. Preserve complete membership rows.
        exclusive_memberships = sqlx::query_scalar(
            "DELETE FROM list_accounts membership USING lists list
             WHERE membership.list_id = list.id AND list.account_id = $1
               AND list.exclusive AND membership.account_id = $2
             RETURNING to_jsonb(membership)",
        ).bind(ALICE).bind(BOB).fetch_all(&owner).await?;
        // An earlier follow deletion can cascade-delete these memberships. What
        // matters is no exclusive suppression now, not whether a seed row survived.
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM list_accounts membership JOIN lists list ON list.id = membership.list_id
             WHERE list.account_id = $1 AND list.exclusive AND membership.account_id = $2",
        ).bind(ALICE).bind(BOB).fetch_one(&runtime).await?, 0,
            "the child author must not be excluded from home by an exclusive list");
        // Home eligibility also requires an unmuted author. The restored timed
        // mute is still a row; this test does not run mute-expiry maintenance.
        viewer_mutes = sqlx::query_scalar(
            "DELETE FROM mutes mute WHERE account_id = $1 AND target_account_id = $2
             RETURNING to_jsonb(mute)",
        ).bind(ALICE).bind(BOB).fetch_all(&owner).await?;
        assert_eq!(viewer_mutes.len(), 1, "fixture Alice-to-Bob mute setup");
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM mutes WHERE account_id = $1 AND target_account_id IN ($2, $3)",
        ).bind(ALICE).bind(BOB).bind(PARENT_AUTHOR).fetch_one(&runtime).await?, 0,
            "neither author may be muted in the recovery feed scenario");
        ingest_child(&queue, &executor, &body, "initial").await?;
        let (child_id, conversation): (i64, i64) = sqlx::query_as(
            "SELECT id, conversation_id FROM statuses WHERE uri = $1",
        ).bind(&child_uri).fetch_one(&runtime).await?;
        let successor_id: i64 = sqlx::query_scalar("SELECT id FROM statuses WHERE uri = $1")
            .bind(&successor_uri).fetch_one(&runtime).await?;
        let successor = status_row(&runtime, successor_id).await?;
        assert_eq!((successor.0, successor.3, successor.4, successor.5),
            (BOB, true, Some(child_id), Some(BOB)), "ordered successor is a resolved self reply");
        let initial = status_row(&runtime, child_id).await?;
        assert_eq!(initial, (BOB, child_visibility, false, true, None, None, conversation));
        let options = TimelineOptions { since_id: Some(child_id - 1), ..TimelineOptions::default() };
        // Only this scenario's statuses: other worker tests may leave newer fixture rows.
        assert_eq!(loader.home_timeline(ALICE, &options).await?.into_iter()
            .map(|status| status.id).filter(|id| [child_id, successor_id].contains(id))
            .collect::<Vec<_>>(), vec![successor_id],
            "unresolved reply is excluded, but its ordered resolved self reply proceeds");
        assert_public_route(&runtime, child_id, false).await?;
        assert_public_route(&runtime, successor_id, child_visibility == 0).await?;
        drain_notifications(&queue, &executor).await?;
        assert_notifications(&runtime, &[(child_id, BOB), (successor_id, BOB)]).await?;
        let (job_id, arguments): (i64, Value) = sqlx::query_as(
            "SELECT id, arguments FROM rustodon.durable_jobs WHERE kind = $1 AND logical_key = $2",
        ).bind(ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND)
            .bind(format!("activitypub:thread:{child_id}")).fetch_one(&runtime).await?;
        assert_eq!(arguments, json!({"child_status_id": child_id, "parent_url": parent_uri}));
        let before_failure = snapshot(&runtime, &uris).await?;
        assert!(executor.process_one("parent-fetch-failure", &[Lane::Pull], Duration::seconds(30)).await?);
        let retry: (i32, i32, Option<String>, bool, bool, Value) = sqlx::query_as(
            "SELECT attempts, max_attempts, last_error, dead_at IS NULL,
                    run_at > clock_timestamp(), arguments
             FROM rustodon.durable_jobs WHERE id = $1",
        ).bind(job_id).fetch_one(&runtime).await?;
        assert_eq!((retry.0, retry.1, retry.3, retry.4), (1, 4, true, true),
            "503 retains the same live durable job with backoff");
        // UnexpectedStatus deliberately omits the numeric status from Display;
        // the fixture supplies 503, while this message identifies its retry category.
        assert_eq!(retry.2.as_deref(), Some(
            "remote reply parent fetch is temporarily unavailable: remote HTTP response was unsuccessful"
        ), "must fail at retryable HTTP status, not an unrelated setup/grant error");
        assert_eq!(retry.5, arguments);
        assert_eq!(snapshot(&runtime, &uris).await?, before_failure,
            "failed fetch must not partially persist a parent, alter the child, or duplicate delivery intents");
        assert_eq!(status_row(&runtime, child_id).await?, initial);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE uri = $1")
            .bind(&parent_uri).fetch_one(&runtime).await?, 0);

        // Advance only this fixture job's clock; never enqueue a replacement for recovery.
        assert_eq!(sqlx::query("UPDATE rustodon.durable_jobs SET run_at = clock_timestamp() WHERE id = $1")
            .bind(job_id).execute(&runtime).await?.rows_affected(), 1);
        assert!(executor.process_one("parent-fetch-recovery", &[Lane::Pull], Duration::seconds(30)).await?);
        assert_completed(&runtime, job_id).await?;
        let requests = tokio::time::timeout(std::time::Duration::from_secs(5), &mut server).await???;
        assert_eq!(requests.len(), 2);
        for request in requests {
            let request = String::from_utf8(request)?;
            assert!(request.starts_with(&format!("GET {} HTTP/1.1\r\n", Url::parse(&parent_uri)?.path())));
            assert!(request.to_ascii_lowercase().contains("\r\nsignature:"), "signed parent GET");
        }
        let parent_id: i64 = sqlx::query_scalar("SELECT id FROM statuses WHERE uri = $1")
            .bind(&parent_uri).fetch_one(&runtime).await?;
        let parent_row = status_row(&runtime, parent_id).await?;
        assert_eq!((parent_row.0, parent_row.1, parent_row.2, parent_row.3, parent_row.4, parent_row.5),
            (PARENT_AUTHOR, parent_visibility, false, false, None, None));
        assert_eq!(status_row(&runtime, child_id).await?,
            (BOB, child_visibility, false, true, Some(parent_id), Some(PARENT_AUTHOR), conversation),
            "repair links the actual author without replacing child identity, privacy or conversation");
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT replies_count FROM status_stats WHERE status_id = $1")
            .bind(parent_id).fetch_one(&runtime).await?, i64::from(child_visibility < 2));
        drain_notifications(&queue, &executor).await?;
        // One legitimate mention per status, NOT duplicate notifications.
        assert_notifications(&runtime, &[(child_id, BOB), (parent_id, PARENT_AUTHOR), (successor_id, BOB)]).await?;
        let mut feed_ids = loader.home_timeline(ALICE, &options).await?.into_iter()
            .map(|status| status.id)
            .filter(|id| [child_id, parent_id, successor_id].contains(id))
            .collect::<Vec<_>>();
        feed_ids.sort_unstable();
        let mut expected_ids = vec![child_id, parent_id, successor_id];
        expected_ids.sort_unstable();
        assert_eq!(feed_ids, expected_ids, "recovered reply and parent are each distributed once in the home feed");
        let context = loader.status_context(child_id).await?.ok_or("child context missing")?;
        assert_eq!(context.ancestors.iter().map(|status| status.id).collect::<Vec<_>>(), vec![parent_id]);
        assert_eq!(context.descendants.iter().map(|status| status.id).collect::<Vec<_>>(), vec![successor_id]);
        assert_public_route(&runtime, parent_id, parent_visibility == 0).await?;
        for viewer in [None, Some(OUTSIDER)] {
            let outsider = RestProjectionLoader::new(repository.clone(), viewer, DOMAIN);
            assert_eq!(outsider.authorized_status(child_id).await?.is_some(), child_visibility < 2,
                "hydration must not broaden the reply audience: {viewer:?}");
        }
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1
             AND payload ->> 'event' = 'update' AND payload ->> 'object_id' = $2
             AND payload ->> 'account_id' = $3",
        ).bind(STREAM_EVENT_KIND).bind(parent_id.to_string()).bind(ALICE.to_string())
            .fetch_one(&runtime).await?, 1, "fetched parent has one home stream intent");
        if child_visibility >= 2 {
            assert_eq!(sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1
                 AND payload ->> 'event' = 'update' AND payload ->> 'object_id' = $2
                 AND (payload ->> 'account_id' IS NULL OR payload ->> 'account_id' <> $3)
                 -- The audience-independent hint (account 0) must not route anywhere.
                 AND NOT (payload ->> 'account_id' = '0'
                      AND NOT coalesce((payload -> 'after' ->> 'public')::boolean, false)
                      AND NOT coalesce((payload -> 'after' ->> 'hashtag')::boolean, false)
                      AND coalesce(jsonb_array_length(payload -> 'after' -> 'lists'), 0) = 0)",
            ).bind(STREAM_EVENT_KIND).bind(child_id.to_string()).bind(ALICE.to_string())
                .fetch_one(&runtime).await?, 0, "private reply never routes to a nonrecipient");
        }
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1",
        ).bind(ACTIVITYPUB_DELIVERY_JOB_KIND).fetch_one(&runtime).await?, 0,
            "fetched remote parent does not authorize outbound forwarding of the reply");
        let stable = snapshot(&runtime, &uris).await?;
        // Fresh queue keys exercise handlers, rather than merely enqueue deduplication.
        ingest_child(&queue, &executor, &body, "duplicate-create").await?;
        let duplicate = queue.enqueue(&JobSpec::new(Lane::Pull,
            ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND, arguments).logical_key("parent-fetch:duplicate-resolve")).await?;
        assert!(executor.process_one("parent-fetch-duplicate", &[Lane::Pull], Duration::seconds(30)).await?);
        assert_completed(&runtime, duplicate).await?;
        let notifications: Vec<(i64, Value)> = sqlx::query_as(
            "SELECT id, payload -> 'arguments' FROM rustodon.outbox_events WHERE kind = $1 ORDER BY id",
        ).bind(NOTIFICATION_CREATE_JOB_KIND).fetch_all(&runtime).await?;
        assert_eq!(notifications.len(), 3);
        for (id, arguments) in notifications {
            queue.enqueue(&JobSpec::new(Lane::Core, NOTIFICATION_CREATE_JOB_KIND, arguments)
                .logical_key(format!("parent-fetch:duplicate-notification:{id}"))).await?;
        }
        drain_notifications(&queue, &executor).await?;
        assert_notifications(&runtime, &[(child_id, BOB), (parent_id, PARENT_AUTHOR), (successor_id, BOB)]).await?;
        assert_eq!(snapshot(&runtime, &uris).await?, stable,
            "duplicate Create/resolution/notification work preserves rows, reply counts, and distribution intents");
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.durable_jobs WHERE lane IN ('ingress', 'pull', 'core')",
        ).fetch_one(&runtime).await?, 0, "no retries or dead letters remain");
        Ok::<(), Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
    server.abort();
    // The server handle may already have been joined successfully above.
    sqlx::query("DELETE FROM follows WHERE id = ANY($1)")
        .bind(&created_follows)
        .execute(&owner)
        .await?;
    for mute in viewer_mutes {
        sqlx::query("INSERT INTO mutes SELECT * FROM jsonb_populate_record(NULL::mutes, $1)")
            .bind(mute)
            .execute(&owner)
            .await?;
    }
    for membership in exclusive_memberships {
        sqlx::query(
            "INSERT INTO list_accounts SELECT * FROM jsonb_populate_record(NULL::list_accounts, $1)",
        ).bind(membership).execute(&owner).await?;
    }
    sqlx::query(
        "DELETE FROM notifications WHERE activity_type = 'Mention' AND activity_id IN
         (SELECT id FROM mentions WHERE status_id IN (SELECT id FROM statuses WHERE uri = ANY($1)))",
    ).bind(&uris).execute(&owner).await?;
    sqlx::query("DELETE FROM conversations WHERE parent_status_id IN (SELECT id FROM statuses WHERE uri = ANY($1))")
        .bind(&uris).execute(&owner).await?;
    sqlx::query("DELETE FROM statuses WHERE uri = ANY($1)")
        .bind(&uris)
        .execute(&owner)
        .await?;
    for (account_id, statuses, last_status) in baseline {
        sqlx::query("UPDATE account_stats SET statuses_count = $2, last_status_at = $3 WHERE account_id = $1")
            .bind(account_id).bind(statuses).bind(last_status).execute(&owner).await?;
    }
    reset().await?;
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

type StatusRow = (i64, i32, bool, bool, Option<i64>, Option<i64>, i64);

async fn status_row(pool: &sqlx::PgPool, id: i64) -> TestResult<StatusRow> {
    Ok(sqlx::query_as(
        "SELECT account_id, visibility, local, reply, in_reply_to_id, in_reply_to_account_id,
                conversation_id FROM statuses WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

async fn assert_completed(pool: &sqlx::PgPool, id: i64) -> TestResult {
    let remaining: Option<(i32, Option<String>)> =
        sqlx::query_as("SELECT attempts, last_error FROM rustodon.durable_jobs WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    assert!(
        remaining.is_none(),
        "job {id} must acknowledge: {remaining:?}"
    );
    Ok(())
}

async fn ingest_child(
    queue: &Queue,
    executor: &WorkerExecutor,
    body: &Value,
    key: &str,
) -> TestResult {
    let child_uri = body["object"]["id"].as_str().ok_or("child URI missing")?;
    let mut successor_body = body.clone();
    successor_body["id"] = json!(format!("{child_uri}/successor/activity"));
    successor_body["object"]["id"] = json!(format!("{child_uri}/successor"));
    successor_body["object"]["inReplyTo"] = json!(child_uri);
    let spec = |body: &Value, key: String| {
        JobSpec::new(
            Lane::Ingress,
            ACTIVITYPUB_INBOX_JOB_KIND,
            json!({"body": body.to_string(), "signature_key_id": format!("{ACTOR}#secondary-key"),
            "remote_domain": "remote.fixture.invalid", "delivery_target_account_id": ALICE}),
        )
        .logical_key(key)
    };
    // Queue both before processing: the unresolved reply must not strand the
    // next activity from this actor, even while its parent remains unavailable.
    let ordering_key = [33_u8; 32];
    assert!(
        queue
            .enqueue_ordered_once(
                &spec(body, format!("parent-fetch:{key}")),
                &ordering_key,
                &[1_u8; 32]
            )
            .await?
    );
    let successor = spec(&successor_body, format!("parent-fetch:{key}:successor"));
    assert!(
        queue
            .enqueue_ordered_once(&successor, &ordering_key, &[2_u8; 32])
            .await?
    );
    for logical_key in [
        format!("parent-fetch:{key}"),
        format!("parent-fetch:{key}:successor"),
    ] {
        let id: i64 =
            sqlx::query_scalar("SELECT id FROM rustodon.durable_jobs WHERE logical_key = $1")
                .bind(logical_key)
                .fetch_one(queue.pool())
                .await?;
        assert!(
            executor
                .process_one(
                    "parent-fetch-ingress",
                    &[Lane::Ingress],
                    Duration::seconds(30),
                )
                .await?
        );
        assert_completed(queue.pool(), id).await?;
    }
    Ok(())
}

async fn drain_notifications(queue: &Queue, executor: &WorkerExecutor) -> TestResult {
    while queue.dispatch_outbox(100).await? != 0 {}
    for _ in 0..20 {
        if !executor
            .process_one(
                "parent-fetch-notifications",
                &[Lane::Core],
                Duration::seconds(30),
            )
            .await?
        {
            break;
        }
    }
    let pending: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT kind, last_error FROM rustodon.durable_jobs WHERE lane = 'core'")
            .fetch_all(queue.pool())
            .await?;
    assert!(
        pending.is_empty(),
        "notification jobs must complete: {pending:?}"
    );
    Ok(())
}

async fn assert_notifications(pool: &sqlx::PgPool, expected: &[(i64, i64)]) -> TestResult {
    let ids = expected.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let mentions: Vec<(i64, i64, bool)> = sqlx::query_as(
        "SELECT status_id, account_id, silent FROM mentions WHERE status_id = ANY($1) ORDER BY status_id",
    ).bind(&ids).fetch_all(pool).await?;
    let mut expected_mentions = ids.iter().map(|id| (*id, ALICE, false)).collect::<Vec<_>>();
    expected_mentions.sort_unstable();
    assert_eq!(mentions, expected_mentions);
    let actual: Vec<(i64, i64, i64, String, bool)> = sqlx::query_as(
        "SELECT mention.status_id, notification.account_id, notification.from_account_id,
                notification.type, notification.filtered
         FROM notifications notification JOIN mentions mention ON mention.id = notification.activity_id
         WHERE notification.activity_type = 'Mention' AND mention.status_id = ANY($1)
         ORDER BY mention.status_id, notification.id",
    ).bind(&ids).fetch_all(pool).await?;
    let mut expected = expected
        .iter()
        .map(|(id, author)| (*id, ALICE, *author, "mention".into(), false))
        .collect::<Vec<_>>();
    expected.sort_unstable();
    assert_eq!(
        actual, expected,
        "one legitimate notification per status and recipient, not one per thread"
    );
    Ok(())
}

async fn snapshot(pool: &sqlx::PgPool, uris: &[String]) -> TestResult<Value> {
    Ok(sqlx::query_scalar(
        "WITH selected AS (SELECT * FROM statuses WHERE uri = ANY($1))
         SELECT jsonb_build_object(
           'statuses', (SELECT jsonb_agg(to_jsonb(s) ORDER BY s.id) FROM selected s),
           'mentions', (SELECT jsonb_agg(to_jsonb(m) ORDER BY m.id) FROM mentions m JOIN selected s ON s.id = m.status_id),
           'notifications', (SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM notifications n JOIN mentions m ON m.id = n.activity_id JOIN selected s ON s.id = m.status_id WHERE n.activity_type = 'Mention'),
           'conversations', (SELECT jsonb_agg(to_jsonb(c) ORDER BY c.id) FROM account_conversations c WHERE c.conversation_id IN (SELECT conversation_id FROM selected)),
           'reply_stats', (SELECT jsonb_agg(to_jsonb(t) ORDER BY t.status_id) FROM status_stats t JOIN selected s ON s.id = t.status_id),
           'account_stats', (SELECT jsonb_agg(to_jsonb(a) ORDER BY a.account_id) FROM account_stats a WHERE a.account_id IN ($2, $3)),
           'outbox', (SELECT jsonb_agg(jsonb_build_array(id, kind, logical_key, payload) ORDER BY id) FROM rustodon.outbox_events))",
    ).bind(uris).bind(BOB).bind(PARENT_AUTHOR).fetch_one(pool).await?)
}

async fn assert_public_route(pool: &sqlx::PgPool, status_id: i64, expected: bool) -> TestResult {
    let routes: Vec<Value> = sqlx::query_scalar(
        "SELECT payload -> 'after' -> 'public' FROM rustodon.outbox_events
         WHERE kind = $1 AND payload ->> 'event' = 'update'
           AND payload ->> 'account_id' = '0' AND payload ->> 'object_id' = $2",
    )
    .bind(STREAM_EVENT_KIND)
    .bind(status_id.to_string())
    .fetch_all(pool)
    .await?;
    assert_eq!(
        routes,
        vec![json!(expected)],
        "public routing for {status_id}"
    );
    Ok(())
}
