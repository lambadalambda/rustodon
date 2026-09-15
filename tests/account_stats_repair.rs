use std::error::Error;

use http::HeaderMap;
use http::header::{AUTHORIZATION, HeaderValue};
use rustodon::mastodon::rest::RestProjectionLoader;
use rustodon::mastodon::{
    BearerAuthenticator, Repository, WRITE_FOLLOWS, WRITE_STATUSES, WriteRepository,
};
use serde_json::Value;

const ALICE: i64 = 116_844_606_259_201_001;
const CAROL: i64 = 116_844_606_259_202_002;
const FOLLOW_TARGET: i64 = -323;

macro_rules! check_eq {
    ($left:expr, $right:expr $(,)?) => {{
        let left = $left;
        let right = $right;
        if left != right {
            return Err(format!("values differ: left={left:?}, right={right:?}").into());
        }
    }};
    ($left:expr, $right:expr, $message:expr $(,)?) => {{
        let left = $left;
        let right = $right;
        if left != right {
            return Err(format!("{}: left={left:?}, right={right:?}", $message).into());
        }
    }};
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn missing_account_stats_are_reconciled_without_touching_existing_rows()
-> Result<(), Box<dyn Error>> {
    let reader_url = std::env::var("RUSTODON_MASTODON_DATABASE_URL")
        .expect("the fixture task must provide RUSTODON_MASTODON_DATABASE_URL");
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer_url = std::env::var("RUSTODON_MASTODON_WRITER_DATABASE_URL")
        .expect("the fixture task must provide RUSTODON_MASTODON_WRITER_DATABASE_URL");
    let owner = sqlx::PgPool::connect(&owner_url).await?;
    let writer = WriteRepository::connect(&writer_url).await?;
    let repository = Repository::connect(&reader_url).await?;
    let loader =
        RestProjectionLoader::new(repository.clone(), None, "fixture-v4-6-5.rustodon.invalid");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticator = BearerAuthenticator::new(repository.clone());
    let status_writer = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let follow_writer = authenticator.authenticate(&headers, WRITE_FOLLOWS).await?;

    let saved_alice: Value =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&owner)
            .await?;
    let saved_carol: Value =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(CAROL)
            .fetch_one(&owner)
            .await?;
    let saved_follow_target: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(FOLLOW_TARGET)
            .fetch_optional(&owner)
            .await?;
    let missing_account_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT account.id FROM accounts account \
         LEFT JOIN account_stats stats ON stats.account_id = account.id \
         WHERE stats.id IS NULL ORDER BY account.id",
    )
    .fetch_all(&owner)
    .await?;
    let expected: (i64, i64, i64, Option<chrono::NaiveDateTime>) = sqlx::query_as(
        "SELECT statuses_count, following_count, followers_count, last_status_at \
         FROM account_stats WHERE account_id = $1",
    )
    .bind(ALICE)
    .fetch_one(&owner)
    .await?;
    let expected_instance_statuses: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(COALESCE(stats.statuses_count, ( \
             SELECT count(*) FROM statuses status \
             WHERE status.account_id = account.id AND status.deleted_at IS NULL \
               AND status.visibility <> 3))), 0) \
         FROM accounts account \
         LEFT JOIN account_stats stats ON stats.account_id = account.id \
         WHERE account.domain IS NULL",
    )
    .fetch_one(&owner)
    .await?;

    let mut created_status_id = None;
    let mut created_follow_id = None;
    let result = async {
        sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
            .bind(ALICE)
            .execute(&owner)
            .await?;

        check_eq!(
            writer.repair_missing_account_stats().await?,
            u64::try_from(missing_account_ids.len())? + 1,
        );
        let repaired = repository
            .account_stat(ALICE)
            .await?
            .ok_or_else(|| std::io::Error::other("the missing stats row should be repaired"))?;
        check_eq!(
            (
                repaired.statuses_count,
                repaired.following_count,
                repaired.followers_count,
                repaired.last_status_at,
            ),
            expected,
        );

        let account = loader.account(ALICE).await?.ok_or_else(|| {
            std::io::Error::other("the repaired account should remain REST-visible")
        })?;
        check_eq!(account.statuses_count, expected.0);
        check_eq!(account.following_count, expected.1);
        check_eq!(account.followers_count, expected.2);
        check_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(sum(stats.statuses_count), 0) FROM account_stats stats \
                 JOIN accounts account ON account.id = stats.account_id \
                 WHERE account.domain IS NULL",
            )
            .fetch_one(&owner)
            .await?,
            expected_instance_statuses,
        );

        check_eq!(
            sqlx::query_scalar::<_, Value>(
                "SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1",
            )
            .bind(CAROL)
            .fetch_one(&owner)
            .await?,
            saved_carol,
            "repairing missing rows must not rewrite an existing stats row",
        );
        let repaired_alice: Value = sqlx::query_scalar(
            "SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1",
        )
        .bind(ALICE)
        .fetch_one(&owner)
        .await?;
        check_eq!(writer.repair_missing_account_stats().await?, 0);
        check_eq!(
            sqlx::query_scalar::<_, Value>(
                "SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1",
            )
            .bind(ALICE)
            .fetch_one(&owner)
            .await?,
            repaired_alice,
            "a second repair pass must not reset or double-count the repaired row",
        );

        sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
            .bind(ALICE)
            .execute(&owner)
            .await?;
        check_eq!(
            writer.repair_missing_account_stats_startup_batch().await?,
            (1, false),
            "the synchronous startup pass must be bounded and repair the remaining gap",
        );
        check_eq!(
            repository
                .account_stat(ALICE)
                .await?
                .ok_or_else(|| std::io::Error::other("startup repair should restore Alice"))?
                .statuses_count,
            expected.0,
        );

        sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
            .bind(ALICE)
            .execute(&owner)
            .await?;
        let status_id = writer
            .create_status(
                &status_writer,
                "missing account stats repair regression",
                &[],
                None,
                Some(false),
                Some("public"),
                None,
                None,
                None,
            )
            .await?
            .status_id;
        created_status_id = Some(status_id);
        check_eq!(
            repository
                .account_stat(ALICE)
                .await?
                .ok_or_else(|| std::io::Error::other("Alice's stats row should exist"))?
                .statuses_count,
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM statuses \
                 WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3",
            )
            .bind(ALICE)
            .fetch_one(&owner)
            .await?,
        );
        writer
            .delete_status(&status_writer, status_id, false)
            .await?;
        check_eq!(
            repository
                .account_stat(ALICE)
                .await?
                .ok_or_else(|| std::io::Error::other("Alice's stats row should exist"))?
                .statuses_count,
            expected.0,
        );

        check_eq!(
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM follows \
             WHERE account_id = $1 AND target_account_id = $2)",
            )
            .bind(ALICE)
            .bind(FOLLOW_TARGET)
            .fetch_one(&owner)
            .await?,
            false,
            "the fixture must not already contain the test follow",
        );
        sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
            .bind(vec![ALICE, FOLLOW_TARGET])
            .execute(&owner)
            .await?;
        let follow = writer
            .set_follow(&follow_writer, FOLLOW_TARGET, true, None, None, None)
            .await?;
        created_follow_id = follow.activity_id;
        let relationship_counts: (i64, i64) = sqlx::query_as(
            "SELECT \
               (SELECT following_count FROM account_stats WHERE account_id = $1), \
               (SELECT followers_count FROM account_stats WHERE account_id = $2)",
        )
        .bind(ALICE)
        .bind(FOLLOW_TARGET)
        .fetch_one(&owner)
        .await?;
        let live_relationship_counts: (i64, i64) = sqlx::query_as(
            "SELECT \
               (SELECT count(*) FROM follows WHERE account_id = $1), \
               (SELECT count(*) FROM follows WHERE target_account_id = $2)",
        )
        .bind(ALICE)
        .bind(FOLLOW_TARGET)
        .fetch_one(&owner)
        .await?;
        check_eq!(relationship_counts, live_relationship_counts);
        writer
            .set_follow(&follow_writer, FOLLOW_TARGET, false, None, None, None)
            .await?;
        check_eq!(
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT \
                   (SELECT following_count FROM account_stats WHERE account_id = $1), \
                   (SELECT followers_count FROM account_stats WHERE account_id = $2)",
            )
            .bind(ALICE)
            .bind(FOLLOW_TARGET)
            .fetch_one(&owner)
            .await?,
            (
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM follows WHERE account_id = $1",)
                    .bind(ALICE)
                    .fetch_one(&owner)
                    .await?,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM follows WHERE target_account_id = $1",
                )
                .bind(FOLLOW_TARGET)
                .fetch_one(&owner)
                .await?,
            ),
        );
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    if let Some(follow_id) = created_follow_id {
        sqlx::query(
            "DELETE FROM rustodon.outbox_events \
             WHERE payload -> 'arguments' ->> 'activity_id' = $1",
        )
        .bind(follow_id.to_string())
        .execute(&owner)
        .await?;
    }
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(FOLLOW_TARGET)
        .execute(&owner)
        .await?;
    if let Some(status_id) = created_status_id {
        sqlx::query(
            "DELETE FROM rustodon.outbox_events \
             WHERE payload -> 'arguments' ->> 'status_id' = $1 \
                OR payload ->> 'object_id' = $1",
        )
        .bind(status_id.to_string())
        .execute(&owner)
        .await?;
        sqlx::query("DELETE FROM status_stats WHERE status_id = $1")
            .bind(status_id)
            .execute(&owner)
            .await?;
        sqlx::query("DELETE FROM conversations WHERE parent_status_id = $1")
            .bind(status_id)
            .execute(&owner)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id = $1")
            .bind(status_id)
            .execute(&owner)
            .await?;
    }
    let mut repaired_account_ids = missing_account_ids;
    repaired_account_ids.extend([ALICE, FOLLOW_TARGET]);
    sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
        .bind(repaired_account_ids)
        .execute(&owner)
        .await?;
    for saved_stats in [Some(saved_alice), saved_follow_target]
        .into_iter()
        .flatten()
    {
        sqlx::query(
            "INSERT INTO account_stats \
             SELECT (jsonb_populate_record(NULL::account_stats, $1)).*",
        )
        .bind(saved_stats)
        .execute(&owner)
        .await?;
    }
    result?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn activitypub_outbox_missing_stats_fallback_excludes_direct_statuses()
