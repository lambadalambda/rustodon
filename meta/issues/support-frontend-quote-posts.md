# Support quote posts from the bundled frontend

## Summary

Mastodon 4.6.5 exposes a prominent “Boost or quote” action. Rustodon now parses the frontend's `quoted_status_id`, persists and federates the quote lifecycle, and rejects invalid intent atomically; final heavy acceptance remains deferred.

## Requirements

- Accept the bundled frontend's quote-status creation parameters and persist Mastodon-compatible quote state atomically with the new status.
- Enforce quoted-status visibility, blocks, lifecycle, approval policy, and self/remote authorization rules.
- Emit the required local stream, notification, counter, and durable ActivityPub quote/authorization effects.
- Reject unsupported or invalid quote requests clearly; never silently publish the user's commentary without its intended quote.
- Preserve existing quote read/serialization and deletion/update behavior.

## Acceptance Criteria

- Choosing Quote in the bundled frontend, composing text, and submitting produces a status that visibly contains the intended quoted status after response and reload.
- Remote and approval-required quote paths converge through the supported ActivityPub representation.
- Hidden/deleted/blocked/denied targets cannot be quoted or disclosed.
- Differential, worker, and browser coverage includes create, approval/denial, update, delete, and failure rollback.

## Status

Completed on 2026-09-16 at the user's explicit request. The implementation,
focused normal tests, pinned-source contracts, offline harness checks, and bounded
independent blocker/high review are complete. Restored-schema, worker,
differential, browser, cutover, and peer fixture execution remains deferred to
the final combined sweep and is not claimed as completed evidence here.

## Evidence

- Exact-source contracts pin Mastodon revision `1440d55b139e39ec722c2a3db7f60b66cd889048` and cover the bundled compose payload/reload renderer plus Rails create, policy, approval, update, delete, counter, and federation boundaries. All eight contracts pass on this checkout when commands use the temporary macOS Paperclip compatibility patch; the wrapper restores `src/paperclip.rs` byte-for-byte after each run.
- The writer contract now requires only `SELECT, INSERT, UPDATE` on `public.quotes`, `USAGE` on `public.quotes_id_seq`, and column-scoped `UPDATE` on `rustodon.durable_jobs(lease_expires_at, updated_at)` for the send-time lease fence. The restored-schema `schema-read-test quote_lifecycle` selector checks this privilege/default contract rather than executing the lifecycle; startup/preflight cases reject quote `DELETE`, sequence `SELECT`/`UPDATE`, table-wide durable-job `UPDATE`, unrelated durable-job columns, and unrelated elevated privileges. The sequence grant follows the pinned schema default `timestamp_id('quotes'::text)`, which consumes `quotes_id_seq`.
- The ignored `quote_lifecycle` differential selector covers frontend-form creation, self quotes, real reblog-to-original normalization, remote pending state, private visibility downgrade, blank/omitted policy defaults, invalid private/reblog policy nonmutation, revoke authorization/nonmutation, policy/deleted/blocked/direct failures and full state rollback snapshots, direct mention success, same-key replay, response-normalized commentary update with status-edit persistence, delete, counters, exact persisted quote state, and exact semantic durable quote notification, QuoteRequest, author-stream, and update intents. It is registered in the required differential inventory but has not been run against the final tree.
- The ignored `quote_lifecycle::quote_federation_lifecycle` worker selector uses the restricted worker-writer pool and drives allowed/denied embedded QuoteRequests, a scalar signed fetch with same-job `503` retry and target-owner key assertion, signed QuoteRequest/Reject/Accept HTTP delivery, decision replay, pre-attributed remote Accept/Reject, authorization Delete/revocation, remote quoting-Note deletion cleanup, exact target and legacy-transition counters, transition-specific stream/status-update intents, signed original-payload forwarding, conflicting approval replay, wrong-actor fail-closed behavior, and block/terminal-transition suppression of reclaimed actively leased deliveries through the durable executor. HTTP signature verification remains outside this selector. It compiles as executable intended evidence but has not been run against the restored final-tree fixture.
- The authenticated Chromium harness now creates a target with Alice, chooses the pinned frontend's `Boost or quote` action, verifies the nested target in the composer, timeline, permalink, and after reload, and fails closed at each quote checkpoint. It records both generated IDs, deletes quote before target, and the cutover rollback restores the quote and mention sequences while deleting only those exact rows. The SQLite rollback double now mutates and compares both sequences rather than only source-checking mention restoration. Offline selector, JavaScript, ordering, failure-matrix, ID-capture, cleanup, and rollback harness tests pass; the real browser/cutover lane has not been run.
- Mastodon 4.6.5's bundled frontend includes `quoted_status_id` in the `POST /api/v1/statuses` payload when the user submits a quote.
- The implementation now parses and fingerprints `quoted_status_id` only on status creation, canonicalizes reblogs, validates policy/visibility/blocks/direct mentions in the status transaction, permits non-distributable targets only for their own author, and creates local accepted or remote pending quote state with silent access, counters, streams, notifications, and durable federation effects. Quote delivery carries exact quote/request/status identity into a final send-time database fence whose relationship/account/status/quote locks span the bounded signed HTTP attempt and suppress sends after either-direction blocks. Terminal transitions cancel pending outbox and safe unleased/expired durable request and decision jobs; actively leased retries remain intact and fail closed at the final fence. Invalid quote intent rolls the entire creation back instead of publishing standalone commentary.
- The ActivityPub implementation adds durable QuoteRequest/Accept/Reject/QuoteAuthorization handling, strict identity and host bindings, signed SSRF-safe bounded scalar-instrument dereference with target-owner signing and retry/permanent classification, embedded-instrument preflight, incoming Note quote reconciliation (including ID-less Tombstone removal without dereference), idempotent accepted-boundary counters, Mastodon-compatible legacy update counters, update convergence, durable Delete-before-attachment authorization markers, authorization revocation, signed Delete forwarding over the quoting status' reach, and local revoke/delete behavior. These are executable source changes; heavy peer and restored-database convergence are not claimed as run evidence.
