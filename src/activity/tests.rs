use super::*;
use crate::mastodon::{BrowserAuthenticationError, WriteRepository};
use chrono::NaiveDateTime;
use sqlx::PgPool;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn pools() -> Result<(PgPool, PgPool, PgPool), Box<dyn std::error::Error>> {
    let owner = PgPool::connect(&std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?).await?;
    let writer = PgPool::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let reader = PgPool::connect(&std::env::var("RUSTODON_WORKER_DATABASE_URL")?).await?;
    let owner_name: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&owner)
        .await?;
    for pool in [&writer, &reader] {
        let (name, restricted): (String, bool) = sqlx::query_as(
            "SELECT rolname::text, NOT (rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls) \
             AND NOT EXISTS (SELECT 1 FROM pg_auth_members WHERE member = r.oid) \
             AND NOT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname IN ('public', 'rustodon') AND nspowner=r.oid) \
             FROM pg_roles r WHERE rolname=current_user",
        ).fetch_one(pool).await?;
        assert_ne!(name, owner_name);
        assert!(restricted);
    }
    sqlx::raw_sql("DELETE FROM rustodon.activity_members; DELETE FROM rustodon.activity_buckets")
        .execute(&owner)
        .await?;
    Ok((owner, writer, reader))
}

async fn user(
    owner: &PgPool,
    label: &str,
    approved: bool,
    confirmed: bool,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "WITH account AS (INSERT INTO accounts (username, created_at, updated_at) \
          VALUES ($1, clock_timestamp(), clock_timestamp()) RETURNING id) \
         INSERT INTO users (account_id, email, approved, confirmed_at, created_at, updated_at) \
         SELECT id, $1 || '@activity.invalid', $2, CASE WHEN $3 THEN clock_timestamp() END, \
           clock_timestamp(), clock_timestamp() FROM account RETURNING id",
    )
    .bind(format!(
        "{label}-{}",
        Utc::now().timestamp_nanos_opt().unwrap()
    ))
    .bind(approved)
    .bind(confirmed)
    .fetch_one(owner)
    .await
}

