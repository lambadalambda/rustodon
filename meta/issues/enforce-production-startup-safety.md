# Enforce production startup safety

## Summary

Apply cutover safety checks before web or worker processes become ready.

## Requirements

- Enforce supported schema, keys, media, domains, operational schema, active
  workflow, and configuration checks during startup.
- Trust forwarded host, scheme, and client data only from configured proxies;
  separate liveness from readiness.

## Acceptance Criteria

- Fatal startup failures bind no service or claim no work, spoofed forwarding
  headers are ignored, and diagnostics remain secret-free.

## Notes

- Web and worker processes run bounded, read-only configuration, media, pinned
  Mastodon schema, key, domain, workflow, operational schema, runtime role, and
  privilege validation before binding or claiming work.
- Forwarding metadata is ignored unless `TRUSTED_PROXY_IP` explicitly trusts the
  peer. Trusted malformed forwarding fails closed, and request authorities are
  restricted to configured canonical and media authorities.
- `/health` is dependency-free. `/ready` uses a bounded database probe and
  verifies `SELECT` remains available on every v1-critical Mastodon relation.
- Real-process integration proves fatal web startup binds no socket, fatal worker
  startup creates no lease, dispatch, or heartbeat, readiness degrades after
  database or privilege loss, and diagnostics do not expose secrets.
- Completed after independent review and sequential local, startup, worker,
  operational schema, preflight, fixture restore/reproducibility, schema, and
  six-case differential acceptance gates passed against Mastodon 4.6.5.
