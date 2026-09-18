# Support advertised media attachments in the bundled frontend

## Current status — OPEN (2026-09-18)

Local upload milestones are complete and archived on combined reviewed
persistence/worker/restricted HTTP and exact-`6f44264` browser evidence. Remote
rich-media caching and generated previews remain unimplemented; the remote MP4
acceptance criterion and combined remote regression coverage remain unmet. The
original diagnosis below is historical for local uploads, not current local status.

## Summary

Rustodon advertises Mastodon-compatible image, video, and audio MIME types, and the bundled composer lets users select them, but creation and remote caching are limited to JPEG/PNG/GIF/WebP. Ordinary phone photos, video, and audio uploads therefore begin and fail with HTTP 422; remote video also lacks a still preview.

## Requirements

- Accept the ordinary HEIC/HEIF/AVIF image, video, and audio formats advertised by the instance and Mastodon 4.6.5 frontend within explicit byte, duration, dimension, and processing-work bounds.
- Preserve playable originals locally; convert modern still images and generate frontend-compatible video previews with correct MIME, dimensions, and metadata.
- Support local compose uploads and bounded remote caching without hotlinking untrusted originals indefinitely.
- Keep media authorization, lease fencing, transactional metadata installation, cleanup, and `status.update` convergence guarantees.
- Return a clear validation error for video that exceeds supported limits.

## Acceptance Criteria

- An HEIC/HEIF/AVIF photo or supported video/audio file selected in the bundled composer uploads, previews appropriately, posts, and survives reload.
- A normal remote MP4 post displays a still preview before playback and plays from an authorized local representation.
- `preview_url` returns an image representation rather than the original video bytes.
- Focused media and browser regressions cover local upload, remote caching, preview generation, playback, failure, and cleanup.

## Evidence

- Rustodon advertises HEIC/HEIF/AVIF, video, and audio MIME types in its instance response, while media creation, `remote_media_is_fetchable`, and worker content types accept only JPEG/PNG/GIF/WebP. The same v2 upload the bundled composer uses returns 422 for the advertised formats.
- A live remote MP4 on `rustodon.social` serialized local proxy URLs, but both `original` and `small` returned the same `video/mp4` bytes. The bundled frontend therefore rendered a black poster until Play; playback itself then succeeded at 640×480.
- This is separate from the repaired GIF-as-`gifv` representation bug: the attachment is a real MP4 and needs a generated still preview.
- Tracked subissues:
  - [Add a bounded rich-media processor](add-bounded-rich-media-processor.md)
  - [Support local rich-media uploads](support-local-rich-media-uploads.md)
  - [Cache remote rich media with previews](cache-remote-rich-media-previews.md)
