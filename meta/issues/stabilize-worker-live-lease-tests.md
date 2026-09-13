# Separate worker live-lease assertions from forced expiry

## Summary

The combined100-worker NAS gate exposed two tests relying on tiny live leases:
crash-before-ack uses100ms and exhausted retry uses25ms. They failed at an
already-finished worker join and Lost versus Dead respectively. Source inspection
supports expiry races; exact original runtime attribution is not independently
proven. Neither assertion exercises the changed boost repository.

## Requirements

- Keep ordinary live leases while arranging crash/retry exhaustion.
- Explicitly expire only the abandoned test lease after verified worker cancellation;
  preserve server barriers, queue counts, replay and retry fencing assertions.
- Do not change production leases, HTTP deadlines or accept Lost as Dead.
- NAS-only focused and combined verification; independent review, topical commit.

## Acceptance Criteria

- Both exact regressions and combined worker gate pass without tiny-lease assumptions.

## Evidence

- RED: `/srv/workspaces/rustodon-audit-green/logs/workers-remaining.log`.
- Read-only diagnosis: worker cancellation can lose a100ms fence during database
  round trips; final25ms claim may expire before the exhausted retry transaction.

## Completion

The corrected serial combined worker gate passes **100/100**, including both
previously failing tests; strict all-target/all-feature Clippy passes. Evidence:
`workers-corrected.log` and `clippy-corrected.log` under the NAS audit-green logs.
Independent correctness/architecture review approved the cancellation ordering,
exactly-one lease expiry, existing runtime permissions, and unchanged fencing.
No production change or HTTP timeout widening.
