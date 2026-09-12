# Investigate remote Update version ordering after semantic no-ops

## Summary

Inbound Update rejection currently compares the supplied version with the last
meaningful `edited_at` (or `created_at`). A newer semantic no-op reconciles
metadata without advancing that version. After a meaningful edit at T1 and a
no-op at T3, an out-of-order meaningful Update at T2 may therefore still apply.
This was deliberately left outside the semantic no-op fix; it is not yet a
confirmed divergence from pinned Mastodon behavior.

## Requirements

- Confirm pinned Mastodon 4.6.5 ordering behavior using the existing read-only
  reference or verified cached-image source; do not infer a contract from 4.7.
- Add a restricted-writer regression for T1 < T2 < T3 delivered as T1, T3, T2,
  including distinct queue identities so enqueue deduplication cannot mask it.
- Distinguish an accepted remote-version watermark from user-visible edit time.
  Cover replay, equal/older versions, metadata reconciliation and edit effects.
- Preserve signer/ownership, timestamp, tombstone, privacy and transaction guards.
  Invalid or rejected activities must not advance any accepted-version fence.
- Decide whether a separate watermark is necessary before proposing schema or
  grant changes. Keep any such change independently reviewed and topical.

## Acceptance Criteria

- Pinned behavior and the chosen ordering policy are documented with evidence.
- A permanent regression establishes that policy; any behavioral correction has
  red/green evidence and does not manufacture edit timestamps or notifications.
- No live replay, historical rewrite or deployment is implied.

## Notes

- Follow-up to [semantic no-op edits](suppress-semantic-noop-remote-edits.md).
- Source entry point: `WriteRepository::apply_remote_note_update`.
- User approved adding this follow-up after the audit implementation checkpoint.
- Open investigation; no watermark implementation or runtime result claimed.
