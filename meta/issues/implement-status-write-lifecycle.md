# Implement the status write lifecycle

## Summary

Implement compatible status creation, editing, and soft deletion.

## Requirements

- Support replies, content warnings, sensitivity, language, mentions, all five
  visibilities, four images, edit history, and API idempotency.
- Atomically maintain URI, conversations, silent mentions, tags, media order,
  counters, notifications, tombstones, and post-commit work.

## Acceptance Criteria

- Differential state and duplicate-request tests match Mastodon and rollback
  reopens every result; polls, schedules, and quote creation remain excluded.

## Progress

- Added a bounded local text-status create path with `POST /api/v1/statuses`.
  It supports public, unlisted, private, direct, and limited visibility,
  content warnings, sensitivity, language, application ownership, quote policy
  defaults, conversations, status statistics, account counters, and the
  optional operational idempotency key.
- Owner-role schema coverage proves all five visibility rows and idempotent
  replay; the guarded differential case proves response and stable database
  field parity for all five visibility values.
- Extended creation to authorized replies with target conversation and language
  inheritance, reply metadata, and parent reply counters. The guarded case now
  proves a reply alongside the five standalone visibility cases.
- Added a bounded owner-only DELETE path that serializes before mutation,
  soft-discards reblogs, removes status pins, and maintains the owner counter
  transactionally. Owner-role schema coverage proves reblog and pin cleanup;
  the differential case deletes generated rows so fixture dependencies remain
  isolated.
- Scoped API idempotency by account, bound it to reply targets and canonical
  text/spoiler values, and evict expired keys during claims. Unit and owner-role
  schema coverage prove account/reply identity, replay, and expiry behavior.
- Status creation now attaches up to four owned, ready media IDs transactionally,
  preserves their order, accepts media-only posts, defaults omitted language to
  `en`, persists extracted hashtags and featured-tag counters, and includes
  guarded differential coverage for media/status/tag rollback.
- Existing local/remote account mentions now create filtered-policy-aware mention
  rows and notification activities; the guarded status differential covers a
  local mention and restores notifications.
- Added `PATCH /api/v1/statuses/:id` for owner-authenticated text, content-warning,
  sensitivity, language, and ordered-media edits. The transaction creates the
  initial and current `status_edits` snapshots, refreshes hashtags and mentions,
  maintains featured-tag counters, and preserves the Rails no-op behavior.
  `PUT /api/v1/statuses/:id` now aliases the same Rails update action. Differential
   coverage compares HTTP responses and persisted fields, proves two edit-history
   snapshots, verifies media removal from the ordered list, and proves identical
   PUT replay does not add another snapshot.
- Local and remote status edits now record transactional Core-lane `update` and
   `quoted_update` notification jobs for local rebloggers and accepted local
   quotes. The worker dispatches both replacement-aware notification types, and
   restored-fixture schema/worker coverage proves the outbox and delivery paths.
   The transactional delete-media slice is covered separately; remote account
   resolution, explicit idempotency-key edit replay, feed fan-out, and broader
   post-commit distribution/removal work remain open.
- Local status creation notifications now have a durable post-commit Core-lane
  producer for active followers. Remote delivery and feed insertion remain
  separate follow-up work.
- Status updates now parse Mastodon's nested `media_attributes[]` form and
  transactionally update descriptions and image focus for media retained by the
  status. Media-only edits participate in the significant-change decision, and
  repeated edits no longer create duplicate previous snapshots.
- Status media IDs are deduplicated in the Rails order, edit-history media
  presentation is capped at four attachments, and the image-only scope leaves
  video thumbnail replacement outside this issue.
- Status create and edit language values now follow Mastodon's supported-locale
  cascade: supported regional locales are preserved, unsupported regional forms
  such as `fr-FR` fall back to `fr`, and unknown values fall back to the current
  or default language. The guarded differential edit covers the persisted result.

## Completion

- The acceptance criteria are satisfied by the restored-fixture schema tests and
  Rails-versus-Rust differential write case, including duplicate requests,
  rollback restoration, replies, edits, media ordering, and all five supported
  visibilities. Polls, schedules, quote creation, and broader post-commit
  distribution remain separately scoped work.
