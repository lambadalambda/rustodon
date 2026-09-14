# Review writer-pool and lock resource safety

## Summary

Complete the read-only review of advisory-lock callbacks, PostgreSQL pool sizing,
session settings, and concurrent worker write paths.

## Requirements

- Trace every `WriteRepository` pool construction and every lock-held callback.
- Prove that configured worker concurrency cannot exhaust a writer pool while a
  lock-held callback needs additional connections.
- Check lock acquisition order, timeout behavior, filesystem boundaries, and
  retry/rollback interactions.
- Compare relevant domain, actor, media, and durable-job behavior with the pinned
  repository-external pinned Mastodon 4.6.5 checkout.

## Acceptance Criteria

- The final review reports severity-ordered confirmed findings, residual risks,
  test coverage, and a release recommendation.
- No application files or pinned-reference files are changed by the review.

## Findings

### Resolved: Remote media claims can be stranded after handler cancellation

`process_activitypub_media` claims a remote attachment by setting `processing = 1`
(`src/worker.rs:2806,3075-3095`). The worker executor drops a handler future
when lease renewal loses the fence (`src/worker.rs:327-340`), and shutdown can
abort in-flight handlers after its drain deadline (`src/worker.rs:4628-4632`).
Those paths do not run the handler's normal error branches that reset the media
state to `0` or mark it `3`.

The claim path now allows a retry to reclaim an attachment already marked
`processing = 1` while its file name is still unset. State-reset/failure updates
also refuse to overwrite a row whose file has since been persisted. A
`WrittenMediaFiles` drop guard cleans staged files when a handler is cancelled
before the metadata transaction commits. The existing restored-fixture failure
case now seeds `processing = 1` to exercise stale-claim recovery
(`tests/workers.rs:4987-4995`, `src/worker.rs:2806,3075-3096`).

The recovery behavior is covered with a seeded stale claim, but an actual lease
fence is now covered by `activitypub_media_fetch_reclaims_after_lease_fence`
(`tests/workers.rs:5200-5384`). The fixture blocks the first remote response,
expires its durable lease, confirms the fenced handler leaves the durable job and
`processing = 1` attachment recoverable, and verifies a recovery invocation materializes
the original and small Paperclip derivatives without a dead letter. This proves
lease-loss cancellation and sequential recovery; concurrent stale final writes
and a shutdown-cancellation-specific media test remain desirable.

### Resolved: Writer preflight omitted remote tombstone and quote capabilities

The configured writer pool is used by inbound Note, Like, and Announce paths.
The review found that `WRITER_PRIVILEGE_QUERY` did not require
`public.tombstones` access or `public.tombstones_id_seq` usage
(`src/preflight.rs:314-792`). Remote Note and interaction handling reads and
inserts tombstones through that pool
(`src/mastodon/write_repository.rs:2697-2717,3356,3781,4042-4502,10944-11042`).
The differential fixture explicitly revokes all public table and sequence
privileges, so the missing contract could let a writer pass startup validation
and later fail remote delete, Undo-before-activity, or stale-object fencing.

`WRITER_PRIVILEGE_QUERY` now requires the tombstone table and sequence
capabilities, and the differential fixture grants exactly those capabilities.
The startup safety matrix verifies that revoking tombstone access is rejected.

The writer query also reads `public.quotes` while forwarding remote activities
(`src/mastodon/write_repository.rs:3555-3575`) without requiring `SELECT`.
The fixture grants that access, but the production cutover document did not
provide a complete writer ACL contract. `SELECT` on `public.quotes` is now
required by preflight.

### Resolved: Direct INSERT grants outside the allowlist passed preflight

The privilege query validated required `INSERT` capabilities but its negative
ACL check only rejected unapproved `UPDATE` and `DELETE` grants, plus
`TRUNCATE`, `REFERENCES`, and `TRIGGER` (`src/preflight.rs:720-787`). A direct
grant such as `INSERT ON public.settings` or `INSERT ON rustodon.schema_migrations`
therefore left the query true even though neither table was part of the writer
contract.

The negative ACL check now allowlists every supported direct `INSERT` table
grant, including the operational schema tables. The startup safety matrix
verifies that an unexpected direct `INSERT ON public.quotes` grant is rejected.

### Resolved: Account-deletion request row locks required an undeclared privilege

`purge_account_after_deletion` locks the due row in
`public.account_deletion_requests` with `FOR UPDATE`
(`src/mastodon/write_repository.rs:1611-1619`). PostgreSQL requires the writer
to have `UPDATE` on that table even when the query only selects
`created_at`; the least-privilege fixture reproduced the failure as
`42501: permission denied for table account_deletion_requests`.

`WRITER_PRIVILEGE_QUERY` now requires the table-level `UPDATE` capability, and
the differential fixture grants `SELECT, INSERT, UPDATE, DELETE` for the full
request lifecycle. The guarded startup suite includes
`least_privilege_writer_can_lock_account_deletion_requests` and now runs four
startup integration tests successfully.

## Findings Not Confirmed

- The former writer-pool self-starvation finding is not supported by the current
  code. Both lock wrappers open a dedicated `PgConnection` from the pool options
  (`src/mastodon/write_repository.rs:663-705`), so the lock connection does not
  consume a writer-pool slot. Reviewed callbacks use their normal pool
  transactions sequentially. A database-wide connection budget and constrained
  pool stress test remain unproved.
