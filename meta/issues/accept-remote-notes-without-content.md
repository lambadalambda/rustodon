# Accept remote notes without content

## Summary

`validate_note_object` (`src/mastodon/activitypub_inbox.rs`) requires `content`
to be a string. Misskey sends `"content": null` (and `"summary": null`) for
notes with only media or a quote, so Rustodon rejects them. On rustodon.social
this dead-lettered 6 boosts of minidisc.tokyo notes (2026-09-18 to 09-22).
Mastodon accepts such notes with empty text.

## Requirements

- Treat a null or missing `content` (with no usable `contentMap`) as empty text
  in every remote Note ingest path; keep the length limit.
- Treat a null `summary` as no content warning.

## Acceptance Criteria

- Unit tests with a Misskey-shaped media-only Note for Create and Announce.