-> Result<(), Box<dyn Error>> {
    let reader_url = std::env::var("RUSTODON_MASTODON_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let owner = sqlx::PgPool::connect(&owner_url).await?;
    let repository = Repository::connect(&reader_url).await?;
    let saved_stats: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_optional(&owner)
            .await?;
    let initially_missing: Vec<i64> = sqlx::query_scalar(
        "SELECT account.id FROM accounts account \
         LEFT JOIN account_stats stats ON stats.account_id = account.id \
         WHERE stats.id IS NULL ORDER BY account.id",
    )
    .fetch_all(&owner)
    .await?;
    let mut direct_status_id = None;

    let result = async {
        let status_id: i64 = sqlx::query_scalar(
            "INSERT INTO statuses (account_id, text, spoiler_text, visibility, local, sensitive, \
                                   reply, created_at, updated_at) \
             VALUES ($1, 'account stats direct fallback probe', '', 3, true, false, false, \
                     clock_timestamp(), clock_timestamp()) RETURNING id",
        )
        .bind(ALICE)
        .fetch_one(&owner)
        .await?;
        direct_status_id = Some(status_id);
        sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
            .bind(ALICE)
            .execute(&owner)
            .await?;

        let compatible_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM statuses \
             WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3",
        )
        .bind(ALICE)
        .fetch_one(&owner)
        .await?;
        let count_including_direct: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM statuses WHERE account_id = $1 AND deleted_at IS NULL",
        )
        .bind(ALICE)
        .fetch_one(&owner)
        .await?;
        check_eq!(count_including_direct, compatible_count + 1);
        check_eq!(
            repository.activitypub_outbox_count_for_test(ALICE).await?,
            compatible_count,
            "missing-stats outbox fallback must match repaired counter visibility rules",
        );
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    if let Some(status_id) = direct_status_id {
        sqlx::query("DELETE FROM statuses WHERE id = $1")
            .bind(status_id)
            .execute(&owner)
            .await?;
    }
    sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
        .bind({
            let mut ids = initially_missing;
            ids.push(ALICE);
            ids
        })
        .execute(&owner)
        .await?;
    if let Some(stats) = saved_stats {
        sqlx::query(
            "INSERT INTO account_stats \
             SELECT (jsonb_populate_record(NULL::account_stats, $1)).*",
        )
        .bind(stats)
        .execute(&owner)
        .await?;
    }
    result?;
    Ok(())
}

