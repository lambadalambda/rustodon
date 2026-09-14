# Diagnose private boost Undo returning HTTP 500

## Summary

The real isolated worker Mastodon/Rustodon interactions scenario reaches bidirectional
Like/Undo and delivery of a followers-only Rustodon Announce, then Rustodon's
unreblog endpoint returns HTTP 500. This is synthetic peer evidence, not a live
instance report or a result inferred from worker unit tests.

## Requirements

- Diagnose the existing failing peer case without weakening its assertions.
- Separate application SQL/queue behavior from fixture-role or harness defects.
- Add a focused red regression before any production correction; preserve exact
  Announce/Undo identity, privacy, recipient distribution and transactionality.
- No live replay, deployment, broader grant escalation or historical mutation.

## Acceptance Criteria

- Concrete cause identified with a bounded regression.
- Restricted-writer regression and real interactions scenario pass after a
  separately reviewed topical correction.

## Evidence

- The interactions scenario returned
  `500 {"error":"Internal Server Error"}` from
  `/api/v1/statuses/<original-id>/unreblog` after successful private Announce.
  Detailed evidence is a historical external artifact not in the repository.
- Public, privacy and Note lifecycle peer scenarios passed independently.
- The initial Rustodon web log was empty; the later parent diagnostic is below.
- Follow-up to [isolated worker peer execution](adapt-and-run-peer-matrix-on-isolated-worker.md).

## Independent source diagnosis

- The peer uses the correct original-status ID. Using the wrapper ID would
  avoid removal rather than fix this failure; do not change the test that way.
- Checked-in writer grants cover the removal SQL, including durable-job DELETE.
  Successful Undo Like uses the same delivery-cancellation helper. No grant
  escalation is justified by current evidence.
- The HTTP mapper discards repository errors; a post-commit reload/serializer
  can also return 500. Existing cleanup did not retain the transaction-state
  discriminator or PostgreSQL error log; the later parent diagnostic is below.
- Next bounded reproduction: one interactions run, retaining task-only PG errors,
  effective writer privileges, wrapper deletion/counters and correlated intent
  metadata before cleanup. Stop at first failure; no unnecessary body/token dumps.

## Parent diagnostic baseline and missing-stats refinement

- Parent reports the original two regressions pass on current production
  (`2 passed, 94 filtered`), after replacing unsupported reqwest `Response.json()`
  with `serde_json::from_str(&response.text().await?)`. Those controls used Alice's
  preseeded stats and did not exercise the missing-row condition.
- Parent's actual peer diagnostic now identifies `stage=repository`, before
  commit, SQLx `Discriminant(6)` = pinned sqlx-core 0.8.6 `RowNotFound`, with no PG
  ERROR. Wrapper remained live and target reblog count remained 1.
- The fresh peer run reproduced the missing-`account_stats` condition for the
  original status, wrapper, author, and boost owner. That newly seeded zero-post
  owner had no stats row; only two other fixture accounts appeared in the stats
  snapshot. The detailed historical external artifact is not in the repository.
- Phase 1 refinement: add HTTP/repository variants that remove and retain Alice's
  full stats row before boost in the disposable fixture, restoring it only on
  success. Use nullable stats snapshots so diagnostic SQL itself cannot raise
  `RowNotFound`. Preserve both existing preseeded controls, original status ID,
  delivered Announce/Undo and transaction assertions. No production/grants/schema
  change before parent establishes this focused red.
- Source inspection in this checkout: removal's inline locked stats read uses
  `fetch_one`; there is no named `lock_account_statuses_count` helper here.
  `increment_account_status_count` and `decrement_account_status_count` are
  UPDATE-only and accept zero affected rows. In contrast, target
  `increment_reblog_count` upserts `status_stats`. Read projections left-join
  account stats and coalesce missing counters to zero. The regression must reach
  removal with the missing row, not fail in its own snapshot or silently insert a
  stats row during setup. That established the failing pre-fix state; the
  authorized correction below materializes absent rows rather than preserving
  silent no-op counter writes or swallowing unrelated SQL errors.

## Focused red and authorized correction

- Parent established the focused HTTP missing-stats **RED** after delivered
  Announce: HTTP 500, `deleted=false`, `statuses_count=None`, live status count 9,
  target reblogs 1, Delete/Undo intents 0, unchanged transaction.
