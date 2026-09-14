# Diagnose missing remote animated media

## Summary

The user reports that a remote GIF does not display. The affected post URL is not yet identified.

## Requirements

- Identify the affected attachment and distinguish download, format processing, persistence and serving failures using read-only diagnostics first.
- Do not display private post bodies or credentials, or alter live attachments to mask the problem.
- Reproduce an identified defect on an isolated worker before any minimal independently reviewed fix.

## Acceptance Criteria

- Establish the failure boundary for the reported attachment.
- Any repair has applicable regression coverage and checks on an isolated worker.
- Verify the recovered attachment is served in a frontend-compatible representation, or record the remaining blocker.

## Notes

- Awaiting a user-provided post link to identify the exact affected media.
- Separate from [remote profile media](diagnose-missing-remote-avatar-and-banner.md) and [reply-thread persistence](repair-remote-reply-thread-persistence.md).
- Read-only diagnostics found one completed cached GIF with no media jobs
  remaining. Its original served HTTP 200 `image/gif` (748440 bytes), and the PNG
  preview served HTTP 200 `image/png`. REST nevertheless reported `type: gifv`.
  This establishes a representation mismatch, not a failed download. No live post
  bodies or credentials were displayed. Exact user-post match still awaited
  confirmation.

## Repair

- The GIF worker preserves original GIF bytes and makes a PNG preview, but
  ingestion assigns stored media type `1`. REST previously mapped every type-1
  attachment to `gifv`, so the bundled frontend attempted to play GIF bytes in
  a video element.
- Serialize type `1` plus cached MIME `image/gif` (case insensitive) as `image`.
  Real MP4 `gifv`, unknown MIME and other stored types retain their mappings.
  Do not infer the file format from the remote URL, which may still end in `.gif`
  after a legitimate video conversion.
- Existing cached rows benefit on serialization without data changes, downloads
  or frontend rebuilds. Original and preview URLs, descriptions and geometry
  remain unchanged. The media modal can use the animated original; this does not
  promise always-animated timeline previews.
- isolated-worker regression reproduced red (`gifv` instead of `image`); independent
  review approved the two-file code/test diff. All 17 serializer tests, formatting
  and all-target/all-feature Clippy with warnings denied passed on an isolated worker.
  Historical external run artifacts are not in the repository.
  Deployment and user-visible confirmation remain pending.

## Live verification — 2026-09-11 UTC

- Deployed source `b2937cf` after all combined isolated-worker gates passed; see the
  [deployment record](repair-remote-reply-thread-persistence.md#combined-isolated-worker-validation-and-live-recovery--2026-09-11-utc).
- The completed cached GIF now serializes as **`image`** without modifying or
  redownloading its existing media. The original returned HTTP 200 `image/gif`
  with 748440 bytes; the preview returned HTTP 200 `image/png` with 352688 bytes.
- The bundled frontend loaded the GIF original in an image element (decoded
  566×421, nonzero rendered dimensions), with **no video element** or browser
  runtime errors. This establishes frontend-compatible rendering; it is not a
  frame-by-frame animation or all-GIF compatibility test.
- Historical external artifacts record this result but are not in the repository.
  The diagnosed GIF is repaired. Keep this issue open only for confirmation that
  this was the user's intended attachment; request its post URL if another GIF
  remains broken. Static timeline-preview behavior remains deliberately unchanged.
