use super::*;
use futures_util::FutureExt;
use rustodon::mastodon::rest::HtmlFormatter;

#[path = "semantic_update_history.rs"]
mod history;

// Behavioral port of the pinned 4.6.5 sanitized-HTML update contract, confirmed
// by the parent from the exact image. Reference checkout (read-only):
// /workspace/rustodon/target/mastodon-v4.6.5, or on Secunda
// /home/lain/repos/rustodon/target/mastodon-v4.6.5 at
// 1440d55b139e39ec722c2a3db7f60b66cd889048. No upstream source is needed to run.
// Only supported Note fields are exercised: inbound Question/poll editing is
// not implemented. Metadata refresh is not itself proof of a meaningful edit.
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const BOB: i64 = 116_844_606_259_202_001;
const RECIPIENT: i64 = 116_844_606_259_201_004;
const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
const RECIPIENT_URI: &str = "https://fixture-v4-6-5.rustodon.invalid/@api_moderator";
const NOTE_URI: &str = "https://remote.fixture.invalid/users/bob/statuses/semantic-update";
const PUBLISHED: &str = "2026-08-25T12:00:00Z";
const FIRST_EDIT: &str = "2026-08-25T12:01:00Z";
const NEWER: &str = "2026-08-25T12:02:00Z";
const CONTENT: &str = "<p>Some content</p>";
const EQUIVALENT: &str = "<p class=\"unsupported\">Some content</p>";

