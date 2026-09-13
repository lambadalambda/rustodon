# Implement the test audit action plan

## Summary

Make existing test coverage dependable, fix the three audited behavior defects with upstream-derived regressions, then execute the existing peer matrix and add selected high-value ports.

## Requirements

- Keep changes topical and independently reviewed; use NAS-only test/build workloads and TDD for implementation.
- Do not remove fixture/privacy/provenance/production transport guards to obtain green results.
- Preserve live services; this plan is code/test work, not an automatic deployment or data replay.

## Acceptance Criteria

- Subissues below have reproducible green evidence or explicitly documented blockers.
- Combined source passes applicable gates; peer outcomes are recorded per scenario, not inferred from compilation.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.

## Subissues

- [Repair default and release test build profiles](repair-test-build-profiles.md)
- [Wire permanent HTTP regression fixture gates](wire-http-regression-gates.md)
- [Provision pinned worker media fixtures explicitly](provision-pinned-worker-media-fixtures.md)
- [Expand automated integration and harness gates](expand-automated-integration-gates.md)
- [Fix remote direct-message versus limited classification](fix-remote-direct-message-classification.md)
- [Suppress semantically unchanged inbound edit effects](suppress-semantic-noop-remote-edits.md)
- [Serve the correct MIME for cached media derivatives](serve-cached-media-derivative-mime.md)
- [Adapt and run the existing peer matrix on NAS](adapt-and-run-peer-matrix-on-nas.md)
- [Port selected Mastodon media and browser behavior matrices](port-mastodon-media-and-browser-matrices.md)

Existing peer acceptance records remain [the peer test issue](add-isolated-federation-peer-tests.md) and [essential parity gates](run-essential-parity-gates-on-secunda.md); link new NAS evidence rather than overwrite historical claims.

## Implementation checkpoint (2026-09-12)

Completed and archived: build profiles, permanent HTTP selectors, pinned worker
media, CI/harness lane wiring, the three initial production defects, guarded NAS
peer execution with recorded outcomes, and the first additional media-state
matrix/fix. Final NAS schema/worker/default/debug/release/fmt/Clippy/harness gates
are green; startup/preflight passed independently. No live deployment occurred.

This umbrella remains **open**. Required Rails operational/differential gates
are blocked by [socket-path NAS DNS](repair-nas-fixture-network-dns.md). Peer
interactions exposed [unreblog HTTP500](diagnose-private-boost-undo-http-500.md).
The mixed-profile-media, actual delayed browser-save and failed-parent-fetch
matrices are individually tracked under the remaining port subissue; Pleroma
exact-image/scenario evidence and extended hosted CI execution remain unclaimed.

## Approved follow-ups

- [Investigate remote Update version ordering after semantic no-ops](investigate-remote-update-version-watermark.md)
- [Assert remote semantic no-op edit-history behavior](assert-remote-noop-edit-history-stability.md)


## Remaining-work checkpoint (2026-09-13)

- Task-owned NAS socket DNS is repaired; operational Rails gate passes.
- Private unboost correction passes six focused tests and all five real Mastodon
  peer scenarios. The corrected combined worker gate passes100/100.
- Ordering/history controls and parent-fetch retry/thread/distribution matrices
  are committed, with targeted mutations caught and restored baselines green.
- Mixed-profile preservation/rejection is committed, exact animated fixtures and
  unchanged HTTP deadlines retained via a selector-scoped optimized test profile.
- OAuth query/fragment/form-post support is committed after focused security,
  schema, ordinary debug/release and strict lint gates. The stale preflight
  classification is corrected and its pinned differential passes.
- Browser runtime audit identified missing extended-description and batch-account
  endpoints. Both are committed and pass focused red/green checks; anonymous and
  authenticated startup audits now pass. Actual delayed-save acceptance remains
  open at the first-save predicate, with bounded sanitized diagnostics prepared.
- No deployment, replay, full-source checkout gate, hosted-CI execution or Pleroma
  acceptance is implied. The ten required differential invocations passed before
  the final browser-API additions; final combined checks are running separately.

Required differential follow-up: all ten configured invocations now pass on NAS
(`required-remaining-complete.log`), including both actor-media-root modes. This
run deliberately did not select the new tests-only extended-description reds.

## Final combined checkpoint

The selected media/browser port umbrella is now satisfied: `final19-browser.log`
passes actual HTTPS leading/trailing settings PUT/reload **and** full rollback;
`final19-cutover.log` independently passes ordinary cutover. HTTPS fixture trust
and exact fixture-user settings rollback are separately reviewed and covered.

All12 schema selectors pass in final17. Final18 passes ordinary default/debug/
release, strict Clippy, harnesses, full operational/Rails (including deterministic
budget regressions) and all preflight cases. The latest startup rerun passes5/5.
The ten required differential invocations have green results across final17/18,
with reauthentication's epoch-clock hazard corrected in a test-only per-instance
seam. See `docs/testing-on-nas.md` for exact log/source boundaries and failures.

This umbrella remains open only for the newly observed
[worker coordination regression](stabilize-worker-executor-coordination-tests.md)
and final evidence reconciliation. The latest worker rerun failed four named
fixture tests before its aggregate watchdog despite earlier100/100 success;
that latest failure is not hidden by the earlier pass. Full-source checkout,
Pleroma exact-image, dependency-policy and hosted/full-differential execution
remain separately blocked or unclaimed, not inferred from other gates.
