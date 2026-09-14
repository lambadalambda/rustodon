# Stabilize worker retry and readiness regressions

## Summary

After the four executor/media coordination corrections passed, the combined
100-worker run reported 98 passed / 2 failed in 812.64 seconds. The
retry/cancellation test incorrectly claimed an allegedly too-early retry; runtime
readiness returned `Elapsed`. Investigate only these two named fixture timing
assumptions without changing production retry, readiness, shutdown, or worker
deadlines.

## Acceptance Criteria

- Source-backed focused red/green controls; exact retry and lifecycle effects remain asserted.
- Independently reviewed minimal test-only synchronization, followed by combined isolated workers.

## Evidence

A historical external run recorded that `retries_cancellation_outbox_dead_letters_and_readiness_are_operational`
asserts `claim("too-early").is_none()` at workers.rs:16397 but obtains a job;
`runtime_publishes_readiness_and_removes_it_on_graceful_shutdown` reports Elapsed.
No original run duration alone proves the precise timeout cause.

## Source-backed correction

- The retry schedule was only 50 ms in the future, so PostgreSQL could correctly
  consider it due before the "too early" claim. The test now verifies the exact
  database-generated future timestamp, then makes only that scheduled unleased
  attempt/generation due explicitly. Abandoned cancellation similarly expires
  only its selected live claim rather than sleeping past a 25 ms lease.
- Runtime observation used three-second readiness freshness with an intentionally
  hourly heartbeat and four 500 ms effect timeouts. Freshness now follows that
  fixture's heartbeat interval; one 120-second watchdog bounds observable state,
  not a performance expectation. All `WorkerConfig` values remain unchanged.
- Both selected outbox records must be dispatched and their matching durable jobs
  acknowledged (dead jobs do not count as completion), alongside remote-lease,
  rate-limit and mute cleanup effects. Exact lane advertisement, missing-lane
  rejection and heartbeat deletion after graceful shutdown remain asserted.
- Observation failures signal/join runtime and attempt mute restoration before
  returning; phase/last-selected-state diagnostics identify the failing condition.

Independent correctness/security/compactness review approved. Source outside the
two named functions is unchanged from the four-fix candidate. Focused and combined
isolated-worker GREEN remained pending at that checkpoint; the actual two failures
were the pre-edit RED.

The retry test itself passes in a historical external run (7.11s), but the
original task-only focused runner then executed the full matrix's trailing CLI
sanity against this single test's deliberately retained heartbeat/dead-job state
and exited unsuccessfully. This is not a complete gate pass. Focused invocation
now finishes its disposable database immediately after the exact test; the
permanent combined worker gate and its CLI/readiness/shutdown checks are untouched.

## Final acceptance

Historical external runs recorded retry (5.22s) and runtime (3.00s) passing. The
unchanged permanent gate then passed **100/100**, plus actual CLI readiness/
shutdown checks. Ordinary default/all-feature/release tests, formatting, strict
Clippy, static/vendored assets, harnesses, and dependency policy pass. Parent
final independent incremental review approved both functions with no blockers.
Production retry/readiness/shutdown configuration is unchanged.
