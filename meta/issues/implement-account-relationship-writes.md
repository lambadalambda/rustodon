# Implement account relationship writes

## Summary

Implement follow, follow-request, block, and mute lifecycles.

## Requirements

- Preserve follow options/languages, local locking, remote pending state,
  counters, notifications, outbox intent, and expiring mute behavior.
- Add follow-request list/accept/reject and follow/unfollow/block/mute APIs.

## Acceptance Criteria

- Differential retries and federation transitions match Mastodon without
  duplicate relationships or counter drift.

## Progress

- Added Rails-compatible POST follow/unfollow routes with `follow`/`write:follows`
  scope alternatives, local direct-follow versus locked/remote request
  selection, follow option updates, advisory locking, and relationship reloads.
- Follow writes maintain source/target account counters, create the supported
  local follow notification, and safely repeat create/remove operations without
  duplicate rows or counter drift.
- Owner-role schema coverage and the guarded write differential case pass for
  option preservation, duplicate follow/unfollow requests, relationship JSON,
  notification restoration, and counter restoration.
- Added Rails-compatible POST block/unblock and mute/unmute routes with
  `write:blocks` and `write:mutes` scope alternatives. Local blocks remove
  both-direction follow relationships transactionally; mutes preserve the
  notification flag and store nullable expiration.
- Owner-role schema coverage now passes 16/16 tests, and the guarded
  nine-case differential suite covers duplicate block/mute retries and
  relationship restoration.
- Added follow-request authorize/reject and remove-from-followers routes. Local
  authorization preserves request options/URI, moves the row to follows,
  updates both counters, removes the dependent request notification, and emits
  the supported local follow notification. Reject and remove operations are
  retry-safe.
- Owner-role schema coverage now passes 17/17 tests, and the guarded nine-case
  differential suite covers request decisions, incoming-row restoration,
  counters, notifications, and retries.
- Follow-request authorization now emits the remote ActivityPub `Accept` only
  when the requester is an ActivityPub account; OStatus requests still become
  local follows without an outbound Accept. Restored-fixture coverage proves
  both protocol paths.
- Relationship teardown now removes the corresponding follow,
  follow-request, favourite, and reblog notification rows transactionally.
  Differential interaction coverage snapshots affected target notifications
  and verifies create/remove cycles return to the Rails baseline.
- Blocking now clears the matching notification-permission exception before
  inserting the block; owner-role schema coverage proves the cleanup and the
  least-privilege writer grants the required delete capability.
- Notification policy checks now ignore expired notification-hiding mutes;
  isolated owner-role coverage proves an expired mute does not drop a new
  notification.
- The issue remains open for active mute expiry/worker cleanup, remote
  federation transitions, complete outbox intent, and concurrent relationship
  differential proof.
- Timed mute writes now record an atomic `rustodon.mastodon.delete_mute`
  outbox event, cancel pending events when the mute is removed or renewed, and
  the worker can execute expiry through a separately configured writer pool.
  Owner-role schema, worker integration, and focused differential coverage
   prove the path while the default runtime role remains read-only. The issue
   remains open for remote relationship transitions and complete outbox intent.
   Local block teardown and concurrent relationship proof are covered
   separately.

## Completion

- The required follow, follow-request, block, mute, counter, notification,
  retry, idempotency, and local outbound-intent behavior is covered by the
  current Rails-versus-Rust relationship differential case, restored Mastodon
  schema integration, and worker integration. Remote peer convergence remains
  tracked by the ActivityPub federation issues rather than this local write
  issue.
