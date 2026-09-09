# Implement image media CRUD

## Summary

Create and update images in Mastodon-compatible Paperclip paths and metadata.

## Requirements

- Implement v1/v2 image media create/show/update/delete with ownership, alt
  text, dimensions, limits, processing state, and orphan cleanup.
- Support avatar/header processing and rollback-compatible styles.

## Acceptance Criteria

- Database/media differential tests match Mastodon and newly written files are
  readable after rollback; new audio/video transcoding is excluded.

## Notes

- Account avatar/header processing now covers local multipart uploads, MIME and
  dimension limits, Paperclip-style paths, GIF static derivatives, safe file
  replacement/removal, and cleared attachment timestamps.
- Image-only v1/v2 media create/show/update/delete now covers `write:media`,
  ownership, descriptions, focus metadata, dimensions, small derivatives,
  blurhash generation, staged file writes, and unattached-delete cleanup.
- The guarded HTTP case now covers v1 create/update/delete, blank-focus no-op
  updates, v2 image create, generated original/small artifact readability, and
  database/filesystem rollback. All 10 guarded differential cases pass.
- Animated-GIF-to-GIFV transcoding, HEIC/HEIF/AVIF conversion, and exact
  libvips byte/blurhash parity are intentionally outside this slice. Generated
  IDs, URL components, blurhashes, and codec-dependent file sizes are validated
  semantically rather than compared byte-for-byte. Crash-time database/file
  compensation remains a follow-up hardening gap covered by the v1 hardening
  issue. The written acceptance criteria for this issue are satisfied and the
  issue is archived.
