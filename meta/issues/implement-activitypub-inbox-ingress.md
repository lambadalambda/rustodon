# Implement ActivityPub inbox ingress

## Summary

Authenticate and durably accept per-user and shared inbox requests.

## Requirements

- Enforce a 1 MiB exact-body limit, synchronous signature/digest verification,
  deduplication/ordering state, and transactional ingress enqueue.
- Return `202` only after durable acceptance.

## Acceptance Criteria

- Shared-inbox duplicates and retries enqueue one logical activity and the web
  process performs no activity mutation or remote work inline.

## Progress

- Added Mastodon-compatible `POST` routes for the instance, shared, username,
  and numeric account inboxes.
- Added a bounded exact 1 MiB body path with synchronous HTTP signature and
  SHA-256 digest verification. Remote key refresh authenticates the request but
  does not persist actor/key mutations from the inbox web path.
- Added transactional ingress enqueueing through the runtime queue. A logical
  activity is retained in `idempotency_keys` for 30 days, while per-signer
  ordering is serialized through `ordering_markers`.
- Ingress jobs remain opaque until the separate ActivityPub Note and relationship
  processing issues provide their worker handlers; the web path performs no
  activity mutation.
- Guarded worker integration against the restored Mastodon 4.6.5 fixture proves
  duplicate enqueue suppression, persistent idempotency markers, and increasing
  per-signer ordering timestamps. The ingress acceptance criteria are satisfied;
  ActivityPub processing remains with the separate activity issues.
