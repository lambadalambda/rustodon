# Complete local block teardown

## Summary

Match Mastodon's local post-block cleanup for persisted conversations and
notification state.

## Requirements

- Remove the block owner's conversations containing the blocked account.
- Remove the block owner's notifications and notification requests from the
  blocked account.
- Keep block insertion, relationship teardown, and these side effects atomic
  and retry-safe.

## Acceptance Criteria

- Owner-role schema coverage proves all three cleanup families and restores the
  fixture state, including outgoing/incoming follow-request behavior and
  self-block no-op behavior.
- Guarded Rails-versus-Rust block retry coverage compares responses and the
  parent relationship issue's restoration checks remain green.

## Progress

- Local block writes now serialize against recipient notification creation,
  preserve the blocker's outgoing follow request, reject the incoming request,
  and atomically remove the block owner's notifications, notification requests,
  and conversations containing the blocked account.
- Owner-role schema coverage and the focused Rails-versus-Rust relationship
  differential pass. Feed cache removal, collection side effects, and remote
  federation delivery remain outside this local database slice.
