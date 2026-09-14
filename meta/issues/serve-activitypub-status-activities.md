# Serve ActivityPub status activity routes

## Summary

Serve the ActivityPub activity representation for local statuses at the
username and numeric account paths already emitted in local Note and outbox
documents.

## Requirements

- Serve `/users/:username/statuses/:id/activity` and
  `/ap/users/:account_id/statuses/:id/activity`.
- Return a `Create` activity for ordinary statuses and an `Announce` activity
  for boosts, using Mastodon-compatible IDs, audiences, and objects.
- Apply ActivityPub signature or authenticated viewer authorization before
  exposing private or direct statuses.

## Acceptance Criteria

- Public activity routes return `application/activity+json` with the expected
  `Create` or `Announce` shape.
- Unauthorized private/direct statuses return `404` and authorized viewers
  can retrieve them.
- Username and numeric routes resolve to the same activity identity.
- Unit and integration checks cover routing, activity shape, and privacy.

## Notes

- Use the pinned Mastodon 4.6.5 checkout at
  a repository-external pinned Mastodon 4.6.5 checkout as the behavior reference.

## Progress

- Added username and numeric status activity routes with content negotiation,
  ActivityPub signature or OAuth viewer authorization, privacy checks, and
  `Create`/`Announce` serialization. Embedded private self-boost Notes retain
  the Mastodon audience and object behavior.
- Added unit coverage for activity type and identity selection, plus guarded
  differential coverage for public, numeric, anonymous private/direct, and
  signed private/direct requests. The differential suite passes 18/18 and the
  aggregate quality gate passes.
