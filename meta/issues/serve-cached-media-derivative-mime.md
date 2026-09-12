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
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.
