# Implement visibility-correct account and status reads

## Summary

Expose the first useful account and status read endpoints while proving that
private, limited, and direct content cannot leak to unauthorized viewers.

## Requirements

- Implement instance v1/v2 and harmless disabled-feature responses required by
  client startup.
- Implement account show, lookup, relationships, statuses, followers, and
  following endpoints.
- Implement status show and context endpoints.
- Apply status authorization separately from viewer timeline filtering.
- Enforce public, unlisted, private, limited, and direct visibility using
  current follows and active or silent mention rows as appropriate.
- Enforce blocks, user domain blocks, and suspended or deleted account state.
- Exclude soft-deleted statuses from ordinary reads.
- Support endpoint-appropriate `max_id`, `min_id`, `since_id`, `limit`, and
  compatible `Link` pagination.

## Acceptance Criteria

- Differential request cases pass for every implemented endpoint.
- A dedicated authorization matrix proves private, limited, and direct statuses
  are never exposed to unauthorized anonymous or authenticated viewers.
- Tests cover current and former followers, active and silent mentions, blocks,
  domain blocks, suspended accounts, and soft-deleted statuses.
- Pagination returns stable ordering, decimal string IDs, and compatible Link
  relations.
- Queries operate directly on PostgreSQL and leave the fixture unchanged.

## Notes

- Depends on `implement-oauth-bearer-authentication.md` and
  `implement-core-rest-serializers.md`.
- Timeline selection, mutes, language preferences, filters, and exclusive lists
  belong to `implement-postgresql-timelines.md`.
