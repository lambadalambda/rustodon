# Implement conversations and markers

## Summary

Implement direct-conversation state and home/notification markers.

## Requirements

- Add conversation list/read/unread/remove and marker read/update endpoints.
- Preserve mention-based access, sorted participant/status arrays, optimistic
  locking, last-status pagination, and deleted-status behavior.

## Acceptance Criteria

- Differential and concurrent-update tests match Mastodon without losing or
  exposing direct-message state.

## Progress

- Marker reads and the writer repository's optimistic marker updates are
  implemented and owner-role tested.
- `POST /api/v1/markers` now accepts Rails nested `home` and `notifications`
  marker parameters, updates all submitted timelines atomically, returns exact
  marker serializers, preserves the read-only default when no writer pool is
  configured, and is covered by the guarded Rails differential case.
- `GET /api/v1/conversations`, `POST .../:id/read`, `POST .../:id/unread`, and
  `DELETE .../:id` now preserve account ownership, participant/status
  serialization, last-status pagination, deleted-last-status validation, and
  Rails response contracts in the guarded differential case.
- Owner-role integration also proves concurrent unread updates produce one
  optimistic-lock conflict without losing the surviving row.
