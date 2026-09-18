# Accept remote rich media in the pinned browser

## Summary

Bounded browser acceptance of application revision
`8b2f49e42a7cddd75b9d2dce4be24cfcd388099c`.
Subissue of [Cache remote rich media with previews](cache-remote-rich-media-previews.md),
under [Support advertised media attachments](support-frontend-video-attachments.md).

## Requirements

- Disposable NAS namespace, PostgreSQL 14, restricted runtime/writer roles,
  pinned Mastodon frontend/source `1440d55b139e39ec722c2a3db7f60b66cd889048`
  with checksum verification; sync only tracked source and explicit fixture files.
- Controlled task-owned HTTP/TLS remote media; actual Note import/apply, enqueue,
  held fetch-media worker, then release through the real handler. No SQL cache
  substitution. Test transport remains debug/test-support-only and isolated.
- Native WebSocket `status.update` and frontend convergence after transactional
  cache installation, without reload substituting for live convergence.
- MP4 real PNG poster before playback, local playback with progressing currentTime
  and decoded frames; audio plays with avatar fallback and no fake small/preview;
  modern still decodes as JPEG; all survive reload.
- Record browser network and assert no requests/hotlinks to origin media host.
  Existing private authorized/unauthorized HTTP proof is a distinct boundary;
  exercise representative followers/private browser case only if available.
- Reuse existing browser CLI images and media-tools image `7203e0222e2b`.
  Heavy workloads serial with CPU/memory/PID/wall-time bounds. No production,
  push, deployment, or full peer lane. Clean only task resources and secrets.
- Harness/tests only; report any application blocker before expanding scope.
  Independent review before substantial harness commit. Actual sanitized run
  evidence remains uncommitted; never claim acceptance with missing assertions.

## Acceptance Criteria

- [ ] Verified exact application and pinned frontend source provenance.
- [ ] Actual import -> enqueue -> held worker -> release -> native update observed.
- [ ] MP4 poster/playback, audio fallback/playback, JPEG decode and reload pass.
- [ ] Captured browser network proves local-only media requests.
- [ ] Named mechanics, accepted evidence, gaps, and task-only cleanup recorded.

## Notes

Created before harness implementation. Prior local-upload evidence at
`/srv/workspaces/rustodon-upload-browser-6f44264-alice/evidence/` is reference
material only, not acceptance of this application revision. Processor, imported
source MIME, and same-URL measured-type regression evidence remain in their
existing issues and are not reimplemented here.
