# Support local rich-media uploads

## Summary

Use the bounded media processor for local v2 composer uploads while preserving
Rustodon's staged metadata, durable cleanup, and asynchronous safety contracts.

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
  actual composer upload, preview/playback, posting, and reload. The bounded slice
  passes asynchronous upload/post and public video/audio playback, but finds
  unattached-preview and browser-cookie media authorization blockers. Keep open;
  do not close this issue merely because processor/persistence or public playback passes.

## HTTP slice status (2026-09-18)

The [bounded local HTTP integration](integrate-local-rich-upload-http.md) is implemented
and left uncommitted for parent review. Real-codec HTTP coverage passes all 24 advertised
external formats plus pending/failure/ownership/deletion contracts, with focused database
and worker regressions. This is **not** browser acceptance; keep this issue open until
composer preview/playback, posting, and reload are exercised separately.

The subsequent evidence-only follow-up also passes the same HTTP/worker lifecycle
under actual narrow runtime/writer roles, with owner access limited to setup and
assertions. Real worker capability/readiness coverage passes with those roles;
no implementation or grant changes were needed. See the HTTP subissue and DEVLOG.
