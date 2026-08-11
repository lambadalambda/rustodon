use std::time::Duration;

use sqlx::postgres::{PgPool, PgPoolOptions};

use super::records::{
    Account, AccountConversation, AccountDomainBlock, AccountRelationshipSeveranceEvent,
    AccountStat, AccountTag, AccountWarning, Block, Bookmark, Collection, CollectionItem,
    Conversation, ConversationMute, CustomFilter, CustomFilterKeyword, CustomFilterStatus,
    DomainAllow, DomainBlock, Favourite, FeaturedTag, Follow, FollowRequest, GeneratedAnnualReport,
    Keypair, List, ListAccount, MediaAttachment, Mention, Mute, Notification,
    NotificationPermission, NotificationPolicy, NotificationRequest, OAuthAccessToken,
    OAuthApplication, Poll, PollVote, Quote, RelationshipSeveranceEvent, Report, Setting, Status,
    StatusEdit, StatusPin, StatusStat, StatusTag, Tag, Tombstone, User, UserRole,
};

#[derive(Clone)]
pub struct Repository {
    pool: PgPool,
}

#[allow(clippy::missing_errors_doc)]
impl Repository {
    pub async fn connect(database_url: &str) -> sqlx::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    sqlx::query("SET TIME ZONE 'UTC'")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET search_path TO pg_catalog, public, pg_temp")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET default_transaction_read_only = on")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub async fn account(&self, id: i64) -> sqlx::Result<Option<Account>> {
        sqlx::query_as::<_, Account>(
            "SELECT a.id, a.username, a.domain, a.actor_type, a.display_name, a.note, \
             a.uri, a.url, a.also_known_as, a.attribution_domains, a.fields, \
             a.avatar_content_type, a.avatar_description, a.avatar_file_name, a.avatar_file_size, \
             a.avatar_remote_url, a.avatar_storage_schema_version, a.avatar_updated_at, \
             a.collections_url, a.discoverable, a.feature_approval_policy, a.featured_collection_url, \
              a.followers_url, a.following_url, a.header_content_type, a.header_description, \
              a.header_file_name, a.header_file_size, a.header_remote_url, \
              a.header_storage_schema_version, a.header_updated_at, a.hide_collections, a.id_scheme, a.inbox_url, \
              a.indexable, a.locked, a.memorial, a.moved_to_account_id, a.outbox_url, a.protocol, \
             a.public_key, a.private_key, a.sensitized_at, a.shared_inbox_url, a.show_featured, \
             a.show_media, a.show_media_replies, a.silenced_at, a.suspended_at, a.suspension_origin, \
             a.trendable, a.created_at, a.updated_at, \
             EXISTS (SELECT 1 FROM users u WHERE u.account_id = a.id) AS has_user, \
              EXISTS (SELECT 1 FROM users u \
                      JOIN user_roles role ON role.id = COALESCE(u.role_id, -99) \
                      WHERE u.account_id = a.id AND u.approved = true AND u.disabled = false \
                        AND u.confirmed_at IS NOT NULL \
                        AND (role.require_2fa = false OR u.otp_required_for_login = true \
                          OR EXISTS (SELECT 1 FROM webauthn_credentials credential \
                                     WHERE credential.user_id = u.id))) \
                AND a.suspended_at IS NULL AND a.memorial = false \
                AND a.moved_to_account_id IS NULL AS login_capable_user \
             FROM accounts a WHERE a.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn account_stat(&self, account_id: i64) -> sqlx::Result<Option<AccountStat>> {
        sqlx::query_as::<_, AccountStat>(
            "SELECT id, account_id, statuses_count, following_count, followers_count, last_status_at \
             FROM account_stats WHERE account_id = $1",
        )
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn user(&self, id: i64) -> sqlx::Result<Option<User>> {
        sqlx::query_as::<_, User>(
            "SELECT id, account_id, email, encrypted_password, chosen_languages, \
              otp_backup_codes::text[] AS otp_backup_codes, otp_required_for_login, otp_secret, \
              settings, sign_up_ip, role_id, approved, disabled, confirmed_at, locale, webauthn_id, \
              EXISTS (SELECT 1 FROM webauthn_credentials credential \
                      WHERE credential.user_id = users.id) AS has_webauthn_credentials \
              FROM users WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn user_role(&self, id: i64) -> sqlx::Result<Option<UserRole>> {
        sqlx::query_as::<_, UserRole>(
            "SELECT id, name, color, position, permissions, highlighted, require_2fa, collection_limit \
             FROM user_roles WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn oauth_application(&self, id: i64) -> sqlx::Result<Option<OAuthApplication>> {
        sqlx::query_as::<_, OAuthApplication>(
            "SELECT id, name, uid, secret, redirect_uri, scopes, confidential, owner_id, owner_type, website \
             FROM oauth_applications WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn oauth_access_token(&self, token: &str) -> sqlx::Result<Option<OAuthAccessToken>> {
        sqlx::query_as::<_, OAuthAccessToken>(
            "SELECT id, resource_owner_id, application_id, token, refresh_token, scopes, expires_in, \
             created_at, revoked_at, last_used_at, last_used_ip \
             FROM oauth_access_tokens WHERE token = $1",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn status(&self, id: i64) -> sqlx::Result<Option<Status>> {
        sqlx::query_as::<_, Status>(
            "SELECT id, account_id, application_id, text, spoiler_text, visibility, local, uri, url, language, \
             sensitive, reply, ordered_media_attachment_ids, conversation_id, in_reply_to_id, \
             in_reply_to_account_id, reblog_of_id, poll_id, quote_approval_policy, \
             deleted_at, edited_at, created_at, updated_at \
             FROM statuses WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn status_including_deleted(&self, id: i64) -> sqlx::Result<Option<Status>> {
        sqlx::query_as::<_, Status>(
            "SELECT id, account_id, application_id, text, spoiler_text, visibility, local, uri, url, language, \
             sensitive, reply, ordered_media_attachment_ids, conversation_id, in_reply_to_id, \
             in_reply_to_account_id, reblog_of_id, poll_id, quote_approval_policy, \
             deleted_at, edited_at, created_at, updated_at \
             FROM statuses WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn status_stat(&self, status_id: i64) -> sqlx::Result<Option<StatusStat>> {
        sqlx::query_as::<_, StatusStat>(
            "SELECT stats.id, stats.status_id, stats.replies_count, stats.reblogs_count, \
             stats.favourites_count, stats.quotes_count, stats.untrusted_reblogs_count, \
             stats.untrusted_favourites_count FROM status_stats stats \
             JOIN statuses status ON status.id = stats.status_id AND status.deleted_at IS NULL \
             WHERE stats.status_id = $1",
        )
        .bind(status_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn status_edits(&self, status_id: i64) -> sqlx::Result<Vec<StatusEdit>> {
        sqlx::query_as::<_, StatusEdit>(
            "SELECT edit.id, edit.status_id, edit.account_id, edit.text, edit.spoiler_text, \
             edit.sensitive, edit.ordered_media_attachment_ids, edit.media_descriptions, \
             edit.poll_options, edit.quote_id, edit.created_at FROM status_edits edit \
             JOIN statuses status ON status.id = edit.status_id AND status.deleted_at IS NULL \
             WHERE edit.status_id = $1 ORDER BY edit.id",
        )
        .bind(status_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn media_attachments(&self, status_id: i64) -> sqlx::Result<Vec<MediaAttachment>> {
        sqlx::query_as::<_, MediaAttachment>(
            "SELECT media.id, media.account_id, media.status_id, media.type AS media_type, \
             media.processing, media.description, media.remote_url, media.file_content_type, \
             media.file_file_name, media.file_file_size, media.file_meta, \
             media.file_storage_schema_version, media.file_updated_at, media.scheduled_status_id, \
             media.shortcode, media.thumbnail_content_type, media.thumbnail_file_name, \
             media.thumbnail_file_size, media.thumbnail_remote_url, \
             media.thumbnail_storage_schema_version, media.thumbnail_updated_at, media.blurhash, \
              media.created_at, media.updated_at FROM statuses status \
              JOIN LATERAL ( \
                SELECT candidate.* FROM ( \
                  SELECT attachment.*, ordering.position \
                  FROM unnest(status.ordered_media_attachment_ids) WITH ORDINALITY \
                    AS ordering(media_id, position) \
                  JOIN media_attachments attachment \
                    ON attachment.id = ordering.media_id AND attachment.status_id = status.id \
                  WHERE status.ordered_media_attachment_ids IS NOT NULL \
                  UNION ALL \
                  SELECT attachment.*, row_number() OVER (ORDER BY attachment.id) AS position \
                  FROM media_attachments attachment \
                  WHERE attachment.status_id = status.id \
                    AND status.ordered_media_attachment_ids IS NULL \
                ) candidate ORDER BY candidate.position LIMIT 4 \
              ) media ON true \
              WHERE status.id = $1 AND status.deleted_at IS NULL ORDER BY media.position",
        )
        .bind(status_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn mentions(&self, status_id: i64) -> sqlx::Result<Vec<Mention>> {
        sqlx::query_as::<_, Mention>(
            "SELECT mention.id, mention.account_id, mention.status_id, mention.silent \
             FROM mentions mention \
             JOIN statuses status ON status.id = mention.status_id AND status.deleted_at IS NULL \
             WHERE mention.status_id = $1 ORDER BY mention.id",
        )
        .bind(status_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn tags(&self, status_id: i64) -> sqlx::Result<Vec<Tag>> {
        sqlx::query_as::<_, Tag>(
            "SELECT t.id, t.name, t.display_name, t.usable, t.trendable, t.listable, t.last_status_at \
             FROM tags t JOIN statuses_tags st ON st.tag_id = t.id \
             JOIN statuses status ON status.id = st.status_id AND status.deleted_at IS NULL \
             WHERE st.status_id = $1 ORDER BY t.id",
        )
        .bind(status_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn status_tags(&self, status_id: i64) -> sqlx::Result<Vec<StatusTag>> {
        sqlx::query_as::<_, StatusTag>(
            "SELECT st.status_id, st.tag_id FROM statuses_tags st \
             JOIN statuses status ON status.id = st.status_id AND status.deleted_at IS NULL \
             WHERE st.status_id = $1 ORDER BY st.tag_id",
        )
        .bind(status_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn account_tags(&self, account_id: i64) -> sqlx::Result<Vec<AccountTag>> {
        sqlx::query_as::<_, AccountTag>(
            "SELECT account_id, tag_id FROM accounts_tags WHERE account_id = $1 ORDER BY tag_id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn featured_tags(&self, account_id: i64) -> sqlx::Result<Vec<FeaturedTag>> {
        sqlx::query_as::<_, FeaturedTag>(
            "SELECT id, account_id, tag_id, name, statuses_count, last_status_at \
             FROM featured_tags WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn conversation(&self, id: i64) -> sqlx::Result<Option<Conversation>> {
        sqlx::query_as::<_, Conversation>(
            "SELECT conversation.id, conversation.uri, conversation.parent_account_id, \
             CASE WHEN status.id IS NOT NULL AND status.deleted_at IS NULL \
                  THEN conversation.parent_status_id END AS parent_status_id \
             FROM conversations conversation \
             LEFT JOIN statuses status ON status.id = conversation.parent_status_id \
             WHERE conversation.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn account_conversations(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<AccountConversation>> {
        sqlx::query_as::<_, AccountConversation>(
            "SELECT account_conversation.id, account_conversation.account_id, \
             account_conversation.conversation_id, \
             CASE WHEN last_status.id IS NOT NULL AND last_status.deleted_at IS NULL \
                  THEN account_conversation.last_status_id END AS last_status_id, \
             account_conversation.participant_account_ids, \
             ARRAY(SELECT status_id FROM unnest(account_conversation.status_ids) \
                   WITH ORDINALITY ids(status_id, ordinal) \
                   JOIN statuses status ON status.id = ids.status_id AND status.deleted_at IS NULL \
                   ORDER BY ids.ordinal) AS status_ids, account_conversation.unread \
             FROM account_conversations account_conversation \
             LEFT JOIN statuses last_status ON last_status.id = account_conversation.last_status_id \
             WHERE account_conversation.account_id = $1 ORDER BY account_conversation.id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn conversation_mutes(&self, account_id: i64) -> sqlx::Result<Vec<ConversationMute>> {
        sqlx::query_as::<_, ConversationMute>(
            "SELECT id, account_id, conversation_id FROM conversation_mutes \
             WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn follows(&self, account_id: i64) -> sqlx::Result<Vec<Follow>> {
        sqlx::query_as::<_, Follow>(
            "SELECT id, account_id, target_account_id, show_reblogs, notify, languages, uri \
             FROM follows WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn follow_requests(
        &self,
        target_account_id: i64,
    ) -> sqlx::Result<Vec<FollowRequest>> {
        sqlx::query_as::<_, FollowRequest>(
            "SELECT id, account_id, target_account_id, show_reblogs, notify, languages, uri \
             FROM follow_requests WHERE target_account_id = $1 ORDER BY id",
        )
        .bind(target_account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn favourites(&self, account_id: i64) -> sqlx::Result<Vec<Favourite>> {
        sqlx::query_as::<_, Favourite>(
            "SELECT favourite.id, favourite.account_id, favourite.status_id, favourite.created_at \
             FROM favourites favourite \
             JOIN statuses status ON status.id = favourite.status_id AND status.deleted_at IS NULL \
             WHERE favourite.account_id = $1 ORDER BY favourite.id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn bookmarks(&self, account_id: i64) -> sqlx::Result<Vec<Bookmark>> {
        sqlx::query_as::<_, Bookmark>(
            "SELECT bookmark.id, bookmark.account_id, bookmark.status_id, bookmark.created_at \
             FROM bookmarks bookmark \
             JOIN statuses status ON status.id = bookmark.status_id AND status.deleted_at IS NULL \
             WHERE bookmark.account_id = $1 ORDER BY bookmark.id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn status_pins(&self, account_id: i64) -> sqlx::Result<Vec<StatusPin>> {
        sqlx::query_as::<_, StatusPin>(
            "SELECT pin.id, pin.account_id, pin.status_id, pin.created_at FROM status_pins pin \
             JOIN statuses status ON status.id = pin.status_id AND status.deleted_at IS NULL \
             WHERE pin.account_id = $1 ORDER BY pin.id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn blocks(&self, account_id: i64) -> sqlx::Result<Vec<Block>> {
        sqlx::query_as::<_, Block>(
            "SELECT id, account_id, target_account_id, uri FROM blocks WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn mutes(&self, account_id: i64) -> sqlx::Result<Vec<Mute>> {
        sqlx::query_as::<_, Mute>(
            "SELECT id, account_id, target_account_id, hide_notifications, expires_at \
             FROM mutes WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn account_domain_blocks(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<AccountDomainBlock>> {
        sqlx::query_as::<_, AccountDomainBlock>(
            "SELECT id, account_id, domain FROM account_domain_blocks WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn lists(&self, account_id: i64) -> sqlx::Result<Vec<List>> {
        sqlx::query_as::<_, List>(
            "SELECT id, account_id, title, replies_policy, exclusive FROM lists WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn list_accounts(&self, list_id: i64) -> sqlx::Result<Vec<ListAccount>> {
        sqlx::query_as::<_, ListAccount>(
            "SELECT id, list_id, account_id, follow_id, follow_request_id \
             FROM list_accounts WHERE list_id = $1 ORDER BY id",
        )
        .bind(list_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn custom_filters(&self, account_id: i64) -> sqlx::Result<Vec<CustomFilter>> {
        sqlx::query_as::<_, CustomFilter>(
            "SELECT id, account_id, phrase, context, action, expires_at \
             FROM custom_filters WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn custom_filter_keywords(
        &self,
        custom_filter_id: i64,
    ) -> sqlx::Result<Vec<CustomFilterKeyword>> {
        sqlx::query_as::<_, CustomFilterKeyword>(
            "SELECT id, custom_filter_id, keyword, whole_word \
             FROM custom_filter_keywords WHERE custom_filter_id = $1 ORDER BY id",
        )
        .bind(custom_filter_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn custom_filter_statuses(
        &self,
        custom_filter_id: i64,
    ) -> sqlx::Result<Vec<CustomFilterStatus>> {
        sqlx::query_as::<_, CustomFilterStatus>(
            "SELECT filter_status.id, filter_status.custom_filter_id, filter_status.status_id \
             FROM custom_filter_statuses filter_status \
             JOIN statuses status ON status.id = filter_status.status_id AND status.deleted_at IS NULL \
             WHERE filter_status.custom_filter_id = $1 ORDER BY filter_status.id",
        )
        .bind(custom_filter_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn notifications(&self, account_id: i64) -> sqlx::Result<Vec<Notification>> {
        self.notification_rows(account_id, false).await
    }

    pub async fn notifications_including_filtered(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<Notification>> {
        self.notification_rows(account_id, true).await
    }

    async fn notification_rows(
        &self,
        account_id: i64,
        include_filtered: bool,
    ) -> sqlx::Result<Vec<Notification>> {
        sqlx::query_as::<_, Notification>(
            "SELECT notification.id, notification.account_id, notification.activity_id, \
              notification.activity_type, notification.from_account_id, \
              notification.type AS notification_type, notification.group_key, \
              notification.filtered, notification.created_at FROM notifications notification \
              JOIN accounts sender ON sender.id = notification.from_account_id \
                AND sender.suspended_at IS NULL \
              WHERE notification.account_id = $1 AND ($2 OR notification.filtered = false) \
              ORDER BY notification.id",
        )
        .bind(account_id)
        .bind(include_filtered)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn relationship_severance_event(
        &self,
        id: i64,
    ) -> sqlx::Result<Option<RelationshipSeveranceEvent>> {
        sqlx::query_as::<_, RelationshipSeveranceEvent>(
            "SELECT id, type AS event_type, target_name, purged, created_at \
             FROM relationship_severance_events WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn account_relationship_severance_event(
        &self,
        id: i64,
    ) -> sqlx::Result<Option<AccountRelationshipSeveranceEvent>> {
        sqlx::query_as::<_, AccountRelationshipSeveranceEvent>(
            "SELECT id, account_id, relationship_severance_event_id, followers_count, \
             following_count, created_at FROM account_relationship_severance_events WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn account_warning(&self, id: i64) -> sqlx::Result<Option<AccountWarning>> {
        sqlx::query_as::<_, AccountWarning>(
            "SELECT id, account_id, target_account_id, report_id, action, text, status_ids, \
             overruled_at, created_at FROM account_warnings WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn generated_annual_report(
        &self,
        id: i64,
    ) -> sqlx::Result<Option<GeneratedAnnualReport>> {
        sqlx::query_as::<_, GeneratedAnnualReport>(
            "SELECT id, account_id, year, schema_version, data, share_key, viewed_at, created_at \
             FROM generated_annual_reports WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn report(&self, id: i64) -> sqlx::Result<Option<Report>> {
        sqlx::query_as::<_, Report>(
            "SELECT id, account_id, target_account_id, action_taken_at, action_taken_by_account_id, \
             application_id, assigned_account_id, category, comment, forwarded, rule_ids, \
             status_ids, uri, created_at FROM reports WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn notification_policy(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Option<NotificationPolicy>> {
        sqlx::query_as::<_, NotificationPolicy>(
            "SELECT id, account_id, for_bots, for_limited_accounts, for_new_accounts, \
             for_not_followers, for_not_following, for_private_mentions \
             FROM notification_policies WHERE account_id = $1",
        )
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn notification_permissions(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<NotificationPermission>> {
        sqlx::query_as::<_, NotificationPermission>(
            "SELECT id, account_id, from_account_id FROM notification_permissions \
             WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn notification_requests(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<NotificationRequest>> {
        sqlx::query_as::<_, NotificationRequest>(
            "SELECT request.id, request.account_id, request.from_account_id, \
              CASE WHEN status.id IS NOT NULL AND status.deleted_at IS NULL \
                   THEN request.last_status_id END AS last_status_id, \
              request.notifications_count, request.created_at FROM notification_requests request \
              JOIN accounts sender ON sender.id = request.from_account_id \
                AND sender.suspended_at IS NULL \
              LEFT JOIN statuses status ON status.id = request.last_status_id \
             WHERE request.account_id = $1 ORDER BY request.id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn domain_allows(&self) -> sqlx::Result<Vec<DomainAllow>> {
        sqlx::query_as::<_, DomainAllow>("SELECT id, domain FROM domain_allows ORDER BY id")
            .fetch_all(&self.pool)
            .await
    }

    pub async fn domain_blocks(&self) -> sqlx::Result<Vec<DomainBlock>> {
        sqlx::query_as::<_, DomainBlock>(
            "SELECT id, domain, severity, reject_media, reject_reports, private_comment, public_comment, obfuscate \
             FROM domain_blocks ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn settings(&self) -> sqlx::Result<Vec<Setting>> {
        sqlx::query_as::<_, Setting>("SELECT id, var, value FROM settings ORDER BY id")
            .fetch_all(&self.pool)
            .await
    }

    pub async fn quotes(&self, account_id: i64) -> sqlx::Result<Vec<Quote>> {
        sqlx::query_as::<_, Quote>(
            "SELECT quote.id, quote.account_id, quote.status_id, quote.quoted_account_id, \
              quote.quoted_status_id, quote.state, quote.activity_uri, quote.approval_uri, quote.legacy \
              FROM quotes quote \
              JOIN statuses status ON status.id = quote.status_id AND status.deleted_at IS NULL \
              WHERE quote.account_id = $1 ORDER BY quote.id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn collections(&self, account_id: i64) -> sqlx::Result<Vec<Collection>> {
        sqlx::query_as::<_, Collection>(
            "SELECT id, account_id, name, description, description_html, local, sensitive, discoverable, \
             item_count, original_number_of_items, language, uri, url, tag_id \
             FROM collections WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn collection_items(&self, collection_id: i64) -> sqlx::Result<Vec<CollectionItem>> {
        sqlx::query_as::<_, CollectionItem>(
            "SELECT id, collection_id, account_id, position, state, activity_uri, approval_uri, object_uri, uri \
             FROM collection_items WHERE collection_id = $1 ORDER BY id",
        )
        .bind(collection_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn poll(&self, id: i64) -> sqlx::Result<Option<Poll>> {
        sqlx::query_as::<_, Poll>(
            "SELECT poll.id, poll.account_id, poll.status_id, poll.options, poll.cached_tallies, \
             poll.votes_count, poll.voters_count, poll.multiple, poll.hide_totals, poll.expires_at \
             FROM polls poll \
             JOIN statuses status ON status.id = poll.status_id AND status.deleted_at IS NULL \
             WHERE poll.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn poll_votes(&self, poll_id: i64) -> sqlx::Result<Vec<PollVote>> {
        sqlx::query_as::<_, PollVote>(
            "SELECT vote.id, vote.account_id, vote.poll_id, vote.choice, vote.uri \
             FROM poll_votes vote JOIN polls poll ON poll.id = vote.poll_id \
             JOIN statuses status ON status.id = poll.status_id AND status.deleted_at IS NULL \
             WHERE vote.poll_id = $1 ORDER BY vote.id",
        )
        .bind(poll_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn keypairs(&self, account_id: i64) -> sqlx::Result<Vec<Keypair>> {
        sqlx::query_as::<_, Keypair>(
            "SELECT id, account_id, type AS key_type, uri, public_key, private_key, revoked, expires_at \
             FROM keypairs WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn tombstone(&self, id: i64) -> sqlx::Result<Option<Tombstone>> {
        sqlx::query_as::<_, Tombstone>(
            "SELECT id, account_id, uri, by_moderator, created_at FROM tombstones WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }
}
