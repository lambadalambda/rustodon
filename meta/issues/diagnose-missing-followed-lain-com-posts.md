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
- Independent code review approved. Deployment and bounded replay verification
  are pending; the five original dead-letter payloads will not be edited.
