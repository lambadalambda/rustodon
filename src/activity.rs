//! Rust-owned exact daily activity. No public counts or historical backfill.
use chrono::{DateTime, Duration, Utc};
use sqlx::{Postgres, Transaction};

// ActiveSupport 8.1.3: 6 * SECONDS_PER_MONTH (2_629_746), not 24 weeks.
pub(crate) const RETENTION_SECONDS: i64 = 15_778_476;

/// Call after a creation or an eligibility transition, in that same transaction.
/// `was_eligible` is the pre-transition confirmed-and-approved state, captured
/// while holding the user's lock. Future approval paths can reuse this hook.
pub(crate) async fn record_activation_in(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    was_eligible: bool,
) -> sqlx::Result<()> {
    if !was_eligible && eligible_user_in(tx, user_id, true).await? {
        let now = write_time_in(tx).await?;
        record_member_in(tx, user_id, now).await?;
    }
    Ok(())
}

/// Only called after complete authentication and the final credential fence,
/// before the session transaction commits. Earlier login audits are not events.
pub(crate) async fn record_login_in(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
) -> sqlx::Result<()> {
    if eligible_user_in(tx, user_id, false).await? {
        let now = write_time_in(tx).await?;
        record_member_in(tx, user_id, now).await?;
    }
    Ok(())
}

async fn eligible_user_in(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    activation: bool,
) -> sqlx::Result<bool> {
    // Returning activity follows the caller's existing endpoint authorization,
    // rather than introducing a new global disabled/suspended/approval policy.
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT u.confirmed_at IS NOT NULL AND (NOT $2 OR u.approved) \
         FROM public.users u JOIN public.accounts a ON a.id = u.account_id \
         WHERE u.id = $1 AND a.domain IS NULL FOR UPDATE OF u",
    )
    .bind(user_id)
    .bind(activation)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(false))
}

async fn write_time_in(tx: &mut Transaction<'_, Postgres>) -> sqlx::Result<DateTime<Utc>> {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await
}

/// Explicit interactive callers only; never a shared session lookup or bearer
/// authentication side effect. The user lock serializes concurrent due claims.
pub(crate) async fn track_returning_in(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
) -> sqlx::Result<()> {
    if !eligible_user_in(tx, user_id, false).await? {
        return Ok(());
    }
    let now = write_time_in(tx).await?;
    track_returning_at_in(tx, user_id, now).await
}

async fn track_returning_at_in(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    let claimed = sqlx::query_scalar::<_, i64>(
        "UPDATE public.users SET last_sign_in_at = COALESCE(current_sign_in_at, $2), \
             current_sign_in_at = $2, updated_at = $2 \
         WHERE id = $1 AND (current_sign_in_at IS NULL OR current_sign_in_at < $3) \
         RETURNING id",
    )
    .bind(user_id)
    .bind(now.naive_utc())
    .bind((now - Duration::hours(24)).naive_utc())
    .fetch_optional(&mut **tx)
    .await?;
    if claimed.is_some() {
        record_member_in(tx, user_id, now).await?;
    }
    Ok(())
}

async fn record_member_in(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    let day = now.date_naive();
    let expires_at = now + Duration::seconds(RETENTION_SECONDS);
    // Insert before locking handles a concurrent first write. Every membership
    // mutation holds this bucket lock, including clearing an expired generation.
    sqlx::query(
        "INSERT INTO rustodon.activity_buckets (day, expires_at) VALUES ($1, $2) \
         ON CONFLICT (day) DO NOTHING",
    )
    .bind(day)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    let previous_expiry = sqlx::query_scalar::<_, DateTime<Utc>>(
        "SELECT expires_at FROM rustodon.activity_buckets WHERE day = $1 FOR UPDATE",
    )
    .bind(day)
    .fetch_one(&mut **tx)
    .await?;
    // Like Redis EXPIRE after PFADD: renew from the actual write clock after
    // waiting for the bucket lock, not from a possibly delayed event timestamp.
    let expiry_write_time = write_time_in(tx).await?;
    if previous_expiry <= expiry_write_time {
        sqlx::query("DELETE FROM rustodon.activity_members WHERE day = $1")
            .bind(day)
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("UPDATE rustodon.activity_buckets SET expires_at = $2 WHERE day = $1")
        .bind(day)
        .bind(expiry_write_time + Duration::seconds(RETENTION_SECONDS))
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO rustodon.activity_members (day, user_id) VALUES ($1, $2) \
         ON CONFLICT (day, user_id) DO NOTHING",
    )
    .bind(day)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
