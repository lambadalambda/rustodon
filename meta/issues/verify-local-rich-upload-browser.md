# Verify local rich uploads in the pinned browser

## Summary

Bounded browser acceptance for [Support local rich-media uploads](support-local-rich-media-uploads.md) and [HTTP integration](integrate-local-rich-upload-http.md), based on c6cf7b3. Neither parent is completed by this slice alone.

## Requirements

- Use the exact clean Mastodon 4.6.5 reference (1440d55b139e39ec722c2a3db7f60b66cd889048), actual composer file input, and preferably agent-browser CLI.
- Only task-owned disposable PostgreSQL 14, media, ports, accounts and bounded sequential containers; narrow runtime/writer, owner for setup only. Archive tracked source only. No production or remote-media work.
- Modern still representative, video and audio: capture 202/poll/ready, preview or decoded/playable media, status submission, reload, authorization and ranges.
- Exercise malformed terminal failure and cleanup when feasible; existing HTTP evidence is distinct from browser evidence.
- Minimal reusable harness only if needed, TDD when feasible, independent review for harness changes; leave changes uncommitted.

## Acceptance Criteria

- Actual decoded still and advancing video/audio currentTime (or justified supported playback evidence), before/after reload, with screenshots and request evidence.
- Record commands, source/image identities, resource cleanup and any blocked/unexecuted gates. No full-matrix or parent-completion claim.

## Notes

- Skills loaded: agent-browser (including installed CLI core), nas-podman, repo-issues.

## Executed browser slice (2026-09-18)

**Partial pass; remains open. No application, authorization, remote-media, or repository harness code changed.**

- Clean tracked archive of `c6cf7b319fd2331411759511319dd32841ff180c`;
  `tools/mastodon-fixture verify-source target/mastodon-v4.6.5` passed at the
  exact reference revision, with clean checkout. Served tracked `public/`;
  all `public/packs/SHA256SUMS` entries passed inside the browser container.
- Real agent-browser **0.31.1**, Chrome for Testing **148.0.7778.96**; actual
  Alice fixture login with recovery code, composer file input, privacy controls,
  Post button, status routes and reload. No mocked API or fabricated Redux actions.
- HEIC, AVIF, WebM video and Ogg audio each produced **POST 202**, stable-ID
  **GET 206** while the Maintenance worker was held, then **GET 200** after
  starting/unpausing the real restricted-role worker. Each attached successfully
  through the Post button (**200**, matching attachment ID/type).
- Public video (`attachment.webm` → MP4): rendered native video plays before and
  after reload, `currentTime` **0 → 0.087504 / 0.089062**, `readyState=4`, three
  decoded frames, **960×540**, no media error. The fixture is intentionally tiny
  (browser duration ~0.133s); this is clock/frame evidence, not just a screenshot.
- Public audio (`boop.ogg` → MP3): actual Play click required by Chromium autoplay
  policy, then rendered audio clock **0 → 0.040242 / 0.040187** before/after reload,
  `readyState=4`, duration ~0.261s, no error. Playback measurements used muted
  native elements; audible output was not asserted.
- Public AVIF → JPEG: native image decoded in a fresh anonymous browser and after
  reload. The original uploading session initially failed to decode after posting
  and reload, then later decoded on ordinary navigation/reload (no URL rewriting
  or cache disabling); final natural dimensions **566×377** under responsive
  `srcset`. Preserve the initial failure, not just the later green screenshot.
- Malformed `malformed.heic`: real file input **202 → 206 → 422**, UI toast
  `Error processing thumbnail for uploaded media`; composer clears the attachment.
  Owner-side assertion: retained `processing=3`, null filename/status, zero
  `rustodon.local_uploads` ownership rows; private raw root contains no files.
- Browser-origin request diagnostics with `Range: bytes=0-15`: public AVIF/video/
  audio return **206**, exactly 16 bytes and correct Content-Range, anonymously,
  with cookies, and with owner bearer. Cookie/bearer success is `private, no-store`
  with `Vary: Authorization, Cookie, Signature`.

## Direct blockers and bounded follow-up proposal

1. **Ready unattached media cannot preview/play in the composer.** AVIF and video
   preview requests returned **404**. Audio Edit → Play returned **404**,
   `currentTime=0`, `readyState=0`, media error 4. `paperclip_status_media_access`
   only checks attached statuses; `Repository::media_attachment_status` explicitly
   excludes `status_id IS NULL`. Even a ready owner upload is therefore unavailable
   before posting. Fix needs explicit owner-only access to ready unattached media,
   not anonymous publication or removal of status authorization.
