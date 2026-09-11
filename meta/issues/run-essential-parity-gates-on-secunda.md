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

## Cached fixture images

- Combined gates hit Docker Hub's unauthenticated pull limit after the ordinary
  all-target/all-feature tests passed. The fixture re-inspects registry indexes
  and pulls on every run even when the exact immutable child is already present.
- Repair the harness to reuse a locally inspected child only when its digest,
  operating system and architecture match the checked-in pin. Retain registry
  index verification and pinned pulling for cache misses; reject mismatches.
  Add offline mock regressions and prove a real restored suite using the cache.
- Cache repair RED: the offline cached-match shell test failed before the change.
  GREEN: all seven mocked cache/mismatch/cold-pull cases, POSIX shell syntax,
  and a real restored worker run using cached pins passed. Independent source
  review found no blockers. No credentials, alternate registries or floating
  images were used.

## Combined security and ingestion checkpoint

After integrating R01/R02/R03/R04/R06/R13 on Secunda:

- Restored worker suite: **53 passed**.
- Schema-read suite: **37 passed** plus saved-status HTTP lifecycle **1 passed**.
- Ordinary all-target/all-feature tests, formatting and warnings-denied Clippy
  passed. Ignored database suites were not counted as ordinary test passes.
- Both recovery-fence and shared-reauthentication-limit differential cases passed
  again on the integrated tree.
- Logs: `/home/lain/rustodon-parity/combined-security-{tests,workers,schema,clippy}.log`
  and `combined-browser_{recovery_fences,reauthentication_limits}.log`.
- This checkpoint is not a final parity gate: audience/lifecycle/client repairs
  and real peer acceptance remain in progress.
