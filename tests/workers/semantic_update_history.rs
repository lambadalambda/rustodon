//! Real HTTP history projection around restricted-writer semantic Updates.
//! Pinned source: read-only /Users/lainsoykaf/repos/rustodon/.local-instance/
//! audit-reference/remaining (4.6.5 image 696439e1...230d0cf). See the two owned
//! issues for line-level evidence and the separate remote-history storage gap.
use super::*;

const DOMAIN: &str = "fixture-v4-6-5.rustodon.invalid";
const VIEWER_TOKEN: &str = "fixture-bearer-api-moderator-v4-6-5";
// seed.sql: matrix-viewer token -> user 107 -> account -323.
const OUTSIDER: i64 = -323;
const OUTSIDER_TOKEN: &str = "fixture-bearer-matrix-viewer-v4-6-5";
const WARNING: &str = "Meaningful history control";

enum Case {
    Noop { prior_edit: bool, stored: bool },
    Privacy(i32),
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn unedited_noop_history_fallback_reconciles_sensitivity_without_a_version() -> TestResult {
    run(Case::Noop {
        prior_edit: false,
        stored: false,
    })
    .await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn edited_noop_history_fallback_preserves_meaningful_timestamp_and_replay() -> TestResult {
    run(Case::Noop {
        prior_edit: true,
        stored: false,
    })
    .await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn noop_preserves_seeded_history_rows_and_rest_snapshots() -> TestResult {
    run(Case::Noop {
        prior_edit: true,
        stored: true,
    })
    .await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn private_noop_history_authorizes_stored_rows_and_fallback() -> TestResult {
    run(Case::Privacy(2)).await
}

#[tokio::test]
#[ignore = "requires the disposable worker PostgreSQL fixture and restricted writer"]
async fn direct_noop_history_authorizes_stored_rows_and_fallback() -> TestResult {
    run(Case::Privacy(3)).await
}

async fn run(case: Case) -> TestResult {
    let fixture = Fixture::new().await?;
    let result = std::panic::AssertUnwindSafe(async {
        // The guard aborts the loopback server on errors AND assertion panics.
        let api = HistoryApi::new(&fixture).await?;
        match case {
            Case::Noop { prior_edit, stored } => noop(&fixture, &api, prior_edit, stored).await,
            Case::Privacy(visibility) => privacy(&fixture, &api, visibility).await,
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

async fn noop(fixture: &Fixture, api: &HistoryApi, prior_edit: bool, stored: bool) -> TestResult {
    assert!(fixture.history_rows().await?.is_empty());
    assert_entries(&api.allowed().await?, &[(PUBLISHED, "", false)]);
    let warning = if prior_edit { WARNING } else { "" };
    let version = if prior_edit { FIRST_EDIT } else { PUBLISHED };
    if prior_edit {
        meaningful_control(fixture, api).await?;
    }
    if stored {
        seed_history(fixture, true).await?;
    }
    let rows = fixture.history_rows().await?;
    assert_eq!(rows.len(), if stored { 2 } else { 0 });
    let entries = if stored {
        vec![(PUBLISHED, "", false), (FIRST_EDIT, WARNING, false)]
    } else {
        vec![(version, warning, false)]
    };
    assert_entries(&api.allowed().await?, &entries);
    let mut expected = fixture.state().await?;
    let mut update = note(Some(NEWER));
    update["content"] = json!(EQUIVALENT);
    update["summary"] = json!(warning);
    update["sensitive"] = json!(true);
    assert_ne!(CONTENT, EQUIVALENT);
    assert_eq!(render(EQUIVALENT), CONTENT);
    expected.sensitive = true;
    for suffix in ["history-noop", "history-noop-replay"] {
        fixture.send(&update, suffix).await?;
        assert_eq!(fixture.state().await?, expected, "{suffix}");
        assert_eq!(
            fixture.history_rows().await?,
            rows,
            "no history insert/update/delete"
        );
        // A fallback exposes current sensitivity. Stored snapshots retain their
        // saved sensitivity; neither path acquires the no-op's NEWER timestamp.
        let expected_entries = if stored {
            entries.clone()
        } else {
            vec![(version, warning, true)]
        };
        assert_entries(&api.allowed().await?, &expected_entries);
    }
    Ok(())
}

async fn meaningful_control(fixture: &Fixture, api: &HistoryApi) -> TestResult {
    let mut update = note(Some(FIRST_EDIT));
    update["summary"] = json!(WARNING);
    fixture.send(&update, "history-real-edit").await?;
    fixture.assert_edit(FIRST_EDIT, 1).await?;
    let state = fixture.state().await?;
    assert_eq!(state.warning, WARNING);
    // CURRENT Rustodon limitation, NOT pinned history parity: remote edits do
    // not store snapshots. Keep storage implementation a separate topical scope.
    assert!(
        state.history_rows.is_empty(),
        "revisit fallback characterization when remote history storage lands"
    );
    assert_entries(&api.allowed().await?, &[(FIRST_EDIT, WARNING, false)]);
    fixture.send(&update, "history-real-edit-replay").await?;
    assert_eq!(fixture.state().await?, state);
    assert_entries(&api.allowed().await?, &[(FIRST_EDIT, WARNING, false)]);
    Ok(())
}

async fn privacy(fixture: &Fixture, api: &HistoryApi, visibility: i32) -> TestResult {
    // Prove the outsider token is valid and has no follower/mention grant; a
    // 401, unrelated block, or accidental follower must not masquerade as 404.
    assert_eq!(api.get(Some(OUTSIDER_TOKEN)).await?.0, http::StatusCode::OK);
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM follows WHERE account_id = $1 AND target_account_id = $2)
         OR EXISTS (SELECT 1 FROM mentions WHERE account_id = $1 AND status_id = $3)",
        )
        .bind(OUTSIDER)
        .bind(BOB)
        .bind(fixture.status_id)
        .fetch_one(&fixture.owner)
        .await?
    );
    // Task-owned row only: this is history authorization setup, not an AP
    // visibility change (remote Updates must not widen the original audience).
    sqlx::query("UPDATE statuses SET visibility = $2 WHERE id = $1")
        .bind(fixture.status_id)
        .bind(visibility)
        .execute(&fixture.owner)
        .await?;
    for stored in [false, true] {
        if stored {
            seed_history(fixture, false).await?;
        }
        let before = fixture.state().await?;
        assert_eq!(before.history_rows.len(), usize::from(stored));
        let mut update = note(Some(NEWER));
        update["content"] = json!(EQUIVALENT);
        update["to"] = if visibility == 2 {
            json!(["https://remote.fixture.invalid/users/bob/followers"])
        } else {
            json!([RECIPIENT_URI])
        };
        update["cc"] = json!([]);
        for stage in ["before", "noop", "replay"] {
            if stage != "before" {
                fixture
                    .send(&update, &format!("history-private-{stored}-{stage}"))
                    .await?;
            }
            assert_eq!(fixture.state().await?, before);
            assert_entries(&api.allowed().await?, &[(PUBLISHED, "", false)]);
            for token in [None, Some(OUTSIDER_TOKEN)] {
                let (code, body) = api.get(token).await?;
                assert_eq!(
                    code,
                    http::StatusCode::NOT_FOUND,
                    "{visibility}/{stored}/{stage}"
                );
                assert!(
                    body.get("error").is_some(),
                    "must return the real REST error"
                );
                assert!(
                    !body.to_string().contains("Some content"),
                    "history content leaked"
                );
            }
        }
    }
    Ok(())
}

// Explicit owner-only setup for a pre-existing persisted history. Do not fake
// production insertion or claim the meaningful remote Update created these rows.
async fn seed_history(fixture: &Fixture, edited: bool) -> TestResult {
    assert!(fixture.history_rows().await?.is_empty());
    let mut transaction = fixture.owner.begin().await?;
    let entries = if edited {
        vec![(PUBLISHED, ""), (FIRST_EDIT, WARNING)]
    } else {
        vec![(PUBLISHED, "")]
    };
    for (version, warning) in entries {
        sqlx::query(
            "INSERT INTO status_edits (status_id, account_id, text, spoiler_text, sensitive,
                ordered_media_attachment_ids, created_at, updated_at)
             VALUES ($1, $2, $3, $4, false, '{}'::bigint[], $5, $5)",
        )
        .bind(fixture.status_id)
        .bind(BOB)
        .bind(CONTENT)
        .bind(warning)
        .bind(timestamp(version))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

fn assert_entries(body: &Value, expected: &[(&str, &str, bool)]) {
    let entries = body.as_array().expect("real REST history array");
    assert_eq!(
        entries.len(),
        expected.len(),
        "no fabricated history version: {body}"
    );
    for (entry, (version, warning, sensitive)) in entries.iter().zip(expected) {
        assert_eq!(entry["content"], CONTENT);
        assert_eq!(entry["spoiler_text"], *warning);
        assert_eq!(entry["sensitive"], *sensitive);
        assert_eq!(
            timestamp(entry["created_at"].as_str().unwrap()),
            timestamp(version)
        );
        assert_eq!(entry["account"]["id"], BOB.to_string());
        assert_eq!(entry["media_attachments"], json!([]));
    }
}

struct HistoryApi {
    client: reqwest::Client,
    url: String,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl HistoryApi {
    async fn new(fixture: &Fixture) -> TestResult<Self> {
        let state = rustodon::web::WebState::new(
            Repository::from_pool(fixture.queue.pool().clone()),
            Url::parse(ORIGIN)?,
            DOMAIN,
            "/system",
            std::env::temp_dir(),
            InstanceRuntimeConfig {
                domain: DOMAIN.to_owned(),
                version: "4.6.5".to_owned(),
                source_url: String::new(),
                streaming_api: String::new(),
                vapid_public_key: None,
                thumbnail_url: String::new(),
                thumbnail_description: String::new(),
                thumbnail_blurhash: None,
                thumbnail_versions: None,
                icons: Vec::new(),
                languages: vec!["en".to_owned()],
                active_month: 0,
                active_halfyear: 0,
                translation_enabled: false,
                limited_federation: false,
                single_user_mode: false,
                terms_of_service_url: None,
                sso_signup_url: None,
                wrapstodon: None,
            },
            Vec::new(),
            vec![DOMAIN.to_owned()],
        )?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!(
            "http://{}/api/v1/statuses/{}/history",
            listener.local_addr()?,
            fixture.status_id
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        let server =
            tokio::spawn(async move { axum::serve(listener, rustodon::web::router(state)).await });
        Ok(Self {
            client,
            url,
            server,
        })
    }

    async fn get(&self, token: Option<&str>) -> TestResult<(http::StatusCode, Value)> {
        let mut request = self.client.get(&self.url).header(HOST, DOMAIN);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        let code = response.status();
        Ok((code, serde_json::from_slice(&response.bytes().await?)?))
    }

    async fn allowed(&self) -> TestResult<Value> {
        let (code, body) = self.get(Some(VIEWER_TOKEN)).await?;
        assert_eq!(
            code,
            http::StatusCode::OK,
            "recipient history access: {body}"
        );
        Ok(body)
    }
}

impl Drop for HistoryApi {
    fn drop(&mut self) {
        self.server.abort();
    }
}
