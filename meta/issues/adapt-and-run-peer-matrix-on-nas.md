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

## Execution outcomes (2026-09-12)

Final production source includes all four audited/ported fixes. Peer builds use
a fresh dedicated target: sharing Cargo artifacts across source roots retained
a wrong compile-time workspace marker and was rejected by the guard. No guard
was weakened. Profile test data was shortened to a seeded account-ID marker to
respect Mastodon's 40-character display-name limit; independent review and the
real rerun passed.

| Scenario | Result | Evidence run |
| --- | --- | --- |
| public | PASS, both directions, signed push and no canonical status GET | `peer-44271789209055836348330` |
| privacy | PASS, recipient/outsider/anonymous and audience checks | `peer-54681789209110617445892` |
| notes | PASS, Create/Update/Delete and retained private access rules | `peer-65321789209171966206942` |
| profile | PASS, full actor Update both directions without actor refetch | `peer-81789210624970092399` |
| interactions | FAIL after private Announce: original-ID unreblog HTTP500 | `peer-86911789209305125467036` |

Logs: `/srv/workspaces/rustodon-peer-tests/logs/<scenario>.log`; detailed runs
under `source/target/<run>/`. Each invocation cleaned its task containers,
network and volume; keys/environment files were removed by the runner.

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

| Scenario | Result | Evidence run |
| --- | --- | --- |
| interactions | PASS, private Announce and Undo included | `peer-71789273216189507306` |
| public | PASS | `peer-19751789273511768764386` |
| privacy | PASS | `peer-31221789273724030004092` |
| notes | PASS | `peer-42931789273971807200191` |
| profile | PASS | `peer-54271789274212335948275` |

Logs: `/srv/workspaces/rustodon-peer-tests/logs/<scenario>-remaining.log`.
A preliminary attempt stopped before compilation because source transfer retained
mode600 files with the Mac owner. Ownership was corrected only on task source
files; capability-drop and peer guards stayed intact. Each real scenario cleaned
its own resources, and the task-owned API session stopped afterward.
