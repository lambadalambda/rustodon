# Implement core REST entity serializers

## Summary

Serialize the read-only Mastodon entities needed by initial client and
frontend API responses.

## Requirements

- Implement serializers for instance v1/v2, account, credential account,
  relationship, status, media attachment, mention, tag, custom emoji, poll,
  quote, shallow quote, collection, tagged collection, filter result,
  notification, notification group, and marker entities needed by the first
  read APIs.
- Emit IDs as decimal JSON strings and timestamps as compatible ISO-8601
  values.
- Preserve expected nullable keys, empty arrays, nested entity shape, rendered
  HTML fields, and authenticated relationship fields.
- Expose Mastodon's internal `limited` visibility using the expected REST
  representation.
- Serialize existing historical polls and preview cards without enabling their
  creation.
- Serialize existing 4.6.5 quote and tagged-collection fields without enabling
  their mutation workflows.
- Serialize every known Mastodon 4.6.5 notification type, including quote and
  collection notification targets, regardless of whether Rustodon v1 creates
  that type.
- Safely omit or represent types introduced after the pinned release without
  mutating stored values.
- Avoid database queries from low-level formatting code where request-level
  batching can supply the needed relationships.

## Acceptance Criteria

- Differential fixtures cover local and remote accounts, boosts, replies,
  direct/private/public statuses, media, filters, polls, quotes, collections,
  and every known readable Mastodon 4.6.5 notification type.
- JSON shape and value comparisons pass against Mastodon 4.6.5 after only the
  documented nondeterministic normalization.
- IDs larger than JavaScript's safe integer range remain exact strings.
- Tests cover authenticated and anonymous status representations.
- Serializer tests do not mutate the fixture database.

## Notes

- Depends on `map-mastodon-4-6-5-schema.md` and
  `build-differential-test-harness.md`.
- Port observable cases from Mastodon's REST serializer specs, not its
  ActiveModelSerializers implementation structure.
