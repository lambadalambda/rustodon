# Cache remote rich media with previews

## Current status — complete (2026-09-18)

Combined acceptance is complete; no unmet remote acceptance blocker remains.

- [Transport](remote-media-policy-and-transport-bounds.md), `be6c7cd`, review 2:
  strict media-family bounds, oversize checks and preserved network guards.
- [Worker](cache-remote-rich-media-worker.md), `41a771e`, review 2: real validated
  outputs, focused 8/8 plus same-URL 1/1 under restricted credentials; mismatch,
  malformed/retry, lease loss, partial write, ambiguity, policy/deletion and cleanup.
  The archived [processor](add-bounded-rich-media-processor.md) supplies the full
  advertised capability/output matrix including HEIF. Worker HEIC and browser
  AVIF are representative integration evidence, not a new full codec run.
- [Representations](remote-rich-media-representations.md), `8b2f49e`, review 2:
  REST/AP local URL/MIME and image-only previews; actual authorized private-byte
  HTTP GET/HEAD/range and unauthorized denial.
- [Browser](accept-remote-rich-media-browser.md), harness `fe6e461`, review 2:
  fresh uninterrupted exact-`8b2f49e` signed import/real worker/native WebSocket
  run; PNG poster before MP4 playback, audio/avatar, AVIF→JPEG and all reloads;
  1,208 captured requests with zero origin-media traffic. Parent offline 8/8.

Parent confirms independent review 2 and approves archival after `fe6e461`.
This current status overrides historical “open”, “uncommitted”, “review pending”
and leave-uncommitted handoff instructions below. Evidence references and
slice-specific boundaries are preserved. No gates were rerun for this docs-only
reconciliation. Actual transport-loss/power-loss simulation, full fixture/release
matrices and peer gates remain unclaimed.

## Summary

Extend the fenced ActivityPub media worker to cache supported remote video,
audio, and modern still images locally and generate frontend-compatible
previews.

## Requirements

- Require advertised and fetched media types to agree, then derive the persisted
  attachment type from independently probed bytes.
- Fetch each media family with explicit transport/storage bounds while retaining
  SSRF, redirect, domain-policy, and host-concurrency protections.
- Preserve processing-claim reclamation, lease cancellation, transactionally
  installed metadata, ambiguous-commit reconciliation, cleanup, and exact
  `status.update` convergence.
- Store authorized playable local originals and real image previews rather than
  hotlinking untrusted remote media or serving video bytes as poster images.
- Serialize correct REST and ActivityPub media type, URLs, preview MIME,
  dimensions, duration, and available metadata.

## Acceptance Criteria

- A normal remote MP4 caches as a local playable representation with a local PNG
  poster, and `preview_url` never returns the original video bytes.
- Supported remote audio and HEIC/HEIF/AVIF cache into validated local outputs.
- Mismatch, oversize, malformed, retry, lease loss, partial write, commit
  ambiguity, policy change, deletion, and cleanup regressions pass.
- Focused browser coverage proves preview-before-playback and reload behavior.

## Browser subissue

[Accept remote rich media in the pinned browser](accept-remote-rich-media-browser.md)
records the completed bounded exact-`8b2f49e` browser slice and explicit evidence
boundaries. See the combined completion mapping above.
