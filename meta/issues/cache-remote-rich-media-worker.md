# Cache remote rich media in the fenced worker

## Summary

Bounded worker subissue of [Cache remote rich media with previews](cache-remote-rich-media-previews.md), starting at `be6c7cd`.

## Requirements

- Use scoped `RemoteMediaFetcher` caps and MIME agreement, central `media_format` import eligibility including bounded missing-advertisement policy; do not broaden shared emoji MIME types.
- Await shared rich-media preparation and persist probed kind, normalized MIME/filename/size, measured geometry/duration, and optional small representation.
- Preserve claims/reclamation, cancellation, row/URL/account/domain/fresh-status/absent-filename fences, account-domain locks, cleanup, ambiguous-install reconciliation, and transactional exact status.update including boosts.
- Preserve installed normalized representation on same-URL Note updates while allowing description/focus edits without replacing measured geometry.
- Reuse per-hop policy checks and install-time visited-domain checks where existing policy permits.
- No schema, grants, jobs, lanes, generation redesign, local_uploads dependency, serialization/proxy changes, or new thumbnail job. Existing lease cancellation plus row fences is not a full SQL generation token.

## Acceptance Criteria

- TDD extends compact existing worker fixtures for real MP4/PNG poster, audio without poster, HEIC/JPEG normalization, mismatch/malformed retry, cancellation/reclamation, partial writes, deletion/policy changes, ambiguity, streams, and same-URL normalized preservation.
- Focused serial task-owned PG14 checks use narrow runtime/writer credentials end-to-end, owner only for setup/assertions, on NAS7203 with native FFmpeg 7.1.5, bounded resources and wall time; no production resources.
- Independent parent review, at most two substantive review/fix rounds; leave changes uncommitted and report exact red/green evidence and gaps. No browser claim.

## Notes

Open until implementation and focused evidence are complete. Parent serialization/proxy and browser acceptance remain separate.

## Implementation and evidence (2026-09-18)

Implemented, **uncommitted and awaiting independent parent review**:

- Worker uses `RemoteMediaFetcher` without changing shared emoji MIME types or
  ordinary transport limits. Existing per-hop domain policy and transactional
  visited-domain recheck are reused; account-domain locking remains in place.
- Shared async rich preparation retains the existing image `spawn_blocking` path.
  Installation records probed kind, output MIME/name/size, measured metadata and
  optional preview. Existing cleanup/ambiguous-commit reconciliation and exact
  status/boost stream collection are unchanged. Install also explicitly compares
  the current account ID to the fetched account ID.
- Import eligibility uses central `RemoteMediaPolicy` (including bounded missing
  advertisement). Same-URL cached rows retain normalized type/MIME/metadata and
  blurhash; description and focus can change/clear without replacing geometry.
- Reused the existing success, partial-write, lease-fence and ambiguity fixtures;
  MP4 now exercises cancellation/reclamation and ambiguous commit. Success helper
  covers GIF plus MP4/PNG, audio/no-small, HEIC/JPEG and exact boost convergence.
  Added compact mismatch/malformed/503 retry/deletion/install-policy cases and a
  direct import/same-URL SQL-helper regression under the narrow writer role.

### Exact red/green

NAS workspace `/srv/workspaces/rustodon-remote-worker-be6c7cd-alice`; logs in
`evidence/`. Tracked source plus explicit new fixture source only. Codec image
`7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`, native
FFmpeg/ffprobe **7.1.5-0+deb13u1**; PG14 image
`1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37`.
Serial containers: 4 CPU/6 GiB/512 PIDs, 870s container and 900s outer wall bounds;
PG 2 CPU/1 GiB/128 PIDs. Separate task-only runtime/writer roles, no owner
fallback; owner used only for setup/reset/assertions and fixture mutations.

- Red `cargo test --offline --locked --all-features --test workers
  activitypub_media_fetch_rich_outputs_and_streams -- --ignored --exact
  --nocapture --test-threads=1`: **0 passed, 1 failed** on base worker, MP4 state
  `(Some(3), None)` instead of retryable `(Some(0), None)` (`red.log`).
- Red `cargo test --offline --locked --all-features --lib
  remote_media_import_and_same_url_normalization -- --ignored --nocapture
  --test-threads=1`: **0 passed, 1 failed**, cached JPEG MIME overwritten with
  `image/heic` (`same-url-red.log`).
- Final green `cargo test --offline --locked --all-features --test workers
  activitypub_media_fetch_ -- --ignored --nocapture --test-threads=1`:
  **6/6, 23.03s** (`final-workers.log`).
- Final green same lib import/same-URL command: **1/1, 0.15s**
  (`final-same-url.log`), including changed advertised kind, focus clear, and
  queued MP4/audio/HEIC/missing-advertisement imports.
- `cargo test --offline --locked --all-features --lib remote_media_transport_
  -- --test-threads=1`: **3/3, 1.66s** (`transport.log`).
