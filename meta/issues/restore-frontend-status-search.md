# Restore status search in the bundled frontend

## Summary

Rustodon's v2 search response always contains an empty `statuses` array. Signed-in users therefore cannot find posts through the bundled frontend, including exact URLs of already known or resolvable statuses.

## Requirements

- Implement the Mastodon 4.6.5 status branch of `GET /api/v2/search` for the bundled frontend's Posts results.
- At minimum, resolve and return an exact eligible status URL independently of optional full-text indexing.
- Apply normal status visibility, blocks, mutes, domain policy, account filters, pagination, and resolution authorization.
- Keep optional broad text search explicitly conditional if Rustodon continues to run without Elasticsearch.

## Acceptance Criteria

- Searching an exact URL for a visible existing status returns it under `statuses`.
- Hidden, deleted, blocked, or otherwise unauthorized statuses are not disclosed.
- Pagination and `type=statuses` behavior match Mastodon 4.6.5 for the supported non-indexed path.
- A bundled-frontend browser regression opens Posts search results and navigates to the returned status.

## Evidence

- Mastodon 4.6.5 resolves exact status URLs through `SearchService` even when textual status indexing is unavailable.
- Rustodon's v2 search handler unconditionally serializes `"statuses": []` in `src/web.rs`.
- Live search of an ActivityPub object URL on `rustodon.social` returned HTTP 200 with no status result, consistent with the source path.
- This is separate from the existing v2 account-search issue, which covers the `accounts` branch.
