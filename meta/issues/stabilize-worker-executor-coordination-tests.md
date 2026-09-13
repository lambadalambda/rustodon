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