async fn relationship_counts_match_live(
    pool: &sqlx::PgPool,
    account_ids: &[i64],
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT NOT EXISTS ( \
           SELECT 1 FROM unnest($1::bigint[]) AS requested(account_id) \
           JOIN account_stats stats ON stats.account_id = requested.account_id \
           WHERE stats.following_count <> ( \
                   SELECT count(*) FROM follows \
                    WHERE follows.account_id = requested.account_id) \
              OR stats.followers_count <> ( \
                   SELECT count(*) FROM follows \
                    WHERE follows.target_account_id = requested.account_id) \
         ) AND (SELECT count(*) FROM account_stats WHERE account_id = ANY($1)) = cardinality($1)",
    )
    .bind(account_ids)
    .fetch_one(pool)
    .await
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn reciprocal_local_follows_preheal_and_serialize_before_source_locks()
-> Result<(), Box<dyn Error>> {
    const SECOND_TOKEN: &str = "fixture-bearer-api-moderator-v4-6-5";

    let reader_url = std::env::var("RUSTODON_MASTODON_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let writer_url = std::env::var("RUSTODON_MASTODON_WRITER_DATABASE_URL")?;
    let owner = sqlx::PgPool::connect(&owner_url).await?;
    let repository = Repository::connect(&reader_url).await?;
    let writer = WriteRepository::connect(&writer_url)
        .await?
        .with_relationship_write_barrier_for_test(2);
    let second_account_id: i64 = sqlx::query_scalar(
        "SELECT account_user.account_id FROM oauth_access_tokens token \
         JOIN users account_user ON account_user.id = token.resource_owner_id \
         JOIN accounts account ON account.id = account_user.account_id \
         WHERE token.token = $1 AND account.domain IS NULL",
    )
    .bind(SECOND_TOKEN)
    .fetch_one(&owner)
    .await?;
    let saved_scopes: String =
        sqlx::query_scalar("SELECT scopes FROM oauth_access_tokens WHERE token = $1")
            .bind(SECOND_TOKEN)
            .fetch_one(&owner)
            .await?;
    let saved_account_states: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('id', id, 'locked', locked, 'silenced_at', silenced_at) \
         FROM accounts WHERE id = ANY($1) ORDER BY id",
    )
    .bind(vec![ALICE, second_account_id])
    .fetch_all(&owner)
    .await?;
    let saved_stats: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(stats) FROM account_stats stats \
         WHERE account_id = ANY($1) ORDER BY account_id",
    )
    .bind(vec![ALICE, second_account_id])
    .fetch_all(&owner)
    .await?;
    let initially_missing: Vec<i64> = sqlx::query_scalar(
        "SELECT account.id FROM accounts account \
         LEFT JOIN account_stats stats ON stats.account_id = account.id \
         WHERE stats.id IS NULL ORDER BY account.id",
    )
    .fetch_all(&owner)
    .await?;
    let saved_follows: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(follow_row) FROM follows follow_row \
         WHERE (account_id = $1 AND target_account_id = $2) \
            OR (account_id = $2 AND target_account_id = $1) ORDER BY id",
    )
    .bind(ALICE)
    .bind(second_account_id)
    .fetch_all(&owner)
    .await?;
    let saved_requests: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(request) FROM follow_requests request \
         WHERE (account_id = $1 AND target_account_id = $2) \
            OR (account_id = $2 AND target_account_id = $1) ORDER BY id",
    )
    .bind(ALICE)
    .bind(second_account_id)
    .fetch_all(&owner)
    .await?;
    let saved_blocks: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(block_row) FROM blocks block_row \
         WHERE (account_id = $1 AND target_account_id = $2) \
            OR (account_id = $2 AND target_account_id = $1) ORDER BY id",
    )
    .bind(ALICE)
    .bind(second_account_id)
    .fetch_all(&owner)
    .await?;

    let result = async {
        sqlx::query(
            "UPDATE oauth_access_tokens SET scopes = scopes || ' write:follows' WHERE token = $1",
        )
        .bind(SECOND_TOKEN)
        .execute(&owner)
        .await?;
        sqlx::query("UPDATE accounts SET locked = false, silenced_at = NULL WHERE id = ANY($1)")
            .bind(vec![ALICE, second_account_id])
            .execute(&owner)
            .await?;
        for table in ["follows", "follow_requests", "blocks"] {
            let query = format!(
                "DELETE FROM {table} WHERE (account_id = $1 AND target_account_id = $2) \
                 OR (account_id = $2 AND target_account_id = $1)"
            );
            sqlx::query(&query)
                .bind(ALICE)
                .bind(second_account_id)
                .execute(&owner)
                .await?;
        }
        sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
            .bind(vec![ALICE, second_account_id])
            .execute(&owner)
            .await?;

        let mut alice_headers = HeaderMap::new();
        alice_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let alice = BearerAuthenticator::new(repository.clone())
            .authenticate(&alice_headers, WRITE_FOLLOWS)
            .await?;
        let mut second_headers = HeaderMap::new();
        second_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-api-moderator-v4-6-5"),
        );
        let second = BearerAuthenticator::new(repository.clone())
            .authenticate(&second_headers, WRITE_FOLLOWS)
            .await?;

        let (alice_follow, second_follow) =
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(
                    writer.set_follow(&alice, second_account_id, true, None, None, None),
                    writer.set_follow(&second, ALICE, true, None, None, None),
                )
            })
            .await?;
        let alice_follow = alice_follow?;
        let second_follow = second_follow?;
        check_eq!(alice_follow.request, false);
        check_eq!(second_follow.request, false);
        check_eq!(
            relationship_counts_match_live(&owner, &[ALICE, second_account_id]).await?,
            true,
            "reciprocal local writes must finish with exact counters",
        );
        Ok::<Vec<i64>, Box<dyn Error>>(
            [alice_follow.activity_id, second_follow.activity_id]
                .into_iter()
                .flatten()
                .collect(),
        )
    }
    .await;

    let created_ids = result.as_ref().map_or(&[][..], Vec::as_slice);
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
         WHERE payload -> 'arguments' ->> 'activity_id' = ANY($1)",
    )
    .bind(
        created_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    )
    .execute(&owner)
    .await?;
    for table in ["follows", "follow_requests", "blocks"] {
        let query = format!(
            "DELETE FROM {table} WHERE (account_id = $1 AND target_account_id = $2) \
             OR (account_id = $2 AND target_account_id = $1)"
        );
        sqlx::query(&query)
            .bind(ALICE)
            .bind(second_account_id)
            .execute(&owner)
            .await?;
    }
    for row in saved_follows {
        sqlx::query("INSERT INTO follows SELECT (jsonb_populate_record(NULL::follows, $1)).*")
            .bind(row)
            .execute(&owner)
            .await?;
    }
    for row in saved_requests {
        sqlx::query(
            "INSERT INTO follow_requests \
             SELECT (jsonb_populate_record(NULL::follow_requests, $1)).*",
        )
        .bind(row)
        .execute(&owner)
        .await?;
    }
    for row in saved_blocks {
        sqlx::query("INSERT INTO blocks SELECT (jsonb_populate_record(NULL::blocks, $1)).*")
            .bind(row)
            .execute(&owner)
            .await?;
    }
    sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
        .bind({
            let mut ids = initially_missing;
            ids.extend([ALICE, second_account_id]);
            ids
        })
        .execute(&owner)
        .await?;
    for stats in saved_stats {
        sqlx::query(
            "INSERT INTO account_stats \
             SELECT (jsonb_populate_record(NULL::account_stats, $1)).*",
        )
        .bind(stats)
        .execute(&owner)
        .await?;
    }
    for state in saved_account_states {
        sqlx::query(
            "UPDATE accounts SET locked = ($1 ->> 'locked')::boolean, \
             silenced_at = ($1 ->> 'silenced_at')::timestamp WHERE id = ($1 ->> 'id')::bigint",
        )
        .bind(state)
        .execute(&owner)
        .await?;
    }
    sqlx::query("UPDATE oauth_access_tokens SET scopes = $2 WHERE token = $1")
        .bind(SECOND_TOKEN)
        .bind(saved_scopes)
        .execute(&owner)
        .await?;
    result?;
    Ok(())
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn inbound_relationship_paths_self_heal_atomically() -> Result<(), Box<dyn Error>> {
    const REMOTE_ACCOUNT: i64 = -331;
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const ALICE_URI: &str = "https://fixture-v4-6-5.rustodon.invalid/users/alice";
    const FOLLOW_URI: &str = "https://remote.fixture.invalid/activities/stats-repair-follow";
    const BLOCK_URI: &str = "https://remote.fixture.invalid/activities/stats-repair-block";
    const ACCEPT_URI: &str =
        "https://fixture-v4-6-5.rustodon.invalid/activities/stats-repair-accept";

    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let writer_url = std::env::var("RUSTODON_MASTODON_WRITER_DATABASE_URL")?;
    let owner = sqlx::PgPool::connect(&owner_url).await?;
    let writer = WriteRepository::connect(&writer_url).await?;
    let remote_uri: String =
        sqlx::query_scalar("SELECT uri FROM accounts WHERE id = $1 AND domain IS NOT NULL")
            .bind(REMOTE_ACCOUNT)
            .fetch_one(&owner)
            .await?;
    let saved_locked: bool = sqlx::query_scalar("SELECT locked FROM accounts WHERE id = $1")
        .bind(ALICE)
        .fetch_one(&owner)
        .await?;
    let saved_remote_silenced_at: Option<chrono::NaiveDateTime> =
        sqlx::query_scalar("SELECT silenced_at FROM accounts WHERE id = $1")
            .bind(REMOTE_ACCOUNT)
            .fetch_one(&owner)
            .await?;
    let saved_stats: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(stats) FROM account_stats stats \
         WHERE account_id = ANY($1) ORDER BY account_id",
    )
    .bind(vec![ALICE, REMOTE_ACCOUNT])
    .fetch_all(&owner)
    .await?;
    let saved_blocks: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(block_row) FROM blocks block_row \
         WHERE (account_id = $1 AND target_account_id = $2) \
            OR (account_id = $2 AND target_account_id = $1) ORDER BY id",
    )
    .bind(ALICE)
    .bind(REMOTE_ACCOUNT)
    .fetch_all(&owner)
    .await?;
    let initially_missing: Vec<i64> = sqlx::query_scalar(
        "SELECT account.id FROM accounts account \
         LEFT JOIN account_stats stats ON stats.account_id = account.id \
         WHERE stats.id IS NULL ORDER BY account.id",
    )
    .fetch_all(&owner)
    .await?;

    let result = async {
        sqlx::query("UPDATE accounts SET locked = false WHERE id = $1")
            .bind(ALICE)
            .execute(&owner)
            .await?;
        sqlx::query("UPDATE accounts SET silenced_at = NULL WHERE id = $1")
            .bind(REMOTE_ACCOUNT)
            .execute(&owner)
            .await?;
        sqlx::query(
            "DELETE FROM follows WHERE (account_id = $1 AND target_account_id = $2) \
               OR (account_id = $2 AND target_account_id = $1)",
        )
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&owner)
        .await?;
        sqlx::query(
            "DELETE FROM follow_requests WHERE (account_id = $1 AND target_account_id = $2) \
               OR (account_id = $2 AND target_account_id = $1)",
        )
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&owner)
        .await?;
        sqlx::query(
            "DELETE FROM blocks WHERE (account_id = $1 AND target_account_id = $2) \
               OR (account_id = $2 AND target_account_id = $1)",
        )
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .execute(&owner)
        .await?;
        sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
            .bind(vec![ALICE, REMOTE_ACCOUNT])
            .execute(&owner)
            .await?;

        let follow = writer
            .apply_remote_follow_for_test(
                REMOTE_ACCOUNT,
                FOLLOW_URI,
                ALICE_URI,
                ORIGIN,
                Some(ALICE),
            )
            .await?
            .ok_or_else(|| std::io::Error::other("remote Follow was ignored"))?;
        let follow_id = match follow {
            rustodon::mastodon::RemoteFollowOutcome::Applied(outcome) if !outcome.request => {
                outcome.activity_id
            }
            rustodon::mastodon::RemoteFollowOutcome::Applied(_) => {
                return Err(
                    std::io::Error::other("remote Follow unexpectedly became a request").into(),
                );
            }
            rustodon::mastodon::RemoteFollowOutcome::Rejected { .. } => {
                return Err(std::io::Error::other("remote Follow was rejected").into());
            }
        };
        check_eq!(
            relationship_counts_match_live(&owner, &[ALICE, REMOTE_ACCOUNT]).await?,
            true,
            "inbound Follow must heal exact counters after inserting the relationship",
        );

        sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
            .bind(vec![ALICE, REMOTE_ACCOUNT])
            .execute(&owner)
            .await?;
        writer
            .apply_remote_undo_follow_for_test(
                REMOTE_ACCOUNT,
                FOLLOW_URI,
                Some(ALICE_URI),
                ORIGIN,
                Some(ALICE),
            )
            .await?;
        check_eq!(
            relationship_counts_match_live(&owner, &[ALICE, REMOTE_ACCOUNT]).await?,
            true,
            "inbound Undo Follow must snapshot the post-delete state",
        );

        sqlx::query(
            "INSERT INTO follow_requests (account_id, target_account_id, show_reblogs, notify, \
                                          languages, uri, created_at, updated_at) \
             VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())",
        )
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .bind(ACCEPT_URI)
        .execute(&owner)
        .await?;
        sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
            .bind(vec![ALICE, REMOTE_ACCOUNT])
            .execute(&owner)
            .await?;
        writer
            .apply_remote_follow_decision_for_test(
                REMOTE_ACCOUNT,
                ACCEPT_URI,
                Some(&remote_uri),
                Some(ALICE_URI),
                true,
                ORIGIN,
                Some(ALICE),
            )
            .await?;
        check_eq!(
            relationship_counts_match_live(&owner, &[ALICE, REMOTE_ACCOUNT]).await?,
            true,
            "inbound Accept must heal both relationship counters exactly once",
        );
        let accepted_follow_id: i64 = sqlx::query_scalar(
            "SELECT id FROM follows WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(ALICE)
        .bind(REMOTE_ACCOUNT)
        .fetch_one(&owner)
        .await?;
        let reverse_follow_id: i64 = sqlx::query_scalar(
            "INSERT INTO follows (account_id, target_account_id, show_reblogs, notify, languages, \
                                  uri, created_at, updated_at) \
             VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp()) \
             RETURNING id",
        )
        .bind(REMOTE_ACCOUNT)
        .bind(ALICE)
        .bind("https://remote.fixture.invalid/activities/stats-repair-reverse-follow")
        .fetch_one(&owner)
        .await?;
        sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
            .bind(vec![ALICE, REMOTE_ACCOUNT])
            .execute(&owner)
            .await?;
        writer
            .apply_remote_block_for_test(REMOTE_ACCOUNT, BLOCK_URI, ALICE_URI, ORIGIN, Some(ALICE))
            .await?;
        check_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM follows \
                 WHERE (account_id = $1 AND target_account_id = $2) \
                    OR (account_id = $2 AND target_account_id = $1)",
            )
            .bind(ALICE)
            .bind(REMOTE_ACCOUNT)
            .fetch_one(&owner)
            .await?,
            0,
            "inbound Block must remove both follow directions",
        );
        check_eq!(
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM blocks \
                 WHERE account_id = $1 AND target_account_id = $2 AND uri = $3)",
            )
            .bind(REMOTE_ACCOUNT)
            .bind(ALICE)
            .bind(BLOCK_URI)
            .fetch_one(&owner)
            .await?,
            true,
            "inbound Block must persist the expected block row",
        );
        check_eq!(
            relationship_counts_match_live(&owner, &[ALICE, REMOTE_ACCOUNT]).await?,
            true,
            "inbound Block bulk removal must aggregate post-delete healing",
        );
        Ok::<[i64; 3], Box<dyn Error>>([follow_id, accepted_follow_id, reverse_follow_id])
    }
    .await;

    let created_ids = result.as_ref().map_or(Vec::new(), |ids| ids.to_vec());
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
         WHERE payload -> 'arguments' ->> 'activity_id' = ANY($1) \
            OR logical_key LIKE '%stats-repair%'",
    )
    .bind(
        created_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    )
    .execute(&owner)
    .await?;
    sqlx::query(
        "DELETE FROM follows WHERE (account_id = $1 AND target_account_id = $2) \
           OR (account_id = $2 AND target_account_id = $1)",
    )
    .bind(ALICE)
    .bind(REMOTE_ACCOUNT)
    .execute(&owner)
    .await?;
    sqlx::query(
        "DELETE FROM follow_requests WHERE (account_id = $1 AND target_account_id = $2) \
           OR (account_id = $2 AND target_account_id = $1)",
    )
    .bind(ALICE)
    .bind(REMOTE_ACCOUNT)
    .execute(&owner)
    .await?;
    sqlx::query(
        "DELETE FROM blocks WHERE (account_id = $1 AND target_account_id = $2) \
           OR (account_id = $2 AND target_account_id = $1)",
    )
    .bind(ALICE)
    .bind(REMOTE_ACCOUNT)
    .execute(&owner)
    .await?;
    for block in saved_blocks {
        sqlx::query("INSERT INTO blocks SELECT (jsonb_populate_record(NULL::blocks, $1)).*")
            .bind(block)
            .execute(&owner)
            .await?;
    }
    sqlx::query("DELETE FROM rustodon.idempotency_keys WHERE key LIKE '%stats-repair%'")
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
        .bind({
            let mut ids = initially_missing;
            ids.extend([ALICE, REMOTE_ACCOUNT]);
            ids
        })
        .execute(&owner)
        .await?;
    for stats in saved_stats {
        sqlx::query(
            "INSERT INTO account_stats \
             SELECT (jsonb_populate_record(NULL::account_stats, $1)).*",
        )
        .bind(stats)
        .execute(&owner)
        .await?;
    }
    sqlx::query("UPDATE accounts SET locked = $2 WHERE id = $1")
        .bind(ALICE)
        .bind(saved_locked)
        .execute(&owner)
        .await?;
    sqlx::query("UPDATE accounts SET silenced_at = $2 WHERE id = $1")
        .bind(REMOTE_ACCOUNT)
        .bind(saved_remote_silenced_at)
        .execute(&owner)
        .await?;
    result?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn cascading_status_delete_heals_every_reblogger_after_tombstoning()
