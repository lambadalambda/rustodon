# Implement authenticated user streaming

## Summary

Serve the first-party authenticated `user` WebSocket stream.

## Requirements

- Emit committed status update/delete, notification, and conversation events
  from durable PostgreSQL/outbox state.
- Authenticate tokens and terminate streams on revocation or owner disablement.

## Acceptance Criteria

- Reconnection and duplicate-event tests are deterministic for the pinned web client.

## Progress

- Added authenticated WebSocket support for `/api/v1/streaming` and its trailing-slash form.
- Matched the pinned client envelope, subscription commands, query/header token handling, stream
  scope rules, binary-message close behavior, heartbeat pings, and periodic token revalidation.
- Added immutable per-recipient stream events to the existing PostgreSQL outbox; the durable-job
  dispatcher excludes these rows so polling does not consume them.
- Wired committed status create/edit/delete, notification, and conversation changes to stream-event
  recording, including the author-side direct-message conversation.
- Kept filtered notifications out of the notification stream until unfiltering, then emit the
  pinned `notifications_merged` event; private and limited status updates are restricted to home
  feed recipients.
- Added protocol unit tests and an ignored PostgreSQL integration test covering deterministic
  duplicate insertion, replay, and non-dispatch behavior.
- Added the stream replay test to `tools/mastodon-fixture operational-schema-test` and verified it
  against the pinned PostgreSQL fixture.
- Added an ignored end-to-end WebSocket fixture test covering query-token authentication, user
  subscription, duplicate insertion, and the serialized delete envelope.
- The fixture task now pins Rust 1.97.1 and runs both stream database and WebSocket integration
  tests.
- Verified with 142 library tests, the complete `cargo +1.97.1 test --quiet` suite, formatting, and
  strict Clippy.
- Corrected limited/direct status fan-out to explicitly mentioned local followers,
  matching Mastodon 4.6.5's `FanOutOnWriteService`; the restored-fixture schema
  regression proves both recipient rows. The 27-test schema integration, 26-test
  worker integration, full local check, and all 18 guarded differential cases pass.
- Serialized stream-event writes and cursor reads with a transaction-level
  PostgreSQL advisory lock so identity values allocated by an uncommitted
  transaction cannot create a cursor gap. The operational fixture proves a
  reader and later writer wait for the earlier commit and replay both events.
- Original-status deletion now emits `delete` events for each soft-deleted
  reblog wrapper as well as the original status, matching Mastodon's recursive
  removal fan-out; restored schema coverage proves the complete local lifecycle.
- Incoming remote Note Create/Update/Delete and Announce/Undo writes now emit
  transactional authenticated user-stream events, including remote reblog
  wrappers removed by Note Delete. Restored worker coverage proves explicit and
  URI-only Undo paths; the broader client stream contract remains open.
- Local suspension and self-service account deletion now emit an atomic `kill`
  system event. Connected WebSockets poll that event without requiring a
  subscription and close with code 1000; the restored fixture covers this live
  suspension path.
- The compatibility audit found that Mastodon's `direct` stream is not yet
  accepted and conversation events are currently routed to `user`; add a
  dedicated `direct` subscription target and regression coverage before closing
  the authenticated streaming contract.
- Added the dedicated `direct` subscription target, scope enforcement, and
  conversation routing without falling back to `user`. User-stream status
  fan-out now applies the pinned home-feed language, reply, mute/block,
  domain-block, exclusive-list, mention, and reblog-author filters. Unit and
  restored schema verification pass; full client filtering and live peer
  convergence remain open.
 - Explicit OAuth token revocation now records a durable token-scoped
   `kill:token` event. Connected streams close only for the revoked token while
   sibling tokens remain connected; password resets and browser-session deletion
   emit the same event. The restored fixture proves revocation, reconnect
   rejection, and fixture-state cleanup.
- Extended the restored WebSocket integration to reconnect after an initial
  event batch, insert one post-handshake event, and prove the reconnect receives
  only that unseen event without replaying earlier rows. The deterministic
  duplicate/reconnect case passes as part of the operational-schema integration;
  the issue acceptance criteria are satisfied. Full pinned-client filtering and
  live peer convergence remain external follow-up evidence under the v1
  acceptance and Mastodon peer-federation issues.
