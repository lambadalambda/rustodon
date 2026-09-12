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
- NAS is authorized for workloads if needed; local/upstream inspection is read-only. No broad test execution is requested for this audit.
