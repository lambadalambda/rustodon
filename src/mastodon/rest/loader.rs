use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use serde_json::Value;

use super::{
    AccountFieldProjection, AccountProjection, AccountRelationshipProjection,
    AccountRoleProjection, AccountWarningProjection, AnnouncementProjection,
    AnnouncementReactionProjection, AppealProjection, CollectionItemProjection,
    CollectionProjection, ConversationProjection, CredentialAccountProjection,
    CredentialRoleProjection, CustomEmojiProjection, FilterKeywordProjection, FilterProjection,
    FilterResultProjection, FilterStatusProjection, FollowedTagsPage,
    GroupedNotificationsProjection, HtmlFormatter, InstanceProjection, InstanceRuntimeConfig,
    MarkerProjection, MediaAttachmentProjection, MentionProjection, NotificationGroupProjection,
    NotificationOptions, NotificationProjection, NotificationRequestProjection,
    PollOptionProjection, PollProjection, PreviewCardProjection, QuoteProjection,
    QuoteTargetAccess, ReportProjection, RestAccountRow, RestStatusRow, RuleProjection,
    SeveranceEventProjection, StatusApplicationProjection, StatusEditProjection, StatusProjection,
    StatusQuotesPage, StatusViewerProjection, TagHistoryProjection, TagProjection,
    grouped_notification_types,
};
use crate::mastodon::StatusVisibility;
use crate::mastodon::policy::{
    AuthenticatedViewerFacts, AuthorRestriction, StatusAccessFacts, StatusAvailability,
    ViewerFacts, status_access,
};
use crate::mastodon::repository::normalize_hashtag;
use crate::mastodon::{
    AccountConversation, Notification, NotificationType, PermissionBits, Repository, UserPermission,
};

#[derive(Clone)]
pub struct RestProjectionLoader {
    repository: Repository,
    viewer_account_id: Option<i64>,
    local_domain: String,
}

#[derive(Debug)]
pub enum AccountSearchError {
    Database(sqlx::Error),
    RemoteResolutionUnsupported,
}

impl From<sqlx::Error> for AccountSearchError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[allow(clippy::too_many_lines)]
impl RestProjectionLoader {
    #[must_use]
    pub fn new(
        repository: Repository,
        viewer_account_id: Option<i64>,
        local_domain: impl Into<String>,
    ) -> Self {
        Self {
            repository,
            viewer_account_id,
            local_domain: local_domain.into(),
        }
    }

