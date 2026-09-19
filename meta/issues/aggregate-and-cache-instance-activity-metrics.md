# Aggregate and cache instance activity metrics

## Current disposition — accepted after parent review 980993

Accepted and archived on parent approval. Review `980993` found no high-severity
blocker and supports focused closure from the combined reviewed recording,
cache, pinned-source, restricted PostgreSQL/worker/main-HTTP evidence and actual
browser acceptance on exact app `10ddf66caf1fe8714f7c41ac4a1a0b8ceaf7fe34`.
This disposition supersedes historical open, review-pending, browser-deferred and
leave-uncommitted statements below; earlier logs remain historical evidence,
not fresh execution claims. The wider combined acceptance sweep remains pending.

### Accepted aggregation boundary

Criteria were reviewed against exact UTC/distinct/history tests, bounded pruning
and renewal races, restricted worker integration, cache tests and actual main HTTP
(including rules/manifest availability follow-up). Exact `COUNT(DISTINCT ...)`,
not approximate reference HyperLogLog, is intentional safer counting. The linked
browser slice now proves ordinary/limited publication and reload on exact `10ddf66`.
Its nonzero history is explicitly a real TODAY helper-recorded bucket moved to
yesterday in setup, not observed midnight or actual historical backfill. Earlier
359 passing library tests had **30 ignored**; those ignored tests are not passes.

## Summary

Bounded aggregation/publication and cleanup slice on 3df2724 after migration 6.
Parent: [Compute real instance activity metrics](compute-real-instance-activity-metrics.md).

## Requirements

- One finite-timeout SELECT computes exact distinct users in UTC [D-28,D) and
  [D-168,D), excluding buckets expired at the captured as-of time. No current
  user status join or historical backfill.
- Shared per-WebState singleflight cache, bounded TTL and UTC rollover invalidation,
  separate from static runtime configuration. Fail closed with 503 without a valid
  cached result; never silently substitute zero. All public/initial metadata agrees.
- Limited federation suppresses only v2; NodeInfo publishes both raw counts.
- Reuse maintenance prune with writer credentials, no new job/grants/migration.
  Lock buckets and atomically delete bounded member chunks and empty expired buckets.
- Document empty rollout and exclusion of today's activity until UTC rollover.

## Acceptance Criteria

- TDD boundaries, exact cross-day uniqueness, expiry, historical status independence;
  cache singleflight/TTL/rollover/failures; endpoint privacy; pruning renewal races.
- Actual restricted PostgreSQL 14 integration and main runtime tests, sequential,
  resource-bounded disposable NAS7203 fixtures. Browser sidebar follow-up separate.
- Independent review at most two rounds; leave uncommitted. No full matrix claim.

## Status

Open; implemented and focused gates passed, uncommitted pending independent parent review.

## Implementation and verification

- Implemented one timeout-bounded exact aggregation SELECT, a shared singleflight
  60-second cache with UTC rollover rejection (including refreshes crossing
  midnight), and 5-second failure coalescing. No stale fallback: valid cache is
  served without refreshing; invalid/absent counts fail closed with 503.
- Dynamic `InstanceActivityCounts` lives on the instance projection, not static
  runtime config. Main, v2, NodeInfo, and initial frontend metadata use it. The
  pinned sidebar fetches the same v2 API; actual browser rendering is deferred.
- Existing maintenance handler uses its existing optional writer pool, leaving
  runtime SELECT-only. Pruning caps 100 bucket locks and 1,000 total members,
  deletes empty expired buckets atomically, and skips recording locks. Recording
  retries bucket creation when cleanup wins before the bucket lock. No new
  migration, privilege, protocol, durable job, or backfill.
- README, `docs/v1-scope.md` API semantics, and `docs/testing.md` rollout/gate
  instructions updated. Today is excluded; new activity contributes after UTC
  rollover, not immediately after first login.

### Actual gates

Disposable NAS worker, PostgreSQL **14.23**, immutable tool image **7203e0222e2b**;
sequential containers limited to 4 CPU / 6 GiB / 512 PIDs / 870s (900s host wall
bound), PostgreSQL 2 CPU / 1 GiB / 128 PIDs. No production access or push.

