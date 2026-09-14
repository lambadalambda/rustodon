# Stabilize worker executor coordination regressions

## Summary

A later worker rerun failed executor resource-capacity, twenty-user-wave and
permit-waiter lease-renewal assertions before its aggregate 1200-second watchdog.
The same 100-worker matrix passed earlier. Investigate source-backed fixture timing
assumptions and deterministically synchronize resource/lease assertions without
changing production concurrency, lease durations, fencing or behavior.

## Acceptance Criteria

- Identify precise causes from source and focused red/green controls.
- Preserve concurrency, renewal, duplicate-effect and fencing assertion strength.
- Independently reviewed focused change and actual combined isolated-worker pass.

## Observed failures

A historical external run recorded:

- `activitypub_media_fetch_reclaims_after_lease_fence`: fixture did not receive first request.
- `executor_bounds_resources_and_duplicate_execution_keeps_one_effect`: observed maximum exceeds 4.
- `executor_processes_twenty_user_waves_without_duplicate_effects`: maximum 20 vs expected 4.
- `executor_renews_a_lease_while_waiting_for_a_resource_permit`: waiter renewal assertion false.

The aggregate 1200-second watchdog later stopped the ingress-ordering test; this
is not an assertion failure in that next test. The isolated worker also had
unrelated compose services; those were observed by name only and left untouched.
Contention was possible but not established as the cause. Investigate only the
named four fixture regressions.

## Source-backed correction

- The handler counter decremented only after successful SQL. Cancellation on
  lease loss or an SQL error leaked the test counter; a maximum-20 observation
  does not prove the production semaphore exceeded 4. A small RAII guard now
  decrements on every future-exit path, with a deterministic cancellation/error
  unit control. Production semaphore code is unchanged.
- Capacity tests replace 10/30 ms overlap sleeps with four-party barriers and use
  the normal 60-second fixture lease, preserving all 20 calls per round,
  duplicate-effect attempts and eight sustained waves. They require maximum
  exactly 4 and active 0.
- The permit waiter observes its selected owner/generation/expiry in PostgreSQL,
  then proves that same live claim extends beyond the observed expiry while only
  the first handler runs. Owned joined futures prevent detached workers on error.
- Media fencing waits for actual request receipt, then expires only the selected
  live job ID/owner/generation before testing cancellation and recovery. The
  15-second fixture lease permits five-second renewal before the unchanged
  production HTTP timeout. Readiness/watchdogs are not success conditions.

The first prepared counter RED and candidate did not compile because this repo's
futures-util features omit `poll!`; that attempt therefore establishes no runtime
acceptance. Standard-library single-poll code replaces it without new dependencies.
Parent-reported isolated-worker behavioral RED/GREEN and focused/combined checks
were pending at that checkpoint.

Behavioral isolated-worker RED/GREEN was then established: the RED run failed at
active 1 versus 0 on cancellation, and the GREEN run passed. All four historical
external-run exact fixture tests pass (media 8.26s, capacity 5.01s, eight waves
29.36s, waiter 65.54s). The full 100-test matrix completed 98/100; the two
remaining retry/readiness failures are [tracked separately](stabilize-worker-retry-and-readiness-tests.md).
No remaining failure in these four tests was observed. Independent final parent
review approved the current source, preserving data/fence/capacity assertions.

## Final acceptance

After the separately reviewed retry/readiness corrections, a historical external
run passed **100/100** plus the unchanged permanent runner's real CLI readiness and
graceful-shutdown checks (978.74s tests). Default/all-feature/release tests,
formatting, strict Clippy, static assets, harnesses, and dependency-policy checks
pass. The only final lint edit spells 120 seconds as `from_mins(2)`. No production
concurrency, lease, renewal or HTTP policy changed. Supplemental scheduling-sensitive
mutations were not run or claimed; the counter behavioral RED and actual named
fixture failures establish the tested corrections.
