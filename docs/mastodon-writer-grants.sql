-- Execute this script after the Rustodon operational schema migration as a
-- database administrator or an owner with grant rights on both schemas.
-- Supply the dedicated writer role with:
--   psql ... --set writer_role=rustodon_writer -f docs/mastodon-writer-grants.sql
--
-- The role must be separate from the read/runtime role. Create it with LOGIN,
-- NO SUPERUSER, NO CREATEDB, NO CREATEROLE, NO REPLICATION, NO BYPASSRLS,
-- and no role memberships before running this script.

\set ON_ERROR_STOP on

BEGIN;

-- Remove database, schema, object, and stale role grants that would broaden
-- the writer contract or allow PUBLIC to reach Mastodon objects.
SELECT format(
  'REVOKE ALL PRIVILEGES ON DATABASE %I FROM PUBLIC', current_database()
) \gexec
SELECT format(
  'REVOKE ALL PRIVILEGES ON DATABASE %I FROM %I',
  current_database(), :'writer_role'
) \gexec
SELECT format(
  'GRANT CONNECT ON DATABASE %I TO %I',
  current_database(), :'writer_role'
) \gexec

REVOKE ALL PRIVILEGES ON SCHEMA public FROM PUBLIC;
REVOKE ALL PRIVILEGES ON SCHEMA public FROM :"writer_role";
GRANT USAGE ON SCHEMA public TO :"writer_role";
REVOKE ALL PRIVILEGES ON SCHEMA rustodon FROM PUBLIC;
REVOKE ALL PRIVILEGES ON SCHEMA rustodon FROM :"writer_role";
GRANT USAGE ON SCHEMA rustodon TO :"writer_role";

REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA public FROM PUBLIC;
REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA public FROM :"writer_role";
REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public FROM PUBLIC;
REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public FROM :"writer_role";
REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA public FROM PUBLIC;
REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA public FROM :"writer_role";
REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA rustodon FROM PUBLIC;
REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA rustodon FROM :"writer_role";
REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA rustodon FROM PUBLIC;
REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA rustodon FROM :"writer_role";
REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA rustodon FROM PUBLIC;
REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA rustodon FROM :"writer_role";

GRANT SELECT ON TABLE
  public.account_aliases, public.account_conversations,
  public.account_deletion_requests, public.account_domain_blocks,
  public.account_migrations, public.account_notes, public.account_pins,
  public.account_relationship_severance_events, public.account_stats,
  public.account_warnings, public.accounts, public.accounts_tags,
  public.admin_action_logs, public.blocks, public.bookmarks,
  public.canonical_email_blocks, public.collection_items,
  public.collection_reports, public.collections, public.conversation_mutes,
  public.conversations, public.custom_emojis, public.custom_filters,
  public.domain_allows, public.domain_blocks,
  public.fasp_follow_recommendations, public.favourites, public.featured_tags,
  public.follow_requests, public.follows, public.generated_annual_reports,
  public.invites, public.keypairs, public.list_accounts, public.lists,
  public.login_activities, public.markers, public.media_attachments,
  public.mentions, public.mutes, public.notification_permissions,
  public.notification_policies, public.notification_requests,
  public.notifications, public.oauth_access_grants,
  public.oauth_access_tokens, public.oauth_applications, public.poll_votes,
  public.polls, public.quotes, public.relationship_severance_events,
  public.relays, public.report_notes, public.reports, public.rules,
  public.scheduled_statuses, public.session_activations,
  public.severed_relationships, public.status_edits, public.status_pins,
  public.status_stats, public.statuses, public.statuses_tags,
  public.tag_follows, public.tags, public.tombstones, public.user_roles,
  public.users, public.web_push_subscriptions, public.webauthn_credentials
  TO :"writer_role";

