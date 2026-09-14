# Deploy parity fixes to the local Podman instance

## Summary

Deploy the reviewed parity changes and source-only followups to the existing
persistent local Rustodon instance without recreating its database, public origin,
or federation identity.

## Requirements

- Inspect and preserve the existing Podman network, database/media volumes, runtime roles, container configuration and public hostname.
- Build an immutable application image from the intended source revision without test-only transport capabilities. Do not assume restarting existing containers deploys new code.
- Build and test in an isolated environment. If deployment architecture requires
  a scoped native build, record that limitation and retain all other isolation
  boundaries.
- Keep the old application image/container configuration available for rollback; take a restricted consistent backup before replacement. Do not display or commit credentials or backup contents.
- Replace only the Rustodon web/worker containers, keeping database and Redis services and persistent data intact.

## Acceptance Criteria

- Applicable build and pre-deployment checks pass, or any explicitly accepted verification limitation is documented.
- The replacement web/worker containers use the recorded new image and retain the original origin, local account identity and persistent volumes.
- Preflight, worker readiness, local health/readiness and public readiness pass after deployment.
- Deployment revision/image identity, backup status, rollback approach and remaining limits are recorded without secrets.

## Notes

- The parity work had not restarted the instance; deployment was handled as a
  separate application-only operation.
- Initial inspection confirmed the expected PostgreSQL, Redis, worker, and web
  service topology; only the web service was exposed. No credentials were read or
  printed.
- An isolated DNS failure was resolved before validation and deployment.

## Build architecture decision and baseline

- The deployment target was ARM64 while the isolated verification environment
  was x86-64. This deployment therefore used a scoped native Podman build;
  broader regression, formatting, and lint gates ran in isolation.
- The prior web/worker image was retained for rollback before building.
- Confirmed local readiness and persistent numeric actor identity. No services
  were stopped for inspection/build.
- Native candidate builds use cached immutable ARM64 Rust 1.97.1 and Debian
  Bookworm bases, `--pull=never`, a two-CPU quota, 3 GiB memory limit and two Cargo
  jobs. Release binary is built without default/test-support features.
- An independently reviewed application-only deployment process preserved data
  services, rollback capability, readiness, and identity. Recovery findings were
  corrected before deployment.
- First native ARM64 candidate (`6b31782`) built successfully and passed live
  database preflight with only the pre-existing SMTP-disabled and unverified
  local-domain-tag warnings. It was not activated at that checkpoint.
- Isolated worker ordinary all-target/all-feature tests passed on fully synchronized
  current source. Six rustfmt-only files were independently reviewed and merged
  as `416339f`; no functional source repair was needed. The first gate task stalled
  before Clippy/database suites; the parent completed those gates below.

## Completed deployment — 2026-09-11 UTC

- Deployed source: `16c0b09face9acfd26f9617d925df1596a18b453` in an
  immutable Linux ARM64 image. Both running applications used that build.
- Isolated-worker gates passed: ordinary all-target/all-feature tests; worker fixture
  **62/62**; default schema/lifecycle suites **42/42**; named `v2_account_search`;
  `browser_recovery_fences`; `browser_reauthentication_limits`; formatting and
  all-target/all-feature Clippy with warnings denied.
- Search validation exposed two incorrect anonymous/authenticated whole-account
  test comparisons. Reviewed test-only fix `16c0b09` preserves full comparisons,
  explicitly expects anonymous feature permission `denied`, and checks the
  authenticated local/remote permissions. Red then green on an isolated worker; no production
  behavior changed. Remaining changes were doc/test Clippy fixes.
- Gate results were retained in historical external artifacts not in the
  repository. Earlier runs against stale source are not evidence.
- The reviewed application-only deployment completed successfully on
  **2026-09-11**. A consistent restricted pre-cutover backup was created that day;
  its contents were not displayed and restoration was not tested.
- PostgreSQL and Redis services and volumes matched the predeployment baseline.
  Both replacement applications retained the existing media, runtime network,
  and web-only publication. No data services or volumes were recreated.
- Local identity snapshots, public WebFinger, and actor GET matched the unchanged
  account identity and public origin.
- Candidate preflight passed before cutover with the two known warnings. Worker
  readiness, local health/readiness and public readiness passed during cutover
  and in the subsequent status check.

## Rollback and remaining limits

- Original stopped web/worker containers and the old image remain available for
  application-only rollback. The backup need not be restored and data services
  must not be recreated for that rollback. Automated recovery on cutover failure
  was available but not needed.
- Backup recovery mock tests on an isolated worker passed partial-stop, TERM and restart-error
  cases. Full cutover rollback and backup restoration were not exercised live.
- Preflight retains the existing SMTP-disabled and unverified persisted
  local-domain-tag warnings. Worker readiness is true with no queued jobs and
  three dead-letter jobs; those jobs were not modified by this deployment.
- Expanded real-peer privacy/lifecycle/profile/interaction scenarios and the
  quota-deferred Pleroma build remain separate acceptance work. Deployment does
  not certify those unexecuted scenarios or close their issues.
