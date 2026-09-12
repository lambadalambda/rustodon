# Assert remote semantic no-op edit-history behavior

## Summary

The semantic Update regressions cover edit timestamps, notifications and streams,
but do not explicitly assert history rows or the REST history projection. The
inbound path does not currently insert `status_edits`; when history is absent,
the REST loader synthesizes an entry from the current status. Both boundaries
need explicit coverage, without assuming that all metadata must remain frozen.

## Requirements

- Verify the supported history contract against pinned Mastodon 4.6.5 before
  setting expectations. Keep implementing remote history storage, if needed,
  separate from this focused regression scope.
- Extend the existing semantic-update fixture coverage for a newer sanitized-
  equivalent Update both without a prior edit and after a meaningful edit.
- Assert persisted history rows and the real REST history response before/after,
  including timestamps and fallback behavior when no history rows exist.
- Include meaningful-edit and replay controls. Distinguish unchanged rendered
  content from reconciled metadata (such as sensitivity) that the current-state
  fallback may legitimately expose; do not blindly require byte-identical JSON.
- Preserve history access authorization for private/direct statuses, and avoid
  duplicate tests or unrelated poll/history feature work.

## Acceptance Criteria

- Permanent fixture tests explicitly demonstrate no fabricated history version
  for semantic no-ops and the documented behavior of the current-state fallback.
- Test evidence distinguishes coverage gaps from confirmed production defects;
  any discovered correction receives its own failing regression and review.
- Relevant NAS fixture and ordinary checks pass; no live state is modified.

## Notes

- Follow-up to [semantic no-op edits](suppress-semantic-noop-remote-edits.md).
- Starting points: `tests/workers/semantic_updates.rs` and the history projection
  in `src/mastodon/rest/loader.rs`.
- User approved adding this follow-up after the audit implementation checkpoint.
- Open coverage task; no new history implementation or runtime result claimed.
