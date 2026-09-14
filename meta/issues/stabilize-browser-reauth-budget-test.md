# Stabilize browser reauthentication budget regression

## Summary

The final required `browser_reauthentication_limits` differential returned 302
rather than 429 after 66.70 seconds. Production uses epoch-aligned five-minute IP
and one-hour user windows. A 66.70-second run can cross an epoch boundary; this is
a source-backed timing hazard, not proof of the original failure cause. Preserve
production limits and assertion strength; deterministic test-only state/clock
control must retain cross-user/IP and pre-bcrypt controls. Obtain red/green and
actual pinned differential evidence.

## Acceptance Criteria

- Source-backed focused regression, independently reviewed minimal correction.
- Isolated-worker-only execution; final affected gate passes without weakening production policy.

## Verification

A test-support-only per-instance reauthentication clock now fixes both epoch
bucket selection and SQL expiry comparison. Both fixture servers use the same
clock; other limiter families and ordinary production clocks are unchanged.
An exhausted historical bucket with infinite expiry catches missing bucket
control; finite historical expiry catches missing SQL-clock control. The main
budget matrix still performs every allowed request through HTTP and real bcrypt,
with cross-user/IP/two-instance limits, Retry-After, and no-bcrypt denial checks.
Explicit expiry touches only the selected fixture IP key.

A historical external isolated-worker run failed the deterministic historical control
(422 versus 429). A later run passed the actual differential (75.70s), and
A historical external run passed the new permanently wired ignored fixed-clock
regression, with an exact nonempty selector. A regular unit checks five-minute
and one-hour epoch rollover; default/all-feature/release and strict Clippy pass.
Independent security/correctness/DRY component review approved. The earlier
66.70s failure is consistent with boundary crossing, not a proven time trace.
