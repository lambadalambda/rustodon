# Self-heal missing account statistics

## Summary

An otherwise healthy local account can have no `account_stats` row. The bundled frontend then shows zero posts, followers, and following on the profile even while rendering the account's posts, and instance status totals also undercount it.

## Requirements

- Detect and repair missing `account_stats` rows for existing accounts without requiring operators to notice incorrect frontend counters.
- Ensure ordinary status and relationship writes cannot silently leave counters at zero when the stats row is absent.
- Preserve Mastodon's counter rules for boosts, replies, direct statuses, deletions, follows, and historical timestamps.
- Reuse the existing account-stat reconciliation semantics rather than introducing a second counting implementation.

## Acceptance Criteria

- A restored account with statuses and follows but no `account_stats` row is repaired to Mastodon-compatible counts.
- Subsequent create/delete/follow/unfollow operations keep the repaired row correct.
- Account REST responses, the bundled profile, and instance status totals expose the repaired values.
- Existing correctly populated stats rows are not reset or double-counted.

## Evidence

- On `rustodon.social`, the public local profile displayed 0 posts, 0 followers, and 0 following while four local statuses were visible and the account had two outgoing follows.
- Read-only database checks confirmed one enabled local user, four undeleted local statuses, and no local `account_stats` row.
- `GET /api/v1/instance` consequently reported `status_count: 0`.
- Current local-user creation inserts `account_stats`, so this is also an upgrade/legacy-integrity gap: existing missing rows do not self-heal, and counter updates are update-only.

## Status 2026-09-23

Implemented (see git log and DEVLOG). Acceptance waits for the final-tree
sweep in [complete-v1-external-acceptance](complete-v1-external-acceptance.md).
