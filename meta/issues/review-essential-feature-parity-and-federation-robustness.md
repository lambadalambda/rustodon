# Review essential feature parity and federation robustness

## Summary

Re-review the current codebase after live Pleroma federation exposed a PEM parsing incompatibility. Assess whether the implemented essential feature surface behaves correctly beyond the existing fixtures.

## Requirements

- Review inbound and outbound federation, authenticated user workflows, and durable processing against the stated v1 scope.
- Distinguish confirmed defects from unverified risks, explicitly deferred features, and known acceptance gaps.
- Report actionable findings with source locations and reproduction or test evidence; do not implement fixes as part of this review.

## Acceptance Criteria

- Independent bounded code reviews cover the critical compatibility boundaries.
- Findings and verification limits are recorded with prioritized follow-up recommendations.

## Notes

- Review requested after commits `884032b` and `1861d33` fixed and demonstrated Pleroma PEM interoperability.

## Result

- Completed the review at baseline `1861d33`; findings, source locations, regression scenarios, verification limits, and follow-up priorities are in [the review report](../essential-feature-parity-review.md).
- Independent reviewers covered federation, social REST, authentication, quality gates, and durable-work behavior. A separate cross-review confirmed the object-provenance and saved-status privacy findings.
- Reproduced null-summary Note rejection and standard actor-image Update rejection through the unmodified inbox parser in an isolated temporary Rust crate. Other scenarios are source-traced, not executed end-to-end.
- Archived this review task only. Seven focused implementation follow-ups remain open; performance observations R17/R18 are recorded for a separate scoping decision. No production changes or live federation writes were made.
