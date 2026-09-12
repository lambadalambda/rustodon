# Expand automated integration and harness gates

## Summary

Current CI omits focused HTTP regressions, startup and important security differential cases; expensive browser/cutover/peer gates and shell/Python harness tests need explicit lanes.

## Requirements

- Build on repaired profiles, permanent selectors and pinned fixture provisioning.
- Keep bounded fast/required and broader scheduled/manual lanes explicit; do not label ordinary checks as all tests.
- Update host instructions for the authorized NAS while preserving resource/source/secret isolation.

## Acceptance Criteria

- Checked-in CI/tasks select focused HTTP, startup and relevant security regression suites plus fast harness checks.
- Broader differential/browser/cutover/peer commands are discoverable and supported, with execution versus configured-only status recorded honestly.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.

## Implementation status

- Phase 1 completed in isolated `task/audit-ci`: offline automation regression
  written before CI, Mise, differential dispatch, or aggregate configuration.
- Scope excludes `tools/mastodon-fixture`, existing selector/media tests, and peer
  runners; parent owns those prerequisites and their integration.
- Parent alone runs all workloads sequentially on the authorized NAS. No local
  tests/builds/formatting, SSH/NAS calls, or commits by this worktree agent.
- Independent phase-1 source review completed; tightened task-command assertions,
  preserved runner executable permissions, and made Python stubs unittest cases.
- **Parent NAS red:** `python3 tools/tests/automation-gates-test.py -v`, 10 tests,
  **FAILED (failures=11, errors=6)**. Log:
  `/srv/workspaces/rustodon-audit-main/logs/automation-red.log`. Parent authorized
  phase 2 after this evidence; failures exposed missing config/runner wiring.
- **Phase 2 configured, not run:** ordinary/default/release profile tasks, offline
  harness aggregation, explicit worker asset verification, startup/preflight CI,
  nine required differential cases plus the relative actor-media-root variant,
  and weekly/manual full differential/cutover/browser lanes on disposable hosted
  runners. Peer tasks are manual and preserve the parent runner's guards.
- Browser CI pins the locally inspected CLI package version 0.31.1 and Node
  24.15.0; fresh hosted jobs install Chromium/dependencies explicitly. This is
  provisioning configuration, not browser execution evidence.
- Harness aggregation discovers only offline `tools/tests/*-test{,.py}` plus the
  existing Pleroma build/TLS proxy units. Parent's worker-media/peer regression
  scripts are included when integrated; none are edited in this worktree.
- Updated [Secunda instructions](../../docs/testing-on-secunda.md) and added
  [NAS lane/prerequisite/status instructions](../../docs/testing-on-nas.md).
- Independent phase-2 source review found the full differential lane also reads
  a pinned source JPEG. Extended hosted jobs now obtain/verify source for every
  matrix entry, with a regression assertion for ordering/unconditional setup;
  NAS docs require reuse or a blocked result, never a fetch fallback. No workload
  was run during review or correction.
- **Green pending; issue remains open.** Parent should synchronize the scoped
  changes including new untracked files, then run sequentially on NAS:
  `python3 tools/tests/automation-gates-test.py -v` and `tools/check-harnesses`.
  Actual fixture/browser/peer runs need their separate prerequisites and evidence;
  neither the stub regression nor configured CI certifies those lanes.

## Completion evidence (2026-09-12)

- Parent integration: 10 automation contract tests passed; complete offline
  harness aggregate passed. Logs:
  `/srv/workspaces/rustodon-audit-green/logs/{automation-green,harnesses}.log`.
- Existing default/all-feature/release and worker gates are green on combined
  source. All ten permanent schema selectors passed as an aggregate.
- Independent correctness/architecture review found no blockers after integrating
  parent fixture prerequisites. Weekly/manual full differential, browser and
  cutover lanes are configured, **not executed acceptance**; no hosted CI run is
  claimed. Manual peer commands retain their separately reviewed guards.
- Operational-schema reached a Rails reopen DNS failure on the NAS bridge;
  [separate infrastructure blocker](repair-nas-fixture-network-dns.md).
  Remaining runtime results are recorded separately, not silently skipped.
