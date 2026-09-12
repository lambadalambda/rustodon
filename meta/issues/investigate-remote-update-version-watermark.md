# Investigate remote Update version ordering after semantic no-ops

## Summary

Inbound Update rejection currently compares the supplied version with the last
meaningful `edited_at` (or `created_at`). A newer semantic no-op reconciles
metadata without advancing that version. After a meaningful edit at T1 and a
no-op at T3, an out-of-order meaningful Update at T2 still applies. The pinned
4.6.5 source confirms this policy; the NAS regression passes against unchanged
production. No separate accepted-version watermark is warranted for this sequence.

## Requirements

- Confirm pinned Mastodon 4.6.5 ordering behavior using the existing read-only
  reference or verified cached-image source; do not infer a contract from 4.7.
- Add a restricted-writer regression for T1 < T2 < T3 delivered as T1, T3, T2,
  including distinct queue identities so enqueue deduplication cannot mask it.
- Distinguish an accepted remote-version watermark from user-visible edit time.
  Cover replay, equal/older versions, metadata reconciliation and edit effects.
- Preserve signer/ownership, timestamp, tombstone, privacy and transaction guards.
  Invalid or rejected activities must not advance any accepted-version fence.
- Decide whether a separate watermark is necessary before proposing schema or
  grant changes. Keep any such change independently reviewed and topical.

## Acceptance Criteria

- Pinned behavior and the chosen ordering policy are documented with evidence.
- A permanent regression establishes that policy; any behavioral correction has
  red/green evidence and does not manufacture edit timestamps or notifications.
- No live replay, historical rewrite or deployment is implied.

## Notes

- Follow-up to [semantic no-op edits](suppress-semantic-noop-remote-edits.md).
- Source entry point: `WriteRepository::apply_remote_note_update`.
- User approved adding this follow-up after the audit implementation checkpoint.
- Phase 1 owned by Alice in `task/remaining-updates`; tests and this issue only.
- No watermark implementation. Parent-reported NAS regression/mutation evidence
  is recorded below; formatting and topical commit remain parent-owned.

## Pinned decision (phase 1)

Read-only source inspected at
`/Users/lainsoykaf/repos/rustodon/.local-instance/audit-reference/remaining/`,
provided from cached pinned 4.6.5 image
`sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`
(expected source revision `1440d55b139e39ec722c2a3db7f60b66cd889048`).
Canonical `/workspace/rustodon/target/mastodon-v4.6.5` and Secunda
`/home/lain/repos/rustodon/target/mastodon-v4.6.5` are unavailable for this
scope; no fetch, SSH, or image workload performed.

- `app/lib/activitypub/activity/update.rb:29-43` locates the owned status and
  delegates to `ActivityPub::ProcessStatusUpdateService`.
- That service, lines 26-35 and 435-436, compares incoming `updated` to
  `status.edited_at`; a strictly newer timestamp enters the explicit path.
  Equal versions enter the implicit path (poll/policy/count/quote-approval
  reconciliation, not text replacement); older versions are rejected.
- Lines 174-184, 403-411 and 428-432 gate `edited_at` and saved snapshots on
  significant changes, comparing formatted HTML rather than wire bytes.
  Sensitivity alone is not significant. A semantic no-op still saves current
  attributes/metadata, but does not advance `edited_at` or save history.
- Therefore T1 meaningful, T3 sanitized-equivalent, T2 meaningful (T1 < T2 < T3)
  **accepts T2**. There is no separate accepted-remote-version fence in this
  path. Preserve this pinned policy; do not add a watermark or schema/grants.
- `spec/services/activitypub/process_status_update_service_spec.rb:59-79`
  directly covers sanitized-equivalent no-history/no-edit behavior; lines
  230-240 cover older rejection. The T1/T3/T2 sequence is a source-derived
  regression, not a claim that an upstream sequence spec or runtime was run.
- Local `app/services/update_status_service.rb:113-127,160-175` uses a broader
  changed-attribute policy and rollback on no changes; do not substitute that
  local-edit contract for inbound processing.

Rustodon's current meaningful-edit fence matches this sequence. Its equal and
implicit metadata handling is not fully identical to upstream (equal is rejected;
implicit counts can reconcile after a prior edit). This is outside the selected
Note ordering regression, not a reason to expand the scope silently.

## Regression coverage

- `semantic_updates::newer_noop_does_not_fence_an_intermediate_meaningful_edit`
  delivers distinct jobs for T1 real edit, T3 sanitized-equivalent plus sensitivity
  refresh, T3 replay, T2 real edit, T2 replay, older conflicting text, and equal
  conflicting text. It checks accepted rendered text/metadata, last meaningful
  edit time, exact history rows, notifications, and both update stream intents.
- Existing semantic-update state comparisons now include full ordered
  `status_edits` rows (including IDs/timestamps), not only edit timestamps/effects.
- No production changes. This is a regression characterization of existing
  correct behavior; the controlled-mutation evidence below is not a production
  defect reproduction.
- No tests, builds, formatter, lint, SSH/NAS workloads, or commits run here.
  Parent owns sequential fixture execution and any shared harness registration.
  Issue remains open for parent finalization; no shared index/archive edits.

Independent read-only review completed: pinned no-watermark inference and
ordering/control coverage confirmed; no ordering or architecture blocker found.
The sibling history test's outsider-account precondition was corrected per review.
Final independent read-only correctness/architecture review approved the corrected
test diff with no remaining blockers. Parent owns formatting and the topical
regression-test commit; no ordinary-check result claimed here.


## Parent-reported NAS evidence

- Baseline: all **15** `semantic_updates::` tests passed against unchanged
  production (nine existing, one ordering, five history).
  Log: `/srv/workspaces/rustodon-audit-green/logs/semantic_updates-baseline.log`.
- Controlled mutation M1 moved the existing `edited_at` SQL update before the
  semantic no-op equality guard in `apply_remote_note_update`, retaining its
  early return. The ordering regression failed at the **T3 state-equality
  assertion**, because the no-op advanced edit time. It stopped before T2:
  this red run did **not** observe or prove T2 rejection.
  Log: `ordering-stamp-mutant.log`.
- The separate history fallback mutation M2 also failed its targeted control
  (see the sibling history issue). Parent restored both mutations, then the
  complete **15-test** semantic-update suite passed again.
  Log: `semantic-updates-restored-green.log`.
- These are parent-reported executions on the authorized NAS disposable fixture;
  this worktree did not execute workloads or independently retrieve the logs.
  Mutation/restore log basenames are recorded exactly as supplied by parent.
- Passing baseline/restored ordering runs exercise T1, T3 no-op/replay, accepted
  meaningful T2/replay, and equal/older conflicting-text controls. The M1 red
  proves sensitivity to fabricated no-op edit time, not a previously existing
  production defect or a separate watermark implementation.
- No schema/grant changes, live replay, history rewrite, or deployment. Full
  remote-history storage and equal/implicit metadata differences remain outside
  this focused ordering scope.

## Completion

NAS formatting and the restored 15-test fixture suite passed. Both controlled
mutations failed at their intended assertions. Final independent correctness and
architecture review found no blockers. Regression scope complete; no production,
schema, grant, deployment or historical replay change.
