# Add a bounded rich-media processor

## Summary

Introduce one production media-processing boundary for advertised modern still,
video, and audio formats, with explicit runtime capability checks and resource
limits.

## Requirements

- Centralize accepted MIME classification and output media kinds instead of
  maintaining independent advertisement, upload, worker, and cache allowlists.
- Probe and process only task-owned local files through fixed argument vectors;
  never give an external processor a remote URL or shell command.
- Bound input/output bytes, decoded dimensions, video matrix, frame rate, frame
  count/duration, process time, threads, stdout/stderr, and temporary storage.
- Convert HEIC/HEIF/AVIF stills to JPEG, normalize supported video to playable
  MP4 with a PNG preview, and normalize supported audio to a playable local
  representation.
- Make processor cancellation kill child work and remove temporary artifacts.
- Fail startup/preflight when advertised runtime capabilities are unavailable.

## Acceptance Criteria

- Focused tests cover MIME/container mismatch, malformed input, each processing
  limit, timeout/cancellation, output validation, and temporary-file cleanup.
- Real tiny fixtures produce bounded JPEG, MP4/PNG, and audio outputs with the
  expected metadata on supported deployment architectures.
- Existing JPEG/PNG/GIF/WebP behavior and image safety limits remain passing.
