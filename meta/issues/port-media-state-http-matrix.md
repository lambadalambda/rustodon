# Port the pinned media-state HTTP matrix

## Summary

Add the planned media ownership/attachment/processing matrix at the real HTTP
boundary. Existing JPEG CRUD, repository races, and cached-file authorization
do not establish these request outcomes or nonmutation on rejected operations.

## Requirements

- Use the pinned 4.6.5 v1 request spec and controller extracted read-only from
  the existing cached image; no 4.7-alpha oracle substitution or source fetch.
- Cover owner versus other user, unattached versus attached, and published
  processing states with valid JPEG records. Assert exact response and durable
  nonmutation on rejection, including cached bytes and cleanup intents.
- The pinned controller permits owner updates while processing and returns 206;
  check the suspected Rust ready-only update rejection with red/green evidence.
- Keep synchronous supported image handling; no async uploader, video/audio
  processing, or new formats. Wire the matrix permanently into schema/CI gates.

## Acceptance Criteria

- Baseline behavioral red and final real HTTP green, with pinned references.
- Formatting, strict lint, relevant combined gates and independent review pass.
- Reject paths preserve database state, files and cleanup intents.

## Notes

- Subissue of [selected matrix ports](port-mastodon-media-and-browser-matrices.md).
- Local extracted oracle: `.local-instance/audit-reference/pinned-media/`, from
  exact pinned image `sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`.
- No live deployment or data replay authorized by this work.

## Phase 1: tests only

- Dedicated worktree: `task/audit-media-state`; ownership limited to
  `tests/media_state.rs` and this issue. Parent owns fixture selector/CI wiring
  and all NAS workloads. No local tests, builds, formatting, SSH, or commits.
- Plan: 14 real-router JPEG requests. Owner/unattached processing 0 and 1:
  GET/PUT 206 with persisted metadata updates; failed 3: GET/PUT 422. Other
  user/ready: GET/PUT/DELETE 404; other user/failed: GET/PUT 404. Owner/attached
  ready: GET/PUT 404, DELETE 422. Snapshot full test-owned media/status rows
  (including timestamps and explicit attachment order), original/preview bytes,
  and cleanup outbox/durable jobs before every rejected request.
- Oracle read-only at
  `/Users/lainsoykaf/repos/rustodon/.local-instance/audit-reference/pinned-media/`:
  `spec/requests/api/v1/media_spec.rb` lines 32–68, 149–244 and
  `app/controllers/api/v1/media_controller.rb` lines 6–7, 23–49, 63–68.
  The controller supplies processing PUT/failed-state outcomes not separately
  enumerated by that request spec. V2 spec lines 8–27 establish supported
  synchronous JPEG context, not async coverage.
- Async upload, video/audio, new formats, existing happy CRUD and concurrency
  scenarios are explicitly excluded. Await parent baseline red before any
  phase-2 production change; issue remains open.
- Phase-1 tests are written: ignored test `pinned_media_state_http_matrix` uses
  the real router and the existing reader/writer/owner database environment.
  Status mismatches accumulate so the baseline can report both processing PUTs
  while exercising all 14 requests. Independent read-only source review found
  no actionable scoped issues. No tests/builds/fmt/lint were run; formatting,
  compilation, baseline red, and fixture/CI selection remain with the parent.

## Phase 2: minimal production fix

- Parent reported real HTTP baseline RED in
  `/srv/workspaces/rustodon-audit-green/logs/media-state-red.log`: only owner
  pending PUT `/api/v1/media/940701` and owner in-progress PUT
  `/api/v1/media/940702` returned 404 instead of 206. The other 12 requests
  matched. Parent corrected the test's missing final closing brace before that
  run; the same correction is now applied in this worktree.
- Expanded the locked metadata-update selection in
  `src/mastodon/write_repository.rs` from ready-only to processing states 0/1/2,
  requiring published `file_file_name` metadata. Owner/unattached lookup, failed
  and unpublished staging rejection, transaction/row locking, and processing
  state preservation remain intact. Existing HTTP response handling supplies
  206 and failed-state 422; no `web.rs`, schema, grants, or async changes needed.
- No local tests, builds, formatting, lint, NAS/SSH, or commits performed.
  Await parent green and independent review; issue remains open.

## Completion evidence (2026-09-12)

- Baseline real HTTP matrix: 12 requests matched; owner pending/in-progress PUT
  expected 206 but returned 404. Test initially needed a missing closing brace
  corrected before the behavioral red; this compile error is not the red claim.
- Minimal locked update predicate permits published processing states 0/1/2,
  retaining filename-present, owner, unattached and failed/staging exclusions.
- Permanent `schema-read-test media_state` green: one matrix / 14 requests, with
  exact outcomes and full row/file/cleanup-intent rejection snapshots.
- Full final NAS gates passed: all 11 schema selectors, 85 workers, ordinary
  default/all-feature debug, release all-feature, formatting, strict Clippy and
  offline harness aggregate. Logs in `/srv/workspaces/rustodon-audit-green/logs/`:
  `media-state-{red,green}.log`, `{schema,workers,default,feature,release,clippy}-final.log`.
- Independent review found no blockers. No new formats, asynchronous uploader,
  schema/grants, live deployment or historical mutation.
