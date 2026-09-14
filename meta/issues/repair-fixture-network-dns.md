# Repair fixture network DNS

## Summary

Combined operational-schema Rust cases passed, but the later pinned Rails reopen
could not resolve its task-owned PostgreSQL container name on the fixture bridge.
This blocked a clean operational gate and could also block differential/cutover
lanes.

## Requirements

- Diagnose only disposable task-owned network/container probes; preserve unrelated
  worker resources and engine configuration. Make no system installation/change,
  floating image substitution, or fixture assertion bypass.
- Preserve loopback publication, database separation, and least-privilege roles.
- Keep worker-infrastructure failures separate from application compatibility
  results.

## Acceptance Criteria

- Reproduce and diagnose the bridge name-resolution failure safely.
- After a bounded correction, rerun failed applicable gates on combined source.

## Evidence

- Historical external evidence not in the repository records pinned Rails
  reporting `PG::ConnectionBad` because it could not translate the task PostgreSQL
  hostname; earlier Rust subtests passed.
- The worker reported netavark with aardvark-dns 1.4.0 and an available executable.
  The evidence did not justify assuming a missing package or reconfiguring the
  engine.
- No live instance or worker-wide networking change was made.

## Bounded probe results

- Two task-owned containers used the exact cached Mastodon image without database
  data/mounts, host overrides, image pulls, or configuration changes. Cleanup
  verified both containers and their network absent.
- Direct engine control passed bridge-IP TCP, libc name lookup, name TCP, and direct
  UDP/TCP DNS.
- The tooling container's existing API control path, after 12 seconds idle, passed
  bridge-IP TCP but failed libc resolution, timed out name TCP, and returned no
  direct DNS records. This isolated a control-path or timing-dependent failure; it
  did not yet establish the root cause.
- The required differential separately failed Rails readiness with an unresolved
  PostgreSQL hostname and Redis connection failure. Startup (**5 tests**) and
  configuration preflight passed independently. Historical external artifacts
  are not in the repository.
- No system installation, service restart, engine configuration, firewall, or
  host-networking change was made.

## Resolution

The failure was isolated to the task-scoped fixture control path rather than
Rails, missing packages, or global engine configuration. A task-scoped correction
restored DNS while preserving cancellation, cleanup, and command status. No global
service, engine configuration, installation, default connection, firewall, or
host-networking change was made.

Historical external run artifacts are not in the repository. Guard and
process-lifecycle tests passed, followed by the complete operational-schema gate.
Rails then started and the first differential reported the separate
[OAuth mode gap](align-oauth-metadata-response-modes.md), not a DNS failure; this
is not a differential-suite pass. Independent review approved the cancellation
correction. Remaining forced-escalation/test-failure cleanup hardening is optional,
not an assertion of complete process-supervisor coverage.
