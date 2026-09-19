# Compute real instance activity metrics

## Current disposition — accepted after parent review 980993

Accepted and archived on parent approval. Review `980993` found no high-severity
blocker and supports focused closure from the combined reviewed recording,
cache, pinned-source, restricted PostgreSQL/worker/main-HTTP evidence and actual
browser acceptance on exact app `10ddf66caf1fe8714f7c41ac4a1a0b8ceaf7fe34`.
This disposition supersedes historical open, review-pending, browser-deferred and
leave-uncommitted statements below; earlier logs remain historical evidence,
not fresh execution claims. The wider combined acceptance sweep remains pending.

### Accepted scope and evidence boundaries

- Recording eligibility, unique membership, UTC/expiry boundaries and transactional
  behavior are covered by the recording slice; aggregation/cache/privacy/pruning
  by the restricted DB, worker, main-HTTP and library gates in the cache slice.
  Exact SQL `COUNT(DISTINCT ...)` deliberately replaces the reference's approximate
  HyperLogLog union: safer exact counting, not bug-for-bug HLL parity.
- Empty-history rollout is intentional: no timestamp-derived historical backfill.
  Browser migration-6 baseline and real eligible confirmed login TODAY both publish
  **0**. Task setup moved that real helper-recorded bucket/member to yesterday:
  ordinary sidebar/v2/initial **1**, NodeInfo **1/1**, including reload. Limited
  sidebar/v2/initial **0**, NodeInfo raw **1/1**, including reload. No observed
  midnight, real historical backfill, production deployment or clock-change claim.
- Returning tracking covers the implemented interactive HTML, credentials,
  settings and session hooks, not every API request or the entire Mastodon
  lifecycle. Full tracking parity is not claimed; media reads remain non-mutating.
- Browser focused library **3 passed / 7 ignored** and activity source contract
  **1 passed** supplement, not replace, the earlier restricted integration gates.
  Broader/full-matrix gates and the combined sweep are not claimed complete.

## Summary

The bundled public frontend always shows zero active users because Rustodon initializes the instance runtime's monthly and half-year activity counts to zero rather than loading Mastodon-compatible values.

## Requirements

- Record unique user IDs for approved account creation and confirmed returning-user sign-ins in daily activity buckets retained for six months, matching Mastodon 4.6.5's `activity:logins` lifecycle.
- Aggregate unique IDs over Mastodon's 4-week monthly and 24-week half-year windows, including its date-boundary behavior; do not derive the result from users' current enabled/suspended state.
- Define a bounded rollout policy for existing installations that have no historical activity buckets, including whether and how available last-sign-in timestamps seed approximate history.
- Expose the 4-week count in the v2 instance response, but preserve Mastodon's endpoint-specific privacy behavior: limited federation suppresses the v2 value while NodeInfo continues to publish both windows.
- Avoid expensive unbounded work on each request; use bounded buckets and appropriate cached/reconciled state.

## Acceptance Criteria

- Focused tests cover first-time approval, confirmed return, repeated logins by one user, distinct users, exact 4/24-week boundaries, and expiry, matching Mastodon 4.6.5 unique-user counts.
- A deployment with newly recorded eligible activity reports a nonzero 4-week value in `GET /api/v2/instance` and the bundled public sidebar; the rollout behavior for preexisting sign-ins is tested and documented.
- NodeInfo reports both 4-week and 24-week counts; under limited federation only the v2 instance value is suppressed.

## Subissues

- [Record transactional daily instance activity](record-transactional-daily-instance-activity.md): first bounded operational-storage/authentication slice; cached aggregation and public counts remain here for the next slice.

- [Aggregate and cache instance activity metrics](aggregate-and-cache-instance-activity-metrics.md): bounded aggregation, shared cache, publication and maintenance pruning follow-up.
- [Accept instance activity in the bundled browser](accept-instance-activity-browser.md): exact `10ddf66` bounded browser acceptance passed; uncommitted harness pending parent review.

## Final browser evidence — pending parent review

- Exact default-feature application `10ddf66caf1fe8714f7c41ac4a1a0b8ceaf7fe34`, task-only PG14.23, migrations 1–6, restricted runtime/writer attachments and grants verified.
- Actual pinned public sidebar, v2 usage and initial metadata agree: empty history **0**, real confirmed login today still **0**, simulated historical bucket **1**; NodeInfo publishes **1/1** for the 4/24-week windows. Public navigation and reload both passed.
- Limited federation also passed actual browser navigation/reload: sidebar/v2/initial **0**, NodeInfo raw **1/1**.
- Historical membership was created by the real authentication recording helper today, then its bucket/member date moved to yesterday in task setup only. **No real historical backfill, observed midnight rollover, production-clock change or deployment claim.**
- Focused existing activity library **3 passed / 7 ignored**; pinned daily-activity source contract **1 passed**; harness regressions **2 + 8 passed**. No full matrix or main HTTP rerun.
- Sanitized screenshots, real XHR/fetch observations, all-source manifest (6,480 tracked files/symlinks), binary/image hashes, grants, commands and cleanup: `target/activity-browser-evidence/evidence/`; [recipe and boundaries](../../tools/activity-browser/README.md).
- Owned resources/credentials cleaned. Delegation unavailable at nesting limit; no independent review claimed. Leave parent/subissue open and changes uncommitted for parent review, then combined sweep.

## Evidence

- Mastodon 4.6.5 records user IDs in daily `activity:logins` unique sets when an account is approved and when a confirmed user returns, then unions the preceding 4 or 24 weeks for instance metrics.
- `rustodon.social` displayed `0 active users` although one enabled, confirmed local user had signed in within 30 days and posted that day.
- `GET /api/v2/instance` returned `usage.users.active_month: 0`.
- At the original report, `src/main.rs` constructed runtime instance metadata with `active_month: 0` and `active_halfyear: 0`; the serializers only forward those values (apart from limited-federation suppression).
