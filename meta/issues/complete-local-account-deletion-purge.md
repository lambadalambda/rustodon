# Complete local account deletion purge

## Summary

Complete the delayed local cleanup behind the authenticated self-service account
deletion request while retaining the account username and ActivityPub signing
identity needed for federation delivery.

## Requirements

- Queue a distinct delayed purge job for an account deletion request.
- Remove local account content, interactions, relationships, profile media, and
  account-owned associations without deleting the retained account identity.
- Disable the retained user, clear the profile, preserve unresolved reported
  content where Mastodon does, and remove the deletion request only after the
  purge commits.
- Keep the purge idempotent, transactional where practical, least-privilege,
  and safe to retry.

## Acceptance Criteria

- Restored-fixture coverage proves due-time gating, purge cleanup, reported
  content retention, counter updates, idempotent retry, and rollback.
- Actor-delete delivery remains durable and signed after the local purge.
- Writer privilege/preflight checks cover every newly mutated table.

## Notes

- The pinned Mastodon 4.6.5 reference uses a 30-day deletion delay and
  `DeleteAccountService` with `reserve_username: true` and `reserve_email: true`.
- Full filesystem garbage collection and relationship-severance event history
  may remain separate if they cannot be made retry-safe in the same milestone.

## Progress

- The purge worker now collects the exact eligible Paperclip paths into its
  leased durable-job arguments before the database transaction. The account
  purge commits database cleanup before removing files, and file-removal
  failures retain the manifest for a fenced retry. Missing or malformed
  manifests fail closed.
- Restored-fixture coverage now forces a filesystem failure after the database
  purge, verifies the job retains its paths and the database state is committed,
  then repairs the media root and proves the retry removes the files. The
  durable queue test also proves stale leases cannot merge job metadata.
- Account-owned writes now recheck lifecycle state inside their write
  transactions through the shared account-write fence, and profile/media
  filesystem operations use the canonical account lock. Actor-delete delivery
  rechecks the lifecycle at the final send boundary, while purge updates
  `status_stats.quotes_count` for accepted quotes attached to deleted source
  statuses.
- Restored worker coverage passes 39/39 and the Mastodon schema gate passes
  35/35. The aggregate `mise run check` gate also passes after adding the
  lifecycle regressions.
- A hard power-loss test is still unavailable; the durable ordering and retry
  behavior are covered without modifying the pinned Mastodon checkout.
