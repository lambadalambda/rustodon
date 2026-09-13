# Port failed parent-fetch recovery and distribution matrix

## Summary

Extend existing reply hydration workers with transient failed parent-fetch followed by recovery, retaining correct author/privacy/thread identity and reply distribution.

## Acceptance Criteria

Verify pinned expectations, durable retry/idempotency and final received thread/notification state. Distinguish per-status notifications from true duplicates; wire into the permanent worker gate. Do not replay historical/live jobs.

## Notes

- Subissue of [selected matrix ports](port-mastodon-media-and-browser-matrices.md).
- Tests first; separate topical implementation and independent review.
- Phase 1 owned in `ilar-task-remaining-reply` (`task/remaining-reply`): tests and this issue only; parent owns shared registration and all execution.
- Inspect existing reply hydration coverage and the read-only extracted cached 4.6.5 oracle before adding the transient-failure/recovery matrix. No production changes, local workloads, NAS/SSH, commits, or live replay.
- Standalone test/mutation verification complete by parent-reported NAS evidence
  below; combined-worker integration remains pending after a follow-prerequisite
  failure. No production defect or firsthand execution claimed by this worktree.

## Completion evidence — parent-reported

| Run | Selected / result | Evidence log |
| --- | --- | --- |
| Unmuted baseline | 3 selected, 3 PASS | `parent-fetch-unmuted-green.log` |
| R: retry policy mutant | 3 selected, 3 FAIL at intended retry assertion (parent-formatted line 198) | `parent-fetch-retry-mutant.log` |
| T: thread repair mutant | 3 selected, 3 FAIL at intended thread assertion (line 229) | `parent-fetch-thread-mutant.log` |
| D: home distribution mutant | 3 selected, 3 FAIL at intended home assertion (line 242) | `parent-fetch-distribution-mutant.log` |
| Restored baseline | 3 selected, 3 PASS in 3.05s | `parent-fetch-restored-green.log` |

Parent confirms no setup/compile failures, sequential controls, and source restoration
after each mutation. These are controlled behavioral reds followed by restored green,
not manufactured production defects. `[child, parent]` remains the feed expectation.
The logs/results are supplied by the parent; no log retrieval or execution here.

Independent read-only reviews covered test correctness/compactness, fixture
corrections, oracle limits and exact mutation controls. The later combined-worker
fixture correction below has also been independently reviewed; its execution is
pending. The standalone green and R/T/D proofs above remain valid evidence for their
source revision, not proof that the new combined correction has run.
Parent formatting and the test-only commit remain parent-owned. Shared
gate/index/archive changes remain parent-owned; this worktree does not claim their
completion. The limited extracted-oracle qualifications below remain unchanged.
No workload, formatting, NAS/SSH operation or commit performed by this worktree.

## Combined-worker follow prerequisite correction

Parent reports all three combined cases fail before guarded setup at the initial
follow-count assertion (current main formatted line 88): actual `1`, expected `2`.
The initial `2` assertion was an assumption about seed survival, not explicit setup.
Current main was inspected read-only; only this worktree's test/issue are edited.

Correction:

- Inside guarded setup, insert only missing Alice → Bob / Alice → parent-author
  follows using `ON CONFLICT (account_id, target_account_id) DO NOTHING RETURNING id`.
  Existing complete follow rows remain untouched. Capture only newly created IDs.
- Keep the **exactly two follows** assertion immediately after setup. All recovery,
  private/direct access, per-status notifications and `[child, parent]` expectations
  remain unchanged.
- Cleanup deletes only captured IDs, restoring entry absence as well as preserving
  entry-present relationships, even after returned errors or caught assertion panics.
- Replace the requirement that a seed exclusive-list membership existed with an
  assertion that no exclusive membership suppresses Bob after isolation. The schema
  has `list_accounts.follow_id ... ON DELETE CASCADE`; earlier follow deletion can
  legitimately leave no membership to remove. Restore only memberships present on
  entry, not arbitrary seed rows.

Independent read-only correction review found no blocker: SQL/unique constraint,
insert-only preservation, cleanup ordering and both missing/existing entry cases
match the schema. No abstraction or shared-file fix needed.