- The aggregate four-test attempt was **1 pass / 3 failures** because retained
  failed fixture state contaminated later cases. This is **not** fresh repository
  red evidence.
- Parent subsequently confirmed the repository **RED on a fresh fixture** before
  overlaying the fix: `row_not_found=true`, `transaction_unchanged=true`, account
  stats count `None`, no wrapper deletion and no Delete/Undo outbox intents.
  The parent-reported historical external artifact is not in the repository.
- Parent applied the production and test patches, formatted on an isolated worker, and confirmed
  **all six `private_unreblog` tests PASS in 17.43s**.
  The parent-reported historical external artifact is not in the repository.
  This establishes focused regression green, not real-peer acceptance; the
  post-fix peer scenarios remain pending.
- Production scope is one private `lock_account_statuses_count` helper and its
  two local reblog call sites. Under the existing account/user row locks, it
  locks an existing stats row unchanged or initializes a missing row from live
  non-direct statuses and actual follows. `ON CONFLICT DO NOTHING` plus a separate
  locked read preserves any winning initializer. Creation calls it before wrapper
  insertion; removal before tombstoning. Ordinary increments/decrements then
  apply exactly once. No broad error catch, reset of existing counters, grant or
  schema changes, or changes to shared counter helpers/other write paths.
- `last_status_at` initialization excludes direct/deleted posts, caps future dates,
  and stays NULL for an empty qualifying set. Imported accounts retain their
  stored live non-direct count; fresh accounts start at zero, then increment.
- Updated tests expect materialized counts rather than the historical `None`
  observation. Existing preseeded controls remain. A delivered-wrapper variant
  removes stats only after delivery to exercise the removal initializer separately.
  A fresh local identity test overlaps duplicate/different-target boosts and
  removals, checks direct/deleted exclusions, and proves noncanonical existing
  counters and row identity are preserved (`41 -> 39`, follow counters `7/11`).
- Independent read-only correctness/architecture review found **no unresolved
  in-scope blockers**. The overlap test is coverage, not a deterministic proof of
  lock contention. Malformed imported direct wrappers remain outside this fix.
- Parent completed focused execution and isolated-worker formatting as recorded above. No
  local tests/builds/fmt, external-worker, or commits were performed here. Keep this issue
  open: actual post-fix peer interactions and the wider peer scenarios are pending.

### Incremental overlay handoff (applied by parent)

Parent ran the fresh repository red **before** applying both overlays below. The
pre-fix test source was retained in a historical external artifact not in the repository. The handoff
used separate incremental production and test parts rather than replacing whole
parent files; an ordinary test diff was also retained privately.

The test parts leave `unreblog_http` and `deliver_pair` untouched, including the
parent's reqwest JSON correction. Parent reports successful application followed
by isolated-worker formatting and focused green. Additional selectors in `--test workers` are:

- `private_unreblog::restricted_writer_private_unreblog_imported_wrapper_without_account_stats`
- `private_unreblog::restricted_writer_fresh_boost_counters_serialize_without_resetting_existing_stats`

Use fresh disposable fixture state per case after any failure, exact selectors,
`--features test-support -- --ignored --exact --nocapture --test-threads=1`.

## Phase 1 ownership (historical)

- The tests-only regression was developed separately.
- Parent exclusively owns peer harness diagnostics and all isolated worker execution. No
  local tests/builds/formatting, remote access, commits, production changes or grants changes
  in this phase. Await the focused missing-stats red before a production correction.

## Phase 1 regression handoff (historical; superseded by overlay above)

- New `tests/workers/private_unreblog.rs`, gated by `test-support`, contains four
  ignored cases (two preseeded controls and two missing-stats variants) using the
  existing worker fixture and dedicated `rustodon_differential_writer`. All create
  a private wrapper around a public
  remote target, run distribution and actual signed HTTP delivery to the author
  and an independent follower, then remove using the **original** status ID.
- The HTTP case checks 200 and original-status response identity, visibility,
  relationship and counter. The companion repository case observes the unmapped
  `WriteError` or committed `ReblogWriteOutcome` directly, without retrying the
  failed HTTP mutation. Both check deletion/counter/Undo intent atomic state and
  exact delivered Announce/Undo identity, audience and recipient set.
