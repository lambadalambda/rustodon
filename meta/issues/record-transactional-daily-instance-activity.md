# Record transactional daily instance activity

## Current disposition — accepted after parent review 980993

Accepted and archived on parent approval. Review `980993` found no high-severity
blocker and supports focused closure from the combined reviewed recording,
cache, pinned-source, restricted PostgreSQL/worker/main-HTTP evidence and actual
browser acceptance on exact app `10ddf66caf1fe8714f7c41ac4a1a0b8ceaf7fe34`.
This disposition supersedes historical open, review-pending, browser-deferred and
leave-uncommitted statements below; earlier logs remain historical evidence,
not fresh execution claims. The wider combined acceptance sweep remains pending.

### Accepted recording boundary

Criteria were reviewed against the recorded eligibility/transaction/expiry tests,
fresh and upgrade migration/bootstrap/grant checks, restricted HTTP tracking and
round-2 fixture follow-up below. Returning tracking covers implemented interactive
HTML, credentials, settings and session hooks, not all API/lifecycle tracking.
The reusable activation helper is not a new approval API. Empty historical rollout
is intentional; no real backfill. Exact unique memberships replace approximate
reference HLL membership semantics without claiming full tracking parity.

## Summary

First bounded slice of [Compute real instance activity metrics](compute-real-instance-activity-metrics.md), based on `16c751c`: operational storage and existing authentication hooks only. Cached aggregation and public counts are a separate next slice.

## Requirements

- Verify clean pinned Mastodon `1440d55b139e39ec722c2a3db7f60b66cd889048`: daily PFADD unique user IDs on confirmed+approved first transition and confirmed returning login. Future aggregation windows are `D-28 <= day < D` and `D-168 <= day < D`, excluding today.
- Rust-owned exact daily/user uniqueness; no public FK/cascade or historical filtering by current deletion/suspension/disabled state. Use bucket-level expiry reset on every eligible write, matching exact source `6.months.seconds`, not 24 weeks or per-row approximation. Explain duration and efficient schema choice.
- Check current migration 5 before allocating 6. Update known writer grants, operational validation/fingerprint and standalone bootstrap. Separate restricted read rights for future aggregation; no broad public grants.
- One transactional helper for eligible local confirmed+approved creation, approved confirmation transition, confirmed approved admin bootstrap, and successful complete password/TOTP/backup authentication after all checks. Reusable for future approval transition, tested by simulation; no new approval API.
- Retained browser returning activity uses existing safe interactive browser/app user tracking hook, strictly more than 24 hours; not every bearer request. Preserve media GET/HEAD cookie authentication without session touches or mutations. Match pinned UserTracking lifecycle; stop for checkpoint if complex auth architecture is needed.
- No Redis, jobs, protocol, backfill or public count implementation. Document empty activation history. Cleanup belongs in existing maintenance pruning in subsequent cached aggregation slice.

## Acceptance Criteria

- TDD focused tests prove eligibility, failed login and partial 2FA exclusion, repeat users, UTC boundaries, exact bucket expiry reset, transactional retries/rollback, and unchanged historical membership after current user disable/delete/suspend.
- Fresh and upgrade migration, bootstrap and restricted-role validation tested with bounded serial disposable PG14 on NAS7203 tool image, never production; record actual evidence separately from unrun gates.
- Parent independent review before any commit; leave changes uncommitted for this request. Maximum two substantive review/fix rounds. Stop/report before major expansion beyond three subsystems; no broad auth refactor or full matrix claims.

## Notes

- Initial checkout clean at `16c751c`. Local reference HEAD verified exact and `git status --porcelain` empty.
- Scope checkpoint already authorized operational migration + auth hooks; aggregation deferred.

## Implemented design

- Migration 6 follows the verified version-5 prefix. `activity_buckets` stores a
  UTC day and one `expires_at`; `activity_members` has exact `(day, user_id)`
  uniqueness. No public FK or cascade, and no current-user joins in historical
  membership reads. Both tables are Rust-owned, without FKs; recording maintains
  their relationship in one transaction. The later maintenance prune must lock
  the bucket and remove members and bucket together.
- Every eligible write, including a duplicate, locks its bucket, clears any
  expired generation, and resets expiry from the actual PostgreSQL clock after
  acquiring that lock. It does not update every member or resurrect old members.
  UTC day and sign-in claim time are captured from PostgreSQL, never TOTP input.
