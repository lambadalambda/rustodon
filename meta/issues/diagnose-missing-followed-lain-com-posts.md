# Diagnose missing posts from followed lain.com account

## Summary

After deploying the parity fixes, the local user reports that following `lain@lain.com` shows the correct follow status but posts are not received.

## Requirements

- Trace the live relationship, inbox acceptance, worker processing and timeline visibility using read-only diagnostics first.
- Preserve the permanent origin, account identities, follow relationship and persistent data; do not print credentials or private post bodies.
- Reproduce any identified code defect on Secunda before implementing a minimal reviewed repair.

## Acceptance Criteria

- Identify the failing boundary with concrete evidence, distinguishing absent historical backfill from missed new deliveries.
- Any code repair has regression coverage and applicable Secunda checks.
- Verify receipt and timeline eligibility of a new remote post, or explicitly record the remaining external verification blocker.

## Notes

- Deployment source `16c0b09`; readiness passed with three dead-letter jobs.
- Read-only live diagnostics on 2026-09-11: accepted follow row `2` links local
  account `117250541985141990` to `117250871421421944` (`https://lain.com/users/lain`),
  created at 11:40:11 UTC; no pending request and no stored statuses from lain.com.
- Five ingress dead letters (`154`, `168`, `412`, `440`, `450`) are signed Create
  Notes from that actor, each permanently rejected after one attempt with
  `ActivityPub inbox activity is invalid`. Latest delivery at 11:40:35 follows
  the accepted relationship and contains a Note published at 11:40:23.
- All five Note objects carry JSON null `sensitive`, string `summary` and array
  `tag`. Latest Note is public in both envelope/object audiences. This establishes
  a processing failure rather than merely absent historical backfill or a hidden
  home-timeline row. Payload bodies/credentials were not displayed or committed.

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
- Secunda passed 29 inbox tests, all 62 worker fixture tests, `cargo fmt --all
  --check` and all-target/all-feature Clippy with warnings denied. Logs:
  `/home/lain/rustodon-parity/nullable-sensitive-{red,green,workers,clippy}.log`.
- Independent code review approved.

## Deployment and recovery — 2026-09-11 UTC

- Deployed `f9b6af78f6863bef16960e918a426c2b13854213`, native ARM64 image
  `da842de6c91857674c18051c2efdba8bc14297184ff324d91d9cbe1f5d99381a`, to both apps.
  Used the existing resource-limited native deployment build exception; tests
  and lint stayed on Secunda. No default/test-support features in the release.
- App-only cutover evidence: `.local-instance/logs/deploy-20260911T115504Z/`.
  Consistent backup: `.local-instance-backups/20260911T115509Z/`; files nonempty
  mode 600, directory mode 700. No contents displayed or restore test performed.
  PostgreSQL/Redis and local identities were preserved; follow row `2` unchanged.
- Previous image `b2e9682f9a50040921a5b7358319fad8f45ebf0ba2f97c5235cad11cb6a01d94`
  and stopped apps ending `-rollback-20260911T115504Z` remain available. Use the
  application-only rollback procedure in the deployment issue with these names;
  never start an old pair alongside the current apps or recreate data services.
- Independently reviewed `.local-instance/retry-null-sensitive.sql` transaction
  requeued exactly the five diagnosed jobs, guarded by IDs, signer/domain,
  activity type, null sensitivity, error/attempt count and unleased dead state.
  Worker was gracefully stopped with restart-on-error protection for the
  transaction, then restarted. The recovery SQL preserved payloads,
  order/idempotency keys, attempts and lease generations. Normal worker claiming
  and validation resumed afterward.
- All five ingress jobs completed; three statuses materialized with sensitivity
  false and public visibility. IDs `117252276511693176`, `117252276513133141`,
  `117252276514092511` are exposed by the public account-statuses API.
- Ran the exact `rest_home_timeline_ids` SQL extracted from current repository
  source, with the real viewer ID and no token access. Both top-level statuses
  (`117252276511693176`, `117252276513133141`) appear on its first page. The third
  is an unresolved reply and is not currently home-eligible. Local diagnostic
  query: `.local-instance/logs/missing-lain-posts/home-check.sql`.
- Preflight passed with the same two existing warnings; worker, local and public
  readiness passed. No dead-letter jobs remained at postcheck.

## Remaining verification

- Keep open pending a fresh post delivered after the repaired deployment and
  user-visible confirmation. Recovery proves processing of already accepted
  signed deliveries, not a new end-to-end HTTP delivery after deployment.
- The other two acknowledged Create jobs did not leave status rows; the exact
  skip reason has not been established. Do not claim all five posts were restored.
- Reply-thread job `472` is retrying with `remote reply thread persistence failed`;
  the post itself is stored, but its parent is unresolved. This newly exposed
  downstream issue was not repaired or bypassed as part of the nullable parser fix.
