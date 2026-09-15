# Compute real instance activity metrics

## Summary

The bundled public frontend always shows zero active users because Rustodon initializes the instance runtime's monthly and half-year activity counts to zero rather than loading Mastodon-compatible values.

## Requirements

- Record unique user IDs for approved account creation and confirmed returning-user sign-ins in daily activity buckets retained for six months, matching Mastodon 4.6.5's `activity:logins` lifecycle.
- Aggregate unique IDs over Mastodon's 4-week monthly and 24-week half-year windows, including its date-boundary behavior; do not derive the result from users' current enabled/suspended state.
- Define a bounded rollout policy for existing installations that have no historical activity buckets, including whether and how available last-sign-in timestamps seed approximate history.
- Expose the 4-week count in the v2 instance response, but preserve Mastodon's endpoint-specific privacy behavior: limited federation suppresses the v2 value while NodeInfo continues to publish both windows.
- Avoid expensive unbounded work on each request; use bounded buckets and appropriate cached/reconciled state.

## Acceptance Criteria

- Focused tests cover first-time approval, confirmed return, repeated logins by one user, distinct users, exact 4/24-week boundaries, and expiry, matching Mastodon 4.6.5 unique-user counts.
- A deployment with newly recorded eligible activity reports a nonzero 4-week value in `GET /api/v2/instance` and the bundled public sidebar; the rollout behavior for preexisting sign-ins is tested and documented.
- NodeInfo reports both 4-week and 24-week counts; under limited federation only the v2 instance value is suppressed.

## Evidence

- Mastodon 4.6.5 records user IDs in daily `activity:logins` unique sets when an account is approved and when a confirmed user returns, then unions the preceding 4 or 24 weeks for instance metrics.
- `rustodon.social` displayed `0 active users` although one enabled, confirmed local user had signed in within 30 days and posted that day.
- `GET /api/v2/instance` returned `usage.users.active_month: 0`.
- `src/main.rs` currently constructs runtime instance metadata with `active_month: 0` and `active_halfyear: 0`; the serializers only forward those values (apart from limited-federation suppression).
