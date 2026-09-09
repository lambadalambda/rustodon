# Harden ActivityPub inbox ordering and deduplication

## Summary

Ensure ActivityPub ingress ordering and duplicate detection remain correct across
key rotation and conflicting retries.

## Requirements

- Derive the durable ordering stream from the verified remote actor/account
  identity, not from an individual signing key ID.
- Preserve one logical activity identity across signing-key rotation.
- Reject or surface a conflicting body fingerprint for an existing logical
  activity instead of silently treating it as a duplicate.
- Keep the web ingress mutation-free and return `202` only for durable,
  non-conflicting acceptance.

## Acceptance Criteria

- Requests for one actor signed by two valid key IDs serialize in one ordering
  stream and cannot overtake one another.
- A repeated logical activity with a different body is not acknowledged as a
  normal duplicate, while an identical retry remains idempotent.
- Restored-fixture regression tests cover both cases.

## Notes

- Signature verification now threads the canonical verified actor URI through
  cached and refreshed key paths. Inbox logical keys and ordering markers use
  that URI rather than a signing key ID, so key rotation preserves identity.
- `Queue::enqueue_ordered_once` compares retained fingerprints and returns a
  conflict; the inbox responds with `409 Conflict` without creating another
  job.
- The restored fixture regression creates a second valid Bob key, proves a
  two-activity predecessor chain across both keys, verifies an identical retry
  is accepted idempotently, and verifies a conflicting retry is rejected.
- Verification: `mise run check`, `mise run worker-integration` (33/33), and
  `mise run differential` (19/19) pass against Mastodon 4.6.5.