- The former `DB_POOL` mismatch finding is not supported by current call sites.
  Web and all inspected admin paths call `connect_with_pool_size` with the
  selected database's `pool_size` (`src/main.rs:358-675,997-1008`).
- The former single-label lock-scope finding is false. The scope builder includes
  every suffix, including `com`, and the behavior is covered by
  `remote_domain_lock_scopes_cover_single_label_ancestors`
  (`src/mastodon/write_repository.rs:14537-14555,16576-16582`).

## Lock, Filesystem, and Retry Review

- Canonical host normalization is used for domain locks, suffix scopes are
  sorted before acquisition, and reviewed paths acquire domain scopes before
  actor/account work. No inverse advisory-lock order or independent PostgreSQL
  deadlock was found in the reviewed paths.
- `lock_timeout` and pool `acquire_timeout` are both ten seconds on production
  pools. Lock transactions roll back on both success and error, releasing
  transaction-scoped advisory locks.
- `PaperclipRoot` anchors reads, writes, and removals beneath an opened root
  with `BENEATH`, `NO_SYMLINKS`, and `NO_MAGICLINKS`; no independent filesystem
  boundary violation was found.
- Durable domain/account jobs are idempotent across normal retries. Remote media
  files are written before the metadata transaction commits; the cleanup guard
  handles cancellation before commit, while the guard is disarmed before commit
  so an ambiguous commit retains files for retry reconciliation
  (`src/worker.rs:2950-2992,3024-3051`).
- Account purge now records its eligible Paperclip paths in the leased durable
  job before the database transaction, commits the database purge first, and
  removes those paths afterward. A file-removal failure therefore leaves both
  the retryable job and its manifest intact; `due_account_purge_retains_reported_content_and_actor_identity`
  (`tests/workers.rs:8396-9172`) exercises the failure and recovery sequence.
- Remote actor upserts recheck domain policy inside their transaction, and remote
   media rechecks policy in its write transaction while the dedicated advisory-lock
  transaction remains held, preserving the canonical domain-lock coverage
  (`src/mastodon/write_repository.rs:2697-2717`, `src/worker.rs:2906-2948`).
- Relevant domain, actor, media, and durable-job behavior was compared with the
  repository-external pinned Mastodon 4.6.5 checkout. No additional
  confirmed compatibility finding was identified in the reviewed paths.

## Unproved and Deferred Behavior

- No production-load stress test proves the database-wide connection budget
  remains safe under configured worker, web, media, remote-HTTP, and standalone
  lock-connection concurrency combinations.
- Hard power-loss compensation, disk-full behavior, live SMTP, and live peer
  federation remain outside the proven surface. Account purge ordering and
  retryable filesystem cleanup are covered without claiming OS-level crash proof.
- A restored-fixture regression now drives ambiguous metadata commits before and
  after PostgreSQL `COMMIT`, proving staged files remain available for retry
  reconciliation. Concurrent stale and replacement fetches are not exercised at
  final persistence; duplicate fetches are serialized there but may still
  duplicate remote HTTP work.

## Coverage

- `mise run fixture-verify`: passed for the pinned Mastodon 4.6.5 fixture.
- `mise run startup-integration`: all 4 guarded startup tests passed in 453.79s.
- `mise run worker-integration`: all 35 restored-fixture worker tests passed,
  including the live remote-media lease-fence recovery case.
- `mise run preflight-integration`: passed against Mastodon 4.6.5.
- `mise run mastodon-schema-integration`: all 35 tests passed in 17.71s.
- `mise exec -- cargo test --locked --all-targets --all-features`: all non-ignored
  targets passed, including 191 library tests, 6 CLI tests, 21 config tests, 12
  crypto tests, 29 differential helper tests, 7 signature tests, 9 mail tests,
  8 fixture tests, 5 type tests, 1 operational-schema migration-plan test, 12
  preflight tests, 5 REST entity tests, 8 REST HTML tests, 15 serializer tests,
  and 13 Paperclip filesystem tests. The command reported 1 ignored library test
  and the expected guarded integration tests.
- `mise run deny`, `mise run fmt`, and `git diff --check` pass.
- `mise run operational-schema-integration` passed the lifecycle, access-drift,
  composition, stream, rate-limiter, and websocket checks.
- `mise run lint` passes with warnings denied.
- `mise run deny` passes advisories, bans, licenses, and sources checks.
- Direct `cargo` commands use the active Rust 1.97.0 toolchain and fail the
  package's Rust 1.97.1 requirement; checks must run through `mise exec`.

## Recommendation

The confirmed pool, pool-sizing, single-label lock, stale-claim, ACL, and
account-purge ordering findings are resolved in the current code. A writer-enabled
production release still needs load/fault-injection coverage for shutdown
cancellation, hard filesystem/database crashes, and the database-wide connection
budget. This review is not a release approval for the complete v1 surface.

## Resolution

- The review acceptance criteria are complete: confirmed findings were recorded
  with severity, residual risks and coverage, and no pinned Mastodon reference
  files were changed. Remediation changes are tracked in the implementation
  issues and development log.
- The tombstone, quote, direct-`INSERT`, and account-deletion lock privilege
  findings are remediated in the writer preflight contract, differential
  grants, and guarded startup tests.
- Account purge filesystem cleanup is now ordered and retry-safe through the
  durable-job manifest and post-commit removal path.
