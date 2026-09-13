# Stabilize operational domain budget regression

## Summary

The final combined operational gate failed remote_domain_budget_coordinates_independent_pools_and_reclaims_expired_leases with Elapsed after2.43s. Investigate source-backed test timing/lease assumptions; keep production budgets and coordination/fencing assertions unchanged. Use deterministic fixture state/control where possible and obtain focused plus operational gate evidence.

## Acceptance Criteria

- Source-backed focused regression, independently reviewed minimal correction.
- NAS-only execution; final affected gate passes without weakening production policy.

## Verification

The cancellation assertion polled for at most1s while asynchronous Drop cleanup
could wait for a pooled PostgreSQL connection. Production lease TTL was75s and
is unchanged. A cfg(test) per-lease completion channel now reports the actual
DELETE result: hold the only connection, observe the lease via an independent
pool, release it, await completion, then verify deletion. A30s deadlock watchdog
is not the success condition. Existing coordination/expiry/fencing checks remain.

NAS `domain-budget-red.log` deterministically fails the old assumption with the
connection held (Elapsed1.24s). `final18-operational.log` passes the new control
(4.04s) and the complete operational/Rails gate. Default/all-feature/release tests,
strict Clippy and harnesses also pass. Independent component review approved;
production SQL/timeouts and ordinary Drop behavior are unchanged.