Independent current-main root trace identified
`lifecycles::accepted_remote_follow_preferences_preserve_relationship` as a
successful-path source of missing Alice → Bob: `tests/workers/lifecycles.rs:67–76`
deletes the original relationship without saving it; `:207–235` explicitly unfollows;
`:237–255` restores account/token settings but not follows or list memberships.
`reset()` only truncates operational `rustodon.*` tables, not public relationships.
Deleting seed follow `8001` cascades both normal/exclusive Bob memberships (`9003`,
`9004`). If this test precedes recovery on the same database, Alice → parent `8010`
remains and the two-target count is exactly one. This is a source-confirmed mutator,
not independently observed execution order. No matching successful-path remover of
Alice → parent-author was found.

The earlier Like/Announce/Undo worker test also temporarily removes Alice → Bob,
restoring it only if it existed on entry; it does not repair lifecycle contamination.
Its normal-list membership cascade residue is separate scope and left untouched.
No evidence connected these mutations to deletion of Alice → Bob mute `9503`, so the
exact mute seed-count assertion remains unchanged rather than expanding this fix.

Combined and standalone reruns of this correction remain parent-owned/pending; no
workload, formatting, NAS/SSH operation, production edit or commit performed here.

## Phase 1 matrix

New file: `tests/workers/parent_fetch_recovery.rs` (three ignored integration tests):

- `public_reply_recovers_after_parent_fetch_503`: public child, unlisted parent.
- `followers_reply_keeps_privacy_after_parent_fetch_503`: followers-only child, public parent.
- `direct_reply_keeps_privacy_after_parent_fetch_503`: explicit direct child, public parent.

All cases ingest Bob's reply through the post-signature-verification ingress worker;
fetch the missing parent by a different existing remote author (`timeline_author`);
serve HTTP 503 then HTTP 200 through the existing loopback fixture; and advance only
the failed fixture job's `run_at` for the second attempt. Production writes use the
restricted writer, not the owner. Owner access is limited to fixture setup/cleanup:
insert missing Alice follows without altering existing rows, then delete only those
inserted IDs on cleanup; temporarily remove and fully restore Bob's exclusive-list
memberships and Alice's Bob mute so the home-feed transition tests thread resolution
rather than independent list/mute exclusion; clean up new statuses; and restore
existing account counters. The parent author has no
`account_stats` row in this fixture, so only Bob's existing row is expected/restored.
Assertion failures also run cleanup.

Assertions cover:

- Child committed before hydration; one durable resolver with the original arguments,
  bounded retry budget, recorded HTTP 503 error, live state and future backoff.
- Failed fetch leaves the parent absent and persisted child/mentions/notifications/
  counters/distribution intents unchanged.
- Recovery acknowledges the **same** durable job; two signed GETs target the exact
  parent path. Author, visibility, parent link and reply-account link are correct;
  the child's existing conversation identity is retained.
- Public reply increments the parent's public reply count once; private/direct
  replies do not increment it.
- Exactly one legitimate mention notification for the child and a separate one for
  the parent, each with the right source author and recipient. Never collapse two
  statuses into an incorrect “one notification per thread” expectation.
- The received home feed transitions from an unresolved reply excluded from the feed
  to both statuses present once; the received child context has the fetched parent.
  Anonymous/nonrecipient readers cannot gain access to private/direct replies.
- Parent home-stream intent exists once; no private child status stream to an
  unauthorized recipient, and no outbound forwarding from this remote hydration.
- Fresh-key duplicate Create, resolver and notification jobs exercise real handlers;
  complete row/counter/outbox snapshots remain stable, with no pending/dead
  ingress/pull/core work. These are newly constructed fixture jobs, not live replay.

### Existing coverage inspected / intentionally not repeated

- `tests/workers.rs::activitypub_reply_resolution_repairs_child_after_parent_arrives`
  covers a parent received through ingress *before* the resolver runs, local computed
  URIs, and signed forwarding/lifecycle cases. It does not drive failed HTTP hydration.
- `tests/workers/reply_emoji_grants.rs` covers successful fetched-parent persistence,
  emoji/media work and restricted grants, not fetch failure/recovery.
- The existing URI-Create resolver test uses `fixture_retry_activitypub_server` for
  503 recovery of the received Note itself. This matrix reuses that HTTP fixture but
  targets the separate parent-thread resolver and already-committed child.
- `tests/workers/direct_visibility.rs` covers general audience classification/REST
  privacy. This matrix tests privacy specifically across failed/recovered hydration,
  not every general audience combination.