enum Scenario {
    Equivalent {
        previously_edited: bool,
        with_media: bool,
    },
    Meaningful {
        field: &'static str,
        value: Value,
    },
    Media,
    Sensitivity,
    TimestampFences,
    NoopVersionOrdering,
    Counts,
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn newer_sanitized_equivalent_html_does_not_create_an_edit() -> TestResult {
    run(Scenario::Equivalent {
        previously_edited: false,
        with_media: false,
    })
    .await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn newer_sanitized_equivalent_html_preserves_an_existing_edit() -> TestResult {
    run(Scenario::Equivalent {
        previously_edited: true,
        with_media: false,
    })
    .await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn newer_equivalent_html_with_unchanged_media_does_not_emit_edit_effects() -> TestResult {
    run(Scenario::Equivalent {
        previously_edited: true,
        with_media: true,
    })
    .await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn meaningful_content_update_still_emits_edit_effects() -> TestResult {
    run(Scenario::Meaningful {
        field: "content",
        value: json!("<p>Actually changed content</p>"),
    })
    .await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn meaningful_content_warning_update_still_emits_edit_effects() -> TestResult {
    run(Scenario::Meaningful {
        field: "summary",
        value: json!("New warning"),
    })
    .await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn sensitivity_only_update_reconciles_without_edit_effects() -> TestResult {
    // The pinned inbound significant-field list excludes standalone sensitivity;
    // the separate local edit API intentionally has a different policy.
    run(Scenario::Sensitivity).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn meaningful_media_updates_still_emit_edit_effects() -> TestResult {
    run(Scenario::Media).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn implicit_older_equal_timestamp_and_duplicate_updates_remain_fenced() -> TestResult {
    run(Scenario::TimestampFences).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn unchanged_render_reconciles_interaction_counts_without_edit_effects() -> TestResult {
    run(Scenario::Counts).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn newer_noop_does_not_fence_an_intermediate_meaningful_edit() -> TestResult {
    run(Scenario::NoopVersionOrdering).await
}

async fn run(scenario: Scenario) -> TestResult {
    let fixture = Fixture::new().await?;
    // Red assertions must not leave rows behind for the next serialized case.
    let result = std::panic::AssertUnwindSafe(async {
        match scenario {
            Scenario::Equivalent {
                previously_edited,
                with_media,
            } => equivalent(&fixture, previously_edited, with_media).await,
            Scenario::Meaningful { field, value } => meaningful(&fixture, field, value).await,
            Scenario::Media => media(&fixture).await,
            Scenario::Sensitivity => sensitivity(&fixture).await,
            Scenario::TimestampFences => timestamp_fences(&fixture).await,
            Scenario::NoopVersionOrdering => noop_version_ordering(&fixture).await,
            Scenario::Counts => counts(&fixture).await,
        }
    })
    .catch_unwind()
    .await;
    fixture.cleanup().await?;
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

async fn equivalent(fixture: &Fixture, previously_edited: bool, with_media: bool) -> TestResult {
    let attachments = if with_media {
        vec![attachment("unchanged", "Unchanged image")]
    } else {
        vec![]
    };
    if previously_edited {
        let mut edit = note(Some(FIRST_EDIT));
        edit["summary"] = json!("Existing warning");
        edit["attachment"] = json!(attachments);
        fixture.send(&edit, "real-edit").await?;
        fixture.assert_edit(FIRST_EDIT, 1).await?;
    }
    let before = fixture.state().await?;
    if with_media {
        assert_eq!(
            before.media,
            vec![(
                "https://media.fixture.invalid/semantic-unchanged.jpg".to_owned(),
                Some("Unchanged image".to_owned()),
            )]
        );
    }
    let mut update = note(Some(NEWER));
    update["attachment"] = json!(attachments);
    update["summary"] = json!(before.warning);
    update["content"] = json!(EQUIVALENT);
    assert_ne!(CONTENT, EQUIVALENT, "wire HTML must actually differ");
    assert_eq!(render(CONTENT), CONTENT);
    assert_eq!(render(EQUIVALENT), CONTENT, "independent expected render");
    assert!(timestamp(NEWER) > before.edited_at.unwrap_or(timestamp(PUBLISHED)));
    fixture.send(&update, "equivalent").await?;
    let after = fixture.state().await?;
    assert_eq!(after.rendered, before.rendered);
    // One composite assertion exposes edited_at AND notification/stream effects
    // in red output, rather than aborting before checking the latter.
    assert_eq!(
        after, before,
        "newer sanitized-equivalent HTML is not an edit"
    );
    fixture.send(&update, "equivalent-replay").await?;
    assert_eq!(fixture.state().await?, before, "replay is also effect-free");
    Ok(())
}

async fn meaningful(fixture: &Fixture, field: &str, value: Value) -> TestResult {
    let mut update = note(Some(FIRST_EDIT));
    update[field] = value;
    fixture.send(&update, "meaningful").await?;
    fixture.assert_edit(FIRST_EDIT, 1).await?;
    let state = fixture.state().await?;
    assert_eq!(state.rendered, render(update["content"].as_str().unwrap()));
    assert_eq!(state.warning, update["summary"].as_str().unwrap());
    assert_eq!(state.sensitive, update["sensitive"].as_bool().unwrap());
    fixture.send(&update, "meaningful-replay").await?;
    assert_eq!(
        fixture.state().await?,
        state,
        "replayed real edit is idempotent"
    );
    Ok(())
}

async fn sensitivity(fixture: &Fixture) -> TestResult {
    let mut expected = fixture.state().await?;
    for (suffix, version, sensitive) in [
        ("sensitivity-on", FIRST_EDIT, true),
        ("sensitivity-off", NEWER, false),
    ] {
        let mut update = note(Some(version));
        update["sensitive"] = json!(sensitive);
        fixture.send(&update, suffix).await?;
        expected.sensitive = sensitive;
        assert_eq!(
            fixture.state().await?,
            expected,
            "standalone sensitivity must reconcile without marking edited or broadcasting"
        );
    }
    Ok(())
}

async fn media(fixture: &Fixture) -> TestResult {
    let first = attachment("first", "First image");
    let described = attachment("first", "Changed alt text");
    let second = attachment("second", "Second image");
    // Every step has identical text/CW/sensitivity. Preserve supported media
    // addition, alt-text edit, replacement, ordering, and removal independently.
    for (index, attachments) in [
        vec![first],
        vec![described.clone()],
        vec![second.clone()],
        vec![second.clone(), described.clone()],
        vec![described, second],
        vec![],
    ]
    .into_iter()
    .enumerate()
    {
        let version = (timestamp(FIRST_EDIT) + Duration::minutes(i64::try_from(index)?))
            .and_utc()
            .to_rfc3339();
        let mut update = note(Some(&version));
        update["attachment"] = json!(attachments);
        fixture.send(&update, &format!("media-{index}")).await?;
        fixture
            .assert_edit(&version, i64::try_from(index)? + 1)
            .await?;
        let state = fixture.state().await?;
        let expected: Vec<(String, Option<String>)> = attachments
            .iter()
            .map(|item| {
                (
                    item["url"].as_str().unwrap().to_owned(),
                    item["name"].as_str().map(str::to_owned),
                )
            })
            .collect();
        assert_eq!(state.media, expected);
        assert_eq!(state.rendered, CONTENT);
        assert_eq!(state.warning, "");
        assert!(!state.sensitive);
        fixture
            .send(&update, &format!("media-{index}-replay"))
            .await?;
        assert_eq!(fixture.state().await?, state);
    }
    Ok(())
}

async fn timestamp_fences(fixture: &Fixture) -> TestResult {
    let mut edit = note(Some(FIRST_EDIT));
    edit["content"] = json!("<p>Accepted edit</p>");
    fixture.send(&edit, "accepted").await?;
    fixture.assert_edit(FIRST_EDIT, 1).await?;
    let before = fixture.state().await?;
    fixture.send(&edit, "duplicate-distinct-job").await?;
    assert_eq!(fixture.state().await?, before);
    for (suffix, version) in [
        ("older", Some("2026-08-25T12:00:30Z")),
        ("equal-timestamp", Some(FIRST_EDIT)),
        ("implicit", None),
    ] {
        let mut rejected = note(version);
        rejected["content"] = json!("<p>This really differs but must not replace the edit</p>");
        rejected["summary"] = json!("Rejected warning");
        rejected["sensitive"] = json!(true);
        rejected["attachment"] = json!([attachment("rejected", "Must not attach")]);
        fixture.send(&rejected, suffix).await?;
        assert_eq!(
            fixture.state().await?,
            before,
            "{suffix} changed edit state"
        );
    }
    Ok(())
}

async fn noop_version_ordering(fixture: &Fixture) -> TestResult {
    // Pinned 4.6.5 ProcessStatusUpdateService: only significant changes advance
    // edited_at (174-184); ordering checks that timestamp (26-35, 435-436).
    // Source: read-only audit-reference/remaining, detailed in the owned issue.
    const T3: &str = "2026-08-25T12:03:00Z";
    assert!(timestamp(FIRST_EDIT) < timestamp(NEWER));
    assert!(timestamp(NEWER) < timestamp(T3));
    let mut t1 = note(Some(FIRST_EDIT));
    t1["summary"] = json!("T1 warning");
    fixture.send(&t1, "ordering-t1").await?;
    fixture.assert_edit(FIRST_EDIT, 1).await?;
    let mut expected = fixture.state().await?;

    let mut t3 = t1.clone();
    t3["updated"] = json!(T3);
    t3["content"] = json!(EQUIVALENT);
    t3["sensitive"] = json!(true);
    assert_ne!(t3["content"], t1["content"]);
    assert_eq!(render(EQUIVALENT), CONTENT);
    expected.sensitive = true;
    for suffix in ["ordering-t3-noop", "ordering-t3-noop-replay"] {
        fixture.send(&t3, suffix).await?;
        assert_eq!(fixture.state().await?, expected, "T3 is not an edit fence");
        fixture.assert_edit(FIRST_EDIT, 1).await?;
    }

    let mut t2 = t1;
    t2["updated"] = json!(NEWER);
    t2["content"] = json!("<p>T2 meaningful content</p>");
    fixture.send(&t2, "ordering-t2-meaningful").await?;
    fixture.assert_edit(NEWER, 2).await?;
    let accepted = fixture.state().await?;
    assert_eq!(accepted.rendered, "<p>T2 meaningful content</p>");
    assert_eq!(accepted.warning, "T1 warning");
    assert!(!accepted.sensitive, "T2 metadata also reconciles");
    fixture.send(&t2, "ordering-t2-replay").await?;
    assert_eq!(fixture.state().await?, accepted);
    for (suffix, version) in [("ordering-older", FIRST_EDIT), ("ordering-equal", NEWER)] {
        let mut rejected = note(Some(version));
        rejected["content"] = json!("<p>Must not replace T2</p>");
        rejected["sensitive"] = json!(true);
        fixture.send(&rejected, suffix).await?;
        assert_eq!(fixture.state().await?, accepted, "{suffix}");
    }
    Ok(())
}

async fn counts(fixture: &Fixture) -> TestResult {
    let before = fixture.state().await?;
    let trusted: (i64, i64) = sqlx::query_as(
        "SELECT favourites_count, reblogs_count FROM status_stats WHERE status_id = $1",
    )
    .bind(fixture.status_id)
    .fetch_one(&fixture.owner)
    .await?;
    for (suffix, version, likes, shares) in [
        ("explicit-counts", Some(NEWER), 7_i64, 3_i64),
        ("implicit-counts", None, 11, 5),
    ] {
        let mut update = note(version);
        update["content"] = json!(EQUIVALENT);
        update["likes"] = json!({"type": "Collection", "totalItems": likes});
        update["shares"] = json!({"type": "Collection", "totalItems": shares});
        fixture.send(&update, suffix).await?;
        assert_eq!(
            sqlx::query_as::<_, (i64, i64, i64, i64)>(
                "SELECT untrusted_favourites_count, untrusted_reblogs_count,
                favourites_count, reblogs_count FROM status_stats WHERE status_id = $1",
            )
            .bind(fixture.status_id)
            .fetch_one(&fixture.owner)
            .await?,
            (likes, shares, trusted.0, trusted.1),
            "metadata must still reconcile"
        );
        assert_eq!(
            fixture.state().await?,
            before,
            "count refresh is not an edit"
        );
    }
    Ok(())
}

fn note(updated: Option<&str>) -> Value {
    let mut object = json!({
        "id": NOTE_URI, "type": "Note", "attributedTo": ACTOR,
        "published": PUBLISHED, "content": CONTENT, "summary": "",
        "sensitive": false, "language": "en",
        "to": ["https://www.w3.org/ns/activitystreams#Public"], "cc": [RECIPIENT_URI],
        "tag": [{"type": "Mention", "href": RECIPIENT_URI, "name": "@api_moderator"}],
        "attachment": [],
    });
    if let Some(updated) = updated {
        object["updated"] = json!(updated);
    }
    object
}

fn attachment(name: &str, description: &str) -> Value {
    json!({"type": "Document", "mediaType": "image/jpeg",
        "url": format!("https://media.fixture.invalid/semantic-{name}.jpg"),
        "name": description, "width": 320, "height": 240})
}

fn timestamp(value: &str) -> NaiveDateTime {
    DateTime::parse_from_rfc3339(value).unwrap().naive_utc()
}

fn render(value: &str) -> String {
    let origin = Url::parse(ORIGIN).unwrap();
    HtmlFormatter::new(&origin, "fixture-v4-6-5.rustodon.invalid")
        .remote_fragment(value)
        .as_str()
        .to_owned()
}

#[derive(Debug, PartialEq)]
struct EditState {
    rendered: String,
    warning: String,
    sensitive: bool,
    edited_at: Option<NaiveDateTime>,
    media: Vec<(String, Option<String>)>,
    // Durable update intents plus both home and notification status streams.
    effects: Vec<Value>,
    notifications: Vec<Value>,
    history_rows: Vec<Value>,
}

struct Fixture {
    owner: sqlx::PgPool,
    queue: Queue,
    executor: WorkerExecutor,
    status_id: i64,
    boost_id: i64,
    mention_id: i64,
    follow_id: Option<i64>,
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    async fn new() -> TestResult<Self> {
        let owner =
            sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?).await?;
        let runtime =
            sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
        let writer =
            sqlx::PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
        reset().await?;
        let writer_role: (String, bool) = sqlx::query_as(
            "SELECT current_user::text, rolsuper FROM pg_roles WHERE rolname = current_user",
        )
        .fetch_one(&writer)
        .await?;
        let owner_role: String = sqlx::query_scalar("SELECT current_user::text")
            .fetch_one(&owner)
            .await?;
        assert_ne!(
            writer_role.0, owner_role,
            "the owner must not execute production writes"
        );
        assert!(!writer_role.1, "worker writer must not be a superuser");
        let queue = Queue::new(runtime);
        let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
            &queue,
            Some(writer),
            None,
            Some(ActivityPubDeliveryConfig {
                origin: Url::parse(ORIGIN)?,
                local_domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
                media_root_url: "/system".to_owned(),
                media_root: None,
                limited_federation: false,
                #[cfg(feature = "test-support")]
                remote_fetch_endpoint: None,
                #[cfg(feature = "test-support")]
                remote_media_endpoint: None,
                #[cfg(feature = "test-support")]
                remote_delivery_endpoint: None,
            }),
        )?;
        let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
        // Seed an existing remote Note, not a fake update implementation. All
        // Updates and notification writes below use the actual restricted worker.
        // Setup is transactional and only adds task-owned rows/relationships.
        let mut transaction = owner.begin().await?;
        let status_id: i64 = sqlx::query_scalar(
            "INSERT INTO statuses (account_id, uri, text, spoiler_text, visibility, local,
                language, sensitive, reply, created_at, updated_at)
             VALUES ($1, $2, $3, '', 0, false, 'en', false, false, $4, $4) RETURNING id",
        )
        .bind(BOB)
        .bind(NOTE_URI)
        .bind(CONTENT)
        .bind(timestamp(PUBLISHED))
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO status_stats (status_id, created_at, updated_at)
            VALUES ($1, clock_timestamp(), clock_timestamp())",
        )
        .bind(status_id)
        .execute(&mut *transaction)
        .await?;
        let boost_id: i64 = sqlx::query_scalar(
            "INSERT INTO statuses (account_id, reblog_of_id, text, spoiler_text,
                visibility, local, sensitive, reply, created_at, updated_at)
             VALUES ($1, $2, '', '', 0, true, false, false,
                clock_timestamp(), clock_timestamp()) RETURNING id",
        )
        .bind(RECIPIENT)
        .bind(status_id)
        .fetch_one(&mut *transaction)
        .await?;
        let mention_id: i64 = sqlx::query_scalar(
            "INSERT INTO mentions (account_id, status_id, silent, created_at, updated_at)
             VALUES ($1, $2, false, clock_timestamp(), clock_timestamp()) RETURNING id",
        )
        .bind(RECIPIENT)
        .bind(status_id)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO notifications (account_id, activity_id, activity_type,
                from_account_id, type, filtered, created_at, updated_at)
             VALUES ($1, $2, 'Mention', $3, 'mention', false,
                clock_timestamp(), clock_timestamp())",
        )
        .bind(RECIPIENT)
        .bind(mention_id)
        .bind(BOB)
        .execute(&mut *transaction)
        .await?;
        let follow_id = sqlx::query_scalar(
            "INSERT INTO follows (account_id, target_account_id, show_reblogs, notify,
                created_at, updated_at)
             VALUES ($1, $2, true, false, clock_timestamp(), clock_timestamp())
             ON CONFLICT (account_id, target_account_id) DO NOTHING RETURNING id",
        )
        .bind(RECIPIENT)
        .bind(BOB)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(Self {
            owner,
            queue,
            executor,
            status_id,
            boost_id,
            mention_id,
            follow_id,
        })
    }

    async fn send(&self, object: &Value, suffix: &str) -> TestResult {
        // A distinct durable key on every call ensures replay tests reach the
        // ingress timestamp/idempotency fences, not queue enqueue de-duplication.
        let body = json!({"type": "Update", "actor": ACTOR, "object": object});
        let job_id = self.queue.enqueue(&JobSpec::new(
            Lane::Ingress, ACTIVITYPUB_INBOX_JOB_KIND, json!({
                "body": body.to_string(), "delivery_target_account_id": RECIPIENT,
                "signature_key_id": "https://remote.fixture.invalid/users/bob#secondary-key",
                "remote_domain": "remote.fixture.invalid",
            }),
        ).logical_key(format!("semantic-update:{}:{suffix}", self.status_id))).await?;
        assert!(
            self.executor
                .process_one(
                    "semantic-update-ingress",
                    &[Lane::Ingress],
                    Duration::seconds(30),
                )
                .await?
        );
        let pending: Option<(i32, Option<String>)> =
            sqlx::query_as("SELECT attempts, last_error FROM rustodon.durable_jobs WHERE id = $1")
                .bind(job_id)
                .fetch_optional(self.queue.pool())
                .await?;
        assert!(pending.is_none(), "Update did not complete: {pending:?}");
        self.queue.dispatch_outbox(100).await?;
        // Materialize notification intents; deliberately never run Pull/Push.
        // Media tests therefore need no files, network, HTTP server or transport.
        for _ in 0..20 {
            if !self
                .executor
                .process_one(
                    "semantic-update-notifications",
                    &[Lane::Core],
                    Duration::seconds(30),
                )
                .await?
            {
                break;
            }
        }
        let pending: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT kind, last_error FROM rustodon.durable_jobs WHERE kind = $1")
                .bind(NOTIFICATION_CREATE_JOB_KIND)
                .fetch_all(self.queue.pool())
                .await?;
        assert!(
            pending.is_empty(),
            "notification jobs did not complete: {pending:?}"
        );
        Ok(())
    }

    async fn state(&self) -> TestResult<EditState> {
        let (text, warning, sensitive, edited_at): (String, String, bool, Option<NaiveDateTime>) =
            sqlx::query_as(
                "SELECT text, spoiler_text, sensitive, edited_at FROM statuses WHERE id = $1",
            )
            .bind(self.status_id)
            .fetch_one(&self.owner)
            .await?;
        let media = sqlx::query_as(
            "SELECT media.remote_url, media.description FROM statuses status
             CROSS JOIN LATERAL unnest(status.ordered_media_attachment_ids)
                WITH ORDINALITY ordered(id, position)
             JOIN media_attachments media ON media.id = ordered.id
             WHERE status.id = $1 ORDER BY ordered.position",
        )
        .bind(self.status_id)
        .fetch_all(&self.owner)
        .await?;
        let effects = sqlx::query_scalar(
            "SELECT jsonb_build_object('kind', kind, 'key', logical_key, 'payload', payload)
             FROM rustodon.outbox_events
             WHERE (kind = $1 AND payload ->> 'object_id' = $3
                    AND payload ->> 'event' IN ('status.update', 'status.update:notification'))
                OR (kind = $2 AND payload -> 'arguments' ->> 'activity_id' = $3
                    AND payload -> 'arguments' ->> 'activity_type' = 'update')
             ORDER BY id",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(self.status_id.to_string())
        .fetch_all(&self.owner)
        .await?;
        let notifications = sqlx::query_scalar(
            "SELECT to_jsonb(notification) FROM notifications notification
             WHERE (activity_type = 'Status' AND activity_id = $1)
                OR (activity_type = 'Mention' AND activity_id = $2) ORDER BY id",
        )
        .bind(self.status_id)
        .bind(self.mention_id)
        .fetch_all(&self.owner)
        .await?;
        Ok(EditState {
            rendered: render(&text),
            warning,
            sensitive,
            edited_at,
            media,
            effects,
            notifications,
            history_rows: self.history_rows().await?,
        })
    }

    async fn history_rows(&self) -> TestResult<Vec<Value>> {
        Ok(sqlx::query_scalar(
            "SELECT to_jsonb(edit) FROM status_edits edit WHERE status_id = $1 ORDER BY id",
        )
        .bind(self.status_id)
        .fetch_all(&self.owner)
        .await?)
    }

    async fn assert_edit(&self, version: &str, expected_versions: i64) -> TestResult {
        let state = self.state().await?;
        assert_eq!(state.edited_at, Some(timestamp(version)));
        for event in ["status.update", "status.update:notification"] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1
                 AND payload ->> 'object_id' = $2 AND payload ->> 'account_id' = $3
                 AND payload ->> 'event' = $4",
                )
                .bind(STREAM_EVENT_KIND)
                .bind(self.status_id.to_string())
                .bind(RECIPIENT.to_string())
                .bind(event)
                .fetch_one(&self.owner)
                .await?,
                expected_versions,
                "missing/duplicate {event} for a real edit"
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events WHERE kind = $1
             AND payload -> 'arguments' ->> 'activity_id' = $2
             AND payload -> 'arguments' ->> 'recipient_account_id' = $3
             AND payload -> 'arguments' ->> 'activity_type' = 'update'",
            )
            .bind(NOTIFICATION_CREATE_JOB_KIND)
            .bind(self.status_id.to_string())
            .bind(RECIPIENT.to_string())
            .fetch_one(&self.owner)
            .await?,
            expected_versions
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications WHERE activity_type = 'Status'
             AND activity_id = $1 AND account_id = $2 AND type = 'update'",
            )
            .bind(self.status_id)
            .bind(RECIPIENT)
            .fetch_one(&self.owner)
            .await?,
            1
        );
        Ok(())
    }

    async fn cleanup(&self) -> TestResult {
        sqlx::query(
            "DELETE FROM notifications WHERE
            (activity_type = 'Status' AND activity_id = $1)
            OR (activity_type = 'Mention' AND activity_id = $2)",
        )
        .bind(self.status_id)
        .bind(self.mention_id)
        .execute(&self.owner)
        .await?;
        sqlx::query("DELETE FROM status_edits WHERE status_id = $1")
            .bind(self.status_id)
            .execute(&self.owner)
            .await?;
        // The media FK uses ON DELETE SET NULL, not CASCADE.
        sqlx::query("DELETE FROM media_attachments WHERE status_id = $1")
            .bind(self.status_id)
            .execute(&self.owner)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id = $1")
            .bind(self.boost_id)
            .execute(&self.owner)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id = $1")
            .bind(self.status_id)
            .execute(&self.owner)
            .await?;
        if let Some(follow_id) = self.follow_id {
            sqlx::query("DELETE FROM follows WHERE id = $1")
                .bind(follow_id)
                .execute(&self.owner)
                .await?;
        }
        reset().await?;
        Ok(())
    }
}
