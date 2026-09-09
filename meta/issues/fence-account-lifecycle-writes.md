# Fence account lifecycle writes and purge side effects

## Summary

Close the remaining races around local account suspension/deletion by fencing
authenticated writes and outbound actor-delete delivery against the account
lifecycle, while keeping denormalized quote counters correct during purge.

## Requirements

- Recheck local account lifecycle state inside every authenticated write
  transaction after bearer authentication.
- Serialize account-owned filesystem work with the canonical account lifecycle
  lock.
- Keep accepted-quote counters consistent when purge removes quote rows.
- Recheck actor-delete delivery state at the final send boundary so an
  unsuspension cannot race an in-flight delivery into a post-restore send.
- Preserve the existing delayed purge contract until the product chooses whether
  self-service timing must match Mastodon's immediate-purge behavior.

## Acceptance Criteria

- Restored-fixture tests reject stale authenticated writes across the supported
  write surface after suspension or deletion request.
- Purge tests prove accepted quote rows and `status_stats.quotes_count` remain
  consistent.
- A blocked actor-delete delivery cannot be sent after the account is
  unsuspended.
- Worker, schema, formatting, lint, and repository checks pass.

## Progress

- Added `begin_account_write` and applied it to the identified authenticated
  status, relationship, report, profile, media, conversation, notification,
  bookmark, mute, favourite, marker, and deletion writes.
- Added final-boundary actor-delete fencing and accepted-quote counter repair
  during account status purge.
- Added a restored-fixture regression covering all identified authenticated write
  entry points, plus stale purge media cleanup, quote-row/counter repair, and
  in-flight actor-delete delivery.

## Verification

- `mise run worker-integration`: 39/39 passed.
- `mise run mastodon-schema-integration`: 35/35 passed.
- `mise run check`: formatting, Clippy, dependency audit, fixture verification,
  and all non-ignored tests passed.

## Remaining

- Hard power-loss compensation and live federation evidence remain unavailable.