- Restricted-role activity integration: **7 passed** (existing recording tests
  plus exact aggregation/history, bounded pruning/renewal, and lock-observed
  recording versus cleanup race).
- Registered maintenance-handler restricted-role integration: **1 passed**.
- Actual main-process HTTP integration: **1 passed**, covering both federation
  modes, API/initial-document values, NodeInfo raw counts, shared warmed cache
  under table locks, and cold-cache 503s. Not the entire startup lane.
- Ordinary all-feature library: **359 passed / 30 ignored**; serializer tests:
  **20 passed**. All-target/all-feature `cargo check`: passed.
- Focused strict Clippy (library, main binary, startup test), formatting, and diff
  checks: passed. Full all-target strict Clippy was attempted and is blocked by
  existing `paperclip` test-helper needless-pass-by-value and `local_uploads`
  test imports-after-statements lints. New lints were fixed; unrelated helpers
  were not changed.
- Initial TDD red for UTC/cache contracts observed missing implementations, along
  with existing macOS/Linux-only rustix compile failures. Green was run on NAS.
  Integration coverage was added alongside implementation, not a complete
  red-before-code sequence for every path.

### Evidence boundaries and attempts

- Ignored local evidence: `target/activity-aggregation-evidence/`; remote source,
  runner, and logs: `/srv/workspaces/rustodon-activity-aggregation-3df2724`.
- Initial source sync stopped at the tracked broken `public/500.html` symlink;
  re-synced tracked files excluding that link, then recreated that exact link.
  No ignored instance data or credentials transferred. Early compile attempts
  identified missing fixture source, unsupported reqwest JSON helper, and one
  projection fixture initializer; fixed and rerun.
- Main initially correctly refused synthetic recording-test accounts without
  signing keys. A fresh disposable restore resolved it; no startup safety was
  weakened. Main helper allows 30s startup and reaps children directly (tool
  image lacks external `kill`). An outer SSH call timed out during the final
  serial sequence; bounded container completed successfully, final log checked.
- Independent review could not be launched: task tool rejected delegation with
  `nesting limit reached (depth 1 of 1)`. **Zero independent review rounds claimed.**
  Leave uncommitted and open for parent review; at most two review/fix rounds.
- Focused browser sidebar is the next slice. No full matrix, source-contract,
  peer, release, or deployment claim.

- Final source/fixture SHA-256 manifest verified against the worker checkout. Task-owned PostgreSQL container, volume, and network removed; source/evidence workspace retained.

## Review 1 follow-up

Parent review found no high-severity issue. Fix only the directly introduced
availability coupling: manifest and rules expose no activity counts, so use a
separate static-instance path through the existing projection loader. Extend the
actual main HTTP test: cold activity failure must leave rules/manifest at 200,
while count-bearing routes remain 503. No cache-policy changes, other medium
findings, or extra gates. Leave uncommitted for parent review 2, then browser.

### Review 2 handoff

- Added a small `WebState::static_instance` path through the existing projection
  loader and shared runtime config. Only manifest and rules use it; unused
  activity fields are defaulted but never serialized by those endpoints. Their
  unrelated loader failures retain ordinary 500 behavior, not activity 503.
- Count-bearing instance, NodeInfo, and frontend routes and all TTL/failure/cache
  code are unchanged.
- TDD: actual-main regression failed with rules **503 instead of 200** under a
  cold-cache activity-table lock; after the fix it passes with rules, `/manifest`,
  and `/manifest.json` **200**, while v2, NodeInfo, and frontend remain **503**.
- Final focused PG14/NAS7203 main regression **1 passed**; focused strict Clippy
  (lib/main/startup test), formatting and diff checks passed. Initial fixture
  setup raced PostgreSQL readiness; retried after readiness, then observed the
  expected red. Fixed Clippy's explicit-default-type lint before final green.
- Reused task-owned disposable fixture with the same CPU/memory/PID/wall bounds;
  no additional gate lanes, other review findings, or production work. Evidence:
  `target/activity-aggregation-evidence/review2/`. Resources cleaned afterwards.
- Left uncommitted for parent review 2 only; browser remains the parent's next
  activity. This is the response to review 1, not a claim that review 2 occurred.
