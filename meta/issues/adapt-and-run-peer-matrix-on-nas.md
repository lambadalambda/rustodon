# Adapt and run the existing peer matrix on NAS

## Summary

Existing peer scenarios are locked to a Secunda host/path and expanded privacy/lifecycle scenarios lack execution evidence.

## Requirements

- Narrowly adapt the runner for explicit authorized NAS workspaces and verified existing pinned source/image prerequisites; keep isolation/TLS/audience audit guards.
- Run existing public, privacy, notes, profile and interactions scenarios, preserving received-state and no-fetch/audience evidence.
- Keep Pleroma build/peer evidence separate; no floating pins, credentials workaround, live peers or broad cleanup.

## Acceptance Criteria

- Runner guard/source validation tests pass before any actual scenario execution.
- Each Mastodon scenario has a clear pass/fail/blocked result on combined source; failures are diagnosed and tracked without weakening assertions.
- Pleroma prerequisite and scenario status is recorded separately, not claimed from Mastodon-only success.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.

## Guarded adapter implementation (2026-09-12)

- Added exact NAS host/physical-workspace profile with rootful-engine identity,
  cache-only three-image verification, host-network/mount probes and resource
  absence checks before ownership/cleanup. Secunda source HEAD check preserved.
- Offline prerequisites first failed, then passed source/profile, cache integrity,
  absence-error, no-pull and numeric unique-run checks. Logs under
  `/srv/workspaces/rustodon-audit-{main,green}/logs/peer-*.log`.
- Initial NAS attempts honestly stopped on missing pinned Redis, a task target
  symlink, then sanitized Cargo's missing rustup home. Normal verified fixture
  tooling supplied the Redis pin; physical target preserved the application
  preflight. Final Cargo explicitly retains only toolchain/cache locations.
- Repeated tooling PID7 now uses a numeric timestamp/PID run ID, preserving Rust
  endpoint/database/comment guards while avoiding retained-evidence collisions.
- Independent initial and incremental reviews found no blockers. All five actual
  scenario reruns are pending; source and mock success are not peer convergence.
- Pleroma remains blocked on its separately tracked exact-image build history;
  no new build or scenario acceptance is claimed. No live resources changed.
