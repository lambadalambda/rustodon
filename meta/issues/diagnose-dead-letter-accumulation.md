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

- Keep instance-specific deployment and operations evidence in the private
  operations note, not this repository issue.

## Diagnosis

- All four observed records are Pull-lane
  `rustodon.activitypub.fetch_profile_media` jobs. No other job kind or lane has
  dead letters.
- One job exhausted all four attempts because its remote origin returned HTTP
  500. A later bounded read-only request returned the same status, so the
  failure remains external and retrying it now would not help.
- Two jobs were rejected permanently on their first attempt. One URL returned
  an HTML document with HTTP 200 rather than image bytes. The other returned an
  SVG; profile media intentionally accepts only decoded JPEG, PNG, GIF, or WebP
  input.
- One job returned a valid 1286×574 GIF with 72 frames. Its 53,147,808 decoded
  frame-pixels exceed the explicit 16,777,216 profile-GIF work limit, so the
  bounded processor rejected it as designed.
- Each job still matches its account's current image URL and generation, and
  each affected cache slot remains empty. The impact is limited to missing
  remote profile media for those accounts. Worker readiness is healthy, every
  lane is covered, and no jobs remain queued.
- The queue retains cumulative attempts and only the final error, not a
  per-attempt history. The exhausted job's exact historical response sequence
  therefore cannot be reconstructed, but its final and current HTTP 500 plus
  the retry count establish its classification.
- Diagnosis used only SELECT-based admin/database inspection and bounded remote
  GETs with public-address validation. No job, account, media, or queue state
  was changed.
- No Rustodon defect was identified: the failures are one persistent remote
  server error, one incorrect remote media URL, and two intentional format/work
  bounds. A later ordinary account refresh can repair the transient case if its
  origin recovers; the three permanent inputs require the remote accounts to
  publish supported media.