async fn count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM rustodon.activity_members")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable migrated PG14 database and distinct restricted roles"]
#[allow(clippy::too_many_lines)]
async fn exact_membership_expiry_and_transactional_eligibility() -> TestResult {
    let (owner, writer, reader) = pools().await?;
    let id = user(&owner, "eligible", true, true).await?;
    let pending = user(&owner, "pending", false, true).await?;
    let unconfirmed = user(&owner, "unconfirmed", true, false).await?;
    let remote = user(&owner, "remote", true, true).await?;
    sqlx::query("UPDATE accounts SET domain='remote.invalid' WHERE id=(SELECT account_id FROM users WHERE id=$1)")
        .bind(remote).execute(&owner).await?;
    let mut tx = writer.begin().await?;
    for excluded in [pending, unconfirmed, remote] {
        record_activation_in(&mut tx, excluded, false).await?;
    }
    tx.commit().await?;
    assert_eq!(count(&reader).await, 0);
    // A future approval transition can use the same helper, without an API.
    let mut tx = owner.begin().await?;
    sqlx::query("UPDATE users SET approved=true WHERE id=$1")
        .bind(pending)
        .execute(&mut *tx)
        .await?;
    record_activation_in(&mut tx, pending, false).await?;
    tx.rollback().await?;
    assert_eq!(count(&reader).await, 0);
    let mut tx = owner.begin().await?;
    sqlx::query("UPDATE users SET approved=true WHERE id=$1")
        .bind(pending)
        .execute(&mut *tx)
        .await?;
    record_activation_in(&mut tx, pending, false).await?;
    tx.commit().await?;
    assert_eq!(count(&reader).await, 1);
    let mut tx = writer.begin().await?;
    record_activation_in(&mut tx, id, true).await?;
    tx.commit().await?;
    assert_eq!(count(&reader).await, 1, "not a first transition");
    let mut tx = writer.begin().await?;
    record_activation_in(&mut tx, id, false).await?;
    tx.rollback().await?;
    assert_eq!(count(&reader).await, 1);
    for _ in 0..2 {
        let mut tx = writer.begin().await?;
        record_activation_in(&mut tx, id, false).await?;
        tx.commit().await?;
    }
    assert_eq!(count(&reader).await, 2, "retry is idempotent");

    // Historical rows cannot change when current account state changes or vanishes.
    sqlx::query("UPDATE users SET disabled=true, approved=false, confirmed_at=NULL WHERE id=$1")
        .bind(id)
        .execute(&owner)
        .await?;
    sqlx::query("UPDATE accounts SET suspended_at=clock_timestamp() WHERE id=(SELECT account_id FROM users WHERE id=$1)")
        .bind(id).execute(&owner).await?;
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(id)
        .execute(&owner)
        .await?;
    assert_eq!(count(&reader).await, 2);

    let mut tx = writer.begin().await?;
    // Keep an exact UTC midnight boundary, separate from today's activations,
    // without assuming a calendar date is in the past on the executing worker.
    let now = (write_time_in(&mut tx).await? - Duration::days(2))
        .date_naive()
        .and_hms_opt(23, 59, 59)
        .unwrap()
        .and_utc();
    record_member_in(&mut tx, 99, now).await?;
    let first_expiry: DateTime<Utc> =
        sqlx::query_scalar("SELECT expires_at FROM rustodon.activity_buckets WHERE day=$1")
            .bind(now.date_naive())
            .fetch_one(&mut *tx)
            .await?;
    let before = write_time_in(&mut tx).await?;
    record_member_in(&mut tx, 99, now + Duration::milliseconds(500)).await?;
    let after = write_time_in(&mut tx).await?;
    let expiry: DateTime<Utc> =
        sqlx::query_scalar("SELECT expires_at FROM rustodon.activity_buckets WHERE day=$1")
            .bind(now.date_naive())
            .fetch_one(&mut *tx)
            .await?;
    assert!(
        expiry > first_expiry,
        "even duplicate members renew the whole bucket"
    );
    assert!(expiry >= before + Duration::seconds(15_778_476));
    assert!(expiry <= after + Duration::seconds(15_778_476));
    record_member_in(&mut tx, 99, now + Duration::seconds(1)).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.activity_members WHERE user_id=99"
        )
        .fetch_one(&mut *tx)
        .await?,
        2,
        "UTC midnight creates another daily membership"
    );
    // Simulate expired storage still awaiting the later maintenance prune.
    sqlx::query("UPDATE rustodon.activity_buckets SET expires_at=clock_timestamp()-interval '1 second' WHERE day=$1")
        .bind(now.date_naive())
        .execute(&mut *tx)
        .await?;
    record_member_in(&mut tx, 100, now + Duration::milliseconds(600)).await?;
    let members: Vec<i64> = sqlx::query_scalar(
        "SELECT user_id FROM rustodon.activity_members WHERE day=$1 ORDER BY user_id",
    )
    .bind(now.date_naive())
    .fetch_all(&mut *tx)
    .await?;
    assert_eq!(members, vec![100], "expired memberships must not resurrect");
    tx.commit().await?;
    // Reader gets precisely SELECT, not mutation rights; writer has no DDL grants.
    for sql in [
        "DELETE FROM rustodon.activity_members",
        "UPDATE rustodon.activity_buckets SET expires_at=clock_timestamp()",
        "INSERT INTO rustodon.activity_members VALUES (CURRENT_DATE, 1)",
    ] {
        let error = sqlx::query(sql).execute(&reader).await.unwrap_err();
        assert_eq!(
            error
                .as_database_error()
                .and_then(sqlx::error::DatabaseError::code)
                .as_deref(),
            Some("42501")
        );
    }
    let error = sqlx::query("ALTER TABLE rustodon.activity_members ADD COLUMN forbidden bigint")
        .execute(&writer)
        .await
        .unwrap_err();
    assert_eq!(
        error
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("42501")
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable migrated PG14 database and distinct restricted roles"]
async fn returning_claim_is_strict_atomic_and_serialized() -> TestResult {
    let (owner, writer, reader) = pools().await?;
    let id = user(&owner, "returning", true, true).await?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner)
        .await?;
    sqlx::query("UPDATE users SET current_sign_in_at=$2, sign_in_count=7 WHERE id=$1")
        .bind(id)
        .bind((now - Duration::hours(24)).naive_utc())
        .execute(&owner)
        .await?;
    let mut tx = writer.begin().await?;
    assert!(eligible_user_in(&mut tx, id, false).await?);
    track_returning_at_in(&mut tx, id, now).await?;
    tx.commit().await?;
    assert_eq!(count(&reader).await, 0, "exactly 24h is not due");
    let mut tx = writer.begin().await?;
    assert!(eligible_user_in(&mut tx, id, false).await?);
    track_returning_at_in(&mut tx, id, now + Duration::microseconds(1)).await?;
    tx.rollback().await?;
    assert_eq!(count(&reader).await, 0);
    let calls = (0..8).map(|_| {
        let writer = writer.clone();
        async move {
            let mut tx = writer.begin().await?;
            assert!(eligible_user_in(&mut tx, id, false).await?);
            track_returning_at_in(&mut tx, id, now + Duration::microseconds(1)).await?;
            tx.commit().await
        }
    });
    for result in futures_util::future::join_all(calls).await {
        result?;
    }
    assert_eq!(count(&reader).await, 1);
    let (last, current, count): (NaiveDateTime, NaiveDateTime, i64) = sqlx::query_as(
        "SELECT last_sign_in_at, current_sign_in_at, sign_in_count::bigint FROM users WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&reader)
    .await?;
    assert_eq!(last, (now - Duration::hours(24)).naive_utc());
    assert_eq!(current, (now + Duration::microseconds(1)).naive_utc());
    assert_eq!(count, 7);
    sqlx::query("UPDATE users SET current_sign_in_at=NULL WHERE id=$1")
        .bind(id)
        .execute(&owner)
        .await?;
    let mut tx = writer.begin().await?;
    track_returning_in(&mut tx, id).await?;
    tx.commit().await?;
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT last_sign_in_at=current_sign_in_at FROM users WHERE id=$1"
        )
        .bind(id)
        .fetch_one(&reader)
        .await?
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable migrated PG14 database and distinct restricted roles"]
async fn returning_tracking_that_is_not_due_takes_no_user_lock() -> TestResult {
    let (owner, writer, reader) = pools().await?;
    let id = user(&owner, "not-due", true, true).await?;
    sqlx::query(
        "UPDATE users SET current_sign_in_at = clock_timestamp() AT TIME ZONE 'UTC' WHERE id=$1",
    )
    .bind(id)
    .execute(&owner)
    .await?;
    let mut holder = owner.begin().await?;
    sqlx::query("SELECT 1 FROM users WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *holder)
        .await?;
    let mut tx = writer.begin().await?;
    sqlx::query("SET LOCAL lock_timeout = '200ms'")
        .execute(&mut *tx)
        .await?;
    let tracked = track_returning_in(&mut tx, id).await;
    tx.rollback().await?;
    holder.rollback().await?;
    tracked?;
    assert_eq!(count(&reader).await, 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable migrated PG14 database and distinct restricted roles"]
#[allow(clippy::too_many_lines)]
async fn complete_session_only_records_after_password_totp_or_backup_fence() -> TestResult {
    let (owner, _, reader) = pools().await?;
    let repository =
        WriteRepository::connect(&std::env::var("RUSTODON_WORKER_WRITE_DATABASE_URL")?).await?;
    let email = format!(
        "login-{}@activity.invalid",
        Utc::now().timestamp_nanos_opt().unwrap()
    );
    let username = format!("activity{}", Utc::now().timestamp_micros());
    let created = repository
        .create_local_user(&email, &username, "activity-password")
        .await?;
    assert_eq!(
        count(&reader).await,
        1,
        "confirmed creation is an activation"
    );
    sqlx::query("DELETE FROM rustodon.activity_members")
        .execute(&owner)
        .await?;
    let ip = "127.0.0.1".parse()?;
    assert!(matches!(
        repository
            .authenticate_browser_user(&email, "wrong", None, 59, ip, "activity-test")
            .await,
        Err(BrowserAuthenticationError::InvalidCredentials)
    ));
    assert_eq!(count(&reader).await, 0);
    let authentication = repository
        .authenticate_browser_user(&email, "activity-password", None, 59, ip, "activity-test")
        .await?;
    assert_eq!(
        count(&reader).await,
        0,
        "early committed audit is not activity"
    );
    // Fail after token/session insertion: membership and issuance must roll back together.
    let sessions_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_activations WHERE user_id=$1")
            .bind(created.user_id)
            .fetch_one(&owner)
            .await?;
    sqlx::query(
        "ALTER TABLE rustodon.activity_members ADD CONSTRAINT activity_test_failure CHECK (false)",
    )
    .execute(&owner)
    .await?;
    assert!(
        repository
            .create_browser_session(&authentication, ip, "activity-test")
            .await
            .is_err()
    );
    sqlx::query("ALTER TABLE rustodon.activity_members DROP CONSTRAINT activity_test_failure")
        .execute(&owner)
        .await?;
    assert_eq!(
        sessions_before,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM session_activations WHERE user_id=$1")
            .bind(created.user_id)
            .fetch_one(&owner)
            .await?
    );
    assert_eq!(count(&reader).await, 0);
    // A credential fence rejection cannot count or issue a session.
    let old_hash: String = sqlx::query_scalar("SELECT encrypted_password FROM users WHERE id=$1")
        .bind(created.user_id)
        .fetch_one(&owner)
        .await?;
    sqlx::query("UPDATE users SET encrypted_password='changed' WHERE id=$1")
        .bind(created.user_id)
        .execute(&owner)
        .await?;
    assert!(
        repository
            .create_browser_session(&authentication, ip, "activity-test")
            .await
            .is_err()
    );
    assert_eq!(count(&reader).await, 0);
    sqlx::query("UPDATE users SET encrypted_password=$2 WHERE id=$1")
        .bind(created.user_id)
        .bind(old_hash)
        .execute(&owner)
        .await?;
    repository
        .create_browser_session(&authentication, ip, "activity-test")
        .await?;
    assert_eq!(count(&reader).await, 1);
    sqlx::query("DELETE FROM rustodon.activity_members")
        .execute(&owner)
        .await?;
    sqlx::query("UPDATE users SET otp_required_for_login=true, otp_secret='GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ', otp_backup_codes=ARRAY['fixture-backup'], consumed_timestep=NULL WHERE id=$1")
        .bind(created.user_id).execute(&owner).await?;
    assert!(matches!(
        repository
            .authenticate_browser_user(&email, "activity-password", None, 59, ip, "activity-test")
            .await,
        Err(BrowserAuthenticationError::TwoFactorRequired)
    ));
    assert!(matches!(
        repository
            .authenticate_browser_user(
                &email,
                "activity-password",
                Some("wrong"),
                59,
                ip,
                "activity-test"
            )
            .await,
        Err(BrowserAuthenticationError::InvalidTwoFactor)
    ));
    assert_eq!(count(&reader).await, 0);
    for code in ["287082", "fixture-backup"] {
        let authentication = repository
            .authenticate_browser_user(
                &email,
                "activity-password",
                Some(code),
                59,
                ip,
                "activity-test",
            )
            .await?;
        assert_eq!(count(&reader).await, 0);
        repository
            .create_browser_session(&authentication, ip, "activity-test")
            .await?;
        assert_eq!(count(&reader).await, 1);
        assert!(sqlx::query_scalar::<_, bool>("SELECT bool_and(day=(clock_timestamp() AT TIME ZONE 'UTC')::date) FROM rustodon.activity_members")
            .fetch_one(&reader).await?, "actual UTC clock, never TOTP time 59");
        sqlx::query("DELETE FROM rustodon.activity_members")
            .execute(&owner)
            .await?;
    }
    let job = crate::jobs::JobSpec::new(
        crate::jobs::Lane::Mail,
        "test.activity_confirmation",
        serde_json::json!({}),
    );
    let signup = repository
        .create_local_user_with_confirmation(
            &format!("unconfirmed-{email}"),
            &format!("new{username}"),
            "activity-password",
            &format!("signup-{email}"),
            None,
            &job,
        )
        .await?;
    assert!(!signup.confirmed);
    assert_eq!(
        count(&reader).await,
        0,
        "unconfirmed creation is not activation"
    );
    // Confirmation only records the first approved transition.
    let pending = user(&owner, "confirmation", true, false).await?;
    sqlx::query("UPDATE users SET confirmation_token='activity-confirm', confirmation_sent_at=clock_timestamp() WHERE id=$1")
        .bind(pending).execute(&owner).await?;
    assert!(!repository.confirm_user_with_token("wrong-confirm").await?);
    assert_eq!(count(&reader).await, 0);
    assert!(
        repository
            .confirm_user_with_token("activity-confirm")
            .await?
    );
    assert!(
        !repository
            .confirm_user_with_token("activity-confirm")
            .await?
    );
    assert_eq!(count(&reader).await, 1);
    let unapproved = user(&owner, "unapproved-confirmation", false, false).await?;
    sqlx::query("UPDATE users SET confirmation_token='pending-activity-confirm', confirmation_sent_at=clock_timestamp() WHERE id=$1")
        .bind(unapproved).execute(&owner).await?;
    assert!(
        repository
            .confirm_user_with_token("pending-activity-confirm")
            .await?
    );
    assert_eq!(
        count(&reader).await,
        1,
        "confirmation without approval is not activation"
    );
    // Read eligibility again inside session creation (not a stale auth snapshot).
    let authentication = repository
        .authenticate_browser_user(
            &email,
            "activity-password",
            Some("081804"),
            1_111_111_109,
            ip,
            "activity-test",
        )
        .await?;
    sqlx::query("UPDATE users SET confirmed_at=NULL WHERE id=$1")
        .bind(created.user_id)
        .execute(&owner)
        .await?;
    repository
        .create_browser_session(&authentication, ip, "activity-test")
        .await?;
    assert_eq!(
        count(&reader).await,
        1,
        "no new global session policy, but no unconfirmed activity"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable migrated PG14 database and distinct restricted roles"]
async fn stored_members_support_exact_exclusive_day_windows() -> TestResult {
    let (_, writer, reader) = pools().await?;
    let mut tx = writer.begin().await?;
    // The simulated read and real-clock retention writes share a clock origin.
    let now = write_time_in(&mut tx).await?;
    for (id, days) in [(1, 28), (1, 1), (2, 29), (3, 168), (4, 169), (5, 0), (6, 2)] {
        record_member_in(&mut tx, id, now - Duration::days(days)).await?;
    }
    sqlx::query("UPDATE rustodon.activity_buckets SET expires_at=$2 WHERE day=$1")
        .bind((now - Duration::days(2)).date_naive())
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    for (days, expected) in [(28, 1), (168, 3)] {
        // This is the future aggregation's storage contract, not a public count implementation.
        let count: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT user_id) FROM rustodon.activity_members JOIN rustodon.activity_buckets USING (day) \
             WHERE day >= $1 AND day < $2 AND expires_at > $3",
        ).bind((now - Duration::days(days)).date_naive()).bind(now.date_naive()).bind(now).fetch_one(&reader).await?;
        assert_eq!(count, expected);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable migrated PG14 database and distinct restricted roles"]
async fn aggregation_exact_windows_and_historical_membership() -> TestResult {
    let (owner, writer, reader) = pools().await?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner)
        .await?;
    let id = user(&owner, "aggregate", true, true).await?;
    let today = now.date_naive();
    // The same user on the oldest and last month day counts once; other IDs
    // deliberately have no live user row. Reporting must not join users.
    for (ago, ids, expired) in [
        (0, vec![90], false),
        (1, vec![id, 91], false),
        (28, vec![id, 92], false),
        (29, vec![93], false),
        (168, vec![94], false),
        (169, vec![95], false),
        (2, vec![96], true),
    ] {
        let day = today - Duration::days(ago);
        sqlx::query("INSERT INTO rustodon.activity_buckets(day,expires_at) VALUES($1,$2)")
            .bind(day)
            .bind(if expired {
                now
            } else {
                now + Duration::days(1)
            })
            .execute(&writer)
            .await?;
        for id in ids {
            sqlx::query("INSERT INTO rustodon.activity_members(day,user_id) VALUES($1,$2)")
                .bind(day)
                .bind(id)
                .execute(&writer)
                .await?;
        }
    }
    let expected = InstanceActivityCounts {
        active_month: 3,
        active_halfyear: 5,
    };
    assert_eq!(aggregate(&reader, now).await?, expected);
    sqlx::query("UPDATE users SET disabled=true, approved=false, confirmed_at=NULL WHERE id=$1")
        .bind(id)
        .execute(&owner)
        .await?;
    assert_eq!(aggregate(&reader, now).await?, expected);
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(id)
        .execute(&owner)
        .await?;
    assert_eq!(aggregate(&reader, now).await?, expected);
    assert!(
        prune(&reader).await.is_err(),
        "runtime must remain SELECT-only"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable migrated PG14 database and distinct restricted roles"]
async fn pruning_is_chunked_and_skips_recording_renewals() -> TestResult {
    let (owner, writer, reader) = pools().await?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner)
        .await?;
    let day = now.date_naive();
    sqlx::query("INSERT INTO rustodon.activity_buckets(day,expires_at) VALUES($1,$2)")
        .bind(day)
        .bind(now - Duration::seconds(1))
        .execute(&writer)
        .await?;
    sqlx::query("INSERT INTO rustodon.activity_members SELECT $1, generate_series(1,1002)")
        .bind(day)
        .execute(&writer)
        .await?;
    assert_eq!(prune(&writer).await?, 1000);
    assert_eq!(count(&reader).await, 2);
    let mut renewal = writer.begin().await?;
    sqlx::query("SELECT day FROM rustodon.activity_buckets WHERE day=$1 FOR UPDATE")
        .bind(day)
        .execute(&mut *renewal)
        .await?;
    assert_eq!(prune(&writer).await?, 0);
    record_member_in(&mut renewal, 1003, now).await?;
    renewal.commit().await?;
    assert_eq!(prune(&writer).await?, 0);
    assert_eq!(count(&reader).await, 1);
    sqlx::query("UPDATE rustodon.activity_buckets SET expires_at=$1")
        .bind(now)
        .execute(&writer)
        .await?;
    assert_eq!(prune(&writer).await?, 1);
    let buckets: i64 = sqlx::query_scalar("SELECT count(*) FROM rustodon.activity_buckets")
        .fetch_one(&reader)
        .await?;
    assert_eq!(buckets, 0);
    // Renewal after complete cleanup recreates bucket and membership atomically.
    let mut renewal = writer.begin().await?;
    record_member_in(&mut renewal, 1003, now).await?;
    renewal.commit().await?;
    assert_eq!(count(&reader).await, 1);
    Ok(())
}

#[tokio::test]
#[ignore = "requires a disposable migrated PG14 database and distinct restricted roles"]
async fn recording_waiting_for_cleanup_recreates_no_orphans() -> TestResult {
    let (owner, writer, reader) = pools().await?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&owner)
        .await?;
    let day = now.date_naive();
    sqlx::query("INSERT INTO rustodon.activity_buckets VALUES($1,$2)")
        .bind(day)
        .bind(now - Duration::seconds(1))
        .execute(&writer)
        .await?;
    // Hold cleanup's deletion uncommitted while a recorder tries to insert/lock.
    let mut cleanup = writer.begin().await?;
    sqlx::query("SELECT day FROM rustodon.activity_buckets WHERE day=$1 FOR UPDATE")
        .bind(day)
        .execute(&mut *cleanup)
        .await?;
    sqlx::query("DELETE FROM rustodon.activity_buckets WHERE day=$1")
        .bind(day)
        .execute(&mut *cleanup)
        .await?;
    let task_writer = writer.clone();
    let recorder = tokio::spawn(async move {
        let mut tx = task_writer.begin().await?;
        sqlx::query("SET LOCAL application_name = 'activity-renewal-race'")
            .execute(&mut *tx)
            .await?;
        record_member_in(&mut tx, 9001, now).await?;
        tx.commit().await
    });
    let waiting = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
                WHERE application_name='activity-renewal-race' AND wait_event_type='Lock')",
            )
            .fetch_one(&owner)
            .await?;
            if waiting {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    // Release before asserting so a failed fixture cannot leave a blocked task.
    cleanup.commit().await?;
    waiting??;
    tokio::time::timeout(std::time::Duration::from_secs(3), recorder).await???;
    assert_eq!(count(&reader).await, 1);
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rustodon.activity_buckets \
        WHERE day=$1 AND expires_at > clock_timestamp())",
    )
    .bind(day)
    .fetch_one(&reader)
    .await?;
    assert!(live);
    assert_eq!(prune(&writer).await?, 0);
    Ok(())
}
