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

#[tokio::test]
async fn cache_coalesces_and_invalidates_ttl_rollover_and_failure() {
    let cache = ActivityCache::default();
    let calls = AtomicUsize::new(0);
    let now = "2026-03-01T23:59:59Z".parse::<DateTime<Utc>>().unwrap();
    let tick = std::time::Instant::now();
    let load = || async {
        calls.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Ok(InstanceActivityCounts {
            active_month: 2,
            active_halfyear: 3,
        })
    };
    let (a, b) = tokio::join!(cache.get_at(now, tick, load), cache.get_at(now, tick, load));
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    cache.get_at(now, tick + CACHE_TTL, load).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let next = now + Duration::seconds(1);
    assert!(
        cache
            .get_at(next, tick + CACHE_TTL, || async {
                Err(sqlx::Error::PoolClosed)
            })
            .await
            .is_err()
    );
    // A failed refresh is coalesced too; no request-by-request failure scan.
    assert!(cache.get_at(next, tick + CACHE_TTL, load).await.is_err());
    cache
        .get_at(next, tick + CACHE_TTL + FAILURE_TTL, load)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn cache_rejects_snapshot_that_finishes_after_utc_rollover() {
    let cache = ActivityCache::default();
    let before = "2026-03-01T23:59:59Z".parse::<DateTime<Utc>>().unwrap();
    let tick = std::time::Instant::now();
    let clock_reads = AtomicUsize::new(0);
    assert!(
        cache
            .get_with(
                || {
                    let read = clock_reads.fetch_add(1, Ordering::SeqCst);
                    (
                        before + Duration::seconds(i64::try_from(read).unwrap()),
                        tick,
                    )
                },
                |_| async { Ok(InstanceActivityCounts::default()) },
            )
            .await
            .is_err()
    );
    assert!(cache.0.lock().await.is_none());
    let counts = cache
        .get_at(before + Duration::seconds(1), tick, || async {
            Ok(InstanceActivityCounts::default())
        })
        .await
        .unwrap();
    assert_eq!(
        counts,
        InstanceActivityCounts::default(),
        "empty fresh installs are real zeros"
    );
}
