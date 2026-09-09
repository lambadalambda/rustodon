# Implement status social interactions

## Summary

Implement favourite, bookmark, and boost lifecycles.

## Requirements

- Add favourite/unfavourite, bookmark/unbookmark, boost/unboost, and by-account
  lists, always targeting the original status.
- Serialize/lock boost creation and maintain counters, notifications, outbox,
  authorization, and idempotency.

## Acceptance Criteria

- Concurrent and duplicate differential cases produce one compatible result.

## Progress

- Added authenticated bookmark and favourite create/remove handlers backed by
  the least-privilege `WriteRepository`, with Rails' POST `unbookmark` and
  `unfavourite` routes and trailing-slash compatibility aliases.
- Bookmark and favourite writes target the original status for boosts, use
  conflict-safe inserts/deletes, maintain favourite counters, and create the
  supported favourite notification activity.
- Added transactional reblog/unreblog writes with Rails-compatible POST routes,
  hashed advisory locking, generated status IDs, default/requested visibility,
  soft removal, original-status counters, and account status-count updates.
- Added owner-role schema coverage and a guarded Rails-versus-Rust write case;
   the case compares bookmark/favourite operations, duplicate reblog creation,
   reblog removal, generated status shape, and restored interaction rows/counters.
   The full 10-case differential suite passes.
- Added a separate guarded concurrent proof for bookmark/favourite/reblog
  creation and removal. It validates exact duplicate-record loser responses,
  duplicate-free rows, status/account counters, and restores interaction,
  conversation, notification, and notification-request state. The owner-role
  schema suite is 21/21, the full repository check passes, and all 10 guarded
  differential cases pass.
- The issue remains open for complete notification/outbox behavior, stable
  duplicate HTTP unreblog behavior outside the no-Sidekiq fixture contract, and
  the remaining interaction APIs.
- Added status conversation mute/unmute routes with `write:mutes` scope checks,
  authorized status loading, idempotent conversation-mute persistence, and
  full status response parity. The guarded interaction differential now covers
  duplicate mute/unmute retries and restores conversation-mute state.
- Added owner-only status pin/unpin routes with `write:accounts` authorization,
  pin-limit and visibility validation, idempotent status-pin persistence,
  trailing-slash routes, and guarded rollback coverage.
- Closed the remaining local write-path parity gaps in the guarded fixture:
  generated-ID and duplicate unreblog requests use Rails' literal-ID branches,
  owned boosts can be removed after the source becomes unreadable, both block
  directions are enforced for creation, and canonical target locks match
  status deletion order. Reblog removal now updates the reblogger's account
  status counter while preserving Rails' pre-worker response projection.
- Added regressions for reverse-blocked creation, removal after blocking the
  author, generated-ID no-op unreblogs, serialized-object relationship flags,
  recursive no-op projections, and counter maintenance. The full 15-case
  differential suite, 26-test schema suite, and aggregate `mise run check` all
  pass against Mastodon 4.6.5.
- The issue remains open for complete notification/outbox delivery semantics,
   local and remote reblog federation distribution, incoming remote Announce
   fetching, and the remaining interaction APIs.
- Reblog create and removal now record authenticated user-stream `update` and
  `delete` events transactionally, with follower fan-out honoring
  `show_reblogs` and preserving the booster event. Restored-fixture coverage
  proves the visible/hidden follower split and both lifecycle events; schema
  integration passes 28/28, worker integration 26/26, and all 18 differential
  cases pass.
- Deleting a status now also emits transactional `delete` events for its
  soft-deleted reblog wrappers, preventing stale boosts in authenticated user
  streams; restored-fixture coverage proves the original and wrapper events.
- Incoming remote Announce and Undo writes now emit authenticated user-stream
  `update`/`delete` events transactionally, including explicit and URI-only Undo
  forms. Remote Note lifecycle stream events are covered alongside the existing
  worker and differential gates; the issue remains open for broader interaction
  APIs and full notification/outbox semantics.
- Favourite and bookmark removal now follows Rails' association-first behavior:
   an existing saved interaction can be removed and serialized after the status
   author blocks the viewer, while an unowned removal still requires status
   authorization. Favourite removal also bypasses remote-domain creation policy,
   so existing rows can be deleted and their Undo delivery recorded. The guarded
   interaction differential and all 35 restored-fixture schema tests pass.

## Completion

- The required favourite, bookmark, boost, by-account list, notification,
  counter, authorization, locking, idempotency, and outbox behavior is covered
  by the restored Mastodon schema gate, concurrent interaction proof, worker
  integration, and the current Rails-versus-Rust differential workflow. The
  acceptance criteria are satisfied; broader live peer convergence remains with
  the federation issues.
