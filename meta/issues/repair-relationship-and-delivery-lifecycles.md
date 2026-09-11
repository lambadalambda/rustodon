# Repair relationship and delivery lifecycles

## Summary

Correct accepted-follow preference updates, delivery cancellation ordering, and distinct Update activity identities.

## Requirements

- Update accepted remote/locked follows without creating a second request or Follow activity.
- Cancellation must not disconnect successors from earlier live jobs in the same delivery stream.
- Distinct status/profile versions need distinct durable wire IDs; retries must preserve version identity.
- Treat each lifecycle defect as a separate red/green fix and topical commit.

## Acceptance Criteria

- Follow/Accept/options/unfollow works for locked local and remote accounts without uniqueness conflicts or leftover requests.
- A multi-job, two-worker cancellation/interleaving test proves Undo cannot overtake its live positive activity.
- Two already-delivered same-second status/profile versions converge at a receiver without ID/body conflicts.

## Notes

- Findings R07, R08, R10 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.
