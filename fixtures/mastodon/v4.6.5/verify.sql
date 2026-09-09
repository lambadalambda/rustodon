\set ON_ERROR_STOP on

DO $$
DECLARE
  actual integer;
BEGIN
  SET LOCAL TIME ZONE 'UTC';

  IF current_database() <> 'rustodon_mastodon_v4_6_5_fixture' THEN
    RAISE EXCEPTION 'restored the fixture into an unexpected database: %', current_database();
  END IF;

  SELECT count(*) INTO actual FROM schema_migrations;
  IF actual <> 588 THEN
    RAISE EXCEPTION 'expected 588 applied migrations, found %', actual;
  END IF;

  IF (SELECT max(version) FROM schema_migrations) <> '20260611150940' THEN
    RAISE EXCEPTION 'schema version mismatch: %', (SELECT max(version) FROM schema_migrations);
  END IF;

  IF EXISTS (
    SELECT 1 FROM accounts
    WHERE domain IS NOT NULL
      AND domain NOT IN ('remote.fixture.invalid', 'account-blocked.fixture.invalid')
  ) THEN
    RAISE EXCEPTION 'fixture contains an account outside its dedicated domains';
  END IF;

  IF EXISTS (
    SELECT 1 FROM users
    WHERE email NOT LIKE '%@fixture.invalid'
  ) THEN
    RAISE EXCEPTION 'fixture contains a user outside its dedicated identities';
  END IF;

  IF (SELECT array_agg(DISTINCT visibility ORDER BY visibility) FROM statuses)
     <> ARRAY[0, 1, 2, 3, 4, 99] THEN
    RAISE EXCEPTION 'fixture does not cover known and unknown stored status visibility values';
  END IF;

  IF EXISTS (
    WITH snowflake_rows(table_name, id, created_at) AS (
      SELECT 'accounts', id, created_at FROM accounts
      UNION ALL SELECT 'statuses', id, created_at FROM statuses
      UNION ALL SELECT 'media_attachments', id, created_at FROM media_attachments
      UNION ALL SELECT 'quotes', id, created_at FROM quotes
      UNION ALL SELECT 'collections', id, created_at FROM collections
      UNION ALL SELECT 'collection_items', id, created_at FROM collection_items
      UNION ALL SELECT 'notification_requests', id, created_at FROM notification_requests
    )
    SELECT 1
    FROM snowflake_rows
    WHERE id >= 0
      AND (id >> 16) <> floor(extract(epoch FROM created_at) * 1000)::bigint
  ) OR (SELECT count(*) FROM accounts) <> 15
     OR (SELECT count(*) FROM statuses) <> 48
     OR (SELECT count(*) FROM media_attachments) <> 14
     OR (SELECT count(*) FROM quotes) <> 9
     OR (SELECT count(*) FROM collections) <> 1
     OR (SELECT count(*) FROM collection_items) <> 1
     OR (SELECT count(*) FROM notification_requests) <> 3 THEN
    RAISE EXCEPTION 'Snowflake ID timestamp prefixes or fixture row counts are incoherent';
  END IF;

  IF (SELECT last_value <> 6 OR NOT is_called FROM accounts_id_seq)
     OR (SELECT last_value <> 14 OR NOT is_called FROM statuses_id_seq)
     OR (SELECT last_value <> 1 OR NOT is_called FROM media_attachments_id_seq)
     OR (SELECT last_value <> 2 OR NOT is_called FROM quotes_id_seq)
     OR (SELECT last_value <> 1 OR NOT is_called FROM collections_id_seq)
     OR (SELECT last_value <> 1 OR NOT is_called FROM collection_items_id_seq)
     OR (SELECT last_value <> 1 OR NOT is_called FROM notification_requests_id_seq) THEN
    RAISE EXCEPTION 'timestamp_id backing sequence state does not match invocation counts';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM pg_sequences
    WHERE schemaname = 'public'
      AND sequencename = ANY(ARRAY[
        'accounts_id_seq', 'statuses_id_seq', 'media_attachments_id_seq',
        'quotes_id_seq', 'collections_id_seq', 'collection_items_id_seq',
        'notification_requests_id_seq'
      ])
      AND last_value >= 65536
  ) THEN
    RAISE EXCEPTION 'timestamp_id backing sequence state is unsafe for 16-bit tails';
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM user_roles
    WHERE id = 92
      AND permissions & 16 = 16
      AND permissions & 1024 = 1024
      AND permissions & 1048576 = 1048576
  ) OR EXISTS (
    SELECT 1
    FROM notifications n
    JOIN users u ON u.account_id = n.account_id
    JOIN user_roles r ON r.id = u.role_id
    WHERE (n.type = 'admin.report' AND r.permissions & 16 <> 16)
       OR (n.type = 'admin.sign_up' AND r.permissions & 1024 <> 1024)
  ) THEN
    RAISE EXCEPTION 'admin notification recipient lacks the required v4.6.5 role permission';
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM accounts
    WHERE id = 116844606259201001
      AND avatar_file_name = '0112603425bb49c1.png'
      AND avatar_content_type = 'image/png'
      AND avatar_file_size > 0
      AND avatar_storage_schema_version = 1
  ) OR NOT EXISTS (
    SELECT 1 FROM media_attachments
    WHERE id = 116844842188806001
      AND processing = 2
      AND blurhash <> ''
      AND file_file_name = 'cd63911ad76f4d5d.jpg'
      AND file_content_type = 'image/jpeg'
      AND file_file_size > 0
      AND file_storage_schema_version = 1
      AND (file_meta->'original'->>'width')::integer = 600
      AND (file_meta->'original'->>'height')::integer = 400
      AND (file_meta->'small'->>'width')::integer = 588
      AND (file_meta->'small'->>'height')::integer = 392
  ) THEN
    RAISE EXCEPTION 'processed media database state or style metadata is incoherent';
  END IF;

  IF (SELECT count(*) FROM oauth_access_tokens WHERE id BETWEEN 401 AND 409) <> 9
     OR NOT EXISTS (
    SELECT 1 FROM oauth_access_tokens
    WHERE id = 401
      AND token = 'fixture-bearer-token-v4-6-5'
      AND revoked_at IS NULL
      AND application_id = 301
      AND resource_owner_id = 101
  ) OR NOT EXISTS (
    SELECT 1 FROM oauth_access_tokens
    WHERE id = 402 AND token = 'fixture-bearer-read-statuses-v4-6-5'
      AND application_id = 301 AND resource_owner_id = 101
      AND scopes = 'read:statuses' AND revoked_at IS NULL
  ) OR NOT EXISTS (
    SELECT 1 FROM oauth_access_tokens
    WHERE id = 403 AND token = 'fixture-bearer-read-accounts-v4-6-5'
      AND application_id = 301 AND resource_owner_id = 101
      AND scopes = 'read:accounts' AND revoked_at IS NULL
  ) OR NOT EXISTS (
    SELECT 1 FROM oauth_access_tokens
    WHERE id = 404 AND token = 'fixture-bearer-insufficient-v4-6-5'
      AND application_id = 301 AND resource_owner_id = 101
      AND scopes = 'push' AND revoked_at IS NULL
  ) OR NOT EXISTS (
    SELECT 1 FROM oauth_access_tokens
    WHERE id = 405 AND token = 'fixture-bearer-revoked-v4-6-5'
      AND application_id = 301 AND resource_owner_id = 101
      AND revoked_at = TIMESTAMP '2026-07-01 12:45:00'
  ) OR NOT EXISTS (
    SELECT 1 FROM oauth_access_tokens
    WHERE id = 406 AND token = 'fixture-bearer-expired-v4-6-5'
      AND application_id = 301 AND resource_owner_id = 101
      AND expires_in = 60 AND created_at = TIMESTAMP '2000-01-01 00:00:00'
  ) OR NOT EXISTS (
    SELECT 1 FROM oauth_access_tokens
    WHERE id = 407 AND token = 'fixture-bearer-application-only-v4-6-5'
      AND resource_owner_id IS NULL AND application_id = 301
  ) OR NOT EXISTS (
    SELECT 1 FROM oauth_access_tokens
    WHERE id = 408 AND token = 'fixture-bearer-disabled-user-v4-6-5'
      AND application_id = 301 AND resource_owner_id = 103
  ) OR NOT EXISTS (
    SELECT 1 FROM oauth_access_tokens
    WHERE id = 409 AND token = 'fixture-bearer-missing-2fa-v4-6-5'
      AND application_id = 301 AND resource_owner_id = 102
  ) THEN
    RAISE EXCEPTION 'fixture OAuth authentication matrix is missing or incoherent';
  END IF;

  IF (SELECT count(*) FROM lists WHERE account_id = 116844606259201001) <> 3
     OR NOT EXISTS (SELECT 1 FROM lists WHERE id = 9001 AND exclusive = false)
     OR NOT EXISTS (SELECT 1 FROM lists WHERE id = 9002 AND exclusive = true)
     OR NOT EXISTS (SELECT 1 FROM lists WHERE id = 9005 AND replies_policy = 2)
     OR NOT EXISTS (SELECT 1 FROM tag_follows WHERE id = 9206)
     OR NOT EXISTS (SELECT 1 FROM follows WHERE id = 8012 AND show_reblogs = false) THEN
    RAISE EXCEPTION 'normal/exclusive list coverage is incomplete';
  END IF;

  IF NOT EXISTS (
    SELECT 1
    FROM custom_filters f
    JOIN custom_filter_keywords k ON k.custom_filter_id = f.id
    JOIN custom_filter_statuses s ON s.custom_filter_id = f.id
    WHERE f.id = 9101 AND k.id = 9102 AND s.status_id = 116845105643525105
  ) THEN
    RAISE EXCEPTION 'custom filter cross-links are incomplete';
  END IF;

  IF NOT EXISTS (
    SELECT 1
    FROM accounts a
    LEFT JOIN users u ON u.account_id = a.id
    WHERE a.id = -99 AND a.domain IS NULL AND a.actor_type = 'Application'
      AND a.attribution_domains IS NULL AND a.private_key <> '' AND a.public_key <> ''
      AND u.id IS NULL
  ) OR NOT EXISTS (
    SELECT 1 FROM users
    WHERE id = 101
      AND settings = '{"default_privacy":"private","nested":{"number":9007199254740993}}'
      AND chosen_languages = ARRAY['en']::varchar[]
      AND otp_backup_codes = ARRAY['fixture-recovery-code']::varchar[]
      AND sign_up_ip = '192.0.2.0/24'::inet
      AND otp_required_for_login = true
      AND webauthn_id = 'fixture-alice-webauthn-id'
      AND EXISTS (SELECT 1 FROM webauthn_credentials WHERE user_id = users.id)
  ) OR NOT EXISTS (
    SELECT 1 FROM users
    WHERE id = 102 AND settings IS NULL
      AND chosen_languages = ARRAY[]::varchar[]
      AND otp_backup_codes = ARRAY[]::varchar[]
      AND otp_required_for_login = false
      AND NOT EXISTS (SELECT 1 FROM webauthn_credentials WHERE user_id = users.id)
  ) OR NOT EXISTS (
    SELECT 1 FROM users
    WHERE id = 103 AND settings = '' AND chosen_languages = ARRAY['en', 'fr']::varchar[]
      AND disabled = true
      AND otp_backup_codes IS NULL
  ) OR NOT EXISTS (
    SELECT 1 FROM users
    WHERE id = 105 AND account_id = -321 AND confirmed_at IS NOT NULL AND approved = false
  ) OR NOT EXISTS (
    SELECT 1 FROM users
    WHERE id = 106 AND account_id = -322 AND confirmed_at IS NULL AND approved = true
  ) OR NOT EXISTS (
    SELECT 1 FROM users
    WHERE id = 107 AND account_id = -323 AND confirmed_at IS NOT NULL
      AND approved = true AND disabled = false AND otp_required_for_login = true
  ) OR NOT EXISTS (
    SELECT 1 FROM accounts
    WHERE id = 116844606259202001
      AND also_known_as = ARRAY['https://alias.remote.fixture.invalid/users/bob']::varchar[]
      AND attribution_domains = ARRAY['media.remote.fixture.invalid']::varchar[]
      AND fields->0->>'value' = 'Exact JSONB value'
      AND id_scheme = 1
  ) OR NOT EXISTS (
    SELECT 1 FROM accounts
    WHERE id = 116844606259201001 AND id_scheme = 0
  ) OR NOT EXISTS (
    SELECT 1 FROM accounts
    WHERE id = 116844606259202003 AND domain = 'remote.fixture.invalid'
      AND suspended_at = '2026-07-01 18:30:00'
  ) OR NOT EXISTS (
    SELECT 1 FROM user_roles
    WHERE id = -99 AND permissions = 1152921504606912512
  ) THEN
    RAISE EXCEPTION 'local account classification or raw user settings/array coverage is incomplete';
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM statuses s
    JOIN status_stats stats ON stats.status_id = s.id
    WHERE s.id = 116846257766400501 AND s.visibility = 99 AND s.deleted_at IS NOT NULL
      AND s.ordered_media_attachment_ids = ARRAY[]::bigint[]
      AND s.poll_id = 8203
  ) OR NOT EXISTS (
    SELECT 1 FROM statuses
    WHERE id = 116844842188805001 AND application_id = 301 AND quote_approval_policy = 2
      AND ordered_media_attachment_ids = ARRAY[-101, 116844842188806001, -102, -103, -104]::bigint[]
  ) OR NOT EXISTS (
    SELECT 1 FROM statuses
    WHERE id = 116844853985285004 AND reply = true
      AND in_reply_to_account_id = 116844606259202001
      AND in_reply_to_id = 116845078118405101
  ) OR NOT EXISTS (
    SELECT 1 FROM statuses
    WHERE id = 116845314048005201 AND ordered_media_attachment_ids = ARRAY[]::bigint[]
      AND EXISTS (SELECT 1 FROM media_attachments WHERE id = -106 AND status_id = statuses.id)
  ) OR NOT EXISTS (
    SELECT 1 FROM status_edits
    WHERE id = 9401 AND ordered_media_attachment_ids = ARRAY[]::bigint[]
      AND media_descriptions = ARRAY[]::text[] AND poll_options IS NULL
  ) OR NOT EXISTS (
    SELECT 1 FROM status_edits
    WHERE id = 9403
      AND ordered_media_attachment_ids = ARRAY[-101, 116844842188806001]::bigint[]
      AND media_descriptions = ARRAY[NULL, 'Deterministic Mastodon test attachment']::text[]
  ) OR NOT EXISTS (
    SELECT 1 FROM tags t
    JOIN statuses_tags st ON st.tag_id = t.id
    WHERE t.id = 9201 AND st.status_id = 116844842188805001
  ) OR NOT EXISTS (
    SELECT 1 FROM accounts_tags WHERE account_id = 116844606259201001 AND tag_id = 9201
  ) OR NOT EXISTS (
    SELECT 1 FROM featured_tags
    WHERE id = 9202 AND account_id = 116844606259201001 AND tag_id = 9201
  ) OR NOT EXISTS (
    SELECT 1 FROM conversations c
    JOIN account_conversations ac ON ac.conversation_id = c.id
    WHERE c.id = 9301 AND ac.id = 9302
      AND ac.participant_account_ids = ARRAY[116844606259202001]::bigint[]
      AND ac.status_ids = ARRAY[116844853985285004, 116846257766400501]::bigint[]
  ) THEN
    RAISE EXCEPTION 'status edge, edit, tag, or conversation fixtures are incomplete';
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM statuses
    WHERE id = 116844846120965002 AND in_reply_to_id = 116844842188805001
      AND in_reply_to_account_id = 116844606259201001
  ) OR NOT EXISTS (
    SELECT 1 FROM statuses
    WHERE id = 116844850053125003 AND in_reply_to_id = 116844846120965002
      AND in_reply_to_account_id = 116844606259201001
  ) THEN
    RAISE EXCEPTION 'status context fixture chain is incomplete';
  END IF;

  IF NOT EXISTS (SELECT 1 FROM mentions WHERE id = 7004 AND silent = true)
     OR NOT EXISTS (SELECT 1 FROM mentions WHERE id = 7005 AND silent = true)
     OR NOT EXISTS (SELECT 1 FROM mentions WHERE id = 7006 AND silent = false)
     OR NOT EXISTS (SELECT 1 FROM follows WHERE id = 8007)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -310 AND account_id = 116844606259202003)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -311 AND visibility = 2)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -312 AND visibility = 2)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -313 AND visibility = 0
       AND in_reply_to_id = 116844842188805001)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -314 AND account_id = -320)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -315
       AND account_id = 116844606259201004 AND in_reply_to_id = 116844842188805001)
     OR NOT EXISTS (SELECT 1 FROM accounts WHERE id = 116844606259201004
       AND silenced_at = '2026-07-01 18:25:00')
     OR NOT EXISTS (SELECT 1 FROM accounts WHERE id = -320 AND domain = 'account-blocked.fixture.invalid')
     OR NOT EXISTS (SELECT 1 FROM blocks WHERE id = 9509) THEN
    RAISE EXCEPTION 'status authorization matrix fixtures are incomplete';
  END IF;

  IF NOT EXISTS (SELECT 1 FROM bookmarks WHERE id = 9501)
     OR NOT EXISTS (SELECT 1 FROM blocks WHERE id = 9502)
     OR NOT EXISTS (SELECT 1 FROM mutes WHERE id = 9503 AND hide_notifications = false)
     OR NOT EXISTS (SELECT 1 FROM account_domain_blocks WHERE id = 9504)
     OR NOT EXISTS (SELECT 1 FROM domain_allows WHERE id = 9601)
     OR NOT EXISTS (SELECT 1 FROM domain_blocks WHERE id = 9602 AND severity = 99)
     OR NOT EXISTS (SELECT 1 FROM notification_policies WHERE id = 9701 AND for_not_following = 99)
     OR NOT EXISTS (SELECT 1 FROM notification_permissions WHERE id = 9702)
     OR NOT EXISTS (SELECT 1 FROM notification_requests WHERE id = 116846261698560601)
     OR NOT EXISTS (SELECT 1 FROM notification_requests WHERE id = -96 AND last_status_id = 116846257766400501)
     OR NOT EXISTS (SELECT 1 FROM notification_requests WHERE id = -95 AND from_account_id = 116844606259202003)
     OR NOT EXISTS (SELECT 1 FROM tombstones WHERE id = 9901)
     OR NOT EXISTS (SELECT 1 FROM conversation_mutes WHERE id = 9303)
     OR NOT EXISTS (SELECT 1 FROM status_pins WHERE id = 9505)
     OR NOT EXISTS (SELECT 1 FROM status_pins WHERE id = 9506)
     OR NOT EXISTS (SELECT 1 FROM status_pins WHERE id = 9508)
     OR NOT EXISTS (SELECT 1 FROM favourites WHERE id = 8102)
     OR NOT EXISTS (SELECT 1 FROM bookmarks WHERE id = 9507)
     OR NOT EXISTS (SELECT 1 FROM custom_filter_statuses WHERE id = 9104)
     OR NOT EXISTS (SELECT 1 FROM custom_emojis WHERE id = 12001 AND shortcode = 'fixtureparty')
      OR NOT EXISTS (SELECT 1 FROM preview_cards WHERE id = 12002 AND type = 3)
      OR NOT EXISTS (SELECT 1 FROM preview_cards_statuses WHERE preview_card_id = 12002 AND status_id = 116845105643525105)
      OR NOT EXISTS (SELECT 1 FROM tagged_objects WHERE id = 12003 AND object_id = 116845549977608801)
      OR NOT EXISTS (SELECT 1 FROM markers WHERE id = 12004 AND user_id = 101 AND lock_version = 7)
      OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = 116844846120965002 AND local IS NULL AND uri IS NULL)
      OR NOT EXISTS (SELECT 1 FROM notifications WHERE id = 10025 AND type IS NULL AND activity_type = 'Status') THEN
    RAISE EXCEPTION 'relationship, domain, notification policy, request, or tombstone coverage is incomplete';
  END IF;

  IF NOT EXISTS (SELECT 1 FROM statuses WHERE id = -400 AND account_id = -330
       AND visibility = 0 AND language = 'en' AND reply = false AND reblog_of_id IS NULL)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -401 AND account_id = -330
       AND visibility = 0 AND language = 'fr')
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -404 AND reply = true
       AND in_reply_to_account_id = account_id)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -408 AND reply = true
       AND in_reply_to_account_id = 116844606259202002)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -409 AND reblog_of_id = -415)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -414 AND account_id = -323
       AND visibility = 0 AND local = true)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -416
       AND account_id = 116844606259201001 AND reblog_of_id = -417)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -417 AND account_id = -323)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -418 AND EXISTS (
       SELECT 1 FROM mentions WHERE status_id = -418 AND account_id = 116844606259202002))
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -420 AND EXISTS (
       SELECT 1 FROM custom_filter_statuses WHERE status_id = -420 AND custom_filter_id = 9101))
     OR NOT EXISTS (SELECT 1 FROM custom_filter_statuses
       WHERE id = 9106 AND status_id = -416 AND custom_filter_id = 9101)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -421 AND account_id = 116844606259201001
       AND visibility = 3)
     OR NOT EXISTS (SELECT 1 FROM statuses WHERE id = -423 AND language = 'fr' AND EXISTS (
       SELECT 1 FROM statuses_tags WHERE status_id = -423 AND tag_id = 9201))
     OR NOT EXISTS (SELECT 1 FROM follows WHERE id = 8010 AND languages = ARRAY['en']::varchar[])
     OR NOT EXISTS (SELECT 1 FROM follows WHERE id = 8012 AND show_reblogs = false)
     OR NOT EXISTS (SELECT 1 FROM list_accounts WHERE id = 9013 AND list_id = 9002 AND follow_id = 8011)
     OR NOT EXISTS (SELECT 1 FROM list_accounts WHERE id = 9015 AND list_id = 9001
       AND account_id = 116844606259201001 AND follow_id IS NULL)
     OR NOT EXISTS (SELECT 1 FROM favourites WHERE id = 8111 AND status_id = -412)
     OR NOT EXISTS (SELECT 1 FROM bookmarks WHERE id = 9511 AND status_id = -412)
     OR NOT EXISTS (SELECT 1 FROM statuses_tags WHERE status_id = 116845314048005201 AND tag_id = 9201)
     OR NOT EXISTS (SELECT 1 FROM oauth_access_tokens WHERE id = 412 AND scopes = 'read:lists')
     OR NOT EXISTS (SELECT 1 FROM oauth_access_tokens WHERE id = 416 AND scopes = 'read:mutes')
      OR NOT EXISTS (SELECT 1 FROM oauth_access_tokens
        WHERE id = 417 AND scopes = 'follow write:blocks write:mutes') THEN
    RAISE EXCEPTION 'timeline and collection compatibility controls are incomplete';
  END IF;

  IF NOT EXISTS (SELECT 1 FROM quotes WHERE id = -94 AND legacy = true AND state = 4)
     OR NOT EXISTS (
       SELECT 1 FROM quotes q JOIN statuses target ON target.id = q.quoted_status_id
       WHERE q.id = -93 AND target.reblog_of_id IS NOT NULL AND q.state = 1
     )
     OR jsonb_array_length((SELECT fields FROM accounts WHERE id = 116844606259201001)) <> 3 THEN
    RAISE EXCEPTION 'legacy quote and local profile mention coverage is incomplete';
  END IF;

  IF (SELECT value FROM settings WHERE id = 9801) <> E'--- true\n'
     OR (SELECT value FROM settings WHERE id = 9802)
        <> E'--- !ruby/hash:ActiveSupport::HashWithIndifferentAccess\nfixture: value\n'
     OR (SELECT value FROM settings WHERE id = 9803) <> E'--- public\n'
     OR (SELECT value FROM settings WHERE id = 9804) <> E'--- authenticated\n'
     OR (SELECT value FROM settings WHERE id = 9805) <> E'--- public\n'
     OR (SELECT value FROM settings WHERE id = 9806) <> E'--- disabled\n' THEN
    RAISE EXCEPTION 'raw scalar/tagged Rails YAML setting bytes are incorrect';
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM users u JOIN accounts a ON a.id = u.account_id
    WHERE u.id = 104 AND a.id = 116844606259201004
      AND u.otp_required_for_login = true AND u.disabled = false
  ) OR NOT EXISTS (
    SELECT 1 FROM oauth_access_tokens
    WHERE id = 410 AND resource_owner_id = 104 AND revoked_at IS NULL
  ) OR (SELECT count(*) FROM notifications WHERE account_id = 116844606259201004) <> 44
     OR (SELECT count(*) FROM notifications WHERE group_key = 'follow-api-moderator-stress') <> 41 THEN
    RAISE EXCEPTION 'functional API moderator notification coverage is incomplete';
  END IF;

  IF NOT EXISTS (
    SELECT 1
    FROM polls p
    JOIN poll_votes v ON v.poll_id = p.id
    JOIN statuses s ON s.poll_id = p.id AND p.status_id = s.id
    WHERE p.id = 8201
      AND p.expires_at = '2024-01-02 12:00:00'
      AND p.cached_tallies = ARRAY[1, 0]::bigint[]
      AND p.votes_count = 1
      AND p.voters_count = 1
      AND v.account_id = 116844606259201001
  ) OR NOT EXISTS (
    SELECT 1 FROM polls p JOIN poll_votes v ON v.poll_id = p.id
    WHERE p.id = 8203 AND p.status_id = 116846257766400501 AND v.id = 8204
  ) THEN
    RAISE EXCEPTION 'historical poll/vote is missing or incoherent';
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM accounts
    WHERE id = 116844606259201001 AND private_key IS NOT NULL AND public_key <> ''
  ) OR EXISTS (
    SELECT 1 FROM accounts WHERE domain IS NOT NULL AND private_key IS NOT NULL
  ) OR NOT EXISTS (
    SELECT 1 FROM keypairs
    WHERE id = 8901 AND account_id = 116844606259202001 AND private_key IS NULL AND public_key <> ''
  ) OR NOT EXISTS (
    SELECT 1 FROM keypairs
    WHERE id = 8902 AND account_id = 116844606259201001 AND type = 0
      AND private_key LIKE '{"p":%' AND revoked = true
  ) THEN
    RAISE EXCEPTION 'Mastodon 4.6.5 signing-key contract is not satisfied';
  END IF;

  IF NOT EXISTS (
    SELECT 1
    FROM collections c
    JOIN collection_items i ON i.collection_id = c.id
    WHERE c.id = 116845549977608801 AND c.local = false AND c.item_count = 1
      AND i.id = 116845549977608802 AND i.account_id = 116844606259201001 AND i.state = 1
  ) THEN
    RAISE EXCEPTION 'collection/item cross-links or counters are incoherent';
  END IF;

  IF (SELECT count(*) FROM quotes WHERE state = 1) <> 6
     OR NOT EXISTS (
       SELECT 1 FROM quotes
       WHERE id = 116845078118408703 AND status_id = 116845078118405101
         AND quoted_status_id = 116844850053125003 AND state = 1
     )
     OR NOT EXISTS (
       SELECT 1 FROM quotes
       WHERE id = 116845317980168701 AND account_id = 116844606259202001 AND status_id = 116845317980165202
         AND quoted_account_id = 116844606259201001 AND quoted_status_id = 116844842188805001
     )
     OR NOT EXISTS (
       SELECT 1 FROM quotes
       WHERE id = 116845314048008702 AND account_id = 116844606259201001 AND status_id = 116845314048005201
         AND quoted_account_id = 116844606259202001 AND quoted_status_id = 116845093847045103
     )
     OR NOT EXISTS (
       SELECT 1 FROM quotes
       WHERE id = -94 AND status_id = 116844846120965002
         AND quoted_status_id = 116846257766400501 AND state = 4
     )
     OR NOT EXISTS (
       SELECT 1 FROM quotes
       WHERE id = -92 AND status_id = 116845105643525105
         AND quoted_status_id = 116844850053125003 AND state = 0
     )
     OR NOT EXISTS (
       SELECT 1 FROM quotes
       WHERE id = -91 AND status_id = 116844842188805001
         AND quoted_status_id = 116845321912325301 AND state = 1
     )
     OR NOT EXISTS (
       SELECT 1 FROM quotes
       WHERE id = -90 AND status_id = quoted_status_id AND state = 1
     ) THEN
    RAISE EXCEPTION 'quote directions or cross-links are incoherent';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM account_stats stats
    WHERE stats.statuses_count <> (
      SELECT count(*)
      FROM statuses status
      WHERE status.account_id = stats.account_id
        AND status.deleted_at IS NULL
        AND status.visibility <> 3
    )
  ) THEN
    RAISE EXCEPTION 'account status counters are incoherent';
  END IF;

  IF (SELECT reblogs_count FROM status_stats WHERE status_id = 116844842188805001) <> 1
     OR (SELECT favourites_count FROM status_stats WHERE status_id = 116844842188805001) <> 1
     OR (SELECT quotes_count FROM status_stats WHERE status_id = 116844842188805001) <> 1
     OR (SELECT quotes_count FROM status_stats WHERE status_id = 116845093847045103) <> 1 THEN
    RAISE EXCEPTION 'status counters are incoherent';
  END IF;

  IF (SELECT count(*) FROM notifications WHERE filtered = false) <> 63
     OR (SELECT count(*) FROM notifications) <> 67
     OR NOT EXISTS (
       SELECT 1 FROM notifications
       WHERE id = 10018 AND type = 'future_event' AND activity_type = 'FutureActivity' AND filtered = true
     ) OR NOT EXISTS (
       SELECT 1 FROM notifications
       WHERE id = 10019 AND type IS NULL AND activity_type = 'FutureActivity' AND filtered = true
     ) OR NOT EXISTS (
       SELECT 1 FROM notifications
       WHERE id = 10020 AND type = 'future_deleted_status'
         AND activity_type = 'Status' AND filtered = true
     ) OR NOT EXISTS (
       SELECT 1 FROM notifications
       WHERE id = 10021 AND type = 'future_suspended'
         AND from_account_id = 116844606259202003 AND filtered = true
     ) THEN
    RAISE EXCEPTION 'expected 63 readable known/legacy/stress notifications and four filtered edge fixtures';
  END IF;

  IF EXISTS (
    WITH expected(type, group_key) AS (
      VALUES
        ('favourite', 'favourite-116844842188805001-495255'),
        ('reblog', 'reblog-116844842188805001-495255'),
        ('follow', 'follow-495252'),
        ('admin.sign_up', 'admin.sign_up-495252')
    )
    SELECT 1
    FROM expected
    LEFT JOIN notifications n ON n.type = expected.type AND n.id <= 10017
    WHERE n.group_key IS DISTINCT FROM expected.group_key
  ) THEN
    RAISE EXCEPTION 'groupable notification key does not match v4.6.5 target/hour format';
  END IF;

  IF EXISTS (
    WITH expected(type, activity_type, activity_id, account_id, from_account_id) AS (
      VALUES
        ('mention', 'Mention', 7001::bigint, 116844606259201001::bigint, 116844606259202001::bigint),
        ('status', 'Status', 116845101711365104, 116844606259201001, 116844606259202001),
        ('reblog', 'Status', 116845321912325301, 116844606259201001, 116844606259202001),
        ('follow', 'Follow', 8002, 116844606259201001, 116844606259202001),
        ('follow_request', 'FollowRequest', 8003, 116844606259201001, 116844606259202002),
        ('favourite', 'Favourite', 8101, 116844606259201001, 116844606259202001),
        ('poll', 'Poll', 8201, 116844606259201001, 116844606259202001),
        ('update', 'Status', 116845105643525105, 116844606259201001, 116844606259202001),
        ('severed_relationships', 'AccountRelationshipSeveranceEvent', 8302, 116844606259201001, 116844606259201001),
        ('moderation_warning', 'AccountWarning', 8401, 116844606259201001, 116844606259201001),
        ('annual_report', 'GeneratedAnnualReport', 8501, 116844606259201001, 116844606259201001),
        ('admin.sign_up', 'Account', 116844606259201003, 116844606259201002, 116844606259201003),
        ('admin.report', 'Report', 8601, 116844606259201002, 116844606259201001),
        ('quote', 'Quote', 116845317980168701, 116844606259201001, 116844606259202001),
        ('quoted_update', 'Status', 116845314048005201, 116844606259201001, 116844606259202001),
        ('added_to_collection', 'CollectionItem', 116845549977608802, 116844606259201001, 116844606259202001),
        ('collection_update', 'Collection', 116845549977608801, 116844606259201001, 116844606259202001)
    )
    SELECT 1
    FROM expected
    LEFT JOIN notifications n ON n.type = expected.type AND n.id <= 10017
    WHERE n.id IS NULL
       OR n.activity_type <> expected.activity_type
       OR n.activity_id <> expected.activity_id
       OR n.account_id <> expected.account_id
       OR n.from_account_id <> expected.from_account_id
  ) THEN
    RAISE EXCEPTION 'notification activity association/from-account contract is incoherent';
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM account_migrations
    WHERE id = 8700 AND account_id = 116844606259201001
      AND acct = 'deleted@remote.fixture.invalid'
      AND followers_count = 3 AND target_account_id IS NULL
  ) OR NOT EXISTS (
    SELECT 1 FROM reports
    WHERE id = 8601 AND account_id = 116844606259201001
      AND target_account_id = 116844606259202001
      AND comment = 'Readable fixture report'
      AND updated_at = TIMESTAMP '2026-07-01 17:15:00'
  ) OR NOT EXISTS (
    SELECT 1 FROM account_warnings
    WHERE id = 8401 AND account_id = 116844606259201002
      AND target_account_id = 116844606259201001
      AND text = 'Readable fixture moderation warning'
  ) OR NOT EXISTS (
    SELECT 1 FROM relationship_severance_events
    WHERE id = 8301 AND target_name = 'blocked.fixture.invalid' AND purged = false
  ) OR NOT EXISTS (
    SELECT 1 FROM severed_relationships
    WHERE id = 8303 AND relationship_severance_event_id = 8301
      AND local_account_id = 116844606259201001
      AND remote_account_id = 116844606259202002
  ) OR NOT EXISTS (
    SELECT 1 FROM account_relationship_severance_events
    WHERE id = 8302 AND account_id = 116844606259201001
      AND relationship_severance_event_id = 8301
  ) OR NOT EXISTS (
    SELECT 1 FROM announcements
    WHERE id = 8701 AND published AND text = 'Preserved fixture announcement'
      AND status_ids = ARRAY[116844842188805001]::bigint[]
      AND notification_sent_at = TIMESTAMP '2026-07-01 17:37:00'
      AND updated_at = TIMESTAMP '2026-07-01 17:37:00'
  ) OR NOT EXISTS (
    SELECT 1 FROM appeals
    WHERE id = 8702 AND account_id = 116844606259201001
      AND account_warning_id = 8402 AND text = 'Preserved fixture appeal'
      AND approved_at IS NULL AND rejected_at IS NULL
  ) OR NOT EXISTS (
    SELECT 1 FROM backups
    WHERE id = 8703 AND user_id = 101 AND processed = false
      AND dump_content_type IS NULL AND dump_file_name IS NULL
      AND dump_file_size IS NULL AND dump_updated_at IS NULL
  ) OR NOT EXISTS (
    SELECT 1 FROM bulk_imports
    WHERE id = 8704 AND account_id = 116844606259201001
      AND type = 0 AND state = 3 AND original_filename = 'following.csv'
      AND total_items = 1 AND imported_items = 0 AND processed_items = 1
  ) OR NOT EXISTS (
    SELECT 1 FROM bulk_import_rows
    WHERE id = 8705 AND bulk_import_id = 8704
      AND data = '{"acct":"missing@remote.fixture.invalid"}'::jsonb
  ) OR NOT EXISTS (
    SELECT 1 FROM report_notes
    WHERE id = 8602 AND account_id = 116844606259201002
      AND report_id = 8601 AND content = 'Preserved fixture report note'
  ) OR NOT EXISTS (
    SELECT 1 FROM web_push_subscriptions
    WHERE id = 8706 AND access_token_id = 401 AND user_id = 101
      AND standard AND endpoint = 'https://fcm.googleapis.com/fcm/send/fixture-alice'
      AND key_auth = 'eH_C8rq2raXqlcBVDa1gLg=='
      AND key_p256dh = 'BEm_a0bdPDhf0SOsrnB2-ategf1hHoCnpXgQsFj5JCkcoMrMt2WHoPfEYOYPzOIs9mZE8ZUaD7VA5vouy0kEkr8='
      AND data->>'policy' = 'all'
  ) THEN
    RAISE EXCEPTION 'Mastodon-owned preservation rows are incomplete';
  END IF;
END
$$;

SELECT 'fixture SQL verification passed' AS result;
