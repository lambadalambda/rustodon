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

## Resolution

- Ordinary signature tests now include one checked-in, publicly known test PEM instead of extracting it from an ignored upstream checkout. Existing golden signature assertions remain unchanged.
- The clean run exposed a second hidden dependency: five ordinary Paperclip tests read upstream media at runtime. Three byte-identical pinned media fixtures are now checked in with provenance, and the tests retain their original assertions.
- RED on `lain@secunda.local`, isolated `/home/lain/rustodon-parity/main` with no upstream source link: `cargo test --locked --all-targets --all-features --no-run` failed at the three compile-time includes. After replacing those, the full suite failed five Paperclip tests with missing fixture errors.
- GREEN on the same isolated Secunda workspace without the source checkout: `cargo fmt --all --check`, `cargo test --locked --all-targets --all-features` (406 passed, 125 explicitly ignored across 22 binaries), and `cargo clippy --locked --all-targets --all-features -- -D warnings` passed. Remote `cmp` verified all three media files against the pinned Git blobs.
- An independent read-only review found no blockers in the fixture changes, preserved tests, or provenance. Explicit integration and pinned-source gates retain their documented prerequisites; ignored tests are not claimed as executed here.
