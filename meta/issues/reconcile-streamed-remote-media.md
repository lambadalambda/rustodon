# Reconcile streamed remote media after caching

## Summary

A newly streamed status with a tall remote attachment initially renders as a horizontally filled center crop, and its lightbox image fails to load. After refreshing, the same status shows the complete attachment and the lightbox works.

## Requirements

- Compare the media attachment serialized into the live stream with the refreshed REST representation.
- Determine whether the stream event races remote-media processing, uses stale metadata, or omits a required follow-up update.
- Reproduce the behavior with a focused regression before applying the smallest fix.
- Preserve bounded remote fetching, local-only media serving, stream authorization, and frontend-compatible media metadata.

## Acceptance Criteria

- A streamed remote status converges to the same usable attachment representation as a refreshed status without requiring a page reload.
- The attachment's aspect metadata and local original/preview URLs are valid when presented to the frontend.
- Applicable media, streaming, and worker regressions pass.
- The fix is independently reviewed and verified through the normal frontend flow.

## Notes

- The exact affected post URL is not yet recorded.
- Keep deployment-specific evidence in the private operations note.

## Read-only diagnosis

- Recent attachment metadata identifies one matching tall remote image: the
  status stream event was recorded about 665 ms before its local WebP cache and
  944×1664 geometry were installed.
- The initial attachment row therefore existed when the `update` event became
  visible, but its local file and `meta.original.aspect` were not ready. This
  matches the frontend's fallback center crop and the failing local lightbox
  URL.
- Media processing completed successfully, which explains why a later REST
  refresh returned complete geometry and working local media.
- No `status.update` stream event followed the media-cache installation, so the
  already-rendered frontend status had no way to converge without reloading.
- Diagnosis inspected identifiers, timestamps, media metadata, and event types
  only; no post body or application state was read or changed.
