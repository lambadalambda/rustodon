# Repair remote reply-thread persistence

## Summary

Following recovery of Pleroma Notes with nullable sensitivity, reply-thread job `472` retries with `remote reply thread persistence failed`. The child status exists but its parent remains unresolved. User approved investigating this downstream failure.

## Requirements

- Diagnose with read-only live metadata and source inspection; do not display credentials, private post bodies, or backups.
- Reproduce any defect on Secunda and implement a minimal independently reviewed repair.
- Preserve canonical identity, provenance, privacy, thread bounds and existing data. Do not bypass validation to force a parent link.

## Acceptance Criteria

- Establish the failing persistence boundary with evidence.
- Applicable regression tests, formatting and lint pass on Secunda.
- If deployed, verify the existing job completes and the parent/child relationship is correct, or record the remaining external blocker.

## Notes

- Related: [missing followed posts](diagnose-missing-followed-lain-com-posts.md).
- Live source at start: `f9b6af7`; recovered child `117252276514092511`.
