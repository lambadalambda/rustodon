# Honor delete_media during status deletion

## Summary

Match Mastodon's status deletion media behavior for the local write path.

## Requirements

- Detach unreported status media when `delete_media` is absent or false.
- Permanently remove unreported status media when `delete_media` is truthy.
- Preserve media for unresolved reported statuses, regardless of the flag.
- Keep status, reblog, pin, counter, and media changes in one writer transaction.

## Acceptance Criteria

- Owner-role schema coverage proves both media modes, reported-status retention,
  reblog/pin cleanup, and counter restoration.
- Guarded Rails-versus-Rust HTTP coverage exercises absent and truthy flags and
  restores generated status, media, and account state after the case.
- JSON numeric boolean values follow Rails casting, including `0` as false.

## Progress

- Added the `delete_media` parameter to owner-authenticated status deletion.
  Unreported media is detached or deleted transactionally; unresolved reported
  media is retained. Reblog owners, original reblog counters, and pins are
  reconciled in the same transaction.
- Added JSON numeric boolean casting coverage and guarded HTTP cases for both
  deletion modes. Owner-role schema coverage and the focused differential case
  pass; post-commit federation delivery and filesystem cleanup remain covered by
  the parent status lifecycle issue.
