# Establish Mastodon-compatible write transactions

## Summary

Provide narrow writable repositories and executable differential write tests.

## Requirements

- Use PostgreSQL defaults with `INSERT ... RETURNING`, explicit NULL/array/JSON
  handling, optimistic locking, and atomic outbox/idempotency writes.
- Compare database, media, ActivityPub, and durable-job results with Mastodon.

## Acceptance Criteria

- Duplicate requests are safe and Mastodon can reopen every resulting clone;
  no generic ORM or user-facing mutation is introduced by this issue.

## Progress

- Added a separate `WriteRepository` with UTC, bounded lock/statement timeouts,
  no read-only session default, and no generic SQL surface.
- Added a typed marker update that requires a functional bearer with
  `write:statuses`, uses `INSERT ... RETURNING`, preserves Rails marker
  lock-version behavior, and returns stale-write conflicts.
- Added atomic idempotency fingerprint claims and optional transactional
  outbox recording to the typed writer boundary. Duplicate keys replay without
  repeating the marker update; mismatched fingerprints conflict.
- Owner-role schema integration covers existing-row updates, first-row
  creation, stale versions, idempotent replay, outbox deduplication, and fixture
  cleanup. A least-privilege Rust writer role and guarded differential case now
  compare the marker transaction with Rails while proving media remains stable.
- `WRITE_DATABASE_URL` now configures an optional separate production writer
  pool without changing the default read-only web path. Broader NULL/array/JSON
  write contracts and the remaining user-facing write milestones remain
  outstanding.

## Completion

- The write-foundation acceptance is satisfied by the narrow typed
  `WriteRepository`, PostgreSQL transaction/locking contracts, idempotent and
  outbox-backed user writes, the 35-test restored Mastodon schema gate, the
  Rails-versus-Rust differential write matrix, and the cutover reopen proof.
  Further feature-specific writes and live peer acceptance remain tracked by
  their owning issues.
