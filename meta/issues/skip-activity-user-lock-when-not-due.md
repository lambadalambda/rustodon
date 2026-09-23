# Skip the activity user-row lock when tracking is not due

## Summary

`track_returning_in` locks the user row (`FOR UPDATE`) in a writer transaction on
every tracked request (`verify_credentials`, web pages, browser sessions), even
when the 24 h sign-in update is not due. Mastodon checks `current_sign_in_at`
first and writes only when due.

## Requirements

- Check due-ness with a lock-free read first; take the lock and write only when
  the update is due. Keep the locked re-check so concurrent claims stay single.

## Acceptance Criteria

- A regression proves a not-due tracked request does not wait on a locked user row.
- Existing activity recording tests pass.

## Done 2026-09-23

Red/green on the NAS PG14 fixture; see DEVLOG 2026-09-23.
