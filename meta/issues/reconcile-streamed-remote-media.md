# Reconcile streamed remote media after caching

## Summary

A newly streamed status with a tall remote attachment initially renders as a horizontally filled center crop, and its lightbox image fails to load. After refreshing, the same status shows the complete attachment and the lightbox works.

## Requirements

- Compare the media attachment serialized into the live stream with the refreshed REST representation.
- Determine whether the stream event races remote-media processing, uses stale metadata, or omits a required follow-up update.
- Reproduce the behavior with a focused regression before applying the smallest fix.
- Preserve bounded remote fetching, local-only media serving, stream authorization, and frontend-compatible media metadata.

## Acceptance Criteria

- A streamed remote status converges to the same usable attachment representation as a refreshed status without requiring a page reload.
- The attachment's aspect metadata and local original/preview URLs are valid when presented to the frontend.
- Applicable media, streaming, and worker regressions pass.
- The fix is independently reviewed and verified through the normal frontend flow.

## Notes

- The exact affected post URL is not yet recorded.
- Keep deployment-specific evidence in the private operations note.

## Read-only diagnosis

- Recent attachment metadata identifies one matching tall remote image: the
  status stream event was recorded about 665 ms before its local WebP cache and
  944×1664 geometry were installed.
- The initial attachment row therefore existed when the `update` event became
  visible, but its local file and `meta.original.aspect` were not ready. This
  matches the frontend's fallback center crop and the failing local lightbox
  URL.
- Media processing completed successfully, which explains why a later REST
  refresh returned complete geometry and working local media.
- No `status.update` stream event followed the media-cache installation, so the
  already-rendered frontend status had no way to converge without reloading.
- Diagnosis inspected identifiers, timestamps, media metadata, and event types
  only; no post body or application state was read or changed.

## Implementation

- Successful remote attachment installation now records a follow-up status
  reconciliation in the same PostgreSQL transaction as the final conditional
  cache-metadata update.
- The worker obtains the attachment's `status_id` with `UPDATE ... RETURNING`
  and delegates to the existing status and notification stream audience
  helpers. The websocket continues to re-authorize and serialize the current
  full status as frontend protocol event `status.update`; media caching does
  not alter the status's semantic edit timestamps or federate an Update.
- Stream logical keys use an explicit `:media:<attachment-id>` namespace,
  making one installation idempotent per recipient without overloading the sign
  of semantic edit versions. Both key kinds share the same status and
  notification audience implementations.
- The same status audience helper records `status.update` for each active boost
  wrapper. A recipient who follows only the booster therefore receives a
  wrapper update whose dynamically serialized nested original contains the
  installed media; wrappers created later already serialize current metadata,
  and deleted wrappers remain suppressed.
- The event write remains behind the final URL/domain/deletion checks and the
  successful file write. Existing failed-fetch, stale/deleted, denied-policy,
  and lease-fence regressions compositionally cover those shared gates.
  Metadata and stream rows commit together. After an ambiguous commit result,
  the worker checks the exact installed metadata marker: confirmed commits are
  acknowledged, while rolled-back final attempts remove files before entering
  the existing failed/dead-letter state.

## Verification

- TDD red, before the first product change: the focused Linux worker
  regression failed after successful media processing with `left: []` versus
  the expected recipient `status.update` event. The original exact regression
  then passed (`1 passed`).
- The blocker regressions now also assert a wrapper-only recipient, an explicit
  non-colliding media key, exact max-attempt pre-/post-commit event/file/state
  outcomes, and a delivered `status.update` payload with local original and
  preview URLs plus `meta.original.aspect`.
- Existing focused regressions compositionally cover failed and denied fetches,
  stale/deleted attachment state, and lease fencing/recovery through the same
  gates. The earlier four-test Linux `activitypub_media_fetch` baseline passed
  before the blocker regressions were expanded.
- Current-tree `cargo fmt --check` and `git diff --check` pass. Native Rust
  checking reaches the changed worker code but remains blocked only by the
  expected Linux-only `rustix` `openat2`, `ResolveFlags`, and `NOATIME` APIs.
  The shared Podman VM did not answer either the API or direct SSH during the
  final rerun, so the expanded Linux worker/streaming suites and strict Clippy
  are not claimed as passed in these notes.
- Independent final-diff review found no correctness blockers in the logical-key
  namespace, shared audience helpers, wrapper fanout, ambiguity reconciliation,
  or delivered payload coverage.
- A deployed browser/live-instance replay has not been run in this workspace;
  normal frontend-flow verification remains pending.
