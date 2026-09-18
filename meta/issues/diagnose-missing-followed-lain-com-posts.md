# Diagnose missing posts from followed lain.com account

## Summary

After deploying the parity fixes, the local user reports that following `lain@lain.com` shows the correct follow status but posts are not received.

## Requirements

- Trace the live relationship, inbox acceptance, worker processing and timeline visibility using read-only diagnostics first.
- Preserve the permanent origin, account identities, follow relationship and persistent data; do not print credentials or private post bodies.
- Reproduce any identified code defect on an isolated worker before implementing a minimal reviewed repair.

## Acceptance Criteria

- Identify the failing boundary with concrete evidence, distinguishing absent historical backfill from missed new deliveries.
- Any code repair has regression coverage and applicable isolated-worker checks.
- Verify receipt and timeline eligibility of a new remote post, or explicitly record the remaining external verification blocker.

## Notes

- Deployment source `16c0b09`; readiness passed with three dead-letter jobs.
- Read-only live diagnostics on 2026-09-11 confirmed an accepted follow with no
  pending request and no stored statuses from the remote account.
- Five ingress dead letters were signed Create Notes from that actor, each
  permanently rejected after one attempt with
  `ActivityPub inbox activity is invalid`. The latest delivery followed the
  accepted relationship.
- All five Note objects carry JSON null `sensitive`, string `summary` and array
  `tag`. The latest Note was public in both envelope/object audiences. This
  establishes a processing failure rather than merely absent historical backfill
  or a hidden home-timeline row. Payload bodies/credentials were not displayed or
  committed.

## Repair and regression evidence

- Shared Note validator rejected any present nonboolean `sensitive`, including
  JSON null. Storage already treats missing/null as false. The minimal repair
  admits null while retaining rejection of every other nonboolean JSON type;
  attribution, audience and URI validation are unchanged.
- Unit matrix covers absent/null/false/true and invalid string/number/array/object
  values for both Create and Update. Corrected a missing Create activity ID in
  the initial test fixture, then reproduced red with only the production guard
  reverted before verifying green.
- Existing worker lifecycle now sends null sensitivity in Create and Update and
  checks persisted false. The Create check occurs before a valid duplicate could
  conceal a rejected first delivery. Existing update, follower stream, privacy,
  deduplication and Delete assertions remain intact.
- Isolated worker passed 29 inbox tests, all 62 worker fixture tests, `cargo fmt --all
  --check` and all-target/all-feature Clippy with warnings denied. Historical
  external run artifacts are not in the repository.
- Independent code review approved.

## Deployment and recovery — 2026-09-11 UTC

- Source `f9b6af78f6863bef16960e918a426c2b13854213` was deployed in a
  native ARM64 release with default/test-support features disabled. Tests and lint
  remained in isolation.
- The application-only deployment completed on **2026-09-11**; its artifacts are
  not in the repository. A consistent restricted backup was nonempty, its
  contents were not displayed, and restoration was not tested. PostgreSQL and
  Redis services, local identities, and the accepted follow were preserved.
- The previous application pair remains stopped and available for rollback; data
  services must not be recreated or run concurrently with the old pair.
- An independently reviewed bounded recovery replayed exactly the five diagnosed
  jobs while preserving payloads, ordering/idempotency keys, attempts, and lease
  generations. Normal worker claiming and validation resumed afterward.
- All five ingress jobs completed; three public, non-sensitive statuses
  materialized and were exposed by the public account-statuses API.
- The exact `rest_home_timeline_ids` SQL extracted from current repository source
  placed both top-level statuses on the first page. The third was an unresolved
  reply and was not yet home-eligible. The local diagnostic result is retained
  only as historical external evidence not in the repository.
- Preflight passed with the same two existing warnings; worker, local and public
  readiness passed. No dead-letter jobs remained at postcheck.

## Remaining verification

- Keep open pending a fresh post delivered after the repaired deployment and
  user-visible confirmation. Recovery proves processing of already accepted
  signed deliveries, not a new end-to-end HTTP delivery after deployment.
- The other two acknowledged Create jobs did not leave status rows; the exact
  skip reason has not been established. Do not claim all five posts were restored.
- The reply-thread job was retrying with `remote reply thread persistence failed`;
  the post itself was stored, but its parent was unresolved. This newly exposed
  downstream issue was not repaired or bypassed as part of the nullable parser fix.

## Follow-up evidence — 2026-09-11 UTC

- A fresh remote top-level status arrived after the nullable-sensitive deployment
  without the bounded replay. The exact repository home selector put it on the
  local account's first page; inbox acceptance also continued afterward. This
  addresses the fresh-delivery evidence gap above.
