# Suppress semantically unchanged inbound edit effects

## Summary

A genuinely newer inbound Update that sanitizes to unchanged content still changes edited_at and emits edit effects, unlike pinned Mastodon.

## Requirements

- Port pinned sanitized-equivalent HTML Update regression with a genuinely newer timestamp.
- Compare the meaningful fields relevant to current supported behavior; preserve real content/media/poll changes and metadata reconciliation.
- Avoid spurious edit notifications/streams while retaining timestamp/idempotency/privacy fences.

## Acceptance Criteria

- Red proves unchanged rendered content creates an edit effect.
- Green proves equivalent updates do not mark edited or emit edit effects; meaningful updates still do, and old/duplicate/implicit update behavior remains covered.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- Implementation plan recorded on 2026-09-12; no live deployment is implied.

## Status: open — parent green and review pending

### Phase 1: behavioral red (parent-owned isolated worker execution)

- The historical external run artifact is not in the repository.
- Parent reported **5 passed, 4 failed** for the nine-case
  `semantic_updates::` worker module against unchanged production code.
- Failures:
  - `newer_equivalent_html_with_unchanged_media_does_not_emit_edit_effects`
  - `newer_sanitized_equivalent_html_does_not_create_an_edit`
  - `newer_sanitized_equivalent_html_preserves_an_existing_edit`
  - `unchanged_render_reconciles_interaction_counts_without_edit_effects`
- These use genuinely newer wire timestamps, not stale-input rejection. The
  state comparison includes rendered HTML, edited_at, durable update intents,
  materialized notifications, and both status-update stream kinds. Real content,
  CW, media, and timestamp/replay controls also exercise the restricted writer.
- Test drafting received a read-only review. Its media-cleanup finding was fixed:
  attachments must be deleted explicitly because the status FK uses SET NULL.

### Phase 2: scoped implementation (not yet green)

- Snapshot the same small edit projection before and after existing inbound
  reconciliation: REST-sanitized HTML, CW, ordered media identity/descriptions.
  Reuse `HtmlFormatter::remote_fragment`; do not introduce another sanitizer or
  compare storage HTML bytes/cache-maintenance timestamps as rendered content.
- Keep metadata/media/emoji/tag/mention/count reconciliation in the transaction.
  Preserve mention-notification handling separately from edit notifications.
  Only a changed edit projection sets edited_at and records update effects.
- Existing ownership, signer/host, tombstone, privacy and implicit/older/equal
  timestamp checks remain before reconciliation. A no-op returns no edit outcome,
  so it does not trigger the worker's signed edit-forwarding path.
- No poll/Question editing, schema, grants, harness, CI, or profile changes.
  The separate profile fix on main (`1a0b9c6`) is not duplicated here.

### Sensitivity correction and reference limits

- The parent supplied the pinned service's significant-field observation:
  **standalone sensitivity is excluded**. The original positive sensitivity
  control inadvertently froze the unconditional Rustodon inbound behavior; it
  is replaced with `sensitivity_only_update_reconciles_without_edit_effects`,
  checking both enabling and disabling sensitivity without edit effects.
- Rustodon's separate local `update_status` path explicitly includes sensitivity
  (and language) in its significant changes. That local UI/API policy is left
  untouched; this issue does not claim that local and inbound policies match.
- The prescribed pinned Mastodon 4.6.5 checkout and the external
  read-only pinned Mastodon source checkout were absent in this coding
  environment. No source was fetched/replaced and the 4.7-alpha discovery checkout
  was not used as an oracle. Parent previously confirmed the exact pinned-image
  service/spec contract; sensitivity relies on the parent's Phase 2 observation,
  not an independently inspected snippet in this worktree. Parent must finalize
  that pinned check and run the corrected sensitivity control against baseline
  production as well as the candidate fix.

### Parent handoff / remaining gates

```sh
cargo test --locked --features test-support --test workers semantic_updates:: \
  -- --ignored --nocapture --test-threads=1
```

- Expected selection remains **9 tests** (sensitivity control renamed/corrected).
- Requires the disposable restored worker fixture and its existing runtime,
  restricted-writer, owner, and admin database URLs. Run serialized: tests reset
  operational queues and share a task-specific Note URI. No upstream media files,
  HTTP transport or Pull/Push execution is required for this filter.
- Parent owns baseline execution of the corrected control, green, formatting,
  lint, broader lifecycle regressions, and independent production review. No
  build/test/fmt/lint/remote/container workload or commit was run by the coding agent.
- Deliberately no new received-version watermark: the existing timestamp fence
  remains based on the last meaningful edited_at (or created_at). If the desired
  contract includes rejecting timestamps newer than the last edit but older than
  a subsequently received no-op, that needs explicit pinned confirmation rather
  than silently adding operational/schema state here.
- Media identity/order/description edits remain significant; cache/preview/focus
  metadata still reconciles without becoming a new standalone edit rule. Parent
  should confirm any broader media-significance requirement against the pin
  before expanding this scope. No green or merge readiness is claimed.

## Completion evidence (2026-09-12)

- Original baseline: nine tests, five passed/four failed; corrected pinned
  sensitivity-only no-edit control separately failed on baseline (0 passed,
  1 failed). Historical external run artifacts are not in the repository.
- Combined isolated-worker suite: 85 passed including all nine semantic cases;
  its historical external artifact is not in the repository.
- Default/all-feature debug, release all-feature, formatting and strict Clippy
  pass. Independent review found no blockers. No live deployment or replay.
- Follow-ups deliberately not added: received-version watermark and explicit
  remote history assertions. No-op metadata reconciliation can still produce
  legitimate new mention notifications; only edit effects are suppressed.

## Approved follow-ups

- [Investigate remote Update version ordering after semantic no-ops](investigate-remote-update-version-watermark.md)
- [Assert remote semantic no-op edit-history behavior](assert-remote-noop-edit-history-stability.md)
