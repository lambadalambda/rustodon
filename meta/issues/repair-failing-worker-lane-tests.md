# Repair the failing worker lane tests

## Summary

The full `tools/mastodon-fixture worker-test` lane fails on main: 93 passed and
29 failed on both 31244a8 (baseline) and the split-domain change (NAS PG14,
2026-09-24, ~40-50 min runs under heavy disk contention). The failures cluster in
features that were committed without a worker-lane run (polls, stream events,
account stats, purge, quotes, readiness). Two `update_versions` tests fail in the
full run but pass alone (`worker-test update_versions`), so some failures come
from shared fixture state or test ordering.

## Failing tests on baseline 31244a8

- `activitypub_actor_update_and_delete_are_processed_idempotently`
- `activitypub_note_create_update_and_delete_are_processed_idempotently`
- `activitypub_status_update_and_delete_distribution_are_durable`
- `direct_visibility::explicit_mention_with_silent_cc_recipient_is_limited`
- `direct_visibility::explicitly_mentioned_cc_recipient_is_direct`
- `direct_visibility::explicitly_mentioned_to_recipient_is_direct`
- `direct_visibility::silent_recipient_is_limited_without_notification_or_conversation`
- `domain_purge_job_removes_remote_accounts_and_emoji`
- `lifecycles::update_versions::delivered_same_second_profile_updates_have_stable_distinct_ids`
- `lifecycles::update_versions::delivered_same_second_status_updates_have_stable_distinct_ids`
- `poll_expirations::bounded_startup_persists_continuation_and_core_baselines_remaining_history`
- `poll_expirations::concurrent_startups_share_one_poll_reconciliation_scan`
- `poll_expirations::poll_expiration_reconciliation_repairs_exact_generations_and_preserves_audit_rows`
- `poll_expirations::remote_expiry_replacement_finalizes_due_generation_before_suppression`
- `poll_expirations::startup_claims_exact_unleased_future_reconciliation`
- `poll_expirations::startup_poll_reconciliation_has_a_hard_readiness_bound`
- `poll_expirations::startup_reclaims_stale_reconciliation_and_fences_the_old_generation`
- `poll_expirations::startups_defer_to_one_existing_periodic_poll_reconciliation`
- `poll_expirations::zero_progress_poll_reconciliation_retries_same_rows_until_dead_letter`
- `quote_lifecycle::quote_federation_lifecycle`
- `remote_rich_media::activitypub_media_fetch_rich_rejections_and_install_fences`
- `runtime_publishes_readiness_and_removes_it_on_graceful_shutdown`
- `stale_account_purge_job_does_not_remove_media_after_unsuspend`
- `stale_authenticated_account_writes_are_rejected_after_deletion_request`
- `unsuspending_account_cancels_pending_deletion_jobs`
- `uri_only_create_is_deduplicated_retried_materialized_and_replayed`
- (three more failures are not in the summary list above; rerun to capture them)

## Acceptance Criteria

- Each failure is classified as a product bug, test-isolation bug or timing
  sensitivity, and fixed.
- The full worker lane passes on an idle worker host.

## Progress 2026-09-24

Full lane now 105 passed / 17 failed before the latest test fixes; a focused run
of the fixed tests passes 7/7. Classified so far:

- Product bug fixed (43cb557): outbox rows inserted as dispatched could get
  `dispatched_at` one microsecond before `created_at` and violate
  `outbox_events_dispatched_check`, failing purge/unsuspend writes.
- Outdated tests fixed (a49e72a, f695f83): the audience-independent stream hint
  (account 0) is intended and must only be unroutable; poll `expires_at` is
  `timestamp`; pending local quotes carry the RE: fallback (Mastodon does the
  same); poll statuses federate as Question; the runtime test must pass its
  writer pool; parent-fetch tests now scope to their own rows and restore their
  missing-stats precondition.
- Cascade: the domain-purge failure left rows that broke parent-fetch tests.

## Still failing

- `activitypub_note_create_update_and_delete_are_processed_idempotently`: a
  `quoted_update` notification for the deleted note remains (likely created by a
  late job after the delete cleanup).
