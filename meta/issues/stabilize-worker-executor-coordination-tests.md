# Stabilize worker executor coordination regressions

## Summary

The final19 worker rerun failed executor resource-capacity, twenty-user-wave and
permit-waiter lease-renewal assertions before its aggregate1200s watchdog. The
same100-worker matrix passed earlier. Investigate source-backed fixture timing
assumptions and deterministically synchronize resource/lease assertions without
changing production concurrency, lease durations, fencing or behavior.

## Acceptance Criteria

- Identify precise causes from source and focused red/green controls.
- Preserve concurrency, renewal, duplicate-effect and fencing assertion strength.
- Independently reviewed focused change and actual combined NAS worker pass.

## Observed failures

`/srv/workspaces/rustodon-audit-green/logs/final19-worker.log`:

- `activitypub_media_fetch_reclaims_after_lease_fence`: fixture did not receive first request.
- `executor_bounds_resources_and_duplicate_execution_keeps_one_effect`: observed maximum exceeds4.
- `executor_processes_twenty_user_waves_without_duplicate_effects`: maximum20 vs expected4.
- `executor_renews_a_lease_while_waiting_for_a_resource_permit`: waiter renewal assertion false.

The aggregate1200s watchdog later stopped the ingress-ordering test; this is not
an assertion failure in that next test. NAS also has unrelated compose services;
those were observed by name only and left untouched. Contention is possible,
not an established cause. Investigate only the named four fixture regressions.

## Source-backed correction

- The handler counter decremented only after successful SQL. Cancellation on
  lease loss or an SQL error leaked the test counter; a maximum20 observation
  does not prove the production semaphore exceeded4. A small RAII guard now
  decrements on every future-exit path, with a deterministic cancellation/error
  unit control. Production semaphore code is unchanged.
- Capacity tests replace10/30ms overlap sleeps with four-party barriers and use
  the normal60s fixture lease, preserving all20 calls per round, duplicate-effect
  attempts and eight sustained waves. They require maximum exactly4 and active0.
- The permit waiter observes its selected owner/generation/expiry in PostgreSQL,
  then proves that same live claim extends beyond the observed expiry while only
  the first handler runs. Owned joined futures prevent detached workers on error.
- Media fencing waits for actual request receipt, then expires only the selected
  live job ID/owner/generation before testing cancellation and recovery. The15s
  fixture lease permits five-second renewal before the unchanged production HTTP
  timeout. Readiness/watchdogs are not success conditions.

The first prepared counter RED and candidate did not compile because this repo's
futures-util features omit `poll!`; final20 therefore establishes no runtime
acceptance. Standard-library single-poll code replaces it without new dependencies.
Parent NAS behavioral RED/GREEN and focused/combined checks are pending.

Behavioral NAS RED/GREEN is now established: `worker-counter-poll-red.log` fails
at active1vs0 on cancellation; `worker-counter-poll-green.log` passes. All four
`worker-focused21-*.log` exact fixture tests pass (media8.26s, capacity5.01s,
eight waves29.36s, waiter65.54s). The full final21 matrix completes98/100; the two
remaining retry/readiness failures are [tracked separately](stabilize-worker-retry-and-readiness-tests.md).
No remaining failure in these four tests was observed. Independent final parent
review approved the current source, preserving data/fence/capacity assertions.

## Final acceptance

After the separately reviewed retry/readiness corrections, `final23-worker.log`
passes **100/100** plus the unchanged permanent runner's real CLI readiness and
graceful-shutdown checks (978.74s tests). Final23 default/all-feature/release tests
pass, and final24 formatting/strict Clippy/static assets/harness/dependency-policy
checks pass. The only final lint edit spells120s as `from_mins(2)`. No production
concurrency, lease, renewal or HTTP policy changed. Supplemental scheduling-sensitive
mutations were not run or claimed; the counter behavioral RED and actual named
fixture failures establish the tested corrections.
