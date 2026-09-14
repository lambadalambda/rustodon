# Adapt and run the existing peer matrix on an isolated worker

## Summary

Existing peer scenarios had nonportable worker/workspace guards, and expanded
privacy/lifecycle scenarios lacked execution evidence.

## Requirements

- Narrowly adapt the runner for explicit isolated-worker workspaces and verified existing pinned source/image prerequisites; keep isolation/TLS/audience audit guards.
- Run existing public, privacy, notes, profile and interactions scenarios, preserving received-state and no-fetch/audience evidence.
- Keep Pleroma build/peer evidence separate; no floating pins, credentials workaround, live peers or broad cleanup.

## Acceptance Criteria

- Runner guard/source validation tests pass before any actual scenario execution.
- Each Mastodon scenario has a clear pass/fail/blocked result on combined source; failures are diagnosed and tracked without weakening assertions.
- Pleroma prerequisite and scenario status is recorded separately, not claimed from Mastodon-only success.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- Implementation plan recorded on 2026-09-12; no live deployment is implied.

## Guarded adapter implementation (2026-09-12)

- Added an isolated execution profile with cache-only verification of three
  images, network/mount probes, and resource-absence checks before ownership or
  cleanup. The earlier worker source HEAD check was preserved.
- Offline prerequisites first failed, then passed source/profile, cache integrity,
  absence-error, no-pull, and unique-run checks. Historical external run artifacts
  are not in the repository.
- Initial attempts stopped on missing pinned Redis, a task target symlink, then a
  missing Rust toolchain home after environment sanitization. Normal verified
  fixture tooling supplied the Redis pin; the dedicated target preserved the
  application preflight. Final Cargo retained only toolchain/cache locations.
- Repeated tooling uses unique run IDs, preserving Rust endpoint/database/comment
  guards while avoiding retained-evidence collisions.
- Independent initial and incremental reviews found no blockers. All five actual
  scenario reruns are pending; source and mock success are not peer convergence.
- Pleroma remains blocked on its separately tracked exact-image build history;
  no new build or scenario acceptance is claimed. No live resources changed.

## Execution outcomes (2026-09-12)

Final production source includes all four audited/ported fixes. Peer builds use
a fresh dedicated target: sharing Cargo artifacts across source roots retained
a wrong compile-time workspace marker and was rejected by the guard. No guard
was weakened. Profile test data was shortened to a seeded account-ID marker to
respect Mastodon's 40-character display-name limit; independent review and the
real rerun passed.

| Scenario | Result |
| --- | --- |
| public | PASS, both directions, signed push and no canonical status GET |
| privacy | PASS, recipient/outsider/anonymous and audience checks |
| notes | PASS, Create/Update/Delete and retained private access rules |
| profile | PASS, full actor Update both directions without actor refetch |
| interactions | FAIL after private Announce: original-ID unreblog HTTP 500 |

Detailed evidence was retained as historical external run artifacts not in the
repository. Each invocation cleaned its task containers, network, and volume;
keys/environment files were removed by the runner.

Adapter/execution acceptance is complete with the failure explicitly retained in
[a separate unresolved issue](diagnose-private-boost-undo-http-500.md). Broader
[peer compatibility acceptance](add-isolated-federation-peer-tests.md) remains
open; four Mastodon passes do not establish full federation or Pleroma parity.
Pleroma has no new exact-image build/scenario evidence.


## Remaining-audit peer rerun (2026-09-13)

All five actual scenarios passed with the missing-account-stats boost correction
(now committed in `db8b540`) and reviewed OAuth modes. The peer workspace retained
its own physical Cargo target and verified cached images; no source-contract or
Pleroma result is inferred.

| Scenario | Result |
| --- | --- |
| interactions | PASS, private Announce and Undo included |
| public | PASS |
| privacy | PASS |
| notes | PASS |
| profile | PASS |

Historical external run artifacts are not in the repository. A preliminary
environment setup attempt stopped before compilation; after a task-scoped
correction, all scenarios ran with isolation guards intact and cleaned their
recorded resources.
