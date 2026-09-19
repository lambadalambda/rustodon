use super::*;
use futures_util::FutureExt;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
type ConversationRow = (i64, Vec<i64>, Vec<i64>, i64, bool);
const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
const ALICE: i64 = 116_844_606_259_201_001;
const MODERATOR: i64 = 116_844_606_259_201_002;
const BOB: i64 = 116_844_606_259_202_001;
const ALICE_TOKEN: &str = "fixture-bearer-token-v4-6-5";
const OUTSIDER_TOKEN: &str = "fixture-bearer-api-moderator-v4-6-5";

// Contract confirmed by the parent against pinned 4.6.5 Create lines 126–150:
// explicit mentions stay direct; adding a silent audience recipient makes it limited.
// This is post-signature-verification ingress, not a replacement peer/signature test.
#[derive(Clone, Copy)]
enum Audience {
    DirectTo,
    DirectCc,
    Silent,
    Mixed,
    Public,
    Unlisted,
    Followers,
}

impl Audience {
    fn label(self) -> &'static str {
        match self {
            Self::DirectTo => "direct-to",
            Self::DirectCc => "direct-cc",
            Self::Silent => "silent",
            Self::Mixed => "mixed",
            Self::Public => "public",
            Self::Unlisted => "unlisted",
            Self::Followers => "followers",
        }
    }

    fn visibility(self) -> (i32, &'static str) {
        match self {
            Self::DirectTo | Self::DirectCc => (3, "direct"),
            Self::Silent | Self::Mixed => (4, "private"),
            Self::Public => (0, "public"),
            Self::Unlisted => (1, "unlisted"),
            Self::Followers => (2, "private"),
        }
    }

    fn explicit(self) -> bool {
        !matches!(self, Self::Silent)
    }
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn explicitly_mentioned_to_recipient_is_direct() -> TestResult {
    check_audience(Audience::DirectTo).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn explicitly_mentioned_cc_recipient_is_direct() -> TestResult {
    check_audience(Audience::DirectCc).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn silent_recipient_is_limited_without_notification_or_conversation() -> TestResult {
    check_audience(Audience::Silent).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn explicit_mention_with_silent_cc_recipient_is_limited() -> TestResult {
    check_audience(Audience::Mixed).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn explicit_mentions_preserve_public_unlisted_and_followers_visibility() -> TestResult {
    for audience in [Audience::Public, Audience::Unlisted, Audience::Followers] {
        check_audience(audience).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn check_audience(audience: Audience) -> TestResult {
    let runtime = sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let writer =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let owner =
        sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
    reset().await?;
    let writer_role: (String, bool) = sqlx::query_as(
        "SELECT current_user::text, rolsuper FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&writer)
    .await?;
    assert_eq!(writer_role, ("rustodon_differential_writer".into(), false));
    let baseline: (i64, Option<NaiveDateTime>) = sqlx::query_as(
        "SELECT statuses_count, last_status_at FROM account_stats WHERE account_id = $1",
    )
    .bind(BOB)
    .fetch_one(&owner)
    .await?;
    let repository = Repository::from_pool(runtime.clone());
    let origin = Url::parse(ORIGIN)?;
    let alice = repository.account(ALICE).await?.ok_or("Alice missing")?;
    let moderator = repository
        .account(MODERATOR)
        .await?
        .ok_or("moderator missing")?;
    let alice_uri = activitypub::actor_url(&origin, &alice);
    let moderator_uri = activitypub::actor_url(&origin, &moderator);
    let public = "https://www.w3.org/ns/activitystreams#Public".to_owned();
    let followers: String = sqlx::query_scalar("SELECT followers_url FROM accounts WHERE id = $1")
        .bind(BOB)
        .fetch_one(&runtime)
        .await?;
    assert!(
        !followers.is_empty(),
        "fixture must advertise Bob's followers collection"
    );
    let (to, cc) = match audience {
        Audience::DirectCc => (vec![], vec![alice_uri.clone()]),
        Audience::Mixed => (vec![alice_uri.clone()], vec![moderator_uri.clone()]),
        Audience::Public => (vec![public], vec![alice_uri.clone()]),
        Audience::Unlisted => (vec![alice_uri.clone()], vec![public]),
        Audience::Followers => (vec![followers], vec![alice_uri.clone()]),
        _ => (vec![alice_uri.clone()], vec![]),
    };
    let tags = if audience.explicit() {
        vec![json!({"type": "Mention", "href": alice_uri, "name": "@alice"})]
    } else {
        vec![]
    };
    let uri = format!("{ACTOR}/statuses/direct-visibility-{}", audience.label());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM statuses WHERE uri = $1")
            .bind(&uri)
            .fetch_one(&runtime)
            .await?,
        0,
        "use a clean disposable fixture"
    );
    let body = json!({
        "id": format!("{uri}/activity"), "type": "Create", "actor": ACTOR,
        "object": {
            "id": uri, "type": "Note", "attributedTo": ACTOR,
            "published": "2026-09-01T12:00:00Z", "content": "<p>Audience regression</p>",
            "summary": "", "to": to, "cc": cc, "tag": tags,
        }
    });
    let queue = Queue::new(runtime.clone());
    let executor = WorkerExecutor::new(
        queue.clone(),
        infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(writer),
            None,
            Some(ActivityPubDeliveryConfig {
                origin,
                local_domain: DOMAIN.to_owned(),
                media_root_url: "/system".to_owned(),
                media_root: None,
                limited_federation: false,
                #[cfg(feature = "test-support")]
                remote_media_endpoint: None,
                #[cfg(feature = "test-support")]
                remote_delivery_endpoint: None,
                #[cfg(feature = "test-support")]
                remote_fetch_endpoint: None,
            }),
        )?,
        1,
        1,
    )?;
    let media_path = std::env::temp_dir().join(format!(
        "rustodon-direct-visibility-{}-{}",
        std::process::id(),
        audience.label()
    ));
    fs::create_dir(&media_path)?;
    let app = status_router(runtime.clone(), media_path.clone())?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    // Red assertions must also clean up, so the other cases can run serially.
    let result = std::panic::AssertUnwindSafe(async {
        ingest(&queue, &executor, &body, "initial").await?;
        let (id, author, local, visibility, conversation): (i64, i64, bool, i32, i64) =
            sqlx::query_as("SELECT id, account_id, local, visibility, conversation_id FROM statuses WHERE uri = $1")
                .bind(&uri).fetch_one(&runtime).await?;
        assert_eq!((author, local), (BOB, false), "preserve remote author provenance");
        assert_eq!(visibility, audience.visibility().0, "{} DB visibility", audience.label());
        let mentions: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT account_id, silent FROM mentions WHERE status_id = $1 ORDER BY account_id",
        ).bind(id).fetch_all(&runtime).await?;
        let mut expected = vec![(ALICE, !audience.explicit())];
        if matches!(audience, Audience::Mixed) {
            expected.push((MODERATOR, true));
        }
        assert_eq!(mentions, expected);
        let (code, status) = get_status(&client, endpoint, id, Some(ALICE_TOKEN)).await?;
        assert_eq!(code, http::StatusCode::OK, "recipient REST access");
        assert_eq!(status["uri"], uri);
        assert_eq!(status["visibility"], audience.visibility().1);
        for token in [None, Some(OUTSIDER_TOKEN)] {
            let (code, _) = get_status(&client, endpoint, id, token).await?;
            let expected = if matches!(audience, Audience::Public | Audience::Unlisted) {
                http::StatusCode::OK
            } else {
                http::StatusCode::NOT_FOUND
            };
            assert_eq!(code, expected, "nonrecipient REST access: {token:?}");
        }
        drain_notifications(&queue, &executor).await?;
        let notified: Vec<i64> = sqlx::query_scalar(
            "SELECT notification.account_id FROM notifications notification
             JOIN mentions mention ON mention.id = notification.activity_id
             WHERE notification.activity_type = 'Mention' AND mention.status_id = $1
             ORDER BY notification.account_id",
        ).bind(id).fetch_all(&runtime).await?;
        assert_eq!(notified, if audience.explicit() { vec![ALICE] } else { vec![] });
        let conversations: Vec<ConversationRow> = sqlx::query_as(
            "SELECT account_id, participant_account_ids, status_ids, last_status_id, unread
             FROM account_conversations WHERE conversation_id = $1 ORDER BY account_id",
        ).bind(conversation).fetch_all(&runtime).await?;
        let direct = visibility == 3;
        assert_eq!(conversations, if direct {
            vec![(ALICE, vec![BOB], vec![id], id, true)]
        } else { vec![] }, "only direct Notes enter account conversations");
        let conversation_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rustodon.outbox_events
             WHERE kind = $1 AND payload ->> 'event' = 'conversation'",
        ).bind(STREAM_EVENT_KIND).fetch_one(&runtime).await?;
        assert_eq!(conversation_events, i64::from(direct));
        if visibility >= 3 {
            assert_eq!(sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                 WHERE kind = $1 AND payload ->> 'object_id' = $2
                   AND payload ->> 'event' = 'update'
                   AND (payload ->> 'account_id' IS NULL
                        OR (payload ->> 'account_id')::bigint <> ALL($3::bigint[]))",
            ).bind(STREAM_EVENT_KIND).bind(id.to_string())
                .bind(expected.iter().map(|(id, _)| *id).collect::<Vec<_>>())
                .fetch_one(&runtime).await?, 0, "no status stream to nonrecipients");
            assert_eq!(sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1",
            ).bind(ACTIVITYPUB_DELIVERY_JOB_KIND).fetch_one(&runtime).await?, 0,
                "no forwarding of direct/limited content");
        }
        let before = snapshot(&runtime, &uri).await?;
        // New queue identity forces real Create replay, rather than enqueue deduplication.
        ingest(&queue, &executor, &body, "replay").await?;
        drain_notifications(&queue, &executor).await?;
        assert_eq!(snapshot(&runtime, &uri).await?, before,
            "replay must not duplicate rows, notifications, conversations or stream intents");
        let (code, replayed) = get_status(&client, endpoint, id, Some(ALICE_TOKEN)).await?;
        assert_eq!(code, http::StatusCode::OK);
        assert_eq!(replayed["visibility"], audience.visibility().1);
        Ok::<(), Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
    server.abort();
    let _ = server.await;
    sqlx::query(
        "DELETE FROM notifications WHERE activity_type = 'Mention' AND activity_id IN
         (SELECT id FROM mentions WHERE status_id IN (SELECT id FROM statuses WHERE uri = $1))",
    )
    .bind(&uri)
    .execute(&owner)
    .await?;
    sqlx::query("DELETE FROM conversations WHERE parent_status_id IN (SELECT id FROM statuses WHERE uri = $1)")
        .bind(&uri).execute(&owner).await?;
    sqlx::query("DELETE FROM statuses WHERE uri = $1")
        .bind(&uri)
        .execute(&owner)
        .await?;
    sqlx::query(
        "UPDATE account_stats SET statuses_count = $2, last_status_at = $3 WHERE account_id = $1",
    )
    .bind(BOB)
    .bind(baseline.0)
    .bind(baseline.1)
    .execute(&owner)
    .await?;
    reset().await?;
    fs::remove_dir_all(media_path)?;
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

async fn ingest(
    queue: &Queue,
    executor: &WorkerExecutor,
    body: &Value,
    suffix: &str,
) -> TestResult {
    let id = queue
        .enqueue(
            &JobSpec::new(
                Lane::Ingress,
                ACTIVITYPUB_INBOX_JOB_KIND,
                json!({"body": body.to_string(), "delivery_target_account_id": ALICE,
            "signature_key_id": format!("{ACTOR}#secondary-key"),
            "remote_domain": "remote.fixture.invalid"}),
            )
            .logical_key(format!("direct-visibility:{suffix}")),
        )
        .await?;
    assert!(
        executor
            .process_one("direct-visibility", &[Lane::Ingress], Duration::seconds(30))
            .await?
    );
    let remaining: Option<(i32, Option<String>)> =
        sqlx::query_as("SELECT attempts, last_error FROM rustodon.durable_jobs WHERE id = $1")
            .bind(id)
            .fetch_optional(queue.pool())
            .await?;
    assert!(
        remaining.is_none(),
        "ingress must acknowledge, not retry/dead-letter: {remaining:?}"
    );
    Ok(())
}

async fn drain_notifications(queue: &Queue, executor: &WorkerExecutor) -> TestResult {
    while queue.dispatch_outbox(100).await? != 0 {}
    for _ in 0..20 {
        if !executor
            .process_one("direct-notifications", &[Lane::Core], Duration::seconds(30))
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

async fn snapshot(pool: &sqlx::PgPool, uri: &str) -> TestResult<Value> {
    Ok(sqlx::query_scalar(
        "SELECT jsonb_build_object(
          'statuses', (SELECT jsonb_agg(to_jsonb(s) ORDER BY s.id) FROM statuses s WHERE s.uri = $1),
          'mentions', (SELECT jsonb_agg(to_jsonb(m) ORDER BY m.id) FROM mentions m JOIN statuses s ON s.id = m.status_id WHERE s.uri = $1),
          'notifications', (SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM notifications n JOIN mentions m ON m.id = n.activity_id JOIN statuses s ON s.id = m.status_id WHERE n.activity_type = 'Mention' AND s.uri = $1),
          'conversations', (SELECT jsonb_agg(to_jsonb(c) ORDER BY c.id) FROM account_conversations c JOIN statuses s ON s.conversation_id = c.conversation_id WHERE s.uri = $1),
          'outbox', (SELECT jsonb_agg(jsonb_build_array(id, kind, logical_key, payload) ORDER BY id) FROM rustodon.outbox_events),
          'stats', (SELECT to_jsonb(a) FROM account_stats a WHERE a.account_id = $2))",
    ).bind(uri).bind(BOB).fetch_one(pool).await?)
}

fn status_router(pool: sqlx::PgPool, media_path: PathBuf) -> TestResult<axum::Router> {
    Ok(rustodon::web::router(rustodon::web::WebState::new(
        Repository::from_pool(pool),
        Url::parse(ORIGIN)?,
        DOMAIN,
        "/system",
        media_path,
        InstanceRuntimeConfig {
            domain: DOMAIN.to_owned(),
            version: "4.6.5".to_owned(),
            source_url: "https://github.com/mastodon/mastodon".to_owned(),
            streaming_api: format!("wss://{DOMAIN}"),
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
        vec![DOMAIN.to_owned()],
    )?))
}

async fn get_status(
    client: &reqwest::Client,
    endpoint: std::net::SocketAddr,
    id: i64,
    token: Option<&str>,
) -> TestResult<(http::StatusCode, Value)> {
    let mut request = client
        .get(format!("http://{endpoint}/api/v1/statuses/{id}"))
        .header(HOST, DOMAIN);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await?;
    let code = response.status();
    Ok((code, serde_json::from_slice(&response.bytes().await?)?))
}
