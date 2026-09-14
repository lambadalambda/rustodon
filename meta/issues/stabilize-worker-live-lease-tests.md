# Separate worker live-lease assertions from forced expiry

## Summary

The combined 100-worker isolated-worker gate exposed two tests relying on tiny
live leases: crash-before-ack uses 100 ms and exhausted retry uses 25 ms. They
failed at an already-finished worker join and `Lost` versus `Dead`, respectively.
Source inspection supports expiry races; exact original runtime attribution is
not independently proven. Neither assertion exercises the changed boost repository.

## Requirements

- Keep ordinary live leases while arranging crash/retry exhaustion.
- Explicitly expire only the abandoned test lease after verified worker cancellation;
  preserve server barriers, queue counts, replay and retry fencing assertions.
- Do not change production leases, HTTP deadlines or accept Lost as Dead.
- Isolated-worker-only focused and combined verification; independent review, topical commit.

## Acceptance Criteria

- Both exact regressions and combined worker gate pass without tiny-lease assumptions.

## Evidence

- RED was recorded in a historical external run; its artifact is not in the repository.
- Read-only diagnosis: worker cancellation can lose a 100 ms fence during database
  round trips; the final 25 ms claim may expire before the exhausted retry transaction.

## Completion

The corrected serial combined worker gate passes **100/100**, including both
previously failing tests; strict all-target/all-feature Clippy passes. Historical
external run artifacts are not in the repository.
Independent correctness/architecture review approved the cancellation ordering,
exactly-one lease expiry, existing runtime permissions, and unchanged fencing.
No production change or HTTP timeout widening.
