# Assert remote semantic no-op edit-history behavior

## Summary

The semantic Update regressions now cover persisted history rows and the real
REST history projection alongside edit timestamps, notifications and streams.
The inbound path still does not insert `status_edits`; when history is absent,
the REST loader synthesizes an entry from the current status. Both boundaries
are covered without assuming that all metadata must remain frozen.

## Requirements

- Verify the supported history contract against pinned Mastodon 4.6.5 before
  setting expectations. Keep implementing remote history storage, if needed,
  separate from this focused regression scope.
- Extend the existing semantic-update fixture coverage for a newer sanitized-
  equivalent Update both without a prior edit and after a meaningful edit.
- Assert persisted history rows and the real REST history response before/after,
  including timestamps and fallback behavior when no history rows exist.
- Include meaningful-edit and replay controls. Distinguish unchanged rendered
  content from reconciled metadata (such as sensitivity) that the current-state
  fallback may legitimately expose; do not blindly require byte-identical JSON.
- Preserve history access authorization for private/direct statuses, and avoid
  duplicate tests or unrelated poll/history feature work.

## Acceptance Criteria

- Permanent fixture tests explicitly demonstrate no fabricated history version
  for semantic no-ops and the documented behavior of the current-state fallback.
- Test evidence distinguishes coverage gaps from confirmed production defects;
  any discovered correction receives its own failing regression and review.
- Relevant NAS fixture and ordinary checks pass; no live state is modified.

## Notes

- Follow-up to [semantic no-op edits](suppress-semantic-noop-remote-edits.md).
- Starting points: `tests/workers/semantic_updates.rs` and the history projection
  in `src/mastodon/rest/loader.rs`.
- User approved adding this follow-up after the audit implementation checkpoint.
- Phase 1 owned by Alice in `task/remaining-updates`; tests and this issue only.
- No new history implementation. Parent-reported NAS regression/mutation evidence
  is recorded below; formatting and topical commit remain parent-owned.

## Pinned findings and scope (phase 1)

Oracle: read-only
`/Users/lainsoykaf/repos/rustodon/.local-instance/audit-reference/remaining/`,
provided from pinned image
`sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`,
expected revision `1440d55b139e39ec722c2a3db7f60b66cd889048`.
No source fetch, SSH, image execution, or modification of oracle files.

- `app/services/activitypub/process_status_update_service.rb:43-55,174-184,403-411`
  builds an original snapshot only when history is empty and saves original
  plus current snapshots only for meaningful changes. Sanitized-equivalent
  HTML does not save snapshots or advance the edit timestamp (428-432).
- `spec/services/activitypub/process_status_update_service_spec.rb:59-79`
  explicitly expects no edits and no edit marker for sanitized-out changes.
  Its meaningful-edit control at 526 onward expects original and edited text.
- The inbound significant-field list excludes sensitivity alone, unlike the
  separate local `UpdateStatusService` changed-attribute/rollback policy.
- Rustodon `apply_remote_note_update` has no `status_edits` insertion. Its real
  `ProjectionLoader::status_history` authorizes first, then returns stored rows,
  or a singleton current-state fallback timestamped `edited_at`/`created_at`.
  Sensitivity can legitimately change that fallback without a new history row.
  This fallback description is verified Rustodon behavior, not a claim that
  the supplied oracle includes the upstream history controller/serializer.

The missing meaningful remote history storage is a confirmed **source-level
parity gap**, separate from no-op stability. Phase 1 must not implement it or
silently add schema. Tests characterize the current fallback explicitly,
plus seed stored snapshots as setup to cover preservation/authorization of
existing history. Seeded snapshots are not evidence of inbound history writes.
A future topical storage change must replace the fallback characterization after
meaningful edits with pinned original-plus-edit assertions and receive its own
red/green evidence/review.

## Regression coverage

`tests/workers/semantic_update_history.rs` is a child of the already registered
`semantic_updates` module; shared `tests/workers.rs` and harness files untouched.

- Unedited sanitized-equivalent Update/replay: zero rows, one REST fallback at
  publication time; sensitivity changes without a new history version.
