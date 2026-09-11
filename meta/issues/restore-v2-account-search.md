# Restore v2 account search for clients

## Summary

The real peer smoke found that `/api/v2/search` always returns an empty accounts array, even though the v1 account-search route already supports cached/local search and authenticated remote resolution. Mainstream clients using v2 cannot discover accounts through that route.

## Requirements

- Return account search results for omitted type or `type=accounts`, while preserving hashtag results and current status-search limits.
- Reuse the existing account search/resolution policy rather than duplicate networking, domain checks, shared limits, or signing logic.
- Keep unauthenticated requests from triggering remote writes or resolution; preserve endpoint-specific scopes, filtering, pagination, and response shape.
- Do not expand this into a new full-text status search backend.

## Acceptance Criteria

- HTTP regressions cover local and cached account results, type separation, pagination, and scope failures.
- Authenticated remote-handle resolution uses the existing safe fetch/persistence path; unauthenticated resolve cannot initiate outbound requests.
- Applicable client and differential gates pass on Secunda and behavior limits are documented.

## Notes

- Discovered by the Mastodon peer smoke, which currently uses v1 account search instead.
- Source confirmed in `src/web.rs::search_v2`; implementation and execution evidence are pending.
- Secunda is currently unreachable. The user requested continuing without it; source changes and independent reviews may proceed, but no local builds/tests/formatting/lint/container fallback is authorized. New checks remain unexecuted until remote access returns.
