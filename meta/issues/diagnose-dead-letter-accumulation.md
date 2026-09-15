# Diagnose dead-letter accumulation

## Summary

Determine why worker jobs reached the dead-letter queue and whether they expose a product defect or expected handling of unrecoverable input.

## Requirements

- Inspect dead-letter metadata without retrying, deleting, or otherwise mutating jobs.
- Classify each failure by job kind, error, attempt history, and originating workflow.
- Distinguish operational/transient failures from actionable Rustodon defects.
- Record a focused remediation recommendation and create a separate implementation issue if a code change is warranted.

## Acceptance Criteria

- Every currently observed dead-letter job has a documented root-cause classification.
- The diagnosis identifies whether application data or worker health is currently affected.
- No dead-letter records are modified during diagnosis.
- Any actionable product defect has a reproducible case or a clearly scoped follow-up.

## Notes

- Keep instance-specific deployment and operations evidence in the private operations note, not this repository issue.
