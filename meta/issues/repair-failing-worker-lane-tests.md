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