- Meaningful Update/replay then sanitized-equivalent Update/replay: meaningful
  edit effects remain a positive control; current zero-row limitation and one
  REST fallback at the meaningful edit time are explicitly characterized.
- Seeded original + meaningful snapshots: all persisted row bytes/IDs/timestamps
  remain unchanged through no-op/replay. The actual HTTP history preserves saved
  content/CW/sensitivity/times rather than substituting mutable current metadata.
- Private and direct cases each cover both zero-row fallback and a stored snapshot
  before/after no-op/replay: recipient receives 200, anonymous and authenticated
  nonrecipient receive 404 without history text. Outsider token validity and lack
  of grants are checked separately. Only task-owned status visibility is seeded.
- The real WebState/router/loader/serializer uses the restricted runtime pool;
  inbound Updates use the existing restricted-writer worker. Owner SQL is setup,
  observation, and cleanup only. No helper reimplements the history projection.
- Exact history rows were also added to the existing semantic-update state
  comparisons (content/CW/media/count/timestamp controls). New seeded rows are
  explicitly cleaned up; HTTP server is aborted on errors and assertion panics.

No production/schema/grant changes; no storage implementation bundled in these
regressions. This worktree ran no local or NAS test/build/fmt/lint workloads and
made no commits. Parent ran the authorized disposable fixture with existing
worker roles; see reported results below. Shared harness registration and NAS
workspace selection remain untouched here. The issue remains open for parent
formatting/ordinary-check gates and finalization; no shared index/archive edits.

Independent review completed (read-only, no execution): one fixture-precondition
bug found and corrected before handoff. The matrix-viewer token belongs to user
107 / account `-323`, not an inferred positive account ID; the nonrecipient grant
check now uses that exact seeded account. No other substantive compile/fixture,
cleanup, correctness, or architecture blocker identified by inspection. Optional
follower-only success coverage is deferred: this case's authorized recipient has
both follow and mention grants. Existing storage/metadata parity gaps remain
separate, not silently fixed here.


## Parent-reported NAS evidence

- Baseline: all **15** `semantic_updates::` tests passed against unchanged
  production (nine existing, one ordering, five history).
  Log: `/srv/workspaces/rustodon-audit-green/logs/semantic_updates-baseline.log`.
- Controlled mutation M2 changed only the no-row history fallback timestamp
  from `current.edited_at.unwrap_or(current.created_at)` to `current.created_at`
  in `ProjectionLoader::status_history`.
  `semantic_updates::history::edited_noop_history_fallback_preserves_meaningful_timestamp_and_replay`
  failed at its **meaningful-edit positive control**: expected T1
  (`2026-08-25T12:01:00Z`), but REST returned publication time
  (`2026-08-25T12:00:00Z`). This red is not a no-op failure or an existing
  production defect; it shows the real REST fallback timestamp assertion works.
  Log: `history-fallback-mutant.log`.
- M1 separately advanced `edited_at` before the no-op guard and failed the
  ordering test at T3 equality (not T2 rejection); see the ordering issue.
  Parent restored both mutations and all **15** semantic-update tests passed.
  Log: `semantic-updates-restored-green.log`.
- Results were supplied by parent from authorized NAS disposable workloads;
  this worktree did not execute them or independently retrieve the logs.
  Mutation/restore log basenames are recorded exactly as supplied by parent.
- No mutation red is claimed for stored-row authorization or sensitivity:
  those assertions ran in the passing baseline/restored suite. No upstream
  runtime differential or full remote-history storage parity is claimed.
- The missing meaningful remote-history storage remains a separate topical
  scope. Seeded history preservation is not evidence that inbound Updates write
  snapshots. No schema/grant/storage implementation was added.
- Final independent read-only correctness/architecture review approved both
  corrected test files with no remaining blockers, including confirmation of the
  outsider account fix. No refactor requested. Follower-only access remains an
  optional separate coverage item. Parent owns formatting, remaining ordinary
  checks, and commit.

## Completion

NAS formatting and the restored 15-test fixture suite passed. Both controlled
mutations failed at their intended assertions. Final independent correctness and
architecture review found no blockers. Regression scope complete; no production,
schema, grant, deployment or historical replay change.
