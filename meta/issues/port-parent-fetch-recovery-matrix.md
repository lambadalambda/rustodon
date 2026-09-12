# Port failed parent-fetch recovery and distribution matrix

## Summary

Extend existing reply hydration workers with transient failed parent-fetch followed by recovery, retaining correct author/privacy/thread identity and reply distribution.

## Acceptance Criteria

Verify pinned expectations, durable retry/idempotency and final received thread/notification state. Distinguish per-status notifications from true duplicates; wire into the permanent worker gate. Do not replay historical/live jobs.

## Notes

- Subissue of [selected matrix ports](port-mastodon-media-and-browser-matrices.md).
- Tests first; separate topical implementation and independent review.
- Pending; no implementation or execution evidence claimed.
