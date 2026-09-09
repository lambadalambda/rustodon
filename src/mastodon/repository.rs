use std::time::Duration;

use chrono::NaiveDateTime;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use unicode_normalization::UnicodeNormalization;
use url::Url;

use crate::crypto::ActiveRecordEncryptionConfig;
use crate::paperclip::{PaperclipAttachment, PaperclipMetadata};
use crate::preflight::V1_CRITICAL_TABLES;
use crate::remote::canonical_remote_host;

use super::policy::{
    AuthenticatedViewerFacts, AuthorRestriction, StatusAccessFacts, StatusAvailability,
    StatusContextFacts, ViewerFacts, ViewerRestriction, global_domain_policy, status_access,
    status_context_access,
};
use super::records::{
    Account, AccountConversation, AccountDomainBlock, AccountRelationshipSeveranceEvent,
    AccountStat, AccountTag, AccountWarning, ActivityPubSignatureKey, Block, Bookmark,
    BrowserSession, Collection, CollectionItem, Conversation, ConversationMute, CustomFilter,
    CustomFilterKeyword, CustomFilterStatus, DomainAllow, DomainBlock, Favourite, FeaturedTag,
    Follow, FollowRequest, GeneratedAnnualReport, Keypair, List, ListAccount, Marker,
    MediaAttachment, Mention, Mute, Notification, NotificationPermission, NotificationPolicy,
    NotificationRequest, OAuthAccessToken, OAuthApplication, OAuthBearerCandidate, Poll, PollVote,
    Quote, RelationshipSeveranceEvent, Report, Setting, Status, StatusEdit, StatusPin, StatusStat,
    StatusTag, Tag, Tombstone, User, UserRole,
};
use super::types::{PermissionBits, SecretText, StatusVisibility, UserPermission};

#[derive(sqlx::FromRow)]
#[allow(clippy::struct_excessive_bools)]
struct StatusPolicyRow {
    id: i64,
    visibility: i32,
    status_deleted: bool,
    author_suspended: bool,
    author_silenced: bool,
    viewer_is_author: bool,
    viewer_follows_author: bool,
    viewer_is_mentioned: bool,
    author_blocks_viewer: bool,
    author_domain_blocks_viewer: bool,
    viewer_blocks_author: bool,
    viewer_domain_blocks_author: bool,
    viewer_mutes_author: bool,
}

#[derive(sqlx::FromRow)]
struct SignatureAccountRow {
    id: i64,
    #[allow(dead_code)]
    username: String,
    #[allow(dead_code)]
    domain: Option<String>,
    #[allow(dead_code)]
    uri: String,
    #[allow(dead_code)]
    id_scheme: Option<i32>,
    public_key: String,
}

fn parse_acct_key_id(key_id: &str) -> Option<(&str, &str)> {
    let value = key_id.strip_prefix("acct:")?;
    let (username, domain) = value.rsplit_once('@')?;
    if username.is_empty()
        || domain.is_empty()
        || username.bytes().any(|byte| byte.is_ascii_whitespace())
        || domain.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return None;
    }
    Some((username, domain))
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct ActivityPubQuoteTarget {
    pub(crate) quote_id: i64,
    pub(crate) id: i64,
    pub(crate) account_id: i64,
    pub(crate) local: bool,
    pub(crate) quoted_account_local: bool,
    pub(crate) id_scheme: Option<super::types::AccountIdScheme>,
    pub(crate) username: String,
    pub(crate) uri: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) approval_uri: Option<String>,
}

impl StatusPolicyRow {
    fn access_facts(&self, viewer_authenticated: bool) -> StatusAccessFacts {
        StatusAccessFacts {
            visibility: StatusVisibility::from(self.visibility),
            availability: if self.status_deleted {
                StatusAvailability::Deleted
            } else if self.author_suspended {
                StatusAvailability::AuthorSuspended
            } else {
                StatusAvailability::Available
            },
            viewer: if viewer_authenticated {
                ViewerFacts::Authenticated(AuthenticatedViewerFacts {
                    is_author: self.viewer_is_author,
                    follows_author: self.viewer_follows_author,
                    is_mentioned: self.viewer_is_mentioned,
                    author_restriction: if self.author_blocks_viewer {
                        AuthorRestriction::BlocksViewer
                    } else if self.author_domain_blocks_viewer {
                        AuthorRestriction::BlocksViewerDomain
                    } else {
                        AuthorRestriction::None
                    },
                })
            } else {
                ViewerFacts::Anonymous
            },
        }
    }

    fn context_facts(&self, viewer_authenticated: bool) -> StatusContextFacts {
        StatusContextFacts {
            access: self.access_facts(viewer_authenticated),
            author_silenced: self.author_silenced,
            viewer_restriction: if self.viewer_blocks_author {
                ViewerRestriction::BlocksAuthor
            } else if self.viewer_domain_blocks_author {
                ViewerRestriction::BlocksAuthorDomain
            } else if self.viewer_mutes_author {
                ViewerRestriction::MutesAuthor
            } else {
                ViewerRestriction::None
            },
        }
    }
}
use super::rest::{
    AccountListKind, AccountListOptions, AccountStatusesOptions, FollowCollectionKind,
    FollowCollectionOptions, FollowedTagsOptions, NotificationOptions, RestAccountHandleRow,
    RestAccountListRow, RestAccountRow, RestAccountWarningRow, RestCredentialRow,
    RestCustomEmojiRow, RestFeaturedTagRow, RestFollowCollectionRow, RestFollowedTagRow,
    RestInstanceCountsRow, RestListedCustomEmojiRow, RestMentionRow, RestNotificationGroupRow,
    RestNotificationTargetRow, RestPollVoteRow, RestPreferencesRow, RestPreviewCardRow,
    RestRelationshipRow, RestRuleRow, RestSavedStatusRow, RestSeveranceEventRow,
    RestStatusQuoteRow, RestStatusRow, RestStatusTagRow, RestTagSuggestionRow,
    RestTaggedCollectionRow, SavedStatusKind, SavedStatusesOptions, TagTimelineOptions,
    TimelineOptions, grouped_notification_types, notification_type_filter_with_exclusions,
};

const REST_LIST_TIMELINE_SQL: &str = "WITH authorized AS ( \
   SELECT status.*, member.follow_id, member_follow.languages AS follow_languages, \
     member_follow.show_reblogs, source.account_id AS source_account_id, \
     source_author.domain AS source_author_domain, author.domain AS author_domain \
   FROM statuses status \
   JOIN accounts author ON author.id = status.account_id \
   JOIN list_accounts member ON member.list_id = $2 \
     AND member.account_id = status.account_id \
     AND (member.follow_id IS NOT NULL OR status.account_id = $1) \
   LEFT JOIN follows member_follow ON member_follow.id = member.follow_id \
   LEFT JOIN statuses source ON source.id = status.reblog_of_id AND source.deleted_at IS NULL \
   LEFT JOIN accounts source_author ON source_author.id = source.account_id \
   LEFT JOIN accounts viewer ON viewer.id = $1 \
   WHERE status.deleted_at IS NULL AND author.suspended_at IS NULL \
     AND (source.id IS NULL OR source_author.suspended_at IS NULL) \
     AND status.visibility IN (0, 1, 2) \
     AND CASE WHEN status.account_id = $1 THEN true \
       WHEN status.visibility = 2 THEN member_follow.id IS NOT NULL \
       WHEN status.visibility IN (0, 1) THEN NOT EXISTS (SELECT 1 FROM blocks author_block \
         WHERE author_block.account_id = status.account_id AND author_block.target_account_id = $1) \
         AND (viewer.domain IS NULL OR NOT EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
           WHERE domain_block.account_id = status.account_id AND domain_block.domain = viewer.domain)) \
       ELSE false END \
 ) SELECT status.id FROM authorized status \
 WHERE (COALESCE(cardinality(status.follow_languages), 0) = 0 OR status.language IS NULL \
     OR status.language = ANY(status.follow_languages)) \
   AND (NOT status.reply OR (status.in_reply_to_id IS NOT NULL \
     AND status.in_reply_to_account_id IS NOT NULL)) \
   AND (NOT status.reply OR status.in_reply_to_account_id = status.account_id \
     OR status.in_reply_to_account_id = $1 \
     OR ($3 = 0 AND EXISTS (SELECT 1 FROM list_accounts reply_member \
       WHERE reply_member.list_id = $2 AND reply_member.account_id = status.in_reply_to_account_id)) \
     OR ($3 = 1 AND EXISTS (SELECT 1 FROM follows reply_follow \
       WHERE reply_follow.account_id = $1 \
         AND reply_follow.target_account_id = status.in_reply_to_account_id))) \
   AND (status.account_id = $1 OR status.reblog_of_id IS NULL OR ( \
     status.source_account_id IS NOT NULL AND status.show_reblogs)) \
   AND (status.account_id = $1 OR NOT EXISTS (SELECT 1 FROM blocks viewer_block \
     WHERE viewer_block.account_id = $1 AND viewer_block.target_account_id = status.account_id)) \
   AND (status.account_id = $1 OR NOT EXISTS (SELECT 1 FROM blocks author_block \
     WHERE author_block.account_id = status.account_id AND author_block.target_account_id = $1)) \
    AND (status.account_id = $1 OR NOT EXISTS (SELECT 1 FROM mutes viewer_mute \
      WHERE viewer_mute.account_id = $1 AND viewer_mute.target_account_id = status.account_id)) \
   AND (status.account_id = $1 OR NOT EXISTS (SELECT 1 FROM mentions mention \
     WHERE mention.status_id IN (status.id, status.reblog_of_id) AND NOT mention.silent \
       AND (EXISTS (SELECT 1 FROM blocks mention_block WHERE mention_block.account_id = $1 \
         AND mention_block.target_account_id = mention.account_id) \
        OR EXISTS (SELECT 1 FROM mutes mention_mute WHERE mention_mute.account_id = $1 \
          AND mention_mute.target_account_id = mention.account_id)))) \
   AND (status.account_id = $1 OR status.source_account_id IS NULL OR ( \
     NOT EXISTS (SELECT 1 FROM blocks source_block WHERE source_block.account_id = $1 \
       AND source_block.target_account_id = status.source_account_id) \
      AND NOT EXISTS (SELECT 1 FROM mutes source_mute WHERE source_mute.account_id = $1 \
        AND source_mute.target_account_id = status.source_account_id) \
     AND NOT EXISTS (SELECT 1 FROM blocks source_author_block \
       WHERE source_author_block.account_id = status.source_account_id \
         AND source_author_block.target_account_id = $1) \
     AND (status.source_author_domain IS NULL OR NOT EXISTS ( \
       SELECT 1 FROM account_domain_blocks source_domain_block \
       WHERE source_domain_block.account_id = $1 \
         AND source_domain_block.domain = status.source_author_domain)))) \
   AND (status.account_id = $1 OR status.author_domain IS NULL OR NOT EXISTS ( \
     SELECT 1 FROM account_domain_blocks domain_block WHERE domain_block.account_id = $1 \
       AND domain_block.domain = status.author_domain)) \
   AND ($4::bigint IS NULL OR status.id < $4) \
   AND (($5::bigint IS NOT NULL AND status.id > $5) \
     OR ($5 IS NULL AND ($6::bigint IS NULL OR status.id > $6))) \
 ORDER BY status.id {ordering} LIMIT $7";

#[derive(Clone)]
pub struct Repository {
    pool: PgPool,
    active_record_encryption: Option<ActiveRecordEncryptionConfig>,
}

fn decrypt_otp_secret(
    encryption: Option<&ActiveRecordEncryptionConfig>,
    secret: Option<SecretText>,
) -> sqlx::Result<Option<SecretText>> {
    let Some(secret) = secret else {
        return Ok(None);
    };
    let Some(encryption) = encryption else {
        return Ok(Some(secret));
    };
    if !secret.as_str().trim_start().starts_with('{') {
        return Ok(Some(secret));
    }
    let plaintext = encryption
        .decrypt_string(secret.as_str(), 256)
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    Ok(Some(SecretText::new(plaintext.expose_secret().to_owned())))
}

