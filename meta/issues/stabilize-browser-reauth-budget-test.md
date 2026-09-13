# Stabilize browser reauthentication budget regression

## Summary

Final required differential browser_reauthentication_limits returned302 rather than429 after66.70s. Production uses epoch-aligned five-minute IP and one-hour user windows. A66.70s run can cross an epoch boundary; this is a source-backed timing hazard, not proof of the original failure cause. Preserve production limits and assertion strength; deterministic test-only state/clock control must retain cross-user/IP and pre-bcrypt controls. Obtain red/green and actual pinned differential evidence.

## Acceptance Criteria

- Source-backed focused regression, independently reviewed minimal correction.
- NAS-only execution; final affected gate passes without weakening production policy.

## Verification

A test-support-only per-instance reauthentication clock now fixes both epoch
bucket selection and SQL expiry comparison. Both fixture servers use the same
clock; other limiter families and ordinary production clocks are unchanged.
An exhausted historical bucket with infinite expiry catches missing bucket
control; finite historical expiry catches missing SQL-clock control. The main
budget matrix still performs every allowed request through HTTP and real bcrypt,
with cross-user/IP/two-instance limits, Retry-After, and no-bcrypt denial checks.
Explicit expiry touches only the selected fixture IP key.

NAS `reauth-clock-red.log` fails the deterministic historical control422vs429.
`final18-reauth.log` passes the actual differential (75.70s), and
`final18-operational.log` passes the new permanently wired ignored fixed-clock
regression, with an exact nonempty selector. A regular unit checks five-minute
and one-hour epoch rollover; default/all-feature/release and strict Clippy pass.
Independent security/correctness/DRY component review approved. The earlier
66.70s failure is consistent with boundary crossing, not a proven time trace.
