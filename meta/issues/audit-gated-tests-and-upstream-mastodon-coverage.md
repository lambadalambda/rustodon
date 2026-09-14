# Audit gated tests and upstream Mastodon coverage

## Summary

The user requests another code/test review: explain the many disabled or ignored tests and assess whether more behavioral tests should be ported from Mastodon, also available locally under `pleroma-org/mastodon`.

## Requirements

- Inventory ignored, feature-gated and unselected tests; distinguish intentional fixture/resource prerequisites from dead, failing or unexecuted coverage.
- Trace documented/automated test entry points and actual recent evidence; do not equate ordinary green tests with integration or real-peer coverage.
- Compare high-risk implemented behavior with concrete upstream Mastodon test cases, recording reference revision and differences from the pinned compatibility version.
- Produce a prioritized, bounded testing plan with source references; do not implement feature fixes, enable expensive suites, mutate upstream checkouts or change live services during this audit.

## Acceptance Criteria

- Explain counts/categories and why ordinary runs skip them, including any silently absent tests or broken default commands.
- Identify actionable harness/automation gaps and specific high-value upstream test ports, with existing coverage and expected payoff.
- Independently review the audit conclusions, record results, and present recommendations to the user.

## Notes

- Start source revision: `f1ed300a42722631b9d42ef2935e9694728ce0d0`.
- Any needed workloads run only in an isolated environment; local and upstream
  inspection remains read-only. No broad test execution is requested for this audit.

## Audit findings

Detailed report: [test gating and Mastodon coverage audit](../test-coverage-audit.md).

- Reconciled 163 ignored source annotations with retained isolated-worker evidence (431 passed,
  163 ignored). Distinguished fixture gates, feature-hidden tests, subprocess
  scaffolding, default/release configuration defects and absent CI selectors.
- Three bounded read-only subreviews covered annotation inventory, upstream API/
  frontend/media tests, and federation/lifecycle tests. Parent traced harness/CI
  wiring and prior execution evidence; no new Rust/test/lint workload ran.
- The user-provided local upstream is clean revision `761c61b42590a2fd91442fc15a0a7583e48bbea4`,
  declaring 4.7.0-alpha.1, not the 4.6.5 compatibility pin. For the direct-message
  and semantic-edit findings, parent also inspected matching source/specs inside
  the exact existing pinned 4.6.5 image on an isolated worker, read-only and network-disabled.
- Findings include nonpermanent HTTP regression selectors, missing fresh-worker
  media prerequisites, incomplete CI/release lanes, and three source-confirmed
  behavioral mismatches: direct/limited classification, meaningless edit side
  effects, and cached proxy derivative MIME. These were not live-reproduced or fixed.
- Recommendations explicitly reuse existing unexecuted peer scenarios and recent
  settings tests; propose selected upstream behavioral matrices, not bulk Rails
  implementation ports. No live service or upstream checkout was changed.
- Independent synthesis review requested before the audit documentation commit.
  Implementation is a separate next step, not part of this audit's acceptance.

## Completion

- Independent synthesis review found no material blocker. Corrected runner/task
  wording, the isolated-execution prerequisite, and per-status reply
  notification expectations before committing.
- Audit acceptance is satisfied and this review issue is archived. The report's
  implementation/porting recommendations remain proposed future work; no bug fix
  or test-gate activation is claimed. TDD execution is not applicable to this
  documentation-only review.
