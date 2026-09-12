# Port the pinned media-state HTTP matrix

## Summary

Add the planned media ownership/attachment/processing matrix at the real HTTP
boundary. Existing JPEG CRUD, repository races, and cached-file authorization
do not establish these request outcomes or nonmutation on rejected operations.

## Requirements

- Use the pinned 4.6.5 v1 request spec and controller extracted read-only from
  the existing cached image; no 4.7-alpha oracle substitution or source fetch.
- Cover owner versus other user, unattached versus attached, and published
  processing states with valid JPEG records. Assert exact response and durable
  nonmutation on rejection, including cached bytes and cleanup intents.
- The pinned controller permits owner updates while processing and returns 206;
  check the suspected Rust ready-only update rejection with red/green evidence.
- Keep synchronous supported image handling; no async uploader, video/audio
  processing, or new formats. Wire the matrix permanently into schema/CI gates.

## Acceptance Criteria

- Baseline behavioral red and final real HTTP green, with pinned references.
- Formatting, strict lint, relevant combined gates and independent review pass.
- Reject paths preserve database state, files and cleanup intents.

## Notes

- Subissue of [selected matrix ports](port-mastodon-media-and-browser-matrices.md).
- Local extracted oracle: `.local-instance/audit-reference/pinned-media/`, from
  exact pinned image `sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`.
- No live deployment or data replay authorized by this work.
