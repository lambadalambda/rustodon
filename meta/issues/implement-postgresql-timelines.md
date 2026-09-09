# Implement PostgreSQL timelines and user collections

## Summary

Replace Mastodon's Redis-derived read feeds with visibility-correct PostgreSQL
queries for the first timeline and user-collection API surface.

## Requirements

- Implement home, public/local, hashtag, and list timelines, and preserve the
  existing account timeline as a regression-tested read.
- Implement favourites, bookmarks, blocks, and mutes as read-only endpoints.
- Apply status authorization before timeline-specific filtering.
- Apply blocks, domain blocks, mutes, silenced or suspended account state,
  follow language selection, chosen languages, reply rules, boost preferences,
  and custom-filter annotations where Mastodon does.
- Suppress an exclusive-list member's statuses from the list owner's home
  timeline while preserving their visibility through the appropriate list
  relationship.
- Support endpoint-appropriate `max_id`, `min_id`, `since_id`, `limit`, and
  compatible `Link` pagination.
- Expose every completed selector through the production web router with exact
  endpoint-specific OAuth scopes.

## Acceptance Criteria

- Differential request cases pass for every implemented endpoint.
- Timeline tests cover replies, boosts, follow and chosen-language filters,
  blocks, mutes, exclusive lists, custom filters, and soft-deleted statuses.
- Tests prove an exclusive-list member's status is omitted from home without
  becoming unauthorized or disappearing from account reads.
- Pagination returns stable ordering, decimal string IDs, and compatible Link
  relations.
- Queries operate directly on PostgreSQL without requiring Mastodon's Redis
  feed keys and leave the fixture unchanged.

## Notes

- Depends on `implement-visibility-correct-account-status-reads.md`.
- Exact Redis feed history and boost aggregation are not compatibility
  requirements; visibility and selection behavior are.
- Account-status selection now rejects boosts whose source status is missing or
  soft-deleted before pagination, matching Mastodon's `kept` scope. The restored
  fixture covers the soft-deleted-source regression.
- Conversations, notifications, and markers are tracked separately because
  their read contracts are coupled to v1 mutations and grouped state.
