# Implement PostgreSQL timelines and user collections

## Summary

Replace Mastodon's Redis-derived read feeds with visibility-correct PostgreSQL
queries for the first timeline and user-collection API surface.

## Requirements

- Implement home, public/local, hashtag, and account timelines.
- Implement favourites, bookmarks, blocks, mutes, conversations,
  notifications, and markers as read-only endpoints where applicable.
- Apply status authorization before timeline-specific filtering.
- Apply blocks, domain blocks, mutes, silenced or suspended account state,
  follow language selection, chosen languages, reply rules, boost preferences,
  and custom-filter annotations where Mastodon does.
- Suppress an exclusive-list member's statuses from the list owner's home
  timeline while preserving their visibility through the appropriate list
  relationship.
- Support endpoint-appropriate `max_id`, `min_id`, `since_id`, `limit`, and
  compatible `Link` pagination.

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
