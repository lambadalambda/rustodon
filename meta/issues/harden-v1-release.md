# Harden the v1 release

## Summary

Prove crash, retry, resource-failure, small-instance load, and rollback safety.

## Requirements

- Crash every worker lane/handler and exercise duplicate requests/deliveries,
  queue saturation, timeout, disk-full, PostgreSQL failure, and media failure.
- Reopen all newly written database/media state through Mastodon.

## Acceptance Criteria

- No failure duplicates or loses posts, relationships, notifications, media,
  or delivery effects for the documented 1-20-user deployment.

## Progress

- Durable queue coverage already proves stale leases, duplicate logical keys,
  retries, cancellation, dead letters, outbox dispatch, and shutdown timeout
  behavior.
- Added `aborted_handlers_in_every_worker_lane_are_reclaimed`, which aborts a
  leased handler in each of the six lanes and proves the job is reclaimed and
  acknowledged exactly once by a later executor.
- Restored worker integration now passes 44/44, including the all-lane abort
  regression. The full local repository gate and Mastodon schema gate pass.
- The cutover rehearsal now writes a fresh JPEG through Rustodon into an isolated
  media root, stops Rustodon, reopens the same database and media through pinned
  Mastodon 4.6.5, verifies the Rails attachment metadata and original/small
  Paperclip files over HTTP, then restores the attachment row, sequence, and
  filesystem baseline. The rehearsal also removes the old Redis container and
  restores Mastodon with a fresh empty Redis instance. `mise run
  cutover-integration` passes. Paperclip writes explicit `0644` file modes so a
  reopened Mastodon process running as a different user can read newly written
  media.
- Worker integration now covers a PostgreSQL failure after a job is claimed but
  before acknowledgement; the job remains reclaimable by a fresh pool. The
  resource/idempotency burst now runs twenty jobs through four remote permits,
  proving queue saturation does not duplicate effects. Paperclip coverage also
  forces derivative storage failure and proves the partial original is removed.
- The restored worker integration passes 44/44, and the full local test gate
  includes the media-failure rollback case.
- Added deterministic ambiguous remote-media metadata commit coverage. One
  restored-fixture case forces a failure before `COMMIT` and an error after the
  database commit, proving that staged original and derivative files are
  retained and both durable retries reconcile without duplicate effects.
- The complete Rails-versus-Rust differential workflow passes 18 general cases
  plus isolated notification-write and status-authorization phases (20 test
  phases total) against Mastodon 4.6.5.
- Signed ActivityPub POST transport fixtures now cover mixed DNS answers,
  redirect-hop rebinding, timeout, oversized response, and permanent redirect
  rejection. The full `mise run check` gate passes with Clippy, formatting,
  all-target tests, dependency policy, and pinned-fixture verification.
- Added a deterministic test-support Paperclip storage-full fault. The media
  write contract removes both partial files after the derivative write fails,
  and the restored worker fixture proves the database remains unmaterialized,
  the durable job retries, and the same remote media succeeds on the next
  attempt. This is simulated quota-failure evidence; actual filesystem quota
  exhaustion remains open.
- The current local acceptance run also passes the 20-phase differential suite,
  35-case Mastodon schema suite, operational schema and streaming integration,
  startup safety, preflight, worker integration (44/44), and cutover/rollback
  rehearsal. True quota exhaustion, sustained load, hard power-loss, and
  production reopen evidence remain external or future hardening work.

## Remaining

- True disk-full/quota exhaustion, sustained 1-20-user load beyond the bounded
  burst, hard power-loss, and production database/media reopen verification
  remain open.
- Live peer, browser/mobile, and production cutover evidence remain external
  acceptance work rather than claims covered by this local test.
