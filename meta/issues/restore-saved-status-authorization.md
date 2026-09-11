# Restore saved-status authorization

## Summary

Recheck current root-status authorization when returning bookmarks and favourites.

## Requirements

- Do not treat association ownership as continuing access to a private status.
- Preserve association pagination semantics when filtering unauthorized statuses.

## Acceptance Criteria

- After follower removal and a subsequent author edit, neither saved endpoint exposes the private status or its new content.
- Mention-granted access and ordinary authorized saved-status reads remain correct.

## Notes

- Findings R02 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.
