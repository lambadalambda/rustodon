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
