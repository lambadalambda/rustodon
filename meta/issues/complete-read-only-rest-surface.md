# Complete the read-only REST surface

## Summary

Expose remaining in-scope read APIs and preserved optional data.

## Requirements

- Add account search/exact remote resolution, status source/history,
  reblogged-by/favourited-by, and frontend-required preserved-data reads.
- Return stable empty responses only for explicitly disabled features.

## Acceptance Criteria

- Every production route has Rails differential coverage and existing data that
  must be preserved is never hidden behind an empty response.

## Progress

- The existing collection projection loader and serializer are ready to wire
  into the production router. The differential suite currently uses a
  test-only collection handler, so `GET /api/v1/collections/:id` is the next
  production read endpoint being moved into `src/web.rs`.
- `GET /api/v1/collections/:id` is now served by `src/web.rs`; its guarded core
  differential case no longer uses a fixture-only route. The next endpoint is
  status source, which needs a dedicated three-field serializer rather than the
  full status shape.
- Added production `GET /api/v1/statuses/:id/source` with exact three-field
  serialization and strict bearer-scope handling.
- Added production `GET /api/v1/statuses/:id/history` with authorized fallback
  snapshots, persisted edit media ordering/descriptions, historical polls and
  quote states, plus Rails differential coverage. The core and full differential
  cases pass after both endpoints moved out of fixture-only handlers.
- Added production `GET /api/v1/statuses/:id/favourited_by` and
  `GET /api/v1/statuses/:id/reblogged_by` with root-status authorization,
  block/mute filtering, application-only token behavior, and association/status
  cursor pagination. Both are covered in the guarded core differential case.
- Added production `GET /api/v1/accounts/search` with required user
  authentication, exact stored local/remote matches, PostgreSQL full-text
  partial search, following-only filtering, limit/offset behavior, and guarded
  differential coverage. `resolve=true` returns an explicit unsupported
  response for complete remote handles; network fetching remains deferred to
  the safe remote-fetch issue.
- Added production `GET /api/v1/markers` with owner-scoped marker loading,
  scalar/array/unknown timeline handling, private protocol headers, and
  differential coverage. The marker read no longer uses a fixture-only route;
  marker writes remain deferred to the write foundation.
- Added production `GET /api/v2/filters` with account-scoped filter definitions,
  nested keyword/status rules, private protocol headers, and differential
  coverage. Filter writes remain deferred to the write foundation.
- Added production `GET /api/v1/lists` with caller-owned list serialization,
  replies-policy mapping, private protocol headers, trailing-slash support, and
  broad/granular/owner-isolation differential coverage. List writes remain
  deferred to the write foundation.
- Added production `GET /api/v1/featured_tags` with caller-owned featured-tag
  loading, tag-name fallback, account-tag URLs, string counts/date serialization,
  private protocol headers, and broad/granular/owner-isolation coverage.
- Added public production `GET /api/v1/accounts/:id/featured_tags` with
  unavailable/suspended-account handling, public access despite unrelated token
  scopes, account-tag URLs, and anonymous/trailing/missing-target differential
  coverage.
- Added public production `GET /api/v1/custom_emojis` with the Rails `listed`
  scope, category/featured metadata, Paperclip URLs, public cache behavior,
  trailing-slash support, and anonymous/authenticated differential coverage.
- Added production `GET /api/v1/featured_tags/suggestions` with required
  account-read authentication, recent-status tag ranking, featured-tag
  exclusion, relationship booleans, trailing-slash support, and differential
  coverage.
- Added production `GET /api/v1/followed_tags` with account-owned tag-follow
  loading, `follow`/`read`/`read:follows` authentication, seven-day empty
  history, featured relationships, cursor pagination, trailing-slash support,
  and guarded differential coverage.
- Added production `GET /api/v1/follow_requests` with suspended-requester
  filtering, account serialization, `follow`/`read`/`read:follows` authentication,
  cursor pagination, trailing-slash support, and guarded differential coverage.
- Added production `GET /api/v1/preferences` with exact user/account ownership,
  Rails settings defaults, locale fallback, private protocol headers, and
  guarded authentication/trailing-slash differential coverage.
- Added production list detail, list-member, and account-list reads with
  owner-scoped `read:lists` authorization, suspended-member filtering,
  max/since cursor pagination, Rails' unlimited-list ordering, trailing-slash
  routes, and guarded differential coverage.
- Added direct `GET /api/v1/media/:id` differential coverage alongside media
  create/update/delete coverage, including normalized generated IDs and the
  exact media response contract.
- Added production account collection and featured-in-collection reads with
  optional versus required authentication matching Rails, discoverability and
  suspension handling, offset pagination, collection serialization, link
  headers, trailing-slash routes, and guarded differential coverage.
- Added production notification-request list/show reads with account/status
  graphs, owner scope enforcement, cursor pagination, trailing-slash routes,
  and guarded differential coverage.
- Added production `GET /api/v1/statuses/:id/quotes` with required
   `read:statuses` authentication, root-status authorization, accepted-quote
   filtering, quote-ID pagination, visibility-aware source status loading,
   trailing-slash routing, and guarded differential coverage.
- Quote pagination now excludes authors who block the requesting account before
  applying the limit, matching Rails' `not_excluded_by_account` scope and
  preventing hidden rows from leaking pagination cursors. Schema integration
  covers the blocked-author case.
- The listed read-only REST endpoints and preserved-data responses now have
  production handlers and guarded Rails-versus-Rust coverage. The full 14-case
  differential suite passes against the pinned Mastodon 4.6.5 fixture; remote
  fetching and remaining write APIs stay in their owning issues.
