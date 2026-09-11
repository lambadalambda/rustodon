use super::*;

#[tokio::test]
#[ignore = "starts a disposable restored Mastodon PostgreSQL fixture through Mise"]
#[allow(clippy::too_many_lines)]
async fn dedicated_writer_persists_emoji_parent_and_installs_media()
-> Result<(), Box<dyn std::error::Error>> {
    const BOB: i64 = 116_844_606_259_202_001;
    const DOMAIN: &str = "remote.fixture.invalid";
    const ACTOR: &str = "https://remote.fixture.invalid/users/bob";
    const PARENT: &str = "http://remote.fixture.invalid/users/bob/statuses/emoji-grants-parent";
    const CHILD: &str = "https://remote.fixture.invalid/users/bob/statuses/emoji-grants-child";
    let runtime = PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?)
        .await?;
    let writer = PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?)
        .await?;
    let owner = PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("RUSTODON_WORKER_OWNER_DATABASE_URL")?)
        .await?;
    reset().await?;
    assert_eq!(
        sqlx::query_as::<_, (String, bool)>(
            "SELECT current_user::text, rolsuper FROM pg_roles WHERE rolname = current_user"
        )
        .fetch_one(&writer)
        .await?,
        ("rustodon_differential_writer".into(), false)
    );
    // Runtime never gains Mastodon writes, including nextval access.
    assert_eq!(
        sqlx::query_as::<_, (bool, bool, bool)>(
            "SELECT has_any_column_privilege(current_user, 'custom_emojis', 'INSERT'),
                has_any_column_privilege(current_user, 'custom_emojis', 'UPDATE'),
                has_sequence_privilege(current_user, 'custom_emojis_id_seq', 'USAGE, UPDATE')"
        )
        .fetch_one(&runtime)
        .await?,
        (false, false, false)
    );
    let baseline: (i64, Option<NaiveDateTime>) = sqlx::query_as(
        "SELECT statuses_count, last_status_at FROM account_stats WHERE account_id = $1",
    )
    .bind(BOB)
    .fetch_one(&owner)
    .await?;
    let child: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (account_id, uri, text, spoiler_text, visibility, local,
             sensitive, reply, created_at, updated_at)
         VALUES ($1, $2, 'child', '', 0, false, false, true,
             clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(BOB)
    .bind(CHILD)
    .fetch_one(&owner)
    .await?;
    let existing: i64 = sqlx::query_scalar(
        "INSERT INTO custom_emojis (shortcode, domain, uri, image_remote_url,
             disabled, visible_in_picker, created_at, updated_at)
         VALUES ('reply_grants_existing', $1, 'https://remote.fixture.invalid/emojis/old',
             'https://media.fixture.invalid/old.gif', true, false, '2020-01-01', '2020-01-01')
         RETURNING id",
    )
    .bind(DOMAIN)
    .fetch_one(&owner)
    .await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = listener.local_addr()?;
    let media_listener = TcpListener::bind("127.0.0.1:0").await?;
    let media_endpoint = media_listener.local_addr()?;
    let root_path = std::env::temp_dir().join(format!(
        "rustodon-reply-emoji-grants-{}",
        std::process::id()
    ));
    fs::create_dir(&root_path)?;
    let media_root = PaperclipRoot::open(&root_path)?;
    let emoji_url = |name: &str| {
        format!(
            "http://media.fixture.invalid:{}/{name}.gif",
            media_endpoint.port()
        )
    };
    let parent = json!({
        "id": PARENT, "type": "Note", "attributedTo": ACTOR,
        "content": "<p>:reply_grants_new: :reply_grants_existing:</p>",
        "published": Utc::now().to_rfc3339(),
        "to": ["https://www.w3.org/ns/activitystreams#Public"], "cc": [],
        "tag": [
            {"type": "Emoji", "id": "https://remote.fixture.invalid/emojis/new",
             "name": ":reply_grants_new:",
             "icon": {"type": "Image", "mediaType": "image/gif", "url": emoji_url("new")}},
            {"type": "Emoji", "id": "https://remote.fixture.invalid/emojis/existing-v2",
             "name": ":reply_grants_existing:",
             "icon": {"type": "Image", "mediaType": "image/gif", "url": emoji_url("existing-v2")}}
        ]
    });
    let server = tokio::spawn(fixture_activitypub_server(
        listener,
        serde_json::to_vec(&parent)?,
    ));
    let queue = Queue::new(runtime.clone());
    let handlers = infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        Some(writer.clone()),
        None,
        Some(ActivityPubDeliveryConfig {
            origin: Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            local_domain: "fixture-v4-6-5.rustodon.invalid".into(),
            media_root_url: "/system".into(),
            media_root: Some(media_root.clone()),
            limited_federation: false,
            remote_fetch_endpoint: Some(endpoint),
            remote_media_endpoint: Some(media_endpoint),
            remote_delivery_endpoint: None,
        }),
    )?;
    let executor = WorkerExecutor::new(queue.clone(), handlers, 1, 1)?;
    let operation = async {
        let job_id = queue.enqueue(&JobSpec::new(Lane::Pull, ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND,
            json!({"child_status_id": child, "parent_url": PARENT}))).await?;
        assert!(executor.process_one("emoji-thread-writer", &[Lane::Pull], Duration::seconds(30)).await?);
        assert_job_completed(&runtime, job_id).await?;
        let (parent_id, conversation): (i64, i64) = sqlx::query_as(
            "SELECT id, conversation_id FROM statuses WHERE uri = $1 AND account_id = $2 AND NOT local"
        ).bind(PARENT).bind(BOB).fetch_one(&runtime).await?;
        assert_eq!(sqlx::query_as::<_, (bool, i64, i64, i64)>(
            "SELECT reply, in_reply_to_id, in_reply_to_account_id, conversation_id FROM statuses WHERE id = $1"
        ).bind(child).fetch_one(&runtime).await?, (true, parent_id, BOB, conversation));
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT replies_count FROM status_stats WHERE status_id = $1"
        ).bind(parent_id).fetch_one(&runtime).await?, 1);
        let emojis: Vec<(i64, String, String, String, bool, bool)> = sqlx::query_as(
            "SELECT id, shortcode, uri, image_remote_url, disabled, visible_in_picker
             FROM custom_emojis WHERE domain = $1 AND shortcode IN ('reply_grants_new', 'reply_grants_existing')
             ORDER BY shortcode"
        ).bind(DOMAIN).fetch_all(&runtime).await?;
        assert_eq!(emojis.len(), 2);
        assert_eq!(emojis[0], (existing, "reply_grants_existing".into(),
            "https://remote.fixture.invalid/emojis/existing-v2".into(), emoji_url("existing-v2"), true, false));
        assert_eq!((&emojis[1].1, &emojis[1].2, &emojis[1].3, emojis[1].4, emojis[1].5),
            (&"reply_grants_new".into(), &"https://remote.fixture.invalid/emojis/new".into(), &emoji_url("new"), false, true));
        assert!(sqlx::query_scalar::<_, bool>(
            "SELECT created_at = '2020-01-01'::timestamp FROM custom_emojis WHERE id = $1"
        ).bind(existing).fetch_one(&runtime).await?);
        let outbox: Vec<Value> = sqlx::query_scalar(
            "SELECT payload->'arguments' FROM rustodon.outbox_events WHERE kind = $1 ORDER BY id"
        ).bind(ACTIVITYPUB_EMOJI_FETCH_JOB_KIND).fetch_all(&runtime).await?;
        assert_eq!(outbox.len(), 2);
        for (id, _, _, url, _, _) in &emojis {
            assert!(outbox.contains(&json!({"emoji_id": id, "remote_url": url,
                "media_type": "image/gif", "domain": DOMAIN})));
        }
        let body = fs::read("target/mastodon-v4.6.5/spec/fixtures/files/attachment.gif")?;
        let media_server = tokio::spawn(fixture_media_server_for_retries(media_listener, body, 2));
        queue.dispatch_outbox(100).await?;
        for _ in 0..2 {
            assert!(executor.process_one("emoji-media-writer", &[Lane::Pull], Duration::seconds(30)).await?);
        }
        assert_eq!(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.durable_jobs WHERE kind = $1"
        ).bind(ACTIVITYPUB_EMOJI_FETCH_JOB_KIND).fetch_one(&runtime).await?, 0);
        tokio::time::timeout(std::time::Duration::from_secs(5), media_server).await???;
        for (id, _, _, _, disabled, visible) in emojis {
            let (file_name, content_type, size, schema, installed, flags):
                (String, String, i64, i32, bool, bool) = sqlx::query_as(
                "SELECT image_file_name, image_content_type, image_file_size::bigint,
                    image_storage_schema_version, image_updated_at IS NOT NULL,
                    disabled = $2 AND visible_in_picker = $3 FROM custom_emojis WHERE id = $1"
            ).bind(id).bind(disabled).bind(visible).fetch_one(&runtime).await?;
            assert!(size > 0 && installed && flags);
            assert_eq!(schema, 1);
            assert_eq!(content_type, "image/gif");
            let metadata = PaperclipMetadata { attachment: PaperclipAttachment::CustomEmojiImage,
                id, remote: true, storage_schema_version: Some(1), file_name,
                content_type: Some(content_type), variant: None };
            for style in ["original", "static"] {
                assert!(media_root.open_file(Path::new(&metadata.relative_path(style).unwrap())).is_ok());
            }
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }.await;
    server.abort();
    // Owner is only fixture setup/cleanup; every production write above uses the dedicated writer.
    sqlx::query("DELETE FROM statuses WHERE uri IN ($1, $2)")
        .bind(CHILD)
        .bind(PARENT)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM custom_emojis WHERE domain = $1 AND shortcode IN ('reply_grants_new', 'reply_grants_existing')")
        .bind(DOMAIN).execute(&owner).await?;
    sqlx::query(
        "UPDATE account_stats SET statuses_count = $2, last_status_at = $3 WHERE account_id = $1",
    )
    .bind(BOB)
    .bind(baseline.0)
    .bind(baseline.1)
    .execute(&owner)
    .await?;
    reset().await?;
    fs::remove_dir_all(&root_path)?;
    operation
}

async fn assert_job_completed(pool: &sqlx::PgPool, id: i64) -> Result<(), sqlx::Error> {
    let remaining: Option<(i32, Option<String>)> =
        sqlx::query_as("SELECT attempts, last_error FROM rustodon.durable_jobs WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    assert!(
        remaining.is_none(),
        "job {id} did not complete: {remaining:?}"
    );
    Ok(())
}
