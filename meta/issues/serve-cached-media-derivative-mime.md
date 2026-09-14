# Serve the correct MIME for cached media derivatives

## Summary

The cached proxy selects PNG video or JPEG converted-image small files but keeps original MIME except for GIF.

## Requirements

- Derive MIME consistently from the served Paperclip style; keep original and preview metadata semantics distinct.
- Test production loader/serializer/HTTP bytes as applicable, not only status codes.
- Preserve access checks, size limits, nosniff and caching; do not add transcoding requirements.

## Acceptance Criteria

- Red regression demonstrates PNG video thumbnail or converted-image derivative MIME mismatch.
- Green checks cover original/small GIF, video, converted and unchanged-image representations with MIME and decoded-byte agreement plus access-denial controls.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- Implementation plan recorded on 2026-09-12; no live deployment is implied.
- Parent-reported isolated-worker behavioral red: `web::cached_media_response_tests` ran seven
  tests, with five passing and two failing at response MIME assertions: MP4 small
  returned `video/mp4` instead of `image/png`; HEIC small returned `image/heic`
  instead of `image/jpeg`. The historical external run artifact is not in the repository.
- The proposed fix shares media-file style MIME selection between Paperclip
  filenames and cached response headers. Parent-reported independent review
  `57b0fa82` found no production blocker; authorization remains unchanged.
- Added the ignored fixture test
  `web::cached_media_response_tests::cached_private_media_http_requires_status_access_even_when_files_exist`.
  It inserts a test-owned private remote status and cached GIF/PNG attachment,
  uses the existing follower and non-follower bearer tokens, and checks actual
  original/small HTTP responses: 404 JSON for anonymous/non-followers (including
  after an authorized request), versus exact decoded bytes/MIME and private
  caching/nosniff for the follower. Uses the schema fixture owner/reader database
  URLs; parent-owned `media_proxy` selector wiring and execution remain pending.

## Completion evidence (2026-09-12)

- Baseline unit regression: five passed/two failed (video PNG and HEIC JPEG
  responses retained original MIME); its historical external artifact is not in
  the repository.
- Combined isolated worker green: seven MIME unit tests; permanent `schema-read-test
  media_proxy` selects one real private-media HTTP regression. Both cached
  styles deny anonymous/non-follower access despite existing files, allow the
  follower with exact bytes/MIME/cache headers, then deny anonymous access again.
- Historical external run artifacts are not in the repository.
  Default/all-feature debug, release all-feature, formatting and strict Clippy
  also pass; independent production and added HTTP reviews found no blockers.
- `media_proxy` is included in the permanent schema aggregate and offline
  selector regression. No access policy, schema, grants, or live state changed.
- Broader DM/block/revocation and post-success outsider matrices remain follow-up
  coverage, not claimed from this focused cached-file authorization test.
