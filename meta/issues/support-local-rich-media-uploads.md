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
- Then wire v2 acceptance and pending/ready/failure polling, preserving account
  lifecycle locking, current focus/description edits, exact artifact ownership,
  retry idempotency, and commit-ambiguity reconciliation.
- Finally verify real composer upload, preview/playback, posting, and reload.
  Do not close this issue merely because the processor or persistence layer passes.
