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
