# Implement outbound ActivityPub delivery

## Summary

Fan out and durably deliver core local activities to remote inboxes.

## Requirements

- Serialize status, follow, favourite, boost, block, undo, actor-update, and
  deletion activities from atomic outbox intent.
- Deduplicate shared inboxes and implement signed POST, retries, permanent-error
  classification, cancellation, and domain health.

## Acceptance Criteria

- Crash/retry tests and a Mastodon peer prove delivery is durable and idempotent.

## Progress

- Verified the complete write-transaction differential case after the
  association-first bookmark/favourite removal changes. The restored durable
  worker suite passes 33/33, including signed delivery, transient retry, crash
  replay, same-inbox cross-worker ordering, and Like/Announce lifecycle
  processing. Local durable ordering and worker behavior are covered; actual
  Mastodon peer convergence and client acceptance remain open.
- Added a transactional local-status distribution outbox event. Push workers build
  Mastodon-compatible `Create`/`Note` activities, deduplicate remote ActivityPub
  follower endpoints including shared inboxes, and enqueue immutable per-inbox
  delivery events before any network request.
- Delivery jobs use the bounded signed JSON POST transport, classify retryable
  versus permanent responses, suppress deleted statuses before sending, respect
  ActivityPub protocol/domain policy and limited-federation mode, and update the
  operational `domain_health` circuit state.
- Public, unlisted, private, limited, direct, mention, and reply audience data is
  represented for this status slice. Local status `Update` and tombstone `Delete`
  activities now use distinct durable distribution and delivery keys, and delete
  cancels pending Create/Update work without suppressing the Delete event.
  Favourite/boost, actor-update activities, full peer delivery, and crash/retry
  delivery tests remain open. Local remote Follow/Undo, Block/Undo Block, Reject, and
  remove-follower actions use the same transactional outbox and signed delivery
  path.
- Local remote Follow/Undo delivery records a deterministic URI before commit,
  deduplicates by URI and inbox, cancels unsent or unleased Follow work when
  unfollowing, and rechecks remote-domain policy at delivery time. Restored-
  fixture schema coverage proves the Follow/Undo payloads and retry behavior.
- Relationship delivery skips stale queued positive Follow or Block activities
   when their URI is no longer present in Mastodon relationship tables. A
   restored worker regression now holds a real signed Follow delivery live,
   records its Undo successor, proves another Push worker cannot claim that
   successor, and verifies Follow-then-Undo wire order after release. A
   matching restored Block regression now proves the same live-lease fence and
   Block-then-Undo wire order; Mastodon peer convergence remains open.
 - Local favourites and boosts now record deterministic signed `Like`, `Announce`,
   `Undo Like`, and `Undo Announce` delivery events for remote status authors.
   Positive interaction events are cancelled before unsent undo events are
   recorded, and origin-aware writes recheck domain and limited-federation policy.
   Restored-fixture schema coverage proves the payload shape and lifecycle.
- Reject activities now derive their outer ID from the numeric local Follow or
  FollowRequest row ID, matching Mastodon 4.6.5, across block teardown, follower
  removal, request rejection, actor deletion, and suspension. Immediate blocked
  Follow activities use the empty no-row suffix emitted by Mastodon. Unit and
  restored-fixture assertions cover both forms; live peer convergence remains
  open.
- Restored-fixture worker coverage now proves local status Create, edited Update,
  and deleted Tombstone delivery payloads, bringing the durable worker suite to
  18/18. Actual remote peer delivery, crash/order behavior, and actor/profile
  Update/Delete activities remain open.
 - Update delivery now uses versioned activity keys, coalesces stale queued edits,
   fences stale payloads before network delivery, and rechecks the remote-domain
   policy at execution time. Reach calculation follows Mastodon’s public and
   unlisted interaction rules while keeping private, direct, and limited status
   audiences bounded. The aggregate suite passes 126 library tests, with worker
   and schema integration at 18/18 and 24/24 respectively.
 - Added a deterministic local inbox transport fixture that exercises the real
     durable delivery worker, signed POST headers, body digest, host binding, and
     successful 2xx response handling. Worker integration now passes 19/19;
     actual Mastodon peer convergence and cross-worker crash/order delivery remain
     open.
- Local account profile writes now enqueue versioned Push-lane actor-update jobs
  transactionally. Workers serialize Mastodon-compatible `Update`/Actor payloads
  with profile media and `PropertyValue` fields, reach followers, reporters,
  recent mentions/follows/requests, and enabled relays, deduplicate inboxes, and
  fence stale profile versions before fan-out or delivery. Restored-fixture worker
  coverage now passes 20/20 and schema coverage remains 24/24; actual peer
  convergence and cross-worker crash/order delivery remain open.
