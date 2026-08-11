# Pin Mastodon 4.6.5 compatibility fixtures

## Summary

Make Mastodon 4.6.5 an exact, reproducible compatibility reference for schema,
request, serialization, and state-transition tests.

## Requirements

- Record the upstream Mastodon 4.6.5 tag and commit in repository metadata.
- Provide a repeatable way to obtain or reference the matching source without
  vendoring the full Mastodon repository into Rustodon.
- Capture the exact Rails schema version and migration list for a fully
  migrated 4.6.5 database.
- Define deterministic database and local-media fixtures that can be loaded by
  both Mastodon and Rustodon tests.
- Include representative local and remote accounts, OAuth tokens, statuses of
  every v1 visibility, relationships, notifications, and local media.
- Include quote, collection, collection-item, keypair, historical poll,
  custom-filter, list, and exclusive-list records that are valid in 4.6.5.
- Include a readable row for every Mastodon 4.6.5 notification type: `mention`,
  `status`, `reblog`, `follow`, `follow_request`, `favourite`, `poll`, `update`,
  `severed_relationships`, `moderation_warning`, `annual_report`,
  `admin.sign_up`, `admin.report`, `quote`, `quoted_update`,
  `added_to_collection`, and `collection_update`.
- Keep fixture secrets and identities test-only and deterministic.
- Document how the compatibility baseline is updated for a later Mastodon
  release without silently changing 4.6.5 behavior.

## Acceptance Criteria

- A clean development environment can obtain the pinned Mastodon reference.
- The fixture database reaches the recorded 4.6.5 schema with no pending
  migration.
- Fixture generation is repeatable and produces stable externally visible
  values after documented normalization.
- Fixture metadata records a structural fingerprint of v1-critical tables,
  columns, types, nullability, defaults, constraints, and indexes.
- The fixture media tree contains at least one valid local account image and
  one status image in Mastodon's expected Paperclip layout.
- Tests fail with a clear message if a different Mastodon revision or schema is
  used accidentally.

## Notes

- Depends on `bootstrap-rust-workspace.md` for project tooling.
- Do not base compatibility fixtures on the moving Mastodon `main` branch.
- Avoid committing real instance data, credentials, or private keys.
