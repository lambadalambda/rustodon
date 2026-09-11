# Run essential parity gates and peer tests on Secunda

## Summary

Execute all builds and tests for the essential-parity fixes on `lain@secunda.local`, leaving the local machine and live Rustodon/Pleroma instances untouched.

## Requirements

- Use isolated task workspaces and rootless Podman on Secunda for fixture/integration and peer tests.
- Reuse the existing read-only pinned Mastodon source at `/home/lain/repos/rustodon/target/mastodon-v4.6.5` (revision `1440d55b139e39ec722c2a3db7f60b66cd889048`); do not retrieve another copy when this one exists.
- Never synchronize local instance credentials, backups, or untracked remote configuration.
- Add reproducible Mastodon-to-Rustodon and Pleroma-to-Rustodon peer coverage where useful, checking ingestion, identity, visibility, notifications, and lifecycle convergence rather than HTTP acceptance alone.
- Keep review observations R17/R18 (queue performance) deferred.

## Acceptance Criteria

- Focused red/green regressions, independent reviews, and the applicable aggregate quality gates run on Secunda for the implemented fixes.
- Containerized peer evidence distinguishes passed activities/directions from remaining unproven cases.
- Commands, prerequisites, isolation, cleanup, and remaining blockers are documented for repeatable runs.

## Notes

- Secunda runs Linux x86-64, has Rust/Cargo 1.97.1, and has an existing checkout at `/home/lain/repos/rustodon` with untracked `tracked-configs/`; that checkout is not modified.
- With explicit user approval, installed Podman 6.1.1, slirp4netns, fuse-overlayfs, and package-manager-selected dependencies on Secunda. Rootless `podman info` succeeds with netavark.
- SSH user is `lain`, not the local machine's default `lainsoykaf`.
