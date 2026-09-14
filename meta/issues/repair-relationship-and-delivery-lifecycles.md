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
- The filtered worker fixture with `CARGO_BUILD_JOBS=2` and filter
  `lifecycles::accepted` failed all three tests at `preferences must not create
  another Follow/request` before the production fix; GREEN passed all three.
  Normal fixture isolation and cleanup were unchanged.
- Isolated worker `cargo fmt --all` and `cargo clippy --locked --all-targets --all-features -- -D warnings` passed. Source verified against the pinned remote `app/services/follow_service.rb`, which checks accepted follows before requests/new-follow branching.
- Independent read-only correctness/architecture review found no blockers. Applied its small suggestions (unlocked remote case; remove redundant completion query). Review did not independently execute tests or read Git diff/upstream; it reviewed the scoped working-tree source.
- Deferred: this prevents new duplicate requests; repairing already-corrupt accepted-follow/pending-request pairs needs separately scoped cleanup.

### R08 — cancellation ordering

- Claiming now fences all earlier non-dead members of the same ordered stream (kind/key), retaining the immediate-predecessor check for legacy/keyless compatibility. Ordered enqueue allocates IDs under the stream marker lock; unlike `run_at`, IDs remain ordered after retries. Physical cancellation no longer disconnects successors from earlier live work.
- Added a real writer/two-executor/HTTP schedule: Like A held live at the receiver → queue Like B → cancel B → dispatch Undo B → probe second worker → unlike A → probe again → release A → drain Undo B then Undo A. Assert B is deleted, neither Undo overtakes A, exactly three wire bodies, original Like identity in Undo A, and no leftover delivery jobs/favourites.
- The filtered worker fixture with `CARGO_BUILD_JOBS=2` and filter
  `lifecycles::cancellation` failed at `Undo B must remain fenced by live Like A
  after Like B is cancelled`; GREEN passed the full cancellation lifecycle. The Docker Hub index-manifest rate-limit interruption was resolved by the parent-owned cache-safe fixture repair `52fa99a` (cherry-picked here as `735a218`); no credentials or unpinning used.
- Independent read-only correctness/architecture review found no blockers.
  Isolated-worker Clippy passed. `cargo test --locked --all-targets --all-features` passed 406 tests with the pinned-source link temporarily absent (ignored DB/source gates are not counted as passes).
- Deferred performance observation: the all-earlier correlated scan may merit a stream-expression partial index after backlog measurement; queue-performance work remains outside this fix.

### R10 — distinct durable Update identities

- Status and actor Update generation share an ID constructor using the persisted microsecond version. Delivery freshness checks share the same version/ID predicate. Existing queued seconds-format IDs remain valid for their original current version; delivery sends their stored body unchanged rather than rewriting IDs on retry. Legacy jobs without microsecond metadata retain their previous ID-only freshness limitation; new producers supply metadata.
- Added two complete sender/receiver lifecycles in `tests/workers/update_versions.rs`. The fixture harness clones an independent receiver database before test connections open, within its PID-isolated PostgreSQL container. Tests use different origins and actor rows, real signed delivery, the real inbox router, and ingress workers. Status Create first materializes a distinct receiver row. Both status/profile version A and B are exactly one microsecond apart in the same second, and A is applied before B exists.
- After accepting A, receiver middleware returns a simulated transient 503. The sender retries the exact stored JSON bytes and ID after receiver application; the receiver accepts the duplicate without another ingress job. B must then be accepted and applied with a different activity ID, the same object ID, changed content, and no leftover delivery/ingress jobs. No captured wire payload is rewritten.
- The filtered worker fixture with `CARGO_BUILD_JOBS=2` and filter
  `update_versions` failed both tests at `distinct delivered same-second versions
  must not conflict at the receiver` (409 instead of 202 for B), after A and its
  retry succeeded; GREEN passed both tests.
- Independent read-only correctness/architecture review found no blockers. Small setup-review findings were addressed before RED (explicit actor ID scheme and cleanup after assertion panics). The precommit review covered the shared constructor/freshness predicate, unchanged legacy retry bodies, receiver database isolation, convergence assertions, and harness cleanup.

### Final verification and scope

All commands below ran only in a task-owned workspace on an isolated worker, with `CARGO_BUILD_JOBS=2`:

- `tools/mastodon-fixture worker-test`: **55 passed**, including all six new lifecycle regressions and existing retry, ordering, distribution, and ingress tests.
- `tools/mastodon-fixture schema-read-test`: **37 passed**.
- `cargo test --locked --all-targets --all-features`: **406 passed**, with source/DB-dependent ignored gates excluded from the pass count. The pinned-source symlink was temporarily absent for this clean-source run and restored afterward.
- `cargo fmt --all --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, and `tools/tests/mastodon-fixture-images-test`: passed. Clippy also ran with the source link absent.

Limits: the R10 profile is image-free and the source status has a persisted HTTP URI. A generated `tag:` atomUri hit a separate existing writer relevance-parser rejection during test setup; that ingress defect was not changed here. These tests prove Rustodon receiver convergence, not a Mastodon/Pleroma peer matrix. No live instance data, environment, or credentials were accessed; the shared user checkout and pinned reference were not modified. Indexes remain untouched for the parent to manage.
