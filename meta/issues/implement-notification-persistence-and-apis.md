# Implement notification persistence and APIs

## Summary

Productionize v1 and grouped-v2 notification reads and core notification writes.

## Requirements

- Implement list/show/dismiss/clear/unread-count APIs, exact grouping,
  pagination, all stored 4.6.5 types, and safe unknown-type preservation.
- Idempotently create core v1 notifications and enforce notification policies,
  blocks, mutes, and conversation mutes.

## Acceptance Criteria

- Differential read/write cases match Mastodon and retries cannot duplicate a
  logical notification.

## Progress

- Production v1 and grouped-v2 index reads now cover all stored 4.6.5 types,
  unknown-type preservation, filters, grouping, cursor pagination, fallback
  serialization, partial accounts, and trailing-slash routes.
- Production v1/v2 unread-count and show reads now enforce notification scope
  and account ownership, support read markers and grouped-type filters, and are
  covered by the guarded Rails differential case. The checked fixture currently
  contains a home marker but no notification-timeline marker.
- Production v1/v2 clear and dismiss writes now use the explicit writer pool,
  enforce `write:notifications` and account ownership, preserve Rails
  `delete_all` semantics for clear, reconcile notification requests for
  destroy-based dismissals, and are covered by a guarded Rails differential
  case. Grouped dismissal now also handles `ungrouped-<id>` keys.
- The internal idempotent creation kernel now resolves all 17 stored 4.6.5
  activity/type associations. It validates activity recipients, suppresses
  unavailable/self/blocked/muted/domain-blocked/conversation-muted activities,
  applies notification permissions and policy actions, handles staff mention
  bypasses, rejects silent or deleted source activities, updates filtered
  mention/quote requests, emits Rails-compatible group keys, and replaces
  update notification rows before re-evaluating policy.
- Owner-role integration covers every stored activity type, grouped keys,
  retries, replacement behavior, recipient validation, and group continuity
  after the last notification row is deleted. Rust-owned expiring ordering
  markers now preserve the Rails 12-hour bucket behavior without changing the
  Mastodon schema. Exact producer integration from status/social/relationship
  lifecycles remains outstanding.
- Added production v1 notification-request list and show reads with account and
  last-status serializer graphs, owner scoping, max/min/since pagination,
  trailing-slash routes, and guarded differential coverage.
- Added production member and bulk notification-request accept/dismiss writes.
  Member operations enforce owner-scoped 404s, bulk operations ignore missing
  IDs like Rails, acceptance inserts notification permissions, and all four
  operations serialize with notification creation through the recipient
  advisory lock. Acceptance now also queues a deduplicated Core-lane worker job
  that clears filtered notifications from each accepted sender and repairs the
  recipient's direct conversations with sorted participants/status IDs. The
  repair is transactional, advisory-locked, and idempotent across multiple
  statuses in one conversation. Guarded differential coverage compares auth
  failures, request/permission mutations, scalar and array IDs, and remains
  green; worker coverage proves unfiltering and conversation repair. Streaming
  merge effects remain deferred.
- Added the authenticated `GET /api/v1/notifications/requests/merged` read with
  both slash forms and differential coverage. The current runtime has no
  Redis-backed unfilter worker, so the endpoint reports the synchronous settled
  state (`merged: true`) while worker-state reporting remains deferred.
- Added authenticated v1 and v2 notification-policy reads with trailing-slash
  routes, Rails defaults when no row exists, v1 boolean compatibility, v2
  accept/filter/drop values, unknown-value preservation, suspended-sender
  summary filtering, and guarded differential/schema coverage.
- Added transactional v1 boolean and v2 enum notification-policy updates with
  account advisory locking, default-preserving upserts, explicit writer grants,
  trailing-slash routes, response serialization, and guarded rollback coverage.
- Status creation now records a transactional Core-lane outbox event. The
  writer-backed handler selects active local followers with `notify=true` for
  public/private statuses and mentioned followers for limited/direct statuses,
  then reuses the idempotent notification-policy kernel. Deleted, suspended,
  and replies to another account are skipped; pending events are cancelled on
  status deletion. Owner-role coverage proves duplicate-safe status delivery,
  and the notification differential case remains green.
- Status mentions, favourites, and reblogs now create notifications through the
  transactional Core-lane outbox path. Post-commit web notification writes were
  removed; worker coverage proves dispatch, policy-kernel resolution, and
  duplicate delivery remain idempotent.
- Status notification fan-out reads Mastodon's `USER_ACTIVE_DAYS` environment
  setting, defaulting to seven days, instead of baking the default into the
  PostgreSQL predicate.
- Follow and follow-request notifications now use the same transactional Core-lane
  outbox path for local relationship writes, approval, and inbound ActivityPub
  relationship writes. Remote Note mentions, Likes, and Announces also enqueue
  notification jobs before their write transactions commit; ingress no longer
  performs direct notification writes.
- Notification deletion now shares the recipient advisory lock with creation,
  cancels pending notification outbox events, and reconciles filtered request rows.
  Local and remote status edits preserve removed mentions as silent rows, reuse
  reintroduced mentions, and cancel only undelivered notification work.
- Least-privilege writer validation now requires only column-scoped UPDATE for
  `mentions.silent` and `mentions.updated_at`; runtime-owned durable jobs remain
  inaccessible to the web writer and are fenced by activity re-resolution.
- The merged-request read now reports `merged: false` while an accepted request's
  transactional unfilter work remains in the outbox or live durable-job queue,
  and reports `true` after the worker completes it. Restored-fixture worker
  coverage proves both pending states and completion.
- Single notification-request dismissal now atomically records a per-request
  cleanup outbox event, and the Core worker deletes the dismissed sender's
  remaining filtered notifications in bounded batches. Bulk dismissal retains
  Rails' direct-destroy behavior. Restored-fixture coverage proves delayed
  deletion and re-enqueueing for a later request from the same sender.

## Completion

- The acceptance criteria are satisfied: guarded read/write differential cases,
  owner-role schema coverage, worker coverage, and the full repository checks
  pass. Streaming merge effects remain tracked by the authenticated streaming
  issue rather than this persistence/API issue.
