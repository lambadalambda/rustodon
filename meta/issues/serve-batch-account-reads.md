# Serve batch account reads used by the web client

## Summary

Authenticated browser startup now fails exactly two GET requests to
`/api/v1/accounts`. This is a real missing batch-read route, not the account
creation POST or single-account GET. Keep unexpected404 rejection intact.

## Requirements

- Verify pinned4.6.5 account index parameter/filter/scope/limit/serializer contract.
- Reuse existing account projection and authentication; no registration changes.
- Add focused normal/error/privacy/header contracts before implementation.
- Keep malformed/missing IDs, duplicates and limit behavior explicit.
- NAS-only red/green, independent review, topical commit; no live deployment.

## Acceptance Criteria

- Focused batch-account regressions and authenticated browser startup pass;
  unrelated browser failures remain separately visible.

## Evidence

- NAS `browser-auth-route-labels.log`: same-origin GET accounts404 twice, all other
  observed API responses200.
- Parent exact cached-image4.6.5 extract: `.local-instance/pinned-batch-accounts.txt`;
  no full-source checkout modification, alternate version, or network fetch.

## Completion

The ordinary inventory and real least-privilege schema HTTP matrix failed on the
missing route, then pass after implementation (`batch-inventory-green.log`,
`batch-http-green.log`,6.36s). Strict all-target/all-feature Clippy passes after
the test helper consumes its owned JSON array (`batch-lint-final.log`). The
permanent schema aggregate now includes this twelfth selector with offline
selection/nonempty guards. Actual authenticated browser startup now passes;
remaining settings-timing acceptance belongs to the separate browser-save issue.
Independent correctness/privacy/DRY review found no blockers. No registration,
write-grant, pagination or live-state change. Inherited Ruby-integer underscore/
overflow edge differences remain outside this endpoint's tested corpus.
