# Diagnose NAS fixture bridge DNS failures

## Summary

Combined operational-schema Rust cases pass, but the later pinned Rails reopen
cannot resolve its task-owned PostgreSQL container name on the fixture bridge.
This blocks a clean operational gate and may block differential/cutover lanes.

## Requirements

- Diagnose only disposable task-owned network/container probes; preserve unrelated
  NAS resources and engine configuration. No system installation/change without
  user authorization, no floating image substitution or fixture assertion bypass.
- Preserve loopback publication, database separation and least-privilege roles.
- Keep NAS infrastructure failures separate from application compatibility results.

## Acceptance Criteria

- Reproduce and diagnose the bridge name-resolution failure safely.
- With an authorized correction, rerun failed applicable gates on combined source.

## Evidence

- `/srv/workspaces/rustodon-audit-green/logs/operational-schema-test.log:288`:
  pinned Rails `PG::ConnectionBad`, cannot translate task PostgreSQL hostname,
  temporary failure in name resolution. Earlier Rust subtests passed.
- NAS reports netavark with aardvark-dns 1.4.0; executable exists. Do not assume
  a missing package or install/reconfigure the engine merely from this symptom.
- No live instance or NAS-wide networking change made.
