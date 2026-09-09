# Implement durable workers and scheduling

## Summary

Run durable ingress, core, push, pull, mail, and maintenance work from PostgreSQL.

## Requirements

- Implement transactional enqueue, leases, crash recovery, delayed jobs,
  logical-key cancellation, bounded retries with jitter, dead letters, and
  worker/scheduler heartbeats.
- Bound remote HTTP and media concurrency separately.

## Acceptance Criteria

- Crash and duplicate-execution tests prove at-least-once handlers recover
  without losing work; readiness and dead-letter inspection are operational.

## Notes

- PostgreSQL owns all queue, outbox, lease, retry, dead-letter, cancellation,
  scheduling, and heartbeat state; Redis is not a worker dependency.
- Worker startup requires a separately granted, validated `NOINHERIT` runtime
  login and refuses the operational schema owner or Mastodon write privileges.
- Handler registration declares executable lane capability. The infrastructure
  worker currently defaults to maintenance only; a configured Mastodon writer
  also registers the Core-lane local status-notification handler. Ingress, push,
  pull, and mail handlers remain later feature milestones without changing queue
  semantics.
- Eight PostgreSQL integration cases and the process lifecycle gate cover
  transactional rollback, concurrent claims, final-attempt crash recovery,
  stale fencing, permit-wait lease renewal, retries, dead letters, keyed outbox
  reuse, cancellation races, least privilege, readiness, and bounded shutdown.
- Independent review found no remaining high- or medium-severity milestone
  blockers. All local, fixture, operational, worker, preflight, schema, and six
  differential acceptance gates passed sequentially.
