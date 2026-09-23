# Handle ActivityPub Note activities

## Summary

Process core remote Note and social activities idempotently.

## Requirements

- Handle Note Create/Update/Delete, Like/Undo, and Announce/Undo with replies,
  mentions, audiences, attachments, edits, soft deletion, and tombstones.
- Prevent deleted-object resurrection and retain text when media fetch fails.

## Acceptance Criteria

- Database/media differential and adversarial duplicate/order tests match a
  Mastodon peer without private/direct leakage.

## Progress

- Added inbound Note Create/Update/Delete processing with signer/object
  identity checks, URI advisory locks, duplicate suppression, delete
  tombstones, stale/deleted Update fencing, raw HTML preservation, audience
  visibility, replies, mentions, hashtags, remote media metadata, edit
  timestamps, and untrusted interaction counts. Restored-fixture coverage now
  exercises duplicate Create/Delete, Update after Delete, and media metadata.
- Remote Note `likes`/`shares` collection totals and legacy count fields are
  now clamped to Mastodon's `0..100_000_000` untrusted-counter bounds, with
  unit and restored-worker coverage for negative and oversized values.
- Outbound ActivityPub Note attachments now preserve stored `blurhash` values
  and original media dimensions, and materialized thumbnails now serialize as
  Mastodon-compatible `Image` icons, with focused serializer coverage and
  federation differential verification.
- Outbound ActivityPub Note attachments now preserve complete numeric media focus
  metadata as `focalPoint` and omit malformed focus values.
- Outbound ActivityPub Notes now mark content sensitive for sensitized authors,
  matching Mastodon's account-level sensitivity rule.
- Outbound quoted Notes now include Mastodon's `quote`, `quoteUri`, and
  `_misskey_quote` identifiers together.
- Outbound Notes now include Mastodon's automatic quote `interactionPolicy`,
  including public, followers, following, and safe actor fallback approvals.
- Accepted remote and local quote approvals now emit `quoteAuthorization` on
  web and durable-worker Notes. Local authorization documents are served at
  both username and numeric account routes with accepted-state, visibility, and
  deleted-object checks.
- Added inbound Like/Undo and Announce/Undo processing with actor/activity host
  checks, known-local-status policy, duplicate and Undo-before-activity
  tombstone fencing, interaction counters, notifications, and restored-fixture
  coverage. The worker fixture now passes 13/13.
- Added relevance gating for inbound Notes, scalar/null `to` and `cc` parsing,
  computed local-status URI matching, and signed Pull-lane parent resolution.
  Child-first replies now remain replies, persist one transactional resolver job,
  fetch and validate missing parents, upsert unknown parent actors, repair the
  parent/account fields transactionally, and increment reply counters exactly
  once. Restored-fixture worker coverage now passes 14/14, including remote and
  computed-local parent resolution.
- Media fetching and the full differential and adversarial peer matrix remain
  open.
- Added bounded image media fetching, Paperclip persistence, media proxy
  authorization, and failed-fetch text retention. Restored-fixture worker
  coverage now passes 15/15; successful remote HTTP media and the full
  differential/adversarial peer matrix remain open.
- Hardened known-remote Announce/Undo and Note Delete handling with nested
  boost resolution, local-follower relevance fencing, self-private remote
  boosts, Group notification suppression, author-scoped deletion, dependent
  boost cleanup, and consistent status lock ordering. Restored-fixture worker
  coverage now passes 21/21 and the library suite passes 147/147. Successful
  remote HTTP media and the full differential/adversarial peer matrix remain
  open.
- Added durable Pull-lane resolution for unknown Announce targets, embedded
  self-boost creation, bounded fetched Note validation, nested Announce
  materialization with a depth limit, local-follower signed fetching, and
  distinct Create-activity versus Note URIs. Remote Note writes now preserve
  ordered media IDs, fence media work after deletion, and remove dependent
  status/mention/quote-update notifications. Restored-fixture worker coverage
  passes 22/22 and the aggregate library suite passes 152/152. Successful
  remote HTTP media and the full differential/adversarial peer matrix remain
  open.
- Identified that incoming remote Note and Announce/Undo writes lacked
  authenticated user-stream status events even though the underlying database
  mutations and notifications committed; restored-fixture coverage and
  transactional stream fan-out were required.
 - Remote Note Create/Update/Delete now record transactional authenticated
   user-stream events, including deleted reblog wrappers. Restored worker
   coverage also proves remote Announce/Undo and URI-only Undo stream events;
   worker integration passes 26/26, schema integration 30/30, and all 18
   differential cases pass. Full peer convergence remains open.
 - Local boost distribution now emits durable Announce and Undo Announce
   activities, preserves remote reblogger reach when an original status is
   deleted, includes enabled relay inboxes for public status delivery, and
   matches Mastodon's private self-boost Note inlining and Announce audience.
   Focused worker and schema integration remain green at 26/26 and 30/30.
- Signed remote replies to local parents are now forwarded to the local
  parent's remote followers with immutable per-activity/inbox delivery keys;
  duplicate ingress does not reset a dispatched forwarding job. Remote Note
  and actor teardown also removes affected favourites and poll data, restores
  counters, and clears related notifications.
- Unknown ActivityPub Note Updates whose `published` timestamp is older than
  24 hours are now ignored before materialization, while known statuses and
  tombstones retain their existing update/delete behavior. Unit and restored
  worker coverage prove the stale-object fence.
- Successful remote HTTP media fetching is now covered by the restored worker
  fixture: a real bounded HTTP response is persisted through Paperclip with
  original and GIF-thumbnail files, metadata, blurhash, and durable-job
  completion. The full differential and adversarial peer matrix remain open.

## Closed 2026-09-23

Implementation complete. Remaining peer, browser, mobile and production
evidence moved to [complete-v1-external-acceptance](complete-v1-external-acceptance.md).