- Subsequent combined repair deployment `b2937cf` preserved the accepted follow
  and recovered the reply thread through its separate reviewed permission repair.
  The child now has its correct parent and public context; see
  [thread recovery](repair-remote-reply-thread-persistence.md#combined-isolated-worker-validation-and-live-recovery--2026-09-11-utc).
- Public bundled profile and status pages now render the recovered remote account
  media and diagnosed GIF. The two earlier acknowledged Creates without rows
  remain unexplained; do not claim historical backfill or recovery of all five.
  Keep open for user confirmation of the original missing-post symptom; no
  broader replay or ingestion-policy bypass was performed.

## Reopened symptom — 2026-09-18 UTC

- Read-only diagnosis confirms new deliveries are accepted but held in the ingress
  queue, not merely missing historical backfill. The accepted follow remains.
- At 11:25–11:31 UTC, 324 ingress jobs were pending: five retrying Create Notes
  and 319 successors waiting on live predecessors. All five failing Notes reply
  to parent URIs absent from the local status table; none of the child statuses
  has been persisted. The reported failure is `remote Note Create write failed`.
- The affected actor has 36 pending activities (16 Creates and 20 Likes), behind
  a Create first accepted at 05:22 UTC that has failed six times. Its latest
  stored status is timestamped 05:13 UTC. Other affected actor streams come
  from two additional domains.
- Worker ingress coverage and heartbeats are healthy. Generic service readiness
  therefore does not establish successful processing of each actor stream.
- No jobs were replayed, skipped, deleted, or changed.
- Source inspection of the exact deployed revision identifies a matching
  nullable-boolean defect in `status_timeline_snapshots`: a public reply with an
  unresolved parent evaluates `(NOT status.reply OR
  status.in_reply_to_account_id = status.account_id)` to SQL NULL, but the
  result is decoded into Rust `bool`. The decode error rolls back ingestion
  and its staged thread-resolution job; the worker retains only the generic
  retry error. PostgreSQL need not log a server error for a client decode failure.
- All five live jobs satisfy the triggering conditions: public `to` audience,
  unsuspended/unsilenced author, and nonlocal parent absent by both URI and URL.
  A read-only SQL expression check confirmed the NULL result. An isolated
  end-to-end regression is still required before repairing and deploying.

## Null-safe routing repair — 2026-09-18 UTC

- Minimal repair: apply `IS TRUE` to the public timeline reply predicate in
  `status_timeline_snapshots`. SQL NULL now routes false, while resolved self
  replies and ordinary true/false results retain their existing meaning. No
  ingestion retry policy, privileges, schema, or production configuration changed.
- Extended the existing parent-fetch worker fixture to queue a distinct same-actor
  self-reply successor before processing either Create. It requires each ingress
  job to acknowledge in order, the unresolved child to persist, and its original
  `resolve_thread` arguments to survive. It checks the unresolved reply's public
  route is false and the resolved self-reply's is true for public visibility, plus
  existing parent recovery, notifications, home visibility, and replay invariants.
- Final-test red: with only the SQL fix removed in isolation, the public recovery
  test failed at the first ingress acknowledgement: `remote Note Create write
  failed`. Final-test green: restoring the SQL fix passed
  `parent_fetch_recovery::public_reply_recovers_after_parent_fetch_503` against a
  fresh restored PostgreSQL fixture with the restricted writer role.
- Execution used a task-owned NAS Linux workspace with tracked source only, a
  disposable database/container/volume/network, serial fixtures, and explicit
  bounds (test runner: 3 CPUs/8 GiB/512 processes/1200 seconds; database:
  2 CPUs/1 GiB/128 processes). No macOS compatibility changes or production access.
  The isolated harness selected the existing test and stopped after database tests;
  those harness-only changes are not source changes in this repair.
- Also passed: `cargo fmt --all --check`,
  `cargo clippy --locked --test workers --all-features -- -D warnings`, and
  `cargo test --locked --all-features --lib mastodon::write_repository::tests`
  (44 tests). External logs are under the task workspace
  `/srv/workspaces/rustodon-null-route-20260918/logs/` (`red-final.log`,
  `final-checks.log`); these are not repository artifacts.
- Evidence boundaries: the full worker lane did **not** pass. After the focused
  test passed, its startup tail refused the tools image's missing media-processor
  capabilities (`PF_MEDIA_PROCESSOR`). A three-recovery-test attempt also exposed
  an existing direct-privacy assertion counting the global routing event as a
  nonrecipient delivery; the same failure was reproduced with the original test
  source. Subsequent tests in that combined attempt failed its existing
  parent-author-statistics baseline assertion. These unrelated failures were not
  repaired. Strict all-target Clippy also remains blocked by existing
  `needless_pass_by_value` findings in `src/paperclip.rs:2901`.
- No full ordinary aggregate, browser, differential, peer, or release gate is
  claimed. Keep this issue open for the parent's separate deployment and fresh
  external delivery verification; nothing was deployed or replayed here.
