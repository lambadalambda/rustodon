# Implement basic moderation and reconciliation

## Summary

Provide reports, required moderation CLI actions, and state repair commands.

## Requirements

- Add report submission and permission-checked CLI report resolution,
  suspend/unsuspend, domain block/unblock, and local-status deletion.
- Preserve audit/history, soft-delete/tombstone/delivery effects, and reconcile
  counters and denormalized data.

## Acceptance Criteria

- Differential and rollback tests prove compatible moderation state without a
  full admin dashboard.

## Notes

- Current implementation slice: add a durable, authorized full domain-purge path
  matching Rails `PurgeDomainService`, separate from delayed account deletion.

- Local `POST /api/v1/reports` now has `write:reports` authorization, transactional
  status/collection persistence, REST serialization, and durable staff
  notification delivery.
- `rustodon admin resolve-report --report-id <id> --actor-account-id <id>` now
  resolves reports, while `--reopen` clears the resolution. Both paths require
  an active account with effective `manage_reports` permission and write a
  transactional `admin_action_logs` record.
- `rustodon admin delete-status --status-id <id> --actor-account-id <id>` now
  uses the existing transactional local deletion lifecycle, including optional
  media cleanup, delivery outbox effects, and a `destroy` audit record.
- `rustodon admin reconcile-account-stats --account-id <id> --actor-account-id <id>`
  now repairs followers, following, status counts, `last_status_at`, and
  `status_stats.replies_count` under effective `manage_users` authorization,
  with an audit record in the same transaction.
- Report creation is covered by restored-fixture schema/worker tests and a
  differential HTTP case, including invalid-collection rollback and silent
  mention attachment behavior. Invalid rule handling, duplicate rule IDs,
  target availability, and least-privilege rule reads are also covered or
  enforced.
- Federated report forwarding now persists one durable ActivityPub `Flag` job
  for the remote target and each distinct eligible remote reply-server inbox,
  excluding the target inboxes and preserving Mastodon's nullable `forwarded`
  field behavior. The schema fixture asserts the exact payload and inbox set;
  the differential report case passes against Mastodon 4.6.5.
- Account suspension and unsuspension now apply the local deletion request,
  canonical email block, Mastodon account warning, unresolved-report
  resolution, audit, local account-update outbox, and remote follow-rejection
  effects transactionally. Local suspensions also enqueue a durable
  `moderation_warning` notification; remote unsuspension is rejected without a
  fresh remote resolution.
- Domain block writes normalize the domain, preserve the strongest matching
  ancestor rule, reject weaker replacements, update matching remote accounts,
  and restore account restrictions on unblock. Account-stat reconciliation and
  status deletion preserve Mastodon's direct-status and public-reply counter
  rules.
- Local Paperclip media and thumbnails now reuse REST status authorization:
  public/private/direct audience rules, blocks, mutes, suspension, and soft
  deletion are enforced before file reads. Discarded media remains available
  only to an authenticated account with effective `manage_reports`, matching
  Mastodon's `MediaAttachmentPolicy#download?` exception. The guarded
  `local_paperclip_deleted_media` differential case proves anonymous denial and
  moderator access after soft deletion.
- The restored Mastodon 4.6.5 schema integration covers the moderation
  lifecycle, notification outbox, counter repairs, least-privilege writer, and
  rollback cleanup. The full schema suite passes 35/35; the aggregate check,
  differential suite, preflight, worker, and startup gates also pass.
- Report creation now queues durable Mail-lane staff report messages for
  moderators whose flat Mastodon `notification_emails.report` setting is
  enabled. The worker renders report context and ID through the existing SMTP
  transport, with outbox and SMTP integration coverage. Report writes enforce
  the 400-per-UTC-day account limit and preserve Mastodon report validation.
- Domain-block updates repair nullable or unknown existing rules without
  weakening them. Local report creation remains allowed for a target domain
  with `reject_reports`, matching Mastodon's `ReportService`; inbound remote
  `Flag` rejection remains with the ActivityPub inbox issue. The full schema
  suite passes 35/35.
- Suspend-level global domain blocks now enqueue durable cleanup after the policy
  transaction. The worker rechecks the block and suspension timestamp, records
  Rails-compatible relationship severance rows and local notifications, removes
  matching Paperclip files/metadata, and deletes remote custom emoji rows. The
  restored worker suite covers the relationship directions, settings, counts,
  notification, and filesystem cleanup.
- Remote actor `Update` activities now validate and persist `toot:suspended`,
  distinguish remote suspension from local suspension and deletion tombstones,
  and support remote unsuspension. Remote actor/Note deletion and due account
  purge also remove eligible Paperclip files while preserving reported content;
  the restored worker suite passes 32/32.
- Local suspensions and self-service account deletion now record an atomic
   `kill` stream event. The WebSocket loop polls system events even before a
   subscription exists and closes the connection with code 1000; the restored
   fixture covers the live suspension path and preserves the normal event cursor.
- Full Mastodon status-filter parity for blocked accounts and filtered reblogs,
  full admin domain purge, token-specific revocation events, and the required
  local/remote suspension side effects are covered.
- Self-service account deletion now uses the existing account-deletion request
  table and local suspension state without adding a moderation warning or
  canonical email block; the durable actor-delete fan-out is covered by worker
  integration.
- Administrative local suspension now enqueues the existing delayed
   `mastodon:account:<id>:purge` job in the same transaction as the deletion
   request and moderation effects, plus a delayed ActivityPub actor-delete job
   with the same due time. The pinned Rails `SuspendedUserCleanupScheduler`
   processes every due deletion request; the restored schema test verifies both
   scheduled jobs and their cancellation on unsuspension.
- Domain block and purge cleanup now share canonical advisory locks with remote
  actor and media writes. The worker holds the lock across metadata collection,
  Paperclip removal, and database mutation; remote media retries reclaim stale
  processing claims and recheck policy before final persistence. Restored worker
  coverage passes 34/34, including the seeded stale-claim case.
- The full administrative domain purge fixture now covers case-normalized exact
  domain selection, preservation of subdomain accounts/emoji/instances,
  remote account/status/report/notification descendants, Paperclip file removal,
  severance flags, local counter preservation, no stream side effects, and safe
  replay after completion.
  `mise run worker-integration` passes 34/34; crash-time filesystem compensation
  and live peer evidence remain outside this local proof.

## Completion

- The required moderation and reconciliation actions satisfy the acceptance
  criterion through restored Mastodon schema/worker coverage, Rails-versus-Rust
  differential cases, and cutover rollback verification. PostgreSQL timeline
  filtering and guarded media reads provide the v1 equivalents of Redis feed and
  media visibility side effects; trends and broader remote-suspension behavior
  remain outside this issue's v1 scope.
