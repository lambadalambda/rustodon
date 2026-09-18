# Support local rich-media uploads

## Current status — complete (2026-09-18)

Combined persistence/worker/restricted HTTP/browser acceptance is complete. All
24 external MIME types passed HTTP; recorded fault tests cover timeout, cancellation,
partial writes, deletion, abandonment and recovery, with committed replay retention
and successful retirement coverage. The exact-`6f44264` browser evidence below
completes ordinary local workflows. No unmet local acceptance blocker remains.

Parent explicitly approves closure and archival. This status supersedes earlier
“open”, “uncommitted”, “review pending” and closure-proposal instructions below;
those record historical handoff stages, not outstanding work. No gates were rerun
for this docs-only reconciliation. Actual transport-loss/power-loss simulation,
full fixture/release matrices and remote-media implementation remain unclaimed.
The [frontend rich-media parent](support-frontend-video-attachments.md) remains
**OPEN** for unimplemented remote caching/previews.

## Summary

Use the bounded media processor for local v2 composer uploads while preserving
Rustodon's staged metadata, durable cleanup, and asynchronous safety contracts.

## Historical closure proposal after the reviewed browser rerun (2026-09-18)

The [exact-6f44264 browser rerun](verify-local-rich-upload-browser.md) **passed**:
HEIC/AVIF/video/audio composer preview/playback **before attachment**, posting,
native decode/advancing playback and immediate same-URL reload in both uploading
and fresh owner browser contexts. All four test posts were followers-only; owner
cookie media reads/ranges succeed and fresh anonymous reads/ranges are denied.
Malformed upload terminal failure/retained row/raw cleanup also passed again.
Both earlier browser blockers are resolved by the reviewed committed fix.

Closure evidence mapping (not a claim all gates were rerun on this turn):

- Format acceptance/ordinary workflow: prior real-codec HTTP lifecycle covers all
  24 advertised external MIME types, including HEIF; this run supplies actual
  HEIC/AVIF/video/audio browser behavior through the pinned composer and worker.
- Fault/lifecycle safety: use the recorded persistence/worker/restricted HTTP fault
  results for timeouts, cancellation, partial writes, deletion, replay and recovery;
  this browser run additionally confirms retained malformed failure and raw cleanup.
- Integration: the earlier restricted HTTP/database results plus this browser run
  now cover pending, success, validation, preview/playback, reload and cleanup.

**Propose closure of the local parent and its completed bounded subissues once the
parent verifies the combined criteria/review evidence.** Browser/auth/HTTP no longer
have an outstanding browser blocker. Do not archive based only on the happy path;
retain the fault-matrix evidence boundary and verify any older review checkpoints.
Remote-media/cache/search issues and release/peer/full-matrix gates are separate
and remain unclaimed. These updates are documentation-only and uncommitted.

## Requirements

- Accept every rich-media format that the instance advertises through the
  bundled composer's v2 upload path.
- Preserve playable originals, generated preview artifacts, correct attachment
  type, metadata, and reload behavior.
- Do not run potentially long video/audio processing on an async web executor;
  stage input and process it under a bounded durable media job.
- Generalize media artifact manifests so audio can omit a small derivative while
  video and converted images install their required previews.
- Preserve account locking, idempotent publication, ambiguous-commit handling,
  media authorization, attachment limits, and durable cleanup.
- Return clear permanent validation errors for unsupported or over-limit media.

## Acceptance Criteria

- HEIC/HEIF/AVIF, supported video, and supported audio uploads reach a ready
  state, can be attached to a status, preview/play, and survive reload.
- Failure, timeout, cancellation, partial writes, deletion, and abandoned upload
  cases leave no untracked files or publishable invalid rows.
- Focused HTTP/database/browser regressions cover pending, success, validation,
  playback/preview, reload, and cleanup behavior.

## Implementation sequence

- [Persist durable local upload ownership](persist-durable-local-upload-ownership.md)
  first: an additive Rust-owned state/migration/privilege boundary, tested
  independently. No production migration as part of implementation.
- [Process and recover durable local uploads](process-and-recover-durable-local-uploads.md):
  worker, private raw filesystem, and cleanup/recovery only; no live HTTP v2.
- [Integrate local rich-upload HTTP](integrate-local-rich-upload-http.md):
  wire v2 acceptance and pending/ready/failure polling, preserving account
  lifecycle locking, current focus/description edits, exact artifact ownership,
  retry idempotency, and commit-ambiguity reconciliation.
- [Verify local rich uploads in the pinned browser](verify-local-rich-upload-browser.md):
  actual composer upload, preview/playback, posting, and reload. Initial blockers
  were fixed in reviewed `6f44264`; the exact-commit rerun now passes. See the
  closure proposal above rather than treating earlier partial evidence as current.

## Historical HTTP slice status (2026-09-18)

The [bounded local HTTP integration](integrate-local-rich-upload-http.md) is implemented
and left uncommitted for parent review. Real-codec HTTP coverage passes all 24 advertised
external formats plus pending/failure/ownership/deletion contracts, with focused database
and worker regressions. This is **not** browser acceptance; keep this issue open until
composer preview/playback, posting, and reload are exercised separately.

The subsequent evidence-only follow-up also passes the same HTTP/worker lifecycle
under actual narrow runtime/writer roles, with owner access limited to setup and
assertions. Real worker capability/readiness coverage passes with those roles;
no implementation or grant changes were needed. See the HTTP subissue and DEVLOG.