-- Web-client snapshots are separate from users.settings. No identity rewrite or delete.
GRANT SELECT ON TABLE public.web_settings TO :"writer_role";
GRANT INSERT (user_id, data, created_at, updated_at) ON TABLE public.web_settings TO :"writer_role";
GRANT UPDATE (data, updated_at) ON TABLE public.web_settings TO :"writer_role";
GRANT USAGE ON SEQUENCE public.web_settings_id_seq TO :"writer_role";

GRANT UPDATE (
  username, domain, display_name, note, actor_type, locked, memorial,
  discoverable, trendable, also_known_as, moved_to_account_id, reviewed_at,
  requested_review_at, hide_collections, indexable, attribution_domains,
  fields, avatar_content_type, avatar_description, avatar_file_name,
  avatar_file_size, avatar_remote_url, avatar_storage_schema_version,
  avatar_updated_at, header_content_type, header_description, header_file_name,
  header_file_size, header_remote_url, header_storage_schema_version,
  header_updated_at, silenced_at, suspended_at, suspension_origin, uri, url,
  inbox_url, outbox_url, followers_url, following_url, shared_inbox_url,
  protocol, public_key, last_webfingered_at, updated_at
) ON TABLE public.accounts TO :"writer_role";
GRANT UPDATE (
  settings, consumed_timestep, otp_backup_codes, otp_required_for_login,
  otp_secret, current_sign_in_at,
  last_sign_in_at, sign_in_count, encrypted_password, reset_password_token,
  reset_password_sent_at, sign_in_token, sign_in_token_sent_at, confirmed_at,
  confirmation_token, confirmation_sent_at, disabled, updated_at
) ON TABLE public.users TO :"writer_role";
GRANT DELETE ON TABLE public.webauthn_credentials TO :"writer_role";
GRANT INSERT (
  username, private_key, public_key, created_at, updated_at
) ON TABLE public.accounts TO :"writer_role";
GRANT INSERT (
  username, domain, actor_type, display_name, note, uri, url, inbox_url,
  shared_inbox_url, protocol, public_key, last_webfingered_at, created_at,
  updated_at
) ON TABLE public.accounts TO :"writer_role";
GRANT INSERT (
  account_id, email, encrypted_password, approved, confirmed_at,
  confirmation_token, confirmation_sent_at, created_at, updated_at
) ON TABLE public.users TO :"writer_role";

GRANT DELETE ON TABLE public.accounts TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE
  public.account_conversations, public.account_deletion_requests,
  public.account_stats, public.accounts_tags, public.blocks, public.bookmarks,
  public.conversations, public.conversation_mutes, public.domain_blocks,
  public.favourites, public.follows, public.follow_requests,
  public.keypairs, public.media_attachments, public.mutes,
  public.notification_policies, public.notifications,
  public.notification_requests, public.oauth_access_grants,
  public.oauth_access_tokens, public.reports, public.session_activations,
  public.status_pins, public.status_stats, public.statuses
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE ON TABLE
  public.account_relationship_severance_events,
  public.relationship_severance_events
  TO :"writer_role";
GRANT SELECT, INSERT ON TABLE
  public.account_warnings, public.admin_action_logs, public.collection_reports,
  public.severed_relationships
  TO :"writer_role";
GRANT SELECT, INSERT, DELETE ON TABLE
  public.canonical_email_blocks, public.login_activities,
  public.mentions, public.notification_permissions,
  public.oauth_applications, public.statuses_tags
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE ON TABLE public.markers TO :"writer_role";
GRANT SELECT, INSERT, UPDATE ON TABLE public.tags TO :"writer_role";
GRANT SELECT, INSERT ON TABLE public.status_edits TO :"writer_role";
GRANT SELECT, UPDATE, DELETE ON TABLE public.featured_tags TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.polls TO :"writer_role";
GRANT INSERT, DELETE ON TABLE public.poll_votes TO :"writer_role";
GRANT USAGE ON SEQUENCE public.polls_id_seq, public.poll_votes_id_seq TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.account_deletion_requests
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.account_stats
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.keypairs
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.statuses
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.conversations
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.follows
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.follow_requests
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.blocks
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.notification_policies
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.notifications
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.notification_requests
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.status_stats
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.media_attachments
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.account_conversations
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.conversation_mutes
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.bookmarks
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.favourites
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.status_pins
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.oauth_access_grants
  TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.oauth_access_tokens
  TO :"writer_role";
