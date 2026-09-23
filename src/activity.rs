//! Rust-owned exact daily activity, cached UTC reporting and bounded cleanup.
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
    // Lock-free fast path: most tracked requests are not due, so they must not
    // queue on the user row. The locked claim below re-checks due-ness.
    let due = sqlx::query_scalar::<_, bool>(
        "SELECT current_sign_in_at IS NULL \
             OR current_sign_in_at < (clock_timestamp() AT TIME ZONE 'UTC') - interval '24 hours' \
         FROM public.users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(false);
    if !due || !eligible_user_in(tx, user_id, false).await? {
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
    let previous_expiry = loop {
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
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(expiry) = previous_expiry {
            break expiry;
        }
        // Cleanup won between conflict detection and locking; recreate safely.
    };
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

#[cfg(test)]
mod aggregation_tests;

use crate::mastodon::rest::InstanceActivityCounts;
use chrono::NaiveDate;
use sqlx::PgPool;
use std::{future::Future, sync::Arc, time::Instant};
use tokio::sync::Mutex;

const CACHE_TTL: std::time::Duration = std::time::Duration::from_mins(1);
const FAILURE_TTL: std::time::Duration = std::time::Duration::from_secs(5);
const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Failed refreshes serve the last good counts up to this age, then zeroes. A
/// user count is not worth failing the web client, instance or `NodeInfo` routes.
const STALE_LIMIT: std::time::Duration = std::time::Duration::from_hours(24);

fn reporting_days(now: DateTime<Utc>) -> (NaiveDate, NaiveDate, NaiveDate) {
    let today = now.date_naive();
    (
        today - Duration::days(28),
        today - Duration::days(168),
        today,
    )
}

/// UTC/as-of captured once. A single snapshot counts exact unions, not daily sums.
async fn aggregate(pool: &PgPool, now: DateTime<Utc>) -> sqlx::Result<InstanceActivityCounts> {
    let (month, halfyear, today) = reporting_days(now);
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout = '3000ms'")
        .execute(&mut *tx)
        .await?;
    let (active_month, active_halfyear) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(DISTINCT member.user_id) FILTER (WHERE bucket.day >= $1), \
                count(DISTINCT member.user_id) \
         FROM rustodon.activity_buckets bucket \
         JOIN rustodon.activity_members member USING (day) \
         WHERE bucket.day >= $2 AND bucket.day < $3 AND bucket.expires_at > $4",
    )
    .bind(month)
    .bind(halfyear)
    .bind(today)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(InstanceActivityCounts {
        active_month,
        active_halfyear,
    })
}

#[derive(Clone, Default)]
pub(crate) struct ActivityCache(Arc<Mutex<CacheState>>);

#[derive(Default)]
struct CacheState {
    current: Option<CachedActivity>,
    last_good: Option<(Instant, InstanceActivityCounts)>,
}

struct CachedActivity {
    day: NaiveDate,
    until: Instant,
    counts: Option<InstanceActivityCounts>,
}

impl CacheState {
    fn fallback(&self, tick: Instant) -> InstanceActivityCounts {
        self.last_good
            .filter(|(at, _)| tick.saturating_duration_since(*at) <= STALE_LIMIT)
            .map(|(_, counts)| counts)
            .unwrap_or_default()
    }
}

impl ActivityCache {
    pub(crate) async fn get(&self, pool: &PgPool) -> InstanceActivityCounts {
        self.get_with(|| (Utc::now(), Instant::now()), |now| aggregate(pool, now))
            .await
    }

    #[cfg(test)]
    async fn get_at<F, Fut>(
        &self,
        now: DateTime<Utc>,
        tick: Instant,
        load: F,
    ) -> InstanceActivityCounts
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = sqlx::Result<InstanceActivityCounts>>,
    {
        self.get_with(|| (now, tick), |_| load()).await
    }

    async fn get_with<C, F, Fut>(&self, clock: C, load: F) -> InstanceActivityCounts
    where
        C: Fn() -> (DateTime<Utc>, Instant),
        F: FnOnce(DateTime<Utc>) -> Fut,
        Fut: Future<Output = sqlx::Result<InstanceActivityCounts>>,
    {
        // Held through refresh: clones of WebState share one flight. Cancellation
        // drops the lock and the SQL transaction; it cannot publish partial data.
        let mut state = self.0.lock().await;
        let (now, tick) = clock();
        if let Some(cached) = state.current.as_ref()
            && cached.day == now.date_naive()
            && tick < cached.until
        {
            return cached.counts.unwrap_or_else(|| state.fallback(tick));
        }
        let result = tokio::time::timeout(QUERY_TIMEOUT, load(now))
            .await
            .unwrap_or(Err(sqlx::Error::PoolTimedOut))
            .ok();
        let (finished, finished_tick) = clock();
        if let Some(counts) = result {
            state.last_good = Some((finished_tick, counts));
        }
        // Do not publish yesterday's snapshot across midnight. Next request
        // starts a fresh flight for the new reporting date.
        state.current = (finished.date_naive() == now.date_naive()).then(|| CachedActivity {
            day: now.date_naive(),
            until: finished_tick
                + if result.is_some() {
                    CACHE_TTL
                } else {
                    FAILURE_TTL
                },
            counts: result,
        });
        result.unwrap_or_else(|| state.fallback(finished_tick))
    }
}

/// Existing maintenance caller supplies its writer pool, never runtime credentials.
/// At most 100 buckets and 1,000 members total per invocation. Bucket locks match
/// recording's lock order; SKIP LOCKED avoids delaying a live renewal.
pub(crate) async fn prune(pool: &PgPool) -> sqlx::Result<u64> {
    tokio::time::timeout(QUERY_TIMEOUT, prune_inner(pool))
        .await
        .unwrap_or(Err(sqlx::Error::PoolTimedOut))
}

async fn prune_inner(pool: &PgPool) -> sqlx::Result<u64> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout = '3000ms'")
        .execute(&mut *tx)
        .await?;
    let days = sqlx::query_scalar::<_, NaiveDate>(
        "SELECT day FROM rustodon.activity_buckets WHERE expires_at <= clock_timestamp() \
         ORDER BY expires_at, day LIMIT 100 FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut remaining = 1000_i64;
    for day in days {
        let removed = sqlx::query(
            "DELETE FROM rustodon.activity_members WHERE day = $1 AND user_id IN \
             (SELECT user_id FROM rustodon.activity_members WHERE day = $1 ORDER BY user_id LIMIT $2)",
        ).bind(day).bind(remaining).execute(&mut *tx).await?.rows_affected();
        remaining -= i64::try_from(removed).expect("bounded member chunk");
        sqlx::query(
            "DELETE FROM rustodon.activity_buckets WHERE day = $1 \
            AND expires_at <= clock_timestamp() \
            AND NOT EXISTS (SELECT 1 FROM rustodon.activity_members WHERE day = $1)",
        )
        .bind(day)
        .execute(&mut *tx)
        .await?;
        if remaining == 0 {
            break;
        }
    }
    tx.commit().await?;
    Ok(u64::try_from(1000 - remaining).expect("nonnegative member count"))
}