- `uri_only_create_is_deduplicated_retried_materialized_and_replayed`: replay after
  the materialization boundary does not restore forwarding work.
- `quote_lifecycle::quote_federation_lifecycle`: `RowNotFound` without context.
- `lifecycles::update_versions::*`: pass alone, fail in the full run (dead letters).
- `poll_expirations::` bounded startup ordering, repair timing
  (`repaired_late <= scan_started_at`), hard readiness bound (unexpected NULL),
  deferral timeout, zero-progress retry: reconciliation startup under load and
  shared state.

## Follow-up (not a test failure)

- Actor deletion emits two idempotent deletes per user stream (keys `:0` and
  `:<version>`); consider one key.
- Review 2026-09-24: `apply_remote_note_delete` holds the actor row FOR UPDATE and
  then takes recipient advisory locks for notification cleanup, while
  `create_notification` takes the recipient lock and then needs a KEY SHARE on the
  actor (FK). A concurrent quoted_update job and remote Delete can deadlock (40P01);
  both retry. Pre-existing pattern in `delete_remote_status_notifications`.
- Local status delete detaches quotes but keeps quoted_update notifications (like
  Mastodon); the remote delete path now removes them. Decide one behavior.

## Progress 2026-09-24 (second pass)

Full lane 118 passed / 4 failed on 0d7ec35-era code; after the later fixes the
focused runs pass everything except two tests. Fixed since the first pass:

- Product: resolved-Note replay recorded forwarding only after a refetch (a failed
  fetch dropped it) — now before the refetch and after the domain policy check.
- Product: remote Note delete detached quotes before its notification cleanup, so
  quoted_update notifications survived; late quoted_update jobs now also skip
  quotes whose quoted status is gone or soft-deleted.
- Tests: quote test delivered to the fixture's protocol-0 Bob and required a
  lazily created status_stats row; poll zero-progress test lacked the activation
  marker; repair test asserted the late job before the scan start instead of at
  its poll expiry; update_versions failures were load/leftover related.

## Still open (need a behavior decision)

- `poll_expirations::startup_poll_reconciliation_has_a_hard_readiness_bound`: with
  the single writer connection held, startup times out without having claimed
  the reservation job (`lease_expires_at` NULL). The test expects the claim to
  happen before writer access. Decide whether startup should claim first.
- `quote_lifecycle::quote_federation_lifecycle` (scalar step): the test expects
  quotes_count 2, but its own earlier step revokes the allowed quote and the
  scalar instrument arrives as a legacy quote. Mastodon 4.6.5 counts a quote
  created accepted even when legacy (`after_create_commit` checks only
  `accepted?`); Rustodon's insert path skips legacy quotes. Verify Mastodon's
  create/accept order for scalar QuoteRequests (differential) before changing
  either the counter or the test.

## Mastodon check 2026-09-24

- Quote scalar step: Mastodon 4.6.5 creates the quoteUrl-only instrument quote as
  legacy and pending, accepts it without a counter change
  (`Quote#update_counter_caches!` returns on `legacy?`), and the earlier revoke
  decremented. Expected count 0; Rustodon already gives 0. Test fixed. The test now
  stops later: "timed out waiting for quote deliveries" (fewer than the 3 expected
  deliveries reach the mock inbox); needs partial-result diagnostics next.
  Mastodon asymmetry to keep in mind: destroying an accepted legacy quote still
  decrements (`decrement_counter_caches!` only checks `accepted?`).
- Poll startup: Mastodon has no startup reconciliation for poll expirations; it
  relies on Sidekiq scheduled jobs (`PollExpirationNotifyWorker.perform_at`), and a
  lost job means a lost notification. Rustodon's reconciliation and its
  claim-before-writer guard are Rustodon's own design; the test expresses that
  design, so the decision is ours.

## Resolution 2026-09-24

Full lane green on the NAS: 122 passed, activity groups 8/8 and 2/2, worker
readiness passes. Poll startup now keeps its lease when no writer connection is
available (claim-before-writer kept as Rustodon's design). Quote deliveries were
missing because of a wrong Accept/Reject sender check (product bug) and test
state leaks; see DEVLOG.