-> Result<(), Box<dyn Error>> {
    let reader_url = std::env::var("RUSTODON_MASTODON_DATABASE_URL")?;
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")?;
    let writer_url = std::env::var("RUSTODON_MASTODON_WRITER_DATABASE_URL")?;
    let owner = sqlx::PgPool::connect(&owner_url).await?;
    let writer = WriteRepository::connect(&writer_url).await?;
    let repository = Repository::connect(&reader_url).await?;
    let loader =
        RestProjectionLoader::new(repository.clone(), None, "fixture-v4-6-5.rustodon.invalid");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = BearerAuthenticator::new(repository)
        .authenticate(&headers, WRITE_STATUSES)
        .await?;
    let saved_stats: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(stats) FROM account_stats stats \
         WHERE account_id = ANY($1) ORDER BY account_id",
    )
    .bind(vec![ALICE, FOLLOW_TARGET, CAROL])
    .fetch_all(&owner)
    .await?;
    let initially_missing: Vec<i64> = sqlx::query_scalar(
        "SELECT account.id FROM accounts account \
         LEFT JOIN account_stats stats ON stats.account_id = account.id \
         WHERE stats.id IS NULL ORDER BY account.id",
    )
    .fetch_all(&owner)
    .await?;
    let mut created_status_ids = Vec::new();

    let result = async {
        let original_id = writer
            .create_status(
                &authenticated,
                "account stats cascade repair target",
                &[],
                None,
                Some(false),
                Some("public"),
                None,
                None,
                None,
            )
            .await?
            .status_id;
        created_status_ids.push(original_id);
        for reblogger_id in [FOLLOW_TARGET, CAROL] {
            let reblog_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO statuses (account_id, text, spoiler_text, visibility, local, sensitive, \
                                       reply, reblog_of_id, created_at, updated_at) \
                 VALUES ($1, '', '', 0, true, false, false, $2, \
                         clock_timestamp(), clock_timestamp()) RETURNING id",
            )
            .bind(reblogger_id)
            .bind(original_id)
            .fetch_one(&owner)
            .await?;
            created_status_ids.push(reblog_id);
            sqlx::query(
                "INSERT INTO status_stats (status_id, created_at, updated_at) \
                 VALUES ($1, clock_timestamp(), clock_timestamp())",
            )
            .bind(reblog_id)
            .execute(&owner)
            .await?;
        }
        sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
            .bind(vec![FOLLOW_TARGET, CAROL])
            .execute(&owner)
            .await?;

        writer
            .delete_status(&authenticated, original_id, false)
            .await?;
        for reblogger_id in [FOLLOW_TARGET, CAROL] {
            let live_count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM statuses \
                 WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3",
            )
            .bind(reblogger_id)
            .fetch_one(&owner)
            .await?;
            let repaired_count: i64 = sqlx::query_scalar(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1",
            )
            .bind(reblogger_id)
            .fetch_one(&owner)
            .await?;
            check_eq!(
                repaired_count,
                live_count,
                "every cascaded reblog deletion must use the final tombstoned snapshot",
            );
            let projected = loader
                .account(reblogger_id)
                .await?
                .ok_or_else(|| std::io::Error::other("reblogger should remain REST-visible"))?;
            check_eq!(projected.statuses_count, live_count);
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let status_id_strings = created_status_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
         WHERE payload -> 'arguments' ->> 'status_id' = ANY($1) \
            OR payload ->> 'object_id' = ANY($1)",
    )
    .bind(&status_id_strings)
    .execute(&owner)
    .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id = ANY($1)")
        .bind(&created_status_ids)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE parent_status_id = ANY($1)")
        .bind(&created_status_ids)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
        .bind(&created_status_ids)
        .execute(&owner)
        .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
        .bind({
            let mut ids = initially_missing;
            ids.extend([ALICE, FOLLOW_TARGET, CAROL]);
            ids
        })
        .execute(&owner)
        .await?;
    for stats in saved_stats {
        sqlx::query(
            "INSERT INTO account_stats \
             SELECT (jsonb_populate_record(NULL::account_stats, $1)).*",
        )
        .bind(stats)
        .execute(&owner)
        .await?;
    }
    result?;
    Ok(())
}