#[allow(clippy::missing_errors_doc)]
impl Repository {
    #[must_use]
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            active_record_encryption: None,
        }
    }

    pub async fn connect(database_url: &str) -> sqlx::Result<Self> {
        let options = database_url.parse::<PgConnectOptions>()?;
        Self::connect_with(options).await
    }

    pub async fn connect_with(options: PgConnectOptions) -> sqlx::Result<Self> {
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
            .connect_with(options)
            .await?;
        Ok(Self {
            pool,
            active_record_encryption: None,
        })
    }

    #[must_use]
    pub fn with_active_record_encryption(
        mut self,
        encryption: ActiveRecordEncryptionConfig,
    ) -> Self {
        self.active_record_encryption = Some(encryption);
        self
    }

    pub async fn ready(&self) -> bool {
        let relations = V1_CRITICAL_TABLES
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query_scalar::<_, bool>(
                "SELECT pg_catalog.has_table_privilege( \
                   CURRENT_USER, \
                   pg_catalog.format('%I.%I', 'public', relation_name), \
                   'SELECT') \
                 FROM unnest($1::text[]) relation_name",
            )
            .bind(&relations)
            .fetch_all(&self.pool),
        )
        .await
        .is_ok_and(|result| result.is_ok_and(|privileges| privileges.into_iter().all(|item| item)))
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
                 AND a.moved_to_account_id IS NULL AS login_capable_user, \
              EXISTS (SELECT 1 FROM users u WHERE u.account_id = a.id AND u.approved = false) \
                AS has_pending_user, \
              EXISTS (SELECT 1 FROM users u WHERE u.account_id = a.id AND u.confirmed_at IS NULL) \
                AS has_unconfirmed_user \
              FROM accounts a WHERE a.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn account_has_deletion_request(&self, account_id: i64) -> sqlx::Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM account_deletion_requests WHERE account_id = $1)",
        )
        .bind(account_id)
        .fetch_one(&self.pool)
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
        let Some(mut user) = sqlx::query_as::<_, User>(
            "SELECT users.id, users.account_id, users.email, users.encrypted_password, users.chosen_languages, \
              otp_backup_codes::text[] AS otp_backup_codes, otp_required_for_login, otp_secret, \
              settings, sign_up_ip, role_id, COALESCE(role.require_2fa, false) AS role_requires_2fa, \
              approved, disabled, confirmed_at, locale, webauthn_id, \
              EXISTS (SELECT 1 FROM webauthn_credentials credential \
                      WHERE credential.user_id = users.id) AS has_webauthn_credentials \
              FROM users LEFT JOIN user_roles role ON role.id = users.role_id WHERE users.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        user.otp_secret =
            decrypt_otp_secret(self.active_record_encryption.as_ref(), user.otp_secret)?;
        Ok(Some(user))
    }

    pub async fn browser_session(&self, session_id: &str) -> sqlx::Result<Option<BrowserSession>> {
        sqlx::query_as::<_, BrowserSession>(
            "SELECT session.user_id, user_record.account_id, access_token.token AS access_token, session.updated_at, \
                (user_record.confirmed_at IS NOT NULL \
                 AND user_record.approved = true \
                 AND user_record.disabled = false \
                 AND account.suspended_at IS NULL \
                 AND account.moved_to_account_id IS NULL \
                 AND (COALESCE(role.require_2fa, false) = false \
                   OR user_record.otp_required_for_login = true \
                   OR EXISTS (SELECT 1 FROM webauthn_credentials credential \
                              WHERE credential.user_id = user_record.id))) AS functional \
              FROM session_activations session \
              JOIN oauth_access_tokens access_token ON access_token.id = session.access_token_id \
              JOIN users user_record ON user_record.id = session.user_id \
              JOIN accounts account ON account.id = user_record.account_id \
              LEFT JOIN user_roles role ON role.id = COALESCE(user_record.role_id, -99) \
             WHERE session.session_id = $1 \
               AND session.updated_at > clock_timestamp() - INTERVAL '30 days' \
               AND access_token.revoked_at IS NULL \
                AND (access_token.expires_in IS NULL OR \
                     access_token.created_at + access_token.expires_in * INTERVAL '1 second' > clock_timestamp()) \
                AND account.memorial = false",
        )
        .bind(session_id)
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

    pub async fn oauth_application_by_uid(
        &self,
        uid: &str,
    ) -> sqlx::Result<Option<OAuthApplication>> {
        sqlx::query_as::<_, OAuthApplication>(
            "SELECT id, name, uid, secret, redirect_uri, scopes, confidential, owner_id, owner_type, website \
             FROM oauth_applications WHERE uid = $1",
        )
        .bind(uid)
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

    pub(crate) async fn oauth_bearer_candidate(
        &self,
        token: &str,
    ) -> sqlx::Result<Option<OAuthBearerCandidate>> {
        sqlx::query_as::<_, OAuthBearerCandidate>(
            "SELECT access_token.id AS token_id, access_token.resource_owner_id, \
             access_token.application_id, access_token.scopes, access_token.expires_in, \
             access_token.created_at, access_token.revoked_at, \
             application.id IS NOT NULL AS application_exists, \
             token_user.id AS user_id, token_user.account_id AS user_account_id, \
             token_user.confirmed_at, token_user.approved, token_user.disabled, \
             token_user.otp_required_for_login, role.require_2fa AS role_requires_2fa, \
             EXISTS (SELECT 1 FROM webauthn_credentials credential \
                     WHERE credential.user_id = token_user.id) AS has_webauthn_credentials, \
              account.id AS account_id, account.suspended_at, \
              EXISTS (SELECT 1 FROM account_deletion_requests deletion_request \
                WHERE deletion_request.account_id = account.id) AS has_deletion_request, \
              account.memorial, \
             account.moved_to_account_id \
             FROM oauth_access_tokens access_token \
             LEFT JOIN oauth_applications application ON application.id = access_token.application_id \
             LEFT JOIN users token_user ON token_user.id = access_token.resource_owner_id \
             LEFT JOIN accounts account ON account.id = token_user.account_id \
             LEFT JOIN user_roles role ON role.id = COALESCE(token_user.role_id, -99) \
             WHERE access_token.token = $1",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn rest_account_rows(
        &self,
        ids: &[i64],
        viewer_account_id: Option<i64>,
    ) -> sqlx::Result<Vec<RestAccountRow>> {
        sqlx::query_as::<_, RestAccountRow>(
            "SELECT account.id, account.username, account.domain, account.actor_type, \
             account.id_scheme, account.display_name, account.note, account.uri, account.url, \
             account.locked, account.discoverable, account.indexable, account.memorial, \
             account.moved_to_account_id, account.suspended_at IS NOT NULL AS suspended, \
             account.silenced_at IS NOT NULL AS limited, \
             account.sensitized_at IS NOT NULL AS sensitized, account.created_at, \
              account.avatar_file_name, account.avatar_content_type, \
              account.avatar_storage_schema_version, account.avatar_description, \
              account.header_file_name, account.header_content_type, \
              account.header_storage_schema_version, account.header_description, \
             COALESCE(stats.followers_count, 0) AS followers_count, \
             COALESCE(stats.following_count, 0) AS following_count, \
             COALESCE(stats.statuses_count, 0) AS statuses_count, stats.last_status_at, \
             account.hide_collections, account.show_media, account.show_media_replies, \
             account.show_featured, account.feature_approval_policy, \
             account_user.settings AS user_settings, role.id AS role_id, role.name AS role_name, \
             role.color AS role_color, role.highlighted AS role_highlighted, \
             EXISTS (SELECT 1 FROM follows viewer_follow \
                     WHERE viewer_follow.account_id = $2 \
                       AND viewer_follow.target_account_id = account.id) AS viewer_follows, \
             EXISTS (SELECT 1 FROM follows account_follow \
                     WHERE account_follow.account_id = account.id \
                       AND account_follow.target_account_id = $2) AS follows_viewer, \
             account.fields \
             FROM accounts account \
             LEFT JOIN account_stats stats ON stats.account_id = account.id \
             LEFT JOIN LATERAL (SELECT candidate.settings, candidate.role_id \
                                FROM users candidate WHERE candidate.account_id = account.id \
                                ORDER BY candidate.id LIMIT 1) account_user ON true \
             LEFT JOIN user_roles role ON role.id = COALESCE(account_user.role_id, -99) \
             WHERE account.id = ANY($1) ORDER BY account.id",
        )
        .bind(ids)
        .bind(viewer_account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_account_handles(
        &self,
        handles: &[String],
    ) -> sqlx::Result<Vec<RestAccountHandleRow>> {
        sqlx::query_as::<_, RestAccountHandleRow>(
            "SELECT id, lower(username || CASE WHEN domain IS NULL THEN '' ELSE '@' || domain END) \
                    AS handle FROM accounts \
             WHERE lower(username || CASE WHEN domain IS NULL THEN '' ELSE '@' || domain END) = ANY($1) \
             ORDER BY id",
        )
        .bind(handles)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_account_id_by_handle(
        &self,
        username: &str,
        domain: Option<&str>,
    ) -> sqlx::Result<Option<i64>> {
        sqlx::query_scalar(
            "SELECT id FROM accounts WHERE lower(username) = lower($1) \
             AND (($2::text IS NULL AND domain IS NULL) OR lower(domain) = lower($2)) \
             ORDER BY id LIMIT 1",
        )
        .bind(username)
        .bind(domain)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn rest_account_last_webfingered_at(
        &self,
        username: &str,
        domain: &str,
    ) -> sqlx::Result<Option<NaiveDateTime>> {
        sqlx::query_scalar(
            "SELECT last_webfingered_at FROM accounts
             WHERE lower(username) = lower($1) AND lower(domain) = lower($2)
             ORDER BY id
             LIMIT 1",
        )
        .bind(username)
        .bind(domain)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn rest_account_following(
        &self,
        viewer_account_id: i64,
        target_account_id: i64,
    ) -> sqlx::Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM follows follow \
              WHERE follow.account_id = $1 AND follow.target_account_id = $2)",
        )
        .bind(viewer_account_id)
        .bind(target_account_id)
        .fetch_one(&self.pool)
        .await
    }

    pub(crate) async fn rest_account_search_ids(
        &self,
        tsquery: &str,
        viewer_account_id: Option<i64>,
        following: bool,
        limit: i64,
        offset: i64,
    ) -> sqlx::Result<Vec<i64>> {
        const TEXT_SEARCH_RANKS: &str = "( \
          setweight(to_tsvector('simple', account.display_name), 'A') || \
          setweight(to_tsvector('simple', account.username), 'B') || \
          setweight(to_tsvector('simple', coalesce(account.domain, '')), 'C') \
        )";
        const BOOST: &str = "( \
          (greatest(0, coalesce(stats.followers_count, 0)) / \
            (greatest(0, coalesce(stats.following_count, 0)) + 1.0) + \
           log(greatest(0, coalesce(stats.followers_count, 0)) + 2) + \
           CASE WHEN stats.last_status_at IS NULL THEN 0 ELSE exp(-1.0 * ( \
             (greatest(0, abs(extract(DAY FROM age(stats.last_status_at))) - 30.0)^2) / \
             (2.0 * ((-1.0 * 30^2) / (2.0 * ln(0.3))) ) \
           )) END) / 3.0 \
        )";
        let limit = limit.clamp(0, 80);
        if let Some(viewer_account_id) = viewer_account_id {
            if following {
                let query = format!(
                    "WITH first_degree AS ( \
                   SELECT target_account_id FROM follows WHERE account_id = $2 \
                   UNION ALL SELECT $2 \
                 ) \
                 SELECT account.id \
                 FROM accounts account \
                 LEFT OUTER JOIN follows follow ON account.id = follow.account_id \
                   AND follow.target_account_id = $2 \
                 LEFT JOIN account_stats stats ON account.id = stats.account_id \
                 WHERE account.id IN (SELECT target_account_id FROM first_degree) \
                   AND to_tsquery('simple', $1) @@ {TEXT_SEARCH_RANKS} \
                   AND account.suspended_at IS NULL \
                   AND account.moved_to_account_id IS NULL \
                 GROUP BY account.id, stats.id \
                 ORDER BY (count(follow.id) + 1) * {BOOST} * \
                   ts_rank_cd({TEXT_SEARCH_RANKS}, to_tsquery('simple', $1), 32) DESC \
                 LIMIT $3 OFFSET $4"
                );
                sqlx::query_scalar::<_, i64>(&query)
                    .bind(tsquery)
                    .bind(viewer_account_id)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
            } else {
                let query = format!(
                    "SELECT account.id \
                 FROM accounts account \
                 LEFT OUTER JOIN follows follow ON \
                   (account.id = follow.account_id AND follow.target_account_id = $2) \
                   OR (account.id = follow.target_account_id AND follow.account_id = $2) \
                 LEFT JOIN users account_user ON account.id = account_user.account_id \
                 LEFT JOIN account_stats stats ON account.id = stats.account_id \
                 WHERE to_tsquery('simple', $1) @@ {TEXT_SEARCH_RANKS} \
                   AND account.suspended_at IS NULL \
                   AND account.moved_to_account_id IS NULL \
                   AND (account.domain IS NOT NULL OR \
                     (account_user.approved = TRUE AND account_user.confirmed_at IS NOT NULL)) \
                 GROUP BY account.id, stats.id \
                 ORDER BY count(follow.id) DESC, {BOOST} * ts_rank_cd( \
                   {TEXT_SEARCH_RANKS}, to_tsquery('simple', $1), 32) DESC \
                 LIMIT $3 OFFSET $4"
                );
                sqlx::query_scalar::<_, i64>(&query)
                    .bind(tsquery)
                    .bind(viewer_account_id)
                    .bind(limit)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await
            }
        } else {
            let query = format!(
                "SELECT account.id \
                 FROM accounts account \
                 LEFT JOIN users account_user ON account.id = account_user.account_id \
                 LEFT JOIN account_stats stats ON account.id = stats.account_id \
                 WHERE to_tsquery('simple', $1) @@ {TEXT_SEARCH_RANKS} \
                   AND account.suspended_at IS NULL \
                   AND account.moved_to_account_id IS NULL \
                   AND (account.domain IS NOT NULL OR \
                     (account_user.approved = TRUE AND account_user.confirmed_at IS NOT NULL)) \
                 ORDER BY {BOOST} * ts_rank_cd( \
                   {TEXT_SEARCH_RANKS}, to_tsquery('simple', $1), 32) DESC \
                 LIMIT $2 OFFSET $3"
            );
            sqlx::query_scalar::<_, i64>(&query)
                .bind(tsquery)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
        }
    }

    pub(crate) async fn rest_account_showable(&self, account_id: i64) -> sqlx::Result<bool> {
        sqlx::query_scalar(
            "SELECT CASE WHEN account.domain IS NULL THEN EXISTS ( \
               SELECT 1 FROM users account_user WHERE account_user.account_id = account.id \
                 AND account_user.confirmed_at IS NOT NULL AND account_user.approved = true) \
             ELSE true END FROM accounts account WHERE account.id = $1",
        )
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
        .map(|value| value.unwrap_or(false))
    }

    pub(crate) async fn rest_credential_row(
        &self,
        user_id: i64,
        account_id: i64,
    ) -> sqlx::Result<Option<RestCredentialRow>> {
        sqlx::query_as::<_, RestCredentialRow>(
            "SELECT account_user.settings, role.id AS role_id, \
              role.name AS role_name, role.permissions AS role_permissions, \
              everyone.permissions AS everyone_permissions, role.color AS role_color, \
              role.highlighted AS role_highlighted, role.collection_limit, \
             account.attribution_domains, \
             (SELECT count(*) FROM (SELECT 1 FROM follow_requests request \
               JOIN accounts source ON source.id = request.account_id \
               WHERE request.target_account_id = account.id AND source.suspended_at IS NULL \
               LIMIT 40) matching_requests) AS follow_requests_count \
             FROM accounts account \
              JOIN users account_user ON account_user.id = $1 \
                AND account_user.account_id = account.id \
             JOIN user_roles role ON role.id = COALESCE(account_user.role_id, -99) \
             JOIN user_roles everyone ON everyone.id = -99 \
              WHERE account.id = $2",
        )
        .bind(user_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn rest_preferences_row(
        &self,
        user_id: i64,
        account_id: i64,
    ) -> sqlx::Result<Option<RestPreferencesRow>> {
        sqlx::query_as::<_, RestPreferencesRow>(
            "SELECT account_user.settings, account_user.locale, account.locked \
             FROM users account_user JOIN accounts account ON account.id = account_user.account_id \
             WHERE account_user.id = $1 AND account_user.account_id = $2",
        )
        .bind(user_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn user_can_view_feeds(&self, user_id: i64, account_id: i64) -> sqlx::Result<bool> {
        sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT role.id, role.permissions, everyone.permissions \
             FROM users account_user \
             JOIN user_roles role ON role.id = COALESCE(account_user.role_id, -99) \
             JOIN user_roles everyone ON everyone.id = -99 \
             WHERE account_user.id = $1 AND account_user.account_id = $2",
        )
        .bind(user_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
        .map(|row| {
            row.is_some_and(|(role_id, role, everyone)| {
                PermissionBits::effective(role_id, PermissionBits(role), PermissionBits(everyone))
                    .contains(UserPermission::ViewFeeds)
            })
        })
    }

    pub(crate) async fn rest_relationship_rows(
        &self,
        viewer_account_id: i64,
        target_account_ids: &[i64],
        with_suspended: bool,
    ) -> sqlx::Result<Vec<RestRelationshipRow>> {
        sqlx::query_as::<_, RestRelationshipRow>(
             "SELECT target.id AS target_account_id, outgoing.id IS NOT NULL AS following, \
              COALESCE(outgoing.show_reblogs, viewer_request.show_reblogs, false) AS showing_reblogs, \
              COALESCE(outgoing.notify, viewer_request.notify, false) AS notifying, \
              COALESCE(outgoing.languages, viewer_request.languages) AS languages, \
             incoming.id IS NOT NULL AS followed_by, viewer_block.id IS NOT NULL AS blocking, \
             target_block.id IS NOT NULL AS blocked_by, mute.id IS NOT NULL AS muting, \
             COALESCE(mute.hide_notifications, false) AS muting_notifications, \
             mute.expires_at AS muting_expires_at, \
             viewer_request.id IS NOT NULL AS requested, \
             target_request.id IS NOT NULL AS requested_by, \
             domain_block.id IS NOT NULL AS domain_blocking, \
             pin.id IS NOT NULL AS endorsed, COALESCE(note.comment, '') AS note \
             FROM accounts target \
             LEFT JOIN follows outgoing ON outgoing.account_id = $1 \
               AND outgoing.target_account_id = target.id \
             LEFT JOIN follows incoming ON incoming.account_id = target.id \
               AND incoming.target_account_id = $1 \
             LEFT JOIN blocks viewer_block ON viewer_block.account_id = $1 \
               AND viewer_block.target_account_id = target.id \
             LEFT JOIN blocks target_block ON target_block.account_id = target.id \
               AND target_block.target_account_id = $1 \
              LEFT JOIN mutes mute ON mute.account_id = $1 \
                AND mute.target_account_id = target.id \
             LEFT JOIN follow_requests viewer_request ON viewer_request.account_id = $1 \
               AND viewer_request.target_account_id = target.id \
             LEFT JOIN follow_requests target_request ON target_request.account_id = target.id \
               AND target_request.target_account_id = $1 \
             LEFT JOIN account_domain_blocks domain_block ON domain_block.account_id = $1 \
               AND domain_block.domain = target.domain \
             LEFT JOIN account_pins pin ON pin.account_id = $1 \
               AND pin.target_account_id = target.id \
             LEFT JOIN account_notes note ON note.account_id = $1 \
               AND note.target_account_id = target.id \
              WHERE target.id = ANY($2) AND ($3 OR target.suspended_at IS NULL) ORDER BY target.id",
        )
        .bind(viewer_account_id)
        .bind(target_account_ids)
        .bind(with_suspended)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_status_rows(
        &self,
        ids: &[i64],
        viewer_account_id: Option<i64>,
    ) -> sqlx::Result<Vec<RestStatusRow>> {
        sqlx::query_as::<_, RestStatusRow>(
            "SELECT status.id, status.account_id, status.text, status.spoiler_text, \
             status.visibility, status.local, status.uri, status.url, status.language, \
             status.sensitive, status.in_reply_to_id, \
             status.in_reply_to_account_id, status.reblog_of_id, status.quote_approval_policy, \
             status.edited_at, status.created_at, COALESCE(stats.replies_count, 0) AS replies_count, \
             COALESCE(stats.untrusted_reblogs_count, stats.reblogs_count, 0) AS reblogs_count, \
             COALESCE(stats.untrusted_favourites_count, stats.favourites_count, 0) AS favourites_count, \
             COALESCE(stats.quotes_count, 0) AS quotes_count, application.name AS application_name, \
             application.website AS application_website, author_user.id IS NOT NULL AS author_has_user, \
             author_user.settings AS author_settings, \
             EXISTS (SELECT 1 FROM follows viewer_follow WHERE viewer_follow.account_id = $2 \
               AND viewer_follow.target_account_id = status.account_id) AS viewer_follows_author, \
              EXISTS (SELECT 1 FROM follows author_follow WHERE author_follow.account_id = status.account_id \
                AND author_follow.target_account_id = $2) AS author_follows_viewer, \
               EXISTS (SELECT 1 FROM blocks author_block WHERE author_block.account_id = status.account_id \
                 AND author_block.target_account_id = $2) AS author_blocks_viewer, \
               EXISTS (SELECT 1 FROM account_domain_blocks author_domain_block \
                 JOIN accounts viewer ON viewer.id = $2 \
                 WHERE author_domain_block.account_id = status.account_id \
                   AND author_domain_block.domain = viewer.domain) AS author_domain_blocks_viewer, \
               author.suspended_at IS NOT NULL AS author_suspended, \
              EXISTS (SELECT 1 FROM blocks viewer_block WHERE viewer_block.account_id = $2 \
               AND viewer_block.target_account_id = status.account_id) AS viewer_blocks_author, \
             EXISTS (SELECT 1 FROM account_domain_blocks domain_block WHERE domain_block.account_id = $2 \
               AND domain_block.domain = author.domain) AS viewer_domain_blocks_author, \
               EXISTS (SELECT 1 FROM mutes viewer_mute WHERE viewer_mute.account_id = $2 \
                 AND viewer_mute.target_account_id = status.account_id) AS viewer_mutes_author, \
               EXISTS (SELECT 1 FROM favourites favourite WHERE favourite.account_id = $2 \
                AND favourite.status_id = status.id) AS favourited, \
               EXISTS (SELECT 1 FROM statuses boost WHERE boost.account_id = $2 \
                AND boost.reblog_of_id = status.id \
                AND boost.deleted_at IS NULL) AS reblogged, \
             EXISTS (SELECT 1 FROM conversation_mutes mute WHERE mute.account_id = $2 \
               AND mute.conversation_id = status.conversation_id) AS muted, \
               EXISTS (SELECT 1 FROM bookmarks bookmark WHERE bookmark.account_id = $2 \
                AND bookmark.status_id = status.id) AS bookmarked, \
             EXISTS (SELECT 1 FROM status_pins pin WHERE pin.account_id = $2 \
               AND pin.status_id = status.id) AS pinned \
             FROM statuses status \
             JOIN accounts author ON author.id = status.account_id \
             LEFT JOIN status_stats stats ON stats.status_id = status.id \
             LEFT JOIN oauth_applications application ON application.id = status.application_id \
             LEFT JOIN LATERAL (SELECT candidate.id, candidate.settings FROM users candidate \
               WHERE candidate.account_id = status.account_id ORDER BY candidate.id LIMIT 1) author_user ON true \
             WHERE status.id = ANY($1) AND status.deleted_at IS NULL ORDER BY status.id",
        )
        .bind(ids)
        .bind(viewer_account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_authorized_status_ids(
        &self,
        ids: &[i64],
        viewer_account_id: Option<i64>,
    ) -> sqlx::Result<Vec<i64>> {
        Ok(self
            .rest_status_policy_rows(ids, viewer_account_id)
            .await?
            .into_iter()
            .filter(|row| status_access(row.access_facts(viewer_account_id.is_some())).is_allowed())
            .map(|row| row.id)
            .collect())
    }

    pub(crate) async fn rest_account_status_ids(
        &self,
        account_id: i64,
        viewer_account_id: Option<i64>,
        options: &AccountStatusesOptions,
    ) -> sqlx::Result<Vec<i64>> {
        let tagged = options.tagged.as_deref().map(normalize_hashtag);
        let filter_reblog_sources =
            !options.exclude_reblogs && !options.only_media && tagged.is_none();
        let ordering = if options.min_id.is_some() {
            "status.id ASC"
        } else if options.pinned {
            "pin.created_at DESC, status.id DESC"
        } else {
            "status.id DESC"
        };
        let query = format!(
            "SELECT status.id FROM statuses status \
             JOIN accounts author ON author.id = status.account_id \
             LEFT JOIN accounts viewer ON viewer.id = $2 \
             LEFT JOIN status_pins pin ON pin.account_id = $1 AND pin.status_id = status.id \
             WHERE status.account_id = $1 AND status.deleted_at IS NULL \
               AND author.suspended_at IS NULL \
               AND CASE \
                 WHEN $2 IS NULL THEN status.visibility IN (0, 1) \
                 WHEN $2 = $1 THEN CASE WHEN $3 THEN status.visibility IN (0, 1, 2) \
                   ELSE status.visibility IN (0, 1, 2, 3, 4) END \
                 WHEN EXISTS (SELECT 1 FROM blocks block WHERE block.account_id = $1 \
                   AND block.target_account_id = $2) \
                   OR (viewer.domain IS NOT NULL AND EXISTS ( \
                     SELECT 1 FROM account_domain_blocks domain_block \
                     WHERE domain_block.account_id = $1 AND domain_block.domain = viewer.domain)) \
                   THEN false \
                 WHEN $3 THEN status.visibility IN (0, 1) OR (status.visibility = 2 AND EXISTS ( \
                   SELECT 1 FROM follows follow WHERE follow.account_id = $2 \
                     AND follow.target_account_id = $1)) \
                 ELSE status.visibility IN (0, 1) \
                   OR (status.visibility = 2 AND EXISTS (SELECT 1 FROM follows follow \
                     WHERE follow.account_id = $2 AND follow.target_account_id = $1)) \
                   OR (status.visibility IN (2, 3, 4) AND EXISTS (SELECT 1 FROM mentions mention \
                     WHERE mention.status_id = status.id AND mention.account_id = $2)) \
               END \
               AND (NOT $4 OR pin.id IS NOT NULL) \
               AND ($5::text IS NULL OR EXISTS (SELECT 1 FROM statuses_tags status_tag \
                 JOIN tags tag ON tag.id = status_tag.tag_id \
                 WHERE status_tag.status_id = status.id AND tag.name = $5)) \
                AND (NOT $6 OR ((status.ordered_media_attachment_ids IS NULL \
                  OR cardinality(status.ordered_media_attachment_ids) > 0) \
                  AND EXISTS (SELECT 1 FROM media_attachments media \
                    WHERE media.status_id = status.id))) \
                AND (NOT $7 OR status.reply = false \
                  OR status.in_reply_to_account_id = status.account_id) \
                AND (NOT $8 OR status.reblog_of_id IS NULL) \
                AND ($9::bigint IS NULL OR status.id < $9) \
                AND (($10::bigint IS NOT NULL AND status.id > $10) \
                  OR ($10 IS NULL AND ($11::bigint IS NULL OR status.id > $11))) \
                AND (status.reblog_of_id IS NULL OR EXISTS ( \
                  SELECT 1 FROM statuses source \
                  WHERE source.id = status.reblog_of_id AND source.deleted_at IS NULL)) \
                AND (NOT $13 OR $2 = $1 OR status.reblog_of_id IS NULL OR $2 IS NULL OR NOT EXISTS ( \
                  SELECT 1 FROM statuses source \
                 JOIN accounts source_author ON source_author.id = source.account_id \
                 WHERE source.id = status.reblog_of_id AND ( \
                   EXISTS (SELECT 1 FROM blocks block WHERE block.account_id = $2 \
                     AND block.target_account_id = source.account_id) \
                   OR EXISTS (SELECT 1 FROM blocks block WHERE block.account_id = source.account_id \
                     AND block.target_account_id = $2) \
                    OR EXISTS (SELECT 1 FROM mutes mute WHERE mute.account_id = $2 \
                      AND mute.target_account_id = source.account_id) \
                   OR (source_author.domain IS NOT NULL AND EXISTS ( \
                     SELECT 1 FROM account_domain_blocks domain_block \
                     WHERE domain_block.account_id = $2 \
                       AND domain_block.domain = source_author.domain))))) \
              ORDER BY {ordering} LIMIT $12"
        );
        let mut ids = sqlx::query_scalar(&query)
            .bind(account_id)
            .bind(viewer_account_id)
            .bind(options.exclude_direct)
            .bind(options.pinned)
            .bind(tagged.as_deref())
            .bind(options.only_media)
            .bind(options.exclude_replies)
            .bind(options.exclude_reblogs)
            .bind(options.max_id)
            .bind(options.min_id)
            .bind(options.since_id)
            .bind(options.limit.clamp(0, 40))
            .bind(filter_reblog_sources)
            .fetch_all(&self.pool)
            .await?;
        if options.min_id.is_some() {
            ids.reverse();
        }
        Ok(ids)
    }

    pub(crate) async fn rest_public_timeline_ids(
        &self,
        viewer_account_id: Option<i64>,
        options: &TimelineOptions,
    ) -> sqlx::Result<Vec<i64>> {
        let ordering = if options.min_id.is_some() {
            "ASC"
        } else {
            "DESC"
        };
        let query = format!(
            "SELECT status.id FROM statuses status \
             JOIN accounts author ON author.id = status.account_id \
             WHERE status.deleted_at IS NULL AND status.visibility = 0 \
               AND author.suspended_at IS NULL AND author.silenced_at IS NULL \
               AND status.reblog_of_id IS NULL \
               AND (status.reply = false OR status.in_reply_to_account_id = status.account_id) \
               AND (NOT $2 OR status.local = true OR status.uri IS NULL) \
               AND (NOT $3 OR status.local = false AND status.uri IS NOT NULL) \
               AND (NOT $4 OR EXISTS (SELECT 1 FROM media_attachments media \
                 WHERE media.status_id = status.id)) \
               AND ($1::bigint IS NULL OR ( \
                 NOT EXISTS (SELECT 1 FROM blocks viewer_block WHERE viewer_block.account_id = $1 \
                   AND viewer_block.target_account_id = status.account_id) \
                 AND NOT EXISTS (SELECT 1 FROM blocks author_block WHERE author_block.account_id = status.account_id \
                   AND author_block.target_account_id = $1) \
                  AND NOT EXISTS (SELECT 1 FROM mutes viewer_mute WHERE viewer_mute.account_id = $1 \
                    AND viewer_mute.target_account_id = status.account_id) \
                 AND (author.domain IS NULL OR NOT EXISTS ( \
                   SELECT 1 FROM account_domain_blocks domain_block WHERE domain_block.account_id = $1 \
                     AND domain_block.domain = author.domain)) \
                 AND (NOT EXISTS (SELECT 1 FROM users viewer_user WHERE viewer_user.account_id = $1 \
                   AND cardinality(viewer_user.chosen_languages) > 0) OR status.language = ANY( \
                     SELECT unnest(viewer_user.chosen_languages) FROM users viewer_user \
                     WHERE viewer_user.account_id = $1)))) \
               AND ($5::bigint IS NULL OR status.id < $5) \
               AND (($6::bigint IS NOT NULL AND status.id > $6) \
                 OR ($6 IS NULL AND ($7::bigint IS NULL OR status.id > $7))) \
             ORDER BY status.id {ordering} LIMIT $8"
        );
        let mut ids = sqlx::query_scalar(&query)
            .bind(viewer_account_id)
            .bind(options.local && !options.remote)
            .bind(options.remote && !options.local)
            .bind(options.only_media)
            .bind(options.max_id)
            .bind(options.min_id)
            .bind(options.since_id)
            .bind(options.limit.clamp(0, 40))
            .fetch_all(&self.pool)
            .await?;
        if options.min_id.is_some() {
            ids.reverse();
        }
        Ok(ids)
    }

    pub(crate) async fn rest_tag_timeline_ids(
        &self,
        tag_name: &str,
        viewer_account_id: Option<i64>,
        options: &TagTimelineOptions,
    ) -> sqlx::Result<Vec<i64>> {
        let tag_name = normalize_hashtag(tag_name);
        if !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM tags WHERE lower(name) = lower($1))",
        )
        .bind(&tag_name)
        .fetch_one(&self.pool)
        .await?
        {
            return Ok(Vec::new());
        }
        let ordering = if options.page.min_id.is_some() {
            "ASC"
        } else {
            "DESC"
        };
        let mut raw_any = Vec::new();
        for name in std::iter::once(tag_name.as_str()).chain(options.any.iter().map(String::as_str))
        {
            if !raw_any.contains(&name) {
                raw_any.push(name);
            }
            if raw_any.len() == 4 {
                break;
            }
        }
        let any = raw_any
            .into_iter()
            .map(normalize_hashtag)
            .collect::<Vec<_>>();
        let all = options
            .all
            .iter()
            .take(4)
            .map(|name| normalize_hashtag(name))
            .collect::<Vec<_>>();
        let none = options
            .none
            .iter()
            .take(4)
            .map(|name| normalize_hashtag(name))
            .collect::<Vec<_>>();
        let query = format!(
            "SELECT status.id FROM statuses status \
             JOIN accounts author ON author.id = status.account_id \
             WHERE status.deleted_at IS NULL AND status.visibility = 0 \
               AND author.suspended_at IS NULL AND author.silenced_at IS NULL \
               AND EXISTS (SELECT 1 FROM statuses_tags status_tag JOIN tags tag ON tag.id = status_tag.tag_id \
                 WHERE status_tag.status_id = status.id AND lower(tag.name) = ANY($2)) \
               AND NOT EXISTS (SELECT 1 FROM unnest($3::text[]) required(name) \
                 JOIN tags required_tag ON lower(required_tag.name) = required.name WHERE NOT EXISTS ( \
                 SELECT 1 FROM statuses_tags status_tag JOIN tags tag ON tag.id = status_tag.tag_id \
                 WHERE status_tag.status_id = status.id AND lower(tag.name) = required.name)) \
               AND NOT EXISTS (SELECT 1 FROM statuses_tags status_tag JOIN tags tag ON tag.id = status_tag.tag_id \
                 WHERE status_tag.status_id = status.id AND lower(tag.name) = ANY($4)) \
               AND (NOT $5 OR status.local = true OR status.uri IS NULL) \
               AND (NOT $6 OR status.local = false AND status.uri IS NOT NULL) \
               AND (NOT $7 OR EXISTS (SELECT 1 FROM media_attachments media \
                 WHERE media.status_id = status.id)) \
               AND ($1::bigint IS NULL OR ( \
                 NOT EXISTS (SELECT 1 FROM blocks viewer_block WHERE viewer_block.account_id = $1 \
                   AND viewer_block.target_account_id = status.account_id) \
                 AND NOT EXISTS (SELECT 1 FROM blocks author_block WHERE author_block.account_id = status.account_id \
                   AND author_block.target_account_id = $1) \
                  AND NOT EXISTS (SELECT 1 FROM mutes viewer_mute WHERE viewer_mute.account_id = $1 \
                    AND viewer_mute.target_account_id = status.account_id) \
                  AND (author.domain IS NULL OR NOT EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                    WHERE domain_block.account_id = $1 AND domain_block.domain = author.domain)))) \
               AND ($8::bigint IS NULL OR status.id < $8) \
               AND (($9::bigint IS NOT NULL AND status.id > $9) \
                 OR ($9 IS NULL AND ($10::bigint IS NULL OR status.id > $10))) \
             ORDER BY status.id {ordering} LIMIT $11"
        );
        let mut ids = sqlx::query_scalar(&query)
            .bind(viewer_account_id)
            .bind(any)
            .bind(all)
            .bind(none)
            .bind(options.page.local && !options.page.remote)
            .bind(options.page.remote && !options.page.local)
            .bind(options.page.only_media)
            .bind(options.page.max_id)
            .bind(options.page.min_id)
            .bind(options.page.since_id)
            .bind(options.page.limit.clamp(0, 40))
            .fetch_all(&self.pool)
            .await?;
        if options.page.min_id.is_some() {
            ids.reverse();
        }
        Ok(ids)
    }

    pub(crate) async fn rest_tag_exists(&self, tag_name: &str) -> sqlx::Result<bool> {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tags WHERE lower(name) = lower($1))")
            .bind(normalize_hashtag(tag_name))
            .fetch_one(&self.pool)
            .await
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn rest_home_timeline_ids(
        &self,
        account_id: i64,
        options: &TimelineOptions,
    ) -> sqlx::Result<Vec<i64>> {
        let ordering = if options.min_id.is_some() {
            "ASC"
        } else {
            "DESC"
        };
        let query = format!(
            "WITH authorized AS ( \
               SELECT status.*, author.silenced_at AS author_silenced_at, author.domain AS author_domain, \
                 viewer_follow.id AS follow_id, viewer_follow.languages AS follow_languages, \
                 viewer_follow.show_reblogs, source.account_id AS source_account_id, \
                 source_author.domain AS source_author_domain \
               FROM statuses status \
               JOIN accounts author ON author.id = status.account_id \
               LEFT JOIN follows viewer_follow ON viewer_follow.account_id = $1 \
                 AND viewer_follow.target_account_id = status.account_id \
               LEFT JOIN statuses source ON source.id = status.reblog_of_id AND source.deleted_at IS NULL \
               LEFT JOIN accounts source_author ON source_author.id = source.account_id \
               LEFT JOIN accounts viewer ON viewer.id = $1 \
               WHERE status.deleted_at IS NULL AND author.suspended_at IS NULL \
                 AND (source.id IS NULL OR source_author.suspended_at IS NULL) \
                 AND CASE \
                   WHEN status.account_id = $1 THEN true \
                   WHEN status.visibility IN (3, 4) THEN EXISTS (SELECT 1 FROM mentions mention \
                     WHERE mention.status_id = status.id AND mention.account_id = $1) \
                   WHEN status.visibility = 2 THEN viewer_follow.id IS NOT NULL OR EXISTS ( \
                     SELECT 1 FROM mentions mention WHERE mention.status_id = status.id \
                       AND mention.account_id = $1) \
                   WHEN status.visibility IN (0, 1) THEN NOT EXISTS (SELECT 1 FROM blocks block \
                     WHERE block.account_id = status.account_id AND block.target_account_id = $1) \
                     AND (viewer.domain IS NULL OR NOT EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                       WHERE domain_block.account_id = status.account_id AND domain_block.domain = viewer.domain)) \
                   ELSE false END \
             ), feed AS ( \
               SELECT status.id FROM authorized status \
               WHERE (status.account_id = $1 OR status.follow_id IS NOT NULL) \
                 AND (status.account_id = $1 OR status.visibility IN (0, 1, 2) OR ( \
                   status.visibility IN (3, 4) AND EXISTS (SELECT 1 FROM mentions mention \
                     WHERE mention.status_id = status.id AND mention.account_id = $1))) \
                 AND (status.account_id = $1 OR NOT EXISTS (SELECT 1 FROM list_accounts list_account \
                   JOIN lists exclusive_list ON exclusive_list.id = list_account.list_id \
                   WHERE exclusive_list.account_id = $1 AND exclusive_list.exclusive \
                     AND list_account.account_id = status.account_id)) \
                  AND (status.account_id = $1 OR COALESCE(cardinality(status.follow_languages), 0) = 0 \
                   OR status.language IS NULL OR status.language = ANY(status.follow_languages)) \
                 AND (status.account_id = $1 OR NOT status.reply \
                   OR (status.in_reply_to_id IS NOT NULL AND status.in_reply_to_account_id IS NOT NULL)) \
                 AND (status.account_id = $1 OR NOT status.reply \
                   OR status.in_reply_to_account_id = status.account_id \
                   OR status.in_reply_to_account_id = $1 \
                   OR EXISTS (SELECT 1 FROM follows reply_follow WHERE reply_follow.account_id = $1 \
                     AND reply_follow.target_account_id = status.in_reply_to_account_id)) \
                  AND (status.account_id = $1 OR status.reblog_of_id IS NULL \
                    OR (status.source_account_id IS NOT NULL AND status.show_reblogs)) \
                  AND (status.account_id = $1 OR NOT EXISTS (SELECT 1 FROM blocks viewer_block \
                    WHERE viewer_block.account_id = $1 AND viewer_block.target_account_id = status.account_id)) \
                  AND (status.account_id = $1 OR NOT EXISTS (SELECT 1 FROM blocks author_block \
                    WHERE author_block.account_id = status.account_id AND author_block.target_account_id = $1)) \
                   AND (status.account_id = $1 OR NOT EXISTS (SELECT 1 FROM mutes viewer_mute \
                    WHERE viewer_mute.account_id = $1 AND viewer_mute.target_account_id = status.account_id)) \
                 AND (status.account_id = $1 OR NOT EXISTS (SELECT 1 FROM mentions mention \
                   WHERE mention.status_id IN (status.id, status.reblog_of_id) AND NOT mention.silent \
                     AND (EXISTS (SELECT 1 FROM blocks mention_block WHERE mention_block.account_id = $1 \
                       AND mention_block.target_account_id = mention.account_id) \
                      OR EXISTS (SELECT 1 FROM mutes mention_mute WHERE mention_mute.account_id = $1 \
                        AND mention_mute.target_account_id = mention.account_id)))) \
                 AND (status.account_id = $1 OR status.source_account_id IS NULL OR ( \
                   NOT EXISTS (SELECT 1 FROM blocks source_block WHERE source_block.account_id = $1 \
                     AND source_block.target_account_id = status.source_account_id) \
                    AND NOT EXISTS (SELECT 1 FROM mutes source_mute WHERE source_mute.account_id = $1 \
                      AND source_mute.target_account_id = status.source_account_id) \
                   AND NOT EXISTS (SELECT 1 FROM blocks source_author_block \
                     WHERE source_author_block.account_id = status.source_account_id \
                       AND source_author_block.target_account_id = $1) \
                   AND (status.source_author_domain IS NULL OR NOT EXISTS ( \
                     SELECT 1 FROM account_domain_blocks source_domain_block \
                     WHERE source_domain_block.account_id = $1 \
                       AND source_domain_block.domain = status.source_author_domain)))) \
                  AND (status.account_id = $1 OR status.author_domain IS NULL OR NOT EXISTS ( \
                    SELECT 1 FROM account_domain_blocks author_domain_block \
                    WHERE author_domain_block.account_id = $1 \
                      AND author_domain_block.domain = status.author_domain)) \
               UNION \
               SELECT status.id FROM authorized status \
               WHERE status.account_id <> $1 AND status.visibility = 0 \
                 AND status.reblog_of_id IS NULL AND status.author_silenced_at IS NULL \
                 AND EXISTS (SELECT 1 FROM statuses_tags status_tag JOIN tag_follows tag_follow \
                   ON tag_follow.tag_id = status_tag.tag_id \
                   WHERE status_tag.status_id = status.id AND tag_follow.account_id = $1) \
                 AND NOT EXISTS (SELECT 1 FROM blocks viewer_block WHERE viewer_block.account_id = $1 \
                   AND viewer_block.target_account_id = status.account_id) \
                  AND NOT EXISTS (SELECT 1 FROM mutes viewer_mute WHERE viewer_mute.account_id = $1 \
                    AND viewer_mute.target_account_id = status.account_id) \
                 AND NOT EXISTS (SELECT 1 FROM mentions mention WHERE mention.status_id = status.id \
                   AND NOT mention.silent AND (EXISTS (SELECT 1 FROM blocks mention_block \
                     WHERE mention_block.account_id = $1 AND mention_block.target_account_id = mention.account_id) \
                    OR EXISTS (SELECT 1 FROM mutes mention_mute WHERE mention_mute.account_id = $1 \
                      AND mention_mute.target_account_id = mention.account_id))) \
                 AND (status.author_domain IS NULL OR NOT EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                   WHERE domain_block.account_id = $1 AND domain_block.domain = status.author_domain)) \
             ) SELECT status.id FROM feed status \
             WHERE ($2::bigint IS NULL OR status.id < $2) \
               AND (($3::bigint IS NOT NULL AND status.id > $3) \
                 OR ($3 IS NULL AND ($4::bigint IS NULL OR status.id > $4))) \
             ORDER BY status.id {ordering} LIMIT $5"
        );
        let mut ids = sqlx::query_scalar(&query)
            .bind(account_id)
            .bind(options.max_id)
            .bind(options.min_id)
            .bind(options.since_id)
            .bind(options.limit.clamp(0, 40))
            .fetch_all(&self.pool)
            .await?;
        if options.min_id.is_some() {
            ids.reverse();
        }
        Ok(ids)
    }

    pub(crate) async fn rest_list_timeline_ids(
        &self,
        account_id: i64,
        list_id: i64,
        options: &TimelineOptions,
    ) -> sqlx::Result<Option<Vec<i64>>> {
        let Some(replies_policy) = sqlx::query_scalar::<_, i32>(
            "SELECT replies_policy FROM lists WHERE id = $1 AND account_id = $2",
        )
        .bind(list_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let ordering = if options.min_id.is_some() {
            "ASC"
        } else {
            "DESC"
        };
        let query = REST_LIST_TIMELINE_SQL.replace("{ordering}", ordering);
        let mut ids = sqlx::query_scalar(&query)
            .bind(account_id)
            .bind(list_id)
            .bind(replies_policy)
            .bind(options.max_id)
            .bind(options.min_id)
            .bind(options.since_id)
            .bind(options.limit.clamp(0, 40))
            .fetch_all(&self.pool)
            .await?;
        if options.min_id.is_some() {
            ids.reverse();
        }
        Ok(Some(ids))
    }

    pub(crate) async fn rest_owned_list_exists(
        &self,
        account_id: i64,
        list_id: i64,
    ) -> sqlx::Result<bool> {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM lists WHERE id = $1 AND account_id = $2)")
            .bind(list_id)
            .bind(account_id)
            .fetch_one(&self.pool)
            .await
    }

    pub(crate) async fn rest_saved_status_rows(
        &self,
        account_id: i64,
        kind: SavedStatusKind,
        options: &SavedStatusesOptions,
    ) -> sqlx::Result<Vec<RestSavedStatusRow>> {
        let table = match kind {
            SavedStatusKind::Favourites => "favourites",
            SavedStatusKind::Bookmarks => "bookmarks",
        };
        let ordering = if options.min_id.is_some() {
            "ASC"
        } else {
            "DESC"
        };
        let query = format!(
            "SELECT saved.id AS cursor_id, saved.status_id FROM {table} saved \
             JOIN statuses status ON status.id = saved.status_id \
             WHERE saved.account_id = $1 AND status.deleted_at IS NULL \
               AND ($2::bigint IS NULL OR saved.id < $2) \
               AND (($3::bigint IS NOT NULL AND saved.id > $3) \
                 OR ($3 IS NULL AND ($4::bigint IS NULL OR saved.id > $4))) \
             ORDER BY saved.id {ordering} LIMIT $5"
        );
        let mut rows = sqlx::query_as(&query)
            .bind(account_id)
            .bind(options.max_id)
            .bind(options.min_id)
            .bind(options.since_id)
            .bind(options.limit.clamp(0, 40))
            .fetch_all(&self.pool)
            .await?;
        if options.min_id.is_some() {
            rows.reverse();
        }
        Ok(rows)
    }

    pub(crate) async fn rest_account_list_rows(
        &self,
        account_id: i64,
        kind: AccountListKind,
        options: &AccountListOptions,
    ) -> sqlx::Result<Vec<RestAccountListRow>> {
        let (table, expiry) = match kind {
            AccountListKind::Blocks => ("blocks", "NULL::timestamp"),
            AccountListKind::Mutes => ("mutes", "relationship.expires_at"),
        };
        let query = format!(
            "SELECT relationship.id AS cursor_id, relationship.target_account_id AS account_id, \
               {expiry} AS mute_expires_at FROM {table} relationship \
             JOIN accounts target ON target.id = relationship.target_account_id \
             WHERE relationship.account_id = $1 AND target.suspended_at IS NULL \
               AND ($2::bigint IS NULL OR relationship.id < $2) \
               AND ($3::bigint IS NULL OR relationship.id > $3) \
             ORDER BY relationship.id DESC LIMIT $4"
        );
        sqlx::query_as(&query)
            .bind(account_id)
            .bind(options.max_id)
            .bind(options.since_id)
            .bind(options.limit.clamp(0, 80))
            .fetch_all(&self.pool)
            .await
    }

    pub(crate) async fn rest_follow_collection_rows(
        &self,
        account_id: i64,
        viewer_account_id: Option<i64>,
        kind: FollowCollectionKind,
        options: &FollowCollectionOptions,
    ) -> sqlx::Result<Vec<RestFollowCollectionRow>> {
        let (owner_column, result_column) = match kind {
            FollowCollectionKind::Followers => ("follow.target_account_id", "follow.account_id"),
            FollowCollectionKind::Following => ("follow.account_id", "follow.target_account_id"),
        };
        let query = format!(
            "SELECT follow.id AS follow_id, {result_column} AS account_id \
             FROM follows follow \
             JOIN accounts owner ON owner.id = $1 \
             WHERE {owner_column} = $1 \
               AND owner.suspended_at IS NULL \
               AND ($2 = $1 OR NOT COALESCE(owner.hide_collections, false)) \
               AND ($2::bigint IS NULL OR NOT EXISTS (SELECT 1 FROM blocks owner_block \
                 WHERE owner_block.account_id = $1 AND owner_block.target_account_id = $2)) \
               AND ($2::bigint IS NULL OR $2 = $1 OR ( \
                 NOT EXISTS (SELECT 1 FROM blocks viewer_block WHERE viewer_block.account_id = $2 \
                   AND viewer_block.target_account_id = {result_column}) \
                 AND NOT EXISTS (SELECT 1 FROM blocks result_block WHERE result_block.account_id = {result_column} \
                   AND result_block.target_account_id = $2) \
                  AND NOT EXISTS (SELECT 1 FROM mutes viewer_mute WHERE viewer_mute.account_id = $2 \
                    AND viewer_mute.target_account_id = {result_column}))) \
               AND ($3::bigint IS NULL OR follow.id < $3) \
               AND ($4::bigint IS NULL OR follow.id > $4) \
             ORDER BY follow.id DESC LIMIT $5"
        );
        sqlx::query_as(&query)
            .bind(account_id)
            .bind(viewer_account_id)
            .bind(options.max_id)
            .bind(options.since_id)
            .bind(options.limit.clamp(0, 80))
            .fetch_all(&self.pool)
            .await
    }

    pub(crate) async fn rest_follow_request_rows(
        &self,
        account_id: i64,
        options: &FollowCollectionOptions,
    ) -> sqlx::Result<Vec<RestFollowCollectionRow>> {
        sqlx::query_as(
            "SELECT request.id AS follow_id, request.account_id \
             FROM follow_requests request \
             JOIN accounts requester ON requester.id = request.account_id \
             WHERE request.target_account_id = $1 AND requester.suspended_at IS NULL \
               AND ($2::bigint IS NULL OR request.id < $2) \
               AND ($3::bigint IS NULL OR request.id > $3) \
             ORDER BY request.id DESC LIMIT $4",
        )
        .bind(account_id)
        .bind(options.max_id)
        .bind(options.since_id)
        .bind(options.limit.clamp(0, 80))
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_favourited_by_rows(
        &self,
        status_id: i64,
        viewer_account_id: Option<i64>,
        options: &FollowCollectionOptions,
    ) -> sqlx::Result<Vec<RestFollowCollectionRow>> {
        sqlx::query_as(
            "SELECT favourite.id AS follow_id, favourite.account_id \
             FROM favourites favourite JOIN accounts account ON account.id = favourite.account_id \
             WHERE favourite.status_id = $1 AND account.suspended_at IS NULL \
               AND ($2::bigint IS NULL OR NOT EXISTS (SELECT 1 FROM blocks viewer_block \
                 WHERE viewer_block.account_id = $2 AND viewer_block.target_account_id = favourite.account_id)) \
               AND ($2::bigint IS NULL OR NOT EXISTS (SELECT 1 FROM blocks account_block \
                 WHERE account_block.account_id = favourite.account_id AND account_block.target_account_id = $2)) \
                AND ($2::bigint IS NULL OR NOT EXISTS (SELECT 1 FROM mutes viewer_mute \
                  WHERE viewer_mute.account_id = $2 AND viewer_mute.target_account_id = favourite.account_id)) \
               AND ($3::bigint IS NULL OR favourite.id < $3) \
               AND ($4::bigint IS NULL OR favourite.id > $4) \
             ORDER BY favourite.id DESC LIMIT $5",
        )
        .bind(status_id)
        .bind(viewer_account_id)
        .bind(options.max_id)
        .bind(options.since_id)
        .bind(options.limit.clamp(0, 80))
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_reblogged_by_rows(
        &self,
        status_id: i64,
        viewer_account_id: Option<i64>,
        options: &FollowCollectionOptions,
    ) -> sqlx::Result<Vec<RestFollowCollectionRow>> {
        sqlx::query_as(
            "SELECT reblog.id AS follow_id, reblog.account_id \
             FROM statuses reblog JOIN accounts account ON account.id = reblog.account_id \
             WHERE reblog.reblog_of_id = $1 AND reblog.deleted_at IS NULL \
               AND reblog.visibility IN (0, 1) AND account.suspended_at IS NULL \
               AND ($2::bigint IS NULL OR NOT EXISTS (SELECT 1 FROM blocks viewer_block \
                 WHERE viewer_block.account_id = $2 AND viewer_block.target_account_id = reblog.account_id)) \
               AND ($2::bigint IS NULL OR NOT EXISTS (SELECT 1 FROM blocks account_block \
                 WHERE account_block.account_id = reblog.account_id AND account_block.target_account_id = $2)) \
                AND ($2::bigint IS NULL OR NOT EXISTS (SELECT 1 FROM mutes viewer_mute \
                  WHERE viewer_mute.account_id = $2 AND viewer_mute.target_account_id = reblog.account_id)) \
               AND ($3::bigint IS NULL OR reblog.id < $3) \
               AND ($4::bigint IS NULL OR reblog.id > $4) \
             ORDER BY reblog.id DESC LIMIT $5",
        )
        .bind(status_id)
        .bind(viewer_account_id)
        .bind(options.max_id)
        .bind(options.since_id)
        .bind(options.limit.clamp(0, 80))
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_follow_collection_hidden(
        &self,
        account_id: i64,
        viewer_account_id: Option<i64>,
    ) -> sqlx::Result<bool> {
        sqlx::query_scalar(
            "SELECT account.suspended_at IS NOT NULL \
               OR ($2 IS DISTINCT FROM account.id AND COALESCE(account.hide_collections, false)) \
               OR ($2::bigint IS NOT NULL AND EXISTS (SELECT 1 FROM blocks owner_block \
                 WHERE owner_block.account_id = account.id AND owner_block.target_account_id = $2)) \
             FROM accounts account WHERE account.id = $1",
        )
        .bind(account_id)
        .bind(viewer_account_id)
        .fetch_one(&self.pool)
        .await
    }

    pub(crate) async fn rest_context_ids(
        &self,
        status_id: i64,
        ancestors_limit: i64,
        descendants_limit: i64,
        descendants_depth_limit: Option<i32>,
    ) -> sqlx::Result<(Vec<i64>, Vec<i64>)> {
        let mut ancestors = sqlx::query_scalar(
            "WITH RECURSIVE search_tree(id, in_reply_to_id, path) AS ( \
               SELECT id, in_reply_to_id, ARRAY[id] FROM statuses \
                 WHERE id = (SELECT in_reply_to_id FROM statuses WHERE id = $1) \
               UNION ALL \
               SELECT status.id, status.in_reply_to_id, path || status.id \
                 FROM search_tree JOIN statuses status ON status.id = search_tree.in_reply_to_id \
                 WHERE NOT status.id = ANY(path) \
             ) SELECT id FROM search_tree ORDER BY path LIMIT $2",
        )
        .bind(status_id)
        .bind(ancestors_limit)
        .fetch_all(&self.pool)
        .await?;
        ancestors.reverse();
        let descendants = sqlx::query_scalar(
            "WITH RECURSIVE search_tree(id, path) AS ( \
               SELECT id, ARRAY[id] FROM statuses WHERE id = $1 \
               UNION ALL \
               SELECT status.id, path || status.id \
                 FROM search_tree JOIN statuses status ON status.in_reply_to_id = search_tree.id \
                 WHERE ($3::int IS NULL OR array_length(path, 1) < $3 + 1) \
                   AND NOT status.id = ANY(path) \
             ) SELECT id FROM search_tree WHERE id <> $1 ORDER BY path LIMIT $2",
        )
        .bind(status_id)
        .bind(descendants_limit)
        .bind(descendants_depth_limit)
        .fetch_all(&self.pool)
        .await?;
        Ok((ancestors, descendants))
    }

    pub(crate) async fn rest_context_visible_status_ids(
        &self,
        ids: &[i64],
        viewer_account_id: Option<i64>,
    ) -> sqlx::Result<Vec<i64>> {
        Ok(self
            .rest_status_policy_rows(ids, viewer_account_id)
            .await?
            .into_iter()
            .filter(|row| {
                status_context_access(row.context_facts(viewer_account_id.is_some())).is_allowed()
            })
            .map(|row| row.id)
            .collect())
    }

    async fn rest_status_policy_rows(
        &self,
        ids: &[i64],
        viewer_account_id: Option<i64>,
    ) -> sqlx::Result<Vec<StatusPolicyRow>> {
        sqlx::query_as(
            "SELECT status.id, status.visibility, status.deleted_at IS NOT NULL AS status_deleted, \
               author.suspended_at IS NOT NULL AS author_suspended, \
               author.silenced_at IS NOT NULL AS author_silenced, \
               $2::bigint IS NOT NULL AND status.account_id = $2 AS viewer_is_author, \
               EXISTS (SELECT 1 FROM follows follow WHERE follow.account_id = $2 \
                 AND follow.target_account_id = status.account_id) AS viewer_follows_author, \
               EXISTS (SELECT 1 FROM mentions mention WHERE mention.status_id = status.id \
                 AND mention.account_id = $2) AS viewer_is_mentioned, \
               EXISTS (SELECT 1 FROM blocks block WHERE block.account_id = status.account_id \
                 AND block.target_account_id = $2) AS author_blocks_viewer, \
               EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                 JOIN accounts viewer ON viewer.id = $2 \
                 WHERE domain_block.account_id = status.account_id \
                   AND domain_block.domain = viewer.domain) AS author_domain_blocks_viewer, \
               EXISTS (SELECT 1 FROM blocks block WHERE block.account_id = $2 \
                 AND block.target_account_id = status.account_id) AS viewer_blocks_author, \
               EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                 WHERE domain_block.account_id = $2 \
                   AND domain_block.domain = author.domain) AS viewer_domain_blocks_author, \
                EXISTS (SELECT 1 FROM mutes mute WHERE mute.account_id = $2 \
                  AND mute.target_account_id = status.account_id) AS viewer_mutes_author \
             FROM statuses status JOIN accounts author ON author.id = status.account_id \
             WHERE status.id = ANY($1) ORDER BY status.id",
        )
        .bind(ids)
        .bind(viewer_account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_media_attachments(
        &self,
        status_ids: &[i64],
    ) -> sqlx::Result<Vec<MediaAttachment>> {
        sqlx::query_as::<_, MediaAttachment>(
            "SELECT media.id, media.account_id, media.status_id, media.type AS media_type, \
             media.processing, media.description, media.remote_url, media.file_content_type, \
             media.file_file_name, media.file_file_size, media.file_meta, \
             media.file_storage_schema_version, media.file_updated_at, media.scheduled_status_id, \
             media.shortcode, media.thumbnail_content_type, media.thumbnail_file_name, \
             media.thumbnail_file_size, media.thumbnail_remote_url, \
             media.thumbnail_storage_schema_version, media.thumbnail_updated_at, media.blurhash, \
             media.created_at, media.updated_at FROM statuses status \
             JOIN LATERAL (SELECT candidate.* FROM ( \
               SELECT attachment.*, ordering.position FROM unnest(status.ordered_media_attachment_ids) \
                 WITH ORDINALITY AS ordering(media_id, position) \
               JOIN media_attachments attachment ON attachment.id = ordering.media_id \
                 AND attachment.status_id = status.id \
               WHERE status.ordered_media_attachment_ids IS NOT NULL \
               UNION ALL \
               SELECT attachment.*, row_number() OVER (ORDER BY attachment.id) AS position \
               FROM media_attachments attachment WHERE attachment.status_id = status.id \
                 AND status.ordered_media_attachment_ids IS NULL \
             ) candidate ORDER BY candidate.position LIMIT 4) media ON true \
             WHERE status.id = ANY($1) AND status.deleted_at IS NULL \
             ORDER BY status.id, media.position",
        )
        .bind(status_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_media_attachments_by_ids(
        &self,
        status_id: i64,
        media_ids: &[i64],
    ) -> sqlx::Result<Vec<MediaAttachment>> {
        sqlx::query_as::<_, MediaAttachment>(
            "SELECT media.id, media.account_id, media.status_id, media.type AS media_type, \
             media.processing, media.description, media.remote_url, media.file_content_type, \
             media.file_file_name, media.file_file_size, media.file_meta, \
             media.file_storage_schema_version, media.file_updated_at, media.scheduled_status_id, \
             media.shortcode, media.thumbnail_content_type, media.thumbnail_file_name, \
             media.thumbnail_file_size, media.thumbnail_remote_url, \
             media.thumbnail_storage_schema_version, media.thumbnail_updated_at, media.blurhash, \
             media.created_at, media.updated_at FROM media_attachments media \
             WHERE media.status_id = $1 AND media.id = ANY($2::bigint[]) \
             ORDER BY array_position($2::bigint[], media.id)",
        )
        .bind(status_id)
        .bind(media_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_mention_rows(
        &self,
        status_ids: &[i64],
    ) -> sqlx::Result<Vec<RestMentionRow>> {
        sqlx::query_as::<_, RestMentionRow>(
            "SELECT mention.status_id, mention.account_id FROM mentions mention \
             JOIN statuses status ON status.id = mention.status_id AND status.deleted_at IS NULL \
             WHERE mention.status_id = ANY($1) AND mention.silent = false \
             ORDER BY mention.status_id, mention.id",
        )
        .bind(status_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_authorization_mention_rows(
        &self,
        status_ids: &[i64],
    ) -> sqlx::Result<Vec<RestMentionRow>> {
        sqlx::query_as::<_, RestMentionRow>(
            "SELECT mention.status_id, mention.account_id FROM mentions mention \
             JOIN statuses status ON status.id = mention.status_id AND status.deleted_at IS NULL \
             WHERE mention.status_id = ANY($1) ORDER BY mention.status_id, mention.id",
        )
        .bind(status_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_status_tag_rows(
        &self,
        status_ids: &[i64],
    ) -> sqlx::Result<Vec<RestStatusTagRow>> {
        sqlx::query_as::<_, RestStatusTagRow>(
            "SELECT statuses_tags.status_id, tag.id, tag.name, tag.display_name \
             FROM statuses_tags JOIN tags tag ON tag.id = statuses_tags.tag_id \
             JOIN statuses status ON status.id = statuses_tags.status_id AND status.deleted_at IS NULL \
             WHERE statuses_tags.status_id = ANY($1) ORDER BY statuses_tags.status_id, tag.id",
        )
        .bind(status_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_polls(&self, status_ids: &[i64]) -> sqlx::Result<Vec<Poll>> {
        sqlx::query_as::<_, Poll>(
            "SELECT poll.id, poll.account_id, poll.status_id, poll.options, poll.cached_tallies, \
             poll.votes_count, poll.voters_count, poll.multiple, poll.hide_totals, poll.expires_at \
             FROM polls poll JOIN statuses status ON status.id = poll.status_id \
               AND status.deleted_at IS NULL WHERE poll.status_id = ANY($1) ORDER BY poll.id",
        )
        .bind(status_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_poll_vote_rows(
        &self,
        poll_ids: &[i64],
        viewer_account_id: Option<i64>,
    ) -> sqlx::Result<Vec<RestPollVoteRow>> {
        sqlx::query_as::<_, RestPollVoteRow>(
            "SELECT poll_id, choice FROM poll_votes \
             WHERE poll_id = ANY($1) AND account_id = $2 ORDER BY poll_id, choice",
        )
        .bind(poll_ids)
        .bind(viewer_account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_quotes(&self, status_ids: &[i64]) -> sqlx::Result<Vec<Quote>> {
        sqlx::query_as::<_, Quote>(
            "SELECT quote.id, quote.account_id, quote.status_id, quote.quoted_account_id, \
             quote.quoted_status_id, quote.state, quote.activity_uri, quote.approval_uri, quote.legacy \
             FROM quotes quote JOIN statuses status ON status.id = quote.status_id \
               AND status.deleted_at IS NULL WHERE quote.status_id = ANY($1) ORDER BY quote.id",
        )
        .bind(status_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_status_quote_rows(
        &self,
        status_id: i64,
        viewer_account_id: Option<i64>,
        options: &FollowCollectionOptions,
    ) -> sqlx::Result<Vec<RestStatusQuoteRow>> {
        sqlx::query_as::<_, RestStatusQuoteRow>(
            "SELECT quote.id AS quote_id, quote.status_id \
             FROM quotes quote \
             JOIN statuses status ON status.id = quote.status_id \
               AND status.deleted_at IS NULL \
             WHERE quote.quoted_status_id = $1 AND quote.state = 1 \
                AND ($2::bigint IS NULL OR status.account_id = $2 OR ( \
                  NOT EXISTS (SELECT 1 FROM blocks block \
                    WHERE block.account_id = $2 AND block.target_account_id = status.account_id) \
                  AND NOT EXISTS (SELECT 1 FROM blocks author_block \
                    WHERE author_block.account_id = status.account_id \
                      AND author_block.target_account_id = $2) \
                  AND NOT EXISTS (SELECT 1 FROM mutes mute \
                    WHERE mute.account_id = $2 AND mute.target_account_id = status.account_id) \
                )) \
               AND ($3::bigint IS NULL OR quote.id < $3) \
               AND ($4::bigint IS NULL OR quote.id > $4) \
             ORDER BY status.id DESC, quote.id DESC LIMIT $5",
        )
        .bind(status_id)
        .bind(viewer_account_id)
        .bind(options.max_id)
        .bind(options.since_id)
        .bind(options.limit.clamp(0, 40))
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_status_quote_visible_ids(
        &self,
        status_ids: &[i64],
        viewer_account_id: Option<i64>,
    ) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(
            "SELECT status.id \
             FROM statuses status JOIN accounts author ON author.id = status.account_id \
             WHERE status.id = ANY($1) \
               AND (status.account_id = $2 OR ( \
                 ($2::bigint IS NULL OR NOT EXISTS ( \
                   SELECT 1 FROM account_domain_blocks domain_block \
                   WHERE domain_block.account_id = $2 AND domain_block.domain = author.domain)) \
                 AND (author.silenced_at IS NULL OR EXISTS ( \
                   SELECT 1 FROM follows follow \
                   WHERE follow.account_id = $2 AND follow.target_account_id = status.account_id)) \
               )) ORDER BY status.id",
        )
        .bind(status_ids)
        .bind(viewer_account_id)
        .fetch_all(&self.pool)
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

    pub(crate) async fn activitypub_outbox_statuses(
        &self,
        account_id: i64,
        viewer_account_id: Option<i64>,
        limit: i64,
        max_id: Option<i64>,
        min_id: Option<i64>,
        since_id: Option<i64>,
    ) -> sqlx::Result<Vec<Status>> {
        let ids = self
            .rest_account_status_ids(
                account_id,
                viewer_account_id,
                &AccountStatusesOptions {
                    max_id,
                    min_id,
                    since_id,
                    limit: limit.clamp(1, 20),
                    ..AccountStatusesOptions::default()
                },
            )
            .await?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as::<_, Status>(
            "SELECT id, account_id, application_id, text, spoiler_text, visibility, local, uri, url, language, \
             sensitive, reply, ordered_media_attachment_ids, conversation_id, in_reply_to_id, \
             in_reply_to_account_id, reblog_of_id, poll_id, quote_approval_policy, deleted_at, \
             edited_at, created_at, updated_at FROM statuses \
             WHERE id = ANY($1::bigint[]) AND deleted_at IS NULL \
             ORDER BY array_position($1::bigint[], id)",
        )
            .bind(&ids)
            .fetch_all(&self.pool)
            .await
    }

    pub(crate) async fn activitypub_remote_follower_ids(
        &self,
        account_id: i64,
        include_suspended: bool,
    ) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(
            "SELECT DISTINCT follow.account_id FROM follows follow \
             JOIN accounts follower ON follower.id = follow.account_id \
             WHERE follow.target_account_id = $1 AND follower.domain IS NOT NULL \
               AND follower.protocol = 1 AND ($2 OR follower.suspended_at IS NULL) \
             ORDER BY follow.account_id",
        )
        .bind(account_id)
        .bind(include_suspended)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_account_reach_account_ids(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(
            "WITH reach_cutoff AS (
                 SELECT CASE
                          WHEN suspended_at IS NOT NULL AND suspension_origin = 0
                            THEN suspended_at - interval '2 days'
                          ELSE clock_timestamp() - interval '2 days'
                        END AS cutoff
                   FROM accounts
                  WHERE id = $1
              ), recent_statuses AS (
                 SELECT status.id FROM statuses status
                 CROSS JOIN reach_cutoff
                  WHERE status.account_id = $1 AND status.deleted_at IS NULL
                    AND status.created_at >= reach_cutoff.cutoff
                    ORDER BY id DESC LIMIT 200
               ), recent_mentions AS (
                  SELECT DISTINCT ON (
                      COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url)
                  ) account.id AS account_id
                    FROM mentions mention
                    JOIN recent_statuses status ON status.id = mention.status_id
                    JOIN accounts account ON account.id = mention.account_id
                   WHERE account.domain IS NOT NULL AND account.protocol = 1
                     AND COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url) <> ''
                   ORDER BY COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url), account.id
                   LIMIT 2000
                 ), recent_follows AS (
                   SELECT DISTINCT ON (
                       COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url)
                   ) account.id AS account_id
                     FROM follows follow
                     JOIN accounts account ON account.id = follow.target_account_id
                     CROSS JOIN reach_cutoff
                    WHERE follow.account_id = $1
                      AND follow.created_at >= reach_cutoff.cutoff
                      AND account.domain IS NOT NULL AND account.protocol = 1
                     AND COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url) <> ''
                   ORDER BY COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url), account.id
                   LIMIT 2000
              ), recent_requests AS (
                  SELECT DISTINCT ON (
                      COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url)
                   ) account.id AS account_id
                     FROM follow_requests request
                     JOIN accounts account ON account.id = request.target_account_id
                     CROSS JOIN reach_cutoff
                    WHERE request.account_id = $1
                      AND request.created_at >= reach_cutoff.cutoff
                     AND account.domain IS NOT NULL AND account.protocol = 1
                     AND COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url) <> ''
                   ORDER BY COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url), account.id
                   LIMIT 2000
             ), reach AS (
                 SELECT follow.account_id
                   FROM follows follow
                  WHERE follow.target_account_id = $1
                 UNION ALL
                 SELECT report.account_id
                   FROM reports report
                  WHERE report.target_account_id = $1
                 UNION ALL SELECT account_id FROM recent_mentions
                 UNION ALL SELECT account_id FROM recent_follows
                 UNION ALL SELECT account_id FROM recent_requests
             )
             SELECT DISTINCT account.id
               FROM reach
               JOIN accounts account ON account.id = reach.account_id
               WHERE account.domain IS NOT NULL
                 AND account.protocol = 1
               ORDER BY account.id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_relay_inboxes(&self) -> sqlx::Result<Vec<String>> {
        sqlx::query_scalar(
            "SELECT inbox_url FROM relays
              WHERE state = 2 AND inbox_url <> ''
              ORDER BY inbox_url",
        )
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_remote_inboxes(&self) -> sqlx::Result<Vec<(String, String)>> {
        sqlx::query_as(
            "SELECT DISTINCT ON (remote.inbox_url) remote.inbox_url, remote.domain
               FROM (
                 SELECT account.id, account.domain,
                        COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url) AS inbox_url
                   FROM accounts account
                  WHERE account.domain IS NOT NULL
                    AND account.protocol = 1
                ) remote
              WHERE remote.inbox_url <> ''
              ORDER BY remote.inbox_url, remote.id",
        )
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_status_reach_account_ids(
        &self,
        status_id: i64,
        include_unsafe: bool,
    ) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(
            "SELECT DISTINCT reach.account_id FROM (
                 SELECT favourite.account_id
                   FROM favourites favourite
                    JOIN accounts account ON account.id = favourite.account_id
                    JOIN statuses status ON status.id = favourite.status_id
                   WHERE favourite.status_id = $1 AND ($2 OR status.visibility IN (0, 1))
                      AND account.domain IS NOT NULL AND account.protocol = 1
                      AND ($2 OR account.suspended_at IS NULL)
                   UNION ALL
                   SELECT reblog.account_id
                    FROM statuses reblog
                    JOIN accounts account ON account.id = reblog.account_id
                    JOIN statuses status ON status.id = reblog.reblog_of_id
                   WHERE reblog.reblog_of_id = $1 AND ($2 OR status.visibility IN (0, 1))
                     AND (reblog.deleted_at IS NULL OR ($2 AND reblog.deleted_at = status.deleted_at))
                      AND account.domain IS NOT NULL AND account.protocol = 1
                      AND ($2 OR account.suspended_at IS NULL)
                   UNION ALL
                   SELECT quote.account_id
                    FROM quotes quote
                    JOIN accounts account ON account.id = quote.account_id
                    JOIN statuses status ON status.id = quote.quoted_status_id
                   WHERE quote.quoted_status_id = $1 AND ($2 OR status.visibility IN (0, 1))
                      AND account.domain IS NOT NULL AND account.protocol = 1
                      AND ($2 OR account.suspended_at IS NULL)
                   UNION ALL
                   SELECT reply.account_id
                     FROM statuses reply
                     JOIN accounts account ON account.id = reply.account_id
                     JOIN statuses status ON status.id = reply.in_reply_to_id
                    WHERE reply.in_reply_to_id = $1 AND ($2 OR status.visibility IN (0, 1))
                      AND reply.deleted_at IS NULL
                      AND account.domain IS NOT NULL AND account.protocol = 1
                      AND ($2 OR account.suspended_at IS NULL)
                   UNION ALL
                    SELECT status.in_reply_to_account_id
                      FROM statuses status
                      JOIN accounts account ON account.id = status.in_reply_to_account_id
                      WHERE status.id = $1 AND status.visibility IN (0, 1)
                        AND status.in_reply_to_account_id IS NOT NULL
                        AND account.domain IS NOT NULL AND account.protocol = 1
                        AND ($2 OR account.suspended_at IS NULL)
                   UNION ALL
                    SELECT follow.account_id
                     FROM statuses status
                      JOIN accounts parent_author ON parent_author.id = status.in_reply_to_account_id
                                                   AND parent_author.domain IS NULL
                      JOIN follows follow ON follow.target_account_id = status.in_reply_to_account_id
                      JOIN accounts account ON account.id = follow.account_id
                      WHERE status.id = $1 AND status.visibility IN (0, 1)
                        AND status.in_reply_to_account_id IS NOT NULL
                        AND account.domain IS NOT NULL AND account.protocol = 1
                       AND ($2 OR account.suspended_at IS NULL)
                       AND NOT EXISTS (
                           SELECT 1 FROM account_domain_blocks domain_block
                            WHERE domain_block.account_id = status.account_id
                              AND domain_block.domain = account.domain)
                   UNION ALL
                   SELECT account.id
                    FROM quotes quote
                     JOIN accounts account ON account.id = quote.quoted_account_id
                    WHERE quote.status_id = $1 AND quote.quoted_account_id IS NOT NULL
                      AND account.domain IS NOT NULL AND account.protocol = 1
                      AND ($2 OR account.suspended_at IS NULL)
               ) reach
              ORDER BY reach.account_id",
        )
        .bind(status_id)
        .bind(include_unsafe)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_reblog_target_account_ids(
        &self,
        status_id: i64,
        include_unsafe: bool,
    ) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(
            "SELECT target.account_id
               FROM statuses status
               JOIN statuses target ON target.id = status.reblog_of_id
               JOIN accounts account ON account.id = target.account_id
              WHERE status.id = $1 AND status.reblog_of_id IS NOT NULL
                AND account.domain IS NOT NULL AND account.protocol = 1
                AND ($2 OR account.suspended_at IS NULL)",
        )
        .bind(status_id)
        .bind(include_unsafe)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_outbox_count(&self, account_id: i64) -> sqlx::Result<i64> {
        sqlx::query_scalar(
            "SELECT COALESCE( \
                (SELECT statuses_count FROM account_stats WHERE account_id = $1), \
                (SELECT count(*) FROM statuses WHERE account_id = $1 AND deleted_at IS NULL) \
              )",
        )
        .bind(account_id)
        .fetch_one(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_status_has_pending_quote(
        &self,
        status_id: i64,
    ) -> sqlx::Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM quotes WHERE status_id = $1 AND state = 0)",
        )
        .bind(status_id)
        .fetch_one(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_quote_target(
        &self,
        status_id: i64,
    ) -> sqlx::Result<Option<ActivityPubQuoteTarget>> {
        sqlx::query_as::<_, ActivityPubQuoteTarget>(
            "SELECT quote.id AS quote_id, quoted.id, quoted_account.id AS account_id, \
             (quoted.local IS TRUE OR quoted.uri IS NULL) AS local, quoted_account.domain IS NULL AS quoted_account_local, \
             quoted_account.id_scheme, quoted_account.username, quoted.uri, quoted.url, quote.approval_uri \
             FROM quotes quote \
             JOIN statuses status ON status.id = quote.status_id AND status.deleted_at IS NULL \
             JOIN statuses quoted ON quoted.id = quote.quoted_status_id AND quoted.deleted_at IS NULL \
             JOIN accounts quoted_account ON quoted_account.id = quoted.account_id \
             WHERE quote.status_id = $1 AND quote.state = 1 \
             ORDER BY quote.id LIMIT 1",
        )
        .bind(status_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_quote_authorization(
        &self,
        quoted_account_id: i64,
        quote_id: i64,
    ) -> sqlx::Result<Option<Quote>> {
        sqlx::query_as::<_, Quote>(
            "SELECT quote.id, quote.account_id, quote.status_id, quote.quoted_account_id, \
             quote.quoted_status_id, quote.state, quote.activity_uri, quote.approval_uri, quote.legacy \
             FROM quotes quote \
             JOIN statuses status ON status.id = quote.status_id AND status.deleted_at IS NULL \
             JOIN statuses quoted ON quoted.id = quote.quoted_status_id AND quoted.deleted_at IS NULL \
             WHERE quote.id = $1 AND quote.quoted_account_id = $2 AND quote.state = 1",
        )
        .bind(quote_id)
        .bind(quoted_account_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_reply_statuses(
        &self,
        account_id: i64,
        status_id: i64,
        only_other_accounts: bool,
        min_id: Option<i64>,
        limit: i64,
    ) -> sqlx::Result<Vec<Status>> {
        sqlx::query_as::<_, Status>(
            "SELECT reply.id, reply.account_id, reply.application_id, reply.text, reply.spoiler_text, \
             reply.visibility, reply.local, reply.uri, reply.url, reply.language, reply.sensitive, \
             reply.reply, reply.ordered_media_attachment_ids, reply.conversation_id, reply.in_reply_to_id, \
             reply.in_reply_to_account_id, reply.reblog_of_id, reply.poll_id, reply.quote_approval_policy, \
             reply.deleted_at, reply.edited_at, reply.created_at, reply.updated_at FROM statuses reply \
             JOIN accounts author ON author.id = reply.account_id \
             WHERE reply.in_reply_to_id = $2 AND reply.deleted_at IS NULL \
               AND reply.visibility IN (0, 1) \
               AND ($3 OR reply.account_id = $1) \
               AND (NOT $3 OR reply.account_id <> $1) \
               AND (NOT $3 OR author.suspended_at IS NULL) \
               AND ($4::bigint IS NULL OR reply.id > $4) \
             ORDER BY reply.id ASC LIMIT $5",
        )
        .bind(account_id)
        .bind(status_id)
        .bind(only_other_accounts)
        .bind(min_id)
        .bind(limit.clamp(1, 60))
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn activitypub_follow_account_ids(
        &self,
        account_id: i64,
        followers: bool,
        limit: i64,
        offset: i64,
    ) -> sqlx::Result<Vec<i64>> {
        let query = if followers {
            "SELECT follow.account_id FROM follows follow \
             WHERE follow.target_account_id = $1 \
             ORDER BY follow.id DESC LIMIT $2 OFFSET $3"
        } else {
            "SELECT follow.target_account_id FROM follows follow \
             WHERE follow.account_id = $1 \
             ORDER BY follow.id DESC LIMIT $2 OFFSET $3"
        };
        sqlx::query_scalar(query)
            .bind(account_id)
            .bind(limit.clamp(1, 12))
            .bind(offset.max(0))
            .fetch_all(&self.pool)
            .await
    }

    pub(crate) async fn activitypub_follow_count(
        &self,
        account_id: i64,
        followers: bool,
    ) -> sqlx::Result<i64> {
        let query = if followers {
            "SELECT COALESCE( \
                (SELECT followers_count FROM account_stats WHERE account_id = $1), \
                (SELECT count(*) FROM follows WHERE target_account_id = $1) \
              )"
        } else {
            "SELECT COALESCE( \
                (SELECT following_count FROM account_stats WHERE account_id = $1), \
                (SELECT count(*) FROM follows WHERE account_id = $1) \
              )"
        };
        sqlx::query_scalar(query)
            .bind(account_id)
            .fetch_one(&self.pool)
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

    pub async fn media_attachment(
        &self,
        account_id: i64,
        id: i64,
    ) -> sqlx::Result<Option<MediaAttachment>> {
        sqlx::query_as::<_, MediaAttachment>(
            "SELECT media.id, media.account_id, media.status_id, media.type AS media_type, \
             media.processing, media.description, media.remote_url, media.file_content_type, \
             media.file_file_name, media.file_file_size, media.file_meta, \
             media.file_storage_schema_version, media.file_updated_at, media.scheduled_status_id, \
             media.shortcode, media.thumbnail_content_type, media.thumbnail_file_name, \
             media.thumbnail_file_size, media.thumbnail_remote_url, \
             media.thumbnail_storage_schema_version, media.thumbnail_updated_at, media.blurhash, \
             media.created_at, media.updated_at FROM media_attachments media \
             WHERE media.id = $1 AND media.account_id = $2 AND media.status_id IS NULL",
        )
        .bind(id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn media_attachment_status(
        &self,
        id: i64,
    ) -> sqlx::Result<Option<(i64, bool)>> {
        sqlx::query_as::<_, (i64, bool)>(
            "SELECT media.status_id, status.id IS NULL OR status.deleted_at IS NOT NULL AS discarded \
             FROM media_attachments media \
             LEFT JOIN statuses status ON status.id = media.status_id \
             WHERE media.id = $1 AND media.status_id IS NOT NULL",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn user_can_manage_reports(&self, account_id: i64) -> sqlx::Result<bool> {
        let permissions = sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT role.id, role.permissions, everyone.permissions \
             FROM users account_user \
             JOIN accounts account ON account.id = account_user.account_id \
             JOIN user_roles role ON role.id = COALESCE(account_user.role_id, -99) \
             JOIN user_roles everyone ON everyone.id = -99 \
             WHERE account.id = $1 AND account.domain IS NULL \
               AND account.suspended_at IS NULL \
               AND account_user.confirmed_at IS NOT NULL \
               AND account_user.approved = true AND account_user.disabled = false",
        )
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(permissions.is_some_and(|(role_id, role, everyone)| {
            PermissionBits::effective(role_id, PermissionBits(role), PermissionBits(everyone))
                .contains(UserPermission::ManageReports)
        }))
    }

    pub(crate) async fn remote_media_attachment(
        &self,
        id: i64,
    ) -> sqlx::Result<Option<MediaAttachment>> {
        sqlx::query_as::<_, MediaAttachment>(
            "SELECT media.id, media.account_id, media.status_id, media.type AS media_type, \
             media.processing, media.description, media.remote_url, media.file_content_type, \
             media.file_file_name, media.file_file_size, media.file_meta, \
             media.file_storage_schema_version, media.file_updated_at, media.scheduled_status_id, \
             media.shortcode, media.thumbnail_content_type, media.thumbnail_file_name, \
             media.thumbnail_file_size, media.thumbnail_remote_url, \
             media.thumbnail_storage_schema_version, media.thumbnail_updated_at, media.blurhash, \
             media.created_at, media.updated_at FROM media_attachments media \
             JOIN statuses status ON status.id = media.status_id AND status.deleted_at IS NULL \
             WHERE media.id = $1 AND media.remote_url <> ''",
        )
        .bind(id)
        .fetch_optional(&self.pool)
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

    pub(crate) async fn rest_featured_tag_rows(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<RestFeaturedTagRow>> {
        sqlx::query_as::<_, RestFeaturedTagRow>(
            "SELECT featured.id, featured.name, tag.name AS tag_name, \
                    tag.display_name AS tag_display_name, featured.statuses_count, \
                    featured.last_status_at, account.username, account.domain \
             FROM featured_tags featured \
             JOIN accounts account ON account.id = featured.account_id \
             JOIN tags tag ON tag.id = featured.tag_id \
             WHERE featured.account_id = $1 ORDER BY featured.statuses_count DESC",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_featured_tag_suggestions(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<RestTagSuggestionRow>> {
        sqlx::query_as::<_, RestTagSuggestionRow>(
            "WITH recent_statuses AS ( \
               SELECT status.id FROM statuses status \
               WHERE status.account_id = $1 AND status.deleted_at IS NULL \
               ORDER BY status.id DESC LIMIT 1000 \
             ) SELECT tag.id, tag.name, tag.display_name, \
                    EXISTS (SELECT 1 FROM tag_follows follow \
                     WHERE follow.account_id = $1 AND follow.tag_id = tag.id) AS following \
             FROM tags tag \
             JOIN statuses_tags status_tag ON status_tag.tag_id = tag.id \
             JOIN recent_statuses status ON status.id = status_tag.status_id \
             WHERE NOT EXISTS (SELECT 1 FROM featured_tags featured \
               WHERE featured.account_id = $1 AND featured.tag_id = tag.id) \
             GROUP BY tag.id, tag.name, tag.display_name \
             ORDER BY count(*) DESC LIMIT 10",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_followed_tags(
        &self,
        account_id: i64,
        options: &FollowedTagsOptions,
    ) -> sqlx::Result<Vec<RestFollowedTagRow>> {
        sqlx::query_as::<_, RestFollowedTagRow>(
            "SELECT tag_follow.id AS tag_follow_id, tag.id, tag.name, tag.display_name, \
                    EXISTS (SELECT 1 FROM featured_tags featured \
                      WHERE featured.account_id = $1 AND featured.tag_id = tag.id) AS featuring \
             FROM tag_follows tag_follow JOIN tags tag ON tag.id = tag_follow.tag_id \
             WHERE tag_follow.account_id = $1 \
               AND ($2::bigint IS NULL OR tag_follow.id < $2) \
               AND ($3::bigint IS NULL OR tag_follow.id > $3) \
               AND ($4::bigint IS NULL OR tag_follow.id > $4) \
             ORDER BY tag_follow.id DESC LIMIT $5",
        )
        .bind(account_id)
        .bind(options.max_id)
        .bind(options.min_id)
        .bind(options.since_id)
        .bind(options.limit)
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

    pub async fn account_conversations_page(
        &self,
        account_id: i64,
        options: &TimelineOptions,
    ) -> sqlx::Result<Vec<AccountConversation>> {
        if options.min_id.is_some() {
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
                 WHERE account_conversation.account_id = $1 \
                   AND account_conversation.last_status_id IS NOT NULL \
                   AND ($2::bigint IS NULL OR account_conversation.last_status_id > $2) \
                   AND ($3::bigint IS NULL OR account_conversation.last_status_id < $3) \
                 ORDER BY account_conversation.last_status_id ASC LIMIT $4",
            )
            .bind(account_id)
            .bind(options.min_id)
            .bind(options.max_id)
            .bind(options.limit)
            .fetch_all(&self.pool)
            .await
        } else {
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
                 WHERE account_conversation.account_id = $1 \
                   AND account_conversation.last_status_id IS NOT NULL \
                   AND ($2::bigint IS NULL OR account_conversation.last_status_id < $2) \
                   AND ($3::bigint IS NULL OR account_conversation.last_status_id > $3) \
                 ORDER BY account_conversation.last_status_id DESC LIMIT $4",
            )
            .bind(account_id)
            .bind(options.max_id)
            .bind(options.since_id)
            .bind(options.limit)
            .fetch_all(&self.pool)
            .await
        }
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

    pub(crate) async fn rest_list_account_ids(
        &self,
        list_id: i64,
        max_id: Option<i64>,
        since_id: Option<i64>,
        limit: i64,
    ) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(
            "SELECT account.id \
             FROM accounts account \
             JOIN list_accounts list_account ON list_account.account_id = account.id \
             WHERE list_account.list_id = $1 \
               AND account.suspended_at IS NULL \
               AND ($2::bigint IS NULL OR account.id < $2) \
               AND ($3::bigint IS NULL OR account.id > $3) \
             ORDER BY CASE WHEN $4 = 0 THEN list_account.id END, \
                      CASE WHEN $4 <> 0 THEN account.id END DESC \
             LIMIT NULLIF($4::bigint, 0)",
        )
        .bind(list_id)
        .bind(max_id)
        .bind(since_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_account_lists(
        &self,
        owner_account_id: i64,
        member_account_id: i64,
    ) -> sqlx::Result<Vec<List>> {
        sqlx::query_as::<_, List>(
            "SELECT list.id, list.account_id, list.title, list.replies_policy, list.exclusive \
             FROM lists list \
             JOIN list_accounts list_account ON list_account.list_id = list.id \
             WHERE list.account_id = $1 AND list_account.account_id = $2 \
             ORDER BY list.id",
        )
        .bind(owner_account_id)
        .bind(member_account_id)
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
        self.notification_rows(account_id, &NotificationOptions::default(), false)
            .await
    }

    pub async fn notifications_including_filtered(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<Notification>> {
        let options = NotificationOptions {
            include_filtered: true,
            ..NotificationOptions::default()
        };
        self.notification_rows(account_id, &options, false).await
    }

    pub(crate) async fn rest_notifications(
        &self,
        account_id: i64,
        options: &NotificationOptions,
        grouped: bool,
    ) -> sqlx::Result<Vec<Notification>> {
        self.notification_rows(account_id, options, grouped).await
    }

    pub(crate) async fn rest_notification(
        &self,
        account_id: i64,
        notification_id: i64,
    ) -> sqlx::Result<Option<Notification>> {
        sqlx::query_as::<_, Notification>(
            "SELECT notification.id, notification.account_id, notification.activity_id, \
                    notification.activity_type, notification.from_account_id, \
                    notification.type AS notification_type, notification.group_key, \
                    notification.filtered, notification.created_at \
             FROM notifications notification \
             JOIN accounts sender ON sender.id = notification.from_account_id \
               AND sender.suspended_at IS NULL \
             WHERE notification.account_id = $1 AND notification.id = $2",
        )
        .bind(account_id)
        .bind(notification_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn rest_notification_by_group_key(
        &self,
        account_id: i64,
        group_key: &str,
    ) -> sqlx::Result<Option<Notification>> {
        let query = if group_key.starts_with("ungrouped-") {
            let Some(notification_id) = group_key
                .strip_prefix("ungrouped-")
                .and_then(|value| value.parse::<i64>().ok())
            else {
                return Ok(None);
            };
            return self.rest_notification(account_id, notification_id).await;
        } else {
            "SELECT notification.id, notification.account_id, notification.activity_id, \
                    notification.activity_type, notification.from_account_id, \
                    notification.type AS notification_type, notification.group_key, \
                    notification.filtered, notification.created_at \
             FROM notifications notification \
             JOIN accounts sender ON sender.id = notification.from_account_id \
               AND sender.suspended_at IS NULL \
             WHERE notification.account_id = $1 AND notification.group_key = $2 \
             ORDER BY notification.id DESC LIMIT 1"
        };
        sqlx::query_as::<_, Notification>(query)
            .bind(account_id)
            .bind(group_key)
            .fetch_optional(&self.pool)
            .await
    }

    async fn notification_rows(
        &self,
        account_id: i64,
        options: &NotificationOptions,
        grouped: bool,
    ) -> sqlx::Result<Vec<Notification>> {
        let grouped_types = grouped_notification_types(&options.grouped_types);
        let type_filter = match &options.types {
            Some(types) if types.is_empty() => Some(Vec::new()),
            Some(types) => notification_type_filter_with_exclusions(types, &options.exclude_types),
            None => notification_type_filter_with_exclusions(&[], &options.exclude_types),
        };
        let order = if options.min_id.is_some() {
            "ASC"
        } else {
            "DESC"
        };
        let cursor = if options.min_id.is_some() {
            "AND id > $9 AND ($8 IS NULL OR id < $8)"
        } else {
            "AND ($8 IS NULL OR id < $8) \
             AND ($10 IS NULL OR id > $10)"
        };
        let query = format!(
            "WITH base AS ( \
               SELECT notification.id, notification.account_id, notification.activity_id, \
                 notification.activity_type, notification.from_account_id, \
                 notification.type AS notification_type, notification.group_key, \
                 notification.filtered, notification.created_at \
               FROM notifications notification \
               JOIN accounts sender ON sender.id = notification.from_account_id \
                 AND sender.suspended_at IS NULL \
               WHERE notification.account_id = $1 \
                 AND ($2 OR notification.filtered = false) \
                 AND ($7 IS NULL OR notification.from_account_id = $7) \
             ), filtered AS ( \
               SELECT * FROM base \
               WHERE (NOT $4 OR notification_type = ANY($3)) \
                 {cursor} \
             ), ranked AS ( \
               SELECT id, account_id, activity_id, activity_type, from_account_id, \
                 notification_type, group_key, filtered, created_at, \
                 row_number() OVER (PARTITION BY CASE WHEN $5 \
                   THEN COALESCE(CASE WHEN notification_type = ANY($6) \
                     THEN CASE WHEN group_key ~ '[^[:space:]]' THEN group_key END END, \
                     'ungrouped-' || id) \
                   ELSE id::text END ORDER BY id {order}) AS group_rank \
               FROM filtered \
             ) SELECT id, account_id, activity_id, activity_type, from_account_id, \
                 notification_type, group_key, filtered, created_at FROM ranked \
               WHERE group_rank = 1 ORDER BY id {order} LIMIT $11",
        );
        sqlx::query_as::<_, Notification>(&query)
            .bind(account_id)
            .bind(options.include_filtered || options.account_id.is_some())
            .bind(type_filter.as_deref().unwrap_or_default())
            .bind(type_filter.is_some())
            .bind(grouped)
            .bind(grouped_types)
            .bind(options.account_id)
            .bind(options.max_id)
            .bind(options.min_id)
            .bind(options.since_id)
            .bind(options.limit)
            .fetch_all(&self.pool)
            .await
    }

    pub(crate) async fn rest_notification_groups(
        &self,
        account_id: i64,
        group_keys: &[String],
        page_min_id: i64,
        page_max_id: Option<i64>,
        page_max_exclusive: bool,
    ) -> sqlx::Result<Vec<RestNotificationGroupRow>> {
        let upper_bound = match page_max_id {
            Some(_) if page_max_exclusive => "AND id < $4",
            Some(_) => "AND id <= $4",
            None => "",
        };
        let query = format!(
            "SELECT key AS group_key, \
               (SELECT id FROM notifications WHERE account_id = $1 AND group_key = key \
                {upper_bound} ORDER BY id DESC LIMIT 1) AS most_recent_notification_id, \
               ARRAY(SELECT from_account_id FROM notifications \
                     WHERE account_id = $1 AND group_key = key {upper_bound} \
                     ORDER BY id DESC LIMIT 8)::bigint[] AS sample_account_ids, \
               (SELECT count(*) FROM notifications WHERE account_id = $1 \
                AND group_key = key {upper_bound}) AS notifications_count, \
               (SELECT id FROM notifications WHERE account_id = $1 AND group_key = key \
                AND id >= $3 ORDER BY id ASC LIMIT 1) AS page_min_id, \
               (SELECT created_at FROM notifications WHERE account_id = $1 AND group_key = key \
                {upper_bound} ORDER BY id DESC LIMIT 1) AS latest_page_notification_at \
             FROM unnest($2::text[]) AS key ORDER BY key",
        );
        let query = sqlx::query_as::<_, RestNotificationGroupRow>(&query)
            .bind(account_id)
            .bind(group_keys)
            .bind(page_min_id);
        if let Some(page_max_id) = page_max_id {
            query.bind(page_max_id).fetch_all(&self.pool).await
        } else {
            query.fetch_all(&self.pool).await
        }
    }

    pub(crate) async fn rest_notification_targets(
        &self,
        notification_ids: &[i64],
    ) -> sqlx::Result<Vec<RestNotificationTargetRow>> {
        sqlx::query_as::<_, RestNotificationTargetRow>(
            "SELECT n.id AS notification_id, \
               CASE effective.kind \
                 WHEN 'mention' THEN mention.status_id \
                 WHEN 'status' THEN n.activity_id \
                 WHEN 'update' THEN n.activity_id \
                 WHEN 'quoted_update' THEN n.activity_id \
                 WHEN 'reblog' THEN boost.reblog_of_id \
                 WHEN 'favourite' THEN favourite.status_id \
                 WHEN 'poll' THEN poll.status_id \
                 WHEN 'quote' THEN quote.status_id \
               END AS status_id, \
               CASE effective.kind \
                 WHEN 'added_to_collection' THEN collection_item.collection_id \
                 WHEN 'collection_update' THEN n.activity_id \
               END AS collection_id \
             FROM notifications n \
             CROSS JOIN LATERAL (SELECT COALESCE(n.type, CASE n.activity_type \
               WHEN 'Mention' THEN 'mention' WHEN 'Status' THEN 'reblog' \
               WHEN 'Follow' THEN 'follow' WHEN 'FollowRequest' THEN 'follow_request' \
               WHEN 'Favourite' THEN 'favourite' WHEN 'Poll' THEN 'poll' \
               WHEN 'Quote' THEN 'quote' END) AS kind) effective \
             LEFT JOIN mentions mention ON effective.kind = 'mention' AND mention.id = n.activity_id \
             LEFT JOIN statuses boost ON effective.kind = 'reblog' AND boost.id = n.activity_id \
             LEFT JOIN favourites favourite ON effective.kind = 'favourite' AND favourite.id = n.activity_id \
             LEFT JOIN polls poll ON effective.kind = 'poll' AND poll.id = n.activity_id \
             LEFT JOIN quotes quote ON effective.kind = 'quote' AND quote.id = n.activity_id \
             LEFT JOIN collection_items collection_item \
               ON effective.kind = 'added_to_collection' AND collection_item.id = n.activity_id \
             WHERE n.id = ANY($1) ORDER BY n.id DESC",
        )
        .bind(notification_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_severance_events(
        &self,
        ids: &[i64],
    ) -> sqlx::Result<Vec<RestSeveranceEventRow>> {
        sqlx::query_as::<_, RestSeveranceEventRow>(
            "SELECT account_event.id, event.type AS event_type, event.purged, event.target_name, \
                    account_event.followers_count, account_event.following_count, \
                    account_event.created_at \
             FROM account_relationship_severance_events account_event \
             JOIN relationship_severance_events event \
               ON event.id = account_event.relationship_severance_event_id \
             WHERE account_event.id = ANY($1) ORDER BY account_event.id",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_account_warnings(
        &self,
        ids: &[i64],
    ) -> sqlx::Result<Vec<RestAccountWarningRow>> {
        sqlx::query_as::<_, RestAccountWarningRow>(
            "SELECT warning.id, warning.action, warning.text, warning.status_ids, \
                    warning.created_at, warning.target_account_id, appeal.text AS appeal_text, \
                    appeal.approved_at AS appeal_approved_at, \
                    appeal.rejected_at AS appeal_rejected_at \
             FROM account_warnings warning \
             LEFT JOIN appeals appeal ON appeal.account_warning_id = warning.id \
             WHERE warning.id = ANY($1) ORDER BY warning.id",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_reports(&self, ids: &[i64]) -> sqlx::Result<Vec<Report>> {
        sqlx::query_as::<_, Report>(
            "SELECT id, account_id, target_account_id, action_taken_at, \
                    action_taken_by_account_id, application_id, assigned_account_id, category, \
                    comment, forwarded, rule_ids, status_ids, uri, created_at \
             FROM reports WHERE id = ANY($1) ORDER BY id",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_report_collections(
        &self,
        report_ids: &[i64],
    ) -> sqlx::Result<Vec<(i64, i64)>> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT report_id, collection_id FROM collection_reports \
             WHERE report_id = ANY($1) ORDER BY report_id, id",
        )
        .bind(report_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_annual_report_years(
        &self,
        ids: &[i64],
    ) -> sqlx::Result<Vec<(i64, i32)>> {
        sqlx::query_as::<_, (i64, i32)>(
            "SELECT id, year FROM generated_annual_reports WHERE id = ANY($1) ORDER BY id",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_instance_counts(&self) -> sqlx::Result<RestInstanceCountsRow> {
        sqlx::query_as::<_, RestInstanceCountsRow>(
            "SELECT \
               (SELECT count(*) FROM users user_record \
                JOIN accounts account ON account.id = user_record.account_id \
                WHERE user_record.confirmed_at IS NOT NULL AND account.suspended_at IS NULL \
               )::bigint AS user_count, \
               (SELECT COALESCE(sum(stats.statuses_count), 0) FROM account_stats stats \
                JOIN accounts account ON account.id = stats.account_id WHERE account.domain IS NULL \
               )::bigint AS status_count, \
               (SELECT count(*) FROM instances)::bigint AS domain_count, \
               COALESCE((SELECT permissions FROM user_roles WHERE id = -99), 0)::bigint \
                 AS everyone_permissions",
        )
        .fetch_one(&self.pool)
        .await
    }

    pub(crate) async fn rest_rule_rows(&self) -> sqlx::Result<Vec<RestRuleRow>> {
        sqlx::query_as::<_, RestRuleRow>(
            "SELECT rule.id, rule.text, rule.hint, translation.language, \
                    translation.text AS translated_text, translation.hint AS translated_hint \
             FROM rules rule LEFT JOIN rule_translations translation ON translation.rule_id = rule.id \
             WHERE rule.deleted_at IS NULL ORDER BY rule.priority, rule.id, translation.language",
        )
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_local_account_id_by_username(
        &self,
        username: &str,
    ) -> sqlx::Result<Option<i64>> {
        sqlx::query_scalar(
            "SELECT id FROM accounts WHERE domain IS NULL AND lower(username) = lower($1) LIMIT 1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
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

    pub async fn notification_policy_summary(&self, account_id: i64) -> sqlx::Result<(i64, i64)> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT COUNT(*)::bigint, COALESCE(SUM(pending.notifications_count), 0)::bigint \
             FROM ( \
               SELECT request.notifications_count \
               FROM notification_requests request \
               JOIN accounts sender ON sender.id = request.from_account_id \
                 AND sender.suspended_at IS NULL \
               WHERE request.account_id = $1 \
               LIMIT 100 \
             ) pending",
        )
        .bind(account_id)
        .fetch_one(&self.pool)
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
               request.notifications_count, request.created_at, request.updated_at FROM notification_requests request \
              JOIN accounts sender ON sender.id = request.from_account_id \
                AND sender.suspended_at IS NULL \
              LEFT JOIN statuses status ON status.id = request.last_status_id \
             WHERE request.account_id = $1 ORDER BY request.id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_notification_requests(
        &self,
        account_id: i64,
        max_id: Option<i64>,
        since_id: Option<i64>,
        min_id: Option<i64>,
        limit: i64,
    ) -> sqlx::Result<Vec<NotificationRequest>> {
        let mut requests = sqlx::query_as::<_, NotificationRequest>(
            "SELECT request.id, request.account_id, request.from_account_id, \
              CASE WHEN status.id IS NOT NULL AND status.deleted_at IS NULL \
                   THEN request.last_status_id END AS last_status_id, \
              request.notifications_count, request.created_at, request.updated_at \
             FROM notification_requests request \
             JOIN accounts sender ON sender.id = request.from_account_id \
               AND sender.suspended_at IS NULL \
             LEFT JOIN statuses status ON status.id = request.last_status_id \
             WHERE request.account_id = $1 \
               AND ($2::bigint IS NULL OR request.id < $2) \
               AND ($3::bigint IS NULL OR request.id > $3) \
               AND ($4::bigint IS NULL OR request.id > $4) \
             ORDER BY CASE WHEN $4::bigint IS NULL THEN request.id END DESC, \
                      CASE WHEN $4::bigint IS NOT NULL THEN request.id END ASC \
             LIMIT $5",
        )
        .bind(account_id)
        .bind(max_id)
        .bind(since_id)
        .bind(min_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        if min_id.is_some() {
            requests.reverse();
        }
        Ok(requests)
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

    pub(crate) async fn remote_domain_allowed(
        &self,
        domain: &str,
        limited_federation: bool,
    ) -> sqlx::Result<bool> {
        let domain = domain_policy_hostname(domain);
        if limited_federation {
            sqlx::query_scalar(
                "SELECT EXISTS (
                   SELECT 1 FROM domain_allows
                   WHERE lower(domain) = lower($1)
                 )",
            )
            .bind(domain)
            .fetch_one(&self.pool)
            .await
        } else {
            let blocks = self.matching_domain_blocks(&domain).await?;
            let rules = blocks
                .iter()
                .map(DomainBlock::policy_rule)
                .collect::<Vec<_>>();
            Ok(!global_domain_policy(&domain, &rules).blocks_federation())
        }
    }

    pub(crate) async fn remote_media_allowed(
        &self,
        domain: &str,
        limited_federation: bool,
    ) -> sqlx::Result<bool> {
        let domain = domain_policy_hostname(domain);
        if limited_federation {
            let allowed = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                   SELECT 1 FROM domain_allows
                   WHERE lower(domain) = lower($1)
                 )",
            )
            .bind(&domain)
            .fetch_one(&self.pool)
            .await?;
            if !allowed {
                return Ok(false);
            }
        }
        let blocks = self.matching_domain_blocks(&domain).await?;
        let rules = blocks
            .iter()
            .map(DomainBlock::policy_rule)
            .collect::<Vec<_>>();
        Ok(!global_domain_policy(&domain, &rules).rejects_media())
    }

    async fn matching_domain_blocks(&self, domain: &str) -> sqlx::Result<Vec<DomainBlock>> {
        sqlx::query_as::<_, DomainBlock>(
            "SELECT id, domain, severity, reject_media, reject_reports, private_comment,
                    public_comment, obfuscate
               FROM domain_blocks
              WHERE lower(domain) = lower(trim(trailing '.' FROM $1))
                 OR lower(trim(trailing '.' FROM $1)) LIKE '%.' || lower(domain)",
        )
        .bind(domain)
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
              item_count, original_number_of_items, language, uri, url, tag_id, created_at, updated_at \
             FROM collections WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn collection_items(&self, collection_id: i64) -> sqlx::Result<Vec<CollectionItem>> {
        sqlx::query_as::<_, CollectionItem>(
            "SELECT id, collection_id, account_id, position, state, activity_uri, approval_uri, object_uri, uri, \
             created_at, updated_at \
             FROM collection_items WHERE collection_id = $1 ORDER BY id",
        )
        .bind(collection_id)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_collections(&self, ids: &[i64]) -> sqlx::Result<Vec<Collection>> {
        sqlx::query_as::<_, Collection>(
            "SELECT id, account_id, name, description, description_html, local, sensitive, discoverable, \
                    item_count, original_number_of_items, language, uri, url, tag_id, created_at, updated_at \
             FROM collections WHERE id = ANY($1) ORDER BY id",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_account_collection_ids(
        &self,
        account_id: i64,
        viewer_account_id: Option<i64>,
        offset: i64,
        limit: i64,
    ) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(
            "SELECT collection.id \
             FROM collections collection \
             WHERE collection.account_id = $1 \
               AND ($2::bigint = $1 OR collection.discoverable) \
               AND ($2::bigint IS NULL OR ( \
                    NOT EXISTS (SELECT 1 FROM blocks block \
                                WHERE block.account_id = collection.account_id \
                                  AND block.target_account_id = $2) \
                    AND NOT EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                                    JOIN accounts viewer ON viewer.id = $2 \
                                    WHERE domain_block.account_id = collection.account_id \
                                      AND domain_block.domain = viewer.domain))) \
             ORDER BY collection.created_at DESC, collection.id DESC \
             OFFSET $3 LIMIT $4",
        )
        .bind(account_id)
        .bind(viewer_account_id)
        .bind(offset)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_account_in_collection_ids(
        &self,
        account_id: i64,
        offset: i64,
        limit: i64,
    ) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(
            "SELECT DISTINCT collection.id \
             FROM collections collection \
             JOIN collection_items item ON item.collection_id = collection.id \
             WHERE item.account_id = $1 \
             ORDER BY collection.id DESC OFFSET $2 LIMIT $3",
        )
        .bind(account_id)
        .bind(offset)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_collection_showable(
        &self,
        collection_id: i64,
        viewer_account_id: Option<i64>,
    ) -> sqlx::Result<Option<bool>> {
        sqlx::query_scalar(
            "SELECT NOT EXISTS (SELECT 1 FROM blocks block \
                 WHERE block.account_id = collection.account_id \
                   AND block.target_account_id = $2) \
                AND NOT EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                 JOIN accounts viewer ON viewer.id = $2 \
                 WHERE domain_block.account_id = collection.account_id \
                   AND domain_block.domain = viewer.domain) \
             FROM collections collection WHERE collection.id = $1",
        )
        .bind(collection_id)
        .bind(viewer_account_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub(crate) async fn rest_collection_items(
        &self,
        collection_ids: &[i64],
    ) -> sqlx::Result<Vec<CollectionItem>> {
        sqlx::query_as::<_, CollectionItem>(
            "SELECT id, collection_id, account_id, position, state, activity_uri, approval_uri, \
                    object_uri, uri, created_at, updated_at \
             FROM collection_items WHERE collection_id = ANY($1) ORDER BY collection_id, id",
        )
        .bind(collection_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_tags(&self, ids: &[i64]) -> sqlx::Result<Vec<Tag>> {
        sqlx::query_as::<_, Tag>(
            "SELECT id, name, display_name, usable, trendable, listable, last_status_at \
             FROM tags WHERE id = ANY($1) ORDER BY id",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_tagged_collections(
        &self,
        status_ids: &[i64],
    ) -> sqlx::Result<Vec<RestTaggedCollectionRow>> {
        sqlx::query_as::<_, RestTaggedCollectionRow>(
            "SELECT status_id, object_id AS collection_id FROM tagged_objects \
             WHERE status_id = ANY($1) AND ap_type = 'FeaturedCollection' \
               AND object_type = 'Collection' AND object_id IS NOT NULL \
             ORDER BY status_id, id",
        )
        .bind(status_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_filter_keywords(
        &self,
        filter_ids: &[i64],
    ) -> sqlx::Result<Vec<CustomFilterKeyword>> {
        sqlx::query_as::<_, CustomFilterKeyword>(
            "SELECT id, custom_filter_id, keyword, whole_word FROM custom_filter_keywords \
             WHERE custom_filter_id = ANY($1) ORDER BY custom_filter_id, id",
        )
        .bind(filter_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_filter_statuses(
        &self,
        filter_ids: &[i64],
    ) -> sqlx::Result<Vec<CustomFilterStatus>> {
        sqlx::query_as::<_, CustomFilterStatus>(
            "SELECT id, custom_filter_id, status_id FROM custom_filter_statuses \
             WHERE custom_filter_id = ANY($1) ORDER BY custom_filter_id, id",
        )
        .bind(filter_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_markers(
        &self,
        user_id: i64,
        timelines: &[String],
    ) -> sqlx::Result<Vec<Marker>> {
        sqlx::query_as::<_, Marker>(
            "SELECT timeline, last_read_id, lock_version, updated_at FROM markers \
             WHERE user_id = $1 AND timeline = ANY($2) ORDER BY timeline",
        )
        .bind(user_id)
        .bind(timelines)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_preview_cards(
        &self,
        status_ids: &[i64],
    ) -> sqlx::Result<Vec<RestPreviewCardRow>> {
        sqlx::query_as::<_, RestPreviewCardRow>(
            "SELECT pcs.status_id, pcs.url AS original_url, pc.id, pc.url, pc.title, \
                    pc.description, pc.language, pc.type AS card_type, pc.author_name, \
                    pc.author_url, pc.author_account_id, pc.unverified_author_account_id, \
                    pc.provider_name, pc.provider_url, pc.html, pc.width, pc.height, \
                    pc.image_file_name, pc.image_storage_schema_version, pc.image_description, \
                    pc.embed_url, pc.blurhash, pc.published_at \
             FROM preview_cards_statuses pcs \
             JOIN preview_cards pc ON pc.id = pcs.preview_card_id \
             WHERE pcs.status_id = ANY($1) ORDER BY pcs.status_id, pc.id",
        )
        .bind(status_ids)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_custom_emojis(
        &self,
        shortcodes: &[String],
        domains: &[String],
        include_local: bool,
    ) -> sqlx::Result<Vec<RestCustomEmojiRow>> {
        sqlx::query_as::<_, RestCustomEmojiRow>(
            "SELECT id, shortcode, domain, image_file_name, image_storage_schema_version, \
                    visible_in_picker \
             FROM custom_emojis \
             WHERE disabled = false AND shortcode = ANY($1) \
               AND (domain = ANY($2) OR ($3 AND domain IS NULL)) \
               AND image_file_name IS NOT NULL \
             ORDER BY domain NULLS FIRST, shortcode, id",
        )
        .bind(shortcodes)
        .bind(domains)
        .bind(include_local)
        .fetch_all(&self.pool)
        .await
    }

    pub(crate) async fn rest_listed_custom_emojis(
        &self,
    ) -> sqlx::Result<Vec<RestListedCustomEmojiRow>> {
        sqlx::query_as::<_, RestListedCustomEmojiRow>(
            "SELECT emoji.id, emoji.shortcode, emoji.domain, emoji.image_file_name, \
                    emoji.image_storage_schema_version, emoji.visible_in_picker, \
                    category.name AS category, \
                    COALESCE(category.featured_emoji_id = emoji.id, false) AS featured \
             FROM custom_emojis emoji \
             LEFT JOIN custom_emoji_categories category ON category.id = emoji.category_id \
             WHERE emoji.domain IS NULL AND emoji.disabled = false \
               AND emoji.visible_in_picker = true \
             ORDER BY emoji.id",
        )
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

    pub async fn activitypub_signature_key(
        &self,
        key_id: &str,
        origin: &str,
    ) -> sqlx::Result<Option<ActivityPubSignatureKey>> {
        if let Some(key) =
            sqlx::query_as::<_, (i64, i64, String, String, bool, Option<NaiveDateTime>)>(
                "SELECT id, account_id, uri, public_key, revoked, expires_at \
             FROM keypairs WHERE uri = $1",
            )
            .bind(key_id)
            .fetch_optional(&self.pool)
            .await?
        {
            return Ok(Some(ActivityPubSignatureKey {
                account_id: key.1,
                key_id: key.2,
                public_key: key.3,
                revoked: key.4,
                expires_at: key.5,
            }));
        }

        let origin = origin.trim_end_matches('/');
        let local_domain = Url::parse(origin)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        let account = if let Some((username, domain)) = parse_acct_key_id(key_id) {
            sqlx::query_as::<_, SignatureAccountRow>(
                "SELECT id, username, domain, uri, id_scheme, public_key \
                 FROM accounts \
                 WHERE username = $1 \
                   AND (lower(domain) = lower($2) OR (domain IS NULL AND lower($2) = lower($3))) \
                 ORDER BY id LIMIT 1",
            )
            .bind(username)
            .bind(domain)
            .bind(&local_domain)
            .fetch_optional(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, SignatureAccountRow>(
                "SELECT id, username, domain, uri, id_scheme, public_key \
                 FROM accounts \
                 WHERE (domain IS NOT NULL AND uri <> '' AND uri || '#main-key' = $1) \
                    OR (domain IS NULL AND ( \
                      $2 || '/users/' || username || '#main-key' = $1 \
                      OR (id_scheme = 1 AND $2 || '/ap/users/' || id::text || '#main-key' = $1) \
                      OR (id = -99 AND $2 || '/actor#main-key' = $1))) \
                 ORDER BY id LIMIT 1",
            )
            .bind(key_id)
            .bind(origin)
            .fetch_optional(&self.pool)
            .await?
        };
        let Some(account) = account else {
            return Ok(None);
        };

        if key_id.starts_with("acct:")
            && let Some(key) =
                sqlx::query_as::<_, (i64, String, String, bool, Option<NaiveDateTime>)>(
                    "SELECT id, uri, public_key, revoked, expires_at \
                 FROM keypairs WHERE account_id = $1 ORDER BY id LIMIT 1",
                )
                .bind(account.id)
                .fetch_optional(&self.pool)
                .await?
        {
            return Ok(Some(ActivityPubSignatureKey {
                account_id: account.id,
                key_id: key_id.to_owned(),
                public_key: key.2,
                revoked: key.3,
                expires_at: key.4,
            }));
        }

        Ok(Some(ActivityPubSignatureKey {
            account_id: account.id,
            key_id: key_id.to_owned(),
            public_key: account.public_key,
            revoked: false,
            expires_at: None,
        }))
    }

    pub async fn tombstone(&self, id: i64) -> sqlx::Result<Option<Tombstone>> {
        sqlx::query_as::<_, Tombstone>(
            "SELECT id, account_id, uri, by_moderator, created_at FROM tombstones WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    #[allow(clippy::too_many_lines)]
    pub async fn paperclip_metadata(
        &self,
        attachment: PaperclipAttachment,
        id: i64,
    ) -> sqlx::Result<Option<PaperclipMetadata>> {
        let row = match attachment {
            PaperclipAttachment::AccountAvatar => {
                sqlx::query_as::<_, PaperclipMetadataRow>(
                    "SELECT domain IS NOT NULL AS remote, avatar_storage_schema_version AS storage_schema_version, \
                     avatar_file_name AS file_name, avatar_content_type AS content_type, NULL::varchar AS variant \
                     FROM accounts WHERE id = $1 AND avatar_file_name IS NOT NULL",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
            }
            PaperclipAttachment::AccountHeader => {
                sqlx::query_as::<_, PaperclipMetadataRow>(
                    "SELECT domain IS NOT NULL AS remote, header_storage_schema_version AS storage_schema_version, \
                     header_file_name AS file_name, header_content_type AS content_type, NULL::varchar AS variant \
                     FROM accounts WHERE id = $1 AND header_file_name IS NOT NULL",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
            }
            PaperclipAttachment::MediaFile => {
                sqlx::query_as::<_, PaperclipMetadataRow>(
                    "SELECT false AS remote, file_storage_schema_version AS storage_schema_version, \
                     file_file_name AS file_name, file_content_type AS content_type, remote_url AS variant \
                     FROM media_attachments WHERE id = $1 AND file_file_name IS NOT NULL",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
            }
            PaperclipAttachment::MediaThumbnail => {
                sqlx::query_as::<_, PaperclipMetadataRow>(
                    "SELECT false AS remote, thumbnail_storage_schema_version AS storage_schema_version, \
                     thumbnail_file_name AS file_name, thumbnail_content_type AS content_type, remote_url AS variant \
                     FROM media_attachments WHERE id = $1 AND thumbnail_file_name IS NOT NULL",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
            }
            PaperclipAttachment::CustomEmojiImage => {
                sqlx::query_as::<_, PaperclipMetadataRow>(
                    "SELECT domain IS NOT NULL AS remote, image_storage_schema_version AS storage_schema_version, \
                     image_file_name AS file_name, image_content_type AS content_type, NULL::varchar AS variant \
                     FROM custom_emojis WHERE id = $1 AND image_file_name IS NOT NULL",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
            }
            PaperclipAttachment::PreviewCardImage => {
                sqlx::query_as::<_, PaperclipMetadataRow>(
                    "SELECT true AS remote, image_storage_schema_version AS storage_schema_version, \
                     image_file_name AS file_name, image_content_type AS content_type, NULL::varchar AS variant \
                     FROM preview_cards WHERE id = $1 AND image_file_name IS NOT NULL",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
            }
            PaperclipAttachment::PreviewCardProviderIcon => {
                sqlx::query_as::<_, PaperclipMetadataRow>(
                    "SELECT false AS remote, NULL::integer AS storage_schema_version, icon_file_name AS file_name, \
                     icon_content_type AS content_type, NULL::varchar AS variant \
                     FROM preview_card_providers WHERE id = $1 AND icon_file_name IS NOT NULL",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
            }
            PaperclipAttachment::SiteUploadFile => {
                sqlx::query_as::<_, PaperclipMetadataRow>(
                    "SELECT false AS remote, NULL::integer AS storage_schema_version, file_file_name AS file_name, \
                     file_content_type AS content_type, var AS variant \
                     FROM site_uploads WHERE id = $1 AND file_file_name IS NOT NULL",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
            }
        };
        Ok(row.map(|row| {
            let remote = if matches!(
                attachment,
                PaperclipAttachment::MediaFile | PaperclipAttachment::MediaThumbnail
            ) {
                !crate::paperclip::rails_blank(row.variant.as_deref().unwrap_or_default())
            } else {
                row.remote
            };
            PaperclipMetadata {
                attachment,
                id,
                remote,
                storage_schema_version: row.storage_schema_version,
                file_name: row.file_name,
                content_type: row.content_type,
                variant: (!matches!(
                    attachment,
                    PaperclipAttachment::MediaFile | PaperclipAttachment::MediaThumbnail
                ))
                .then_some(row.variant)
                .flatten(),
            }
        }))
    }
}

#[derive(sqlx::FromRow)]
struct PaperclipMetadataRow {
    remote: bool,
    storage_schema_version: Option<i32>,
    file_name: String,
    content_type: Option<String>,
    variant: Option<String>,
}

fn domain_policy_hostname(domain: &str) -> String {
    canonical_remote_host(domain).unwrap_or_else(|_| domain.trim_end_matches('.').to_owned())
}

fn normalize_hashtag(value: &str) -> String {
    const NON_ASCII: &str = "ÀÁÂÃÄÅàáâãäåĀāĂăĄąÇçĆćĈĉĊċČčÐðĎďĐđÈÉÊËèéêëĒēĔĕĖėĘęĚěĜĝĞğĠġĢģĤĥĦħÌÍÎÏìíîïĨĩĪīĬĭĮįİıĴĵĶķĸĹĺĻļĽľĿŀŁłÑñŃńŅņŇňŉŊŋÒÓÔÕÖØòóôõöøŌōŎŏŐőŔŕŖŗŘřŚśŜŝŞşŠšſŢţŤťŦŧÙÚÛÜùúûüŨũŪūŬŭŮůŰűŲųŴŵÝýÿŶŷŸŹźŻżŽž";
    const ASCII: &str = "AAAAAAaaaaaaAaAaAaCcCcCcCcCcDdDdDdEEEEeeeeEeEeEeEeEeGgGgGgGgHhHhIIIIiiiiIiIiIiIiIiJjKkkLlLlLlLlLlNnNnNnNnnNnOOOOOOooooooOoOoOoRrRrRrSsSsSsSssTtTtTtUUUUuuuuUuUuUuUuUuUuWwYyyYyYZzZzZz";
    value
        .nfkc()
        .flat_map(char::to_lowercase)
        .map(|character| {
            NON_ASCII
                .chars()
                .position(|candidate| candidate == character)
                .and_then(|index| ASCII.chars().nth(index))
                .unwrap_or(character)
        })
        .filter(|character| {
            character.is_alphanumeric()
                || matches!(
                    character,
                    '_' | '\u{00b7}' | '\u{30fb}' | '\u{200c}' | '\u{0e47}'..='\u{0e4e}'
                )
        })
        .collect()
}