GRANT SELECT, DELETE ON TABLE
  public.account_aliases, public.account_domain_blocks,
  public.account_migrations, public.account_notes, public.account_pins,
  public.collection_items, public.collections, public.custom_filters,
  public.custom_emojis, public.invites, public.list_accounts, public.lists,
  public.poll_votes, public.report_notes, public.scheduled_statuses,
  public.tag_follows, public.generated_annual_reports,
  public.fasp_follow_recommendations, public.web_push_subscriptions
  TO :"writer_role";
GRANT SELECT, INSERT ON TABLE public.tombstones TO :"writer_role";
GRANT UPDATE (silent, updated_at) ON TABLE public.mentions TO :"writer_role";

-- Remote Note emoji ingestion and cache installation. INSERT initializes moderation
-- flags; UPDATE cannot change moderation, category, ID, shortcode, or domain.
-- Any granted UPDATE column also permits the ingestion SELECT ... FOR UPDATE lock.
GRANT INSERT (
  shortcode, domain, uri, image_remote_url, disabled, visible_in_picker,
  created_at, updated_at
) ON TABLE public.custom_emojis TO :"writer_role";
GRANT UPDATE (
  uri, image_remote_url, updated_at, image_content_type, image_file_name,
  image_file_size, image_storage_schema_version, image_updated_at
) ON TABLE public.custom_emojis TO :"writer_role";
GRANT USAGE ON SEQUENCE public.custom_emojis_id_seq TO :"writer_role";

GRANT USAGE, SELECT ON SEQUENCE
  public.account_conversations_id_seq, public.account_deletion_requests_id_seq,
  public.account_relationship_severance_events_id_seq,
  public.account_stats_id_seq, public.account_warnings_id_seq,
  public.accounts_id_seq, public.admin_action_logs_id_seq,
  public.blocks_id_seq, public.bookmarks_id_seq,
  public.canonical_email_blocks_id_seq, public.collection_reports_id_seq,
  public.conversations_id_seq, public.domain_blocks_id_seq,
  public.favourites_id_seq, public.follows_id_seq,
  public.follow_requests_id_seq, public.keypairs_id_seq,
  public.login_activities_id_seq, public.markers_id_seq,
  public.media_attachments_id_seq, public.mentions_id_seq, public.mutes_id_seq,
  public.notification_permissions_id_seq, public.notification_policies_id_seq,
  public.notification_requests_id_seq, public.notifications_id_seq,
  public.oauth_access_grants_id_seq, public.oauth_access_tokens_id_seq,
  public.oauth_applications_id_seq, public.reports_id_seq,
  public.relationship_severance_events_id_seq, public.severed_relationships_id_seq,
  public.session_activations_id_seq, public.status_edits_id_seq,
  public.status_pins_id_seq, public.status_stats_id_seq, public.statuses_id_seq,
  public.tags_id_seq, public.tombstones_id_seq, public.users_id_seq
  TO :"writer_role";

GRANT EXECUTE ON FUNCTION public.timestamp_id(text) TO :"writer_role";
GRANT EXECUTE ON FUNCTION public.rustodon_refresh_instances() TO :"writer_role";

GRANT USAGE, SELECT ON SEQUENCE rustodon.outbox_events_id_seq
  TO :"writer_role";
GRANT SELECT, DELETE ON TABLE rustodon.durable_jobs TO :"writer_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE
  rustodon.idempotency_keys, rustodon.outbox_events,
  rustodon.ordering_markers, rustodon.rate_limit_windows
  TO :"writer_role";

COMMIT;
