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

## Resolution

The socket service launches netavark without the root login's user-bus
environment. During container creation it reports `Failed to connect to bus:
No medium found`; no rootful aardvark process/listener appears, even before API
idle shutdown. Direct root-login CLI has `/run/user/0` and its bus. This is not
a Rails hostname mismatch or general bridge connectivity failure.

`tools/nas-fixture-session` now supervises a task-owned, mode-0700 Unix API
socket inheriting that active root-login environment. The tooling container
binds this socket at its existing explicit wrapper path. No global service,
root linger, engine config, installation, default connection or firewall change
was made. The helper checks the authorized host/root/physical workspace/bus,
forwards cancellation to an isolated command group, permits fixture cleanup
before stopping the API, and preserves command status and task evidence.

Evidence in `/srv/workspaces/rustodon-audit-green/`:

- `logs/dns-lifetime-probe/host-{initial,idle}.log`: no rootful DNS helper.
- `logs/dns-probe-task-socket/`: bridge IP/name TCP, libc resolution and direct
  UDP/TCP DNS pass after twelve idle seconds; disposable resources removed.
- `logs/nas-session-profile-{red,green}.log`, `logs/nas-lifecycle-green.log`:
  guard checks and real fake-API process tests pass, including exit 0/7, TERM143,
  child cleanup while the API is alive, and no tracked PID/socket leaks.
- `logs/operational-task-socket.log`: entire operational-schema gate passes.
- `logs/differential-task-socket.log`: Rails now starts and the first differential
  runs; it reports the separate [OAuth mode gap](align-oauth-metadata-response-modes.md),
  not DNS failure. This is not a differential-suite pass.
- Independent initial review identified cancellation supervision; correction
  and incremental review passed. Remaining forced-escalation/test-failure cleanup
  hardening is optional, not an assertion of complete process-supervisor coverage.
