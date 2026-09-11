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
- Secunda initially failed DNS resolution; that blocker was resolved before validation and deployment.

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
  and identity; its operational safety review was approved before use.
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
  before Clippy/database suites; the parent completed those gates below.

## Completed deployment — 2026-09-11 UTC

- Deployed source: `16c0b09face9acfd26f9617d925df1596a18b453`.
- Immutable Linux ARM64 image: `b2e9682f9a50040921a5b7358319fad8f45ebf0ba2f97c5235cad11cb6a01d94`.
  Revision tag: `localhost/rustodon-lain:16c0b09face9acfd26f9617d925df1596a18b453`;
  `latest` and both running application containers resolve to that image.
- Secunda gates passed: ordinary all-target/all-feature tests; worker fixture
  **62/62**; default schema/lifecycle suites **42/42**; named `v2_account_search`;
  `browser_recovery_fences`; `browser_reauthentication_limits`; formatting and
  all-target/all-feature Clippy with warnings denied.
- Search validation exposed two incorrect anonymous/authenticated whole-account
  test comparisons. Reviewed test-only fix `16c0b09` preserves full comparisons,
  explicitly expects anonymous feature permission `denied`, and checks the
  authenticated local/remote permissions. Red then green on Secunda; no production
  behavior changed. Remaining changes were doc/test Clippy fixes.
- Remote evidence: `/home/lain/rustodon-parity/deploy-gates-6b31782-synced-tests.log`
  and `deployment-{workers,schema,v2_account_search,browser_recovery_fences,browser_reauthentication_limits,clippy-final}.log`.
  Earlier gate logs without `-synced-` were from stale source and are not evidence.
- Reviewed app-only helper completed successfully at `20260911T111634Z`.
  Evidence directory: `.local-instance/logs/deploy-20260911T111634Z/`.
- Consistent pre-cutover backup: `.local-instance-backups/20260911T111639Z/`.
  PostgreSQL dump, role dump, media archive and instance credential file are
  nonempty, mode 600, inside mode-700 directories. Contents were not displayed;
  restore testing was not performed.
- PostgreSQL and Redis container IDs and volume mounts match the predeployment
  baseline exactly. Both replacement applications retain `rustodon-lain-media`
  at `/media`, the existing runtime env-file/network, and the web-only loopback
  publication `127.0.0.1:3100:3000`. No data services or volumes were recreated.
- Local identity snapshots match. Public WebFinger and actor GET confirm `lain`
  remains `117250541985141990` at
  `https://rustodon-lain.tunnel.eosrift.com/ap/users/117250541985141990`.
- Candidate preflight passed before cutover with the two known warnings. Worker
  readiness, local health/readiness and public readiness passed during cutover
  and in the subsequent status check. The existing `rustodon-eosrift` tunnel
  remains in use.

## Rollback and remaining limits

- Original stopped containers are retained as
  `rustodon-lain-web-rollback-20260911T111634Z` and
  `rustodon-lain-worker-rollback-20260911T111634Z`, with the old image above retained
  under `rollback-pre-parity` and `rollback-20260911T111634Z` tags.
- Rollback is application-only: with one operator and no concurrent management
  command, stop/remove both new application containers, restore both original
  container names, retag the old image as `localhost/rustodon-lain:latest`, then
  run `.local-instance/manage.sh start` and `status`. Validate original IDs using
  `original-ids.txt` before starting. Do not restore the backup or recreate data
  services merely to roll back the application. The deployment helper performs
  this recovery automatically on cutover failure; it was not needed here.
- Backup recovery mock tests on Secunda passed partial-stop, TERM and restart-error
  cases. Full cutover rollback and backup restoration were not exercised live.
- Preflight retains the existing SMTP-disabled and unverified persisted
  local-domain-tag warnings. Worker readiness is true with no queued jobs and
  three dead-letter jobs; those jobs were not modified by this deployment.
- Expanded real-peer privacy/lifecycle/profile/interaction scenarios and the
  quota-deferred Pleroma build remain separate acceptance work. Deployment does
  not certify those unexecuted scenarios or close their issues.