- Pinned `ActivityTracker` uses PFADD then EXPIRE and exclusive ending dates.
  The exact TTL is **15,778,476 seconds** (182 days, 14:54:36), not calendar-month
  SQL arithmetic or 168 days. Pinned `Gemfile.lock` requires ActiveSupport 8.1.3.
  Read its cached gem source and executed `6.months.seconds`, obtaining 15778476;
  details and command evidence are in DEVLOG/ignored evidence.
- Activation accepts a pre-transition `was_eligible` flag and rechecks local
  confirmed+approved state under a user lock. Confirmed creation and approved
  confirmation use this helper; a simulated approval transition tests reuse.
  No approval API was added. Bootstrap retains the newly inserted admin ID and
  records it after migration in the same installation transaction. Verification
  reruns validate exact initial activity without refreshing it.
- Complete password/TOTP/backup login records after the final password fence and
  session/token insertion, before commit; the helper rereads current local
  confirmed eligibility. Earlier committed login audit/OTP work is not activity.
  Existing auth policies and the two-transaction architecture are unchanged.
- Returning activity checks confirmed locality and relies on the existing
  endpoint authorization policy. Under a user lock, only nil or strictly older
  than 24h `current_sign_in_at` claims activity. Previous/current timestamp
  updates and membership commit together; `sign_in_count` does not increment.
- Explicit interactive hooks: frontend HTML, `verify_credentials` API, existing
  required browser settings/session paths. Pinned API `require_user!` also calls
  tracking after its functional checks. This is a **bounded subset**, not full
  Rails controller tracking. No generic bearer middleware/repository lookup or
  media authentication writes were added.
- Reader/runtime receives only SELECT on the new tables. Known writer receives
  operational CRUD. Migration discovery/grants, catalog fingerprint, exact role
  validation, bootstrap and current fixture grant profiles were updated. No new
  grants on public tables.
- Rollout starts empty: no historical timestamp backfill. Fresh bootstrap's owner
  is a new activation. Expired buckets must be excluded from later aggregation
  even before physical cleanup. Cached aggregation, maintenance pruning and
  public v2/NodeInfo counts remain the next slice; their current values are
  deliberately unchanged.

## Execution status

- Implemented and left uncommitted for parent independent review. Issue remains
  open pending that review; no independent review or full matrix is claimed.
- TDD red migration-plan assertion and fresh bootstrap baseline rejection were
  observed, then green. Final bounded NAS7203/PG14 evidence: activity **4/4**,
  media GET/HEAD immutability plus explicit interactive HTTP tracking **1/1**,
  fresh bootstrap/verification/role checks **1/1**, v5 upgrade/roles/drift **1/1**,
  pinned activity source contract **1/1**, ordinary library **356 passed / 26
  ignored**, migration plan **1/1**, existing local-upload schema upgrade **1/1**.
  Fresh operational lifecycle **1/1** also passed. Focused strict Clippy passed.
- Full all-target Clippy has unrelated existing test-helper lints. Aggregate
  offline harness stopped at absent Node; standalone harness Python was absent
  from the tool image. Earlier shell peer/image/selector harness checks passed.
  These are explicit evidence boundaries, not extra implementation scope.
- Reproducible commands, failed attempts, final logs and source/catalog hashes:
  ignored `target/daily-activity-evidence/`; remote task workspace
  `/srv/workspaces/rustodon-daily-activity-16c751c-alice`. No production access,
  Redis, job, protocol, auth refactor or public count implementation.

## Parent review follow-up (round 2, tests only)

- Parent accepted production source with no high-severity findings. Address the
  medium fixture-date reliability finding: use PostgreSQL-relative simulated
  dates/expiry, while preserving exact UTC-midnight and 24h boundary assertions.
- Extend the existing HTTP fixture with due settings and `/auth/session`
  assertions if possible without new browser infrastructure. No production,
  aggregation or pruning changes. Re-run the focused four activity tests and
  restricted-role HTTP fixture, formatting/Clippy and source-hash verification.
- Leave uncommitted for parent review 2; retain prior logs as historical evidence.

### Round-2 result

- Implemented the two-test-file follow-up: PostgreSQL-relative dates and explicit
  past expiry, with exact UTC midnight preserved; due settings and `/auth/session`
  assertions fit the existing HTTP fixture and were added (not deferred).
- Restricted PG14/NAS7203 focused activity **4/4** and HTTP **1/1** passed. Focused
  strict Clippy, formatting/diff checks and **22/22** worker/source hashes passed.
  Only those two test files differ from the prior tested source manifest.
- Evidence retained separately in `target/daily-activity-evidence/review2/`;
  prior logs remain accurate and untouched. Task container/volume/network removed.
  No production source or aggregate/prune changes. Parent review 2 pending;
  still uncommitted.
