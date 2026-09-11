# Define hashtag stream history cleanup

## Summary

Current hashtag-stream fan-out selects currently eligible recipients. A client that previously received a status may miss later cleanup after tag removal, tag unfollow or a policy change. Define and test the historical client-cache contract separately from current-recipient fan-out.

## Requirements

- Compare the pinned Mastodon behavior and document when clients must remove, invalidate or refetch previously received home entries.
- Cover removing the last matching tag, unfollowing the tag and losing access through policy changes before a subsequent edit/delete.
- If server-side cleanup is required, use bounded history or an appropriate invalidation mechanism; avoid an unbounded recipient-history framework.
- Cleanup must not retransmit newly private content to revoked recipients. Preserve visibility and block/mute policy checks and deduplicate account-follow/tag-follow overlap.

## Acceptance Criteria

- WebSocket plus REST regressions demonstrate the agreed client-cache behavior across each lifecycle, including reconnect/resume where relevant.
- Former recipients receive no unauthorized status body, and unrelated users receive no cleanup events.
- Independent review and applicable Secunda stream/fixture gates pass; deliberate protocol limits are documented explicitly.

## Notes

- Follow-up to R12 in [reviewed client workflows](close-reviewed-client-workflow-gaps.md).
- Current-eligibility and actual WebSocket lifecycle tests passed remotely. These historical schedules were not exercised; stale-cache impact must be established before choosing an implementation.

- Tracking only: no implementation or tests were performed for this issue. Builds, tests, formatting, lint and containers remain Secunda-only.
