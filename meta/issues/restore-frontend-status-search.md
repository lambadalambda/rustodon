# Restore status search in the bundled frontend

## Current disposition — post-0994711

Accepted and archived on parent approval. The approved minimum is canonical exact-URL status search without Elasticsearch, not broad text search or complete URL discovery. Known (`54dc5c9`), uncached (`c3497f5`) and browser (`0994711`) slices are accepted under reviews `knownlookupreview2collision`, `uncachedreview2policy`, and `browserreview8e75`, with no remaining high-severity/minimum-criterion blocker reported. Focused HTTP tests cover visibility, type and pagination; final uninterrupted native-input browser evidence covers Posts, exact navigation/reload, cached additional fetches 0, one signed TLS public GET, denied empty results and no private persistence/mention. Coordinator harness reruns: local 4 passed / 1 pinned-source skip; Linux pinned-source 5 passed. All-tab empty results and exactly-one public-fetch count were observed rather than hard assertions; these reviewer follow-ups are nonblocking.

Closure is bounded to canonical exact URLs without ES. HTML alternate discovery,
unredirected mismatched display URL/canonical ID discovery, actor-URL dispatch and
full-text search remain unsupported/deferred. Existing cached display-URL lookup
is not a claim of broader discovery. Full fixture matrix remains pending;
nonzero-offset browser execution is not claimed.

Pinned Mastodon SearchService uses an exclusive resolve=true URL branch: zero
limit returns none; positive offset suppresses a typed result, while absent/blank
type ignores offset. account_id/min_id/max_id belong to textual status search;
following is passed to account search. None constrains exact URL lookup. This
clarifies the original account-filter requirement, not broad-text implementation.
Stricter search mute/block/domain suppression is an intentional safety divergence
from reference context silencing.


This disposition supersedes historical open/review-pending/leave-uncommitted
statements below. Original requirements, hashes, timings and red/green records
are retained as historical evidence, not fresh current-tree gate claims.

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

## Bounded slices

- [Known exact persisted status URLs](search-known-exact-status-urls.md): implementation and focused NAS tests complete; independent review pending. No remote fetching or Elasticsearch.
- URL-branch clarification from pinned `SearchService`: `account_id`, `min_id`, `max_id`, and `following` filter textual searches, not exact URL resolution. The URL branch is exclusive and ignores those filters. This refines the account-filter requirement above rather than imposing contradictory URL semantics.
- Search intentionally suppresses viewer-blocked, muted, and viewer-domain-blocked authors in addition to normal audience authorization. This is a stricter safety choice, not pinned bug-for-bug context silencing behavior.
- [Bounded status-search browser acceptance](accept-status-search-browser.md): exact c3497f5 final isolated NAS controller passed native input/Posts/permalink/reload for known and uncached canonical public status URLs; representative hidden/private zero display and no private persistence/mention, real resolve=true/type/limit requests and zero-fetch cached repeat. Independent parent harness review pending; singleton nonzero-offset browser pagination and full fixture gates not claimed. Parent stays open.

- [Uncached exact status URL resolution](search-uncached-exact-status-urls.md): bounded signed-fetch follow-up implemented with focused NAS red/green evidence; parent review 1 completed without blockers/high findings; test-isolation follow-up verified, review 2 pending. HTML/full-text/browser remain deferred.