    /// Loads secret-free account serializer projections in requested order.
    ///
    /// # Errors
    ///
    /// Returns a database error when the read-only account query fails.
    pub async fn accounts(&self, ids: &[i64]) -> sqlx::Result<Vec<AccountProjection>> {
        let mut accounts = self.accounts_without_profile_mentions(ids).await?;
        let handles = accounts
            .iter()
            .filter(|account| account.local())
            .flat_map(|account| account_profile_handles(account, &self.local_domain))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let handle_rows = self.repository.rest_account_handles(&handles).await?;
        let mentioned_ids = handle_rows
            .iter()
            .map(|row| row.id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mentioned_accounts = self
            .accounts_without_profile_mentions(&mentioned_ids)
            .await?
            .into_iter()
            .map(|account| (account.id, account))
            .collect::<BTreeMap<_, _>>();
        let handle_ids = handle_rows
            .into_iter()
            .map(|row| (row.handle, row.id))
            .collect::<BTreeMap<_, _>>();
        for account in &mut accounts {
            account.profile_mentions = account_profile_handles(account, &self.local_domain)
                .into_iter()
                .filter_map(|handle| handle_ids.get(&handle))
                .filter_map(|id| mentioned_accounts.get(id).cloned())
                .collect();
        }
        Ok(accounts)
    }

    /// Loads the authenticated account's visibility-correct conversations.
    ///
    /// # Errors
    ///
    /// Returns a database error when conversation, account, or status loading fails.
    pub async fn conversations(
        &self,
        account_id: i64,
        options: &super::TimelineOptions,
    ) -> sqlx::Result<Vec<ConversationProjection>> {
        let rows = self
            .repository
            .account_conversations_page(account_id, options)
            .await?;
        let mut projections = Vec::with_capacity(rows.len());
        for row in rows {
            projections.push(self.conversation_projection(row).await?);
        }
        Ok(projections)
    }

    /// Loads one account-owned conversation with its participant and status graph.
    ///
    /// # Errors
    ///
    /// Returns a database error when conversation, account, or status loading fails.
    pub async fn conversation(
        &self,
        account_id: i64,
        conversation_id: i64,
    ) -> sqlx::Result<Option<ConversationProjection>> {
        let row = self
            .repository
            .account_conversations(account_id)
            .await?
            .into_iter()
            .find(|conversation| conversation.id == conversation_id);
        match row {
            Some(row) => Ok(Some(self.conversation_projection(row).await?)),
            None => Ok(None),
        }
    }

    async fn conversation_projection(
        &self,
        conversation: AccountConversation,
    ) -> sqlx::Result<ConversationProjection> {
        let participant_account_ids = if conversation.participant_account_ids.is_empty() {
            vec![conversation.account_id]
        } else {
            conversation.participant_account_ids.clone()
        };
        let accounts = self
            .accounts(&participant_account_ids)
            .await?
            .into_iter()
            .map(|account| (account.id, account))
            .collect::<BTreeMap<_, _>>();
        let last_status = match conversation.last_status_id {
            Some(id) => self.authorized_status(id).await?,
            None => None,
        };
        let participant_accounts = participant_account_ids
            .into_iter()
            .filter_map(|id| accounts.get(&id).cloned())
            .collect();
        Ok(ConversationProjection {
            id: conversation.id,
            unread: conversation.unread,
            participant_accounts,
            last_status,
        })
    }

    async fn accounts_without_profile_mentions(
        &self,
        ids: &[i64],
    ) -> sqlx::Result<Vec<AccountProjection>> {
        let requested = ids.iter().copied().collect::<BTreeSet<_>>();
        let rows = self
            .repository
            .rest_account_rows(ids, self.viewer_account_id)
            .await?;
        let moved_ids = rows
            .iter()
            .filter_map(|row| row.moved_to_account_id)
            .filter(|id| !requested.contains(id))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let moved_rows = self
            .repository
            .rest_account_rows(&moved_ids, self.viewer_account_id)
            .await?;
        let mut moved = moved_rows
            .into_iter()
            .map(|row| {
                (
                    row.id,
                    account_projection(row, None, self.viewer_account_id),
                )
            })
            .collect::<BTreeMap<_, _>>();
        self.hydrate_account_emojis(&mut moved).await?;
        let mut by_id = rows
            .into_iter()
            .map(|row| {
                let moved_account = row
                    .moved_to_account_id
                    .and_then(|id| moved.get(&id).cloned())
                    .map(Box::new);
                (
                    row.id,
                    account_projection(row, moved_account, self.viewer_account_id),
                )
            })
            .collect::<BTreeMap<_, _>>();
        self.hydrate_account_emojis(&mut by_id).await?;
        Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
    }

    /// Loads one account projection by ID.
    ///
    /// # Errors
    ///
    /// Returns a database error when the read-only account query fails.
    pub async fn account(&self, id: i64) -> sqlx::Result<Option<AccountProjection>> {
        Ok(self.accounts(&[id]).await?.into_iter().next())
    }

    /// Loads published announcements with their authenticated viewer state and serializer graph.
    ///
    /// # Errors
    ///
    /// Returns a database error when announcement or related projection loading fails.
    pub async fn announcements(
        &self,
        viewer_account_id: i64,
    ) -> sqlx::Result<Vec<AnnouncementProjection>> {
        let rows = self
            .repository
            .rest_announcements(viewer_account_id)
            .await?;
        let announcement_ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
        let status_ids = rows
            .iter()
            .flat_map(|row| row.status_ids.iter().flatten().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let statuses = self
            .preauthorized_statuses(&status_ids)
            .await?
            .into_iter()
            .filter(|status| matches!(status.visibility, 0 | 1))
            .map(|status| (status.id, status))
            .collect::<BTreeMap<_, _>>();
        let handles = mention_handles(rows.iter().map(|row| row.text.as_str()), &self.local_domain);
        let handle_rows = self.repository.rest_account_handles(&handles).await?;
        let accounts = self
            .accounts(&handle_rows.iter().map(|row| row.id).collect::<Vec<_>>())
            .await?
            .into_iter()
            .map(|account| (account.id, account))
            .collect::<BTreeMap<_, _>>();
        let account_ids_by_handle = handle_rows
            .into_iter()
            .map(|row| (row.handle, row.id))
            .collect::<BTreeMap<_, _>>();
        let emoji_demands = rows
            .iter()
            .map(|row| (None, emoji_shortcodes(std::iter::once(row.text.as_str()))))
            .collect::<Vec<_>>();
        let emojis = self.load_custom_emojis(&emoji_demands).await?;
        let reactions = self
            .repository
            .rest_announcement_reactions(&announcement_ids, viewer_account_id)
            .await?
            .into_iter()
            .fold(BTreeMap::<i64, Vec<_>>::new(), |mut grouped, row| {
                let custom_emoji = row.custom_emoji_id.and_then(|id| {
                    Some(CustomEmojiProjection {
                        id,
                        shortcode: row.shortcode?,
                        domain: row.domain,
                        file_name: row.image_file_name?,
                        storage_schema_version: row.image_storage_schema_version,
                        visible_in_picker: row.visible_in_picker?,
                        category: None,
                        featured: None,
                    })
                });
                grouped.entry(row.announcement_id).or_default().push(
                    AnnouncementReactionProjection {
                        name: row.name,
                        count: row.count,
                        me: row.me,
                        custom_emoji,
                    },
                );
                grouped
            });

        Ok(rows
            .into_iter()
            .map(|row| {
                let handles =
                    mention_handles(std::iter::once(row.text.as_str()), &self.local_domain);
                let mentions = handles
                    .iter()
                    .filter_map(|handle| account_ids_by_handle.get(handle))
                    .filter_map(|id| accounts.get(id).cloned())
                    .map(|account| MentionProjection { account })
                    .collect();
                let shortcodes = emoji_shortcodes(std::iter::once(row.text.as_str()));
                let announcement_emojis = project_emojis(None, &shortcodes, &emojis);
                let announcement_statuses = row
                    .status_ids
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|id| statuses.get(id).cloned())
                    .collect();
                AnnouncementProjection {
                    id: row.id,
                    text: row.text.clone(),
                    starts_at: row.starts_at,
                    ends_at: row.ends_at,
                    all_day: row.all_day,
                    published_at: row.published_at,
                    updated_at: row.updated_at,
                    read: row.read,
                    mentions,
                    statuses: announcement_statuses,
                    tags: hashtag_names(&row.text),
                    emojis: announcement_emojis,
                    reactions: reactions.get(&row.id).cloned().unwrap_or_default(),
                }
            })
            .collect())
    }

    /// Loads one report with the target account required by the REST serializer.
    ///
    /// # Errors
    ///
    /// Returns a database error when the report, collection, or account projection queries fail.
    pub async fn report(&self, id: i64) -> sqlx::Result<Option<ReportProjection>> {
        let Some(report) = self.repository.report(id).await? else {
            return Ok(None);
        };
        let Some(target_account) = self.account(report.target_account_id).await? else {
            return Ok(None);
        };
        let collection_ids = self
            .repository
            .rest_report_collections(&[id])
            .await?
            .into_iter()
            .map(|(_, collection_id)| collection_id)
            .collect();
        Ok(Some(ReportProjection {
            id: report.id,
            action_taken_at: report.action_taken_at,
            category: report.category.0,
            comment: report.comment,
            forwarded: report.forwarded,
            created_at: report.created_at,
            status_ids: report.status_ids,
            rule_ids: report.rule_ids,
            collection_ids,
            target_account,
        }))
    }

    /// Loads the credential representation for one local account.
    ///
    /// # Errors
    ///
    /// Returns a database error when either read-only projection query fails.
    pub async fn credential_account(
        &self,
        user_id: i64,
        account_id: i64,
    ) -> sqlx::Result<Option<CredentialAccountProjection>> {
        let Some(mut account) = self.account(account_id).await? else {
            return Ok(None);
        };
        let Some(row) = self
            .repository
            .rest_credential_row(user_id, account_id)
            .await?
        else {
            return Ok(None);
        };
        let settings = row
            .settings
            .as_deref()
            .and_then(|settings| serde_json::from_str::<Value>(settings).ok())
            .unwrap_or_else(|| Value::Object(serde_json::Map::default()));
        account.noindex = Some(
            settings
                .get("noindex")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        );
        account.roles = Some(if row.role_highlighted {
            vec![AccountRoleProjection {
                id: row.role_id,
                name: row.role_name.clone(),
                color: row.role_color.clone(),
            }]
        } else {
            Vec::new()
        });
        let privacy = settings
            .get("default_privacy")
            .and_then(Value::as_str)
            .map_or_else(
                || if account.locked { "private" } else { "public" }.to_owned(),
                str::to_owned,
            );
        let sensitive = settings
            .get("default_sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let language = settings
            .get("default_language")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let quote_policy = settings
            .get("default_quote_policy")
            .and_then(Value::as_str)
            .unwrap_or("public")
            .to_owned();
        let permissions = PermissionBits::effective(
            row.role_id,
            PermissionBits(row.role_permissions),
            PermissionBits(row.everyone_permissions),
        );
        Ok(Some(CredentialAccountProjection {
            account,
            privacy,
            sensitive,
            language,
            follow_requests_count: row.follow_requests_count,
            attribution_domains: row.attribution_domains,
            quote_policy,
            role: CredentialRoleProjection {
                id: row.role_id,
                name: row.role_name,
                permissions: permissions.raw(),
                color: row.role_color,
                highlighted: row.role_highlighted,
                collection_limit: row.collection_limit,
            },
        }))
    }

    /// Loads the authenticated user's REST preference values without credential fields.
    ///
    /// # Errors
    ///
    /// Returns a database error when the exact user/account settings row cannot be loaded.
    pub async fn preferences(
        &self,
        user_id: i64,
        account_id: i64,
    ) -> sqlx::Result<Option<super::PreferencesProjection>> {
        let Some(row) = self
            .repository
            .rest_preferences_row(user_id, account_id)
            .await?
        else {
            return Ok(None);
        };
        let settings = row
            .settings
            .as_deref()
            .and_then(|settings| serde_json::from_str::<Value>(settings).ok())
            .unwrap_or_else(|| Value::Object(serde_json::Map::default()));
        let setting_string = |key: &str| settings.get(key).and_then(Value::as_str);
        Ok(Some(super::PreferencesProjection {
            posting_default_visibility: setting_string("default_privacy").map_or_else(
                || if row.locked { "private" } else { "public" }.to_owned(),
                str::to_owned,
            ),
            posting_default_sensitive: settings
                .get("default_sensitive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            posting_default_language: setting_string("default_language")
                .filter(|language| !language.is_empty())
                .or(row.locale.as_deref().filter(|locale| !locale.is_empty()))
                .unwrap_or("en")
                .to_owned(),
            posting_default_quote_policy: setting_string("default_quote_policy")
                .unwrap_or("public")
                .to_owned(),
            reading_default_sensitive_media: setting_string("web.display_media")
                .unwrap_or("default")
                .to_owned(),
            reading_default_sensitive_text: settings
                .get("web.expand_content_warnings")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            reading_autoplay_gifs: settings
                .get("web.auto_play")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }))
    }

    /// Loads relationship decorations in requested target order.
    ///
    /// # Errors
    ///
    /// Returns a database error when the read-only relationship query fails.
    pub async fn relationships(
        &self,
        target_account_ids: &[i64],
        with_suspended: bool,
    ) -> sqlx::Result<Vec<AccountRelationshipProjection>> {
        let Some(viewer_account_id) = self.viewer_account_id else {
            return Ok(Vec::new());
        };
        let rows = self
            .repository
            .rest_relationship_rows(viewer_account_id, target_account_ids, with_suspended)
            .await?;
        let mut by_id = rows
            .into_iter()
            .map(|row| {
                (
                    row.target_account_id,
                    AccountRelationshipProjection {
                        target_account_id: row.target_account_id,
                        following: row.following,
                        showing_reblogs: row.showing_reblogs,
                        notifying: row.notifying,
                        languages: row.languages,
                        followed_by: row.followed_by,
                        blocking: row.blocking,
                        blocked_by: row.blocked_by,
                        muting: row.muting,
                        muting_notifications: row.muting_notifications,
                        muting_expires_at: row.muting_expires_at,
                        requested: row.requested,
                        requested_by: row.requested_by,
                        domain_blocking: row.domain_blocking,
                        endorsed: row.endorsed,
                        note: row.note,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        Ok(target_account_ids
            .iter()
            .filter_map(|id| by_id.remove(id))
            .collect())
    }

    /// Resolves one already-stored account handle without `WebFinger`.
    ///
    /// # Errors
    ///
    /// Returns a database error when account resolution or projection loading fails.
    pub async fn lookup_account(&self, handle: &str) -> sqlx::Result<Option<AccountProjection>> {
        let handle = handle.trim().trim_start_matches('@');
        let mut parts = handle.split('@');
        let Some(username) = parts.next().filter(|username| !username.is_empty()) else {
            return Ok(None);
        };
        let domain = match (parts.next(), parts.next()) {
            (None, None) => None,
            (Some(domain), None) if !domain.is_empty() => {
                (!domain.eq_ignore_ascii_case(&self.local_domain)).then_some(domain)
            }
            _ => return Ok(None),
        };
        let Some(id) = self
            .repository
            .rest_account_id_by_handle(username, domain)
            .await?
        else {
            return Ok(None);
        };
        self.account(id).await
    }

    /// Searches stored accounts using Mastodon's database search semantics.
    ///
    /// Network resolution is intentionally not performed by this read-only loader;
    /// exact local and already-stored remote rows still participate in the search.
    ///
    /// # Errors
    ///
    /// Returns a database error when exact-match, search, or projection loading fails.
    pub async fn account_search(
        &self,
        query: Option<&str>,
        resolve: bool,
        following: bool,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<AccountProjection>, AccountSearchError> {
        let query = query.unwrap_or_default().trim();
        let query = query.strip_prefix('@').unwrap_or(query);
        if query.is_empty() || limit < 1 {
            return Ok(Vec::new());
        }

        let exact_handle = if offset == 0 {
            complete_account_handle(query, &self.local_domain)
        } else {
            None
        };
        if resolve && exact_handle.is_some_and(|(_, domain)| domain.is_some()) {
            return Err(AccountSearchError::RemoteResolutionUnsupported);
        }
        let stored_exact_id = if let Some((username, domain)) = exact_handle {
            self.repository
                .rest_account_id_by_handle(username, domain)
                .await?
        } else {
            None
        };
        let exact_id = if offset == 0 { stored_exact_id } else { None };
        let exact_id = if following {
            match (exact_id, self.viewer_account_id) {
                (Some(id), Some(viewer))
                    if self.repository.rest_account_following(viewer, id).await? =>
                {
                    Some(id)
                }
                _ => None,
            }
        } else {
            exact_id
        };
        let exact_count = i64::from(exact_id.is_some());
        let search_limit = limit.saturating_sub(exact_count);
        let terms = query
            .split_once('@')
            .filter(|(_, domain)| domain.eq_ignore_ascii_case(&self.local_domain))
            .map_or(query, |(username, _)| username);
        let tsquery = account_search_tsquery(terms);
        let search_ids = if search_limit > 0 {
            self.repository
                .rest_account_search_ids(
                    &tsquery,
                    self.viewer_account_id,
                    following,
                    search_limit,
                    offset,
                )
                .await?
        } else {
            Vec::new()
        };
        let mut seen = BTreeSet::new();
        let account_ids = exact_id
            .into_iter()
            .chain(search_ids)
            .filter(|id| seen.insert(*id))
            .collect::<Vec<_>>();
        self.accounts(&account_ids).await.map_err(Into::into)
    }

    async fn preauthorized_statuses(&self, ids: &[i64]) -> sqlx::Result<Vec<StatusProjection>> {
        let root_rows = self
            .repository
            .rest_status_rows(ids, self.viewer_account_id)
            .await?;
        let reblog_ids = root_rows
            .iter()
            .filter_map(|row| row.reblog_of_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let nested_rows = self
            .repository
            .rest_status_rows(&reblog_ids, self.viewer_account_id)
            .await?;
        let first_rows = root_rows
            .iter()
            .chain(&nested_rows)
            .cloned()
            .collect::<Vec<_>>();
        let first_status_ids = first_rows.iter().map(|row| row.id).collect::<Vec<_>>();
        let quotes = self.repository.rest_quotes(&first_status_ids).await?;
        let quote_target_ids = quotes
            .iter()
            .filter_map(|quote| quote.quoted_status_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let quote_target_rows = self
            .repository
            .rest_status_rows(&quote_target_ids, self.viewer_account_id)
            .await?;
        let nested_quotes = self.repository.rest_quotes(&quote_target_ids).await?;
        let nested_quote_target_ids = nested_quotes
            .iter()
            .filter_map(|quote| quote.quoted_status_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let nested_quote_target_rows = self
            .repository
            .rest_status_rows(&nested_quote_target_ids, self.viewer_account_id)
            .await?;
        let all_rows = first_rows
            .iter()
            .chain(&quote_target_rows)
            .chain(&nested_quote_target_rows)
            .cloned()
            .collect::<Vec<_>>();
        let all_status_ids = all_rows.iter().map(|row| row.id).collect::<Vec<_>>();
        let preview_card_rows = self.repository.rest_preview_cards(&all_status_ids).await?;
        let tagged_collection_rows = self
            .repository
            .rest_tagged_collections(&all_status_ids)
            .await?;
        let tagged_collection_ids = tagged_collection_rows
            .iter()
            .map(|row| row.collection_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let tagged_collections_by_id = self
            .collections(&tagged_collection_ids)
            .await?
            .into_iter()
            .map(|collection| (collection.id, collection))
            .collect::<BTreeMap<_, _>>();
        let tagged_collections = tagged_collection_rows.into_iter().fold(
            BTreeMap::<i64, Vec<_>>::new(),
            |mut grouped, row| {
                if let Some(collection) = tagged_collections_by_id.get(&row.collection_id) {
                    grouped
                        .entry(row.status_id)
                        .or_default()
                        .push(collection.clone());
                }
                grouped
            },
        );
        let mut account_ids = all_rows
            .iter()
            .map(|row| row.account_id)
            .collect::<BTreeSet<_>>();
        let mention_rows = self.repository.rest_mention_rows(&all_status_ids).await?;
        let authorization_mention_rows = self
            .repository
            .rest_authorization_mention_rows(&all_status_ids)
            .await?;
        account_ids.extend(mention_rows.iter().map(|row| row.account_id));
        account_ids.extend(
            preview_card_rows
                .iter()
                .filter_map(|row| row.author_account_id),
        );
        let accounts = self
            .accounts(&account_ids.into_iter().collect::<Vec<_>>())
            .await?
            .into_iter()
            .map(|account| (account.id, account))
            .collect::<BTreeMap<_, _>>();
        let preview_cards = preview_card_rows
            .into_iter()
            .map(|row| {
                (
                    row.status_id,
                    PreviewCardProjection {
                        original_url: row.original_url,
                        id: row.id,
                        url: row.url,
                        title: row.title,
                        description: row.description,
                        language: row.language,
                        card_type: row.card_type,
                        author_name: row.author_name,
                        author_url: row.author_url,
                        author_account: row
                            .author_account_id
                            .and_then(|id| accounts.get(&id).cloned()),
                        unverified_author_account_id: row.unverified_author_account_id,
                        provider_name: row.provider_name,
                        provider_url: row.provider_url,
                        html: row.html,
                        width: row.width,
                        height: row.height,
                        image_file_name: row.image_file_name,
                        image_storage_schema_version: row.image_storage_schema_version,
                        image_description: row.image_description,
                        embed_url: row.embed_url,
                        blurhash: row.blurhash,
                        published_at: row.published_at,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let media = self
            .repository
            .rest_media_attachments(&all_status_ids)
            .await?
            .into_iter()
            .fold(BTreeMap::<i64, Vec<_>>::new(), |mut grouped, media| {
                if let Some(status_id) = media.status_id {
                    grouped.entry(status_id).or_default().push(media);
                }
                grouped
            });
        let mentions = mention_rows.into_iter().fold(
            BTreeMap::<i64, Vec<_>>::new(),
            |mut grouped, mention| {
                grouped.entry(mention.status_id).or_default().push(mention);
                grouped
            },
        );
        let authorization_mentions = authorization_mention_rows.into_iter().fold(
            BTreeMap::<i64, Vec<_>>::new(),
            |mut grouped, mention| {
                grouped.entry(mention.status_id).or_default().push(mention);
                grouped
            },
        );
        let tags = self
            .repository
            .rest_status_tag_rows(&all_status_ids)
            .await?
            .into_iter()
            .fold(BTreeMap::<i64, Vec<_>>::new(), |mut grouped, tag| {
                grouped.entry(tag.status_id).or_default().push(tag);
                grouped
            });
        let polls = self.repository.rest_polls(&all_status_ids).await?;
        let poll_ids = polls.iter().map(|poll| poll.id).collect::<Vec<_>>();
        let poll_votes = self
            .repository
            .rest_poll_vote_rows(&poll_ids, self.viewer_account_id)
            .await?
            .into_iter()
            .fold(BTreeMap::<i64, Vec<i32>>::new(), |mut grouped, vote| {
                grouped.entry(vote.poll_id).or_default().push(vote.choice);
                grouped
            });
        let mut polls = polls
            .into_iter()
            .map(|poll| {
                let own_votes = self
                    .viewer_account_id
                    .map(|_| poll_votes.get(&poll.id).cloned().unwrap_or_default());
                let voted = self.viewer_account_id.map(|viewer| {
                    viewer == poll.account_id
                        || own_votes.as_ref().is_some_and(|votes| !votes.is_empty())
                });
                let options = poll
                    .options
                    .iter()
                    .enumerate()
                    .map(|(index, title)| PollOptionProjection {
                        title: title.clone(),
                        votes_count: if poll.hide_totals
                            && poll.expires_at.is_none_or(|expires_at| {
                                chrono::Utc::now().naive_utc() < expires_at
                            }) {
                            None
                        } else {
                            Some(poll.cached_tallies.get(index).copied().unwrap_or(0))
                        },
                    })
                    .collect();
                (
                    poll.status_id,
                    PollProjection {
                        id: poll.id,
                        expires_at: poll.expires_at,
                        multiple: poll.multiple,
                        votes_count: poll.votes_count,
                        voters_count: poll.voters_count,
                        options,
                        emojis: Vec::new(),
                        voted,
                        own_votes,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let emoji_demands = all_rows
            .iter()
            .filter_map(|row| {
                let account = accounts.get(&row.account_id)?;
                let poll_options = polls
                    .get(&row.id)
                    .into_iter()
                    .flat_map(|poll| poll.options.iter().map(|option| option.title.as_str()));
                Some((
                    account.domain.clone(),
                    emoji_shortcodes(
                        [row.spoiler_text.as_str(), row.text.as_str()]
                            .into_iter()
                            .chain(poll_options),
                    ),
                ))
            })
            .collect::<Vec<_>>();
        let custom_emojis = self.load_custom_emojis(&emoji_demands).await?;
        let status_emojis = all_rows
            .iter()
            .filter_map(|row| {
                let account = accounts.get(&row.account_id)?;
                let poll_options = polls
                    .get(&row.id)
                    .into_iter()
                    .flat_map(|poll| poll.options.iter().map(|option| option.title.as_str()));
                let shortcodes = emoji_shortcodes(
                    [row.spoiler_text.as_str(), row.text.as_str()]
                        .into_iter()
                        .chain(poll_options),
                );
                Some((
                    row.id,
                    project_emojis(account.domain.as_ref(), &shortcodes, &custom_emojis),
                ))
            })
            .collect::<BTreeMap<_, _>>();
        for (status_id, poll) in &mut polls {
            let Some(account) = all_rows
                .iter()
                .find(|row| row.id == *status_id)
                .and_then(|row| accounts.get(&row.account_id))
            else {
                continue;
            };
            let shortcodes =
                emoji_shortcodes(poll.options.iter().map(|option| option.title.as_str()));
            poll.emojis = project_emojis(account.domain.as_ref(), &shortcodes, &custom_emojis);
        }
        let all_rows_by_id = all_rows
            .iter()
            .map(|row| (row.id, row))
            .collect::<BTreeMap<_, _>>();
        let mut filter_results = if self.viewer_account_id.is_some() {
            let now = chrono::Utc::now().naive_utc();
            let filters = self
                .filters()
                .await?
                .into_iter()
                .filter(|filter| filter.expires_at.is_none_or(|expires_at| expires_at > now))
                .collect::<Vec<_>>();
            all_rows
                .iter()
                .filter_map(|row| {
                    let proper = row
                        .reblog_of_id
                        .and_then(|id| all_rows_by_id.get(&id).copied())
                        .unwrap_or(row);
                    let searchable = searchable_text(proper, &media, &polls);
                    let results = filters
                        .iter()
                        .filter_map(|filter| {
                            let keyword_matches = filter
                                .keywords
                                .iter()
                                .enumerate()
                                .filter_map(|(order, keyword)| {
                                    keyword_match(&searchable, &keyword.keyword, keyword.whole_word)
                                        .map(|(position, matched)| (position, order, matched))
                                })
                                .min_by_key(|(position, order, _)| (*position, *order))
                                .map(|(_, _, matched)| vec![matched]);
                            let status_matches = [Some(row.id), row.reblog_of_id]
                                .into_iter()
                                .flatten()
                                .filter(|status_id| {
                                    filter
                                        .statuses
                                        .iter()
                                        .any(|status| status.status_id == *status_id)
                                })
                                .collect::<Vec<_>>();
                            (keyword_matches.is_some() || !status_matches.is_empty()).then(|| {
                                FilterResultProjection {
                                    filter: filter.clone(),
                                    keyword_matches,
                                    status_matches: (!status_matches.is_empty())
                                        .then_some(status_matches),
                                }
                            })
                        })
                        .collect::<Vec<_>>();
                    (!results.is_empty()).then_some((row.id, results))
                })
                .collect::<BTreeMap<_, _>>()
        } else {
            BTreeMap::new()
        };
        for row in &root_rows {
            if let (Some(reblog_id), Some(results)) =
                (row.reblog_of_id, filter_results.get(&row.id).cloned())
            {
                filter_results.insert(reblog_id, results);
            }
        }
        let nested_by_id = nested_rows
            .iter()
            .map(|row| (row.id, row))
            .collect::<BTreeMap<_, _>>();
        let quote_targets_by_id = quote_target_rows
            .iter()
            .chain(&nested_quote_target_rows)
            .map(|row| (row.id, row))
            .collect::<BTreeMap<_, _>>();
        let quotes = quotes
            .into_iter()
            .chain(nested_quotes)
            .map(|quote| (quote.status_id, quote))
            .collect::<BTreeMap<_, _>>();
        let context = StatusBuildContext {
            accounts: &accounts,
            media: &media,
            mentions: &mentions,
            authorization_mentions: &authorization_mentions,
            tags: &tags,
            polls: &polls,
            quotes: &quotes,
            quote_targets: &quote_targets_by_id,
            filter_results: &filter_results,
            preview_cards: &preview_cards,
            tagged_collections: &tagged_collections,
            status_emojis: &status_emojis,
            viewer_account_id: self.viewer_account_id,
        };
        let mut by_id = root_rows
            .iter()
            .filter_map(|row| {
                build_status(
                    row,
                    row.reblog_of_id
                        .and_then(|id| nested_by_id.get(&id).copied()),
                    &context,
                    1,
                )
                .map(|status| (row.id, status))
            })
            .collect::<BTreeMap<_, _>>();
        Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
    }

    /// Authorizes root statuses and loads only the visible serializer graphs.
    ///
    /// # Errors
    ///
    /// Returns a database error when authorization or projection loading fails.
    pub async fn authorized_statuses(&self, ids: &[i64]) -> sqlx::Result<Vec<StatusProjection>> {
        let authorized = self
            .repository
            .rest_authorized_status_ids(ids, self.viewer_account_id)
            .await?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let ids = ids
            .iter()
            .copied()
            .filter(|id| authorized.contains(id))
            .collect::<Vec<_>>();
        self.preauthorized_statuses(&ids).await
    }

    /// Authorizes and loads one root status.
    ///
    /// # Errors
    ///
    /// Returns a database error when authorization or projection loading fails.
    pub async fn authorized_status(&self, id: i64) -> sqlx::Result<Option<StatusProjection>> {
        Ok(self.authorized_statuses(&[id]).await?.into_iter().next())
    }

    /// Resolves a poll through its parent status so status visibility remains the authority.
    ///
    /// # Errors
    ///
    /// Returns a database error when the poll or its authorized parent cannot be loaded.
    pub async fn authorized_poll(&self, id: i64) -> sqlx::Result<Option<PollProjection>> {
        let Some(poll) = self.repository.poll(id).await? else {
            return Ok(None);
        };
        Ok(self
            .authorized_status(poll.status_id)
            .await?
            .and_then(|status| status.poll))
    }

    /// Loads one status without applying presentation policy after a write has
    /// already authorized ownership of the response-producing action.
    ///
    /// # Errors
    ///
    /// Returns a database error when the projection queries fail.
    pub async fn status_without_authorization(
        &self,
        id: i64,
    ) -> sqlx::Result<Option<StatusProjection>> {
        Ok(self.preauthorized_statuses(&[id]).await?.into_iter().next())
    }

    /// Loads the authorized edit history, or the current status snapshot when it has no edits.
    ///
    /// # Errors
    ///
    /// Returns a database error when status authorization or history projection loading fails.
    pub async fn status_history(&self, id: i64) -> sqlx::Result<Option<Vec<StatusEditProjection>>> {
        let Some(current) = self.authorized_status(id).await? else {
            return Ok(None);
        };
        let history_quote = if current.quote.is_some() {
            current.quote.clone()
        } else {
            self.repository
                .rest_quotes(&[id])
                .await?
                .into_iter()
                .find(|quote| quote.legacy && quote.state.0 != 1)
                .map(|quote| QuoteProjection {
                    state: quote_state(quote.state.0).to_owned(),
                    accepted: false,
                    quoted_status_id: quote.quoted_status_id,
                    target_access: QuoteTargetAccess::Deleted,
                    target_serializable: false,
                    target_link: None,
                    quoted_status: None,
                })
        };
        let edits = self.repository.status_edits(id).await?;
        if edits.is_empty() {
            return Ok(Some(vec![StatusEditProjection {
                account: current.account.clone(),
                text: current.text.clone(),
                spoiler_text: current.spoiler_text.clone(),
                sensitive: Some(current.sensitive),
                created_at: current.edited_at.unwrap_or(current.created_at),
                media_attachments: current.media_attachments.clone(),
                emojis: current.emojis.clone(),
                quote: history_quote.clone(),
                poll_options: current.poll.as_ref().map(|poll| {
                    poll.options
                        .iter()
                        .map(|option| option.title.clone())
                        .collect()
                }),
            }]));
        }
        let account_ids = edits
            .iter()
            .filter_map(|edit| edit.account_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let accounts = self
            .accounts(&account_ids)
            .await?
            .into_iter()
            .map(|account| (account.id, account))
            .collect::<BTreeMap<_, _>>();
        let mut history = Vec::with_capacity(edits.len());
        for edit in edits {
            let media_attachments = match edit.ordered_media_attachment_ids.as_deref() {
                Some(ids) => {
                    let ids = ids.iter().take(4).copied().collect::<Vec<_>>();
                    self.repository
                        .rest_media_attachments_by_ids(id, &ids)
                        .await?
                        .into_iter()
                        .enumerate()
                        .map(|(index, media)| {
                            let description =
                                edit.media_descriptions.as_ref().and_then(|descriptions| {
                                    descriptions.get(index).cloned().flatten()
                                });
                            media_projection(&media, description)
                        })
                        .collect()
                }
                None => Vec::new(),
            };
            history.push(StatusEditProjection {
                account: edit
                    .account_id
                    .and_then(|account_id| accounts.get(&account_id).cloned())
                    .unwrap_or_else(|| current.account.clone()),
                text: edit.text,
                spoiler_text: edit.spoiler_text,
                sensitive: edit.sensitive,
                created_at: edit.created_at,
                media_attachments,
                emojis: current.emojis.clone(),
                quote: edit.quote_id.and_then(|_| history_quote.clone()),
                poll_options: edit.poll_options,
            });
        }
        Ok(Some(history))
    }

    /// Loads accepted quotes of one visibility-authorized status.
    ///
    /// # Errors
    ///
    /// Returns a database error when status authorization, quote selection, or
    /// status projection loading fails.
    pub async fn status_quotes(
        &self,
        id: i64,
        options: &super::FollowCollectionOptions,
    ) -> sqlx::Result<Option<StatusQuotesPage>> {
        if self.authorized_status(id).await?.is_none() {
            return Ok(None);
        }
        let rows = self
            .repository
            .rest_status_quote_rows(id, self.viewer_account_id, options)
            .await?;
        let status_ids = rows.iter().map(|row| row.status_id).collect::<Vec<_>>();
        let visible_status_ids = self
            .repository
            .rest_status_quote_visible_ids(&status_ids, self.viewer_account_id)
            .await?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut statuses = self.authorized_statuses(&status_ids).await?;
        statuses.retain(|status| visible_status_ids.contains(&status.id));
        let first_cursor = rows.first().map(|row| row.quote_id);
        let last_cursor = rows.last().map(|row| row.quote_id);
        Ok(Some(StatusQuotesPage {
            statuses,
            first_cursor,
            last_cursor,
            records_continue: options.limit > 0
                && rows.len() == usize::try_from(options.limit).unwrap_or_default(),
        }))
    }

    /// Loads one account's visibility-correct status page.
    ///
    /// # Errors
    ///
    /// Returns a database error when status selection or projection loading fails.
    pub async fn account_statuses(
        &self,
        account_id: i64,
        options: &super::AccountStatusesOptions,
    ) -> sqlx::Result<Vec<StatusProjection>> {
        let ids = self
            .repository
            .rest_account_status_ids(account_id, self.viewer_account_id, options)
            .await?;
        self.preauthorized_statuses(&ids).await
    }

    /// Loads a visibility-correct public timeline directly from `PostgreSQL`.
    ///
    /// # Errors
    ///
    /// Returns a database error when timeline selection or projection loading fails.
    pub async fn public_timeline(
        &self,
        options: &super::TimelineOptions,
    ) -> sqlx::Result<Vec<StatusProjection>> {
        let ids = self
            .repository
            .rest_public_timeline_ids(self.viewer_account_id, options)
            .await?;
        self.preauthorized_statuses(&ids).await
    }

    /// Loads a visibility-correct hashtag timeline directly from `PostgreSQL`.
    ///
    /// # Errors
    ///
    /// Returns a database error when timeline selection or projection loading fails.
    pub async fn tag_timeline(
        &self,
        tag_name: &str,
        options: &super::TagTimelineOptions,
    ) -> sqlx::Result<Vec<StatusProjection>> {
        let ids = self
            .repository
            .rest_tag_timeline_ids(tag_name, self.viewer_account_id, options)
            .await?;
        self.preauthorized_statuses(&ids).await
    }

    /// Returns whether a normalized hashtag exists before pagination is evaluated.
    ///
    /// # Errors
    ///
    /// Returns a database error when hashtag lookup fails.
    pub async fn tag_exists(&self, tag_name: &str) -> sqlx::Result<bool> {
        self.repository.rest_tag_exists(tag_name).await
    }

    /// Loads a current user's PostgreSQL-backed home timeline.
    ///
    /// # Errors
    ///
    /// Returns a database error when timeline selection or projection loading fails.
    pub async fn home_timeline(
        &self,
        account_id: i64,
        options: &super::TimelineOptions,
    ) -> sqlx::Result<Vec<StatusProjection>> {
        let ids = self
            .repository
            .rest_home_timeline_ids(account_id, options)
            .await?;
        self.preauthorized_statuses(&ids).await
    }

    /// Loads one owned PostgreSQL-backed list timeline.
    ///
    /// # Errors
    ///
    /// Returns a database error when list lookup, selection, or projection loading fails.
    pub async fn list_timeline(
        &self,
        account_id: i64,
        list_id: i64,
        options: &super::TimelineOptions,
    ) -> sqlx::Result<Option<Vec<StatusProjection>>> {
        let Some(ids) = self
            .repository
            .rest_list_timeline_ids(account_id, list_id, options)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(self.preauthorized_statuses(&ids).await?))
    }

    /// Returns whether the current account owns a list before pagination is evaluated.
    ///
    /// # Errors
    ///
    /// Returns a database error when list lookup fails.
    pub async fn owned_list_exists(&self, account_id: i64, list_id: i64) -> sqlx::Result<bool> {
        self.repository
            .rest_owned_list_exists(account_id, list_id)
            .await
    }

    /// Loads a current user's favourites or bookmarks with association cursors.
    ///
    /// # Errors
    ///
    /// Returns a database error when association selection or projection loading fails.
    pub async fn saved_statuses(
        &self,
        account_id: i64,
        kind: super::SavedStatusKind,
        options: &super::SavedStatusesOptions,
    ) -> sqlx::Result<super::SavedStatusesPage> {
        let rows = self
            .repository
            .rest_saved_status_rows(account_id, kind, options)
            .await?;
        let ids = rows.iter().map(|row| row.status_id).collect::<Vec<_>>();
        // Saving a status does not preserve access after its audience changes.
        // Pagination still describes the selected associations, before authorization.
        Ok(super::SavedStatusesPage {
            statuses: self.authorized_statuses(&ids).await?,
            first_cursor: rows.first().map(|row| row.cursor_id),
            last_cursor: rows.last().map(|row| row.cursor_id),
            records_continue: usize::try_from(options.limit.clamp(0, 40))
                .is_ok_and(|limit| rows.len() == limit),
        })
    }

    /// Loads a current user's blocks or mutes with relationship cursors.
    ///
    /// # Errors
    ///
    /// Returns a database error when relationship selection or account loading fails.
    pub async fn account_list(
        &self,
        account_id: i64,
        kind: super::AccountListKind,
        options: &super::AccountListOptions,
    ) -> sqlx::Result<super::AccountListPage> {
        let rows = self
            .repository
            .rest_account_list_rows(account_id, kind, options)
            .await?;
        let account_ids = rows.iter().map(|row| row.account_id).collect::<Vec<_>>();
        let mut accounts = self
            .accounts(&account_ids)
            .await?
            .into_iter()
            .map(|account| (account.id, account))
            .collect::<BTreeMap<_, _>>();
        Ok(super::AccountListPage {
            entries: rows
                .iter()
                .filter_map(|row| {
                    Some(super::AccountListEntryProjection {
                        account: accounts.remove(&row.account_id)?,
                        mute_expires_at: row.mute_expires_at,
                    })
                })
                .collect(),
            first_cursor: rows.first().map(|row| row.cursor_id),
            last_cursor: rows.last().map(|row| row.cursor_id),
        })
    }

    /// Loads one visibility-correct followers or following page.
    ///
    /// # Errors
    ///
    /// Returns a database error when follow selection or account loading fails.
    pub async fn follow_collection(
        &self,
        account_id: i64,
        kind: super::FollowCollectionKind,
        options: &super::FollowCollectionOptions,
    ) -> sqlx::Result<super::FollowCollectionPage> {
        let rows = self
            .repository
            .rest_follow_collection_rows(account_id, self.viewer_account_id, kind, options)
            .await?;
        let ids = rows.iter().map(|row| row.account_id).collect::<Vec<_>>();
        Ok(super::FollowCollectionPage {
            accounts: self.accounts(&ids).await?,
            first_cursor: rows.first().map(|row| row.follow_id),
            last_cursor: rows.last().map(|row| row.follow_id),
        })
    }

    /// Loads pending follow requests for one authenticated account.
    ///
    /// # Errors
    ///
    /// Returns a database error when follow-request selection or account loading fails.
    pub async fn follow_requests(
        &self,
        account_id: i64,
        options: &super::FollowCollectionOptions,
    ) -> sqlx::Result<super::FollowCollectionPage> {
        let rows = self
            .repository
            .rest_follow_request_rows(account_id, options)
            .await?;
        let ids = rows.iter().map(|row| row.account_id).collect::<Vec<_>>();
        Ok(super::FollowCollectionPage {
            accounts: self.accounts(&ids).await?,
            first_cursor: rows.first().map(|row| row.follow_id),
            last_cursor: rows.last().map(|row| row.follow_id),
        })
    }

    /// Loads accounts that favourited an authorized status.
    ///
    /// # Errors
    ///
    /// Returns a database error when status authorization or favourite selection fails.
    pub async fn favourited_by(
        &self,
        status_id: i64,
        options: &super::FollowCollectionOptions,
    ) -> sqlx::Result<Option<super::FollowCollectionPage>> {
        if self.authorized_status(status_id).await?.is_none() {
            return Ok(None);
        }
        let rows = self
            .repository
            .rest_favourited_by_rows(status_id, self.viewer_account_id, options)
            .await?;
        let ids = rows.iter().map(|row| row.account_id).collect::<Vec<_>>();
        Ok(Some(super::FollowCollectionPage {
            accounts: self.accounts(&ids).await?,
            first_cursor: rows.first().map(|row| row.follow_id),
            last_cursor: rows.last().map(|row| row.follow_id),
        }))
    }

    /// Loads accounts that reblogged an authorized status.
    ///
    /// # Errors
    ///
    /// Returns a database error when status authorization or reblog selection fails.
    pub async fn reblogged_by(
        &self,
        status_id: i64,
        options: &super::FollowCollectionOptions,
    ) -> sqlx::Result<Option<super::FollowCollectionPage>> {
        if self.authorized_status(status_id).await?.is_none() {
            return Ok(None);
        }
        let rows = self
            .repository
            .rest_reblogged_by_rows(status_id, self.viewer_account_id, options)
            .await?;
        let ids = rows.iter().map(|row| row.account_id).collect::<Vec<_>>();
        Ok(Some(super::FollowCollectionPage {
            accounts: self.accounts(&ids).await?,
            first_cursor: rows.first().map(|row| row.follow_id),
            last_cursor: rows.last().map(|row| row.follow_id),
        }))
    }

    /// Returns whether follow collections are unavailable before pagination is evaluated.
    ///
    /// # Errors
    ///
    /// Returns a database error when collection visibility lookup fails.
    pub async fn follow_collection_hidden(&self, account_id: i64) -> sqlx::Result<bool> {
        self.repository
            .rest_follow_collection_hidden(account_id, self.viewer_account_id)
            .await
    }

    /// Loads a root-authorized status context with member-specific filtering.
    ///
    /// # Errors
    ///
    /// Returns a database error when traversal, authorization, or projection loading fails.
    pub async fn status_context(
        &self,
        status_id: i64,
    ) -> sqlx::Result<Option<super::StatusContextProjection>> {
        if self.authorized_status(status_id).await?.is_none() {
            return Ok(None);
        }
        let authenticated = self.viewer_account_id.is_some();
        let (ancestor_ids, descendant_ids) = self
            .repository
            .rest_context_ids(
                status_id,
                if authenticated { 4_096 } else { 40 },
                if authenticated { 4_096 } else { 60 },
                (!authenticated).then_some(20),
            )
            .await?;
        let all_ids = ancestor_ids
            .iter()
            .chain(&descendant_ids)
            .copied()
            .collect::<Vec<_>>();
        let visible = self
            .repository
            .rest_context_visible_status_ids(&all_ids, self.viewer_account_id)
            .await?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let ancestors = ancestor_ids
            .into_iter()
            .filter(|id| visible.contains(id))
            .collect::<Vec<_>>();
        let descendants = descendant_ids
            .into_iter()
            .filter(|id| visible.contains(id))
            .collect::<Vec<_>>();
        let descendant_rows = self
            .repository
            .rest_status_rows(&descendants, self.viewer_account_id)
            .await?
            .into_iter()
            .map(|row| (row.id, row))
            .collect::<BTreeMap<_, _>>();
        let (mut self_replies, other_descendants): (Vec<_>, Vec<_>) =
            descendants.into_iter().partition(|id| {
                descendant_rows
                    .get(id)
                    .is_some_and(|row| row.in_reply_to_account_id == Some(row.account_id))
            });
        self_replies.extend(other_descendants);
        Ok(Some(super::StatusContextProjection {
            ancestors: self.preauthorized_statuses(&ancestors).await?,
            descendants: self.preauthorized_statuses(&self_replies).await?,
        }))
    }

    /// Loads one collection and the items visible to the configured viewer.
    ///
    /// # Errors
    ///
    /// Returns a database error when any read-only projection query fails.
    pub async fn collection(&self, id: i64) -> sqlx::Result<Option<CollectionProjection>> {
        Ok(self.collections(&[id]).await?.into_iter().next())
    }

    /// Loads collections and their viewer-visible item graphs in requested order.
    ///
    /// # Errors
    ///
    /// Returns a database error when any read-only projection query fails.
    pub async fn collections(&self, ids: &[i64]) -> sqlx::Result<Vec<CollectionProjection>> {
        let collections = self.repository.rest_collections(ids).await?;
        let collection_ids = collections
            .iter()
            .map(|collection| collection.id)
            .collect::<Vec<_>>();
        let owners = collections
            .iter()
            .map(|collection| (collection.id, collection.account_id))
            .collect::<BTreeMap<_, _>>();
        let mut items = self
            .repository
            .rest_collection_items(&collection_ids)
            .await?
            .into_iter()
            .filter(|item| {
                item.state.0 == 1
                    || (item.state.0 == 0
                        && owners.get(&item.collection_id).copied() == self.viewer_account_id)
            })
            .collect::<Vec<_>>();
        let item_account_ids = items
            .iter()
            .filter_map(|item| item.account_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if self.viewer_account_id.is_some() {
            let blocked = self
                .relationships(&item_account_ids, false)
                .await?
                .into_iter()
                .filter(|relationship| relationship.blocking)
                .map(|relationship| relationship.target_account_id)
                .collect::<BTreeSet<_>>();
            items.retain(|item| item.account_id.is_none_or(|id| !blocked.contains(&id)));
        }
        let mut account_ids = collections
            .iter()
            .map(|collection| collection.account_id)
            .collect::<BTreeSet<_>>();
        account_ids.extend(items.iter().filter_map(|item| item.account_id));
        let accounts = self
            .accounts(&account_ids.into_iter().collect::<Vec<_>>())
            .await?
            .into_iter()
            .map(|account| (account.id, account))
            .collect::<BTreeMap<_, _>>();
        let tag_ids = collections
            .iter()
            .filter_map(|collection| collection.tag_id)
            .collect::<Vec<_>>();
        let tags = self
            .repository
            .rest_tags(&tag_ids)
            .await?
            .into_iter()
            .map(|tag| {
                (
                    tag.id,
                    TagProjection {
                        id: tag.id,
                        name: tag.name,
                        display_name: tag.display_name,
                        history: Vec::new(),
                        following: None,
                        featuring: None,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let items = items
            .into_iter()
            .fold(BTreeMap::<i64, Vec<_>>::new(), |mut grouped, item| {
                grouped.entry(item.collection_id).or_default().push(item);
                grouped
            });
        let mut by_id = collections
            .into_iter()
            .filter_map(|collection| {
                let account = accounts.get(&collection.account_id)?.clone();
                let collection_items = items.get(&collection.id).cloned().unwrap_or_default();
                let item_accounts = collection_items
                    .iter()
                    .filter_map(|item| item.account_id)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .filter_map(|id| accounts.get(&id).cloned())
                    .collect();
                Some((
                    collection.id,
                    CollectionProjection {
                        id: collection.id,
                        account,
                        name: collection.name,
                        description: collection.description,
                        description_html: collection.description_html,
                        local: collection.local,
                        sensitive: collection.sensitive,
                        discoverable: collection.discoverable,
                        language: collection.language,
                        stored_uri: collection.uri,
                        stored_url: collection.url,
                        created_at: collection.created_at,
                        updated_at: collection.updated_at,
                        tag: collection.tag_id.and_then(|id| tags.get(&id).cloned()),
                        items: collection_items
                            .into_iter()
                            .map(|item| CollectionItemProjection {
                                id: item.id,
                                account_id: item.account_id,
                                state: item.state.0,
                                created_at: item.created_at,
                            })
                            .collect(),
                        item_accounts,
                    },
                ))
            })
            .collect::<BTreeMap<_, _>>();
        Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
    }

    /// Loads complete v2 filter definitions for the configured viewer.
    ///
    /// # Errors
    ///
    /// Returns a database error when any read-only filter query fails.
    pub async fn filters(&self) -> sqlx::Result<Vec<FilterProjection>> {
        let Some(account_id) = self.viewer_account_id else {
            return Ok(Vec::new());
        };
        let filters = self.repository.custom_filters(account_id).await?;
        let filter_ids = filters.iter().map(|filter| filter.id).collect::<Vec<_>>();
        let keywords = self
            .repository
            .rest_filter_keywords(&filter_ids)
            .await?
            .into_iter()
            .fold(BTreeMap::<i64, Vec<_>>::new(), |mut grouped, keyword| {
                grouped
                    .entry(keyword.custom_filter_id)
                    .or_default()
                    .push(keyword);
                grouped
            });
        let statuses = self
            .repository
            .rest_filter_statuses(&filter_ids)
            .await?
            .into_iter()
            .fold(BTreeMap::<i64, Vec<_>>::new(), |mut grouped, status| {
                grouped
                    .entry(status.custom_filter_id)
                    .or_default()
                    .push(status);
                grouped
            });
        Ok(filters
            .into_iter()
            .map(|filter| FilterProjection {
                id: filter.id,
                title: filter.phrase,
                context: filter.context,
                expires_at: filter.expires_at,
                action: filter.action.0,
                keywords: keywords
                    .get(&filter.id)
                    .into_iter()
                    .flatten()
                    .map(|keyword| FilterKeywordProjection {
                        id: keyword.id,
                        keyword: keyword.keyword.clone(),
                        whole_word: keyword.whole_word,
                    })
                    .collect(),
                statuses: statuses
                    .get(&filter.id)
                    .into_iter()
                    .flatten()
                    .map(|status| FilterStatusProjection {
                        id: status.id,
                        status_id: status.status_id,
                    })
                    .collect(),
            })
            .collect())
    }

    /// Loads markers owned by one authenticated user for the requested timelines.
    ///
    /// # Errors
    ///
    /// Returns a database error when the read-only marker query fails.
    pub async fn markers(
        &self,
        user_id: i64,
        timelines: &[String],
    ) -> sqlx::Result<Vec<MarkerProjection>> {
        Ok(self
            .repository
            .rest_markers(user_id, timelines)
            .await?
            .into_iter()
            .map(|marker| MarkerProjection {
                timeline: marker.timeline,
                last_read_id: marker.last_read_id,
                version: marker.lock_version,
                updated_at: marker.updated_at,
            })
            .collect())
    }

    /// Loads lists owned by one authenticated account.
    ///
    /// # Errors
    ///
    /// Returns a database error when the read-only list query fails.
    pub async fn lists(&self, account_id: i64) -> sqlx::Result<Vec<super::ListProjection>> {
        Ok(self
            .repository
            .lists(account_id)
            .await?
            .into_iter()
            .map(|list| super::ListProjection {
                id: list.id,
                title: list.title,
                replies_policy: list.replies_policy.0,
                exclusive: list.exclusive,
            })
            .collect())
    }

    /// Loads notification requests and their account/status serializer graphs.
    ///
    /// # Errors
    ///
    /// Returns a database error when any request, account, or status projection
    /// query fails.
    pub async fn notification_requests(
        &self,
        account_id: i64,
        max_id: Option<i64>,
        since_id: Option<i64>,
        min_id: Option<i64>,
        limit: i64,
    ) -> sqlx::Result<Vec<NotificationRequestProjection>> {
        let requests = self
            .repository
            .rest_notification_requests(account_id, max_id, since_id, min_id, limit)
            .await?;
        let account_ids = requests
            .iter()
            .map(|request| request.from_account_id)
            .collect::<Vec<_>>();
        let accounts = self
            .accounts(&account_ids)
            .await?
            .into_iter()
            .map(|account| (account.id, account))
            .collect::<BTreeMap<_, _>>();
        let status_ids = requests
            .iter()
            .filter_map(|request| request.last_status_id)
            .collect::<Vec<_>>();
        let statuses = self
            .authorized_statuses(&status_ids)
            .await?
            .into_iter()
            .map(|status| (status.id, status))
            .collect::<BTreeMap<_, _>>();
        Ok(requests
            .into_iter()
            .filter_map(|request| {
                Some(NotificationRequestProjection {
                    id: request.id,
                    account: accounts.get(&request.from_account_id)?.clone(),
                    last_status: request
                        .last_status_id
                        .and_then(|id| statuses.get(&id).cloned()),
                    notifications_count: request.notifications_count,
                    created_at: request.created_at,
                    updated_at: request.updated_at,
                })
            })
            .collect())
    }

    /// Loads one account-owned notification request.
    ///
    /// # Errors
    ///
    /// Returns a database error when the request projection query fails.
    pub async fn notification_request(
        &self,
        account_id: i64,
        request_id: i64,
    ) -> sqlx::Result<Option<NotificationRequestProjection>> {
        Ok(self
            .notification_requests(account_id, None, None, None, i64::MAX)
            .await?
            .into_iter()
            .find(|request| request.id == request_id))
    }

    /// Loads featured tags owned by one authenticated account.
    ///
    /// # Errors
    ///
    /// Returns a database error when the read-only featured-tag query fails.
    pub async fn featured_tags(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<super::FeaturedTagProjection>> {
        Ok(self
            .repository
            .rest_featured_tag_rows(account_id)
            .await?
            .into_iter()
            .map(|tag| super::FeaturedTagProjection {
                id: tag.id,
                name: tag
                    .name
                    .or(tag.tag_display_name)
                    .unwrap_or(tag.tag_name.clone()),
                tag_name: tag.tag_name,
                statuses_count: tag.statuses_count,
                last_status_at: tag.last_status_at,
                username: tag.username,
                domain: tag.domain,
            })
            .collect())
    }

    /// Loads recently used, not-yet-featured tags for one authenticated account.
    ///
    /// # Errors
    ///
    /// Returns a database error when the suggestion query fails.
    pub async fn featured_tag_suggestions(
        &self,
        account_id: i64,
    ) -> sqlx::Result<Vec<TagProjection>> {
        Ok(self
            .repository
            .rest_featured_tag_suggestions(account_id)
            .await?
            .into_iter()
            .map(|tag| TagProjection {
                id: tag.id,
                name: tag.name,
                display_name: tag.display_name,
                history: recent_tag_history(),
                following: Some(tag.following),
                featuring: Some(false),
            })
            .collect())
    }

    /// Searches listable hashtags by normalized name prefix.
    ///
    /// # Errors
    ///
    /// Returns a database error when the tag search fails.
    pub async fn tag_search(
        &self,
        query: &str,
        limit: i64,
        offset: i64,
        exclude_unreviewed: bool,
    ) -> sqlx::Result<Vec<TagProjection>> {
        let tags = self
            .repository
            .rest_tag_search(query, limit, offset, exclude_unreviewed)
            .await?;
        let relationships = if let Some(account_id) = self.viewer_account_id {
            self.repository
                .rest_tag_relationships(
                    account_id,
                    &tags.iter().map(|tag| tag.id).collect::<Vec<_>>(),
                )
                .await?
                .into_iter()
                .map(|(id, following, featuring)| (id, (following, featuring)))
                .collect::<HashMap<_, _>>()
        } else {
            HashMap::new()
        };
        Ok(tags
            .into_iter()
            .map(|tag| {
                let relationship = relationships.get(&tag.id).copied();
                TagProjection {
                    id: tag.id,
                    name: tag.name,
                    display_name: tag.display_name,
                    history: recent_tag_history(),
                    following: relationship.map(|value| value.0),
                    featuring: relationship.map(|value| value.1),
                }
            })
            .collect())
    }

    /// Loads tags followed by one authenticated account with cursor metadata.
    ///
    /// # Errors
    ///
    /// Returns a database error when the read-only tag-follow query fails.
    pub async fn followed_tags(
        &self,
        account_id: i64,
        options: &super::FollowedTagsOptions,
    ) -> sqlx::Result<FollowedTagsPage> {
        let rows = self
            .repository
            .rest_followed_tags(account_id, options)
            .await?;
        let first_cursor = rows.first().map(|row| row.tag_follow_id);
        let last_cursor = rows.last().map(|row| row.tag_follow_id);
        let tags = rows
            .into_iter()
            .map(|tag| TagProjection {
                id: tag.id,
                name: tag.name,
                display_name: tag.display_name,
                history: recent_tag_history(),
                following: Some(true),
                featuring: Some(tag.featuring),
            })
            .collect();
        Ok(FollowedTagsPage {
            tags,
            first_cursor,
            last_cursor,
        })
    }

    /// Loads the locally listed custom emojis for the public picker endpoint.
    ///
    /// # Errors
    ///
    /// Returns a database error when the listed emoji query fails.
    pub async fn custom_emojis(&self) -> sqlx::Result<Vec<CustomEmojiProjection>> {
        Ok(self
            .repository
            .rest_listed_custom_emojis()
            .await?
            .into_iter()
            .map(|emoji| {
                let category = emoji.category;
                CustomEmojiProjection {
                    id: emoji.id,
                    shortcode: emoji.shortcode,
                    domain: emoji.domain,
                    file_name: emoji.image_file_name,
                    storage_schema_version: emoji.image_storage_schema_version,
                    visible_in_picker: emoji.visible_in_picker,
                    featured: category.as_ref().map(|_| emoji.featured),
                    category,
                }
            })
            .collect())
    }

    /// Loads the shared database-backed instance projection for v1 and v2 serializers.
    ///
    /// # Errors
    ///
    /// Returns a database error when any read-only instance query fails.
    pub async fn instance(
        &self,
        runtime: InstanceRuntimeConfig,
    ) -> sqlx::Result<InstanceProjection> {
        let counts = self.repository.rest_instance_counts().await?;
        let settings = self
            .repository
            .settings()
            .await?
            .into_iter()
            .filter_map(|setting| {
                setting
                    .value
                    .map(|value| (setting.var, value.raw().to_owned()))
            })
            .collect::<BTreeMap<_, _>>();
        let contact_username = setting_string(&settings, "site_contact_username")
            .unwrap_or_default()
            .trim()
            .trim_start_matches('@')
            .split('@')
            .next()
            .unwrap_or_default()
            .to_owned();
        let contact_account = if contact_username.is_empty() {
            None
        } else {
            match self
                .repository
                .rest_local_account_id_by_username(&contact_username)
                .await?
            {
                Some(id) => self.account(id).await?,
                None => None,
            }
        };
        let mut rules = Vec::<RuleProjection>::new();
        for row in self.repository.rest_rule_rows().await? {
            if rules.last().is_none_or(|rule| rule.id != row.id) {
                rules.push(RuleProjection {
                    id: row.id,
                    text: row.text.clone(),
                    hint: row.hint.clone(),
                    translations: BTreeMap::new(),
                });
            }
            if let (Some(language), Some(text), Some(hint)) =
                (row.language, row.translated_text, row.translated_hint)
                && let Some(rule) = rules.last_mut()
            {
                rule.translations.insert(language, (text, hint));
            }
        }
        Ok(InstanceProjection {
            runtime,
            title: setting_string(&settings, "site_title").unwrap_or_else(|| "Mastodon".to_owned()),
            short_description: setting_string(&settings, "site_short_description")
                .unwrap_or_default(),
            legacy_description: setting_string(&settings, "site_description").unwrap_or_default(),
            contact_email: setting_string(&settings, "site_contact_email").unwrap_or_default(),
            status_page_url: setting_string(&settings, "status_page_url")
                .filter(|value| !value.is_empty()),
            user_count: counts.user_count,
            status_count: counts.status_count,
            domain_count: counts.domain_count,
            registrations_mode: setting_string(&settings, "registrations_mode")
                .unwrap_or_else(|| "none".to_owned()),
            require_invite_text: setting_bool(&settings, "require_invite_text").unwrap_or(false),
            closed_registrations_message: setting_string(&settings, "closed_registrations_message")
                .filter(|value| !value.is_empty()),
            min_age: setting_i32(&settings, "min_age"),
            invites_enabled: PermissionBits(counts.everyone_permissions)
                .contains(UserPermission::InviteUsers),
            local_live_feed_access: setting_string(&settings, "local_live_feed_access")
                .unwrap_or_else(|| "public".to_owned()),
            remote_live_feed_access: setting_string(&settings, "remote_live_feed_access")
                .unwrap_or_else(|| "public".to_owned()),
            local_topic_feed_access: setting_string(&settings, "local_topic_feed_access")
                .unwrap_or_else(|| "public".to_owned()),
            remote_topic_feed_access: setting_string(&settings, "remote_topic_feed_access")
                .unwrap_or_else(|| "public".to_owned()),
            contact_account,
            rules,
        })
    }

    /// Loads v1 notification entities and their reusable account/status/collection graphs.
    ///
    /// # Errors
    ///
    /// Returns a database error when any read-only notification projection query fails.
    pub async fn notifications(
        &self,
        account_id: i64,
        options: &NotificationOptions,
    ) -> sqlx::Result<Vec<NotificationProjection>> {
        let notifications = self
            .repository
            .rest_notifications(account_id, options, false)
            .await?;
        self.notification_projections(notifications).await
    }

    async fn notification_projections(
        &self,
        mut notifications: Vec<Notification>,
    ) -> sqlx::Result<Vec<NotificationProjection>> {
        notifications.sort_unstable_by_key(|notification| std::cmp::Reverse(notification.id));
        let effective_types = notifications
            .iter()
            .filter_map(|notification| {
                effective_notification_type(
                    notification.notification_type.as_ref(),
                    &notification.activity_type.0,
                )
                .map(|kind| (notification.id, kind))
            })
            .collect::<BTreeMap<_, _>>();
        let notification_ids = notifications
            .iter()
            .map(|notification| notification.id)
            .collect::<Vec<_>>();
        let targets = self
            .repository
            .rest_notification_targets(&notification_ids)
            .await?
            .into_iter()
            .map(|target| (target.notification_id, target))
            .collect::<BTreeMap<_, _>>();
        let severance_ids = notifications
            .iter()
            .filter(|notification| {
                effective_types
                    .get(&notification.id)
                    .is_some_and(|kind| kind.raw() == "severed_relationships")
            })
            .map(|notification| notification.activity_id)
            .collect::<Vec<_>>();
        let severance_events = self
            .repository
            .rest_severance_events(&severance_ids)
            .await?
            .into_iter()
            .map(|event| {
                (
                    event.id,
                    SeveranceEventProjection {
                        id: event.id,
                        event_type: event.event_type,
                        purged: event.purged,
                        target_name: event.target_name,
                        followers_count: event.followers_count,
                        following_count: event.following_count,
                        created_at: event.created_at,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let warning_ids = notifications
            .iter()
            .filter(|notification| {
                effective_types
                    .get(&notification.id)
                    .is_some_and(|kind| kind.raw() == "moderation_warning")
            })
            .map(|notification| notification.activity_id)
            .collect::<Vec<_>>();
        let warning_rows = self.repository.rest_account_warnings(&warning_ids).await?;
        let report_ids = notifications
            .iter()
            .filter(|notification| {
                effective_types
                    .get(&notification.id)
                    .is_some_and(|kind| kind.raw() == "admin.report")
            })
            .map(|notification| notification.activity_id)
            .collect::<Vec<_>>();
        let report_rows = self.repository.rest_reports(&report_ids).await?;
        let report_collections = self
            .repository
            .rest_report_collections(&report_ids)
            .await?
            .into_iter()
            .fold(BTreeMap::<i64, Vec<_>>::new(), |mut grouped, row| {
                grouped.entry(row.0).or_default().push(row.1);
                grouped
            });
        let annual_report_ids = notifications
            .iter()
            .filter(|notification| {
                effective_types
                    .get(&notification.id)
                    .is_some_and(|kind| kind.raw() == "annual_report")
            })
            .map(|notification| notification.activity_id)
            .collect::<Vec<_>>();
        let annual_report_years = self
            .repository
            .rest_annual_report_years(&annual_report_ids)
            .await?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let mut account_ids = notifications
            .iter()
            .map(|notification| notification.from_account_id)
            .collect::<BTreeSet<_>>();
        account_ids.extend(
            warning_rows
                .iter()
                .filter_map(|warning| warning.target_account_id),
        );
        account_ids.extend(report_rows.iter().map(|report| report.target_account_id));
        let accounts = self
            .accounts(&account_ids.into_iter().collect::<Vec<_>>())
            .await?
            .into_iter()
            .map(|account| (account.id, account))
            .collect::<BTreeMap<_, _>>();
        let status_ids = targets
            .values()
            .filter_map(|target| target.status_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let statuses = self
            .preauthorized_statuses(&status_ids)
            .await?
            .into_iter()
            .map(|status| (status.id, status))
            .collect::<BTreeMap<_, _>>();
        let collection_ids = targets
            .values()
            .filter_map(|target| target.collection_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let collections = self
            .collections(&collection_ids)
            .await?
            .into_iter()
            .map(|collection| (collection.id, collection))
            .collect::<BTreeMap<_, _>>();
        let warnings = warning_rows
            .into_iter()
            .map(|warning| {
                (
                    warning.id,
                    AccountWarningProjection {
                        id: warning.id,
                        action: warning.action,
                        text: warning.text,
                        status_ids: warning.status_ids.map(|ids| {
                            ids.into_iter()
                                .filter_map(|id| id.parse::<i64>().ok())
                                .collect()
                        }),
                        created_at: warning.created_at,
                        target_account: warning
                            .target_account_id
                            .and_then(|id| accounts.get(&id).cloned()),
                        appeal: warning.appeal_text.map(|text| AppealProjection {
                            text,
                            approved: warning.appeal_approved_at.is_some(),
                            rejected: warning.appeal_rejected_at.is_some(),
                        }),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let reports = report_rows
            .into_iter()
            .filter_map(|report| {
                Some((
                    report.id,
                    ReportProjection {
                        id: report.id,
                        action_taken_at: report.action_taken_at,
                        category: report.category.0,
                        comment: report.comment,
                        forwarded: report.forwarded,
                        created_at: report.created_at,
                        status_ids: report.status_ids,
                        rule_ids: report.rule_ids,
                        collection_ids: report_collections
                            .get(&report.id)
                            .cloned()
                            .unwrap_or_default(),
                        target_account: accounts.get(&report.target_account_id)?.clone(),
                    },
                ))
            })
            .collect::<BTreeMap<_, _>>();
        Ok(notifications
            .into_iter()
            .filter_map(|notification| {
                let notification_type = effective_types.get(&notification.id)?.clone();
                let account = accounts.get(&notification.from_account_id)?.clone();
                let target = targets.get(&notification.id);
                Some(NotificationProjection {
                    id: notification.id,
                    notification_type,
                    created_at: notification.created_at,
                    group_key: notification.group_key,
                    filtered: notification.filtered,
                    account,
                    status: target
                        .and_then(|target| target.status_id)
                        .and_then(|id| statuses.get(&id).cloned()),
                    collection: target
                        .and_then(|target| target.collection_id)
                        .and_then(|id| collections.get(&id).cloned()),
                    report: reports.get(&notification.activity_id).cloned(),
                    event: severance_events.get(&notification.activity_id).cloned(),
                    moderation_warning: warnings.get(&notification.activity_id).cloned(),
                    annual_report_year: annual_report_years.get(&notification.activity_id).copied(),
                })
            })
            .collect())
    }

    /// Loads one notification owned by the authenticated account.
    ///
    /// # Errors
    ///
    /// Returns a database error when notification or dependent projections fail.
    pub async fn notification(
        &self,
        account_id: i64,
        notification_id: i64,
    ) -> sqlx::Result<Option<NotificationProjection>> {
        let Some(notification) = self
            .repository
            .rest_notification(account_id, notification_id)
            .await?
        else {
            return Ok(None);
        };
        Ok(self
            .notification_projections(vec![notification])
            .await?
            .into_iter()
            .next())
    }

    /// Loads the default v2 grouped-notification envelope.
    ///
    /// # Errors
    ///
    /// Returns a database error when any reused notification projection query fails.
    pub async fn grouped_notifications(
        &self,
        account_id: i64,
        options: &NotificationOptions,
    ) -> sqlx::Result<GroupedNotificationsProjection> {
        let rows = self
            .repository
            .rest_notifications(account_id, options, true)
            .await?;
        let notifications = self.notification_projections(rows).await?;
        let Some(page_max_id) = notifications.first().map(|notification| notification.id) else {
            return Ok(GroupedNotificationsProjection {
                accounts: Vec::new(),
                statuses: Vec::new(),
                groups: Vec::new(),
            });
        };
        let page_limit = usize::try_from(options.limit).unwrap_or(usize::MAX);
        let incomplete_page = notifications.len() < page_limit;
        let page_min_id = if options.min_id.is_some() {
            notifications
                .last()
                .map_or(0, |notification| notification.id)
        } else if incomplete_page {
            options
                .since_id
                .map_or(0, |since_id| since_id.saturating_add(1))
        } else {
            notifications
                .last()
                .map_or(page_max_id, |notification| notification.id)
        };
        let page_max_id = if options.min_id.is_some() && incomplete_page {
            options.max_id
        } else {
            Some(page_max_id)
        };
        let page_max_exclusive =
            options.min_id.is_some() && incomplete_page && page_max_id.is_some();
        let grouped_types = grouped_notification_types(&options.grouped_types);
        let group_keys = notifications
            .iter()
            .filter(|notification| {
                grouped_types
                    .iter()
                    .any(|kind| kind == notification.notification_type.raw())
            })
            .filter_map(|notification| {
                notification
                    .group_key
                    .as_deref()
                    .filter(|key| !key.trim().is_empty())
                    .map(str::to_owned)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let group_data = self
            .repository
            .rest_notification_groups(
                account_id,
                &group_keys,
                page_min_id,
                page_max_id,
                page_max_exclusive,
            )
            .await?
            .into_iter()
            .map(|group| (group.group_key.clone(), group))
            .collect::<BTreeMap<_, _>>();
        let sample_account_ids = group_data
            .values()
            .flat_map(|group| group.sample_account_ids.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let sample_accounts = self
            .accounts(&sample_account_ids)
            .await?
            .into_iter()
            .map(|account| (account.id, account))
            .collect::<BTreeMap<_, _>>();
        let groups = notifications
            .into_iter()
            .map(|notification| {
                let grouped = grouped_types
                    .iter()
                    .any(|kind| kind == notification.notification_type.raw())
                    .then_some(
                        notification
                            .group_key
                            .as_deref()
                            .filter(|key| !key.trim().is_empty()),
                    )
                    .flatten()
                    .and_then(|key| group_data.get(key));
                if let Some(group) = grouped {
                    NotificationGroupProjection {
                        group_key: group.group_key.clone(),
                        sample_accounts: group
                            .sample_account_ids
                            .iter()
                            .filter_map(|id| sample_accounts.get(id).cloned())
                            .collect(),
                        notifications_count: group.notifications_count,
                        most_recent_notification_id: group.most_recent_notification_id,
                        page_min_id: Some(group.page_min_id),
                        page_max_id: Some(group.most_recent_notification_id),
                        latest_page_notification_at: Some(group.latest_page_notification_at),
                        notification,
                    }
                } else {
                    NotificationGroupProjection {
                        group_key: format!("ungrouped-{}", notification.id),
                        sample_accounts: vec![notification.account.clone()],
                        notifications_count: 1,
                        most_recent_notification_id: notification.id,
                        page_min_id: Some(notification.id),
                        page_max_id: Some(notification.id),
                        latest_page_notification_at: Some(notification.created_at),
                        notification,
                    }
                }
            })
            .collect::<Vec<_>>();
        let mut account_ids = BTreeSet::new();
        let accounts = groups
            .iter()
            .flat_map(|group| &group.sample_accounts)
            .filter(|account| account_ids.insert(account.id))
            .cloned()
            .collect();
        let mut status_ids = BTreeSet::new();
        let statuses = groups
            .iter()
            .filter_map(|group| group.notification.status.as_ref())
            .filter(|status| status_ids.insert(status.id))
            .cloned()
            .collect();
        Ok(GroupedNotificationsProjection {
            accounts,
            statuses,
            groups,
        })
    }

    /// Loads one v2 notification group without pagination metadata.
    ///
    /// # Errors
    ///
    /// Returns a database error when notification or dependent projections fail.
    pub async fn grouped_notification(
        &self,
        account_id: i64,
        group_key: &str,
    ) -> sqlx::Result<Option<GroupedNotificationsProjection>> {
        let Some(notification) = self
            .repository
            .rest_notification_by_group_key(account_id, group_key)
            .await?
        else {
            return Ok(None);
        };
        let Some(notification) = self
            .notification_projections(vec![notification])
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let groupable = grouped_notification_types(&[]);
        let group_data = (groupable
            .iter()
            .any(|kind| kind == notification.notification_type.raw())
            && notification
                .group_key
                .as_deref()
                .is_some_and(|key| !key.trim().is_empty()))
        .then(|| notification.group_key.clone())
        .flatten()
        .map(|key| async move {
            self.repository
                .rest_notification_groups(
                    account_id,
                    std::slice::from_ref(&key),
                    0,
                    Some(i64::MAX),
                    false,
                )
                .await
        });
        let group_data = match group_data {
            Some(query) => query.await?.into_iter().next(),
            None => None,
        };
        let (group_key, sample_accounts, notifications_count, most_recent_notification_id) =
            match group_data {
                Some(group) => (
                    group.group_key,
                    self.accounts(&group.sample_account_ids).await?,
                    group.notifications_count,
                    group.most_recent_notification_id,
                ),
                None => (
                    format!("ungrouped-{}", notification.id),
                    vec![notification.account.clone()],
                    1,
                    notification.id,
                ),
            };
        let accounts = sample_accounts.clone();
        let statuses = notification.status.clone().into_iter().collect();
        Ok(Some(GroupedNotificationsProjection {
            accounts,
            statuses,
            groups: vec![NotificationGroupProjection {
                notification,
                group_key,
                sample_accounts,
                notifications_count,
                most_recent_notification_id,
                page_min_id: None,
                page_max_id: None,
                latest_page_notification_at: None,
            }],
        }))
    }

    async fn hydrate_account_emojis(
        &self,
        accounts: &mut BTreeMap<i64, AccountProjection>,
    ) -> sqlx::Result<()> {
        let demands = accounts
            .values()
            .map(|account| {
                (
                    account.domain.clone(),
                    emoji_shortcodes(
                        std::iter::once(account.display_name.as_str())
                            .chain(std::iter::once(account.note.as_str()))
                            .chain(
                                account
                                    .fields
                                    .iter()
                                    .flat_map(|field| [field.name.as_str(), field.value.as_str()]),
                            ),
                    ),
                )
            })
            .collect::<Vec<_>>();
        let emojis = self.load_custom_emojis(&demands).await?;
        for account in accounts.values_mut() {
            let shortcodes = emoji_shortcodes(
                std::iter::once(account.display_name.as_str())
                    .chain(std::iter::once(account.note.as_str()))
                    .chain(
                        account
                            .fields
                            .iter()
                            .flat_map(|field| [field.name.as_str(), field.value.as_str()]),
                    ),
            );
            account.emojis = project_emojis(account.domain.as_ref(), &shortcodes, &emojis);
        }
        Ok(())
    }

    async fn load_custom_emojis(
        &self,
        demands: &[(Option<String>, Vec<String>)],
    ) -> sqlx::Result<BTreeMap<(Option<String>, String), CustomEmojiProjection>> {
        let shortcodes = demands
            .iter()
            .flat_map(|(_, shortcodes)| shortcodes.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let domains = demands
            .iter()
            .filter_map(|(domain, _)| domain.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let include_local = demands.iter().any(|(domain, _)| domain.is_none());
        Ok(self
            .repository
            .rest_custom_emojis(&shortcodes, &domains, include_local)
            .await?
            .into_iter()
            .map(|emoji| {
                (
                    (emoji.domain.clone(), emoji.shortcode.clone()),
                    CustomEmojiProjection {
                        id: emoji.id,
                        shortcode: emoji.shortcode,
                        domain: emoji.domain,
                        file_name: emoji.image_file_name,
                        storage_schema_version: emoji.image_storage_schema_version,
                        visible_in_picker: emoji.visible_in_picker,
                        category: None,
                        featured: None,
                    },
                )
            })
            .collect())
    }
}

fn recent_tag_history() -> Vec<TagHistoryProjection> {
    let today = Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight is always a valid UTC time");
    (0..7)
        .map(|days| TagHistoryProjection {
            day: (today - Duration::days(days))
                .and_utc()
                .timestamp()
                .to_string(),
            accounts: "0".to_owned(),
            uses: "0".to_owned(),
        })
        .collect()
}

fn effective_notification_type(
    stored: Option<&NotificationType>,
    activity_type: &str,
) -> Option<NotificationType> {
    stored.cloned().or_else(|| {
        Some(match activity_type {
            "Mention" => NotificationType::Mention,
            "Status" => NotificationType::Reblog,
            "Follow" => NotificationType::Follow,
            "FollowRequest" => NotificationType::FollowRequest,
            "Favourite" => NotificationType::Favourite,
            "Poll" => NotificationType::Poll,
            "Quote" => NotificationType::Quote,
            _ => return None,
        })
    })
}

fn account_profile_handles(account: &AccountProjection, local_domain: &str) -> Vec<String> {
    mention_handles(
        std::iter::once(account.display_name.as_str())
            .chain(std::iter::once(account.note.as_str()))
            .chain(
                account
                    .fields
                    .iter()
                    .flat_map(|field| [field.name.as_str(), field.value.as_str()]),
            ),
        local_domain,
    )
}

fn complete_account_handle<'a>(
    query: &'a str,
    local_domain: &str,
) -> Option<(&'a str, Option<&'a str>)> {
    let (username, domain) = query.split_once('@')?;
    if !valid_account_handle_component(username)
        || !valid_account_handle_component(domain)
        || domain.contains('@')
    {
        return None;
    }
    Some((
        username,
        (!domain.eq_ignore_ascii_case(local_domain)).then_some(domain),
    ))
}

fn valid_account_handle_component(value: &str) -> bool {
    let mut awaiting_word = true;
    let mut has_word = false;
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'_' {
            awaiting_word = false;
            has_word = true;
        } else if matches!(byte, b'.' | b'-') {
            if awaiting_word {
                return false;
            }
            awaiting_word = true;
        } else {
            return false;
        }
    }
    has_word && !awaiting_word
}

fn account_search_tsquery(terms: &str) -> String {
    let terms = terms
        .chars()
        .map(|character| {
            matches!(character, '\'' | '?' | '\\' | ':' | '\u{2018}' | '\u{2019}')
                .then_some(' ')
                .unwrap_or(character)
        })
        .collect::<String>();
    format!("' {terms} ':*")
}

fn mention_handles<'a>(
    texts: impl IntoIterator<Item = &'a str>,
    local_domain: &str,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut handles = Vec::new();
    for text in texts {
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != b'@'
                || (index > 0
                    && (bytes[index - 1].is_ascii_alphanumeric()
                        || matches!(bytes[index - 1], b'_' | b'@')))
            {
                index += 1;
                continue;
            }
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end == start {
                index += 1;
                continue;
            }
            let username_end = end;
            if end < bytes.len() && bytes[end] == b'@' {
                let domain_start = end + 1;
                end = domain_start;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'.' | b'-'))
                {
                    end += 1;
                }
                if end == domain_start {
                    index += 1;
                    continue;
                }
            }
            let domain = (end > username_end).then(|| &text[username_end + 1..end]);
            let handle_end =
                if domain.is_some_and(|domain| domain.eq_ignore_ascii_case(local_domain)) {
                    username_end
                } else {
                    end
                };
            let handle = String::from_utf8_lossy(&bytes[start..handle_end]).to_lowercase();
            if seen.insert(handle.clone()) {
                handles.push(handle);
            }
            index = end;
        }
    }
    handles
}

fn setting_string(settings: &BTreeMap<String, String>, name: &str) -> Option<String> {
    let value = settings.get(name)?.trim();
    let value = value.strip_prefix("---").unwrap_or(value).trim();
    if value.is_empty() || matches!(value, "null" | "~") {
        return None;
    }
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return serde_json::from_str(value).ok();
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return Some(value[1..value.len() - 1].replace("''", "'"));
    }
    Some(value.to_owned())
}

fn setting_bool(settings: &BTreeMap<String, String>, name: &str) -> Option<bool> {
    match setting_string(settings, name)?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn setting_i32(settings: &BTreeMap<String, String>, name: &str) -> Option<i32> {
    setting_string(settings, name)?.parse().ok()
}

fn emoji_shortcodes<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut shortcodes = Vec::new();
    for text in texts {
        let bytes = text.as_bytes();
        let mut start = 0;
        while let Some(relative) = bytes[start..].iter().position(|byte| *byte == b':') {
            let opening = start + relative;
            let content_start = opening + 1;
            let Some(closing_relative) =
                bytes[content_start..].iter().position(|byte| *byte == b':')
            else {
                break;
            };
            let closing = content_start + closing_relative;
            let shortcode = &bytes[content_start..closing];
            let valid_boundary = (opening == 0
                || (!bytes[opening - 1].is_ascii_alphanumeric() && bytes[opening - 1] != b':'))
                && (closing + 1 == bytes.len()
                    || (!bytes[closing + 1].is_ascii_alphanumeric() && bytes[closing + 1] != b':'));
            if shortcode.len() >= 2
                && shortcode
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                && valid_boundary
            {
                let shortcode = String::from_utf8_lossy(shortcode).into_owned();
                if seen.insert(shortcode.clone()) {
                    shortcodes.push(shortcode);
                }
            }
            start = closing + 1;
        }
    }
    shortcodes
}

fn hashtag_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    let mut characters = text.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if !matches!(character, '#' | '＃')
            || (index > 0
                && text[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|previous| !previous.is_whitespace()))
        {
            continue;
        }
        let mut name = String::new();
        while let Some((_, candidate)) = characters.peek() {
            if candidate.is_alphanumeric()
                || matches!(
                    candidate,
                    '_' | '·' | '・' | '\u{200c}' | '\u{0e47}'..='\u{0e4e}'
                )
            {
                name.push(*candidate);
                characters.next();
            } else {
                break;
            }
        }
        let normalized = normalize_hashtag(&name);
        if !normalized.is_empty()
            && normalized.chars().any(char::is_alphabetic)
            && seen.insert(normalized.clone())
        {
            names.push(normalized);
        }
    }
    names
}

fn project_emojis(
    domain: Option<&String>,
    shortcodes: &[String],
    emojis: &BTreeMap<(Option<String>, String), CustomEmojiProjection>,
) -> Vec<CustomEmojiProjection> {
    shortcodes
        .iter()
        .filter_map(|shortcode| emojis.get(&(domain.cloned(), shortcode.clone())).cloned())
        .collect()
}

fn searchable_text(
    status: &RestStatusRow,
    media: &BTreeMap<i64, Vec<crate::mastodon::MediaAttachment>>,
    polls: &BTreeMap<i64, PollProjection>,
) -> String {
    let text = if status.local == Some(true) || status.uri.is_none() {
        status.text.clone()
    } else {
        HtmlFormatter::remote_plain_text(&status.text)
    };
    let poll_options = polls.get(&status.id).map(|poll| {
        poll.options
            .iter()
            .map(|option| option.title.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    });
    let media_descriptions = media
        .get(&status.id)
        .into_iter()
        .flatten()
        .filter_map(|media| media.description.as_deref())
        .collect::<Vec<_>>()
        .join("\n\n");
    [
        Some(status.spoiler_text.clone()),
        Some(text),
        poll_options,
        Some(media_descriptions),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n\n")
}

fn keyword_match(text: &str, keyword: &str, whole_word: bool) -> Option<(usize, String)> {
    let folded_keyword = keyword.to_lowercase();
    let boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect::<Vec<_>>();
    for (start_index, start) in boundaries
        .iter()
        .copied()
        .enumerate()
        .take(boundaries.len() - 1)
    {
        for end in boundaries.iter().copied().skip(start_index + 1) {
            let candidate = &text[start..end];
            if candidate.to_lowercase() != folded_keyword {
                continue;
            }
            let starts_with_word = keyword.chars().next().is_some_and(is_word_character);
            let ends_with_word = keyword.chars().next_back().is_some_and(is_word_character);
            let leading_boundary = !whole_word
                || !starts_with_word
                || text[..start]
                    .chars()
                    .next_back()
                    .is_none_or(|character| !is_word_character(character));
            let trailing_boundary = !whole_word
                || !ends_with_word
                || text[end..]
                    .chars()
                    .next()
                    .is_none_or(|character| !is_word_character(character));
            if leading_boundary && trailing_boundary {
                return Some((start, candidate.to_owned()));
            }
        }
    }
    None
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

struct StatusBuildContext<'a> {
    accounts: &'a BTreeMap<i64, AccountProjection>,
    media: &'a BTreeMap<i64, Vec<crate::mastodon::MediaAttachment>>,
    mentions: &'a BTreeMap<i64, Vec<super::RestMentionRow>>,
    authorization_mentions: &'a BTreeMap<i64, Vec<super::RestMentionRow>>,
    tags: &'a BTreeMap<i64, Vec<super::RestStatusTagRow>>,
    polls: &'a BTreeMap<i64, PollProjection>,
    quotes: &'a BTreeMap<i64, crate::mastodon::Quote>,
    quote_targets: &'a BTreeMap<i64, &'a RestStatusRow>,
    filter_results: &'a BTreeMap<i64, Vec<FilterResultProjection>>,
    preview_cards: &'a BTreeMap<i64, PreviewCardProjection>,
    tagged_collections: &'a BTreeMap<i64, Vec<CollectionProjection>>,
    status_emojis: &'a BTreeMap<i64, Vec<CustomEmojiProjection>>,
    viewer_account_id: Option<i64>,
}

pub(crate) fn media_projection(
    media: &crate::mastodon::MediaAttachment,
    description: Option<String>,
) -> MediaAttachmentProjection {
    MediaAttachmentProjection {
        id: media.id,
        media_type: media.media_type.0,
        processing: media.processing.map(|processing| processing.0),
        remote_url: media.remote_url.clone(),
        file_content_type: media.file_content_type.clone(),
        file_name: media.file_file_name.clone(),
        file_storage_schema_version: media.file_storage_schema_version,
        thumbnail_file_name: media.thumbnail_file_name.clone(),
        thumbnail_storage_schema_version: media.thumbnail_storage_schema_version,
        thumbnail_remote_url: media.thumbnail_remote_url.clone(),
        shortcode: media.shortcode.clone(),
        meta: media.file_meta.clone(),
        description,
        blurhash: media.blurhash.clone(),
        discarded: false,
    }
}

#[allow(clippy::too_many_lines)]
fn build_status(
    row: &RestStatusRow,
    reblog: Option<&RestStatusRow>,
    context: &StatusBuildContext<'_>,
    quote_depth: u8,
) -> Option<StatusProjection> {
    let account = context.accounts.get(&row.account_id)?.clone();
    let nested =
        reblog.and_then(|nested| build_status(nested, None, context, quote_depth).map(Box::new));
    let proper = reblog.unwrap_or(row);
    let quote_automatic = policy_keys(proper.quote_approval_policy >> 16);
    let quote_manual = policy_keys(proper.quote_approval_policy & 0xffff);
    let quote_current_user = quote_policy_for(proper, context.viewer_account_id);
    let author_settings = row
        .author_settings
        .as_deref()
        .and_then(|settings| serde_json::from_str::<Value>(settings).ok());
    let author_shows_application = author_settings
        .as_ref()
        .and_then(|settings| settings.get("show_application"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let viewer = context
        .viewer_account_id
        .map(|viewer_account_id| StatusViewerProjection {
            viewer_account_id,
            favourited: row.favourited,
            reblogged: row.reblogged,
            muted: row.muted,
            bookmarked: row.bookmarked,
            pinned: (viewer_account_id == row.account_id
                && row.reblog_of_id.is_none()
                && matches!(row.visibility, 0..=2))
            .then_some(row.pinned),
            filtered: context
                .filter_results
                .get(&row.id)
                .cloned()
                .unwrap_or_default(),
        });
    let quote = context.quotes.get(&row.id).and_then(|quote| {
        let target = quote
            .quoted_status_id
            .and_then(|id| context.quote_targets.get(&id).copied());
        let stored_state = quote_state(quote.state.0);
        if quote.legacy && stored_state != "accepted" {
            return None;
        }
        let filter_state = target.map(|target| quote_filter_state(target, context));
        let state = if stored_state == "accepted" {
            filter_state.unwrap_or("deleted")
        } else {
            stored_state
        }
        .to_owned();
        let target_access = if target.is_none() {
            QuoteTargetAccess::Deleted
        } else if filter_state == Some("unauthorized") {
            QuoteTargetAccess::Unauthorized
        } else {
            QuoteTargetAccess::Visible
        };
        let visible_target = (target_access == QuoteTargetAccess::Visible)
            .then_some(target)
            .flatten();
        Some(QuoteProjection {
            state,
            accepted: stored_state == "accepted",
            quoted_status_id: quote.quoted_status_id,
            target_access,
            target_serializable: visible_target.is_some_and(|target| {
                target.reblog_of_id.is_none() && context.accounts.contains_key(&target.account_id)
            }),
            target_link: visible_target.and_then(|target| {
                Some(super::QuoteTargetLinkProjection {
                    id: target.id,
                    account: context.accounts.get(&target.account_id)?.clone(),
                    local: target.local == Some(true) || target.uri.is_none(),
                    stored_url: target.url.clone(),
                })
            }),
            quoted_status: (quote_depth > 0)
                .then_some(visible_target)
                .flatten()
                .filter(|target| target.reblog_of_id.is_none())
                .and_then(|target| {
                    build_status(target, None, context, quote_depth - 1).map(Box::new)
                }),
        })
    });
    Some(StatusProjection {
        id: row.id,
        account,
        text: row.text.clone(),
        spoiler_text: row.spoiler_text.clone(),
        visibility: row.visibility,
        local: row.local == Some(true) || row.uri.is_none(),
        stored_uri: row.uri.clone(),
        stored_url: row.url.clone(),
        language: row.language.clone(),
        sensitive: row.sensitive,
        in_reply_to_id: row.in_reply_to_id,
        in_reply_to_account_id: row.in_reply_to_account_id,
        replies_count: row.replies_count,
        reblogs_count: row.reblogs_count,
        favourites_count: row.favourites_count,
        quotes_count: row.quotes_count,
        edited_at: row.edited_at,
        created_at: row.created_at,
        viewer,
        reblog: nested,
        show_application: (row.author_has_user && author_shows_application)
            || context.viewer_account_id == Some(row.account_id),
        application: row
            .application_name
            .as_ref()
            .map(|name| StatusApplicationProjection {
                name: name.clone(),
                website: row.application_website.clone(),
            }),
        media_attachments: context
            .media
            .get(&row.id)
            .into_iter()
            .flatten()
            .map(|media| media_projection(media, media.description.clone()))
            .collect(),
        mentions: context
            .mentions
            .get(&row.id)
            .into_iter()
            .flatten()
            .filter_map(|mention| context.accounts.get(&mention.account_id).cloned())
            .map(|account| MentionProjection { account })
            .collect(),
        tags: context
            .tags
            .get(&row.id)
            .into_iter()
            .flatten()
            .map(|tag| TagProjection {
                id: tag.id,
                name: tag.name.clone(),
                display_name: tag.display_name.clone(),
                history: Vec::new(),
                following: None,
                featuring: None,
            })
            .collect(),
        emojis: context
            .status_emojis
            .get(&row.id)
            .cloned()
            .unwrap_or_default(),
        tagged_collections: context
            .tagged_collections
            .get(&row.id)
            .cloned()
            .unwrap_or_default(),
        quote,
        card: context.preview_cards.get(&row.id).cloned(),
        poll: context.polls.get(&row.id).cloned(),
        quote_automatic,
        quote_manual,
        quote_current_user,
    })
}

fn quote_filter_state(row: &RestStatusRow, context: &StatusBuildContext<'_>) -> &'static str {
    quote_filter_state_from(
        status_access(status_access_facts(row, context)).is_allowed(),
        context.viewer_account_id == Some(row.account_id),
        if row.viewer_domain_blocks_author {
            QuoteViewerRestriction::DomainBlock
        } else if row.viewer_blocks_author {
            QuoteViewerRestriction::AccountBlock
        } else if row.viewer_mutes_author {
            QuoteViewerRestriction::Mute
        } else {
            QuoteViewerRestriction::None
        },
    )
}

#[derive(Clone, Copy)]
enum QuoteViewerRestriction {
    None,
    DomainBlock,
    AccountBlock,
    Mute,
}

const fn quote_filter_state_from(
    authorized: bool,
    viewer_is_author: bool,
    restriction: QuoteViewerRestriction,
) -> &'static str {
    if !authorized {
        "unauthorized"
    } else if viewer_is_author {
        "accepted"
    } else {
        match restriction {
            QuoteViewerRestriction::None => "accepted",
            QuoteViewerRestriction::DomainBlock => "blocked_domain",
            QuoteViewerRestriction::AccountBlock => "blocked_account",
            QuoteViewerRestriction::Mute => "muted_account",
        }
    }
}

fn status_access_facts(row: &RestStatusRow, context: &StatusBuildContext<'_>) -> StatusAccessFacts {
    let viewer_account_id = context.viewer_account_id;
    let mentioned = viewer_account_id.is_some_and(|viewer_account_id| {
        context
            .authorization_mentions
            .get(&row.id)
            .is_some_and(|mentions| {
                mentions
                    .iter()
                    .any(|mention| mention.account_id == viewer_account_id)
            })
    });
    StatusAccessFacts {
        visibility: StatusVisibility::from(row.visibility),
        availability: if row.author_suspended {
            StatusAvailability::AuthorSuspended
        } else {
            StatusAvailability::Available
        },
        viewer: viewer_account_id.map_or(ViewerFacts::Anonymous, |viewer_account_id| {
            ViewerFacts::Authenticated(AuthenticatedViewerFacts {
                is_author: viewer_account_id == row.account_id,
                follows_author: row.viewer_follows_author,
                is_mentioned: mentioned,
                author_restriction: if row.author_blocks_viewer {
                    AuthorRestriction::BlocksViewer
                } else if row.author_domain_blocks_viewer {
                    AuthorRestriction::BlocksViewerDomain
                } else {
                    AuthorRestriction::None
                },
            })
        }),
    }
}

fn quote_state(value: i32) -> &'static str {
    match value {
        0 => "pending",
        1 => "accepted",
        2 => "rejected",
        3 => "revoked",
        _ => "deleted",
    }
}

fn quote_policy_for(row: &RestStatusRow, viewer_account_id: Option<i64>) -> String {
    let Some(viewer_account_id) = viewer_account_id else {
        return "denied".to_owned();
    };
    if row.visibility == 3 || row.reblog_of_id.is_some() {
        return "denied".to_owned();
    }
    if viewer_account_id == row.account_id {
        return "automatic".to_owned();
    }
    if policy_allows(
        row.quote_approval_policy >> 16,
        row.viewer_follows_author,
        row.author_follows_viewer,
    ) {
        "automatic".to_owned()
    } else if policy_allows(
        row.quote_approval_policy & 0xffff,
        row.viewer_follows_author,
        row.author_follows_viewer,
    ) {
        "manual".to_owned()
    } else if row.quote_approval_policy & 0x0001_0001 != 0 {
        "unknown".to_owned()
    } else {
        "denied".to_owned()
    }
}

#[allow(clippy::too_many_lines)]
fn account_projection(
    row: RestAccountRow,
    moved: Option<Box<AccountProjection>>,
    viewer_account_id: Option<i64>,
) -> AccountProjection {
    let local = row.domain.is_none();
    let (automatic, manual) = if local {
        if row.discoverable == Some(true) {
            (
                vec![if row.locked { "followers" } else { "public" }.to_owned()],
                Vec::new(),
            )
        } else {
            (Vec::new(), Vec::new())
        }
    } else {
        (
            policy_keys(row.feature_approval_policy >> 16),
            policy_keys(row.feature_approval_policy & 0xffff),
        )
    };
    let current_user = if viewer_account_id.is_none() || (local && row.discoverable != Some(true)) {
        "denied"
    } else if viewer_account_id == Some(row.id) {
        "automatic"
    } else if local {
        if row.locked && !row.viewer_follows {
            "denied"
        } else {
            "automatic"
        }
    } else if row.feature_approval_policy == 0 {
        "missing"
    } else if policy_allows(
        row.feature_approval_policy >> 16,
        row.viewer_follows,
        row.follows_viewer,
    ) {
        "automatic"
    } else if policy_allows(
        row.feature_approval_policy & 0xffff,
        row.viewer_follows,
        row.follows_viewer,
    ) {
        "manual"
    } else if row.feature_approval_policy & 0x0001_0001 != 0 {
        "unknown"
    } else {
        "denied"
    };
    let noindex = local.then(|| {
        row.user_settings
            .as_deref()
            .and_then(|settings| serde_json::from_str::<Value>(settings).ok())
            .and_then(|settings| settings.get("noindex").and_then(Value::as_bool))
            .unwrap_or(false)
    });
    let roles = local.then(|| {
        match (
            row.role_id,
            row.role_name,
            row.role_color,
            row.role_highlighted,
        ) {
            (Some(id), Some(name), Some(color), Some(true)) => {
                vec![AccountRoleProjection { id, name, color }]
            }
            _ => Vec::new(),
        }
    });
    AccountProjection {
        id: row.id,
        username: row.username,
        domain: row.domain,
        actor_type: row.actor_type,
        id_scheme: row.id_scheme,
        display_name: row.display_name,
        note: row.note,
        stored_uri: row.uri,
        stored_url: row.url,
        locked: row.locked,
        discoverable: row.discoverable,
        indexable: row.indexable,
        memorial: row.memorial,
        moved,
        suspended: row.suspended,
        limited: row.limited,
        sensitized: row.sensitized,
        created_at: row.created_at,
        avatar_file_name: row.avatar_file_name,
        avatar_content_type: row.avatar_content_type,
        avatar_storage_schema_version: row.avatar_storage_schema_version,
        avatar_description: row.avatar_description,
        header_file_name: row.header_file_name,
        header_content_type: row.header_content_type,
        header_storage_schema_version: row.header_storage_schema_version,
        header_description: row.header_description,
        followers_count: row.followers_count,
        following_count: row.following_count,
        statuses_count: row.statuses_count,
        last_status_at: row.last_status_at,
        hide_collections: row.hide_collections,
        show_media: row.show_media,
        show_media_replies: row.show_media_replies,
        show_featured: row.show_featured,
        noindex,
        feature_automatic: automatic,
        feature_manual: manual,
        feature_current_user: current_user.to_owned(),
        email_subscriptions: None,
        roles,
        emojis: Vec::new(),
        fields: parse_fields(row.fields),
        profile_mentions: Vec::new(),
    }
}

fn parse_fields(fields: Option<Value>) -> Vec<AccountFieldProjection> {
    fields
        .and_then(|fields| fields.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|field| {
            let field = field.as_object()?;
            let name = field.get("name")?.as_str()?.trim().to_owned();
            let value = field.get("value")?.as_str()?.trim().to_owned();
            let verified_at = field
                .get("verified_at")
                .and_then(Value::as_str)
                .and_then(parse_datetime);
            Some(AccountFieldProjection {
                name,
                value,
                verified_at,
            })
        })
        .collect()
}

fn parse_datetime(value: &str) -> Option<NaiveDateTime> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.naive_utc())
        .or_else(|| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f").ok())
}

fn policy_keys(bitmap: i32) -> Vec<String> {
    [
        (0, "unsupported_policy"),
        (1, "public"),
        (2, "followers"),
        (3, "following"),
        (4, "disabled"),
    ]
    .into_iter()
    .filter(|(bit, _)| bitmap & (1 << bit) != 0)
    .map(|(_, name)| name.to_owned())
    .collect()
}

fn policy_allows(bitmap: i32, viewer_follows: bool, follows_viewer: bool) -> bool {
    bitmap & (1 << 1) != 0
        || (bitmap & (1 << 2) != 0 && viewer_follows)
        || (bitmap & (1 << 3) != 0 && follows_viewer)
}

#[cfg(test)]
mod tests {
    use super::{QuoteViewerRestriction, hashtag_names, quote_filter_state_from};

    #[test]
    fn announcement_hashtags_follow_mastodon_boundaries_and_separators() {
        assert_eq!(
            hashtag_names("word#ignored #FixtureTag #foo\u{00b7}bar #123"),
            ["fixturetag", "foo\u{00b7}bar"]
        );
    }

    #[test]
    fn quote_filter_never_bypasses_failed_status_authorization() {
        assert_eq!(
            quote_filter_state_from(false, true, QuoteViewerRestriction::None),
            "unauthorized"
        );
        assert_eq!(
            quote_filter_state_from(false, true, QuoteViewerRestriction::DomainBlock),
            "unauthorized"
        );
        assert_eq!(
            quote_filter_state_from(true, true, QuoteViewerRestriction::DomainBlock),
            "accepted"
        );
    }
}