- Push delivery now derives a stable source-account/inbox ordering key. Pending
  outbox events for one delivery stream are dispatched in event order, ordered
  jobs retain their predecessor ID, and claims fence successors while a prior job
  is live or recovering. First-marker creation is serialized across concurrent
  enqueuers, and marker cleanup preserves live ordered chains, including legacy
  markers. Worker integration now passes 24/24, including concurrent enqueue,
  expired-marker, and abandoned-lease recovery coverage. Actual Mastodon peer
  convergence and wire-level crash/retry proof remain open.
- Added a real durable-worker transient delivery test: a signed inbox POST that
  returns `503` is retried after domain-health cooldown and succeeds on the next
  `202` response, with the durable job cleared and domain failures reset. Worker
  integration now passes 25/25; process-crash duplicate delivery and actual
  Mastodon peer convergence remain open.
 - Added a transport-boundary crash test: a local peer accepts the first signed
   POST while withholding the response body, the worker is aborted before durable
   acknowledgement, lease recovery replays the activity, and the ordered Delete
   remains fenced until replay completes. The worker integration now passes 26/26;
   actual peer-side idempotency and Mastodon convergence remain open.
 - Local boost distribution now records a durable Push-lane Announce job and
   serializes public/private Announce and Undo Announce payloads for remote
   followers, enabled relays, and remote status authors. Pending boost delivery
   is cancelled transactionally on unboost, original-status deletion preserves
   remote reblogger recipients, and private self-boosts inline the original Note.
   Worker integration passes 26/26 and schema integration passes 30/30; actual
   peer convergence and cross-worker ordering remain open.
 - Signed remote replies to local parents are forwarded to the parent's remote
   followers through the durable signed-delivery path, excluding the source
   inbox and deduplicating by activity/inbox. Actual peer-side idempotency and
   full Mastodon convergence remain open.
 - Self-service local account deletion now records a durable actor `Delete`
   intent, fans it out to all eligible remote account/shared inboxes and relays,
   deduplicates by actor/inbox, and fences stale actor-update work after local
   suspension. Full account-content purge and relationship severance remain
   open.
  - Corrected status reach for remote quotes: updates and deletions now look up
    quote authors through `quotes.quoted_status_id` instead of the quoting
    status column. Restored-fixture worker coverage isolates a remote quoter and
     proves the update reaches its inbox; worker integration passes 30/30. Live
    peer convergence and cross-worker ordering remain open.
  - Account-update delivery now fences both the durable microsecond version and
    the second-resolution ActivityPub activity ID, preventing stale profile
     payloads when updates share a second. Outbound status, account, mention,
     report, and reply-forwarding paths now require ActivityPub protocol records;
     focused unit tests pass, while the restored-fixture regressions and live
     peer convergence remain open.
    - Signed remote Note forwarding now reaches followers of local reply parents,
      rebloggers, and quoters for Create, Update, and Delete activities. Original
      activity JSON is retained for the lifecycle, and forwarding remains
      deduplicated by signer, activity, and inbox.
     - Original-status deletion now records durable Delete distribution intents for
        affected local reblog wrappers, including when the original Delete arrives
        from a remote actor, so their Undo Announce reaches remote followers.
      - Deleted-status reach now keeps Mastodon's public/unlisted-only rule for
      reply targets and local-parent followers even when unsafe deletion reach
      is enabled, preventing deleted direct or limited replies from leaking to
      unrelated remote recipients. Worker integration remains 30/30.
   - Final reach and delivery reconciliation now covers suspension-triggered
      actor Updates, microsecond-versioned same-second account updates, preferred
      inbox grouping before recent-reach caps, alternate-host inboxes, direct
      Like delivery, and shared-inbox Announce deduplication. The final local
      gates pass: 171 library tests plus all target binaries, 29/29 worker tests,
      30/30 schema tests, and 19/19 differential cases. Live Mastodon peer
      convergence,
      cross-worker ordering, and client acceptance remain open.
    - Account-update reach now matches Rails' local-suspension grace window by
      using `suspended_at - 2 days` for recent mentions, follows, and requests.
      Restored worker coverage proves a delayed update reaches a recently followed
      remote account.
     - Current post-safety gates pass: `mise run check` covers 187 library tests,
       schema and worker integration pass 34/34 and 30/30, startup, preflight,
       operational-schema/streaming checks pass, and all 19 differential cases
       pass. Live peer convergence, cross-worker ordering, and client acceptance
       remain open.
    - Completed the local signed ActivityPub POST transport hardening slice.
      Test-support endpoint injection now exercises every resolved DNS answer,
      redirect-hop rebinding, same-origin 307/308 follow-up, 302 rejection,
      timeout, oversized response, and failure classification without opening
       a real remote connection. The focused transport tests pass, and the full
       `mise run check` gate is green; live Mastodon peer convergence remains
       open.
    - Outbound `401` classification now consults the source account's deletion
      request state, matching Mastodon’s permanent-unavailability behavior while
      preserving retries for active and temporarily suspended accounts.

## Closed 2026-09-23

Implementation complete. Remaining peer, browser, mobile and production
evidence moved to [complete-v1-external-acceptance](complete-v1-external-acceptance.md).
