# Repair relationship and delivery lifecycles

## Summary

Correct accepted-follow preference updates, delivery cancellation ordering, and distinct Update activity identities.

## Requirements

- Update accepted remote/locked follows without creating a second request or Follow activity.
- Cancellation must not disconnect successors from earlier live jobs in the same delivery stream.
- Distinct status/profile versions need distinct durable wire IDs; retries must preserve version identity.
- Treat each lifecycle defect as a separate red/green fix and topical commit.

## Acceptance Criteria

- Follow/Accept/options/unfollow works for locked local and remote accounts without uniqueness conflicts or leftover requests.
- A multi-job, two-worker cancellation/interleaving test proves Undo cannot overtake its live positive activity.
- Two already-delivered same-second status/profile versions converge at a receiver without ID/body conflicts.

## Notes

- Findings R07, R08, R10 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.

## Implementation work

### R07 — accepted follow preferences

- The writer checks accepted follows under the existing relationship lock before deciding whether a new follow needs approval. Preference-only writes retain ID/URI and counters, emit no Follow/request notification, and report an accepted relationship.
- Added three lifecycle regressions (locked-local target, unlocked remote target, silenced source): Follow → Accept → change all options → repeat with omitted options → replay remote Accept → unfollow. Assert original ID/URI/options, zero pending requests, unchanged outbox/counters, successful ingress completion, and Undo of the original remote Follow.
- Secunda RED: `CARGO_BUILD_JOBS=2 LIFECYCLE_FILTER=lifecycles::accepted tools/.lifecycles-fixture worker-test` failed all three tests at `preferences must not create another Follow/request` before the production fix (`target/r07-red.log`). GREEN: the same command passed all three (`target/r07-green.log`). The task-local harness is a copy of `tools/mastodon-fixture` with only the worker test name filter added; PID-specific fixture isolation and cleanup are unchanged.
- Secunda `cargo fmt --all` and `cargo clippy --locked --all-targets --all-features -- -D warnings` passed. Source verified against the pinned remote `app/services/follow_service.rb`, which checks accepted follows before requests/new-follow branching.
- Independent read-only correctness/architecture review found no blockers. Applied its small suggestions (unlocked remote case; remove redundant completion query). Review did not independently execute tests or read Git diff/upstream; it reviewed the scoped working-tree source.
- Deferred: this prevents new duplicate requests; repairing already-corrupt accepted-follow/pending-request pairs needs separately scoped cleanup.

### Remaining work

- R08 and R10 follow as separate red/green commits. Regression tests live in `tests/workers/lifecycles.rs` to minimize shared-test conflicts; `tests/workers.rs` only declares the module.
