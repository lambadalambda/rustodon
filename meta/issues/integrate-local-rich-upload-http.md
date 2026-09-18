# Integrate local rich-upload HTTP

## Summary

Bounded HTTP slice of [Support local rich-media uploads](support-local-rich-media-uploads.md), on reviewed persistence/worker main `dee7526`.

## Requirements

- Retain accepted terminal failures as processing=3 before enabling v2; no invented final filenames or preview URLs.
- V2 staging before durable private raw writes, then transactional acceptance/job intent under account lock and authorization. Preserve legacy v1 and ordinary JPEG success.
- Stable POST 202 ID/url:null, owner GET 206 pending then 200 ready; failed GET/PUT 422 `Error processing thumbnail for uploaded media`, other owner 404. Failed PUT must not mutate.
- Pending edits survive completion. Explicit pending/failed deletion fences late workers and durably cleans raw/output manifests. Ready attachment eligibility still requires processing=2.
- Every advertised MIME accepted via bounded processing; image CPU stays off async executor. Cheap invalid MIME/size fails 422 without orphan ownership.
- No remote/search/browser, new schema/privileges, or openat2 weakening. Worker configuration/readiness must cover acceptance.

## Acceptance Criteria

- Focused API/database/worker tests prove held-worker pending polling, stable identity, scopes/owner isolation, edits, attachment refusal, ready/failed polling, deletion and replay.
- Run bounded task-owned PostgreSQL 14 and real codec evidence with supplied NAS tools image, plus worker regression; record actual red/green and limitations.
- Parent independent review, uncommitted changes. Keep open until separate browser acceptance; no overall completion claim.

## Notes

- Read prior DEVLOG/testing evidence; reuse exact tools image and bounded resources, no production operations.
- Modern still decoding may be asynchronous rather than pinned synchronous behavior; document this safe ordinary-client-compatible difference.
- Stop for scope checkpoint before schema/privilege expansion, and after no more than two substantive review/fix rounds.

## Implementation and evidence (2026-09-18, uncommitted)

- V2 external formats stage a filename-null row and private raw manifest, synchronously
  write/fsync under the account lock, then accept and enqueue the existing generation-keyed
  process intent atomically. Ambiguous staging/accept commits reload the exact identity;
  unresolved outcomes retain ownership rather than speculatively unlinking.
- Acceptance moves processing to 1 (staging remains 0). The owner sees POST 202 with the
  stable ID and null URL/preview, GET/PUT 206 pending, and GET 200 ready. Initial metadata
  and later pending edits are preserved by publication. Legacy v1/JPEG success remains 200.
- Accepted terminal failure now retains processing=3 and no invented filename. Failed
  GET/PUT return 422 `Error processing thumbnail for uploaded media`, other owners 404;
  the writer rechecks failure under the lock, so PUT cannot race into mutation.
- Pending/failed deletion leaves the exact durable manifest for processor/recovery cleanup.
  Failed cleanup validates the surviving owner before unlink and retires ownership only
  after durable removal; replay cannot publish a failed/deleted row. Ready attachment
  eligibility remains processing=2, unchanged.
- Legacy image decode/resize and rich-image/preview normalization use `spawn_blocking`;
  acceptance hashes raw bytes off the web executor. Raw write/fsync intentionally remains
  synchronous under the lock so cancellation cannot leave a detached writer behind cleanup.
- Modern stills are intentionally asynchronous, unlike the pinned controller's synchronous
  modern-still processing. Declared MIME/size errors fail immediately without staging;
  byte/MIME mismatch may instead produce accepted 202 followed by terminal 422. This
  follows the pinned composer's ordinary 202/poll/error contract, not implementation parity.
- Worker registration remains on Maintenance/Media with the configured writer and media
  root. Heartbeats advertise both local handlers only on the selected Maintenance lane;
  writable `admin worker-readiness` rejects a generic/missing/stale local processor.
  `/ready` remains the existing database readiness check, not a codec/worker certification.
- Baseline red: terminal-failure test observed a deleted row; HTTP test got HEIC 422 rather
  than 202. Green: HTTP lifecycle 1/1 covering all **24** external MIME types with real
  codecs; persistence 5/5; local worker 9/9; pinned legacy media state HTTP 1/1; legacy
  local-media cleanup worker 1/1. Focused strict Clippy and formatting pass.
- Evidence is under `/srv/workspaces/rustodon-upload-http-dee7526-alice/evidence/`;
  exact runner, selectors, resource limits, ancillary ordinary checks, and limitations are
  recorded in DEVLOG. These are task-owned PostgreSQL 14.23/owner-connection HTTP and
  worker tests, plus the separate narrow restricted-role persistence test—not an end-to-end
  least-privilege lane or full fixture-matrix result.
- No schema/grant/remote/search/production/openat2 changes. Parent independent review and
  separate browser preview/playback/post/reload acceptance remain outstanding. Keep open.

## Follow-up evidence scope (2026-09-18, before commit)

- Replace owner-connected lifecycle execution with explicit narrow runtime/writer
  connections; owner remains only for fixture setup/assertions. Apply unchanged
  documented writer grants and the existing runtime contract, not permissive grants.
- Rerun the focused HTTP/worker lifecycle and verify actual worker capability
  heartbeat/readiness behavior on fresh bounded NAS PostgreSQL 14 resources.
- Test wiring/docs only while parent review is ongoing. Report permission failures
  and a minimal proposal before any implementation or grant correction. No browser,
  production, broad matrix, feature expansion, or commit.

### Restricted-role follow-up result

- The lifecycle now requires distinct owner/runtime/writer URLs. HTTP reads, rate
  limits, queue dispatch and readiness use runtime; HTTP writes and worker handlers
  use writer. Owner is only fixture setup/assertions. No `src/` or grant changes.
- Used unmodified documented writer/refresh-function SQL and the exact runtime SQL
  from `bootstrap::apply_runtime_grants`. Roles have no ownership, memberships,
  superuser, role/database creation, replication, or RLS bypass privileges.
- Intentional owner-fallback negative run fails the role guard. Actual forbidden
  runtime public writes/private upload reads and writer heartbeat reads/job inserts/
  private-key updates return SQLSTATE 42501. **No unexpected application permission
  denial occurred**, so no implementation or grant correction is proposed.
- Fresh restricted HTTP/worker lifecycle passes **1/1**, all 24 external MIME types
  and the same pending/ready/error/edit/delete/replay/attachment/scopes cases. Real
  restricted worker heartbeat/readiness passes **1/1**, including generic worker,
  writer without media root, wrong selected lane, valid local Maintenance, refresh,
  and shutdown. This executes the capability predicate used by writable admin
  readiness, not a separate full CLI/startup gate.
- Test setup corrections only: supply writer-role context to the existing schema
  validator; wait for both separately emitted worker/scheduler heartbeats. Focused
  strict Clippy and formatting pass. Evidence:
  `/srv/workspaces/rustodon-upload-http-roles-dee7526-alice/evidence/`.
- Earlier owner-connected results remain historical evidence; this follow-up adds
  narrow-role end-to-end proof without widening privileges. Still uncommitted and
  open for parent review and the separate browser slice.
