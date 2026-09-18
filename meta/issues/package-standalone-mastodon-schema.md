# Package a standalone Mastodon-compatible schema

## Summary

Generate and commit a deterministic PostgreSQL 14 schema-only artifact for the
pinned Mastodon 4.6.5 compatibility target so Rustodon can initialize an empty
database without Rails or a Mastodon image.

## Requirements

- Generate from the verified pinned Mastodon revision and PostgreSQL 14 fixture.
- Exclude fixture rows, credentials, keys, domains, sequence state, comments,
  ownership, and privileges.
- Replace the fixture-specific `timestamp_id` salt with an installer sentinel.
- Remove all psql meta-commands so SQLx can execute the artifact directly.
- Record generation provenance, normalization rules, checksum, and AGPL source
  provenance.
- Add static verification that rejects data statements, fixture identities,
  unknown meta-commands, or artifact drift.

## Acceptance Criteria

- The artifact installs into a fresh PostgreSQL 14 database using SQLx without
  Mastodon or external PostgreSQL client commands.
- The installed empty catalog and exact migration inventory satisfy Rustodon's
  supported Mastodon schema validation after baseline rows are seeded.
- Artifact verification is part of an ordinary or bounded named test lane.
