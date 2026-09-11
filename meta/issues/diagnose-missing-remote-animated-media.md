# Diagnose missing remote animated media

## Summary

The user reports that a remote GIF does not display. The affected post URL is not yet identified.

## Requirements

- Identify the affected attachment and distinguish download, format processing, persistence and serving failures using read-only diagnostics first.
- Do not display private post bodies or credentials, or alter live attachments to mask the problem.
- Reproduce an identified defect on Secunda before any minimal independently reviewed fix.

## Acceptance Criteria

- Establish the failure boundary for the reported attachment.
- Any repair has applicable regression coverage and checks on Secunda.
- Verify the recovered attachment is served in a frontend-compatible representation, or record the remaining blocker.

## Notes

- Awaiting a user-provided post link to identify the exact affected media.
- Separate from [remote profile media](diagnose-missing-remote-avatar-and-banner.md) and [reply-thread persistence](repair-remote-reply-thread-persistence.md).
- Read-only diagnostics found one cached GIF: attachment `117252276513140040`
  on status `117252276513133141`. Processing is complete; no media jobs remain.
  Its original serves HTTP 200 `image/gif` (748440 bytes), and the PNG preview
  serves HTTP 200 `image/png`. REST nevertheless reports `type: gifv`.
  This establishes a representation mismatch, not a failed download. No live
  post bodies or credentials were displayed. Exact user-post match is still
  awaiting confirmation.

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
- Secunda regression reproduced red (`gifv` instead of `image`); independent
  review approved the two-file code/test diff. All 17 serializer tests, formatting
  and all-target/all-feature Clippy with warnings denied passed on Secunda.
  Logs: `/home/lain/rustodon-parity/raw-gif-{red,green,clippy}.log`.
  Deployment and user-visible confirmation remain pending.
