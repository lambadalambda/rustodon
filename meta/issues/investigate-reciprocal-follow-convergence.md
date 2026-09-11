# Investigate reciprocal follow convergence under concurrency

## Summary

An overlapping reciprocal-follow trial in the isolated Mastodon/Rustodon peer harness encountered a pinned Mastodon `account_stats` deadlock and left pending requests after retry. Sequential convergence passed; concurrent convergence remains unproven.

## Requirements

- Reproduce the overlapping schedule with fresh isolated peers and controlled concurrency; retain both peers' transaction, inbox and worker evidence.
- Distinguish a Mastodon deadlock/retry issue, Rustodon ordering/retry defect and harness artifact before proposing a fix. Do not modify the pinned upstream checkout.
- Preserve stable Follow/Accept identities, idempotency, ordering and bounded retry behavior; avoid hiding the failure by serializing the stress scenario.
- Keep this a relationship-correctness investigation, not the deferred R17/R18 queue-performance project.

## Acceptance Criteria

- Repeated simultaneous reciprocal follows converge to exactly one accepted relationship in each direction, with correct counters and no orphan requests or permanently stuck jobs; or an upstream-only blocker is precisely reproduced and documented.
- Follow-option changes and unfollow remain correct after convergence.
- Actual database/worker state, not inbox HTTP acceptance, establishes the result. Independent review and repeated task-owned cleanup checks pass on Secunda.

## Notes

- Follow-up to [isolated federation peer tests](add-isolated-federation-peer-tests.md); see the stress limit in [peer smoke documentation](../../docs/federation-peer-smoke.md).
- The observed deadlock is not yet attributed to a Rustodon defect. Initial sequential public smoke passed, while the expanded source-only scenarios await remote execution.

- Tracking only: no implementation or tests were performed for this issue. Builds, tests, formatting, lint and containers remain Secunda-only.
