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