## Pinned oracle and scope of claims

Read-only extracted oracle supplied by the parent:
`/Users/lainsoykaf/repos/rustodon/.local-instance/audit-reference/remaining/`, from
cached Mastodon 4.6.5 image
`sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`.
The canonical `/workspace/rustodon/target/mastodon-v4.6.5` and Secunda
`/home/lain/repos/rustodon/target/mastodon-v4.6.5` are absent locally. Nothing was
fetched, and the supplied extraction was not modified.

Inspected oracle:

- `app/lib/activitypub/activity/create.rb:34–43,69–91,103–117,373–376`:
  existing-status author protection; persistence followed by independent thread
  resolution and distribution; account/privacy/thread parameters; unresolved reply
  jobs. Distribution explicitly includes home/list feeds and mentioned-account
  notifications.
- `app/services/activitypub/fetch_remote_status_service.rb:28–56,86–98`:
  fetched object/actor identity, trustworthy attribution, existing-status handling,
  and fetch failure handling.
- `spec/services/activitypub/fetch_remote_status_service_spec.rb`: Note/Create
  import, wrong-ID rejection, existing-status fetch/deletion cases, and reply-chain
  discovery limit.

The extracted fetch service **does not establish an HTTP-503 durable retry contract**:
its `UnexpectedResponseError` rescue returns on non-404 responses. Therefore 503
retry/backoff and exact Rustodon queue fields are explicitly a Rustodon reliability
contract (`remote_thread_fetch_failure`), not falsely attributed upstream behavior.
Likewise this matrix checks Rustodon's PostgreSQL home-feed projection and durable
stream intents, not Mastodon's Redis scheduling timing or live WebSocket receipt.

## Handoff / verification

- Shared `tests/workers.rs` registration remains parent-owned. Add the module under
  `#[cfg(feature = "test-support")]` with path `workers/parent_fetch_recovery.rs`.
- Parent should wire the `parent_fetch_recovery::` filter into the permanent worker
  gate and run all three cases serially in the disposable NAS fixture with
  `test-support` and `--ignored`; no shared harness/gate file changed here.
- **No local tests, builds, formatting, lint, NAS/SSH workloads, or commits run.**
  Phase 1 is test preparation, not verified red or green; TDD execution is delegated
  to the parent by explicit instruction.
- Source inspection suggests existing behavior should pass. Run the unchanged
  baseline first. If green, demonstrate red with a parent-controlled temporary
  mutation (e.g. acknowledge a missing-parent resolution without repairing its
  link), then restore and rerun green. Do not manufacture a production defect.
- Independent read-only correctness/architecture review found two test-fixture
  blockers, not production defects: no parent-author `account_stats` row, and Bob's
  exclusive-list membership hiding the recovered child. Both are corrected in the
  test only. Focused independent re-review confirmed both blockers resolved, no new
  blocking issue, and no refactor needed. **Phase 1 ready for parent baseline**;
  expected green by source inspection, execution unverified. Issue remains open
  pending registration and execution.

### Parent baseline follow-up

Parent reports all three serialized baseline cases reached the expected transient
HTTP-failure boundary but stopped on the test's incorrect `last_error.contains("503")`
assertion. Actual error:
`remote reply parent fetch is temporarily unavailable: remote HTTP response was unsuccessful`.
`RemoteFetchError::UnexpectedStatus` deliberately omits the status number in its
Display implementation; the test now asserts the actual retryable-status category
message. No production change and **not a behavioral red**. No concurrent-suite
interference is claimed; parent suites were serialized.

Parent must rerun the unchanged-production baseline through the subsequent recovery,
privacy/thread/feed, notification and idempotency assertions before mutation controls.
No test, formatting, NAS/SSH workload or commit performed for this correction.
Focused independent read-only review confirmed the exact message matches both
production error layers, retains a meaningful retryable-HTTP boundary, and leaves
all subsequent recovery assertions unchanged. **Correction patch ready**; later
baseline steps remain unverified until the parent reruns them.

### Second baseline: feed exclusion diagnosis

Parent reports `parent_fetch_recovery-baseline2.log`: all three serialized cases
pass recovery, repaired author/thread/privacy rows and per-status notifications,
then fail the home-feed assertion with only the fetched parent ID. No log fetched
or workload run here.

Source diagnosis:

- Alice follows Bob (`follows.id = 8001`) and parent author `-330` (`8010`):
  `fixtures/mastodon/v4.6.5/seed.sql:792–803`. The test also asserts both relationships.
- Exclusive-list interference is already removed/restored by guarded fixture setup.
- A second, independent exclusion remains: mute `9503`, Alice → Bob,
  `hide_notifications = false`, `expires_at = 2026-08-01`:
  `seed.sql:845–851` and `database.sql:5482–5483`.
- `src/mastodon/repository.rs:1327–1328` excludes a home status whenever that mute
  row exists, without testing its timestamp. This affects all three child audiences;
  the public/unlisted parent by the unmuted author is unaffected.
- Passing mention notifications do not prove home eligibility: this mute explicitly
  leaves notifications enabled. Passing repaired-link assertions rules out a missing
  thread link at the observed failure boundary.
- Rustodon removes timed mutes through its maintenance handler
  (`src/worker.rs:5365–5377`); this restored fixture does not run that maintenance
  path during the recovery test. Timestamp alone must not be assumed to mean the
  fixture has no mute relationship.

The two-status feed assertion is retained. Independent read-only pinned policy
review confirmed this baseline is confounded by the mute and recommended isolating
the unmuted-audience prerequisite, not changing production filtering. The test now
saves/deletes/restores complete Alice → Bob mute rows inside guarded setup/cleanup,
and explicitly asserts neither author is muted before ingestion. All recovery,
privacy, home/context, notification and idempotency expectations remain unchanged.

Oracle qualification: the supplied Create source establishes separate distribution,
thread resolution and explicit-direct classification. The extracted `FeedManager`,
`ThreadResolveWorker` and `UnmuteWorker` implementations are absent; recipient-direct
home filtering, post-resolution Redis reinsertion and mute-expiry read semantics
were **not independently certified** from those missing files. The retained feed
expectation is for Rustodon's deliberately unmuted, followed, explicitly addressed
PostgreSQL projection scenario, supported by its current query and existing resolved
reply coverage. No production recovery or expired-mute bug is claimed from this
fixture failure. No production edit authorized or made.

Focused independent review confirmed the SQL/JSONB types, narrow fixture scope,
full-row restoration on returned errors/caught panics, and unchanged two-status
expectation. **Fixture correction patch ready**; parent must rerun the complete
serialized baseline before treating this matrix as green or starting mutation
controls. No workload, formatting, NAS/SSH or commit run here.

## Parent-reported green baseline and bounded mutation plan

Parent reports a fresh NAS run after the scoped mute fixture correction:
**all three cases PASS**, log `parent-fetch-unmuted-green.log`. This is reported
parent execution evidence, not a workload or log inspection performed in this
worktree. The preserved `[child, parent]` feed expectation passed. Earlier failures
were test-error/fixture-boundary corrections, not established production defects.
The bounded plan below was subsequently executed by the parent, with results in
Completion evidence above. It was not applied or executed by this worktree. The
limited extracted-oracle qualification above still applies.

### Parent execution protocol

1. Use a disposable parent-owned NAS workspace with the exact source/tests of the
   green baseline. Preserve that baseline's source bytes, including parent formatting;
   do not restore from this worktree or overwrite unrelated edits.
2. Apply **one** mutation at a time, only inside the named function. Match the old
   fragment exactly and require a single match; stop on drift rather than guessing
   by line number. Keep tests and fixture isolation unchanged.
3. Reuse the green baseline worker harness/environment and `test-support` build
   configuration, retaining the parent-owned module registration. Rebuild the test
   executable from each mutated source on the authorized worker; never reuse a stale
   baseline executable. Exact test binary: `workers`. Filter for each control:
   `parent_fetch_recovery::` with `--ignored --test-threads=1` (three tests expected).
   Full names, if the harness requires individual selectors:
   - `parent_fetch_recovery::public_reply_recovers_after_parent_fetch_503`
   - `parent_fetch_recovery::followers_reply_keeps_privacy_after_parent_fetch_503`
   - `parent_fetch_recovery::direct_reply_keeps_privacy_after_parent_fetch_503`
4. Require the indicated assertion failure in all three cases—not a compile error,
   zero selected tests, fixture/SQL error, timeout, or failure at an earlier unrelated
   boundary. Keep the log and identify the control. Do not run suites concurrently.
