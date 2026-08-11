# Map the Mastodon 4.6.5 schema in Rust

## Summary

Introduce tested, read-only Rust representations for the Mastodon 4.6.5 tables
and data formats needed by the first API and federation milestones.

## Requirements

- Map accounts, users, roles, OAuth applications/tokens, statuses, status
  statistics, media, mentions, tags, conversations, relationships, filters,
  notifications, domain policy, settings, quotes, collections,
  collection-items, polls, and signing-key data used by v1.
- Preserve signed 64-bit IDs, negative sentinel IDs, enum integer values,
  polymorphic strings, PostgreSQL arrays, JSON/JSONB, inet values, and nullable
  fields.
- Decode Rails JSON and YAML-serialized settings without rewriting them.
- Treat `accounts.domain IS NULL` as local while distinguishing service actors
  from login-capable users.
- Exclude soft-deleted statuses from normal queries.
- Provide explicit handling for unknown enum and notification values so newer
  rows are preserved rather than coerced.
- Keep database access read-only for this issue.

## Acceptance Criteria

- Integration tests load every mapped record type from the pinned 4.6.5
  fixture.
- Tests load valid quote, collection, collection-item, and keypair rows from
  Mastodon 4.6.5 without classifying them as future-schema data.
- Tests cover NULL-versus-empty arrays and strings, negative IDs, all v1 status
  visibility values, and unknown stored values.
- Rust representations round-trip externally visible values without precision
  loss.
- The database role used by this issue cannot modify Mastodon-owned tables.
- No ActiveRecord callback behavior is modeled as an implicit database write.

## Notes

- Depends on `bootstrap-rust-workspace.md` and
  `pin-mastodon-4-6-5-fixtures.md`.
- Prefer focused SQL queries over creating a generic ActiveRecord clone.
- Writes and Rustodon-owned operational tables belong to later issues.
