# Deploy parity fixes to the local Podman instance

## Summary

Deploy the reviewed parity changes and source-only followups to the existing persistent local Rustodon instance at `rustodon-lain.tunnel.eosrift.com`, without recreating its database or federation identity.

## Requirements

- Inspect and preserve the existing Podman network, database/media volumes, runtime roles, container configuration and public hostname.
- Build an immutable application image from the intended source revision without test-only transport capabilities. Do not assume restarting existing containers deploys new code.
- Follow the Secunda-only build/test policy unless the user explicitly authorizes a deployment-specific exception or another build host.
- Keep the old application image/container configuration available for rollback; take a restricted consistent backup before replacement. Do not display or commit credentials or backup contents.
- Replace only the Rustodon web/worker containers, keeping database and Redis services and persistent data intact.

## Acceptance Criteria

- Applicable build and pre-deployment checks pass, or any explicitly accepted verification limitation is documented.
- The replacement web/worker containers use the recorded new image and retain the original origin, local account identity and persistent volumes.
- Preflight, worker readiness, local health/readiness and public readiness pass after deployment.
- Deployment revision/image identity, backup location, rollback approach and remaining limits are recorded without secrets.

## Notes

- User requested deployment after confirming the parity work had not restarted the instance.
- Initial inspection: `rustodon-lain-postgres`, `rustodon-lain-redis`, `rustodon-lain-worker` and `rustodon-lain-web` are running. Web publishes only `127.0.0.1:3100`.
- Existing `.local-instance/manage.sh` supports start/stop/status/backup but not image replacement. Its credential-bearing environment files were not read or printed.
- Secunda still fails DNS resolution. No services have been stopped or rebuilt; build-host authorization is pending.

## Build-host decision and baseline

- User confirmed Secunda was restored and explicitly proposed building locally
  because deployment is ARM64 while Secunda is x86-64. Confirmed both: local
  application/base images are Linux ARM64; Secunda reports `x86_64`.
- This deployment uses a scoped native local Podman build; broader regression,
  formatting and lint gates run on Secunda. The general Secunda-only policy is
  not otherwise changed.
- Existing web/worker image: `c6f9bbdf30267fa9dd63db09083d8d21889fa5320ec3359d143013ca25e99ba4`,
  retained as `localhost/rustodon-lain:rollback-pre-parity` before building.
- Confirmed local readiness and persistent local account ID `117250541985141990`
  (`lain`, numeric actor scheme). No services were stopped for inspection/build.
- Native candidate builds use cached immutable ARM64 Rust 1.97.1 and Debian
  Bookworm bases, `--pull=never`, a two-CPU quota, 3 GiB memory limit and two Cargo
  jobs. Release binary is built without default/test-support features.
- The existing full-container recreation script is deliberately not used: it
  recreates PostgreSQL/Redis too. A locally retained app-only replacement helper
  preflights, backs up, retains stopped rollback containers and checks readiness
  and identity; its operational safety is being independently reviewed.
- First native ARM64 candidate (`6b31782`) built successfully and passed live
  database preflight with only the pre-existing SMTP-disabled and unverified
  local-domain-tag warnings. It has not been activated.
- Operational review found and corrected three recovery gaps before any stop:
  backup now arms restart cleanup before stopping writers; rollback reconciles
  captured container IDs across interrupted renames; candidates must be removed
  and both original identities restored before old services are restarted.
  Source re-review approved; deployment helper shell syntax passed.
- Secunda ordinary all-target/all-feature tests passed on fully synchronized
  current source. Six rustfmt-only files were independently reviewed and merged
  as `416339f`; no functional source repair was needed. The first gate task stalled
  before Clippy/database suites, which remain required before switching.
