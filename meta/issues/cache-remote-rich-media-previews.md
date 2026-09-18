# Cache remote rich media with previews

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
records the bounded exact-`8b2f49e` browser slice, observed evidence and remaining
review/gate boundaries. Parent remains open; this is not a combined completion claim.
