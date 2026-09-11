# Restore clean-checkout quality gates

## Summary

Remove the hidden upstream-source prerequisite from ordinary test/Clippy compilation or make its acquisition explicit and ordered.

## Requirements

- Prefer a checked-in explicitly test-only key and shared helper for ordinary signing tests, leaving upstream source contracts in their dedicated gate.
- If retaining the dependency, make source acquisition a prerequisite of both test and lint, not a parallel sibling of check.

## Acceptance Criteria

- Tests and all-target/all-feature Clippy compile in a clean checkout without preexisting target contents.
- The CI check job and documented local gate have all prerequisites declared.

## Notes

- Findings R16 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.
