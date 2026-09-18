# Add a bounded rich-media processor

## Summary

Introduce one production media-processing boundary for advertised modern still,
video, and audio formats, with explicit runtime capability checks and resource
limits.

## Requirements

- Centralize accepted MIME classification and output media kinds instead of
  maintaining independent advertisement, upload, worker, and cache allowlists.
- Probe and process only supplied local bytes through fixed argument vectors and
  anonymous pipes; never give an external processor a remote URL or shell command.
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

## Completion evidence — 2026-09-18

- Processor and preflight implemented in `f196f18` and `e46c277`, with independent
  review finding no remaining blocker/high findings. Processing is pipe-only;
  no temporary media files are created.
- Focused supervisor integration tests: 7 passed; processor validator unit tests:
  8 passed; legacy Paperclip regressions: 22 passed. Ordinary macOS runs used an
  isolated portability adaptation, not a substitute for Linux path-safety proof.
- Real Linux ARM64 `media-processor` integration executable at `228b52a` passed
  all 3 ignored tests in a disposable, network-disabled runtime container. It
  had no live database, environment, or media mounts and explicit resource/time
  bounds. FFmpeg/ffprobe 7.1.5 came from signed Debian packages; executable
  hashes matched the candidate runtime. This covered the full capability
  matrix, normalized outputs, and declared-type mismatch rejection.
- Formatting, all-target check, and strict library Clippy passed at the processor
  checkpoint. Full library/database/browser/peer aggregate gates are not claimed.
- The processor foundation is complete. Durable local uploads and remote caching
  remain separate open issues; completion here does not claim those integrations.
