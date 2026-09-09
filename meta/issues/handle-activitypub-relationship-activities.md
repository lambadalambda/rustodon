# Handle ActivityPub relationship activities

## Summary

Process remote relationship and actor lifecycle activities idempotently.

## Requirements

- Handle Follow, Accept, Reject, Undo Follow, Block, Undo Block, actor Update,
  and actor Delete with canonical identity and tombstone checks.
- Preserve pending follow semantics and both account counters.

## Acceptance Criteria

- Duplicate and out-of-order peer cases converge to Mastodon-compatible state.

## Progress

- Added durable worker processing for singular `Follow` and `Undo`/`Follow`
  activities. Verified signer-owned actor identity is reused or persisted by the
  worker, while the web ingress remains mutation-free.
- Preserved active and pending follow semantics, activity URIs, account
  counters, relationship locking, notification persistence, embedded and
  URI-only Undo forms, and six-hour Undo-before-Follow tombstones. The worker
  rechecks remote-domain policy, and an embedded Undo target that is not a
  known local actor is ignored rather than falling back to its activity URI.
- Added ignored restored-fixture coverage for active, duplicate, pending,
  URI-only Undo, counter, notification, and out-of-order cases. The durable
  worker now also processes Block, Undo Block, Accept, and Reject, including
  blocked-follow rejection and URI-bound follow decisions. Manual approval of
  a remote follow request records a signed Accept outbox event in the same
  transaction as the decision. Restored-fixture coverage now exercises block
  teardown, blocked-follow rejection, and inbound Accept/Reject transitions.
- Local Follow/Undo, Block/Undo Block, Reject, and remove-follower actions now
  persist stable URI identities and enqueue policy-aware per-inbox delivery
  events transactionally. Block teardown removes the incoming request or
  relationship and emits the corresponding Reject or Undo Follow while
  preserving Mastodon's outgoing pending-request behavior. Actor Update/Delete
  now validate signer/object identity, update bounded remote profile fields,
  sever remote relationships, soft-delete remote statuses, and prevent profile
  resurrection after Delete. The full peer duplicate/order matrix remains open.
- URI-only remote Accept/Reject decisions now require an exact follow URI and
  cannot consume a relationship row whose URI is NULL. Restored-fixture worker
  coverage proves both mismatched decisions are no-ops; matching-URI decisions
  remain covered by the existing transition cases.
- Embedded `Undo Follow` and `Undo Block` now delete only when the persisted
  relationship URI matches the referenced activity URI, so stale embedded
  undos retain newer relationships for the same actor pair. Restored-fixture
  coverage proves old Follow and Block activities cannot remove newer rows,
  while matching embedded undos still remove the current relationship.
- Outbound Reject activities now use the persisted Follow or FollowRequest row ID
  in their Mastodon-compatible outer identity, while immediate blocked Follows
  use the no-row identity. Restored schema and worker coverage exercises block,
  follower removal, request rejection, actor deletion, suspension, and immediate
  blocked-follow paths.
- Expanded the restored relationship worker matrix with duplicate Accept,
  Reject, Block, and Undo Block deliveries. Each duplicate now has explicit
  convergence assertions for the relationship row; the worker integration gate
  passes 43/43. Full live-peer duplicate and cross-instance ordering evidence
  remains open.
