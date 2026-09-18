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

- [x] Verified exact application and pinned frontend source provenance.
- [x] Actual import -> enqueue -> held worker -> release -> native update observed.
- [x] MP4 poster/playback, audio fallback/playback, JPEG decode and reload pass.
- [x] Captured browser network proves local-only media requests.
- [x] Named mechanics, accepted evidence, gaps, and task-only cleanup recorded.
- [ ] Parent independent review of substantial harness before commit/closure.

## Notes

Created before harness implementation. Prior local-upload evidence at
`/srv/workspaces/rustodon-upload-browser-6f44264-alice/evidence/` is reference
material only, not acceptance of this application revision. Processor, imported
source MIME, and same-URL measured-type regression evidence remain in their
existing issues and are not reimplemented here.

## Focused run evidence — pending parent review

Application was built from a tracked-only archive of exact `8b2f49e`, not the
subsequent issue-documentation commit. Debug/test-support binary SHA256:
`8ed97c9182cf111a1fc2e930028c6bf5eb2158a3b7b015b367833318cde5b566`.
Pinned reference source verification and tracked frontend SHA256SUMS passed.
PostgreSQL 14.23 and distinct restricted runtime/writer roles are recorded.

Sanitized, **uncommitted** evidence:
`/srv/workspaces/rustodon-remote-browser-8b2f49e-alice/evidence/`, mirrored locally
at `target/remote-browser-8b2f49e-evidence/`, with `SHA256SUMS`.

- Final signed imports: video status `117294039290491438`, audio
  `117294040063949500`, AVIF `117294040956846177`. Each has a captured unleased,
  zero-attempt `rustodon.activitypub.fetch_media` job, processing=0, no cache
  filename, native pending `update`, then native `status.update` after real Pull
  worker release. Owner SQL never manufactured cached attachments or bytes.
- Same live home page converged without reload. MP4 PNG poster decoded before
  play (paused/time=0); playback advanced from 0 to 0.104498 seconds and decoded
  frames from 0 to 2. Audio advanced from 0 to 0.049342 seconds, had no
  preview/small, and its rendered fallback equaled and decoded the account avatar.
  AVIF rendered JPEG magic/MIME and decoded at 566x377. Dedicated status reloads
  repeated these native checks successfully.
- Final network record: 888 requests, zero to `remote.fixture.invalid`, positive
  successful local media requests and no media DOM hotlinks. Network records
  strip headers, query strings, fragments and bodies. Credential-pattern scan
  passed before evidence export.
- TDD offline harness contracts: 4/4; shell syntax and diff whitespace checks pass.
  No application changes, production work, push, deployment or full-peer gate.
- All eight task containers, task PG volume/network, media root, generated keys,
  certificates, env/raw-network/session artifacts, task build output and binary
  removed. Exact source, harness and sanitized evidence retained.

### Explicit gaps / corrections

This is a focused browser slice, **not** the full `browser-integration` lane.
The final controller stopped at an over-strict pending AVIF preview assertion:
the existing local `/media_proxy/<id>/small` is allowed before caching. After
correcting the harness, still/live/reload steps resumed and passed; an uninterrupted
execution of the final controller is not claimed. Earlier exploratory TLS and
baseline mute/exclusive-list fixture corrections are preserved separately, not
substituted for final evidence. No application blocker was found.

Private browser coverage was not added; existing restricted authorized/unauthorized
private-byte HTTP evidence remains in the representation issue, not rerun or
claimed as browser evidence here. Nested independent review was unavailable in
this task session. Harness remains uncommitted for parent correctness/DRY review;
issue and parent remain open, with no combined completion claim.

## Review 1 corrections — fresh uninterrupted final gate

This supersedes the segmented-controller boundary above; parent review 2 is still
pending and all changes/evidence remain uncommitted.

- Reload assertions now consume the actual new-document frontend XHR response.
  Audio independently asserts null `preview_url` and absent `meta.small` after
  reload. Saved live records cannot mask a reload regression.
- Each live/navigation/reload interval is recorded, validated, appended to a
  sanitized aggregate, then reset. Each dedicated reload proves its own local-media
  success and zero network/DOM origin hotlinks. Eight focused offline tests pass,
  including rejection of fake reload audio previews/small, absent reload responses,
  missing stage positives, origin traffic and per-stage DOM hotlinks.
- `run.sh` and `cleanup.sh` preserve the full fixed-namespace bounded recipe.
  The first launcher attempt failed before the controller on initial navigation;
  one permitted harness correction added CA-verified HTTP readiness. A subsequent
  fresh DB/media/build run passed the complete controller uninterrupted, exit 0.
  No application blocker or application edit; exact `8b2f49e` binary SHA256 remains
  `8ed97c9182cf111a1fc2e930028c6bf5eb2158a3b7b015b367833318cde5b566`.
- Final statuses: video `117294127514309097`, audio `117294128711094918`,
  still `117294129744289698`. All have zero-attempt held-job, native pending/update,
  live and reload evidence. Video progressed 0->0.103098 seconds live and
  0->0.107221 on reload, decoded frames 0->2 both times; audio 0->0.049342 both
  times; JPEG decoded 566x377. Actual reloaded audio response has no preview/small.
- Aggregate network: **1,208 requests, zero origin media requests**. Dedicated
  reload intervals: video **148**, audio **148**, still **147**, each independently
  positive for its local cached media and with no DOM hotlinks.
- New sanitized evidence: NAS
  `/srv/workspaces/rustodon-remote-browser-8b2f49e-r2-alice/evidence/`, local
  `target/remote-browser-8b2f49e-r2-evidence/`. `accept.log` and `result.txt` record
  uninterrupted PASS; `readiness.txt`, actual `*-reload-response.json`, stage and
  aggregate network records, held-job snapshots, screenshots and SHA256SUMS retain
  the mechanics. Failed startup is separate `startup-attempt-evidence/` remotely.
- All eight task containers, PG volume/network, media, keys/certs/env/sessions,
  binary and build output removed. No remaining task containers. Source/harness
  and sanitized evidence retained; no production/push/deploy/full matrix/private
  browser expansion. DEVLOG updated with key findings.
