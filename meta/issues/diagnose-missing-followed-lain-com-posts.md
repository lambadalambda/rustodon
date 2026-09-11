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

- Deployment source `16c0b09`; readiness passed with three dead-letter jobs, not yet correlated with this report.