- Failure output includes original/wrapper IDs and before/after deletion,
  account/status counters and correlated intent counts. Repository errors expose
  only typed discriminants, an explicit `row_not_found` flag and original
  PostgreSQL SQLSTATE (including nested job SQL errors); never `Debug`-dump the raw error, SQL detail/context, headers,
  credentials or activity/response bodies. HTTP mapper errors remain unavailable
  to this tests-only code; the separate repository run is **not** evidence of the
  exact HTTP request's SQL error.
- Failure retains fixture rows for inspection. Run each case on a fresh disposable
  fixture, separately and sequentially, stopping on failure; do not run the second
  case over the first case's retained state. Success restores fixture changes.
- Apply incremental patch parts, **not** the whole worktree file, to preserve
  the parent's isolated-worker formatting and reqwest JSON fix. A historical
  external patch artifact not in the repository supplies an old/new fragment for
  the same Rust file, allowing whitespace-aware application to the parent's
  formatted source. Neither `unreblog_http` nor `deliver_pair` is replaced.
  With the existing worker fixture environment provisioned on an isolated worker, the new
  focused selectors are (commands provided only, not run here):

  ```sh
  cargo test --locked --features test-support --test workers \
    private_unreblog::restricted_writer_private_unreblog_http_without_account_stats \
    -- --ignored --exact --nocapture --test-threads=1
  cargo test --locked --features test-support --test workers \
    private_unreblog::restricted_writer_private_unreblog_repository_without_account_stats \
    -- --ignored --exact --nocapture --test-threads=1
  ```

  `tools/mastodon-fixture worker-test` currently has no case-selector argument;
  this task does not modify that harness or `tools/federation-peer-smoke`.
- TDD red execution and formatting/compilation are deferred to the parent by
  instruction. The prior preseeded controls passed per parent; the new missing-stats
  cases have not run and must not be called red or green until the parent runs them.
  Snapshots retain `None` for missing account stats, independently count live
  statuses, and report `transaction_unchanged`. Expected red is after Announce
  delivery at original-ID removal, with unchanged live wrapper, absent stats,
  target count 1 and zero Delete/Undo intents—not an earlier snapshot error.
- Independent read-only review completed. Corrected and re-reviewed fixture Bob's
  ActivityPub protocol setup/restoration and exact Undo ID/full embedded Announce
  equality (including publication timestamp). The missing-stats refinement and
  patch-part contents also passed independent source-only review with no blocking
  findings. Tests are ready for parent execution; isolated worker-formatted patch application
  and the focused red have not been verified here.
- Peripheral review finding, deliberately not changed: shared
  `fixture_delivery_request` recognizes only title-case `Content-Length:`. A
  lowercase header plus split header/body read can cause a spurious delivery JSON
  failure. Parent should distinguish that fixture failure from unreblog HTTP 500;
  a separate shared-helper correction needs approval.

### Narrow task diagnostics proposal

First use the parent's retained PG errors and pre-cleanup counters/intent metadata
from the unchanged interactions assertion. If these still cannot distinguish the
failing HTTP stage, propose a separately approved, task-only diagnostic at
`status_reblog_write`'s repository-error, post-commit reload-error and serializer
failure branches: stage, original/wrapper IDs when known, outcome flags, typed
error class and SQLSTATE only. No raw error `Debug`, PostgreSQL detail/context,
statement parameters or bodies. This is a proposal, **not implemented**; do not
broaden grants or change the status ID to make a diagnostic run pass.

## Completion

Fresh repository RED confirmed `row_not_found=true` and unchanged transaction;
HTTP RED independently returned 500. All six corrected restricted-writer tests,
the combined 100-worker gate and strict Clippy pass. The real pinned Mastodon
interactions scenario now passes, including private Announce and Undo, as recorded
in the later [five-scenario run](adapt-and-run-peer-matrix-on-isolated-worker.md#remaining-audit-peer-rerun-2026-09-13).
Public/privacy/notes/profile also pass on the same production changes. No grant
widening, blanket error catch, live deployment or historical replay. Independent
correctness/architecture reviews approved the implementation and incremental tests.