- `cargo test --offline --locked --test media_formats`: **4/4** (`formats.log`).
- `cargo clippy --offline --locked --all-features --lib --test workers --
  -D warnings`: **pass** (`final-clippy.log`). Local `cargo fmt --check` via
  Mise and `git diff --check`: **pass**.

Setup initially observed PostgreSQL's transient bootstrap readiness before the
requested database existed; corrected by waiting for successful SQL against
**worker**, then restoring the intended tracked fixture and migrating current
source. Separate task-only setup test calls `operational_schema::migrate`, not
local-upload helpers. No grant changes in repository. First intermediate green
exposed a fixture assertion that assumed audio had a small path; fixed to assert
its absence. Two new fixture Clippy findings were corrected before final green.
Source hashes recorded in `evidence/final-source.sha256`. Test containers,
PG container and task network removed; source/build/evidence retained for review.

### Boundaries / remaining work

- No serializer/proxy edits, browser, peer, full worker/schema lane, or release
  claim. Same-URL regression directly executes the shared Note attachment import
  SQL helper, not an HTTP/federated end-to-end Note UPDATE.
- Lease cancellation plus row fences is **not** a full SQL generation token.
  No schema/grants/job/lane/generation or local_uploads dependency redesign.
- Real codec output passes for MP4, MP3 and HEIC here; complete advertised codec
  matrix remains the separate media-processor gate.
- Nested delegation was unavailable (depth limit); independent parent review is
  pending, not represented as completed. Keep this and the parent issue open.

## Review 1 follow-up — 173488

One bounded correction round requested: reread current focus/metadata under the
install row lock, and retain file cleanup ownership through awaited stream flush
until immediately before COMMIT. Add deterministic barrier regressions for focus
edit/clear during fetch and cancellation during stream flush, then rerun narrow
credential tests and scoped formatting/Clippy. No generalized redesign or change
to unresolved COMMIT ambiguity semantics. Leave uncommitted for review 2.

### Review 1 correction results — ready for review 2

- Removed the prefetch `file_meta` snapshot. The existing `FOR UPDATE OF media,
  account, status` query now also reads current metadata; its non-geometry fields
  are merged into processor metadata under that lock. In-flight focus edits and
  clearing survive; `original`/`small` remain processor-owned.
- Moved awaited stream flush before cleanup-guard preservation. Production now
  calls `preserve()` immediately before `transaction.commit().await`. The
  injected pre-commit rollback also retains the guard through its await. Existing
  post-COMMIT reconciliation and unresolved-commit file retention are unchanged.
- Extended the same compact barrier fixture: HTTP-response barrier covers focus
  edit/clear and attempted geometry replacement. An actual PostgreSQL stream-order
  advisory-lock barrier proves both output files exist while flush waits; aborting
  the worker removes both before releasing that lock. A synchronized row probe
  then proves metadata rollback, reclaimable claim, and no stream publication.
  No production test hooks or new coordination framework were introduced.

Same NAS workspace/image and narrow runtime/writer credentials as above. Fresh
PG14 tmpfs database; readiness verified with successful TCP SQL against `worker`
(not bootstrap `pg_isready`). Owner used only for fixture setup/mutation,
assertions, and the barrier. Execution serial and resource/wall-time bounded.

Exact commands (all use `cargo ... --offline --locked`):

- Red `test --all-features --test workers
  activitypub_media_fetch_current_focus_during_fetch -- --ignored --nocapture
  --test-threads=1`: **0/1**, old focus `{x:0.1,y:0.2}` replaced the edit
  `{x:0.7,y:-0.4}` (`review1-focus-red.log`).
- Red same command with `activitypub_media_fetch_cancel_during_stream_flush`:
  **0/1**, `cancel before COMMIT must clean original`
  (`review1-flush-red.log`). Both reds ran before the worker fixes.
- Green `test --all-features --test workers activitypub_media_fetch_
  -- --ignored --nocapture --test-threads=1`: **8/8, 20.79s**, including both
  barriers, focus clearing, and existing ambiguous-commit before/after cases
  (`review1-green.log`).
- `test --all-features --lib remote_media_import_and_same_url_normalization
  -- --ignored --nocapture --test-threads=1`: **1/1, 0.12s**
  (`review1-same-url.log`).
- `clippy --all-features --lib --test workers -- -D warnings`: **pass**
  (`review1-clippy.log`).
- Additional production-cfg `clippy --lib -- -D warnings` found the existing
  `paperclip.rs::sync_directory` `unused_self` lint. No unrelated edit made;
  rerun with only `-A clippy::unused_self`: **pass**, covering the non-test-support
  COMMIT branch (`review1-production-clippy-known-lint.log`).
- Local Mise `cargo fmt --check` and `git diff --check`: **pass**.

Source hashes match local files (`review1-source.sha256`); task PG/container data
and network removed (`review1-cleanup.log`). One requested correction round is
complete. All changes remain uncommitted for independent parent **review 2**;
no scope expansion, browser, or full-lane claim.
