# Tear down follows from locally suspended remote actors

## Summary

Match Mastodon when local moderation suspends a remote ActivityPub actor by
removing that actor's follows to local accounts and delivering Rejects. A
remote-origin actor `Update` remains distinct: its `suspended` flag must not
trigger local relationship rejection.

## Requirements

- Exercise the local moderation suspension transaction for a remote actor.
- Remove every follow from the suspended remote actor to local accounts and
  maintain both account counters and notification state.
- Cancel pending Accept delivery for removed follows and record durable signed
  Reject delivery intents in the same transaction as the suspension transition.
- Keep remote-origin actor suspension updates from rejecting relationships.

## Acceptance Criteria

- The restored worker fixture proves remote suspension removes the follow,
  preserves counters, removes its notification, cancels a pending Accept, and
  records the expected Reject outbox payload.
- Existing actor Update/Delete, idempotency, and full local gates remain green.

## Completion

- `administrative_remote_purge_undoes_passive_follows` now seeds an incoming
  remote follow and pending Accept, then proves local suspension removes the
  relationship and notification, restores counters, cancels Accept, and records
  Reject before the existing purge/Undo assertions.
- Accept outbox creation now locks and rechecks the follow row, preventing a
  concurrent Follow handler from recreating an Accept after suspension.
- `mise exec -- mise run worker-integration` passes 44/44, including the
  remote-origin actor Update lifecycle, whose follow-preservation assertion is
  explicit.
