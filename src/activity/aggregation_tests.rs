use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn utc_windows_are_exact_weeks_not_calendar_months() {
    let now = "2026-03-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let (month, halfyear, today) = reporting_days(now);
    assert_eq!(today.to_string(), "2026-03-01");
    assert_eq!((today - month).num_days(), 28);
    assert_eq!((today - halfyear).num_days(), 168);
    assert_eq!(
        reporting_days(now + Duration::hours(23)),
        (month, halfyear, today)
    );
    assert_ne!(reporting_days(now - Duration::seconds(1)).2, today);
}

const COUNTS: InstanceActivityCounts = InstanceActivityCounts {
    active_month: 2,
    active_halfyear: 3,
};

async fn failing() -> sqlx::Result<InstanceActivityCounts> {
    Err(sqlx::Error::PoolClosed)
}

#[tokio::test]
async fn cache_coalesces_and_serves_last_good_counts_on_failure() {
    let cache = ActivityCache::default();
    let calls = AtomicUsize::new(0);
    let now = "2026-03-01T23:59:59Z".parse::<DateTime<Utc>>().unwrap();
    let tick = std::time::Instant::now();
    let load = || async {
        calls.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Ok(COUNTS)
    };
    let (a, b) = tokio::join!(cache.get_at(now, tick, load), cache.get_at(now, tick, load));
    assert_eq!((a, b), (COUNTS, COUNTS));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    cache.get_at(now, tick + CACHE_TTL, load).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let next = now + Duration::seconds(1);
    let stale = tick + CACHE_TTL * 2;
    assert_eq!(cache.get_at(next, stale, failing).await, COUNTS);
    // A failed refresh is coalesced too; no request-by-request failure scan.
    assert_eq!(cache.get_at(next, stale, load).await, COUNTS);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    cache.get_at(next, stale + FAILURE_TTL, load).await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn cache_serves_zero_without_recent_good_counts() {
    let cache = ActivityCache::default();
    let now = "2026-03-01T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let tick = std::time::Instant::now();
    assert_eq!(
        cache.get_at(now, tick, failing).await,
        InstanceActivityCounts::default(),
        "cold failure"
    );
    cache
        .get_at(now, tick + FAILURE_TTL, || async { Ok(COUNTS) })
        .await;
    let late = tick + FAILURE_TTL + STALE_LIMIT + CACHE_TTL;
    assert_eq!(
        cache.get_at(now, late, failing).await,
        InstanceActivityCounts::default(),
        "stale beyond limit"
    );
}

#[tokio::test]
async fn cache_returns_but_does_not_publish_snapshot_after_utc_rollover() {
    let cache = ActivityCache::default();
    let before = "2026-03-01T23:59:59Z".parse::<DateTime<Utc>>().unwrap();
    let tick = std::time::Instant::now();
    let clock_reads = AtomicUsize::new(0);
    let counts = cache
        .get_with(
            || {
                let read = clock_reads.fetch_add(1, Ordering::SeqCst);
                (
                    before + Duration::seconds(i64::try_from(read).unwrap()),
                    tick,
                )
            },
            |_| async { Ok(COUNTS) },
        )
        .await;
    assert_eq!(counts, COUNTS);
    assert!(cache.0.lock().await.current.is_none());
    let counts = cache
        .get_at(before + Duration::seconds(1), tick, || async {
            Ok(InstanceActivityCounts::default())
        })
        .await;
    assert_eq!(
        counts,
        InstanceActivityCounts::default(),
        "empty fresh installs are real zeros"
    );
}
