# Keep instance routes available when activity counts fail

## Summary

An activity aggregation failure returns 503 from the web UI HTML (`/` and all
frontend routes), `/api/v1/instance`, `/api/v2/instance` and `/nodeinfo/2.0`.
The failure is cached for 5 s, so one slow count query blocks every page load
for all users. Separately, an activity prune failure stops the maintenance job
before idempotency-key and ordering-marker cleanup runs.

## Requirements

- A failed refresh serves the last successful counts if they are at most 24 h
  old; otherwise it serves zero counts.
- Keep single-flight refresh and the 5 s failure back-off.
- A snapshot that finishes after UTC midnight is returned but not cached.
- Activity prune failure does not prevent the remaining maintenance cleanup;
  the job still retries afterwards.
- Limited-federation privacy (zero counts in v2) is unchanged.

## Acceptance Criteria

- Unit tests prove stale fallback, zero cold fallback, bounded staleness,
  coalesced failure and midnight behavior.
- The main-runtime startup regression expects 200 with zero counts during a cold
  activity failure.
- Ordinary tests, fmt and strict Clippy for the change pass on Linux.

## Notes

- This intentionally reverses the fail-closed 503 decision in
  [aggregate-and-cache-instance-activity-metrics](aggregate-and-cache-instance-activity-metrics.md).
  A user count is not worth making the whole web UI unavailable. Approved by the
  user on 2026-09-23 after review.

## Done 2026-09-23

Red/green on the NAS PG14 fixture; see DEVLOG 2026-09-23.
