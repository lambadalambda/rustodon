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

## Bounded probe results

- Two task-owned containers from the exact cached Mastodon image, no database
  data/mounts, host overrides, image pulls or configuration changes. Cleanup
  verified both containers and their network absent.
- Direct NAS CLI control: bridge-IP TCP, libc name lookup, name TCP, and direct
  UDP/TCP DNS all passed. `logs/dns-probe-direct-cli/`.
- Tooling container using the actual Unix-socket wrapper, after 12 seconds idle:
  bridge-IP TCP passed; libc resolution failed, name TCP timed out, direct DNS
  returned no records. `logs/dns-probe-socket-idle/`. Both directories are under
  `/srv/workspaces/rustodon-audit-green/`.
- Socket service inspection reports `PrivateNetwork=no`, `KillMode=process` and
  an idle/inactive default system service. This isolates a socket/control-path
  or timing-dependent DNS failure; it does **not** yet establish the root cause.
- Required differential also failed Rails readiness with unresolved PostgreSQL
  hostname and Redis connection failure; `logs/differential-required.log`.
  Startup (5 tests) and configuration preflight passed independently.
- No system installation, service restart, engine config, firewall or host
  networking change made. Obtain permission before any NAS-wide correction.
