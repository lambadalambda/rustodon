# Implement local hashtag control APIs

## Summary

Bounded API/database subissue of [Support hashtag and featured-tag controls](support-hashtag-and-featured-tag-controls.md), starting at d9f8ae1. Parent retains browser and complete compatibility evidence.

## Requirements

- Add tag lookup, follow/unfollow, feature/unfeature, and featured-tag create/delete using existing tables, projections, transactions and account locks.
- Match verified clean Mastodon 4.6.5 revision 1440d55b139e39ec722c2a3db7f60b66cd889048 normalization, authorization, unknown tag, ownership, duplicate and limit contracts.
- Serialize the per-account featured-tag limit; initialize and reuse existing public/unlisted featured counts maintenance.
- Add narrowly scoped writer grants and privilege preflight for existing relations/sequences only.
- Provide meaningful bounded public hashtag history from existing status/tag data if viable without new storage/jobs; explicitly report limitations.
- Exercise persisted HTTP state, dynamic home inclusion and existing stream fanout through real API mutations.

## Acceptance Criteria

- Focused TDD covers lookup, normalization, authentication/scopes/application-only/suspension, ownership, duplicate and concurrent limit handling, lifecycle counts and follow-state projections.
- Restricted runtime/writer PostgreSQL 14 end-to-end tests run on disposable bounded Linux NAS resources if accessible; distinguish authored tests from executed gates.
- Independent correctness and compactness review, at most two substantive review/fix rounds; leave all changes uncommitted.

## Boundaries

No browser work, schema, new job or new protocol. ActivityPub AddHashtag/RemoveHashtag peer projections are deferred to the parent, not implemented by this local slice. Historical stream cleanup remains in [Define hashtag stream history cleanup](define-hashtag-stream-history-cleanup.md). Differential evidence requires actual pinned-source-derived expectations and executable fixtures; synthetic tests are not differential evidence.

## Implementation and evidence (2026-09-19)

Implemented locally, uncommitted; independent parent review is still required. Parent and subissue remain open.

- Seven routes plus slash aliases, scope/user checks, current relationship projections, unsaved unknown lookup, pinned normalization and name-vs-tag duplicate behavior.
- Transactional account-locked follow/feature operations, serialized ten-feature limit, owner-scoped deletion; narrow INSERT and sequence-USAGE grants with positive and missing-grant preflight tests.
- Featured counts initialize from public/unlisted statuses and reuse create/edit maintenance. A failing HTTP lifecycle test exposed missing local-delete decrement; deletion now calls the existing helper.
- Header history aggregates current public non-boost statuses/distinct authors over seven UTC days, excluding deleted statuses and suspended/silenced authors, with a two-second database statement timeout. This is not Redis retention/registration-time/trend parity; other existing search/followed/suggestion histories remain unchanged.
- Existing stream fixture now follows/unfollows through HTTP using restricted runtime/writer roles. Its stale silenced-author delete expectation was aligned with existing cleanup fanout; no engine change.

Executed sequentially on disposable NAS PG14 and immutable 7203 tools image:

- Initial route regression RED (missing route), then GREEN. Count-delete regression RED, then GREEN.
- Default library tests: **343 passed, 7 ignored**; all-feature library tests: **354 passed, 21 ignored**.
- Restricted-role HTTP controls fixture: **1 passed**, covering scopes/application-only/suspension, persistence, normalization, ownership, duplicates, concurrent limit/follows, initialization/create/edit/delete counts, public history/profile reads, rate limiting/idempotence, and revoked-grant preflight failures.
- Reused real WebSocket/home lifecycle fixture: **1 passed**, including API-driven follow and unfollow removal.
- Source contracts: **9 passed**. Clean pinned source verified with host verification script, then mounted read-only for the test binary. Tools image lacks git, so the combined wrapper could not execute its verification step.
- All-target/all-feature `cargo check`, all-feature library strict Clippy, formatting and diff checks passed. Full all-target strict Clippy remains blocked by pre-existing lints in media_processor, paperclip test helpers, and worker/local_uploads tests.

Two-request concurrency passed. Initial six-request stress runs were SIGKILLed within the 6-GiB test container; no six-request stress pass is claimed. No browser, Rails HTTP differential, full fixture matrix or peer gate ran. Existing differential cases cover collection reads/search, not this new mutation contract; source contracts are not differential evidence. AddHashtag/RemoveHashtag and historical stream cleanup remain excluded. Independent review could not be delegated from this child (nesting limit); parent must review before commit.

Evidence: ignored `target/hashtag-controls-evidence/` and NAS `/srv/workspaces/rustodon-hashtag-d9f8ae1-alice/evidence/`. Intended source hashes matched. Task-owned PG container/anonymous volume and internal network removed; shared caches/reference checkout untouched. Reference remained clean.

## Review 1 ce0d fixes — ready for parent review 2

Review 1 reported no high findings and two directly relevant medium findings. Both are resolved without expanding scope:

- Moved the tag-follow INSERT and tag/featured relationship sequence-USAGE grants adjacent to featured-tag grants, before the existing COMMIT. A regression test failed on the previous placement and now passes.
- Follow rate-limit idempotence now uses one normalized `EXISTS` relationship query, not a tag projection/history aggregate. Response history remains a single bounded post-commit readback. Explicit failure policy: history/readback failure returns HTTP 500, never fabricated zero history; the committed idempotent relationship remains persisted and may be retried. An actual table-lock/statement-timeout test failed with zero persisted follows before the fix, and now proves persistence, no duplicate on repeated failed readback, and successful retry after unlocking.
- Added direct fixture history assertions for all seven UTC days, inclusive oldest midnight, one microsecond before the oldest boundary, midnight bucket separation, future exclusion, distinct authors versus repeated uses, public-only visibility, and exclusion of silenced/suspended authors, boosts and deleted statuses.

Review-fix verification on the same bounded disposable NAS PG14/tool-image lane:

- Focused controls/grants/HTTP tests: **3 passed** (`review2-final-http.log`), including restricted runtime/writer credentials and four negative revoked-grant checks.
- Existing API-driven home/WebSocket fixture: **1 passed** (`review2-streams.log`).
- Host verified clean exact pinned source; read-only source contracts: **9 passed** (`review2-verified-source.log`, `review2-source-contracts.log`).
- Strict all-feature library Clippy, formatting and diff checks passed. No stress investigation or unrelated lint changes.

Ready for independent parent review 2; no second independent review is claimed by this child. Still uncommitted, no production access/push, no new schema/cache/job/protocol/browser work. Existing history/peer/differential limitations remain. Review-fix task PG/anonymous volume/network removed; source/evidence retained in the existing ignored task evidence directories.
