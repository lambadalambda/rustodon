# Support account-scoped legacy tag identities

## Summary

Investigate compatibility with preserved remote statuses whose only stored URI is an OStatus `tag:` identifier. Ignoring opaque atom metadata makes fresh canonical ingestion safe but does not map these historical rows and can leave duplicate or stale records.

## Requirements

- Establish the legacy-data contract against the pinned Mastodon source and restored fixtures before selecting a migration or lookup fallback.
- Never infer authenticated origin/account authority from a tag's text. Preserve signer binding, canonical HTTP(S) identity checks and stored-account ownership.
- Give canonical matches precedence; reject ambiguous or conflicting aliases rather than selecting an arbitrary row.
- Keep Create/Update lookup, forwarding, media cleanup, Delete and tombstone selection consistent. Do not broadly relax URI validation.

## Acceptance Criteria

- A restored tag-only remote row can be reconciled with its authenticated canonical object without duplicate statuses, lost history or resurrection after deletion.
- Cross-account/cross-origin tags and conflicting canonical/legacy rows cannot mutate content, remove media, forward activities or create tombstones for a victim.
- Regression tests, independent review and applicable Secunda gates pass before acceptance is claimed.

## Notes

- Follow-up to [opaque atom metadata handling](accept-activitypub-tag-atom-identifiers.md), not a reopening of the provenance security fix.
- This is a documented compatibility limit, not an executed legacy-mapping regression. Source-only repair `a3fe48a` still has its own pending remote verification.

- Tracking only: no implementation or tests were performed for this issue. Builds, tests, formatting, lint and containers remain Secunda-only.