2. **Native media ignores the authenticated browser session.** The author's
   followers-only HEIC status renders but its native image returns **404** before
   and after reload. Range diagnostics: anonymous **404**, owner cookie **404**,
   owner bearer **206**. `optional_viewer_owner` only authenticates Authorization;
   it does not resolve the browser session cookie. Fix needs a scoped existing
   browser-session viewer path for media while retaining status visibility checks,
   invalid/revoked-session behavior, and private cache/Vary policy.
3. **Transient failed-image reuse after posting.** The public AVIF's earlier 404
   appeared to remain in Chromium's image cache across immediate navigation/reload,
   despite same-URL fetch returning 200. A fresh session decoded; the uploading
   session also decoded later. Error responses had no Cache-Control/Vary in the
   recorded diagnostics. Exact cache mechanism is not proven. Recheck this sequence
   with the unattached-preview fix; do not broadly rewrite caching in this slice.

These blockers were reported before any proposed source expansion. No source fix
was attempted. A follow-up should add focused authorization regressions first,
then rerun these exact browser cases; no remote-media protocol, schema, privilege
expansion, or full fixture matrix is required by this proposal.

## Isolation, evidence and cleanup

- NAS workspace: `/srv/workspaces/rustodon-upload-browser-c6cf7b3-alice/`.
  Local selected evidence: `target/local-upload-browser-c6cf7b3-alice/evidence/`.
  NAS `evidence/` retains setup/build scripts, CLI observations, request projections,
  image/source identities, SQL assertions, logs and screenshot checksums. It does
  not depend on production credentials or environments.
- Screenshot examples: `video-reloaded.png`, `audio-reloaded.png`,
  `avif-fresh-reloaded.png`, `avif-uploading-session-final-reloaded.png`,
  `audio-edit.png`, `malformed-terminal.png`. Video/audio reload, fresh AVIF reload
  and malformed-terminal screenshots were visually inspected; JSON clock/decode/
  request results are the behavioral evidence.
- Codec image `7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`;
  PostgreSQL 14.23 `1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37`.
  Cached CLI image lacked Chrome; task-only browser image
  `d6337b96fb60b14e3bbd27ead60d4a781ef4ef3317c3f610b4ec4b21b655c094`
  combines immutable existing CLI and Playwright 1.60.0 image IDs, offline.
- Default-feature binary, **no test-support**, built `--locked --offline` from the
  archive. Setup/owner assertions use postgres; application/worker use distinct
  restricted `upload_runtime` / `upload_writer`, unchanged documented grants.
- Bounds: build 4 CPU/6 GiB/512 PIDs/870s (900s outer); DB 1 CPU/512 MiB/128 PIDs/
  10800s; web 2 CPU/1 GiB/256 PIDs/9500s; worker 2 CPU/2 GiB/256 PIDs/8000s;
  browser 2 CPU/2 GiB/512 PIDs/9000s; loopback forwarder 0.5 CPU/128 MiB/32 PIDs/
  7000s. CLI steps 30s +5s kill. Workload jobs ran sequentially; only fixture
  sidecars overlapped. No host ports published.
- Reused `tools/browser-fixture-tls`: exact fixture SPKI trust, not disabled TLS
  validation. A task-only raw TCP loopback 443 forwarder reaches its random port
  for canonical URLs; only that container adds `NET_BIND_SERVICE`. Initial setup
  corrections: mount tracked `/workspace/public` for compiled frontend path; give
  the forwarder the bind capability. Neither required a source/security change.
- Both browser sessions closed. All task containers, PostgreSQL anonymous volume,
  task network and media root removed; namespace-dependent DB removal retried after
  its clients were removed. Worker/browser/forwarder hit the 5s stop grace and
  were killed; no graceful-shutdown claim. Evidence, source archive and task browser
  image retained. Production untouched; no broad engine cleanup.
- This was observation against committed code, so TDD was not applicable. No reusable
  repository harness was added while source blockers remain. Full source-contract,
  HTTP, schema, cutover, peer and aggregate browser gates were **not** rerun or claimed.
  Independent parent review remains pending; nested reviewer delegation was
  unavailable at this task depth. Leave changes uncommitted and both parents open.

## Scoped blocker implementation follow-up

[Fix local-upload browser media authorization](fix-local-upload-browser-media-authorization.md)
tracks the pre-code indexed follow-up on `373da56`: owner-only ready unattached reads,
scoped native session reads and private denial caching. HTTP/unit/codec evidence is
recorded there and in DEVLOG; independent review and this browser rerun remain pending.