5. Restore the exact baseline bytes after each control, even when execution fails;
   verify no mutation remains before starting another. Finally rerun the unchanged
   three-test baseline green and retain that log. No commit/live job replay.

### R — retry classification control

File `src/worker.rs`, function `remote_thread_fetch_failure`. Replace exactly:

```rust
            HandlerFailure::retry(format!(
                "remote reply parent fetch is temporarily unavailable: {error}"
            ))
```

with:

```rust
            HandlerFailure::permanent(format!(
                "remote reply parent fetch is temporarily unavailable: {error}"
            ))
```

This deliberately dead-letters the fixture's 503 without changing its displayed
error. Expected failure: **`503 retains the same live durable job with backoff`**,
the `(attempts, max_attempts, dead_at IS NULL, run_at > now)` tuple assertion. The
mutant's job is no longer live (`dead_at IS NULL == false`), rather than the required
`(1, 4, true, true)`. This is a retry-policy control, not another message-string test.

### T — post-fetch thread-repair control

File `src/worker.rs`, function `process_activitypub_thread_resolution`. Replace only
its **final** repair/result block, after successful `.apply_remote_note_create(...)`:

```rust
    if writer
        .resolve_remote_note_thread(child_status_id, parent_uri, config.origin.as_str())
        .await
        .map_err(|error| remote_thread_write_failure(&error))?
    {
        Ok(())
    } else {
        Err(HandlerFailure::retry(
            "fetched remote reply parent was not persisted",
        ))
    }
```

with:

```rust
    Ok(())
```

Leave the initial local-parent check and all fetching/import logic unchanged. The
mutant acknowledges the retry and persists the correct parent but fails to attach
the existing child. Expected failure: **`repair links the actual author without
replacing child identity, privacy or conversation`**. Child `in_reply_to_id` and
`in_reply_to_account_id` remain `None`, instead of `Some(parent_id)` and
`Some(PARENT_AUTHOR)`. It should reach this assertion after the original job was
acknowledged, both signed GETs were observed, and the parent identity check passed.

### D — followed-parent home-distribution control

File `src/mastodon/repository.rs`, function `rest_home_timeline_ids`. In that
function's home-feed reply-follow clause, replace exactly these Rust string lines
(the trailing backslashes are literal source continuation characters):

```rust
                   OR EXISTS (SELECT 1 FROM follows reply_follow WHERE reply_follow.account_id = $1 \
                     AND reply_follow.target_account_id = status.in_reply_to_account_id)) \
```

with:

```rust
                   OR EXISTS (SELECT 1 FROM follows reply_follow WHERE reply_follow.account_id = $1 \
                     AND false AND reply_follow.target_account_id = status.in_reply_to_account_id)) \
```

This disables only the followed-other-parent admission branch; author/self-reply
and reply-to-viewer branches, persisted thread repair, privacy checks, and fixture
follows/mutes stay untouched. Expected failure: **`recovered reply and parent are
each distributed once in the home feed`**: actual `[parent_id]`, expected the sorted
`[child_id, parent_id]`. Both authors differ from Alice and each other, so every
case depends on precisely this branch. Recovery/thread/count/notification assertions
before the home assertion should pass. This is a PostgreSQL received-feed policy
control, **not** evidence about Mastodon Redis reinsertion or live stream receipt.

Independent read-only review confirmed all three exact fragments, type/SQL
plausibility, intended earliest assertions, and one-at-a-time restoration protocol.
No blocking plan defect found; the parent subsequently confirmed all three intended
behavioral reds and restored green (see Completion evidence).
No production mutations, workload, formatting, NAS/SSH operation or commit
performed by this worktree.

Nonblocking review limits retained: the positive stream assertion is for the
parent, not a recovered-child live stream delivery; account cleanup restores
counters/`last_status_at` but not `account_stats.updated_at` (minor fixture metadata
residue, deferred rather than expanding this phase).


## Completion

The self-contained relationship setup passed the corrected **100/100 combined
worker gate** on NAS (`workers-corrected.log`,375.35s), with strict all-target/
all-feature Clippy green (`clippy-corrected.log`). Earlier focused R/T/D mutations
each failed all three intended assertions, then restored baseline passed3/3.
The follow/membership correction was independently reviewed; it retains both
required follows and every privacy/thread/home assertion. No production change.
