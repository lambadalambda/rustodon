#![allow(clippy::struct_excessive_bools)]

use std::collections::BTreeMap;

use chrono::NaiveDateTime;
use serde_json::Value;

use super::MentionTarget;
use crate::mastodon::AccountIdScheme;

#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct RestAccountRow {
    pub id: i64,
    pub username: String,
    pub domain: Option<String>,
    pub actor_type: Option<String>,
    pub id_scheme: Option<AccountIdScheme>,
    pub display_name: String,
    pub note: String,
    pub uri: String,
    pub url: Option<String>,
    pub locked: bool,
    pub discoverable: Option<bool>,
    pub indexable: bool,
    pub memorial: bool,
    pub moved_to_account_id: Option<i64>,
    pub suspended: bool,
    pub limited: bool,
    pub sensitized: bool,
    pub created_at: NaiveDateTime,
    pub avatar_file_name: Option<String>,
    pub avatar_content_type: Option<String>,
    pub avatar_storage_schema_version: Option<i32>,
    pub avatar_description: String,
    pub header_file_name: Option<String>,
    pub header_content_type: Option<String>,
    pub header_storage_schema_version: Option<i32>,
    pub header_description: String,
    pub followers_count: i64,
    pub following_count: i64,
    pub statuses_count: i64,
    pub last_status_at: Option<NaiveDateTime>,
    pub hide_collections: Option<bool>,
    pub show_media: bool,
    pub show_media_replies: bool,
    pub show_featured: bool,
    pub feature_approval_policy: i32,
    pub user_settings: Option<String>,
    pub role_id: Option<i64>,
    pub role_name: Option<String>,
    pub role_color: Option<String>,
    pub role_highlighted: Option<bool>,
    pub viewer_follows: bool,
    pub follows_viewer: bool,
    pub fields: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct RestAccountHandleRow {
    pub id: i64,
    pub handle: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct RestCredentialRow {
    pub settings: Option<String>,
    pub role_id: i64,
    pub role_name: String,
    pub role_permissions: i64,
    pub everyone_permissions: i64,
    pub role_color: String,
    pub role_highlighted: bool,
    pub collection_limit: i32,
    pub attribution_domains: Option<Vec<String>>,
    pub follow_requests_count: i64,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct RestPreferencesRow {
    pub settings: Option<String>,
    pub locale: Option<String>,
    pub locked: bool,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct RestRelationshipRow {
    pub target_account_id: i64,
    pub following: bool,
    pub showing_reblogs: bool,
    pub notifying: bool,
    pub languages: Option<Vec<String>>,
    pub followed_by: bool,
    pub blocking: bool,
    pub blocked_by: bool,
    pub muting: bool,
    pub muting_notifications: bool,
    pub muting_expires_at: Option<NaiveDateTime>,
    pub requested: bool,
    pub requested_by: bool,
    pub domain_blocking: bool,
    pub endorsed: bool,
    pub note: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct RestStatusRow {
    pub id: i64,
    pub account_id: i64,
    pub text: String,
    pub spoiler_text: String,
    pub visibility: i32,
    pub local: Option<bool>,
    pub uri: Option<String>,
    pub url: Option<String>,
    pub language: Option<String>,
    pub sensitive: bool,
    pub in_reply_to_id: Option<i64>,
    pub in_reply_to_account_id: Option<i64>,
    pub reblog_of_id: Option<i64>,
    pub quote_approval_policy: i32,
    pub edited_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
    pub replies_count: i64,
    pub reblogs_count: i64,
    pub favourites_count: i64,
    pub quotes_count: i64,
    pub application_name: Option<String>,
    pub application_website: Option<String>,
    pub author_has_user: bool,
    pub author_settings: Option<String>,
    pub viewer_follows_author: bool,
    pub author_follows_viewer: bool,
    pub author_blocks_viewer: bool,
    pub author_domain_blocks_viewer: bool,
    pub author_suspended: bool,
    pub viewer_blocks_author: bool,
    pub viewer_domain_blocks_author: bool,
    pub viewer_mutes_author: bool,
    pub favourited: bool,
    pub reblogged: bool,
    pub muted: bool,
    pub bookmarked: bool,
    pub pinned: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountStatusesOptions {
    pub max_id: Option<i64>,
    pub min_id: Option<i64>,
    pub since_id: Option<i64>,
    pub limit: i64,
    pub pinned: bool,
    pub tagged: Option<String>,
    pub only_media: bool,
    pub exclude_replies: bool,
    pub exclude_reblogs: bool,
    pub exclude_direct: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineOptions {
    pub max_id: Option<i64>,
    pub min_id: Option<i64>,
    pub since_id: Option<i64>,
    pub limit: i64,
    pub local: bool,
    pub remote: bool,
    pub only_media: bool,
}

impl Default for TimelineOptions {
    fn default() -> Self {
        Self {
            max_id: None,
            min_id: None,
            since_id: None,
            limit: 20,
            local: false,
            remote: false,
            only_media: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct TagTimelineOptions {
    pub page: TimelineOptions,
    pub any: Vec<String>,
    pub all: Vec<String>,
    pub none: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SavedStatusKind {
    Favourites,
    Bookmarks,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedStatusesOptions {
    pub max_id: Option<i64>,
    pub min_id: Option<i64>,
    pub since_id: Option<i64>,
    pub limit: i64,
}

impl Default for SavedStatusesOptions {
    fn default() -> Self {
        Self {
            max_id: None,
            min_id: None,
            since_id: None,
            limit: 20,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationOptions {
    pub max_id: Option<i64>,
    pub min_id: Option<i64>,
    pub since_id: Option<i64>,
    pub limit: i64,
    pub account_id: Option<i64>,
    pub types: Option<Vec<String>>,
    pub exclude_types: Vec<String>,
    pub grouped_types: Vec<String>,
    pub include_filtered: bool,
}

impl Default for NotificationOptions {
    fn default() -> Self {
        Self {
            max_id: None,
            min_id: None,
            since_id: None,
            limit: 40,
            account_id: None,
            types: None,
            exclude_types: Vec::new(),
            grouped_types: Vec::new(),
            include_filtered: false,
        }
    }
}

pub const KNOWN_NOTIFICATION_TYPES: [&str; 17] = [
    "mention",
    "status",
    "reblog",
    "follow",
    "follow_request",
    "favourite",
    "poll",
    "update",
    "severed_relationships",
    "moderation_warning",
    "annual_report",
    "admin.sign_up",
    "admin.report",
    "quote",
    "quoted_update",
    "added_to_collection",
    "collection_update",
];

pub const GROUPABLE_NOTIFICATION_TYPES: [&str; 4] =
    ["favourite", "reblog", "follow", "admin.sign_up"];

#[must_use]
pub fn notification_type_filter(requested_types: &[String]) -> Option<Vec<String>> {
    if requested_types.is_empty() {
        return None;
    }
    let known = KNOWN_NOTIFICATION_TYPES
        .into_iter()
        .filter(|kind| requested_types.iter().any(|requested| requested == kind))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (known.len() != KNOWN_NOTIFICATION_TYPES.len()).then_some(known)
}

#[must_use]
pub fn notification_type_filter_with_exclusions(
    requested_types: &[String],
    excluded_types: &[String],
) -> Option<Vec<String>> {
    let excluded = KNOWN_NOTIFICATION_TYPES
        .into_iter()
        .filter(|kind| excluded_types.iter().any(|excluded| excluded == kind))
        .collect::<Vec<_>>();
    if excluded.is_empty() {
        return notification_type_filter(requested_types);
    }
    let mut allowed = notification_type_filter(requested_types).unwrap_or_else(|| {
        KNOWN_NOTIFICATION_TYPES
            .into_iter()
            .map(str::to_owned)
            .collect()
    });
    allowed.retain(|kind| !excluded.iter().any(|excluded| excluded == kind));
    (allowed.len() != KNOWN_NOTIFICATION_TYPES.len()).then_some(allowed)
}

#[must_use]
pub fn grouped_notification_types(requested_types: &[String]) -> Vec<String> {
    if requested_types.is_empty() {
        return GROUPABLE_NOTIFICATION_TYPES
            .into_iter()
            .map(str::to_owned)
            .collect();
    }
    GROUPABLE_NOTIFICATION_TYPES
        .into_iter()
        .filter(|kind| requested_types.iter().any(|requested| requested == kind))
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedStatusesPage {
    pub statuses: Vec<StatusProjection>,
    pub first_cursor: Option<i64>,
    pub last_cursor: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusQuotesPage {
    pub statuses: Vec<StatusProjection>,
    pub first_cursor: Option<i64>,
    pub last_cursor: Option<i64>,
    pub records_continue: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct RestSavedStatusRow {
    pub cursor_id: i64,
    pub status_id: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct RestStatusQuoteRow {
    pub quote_id: i64,
    pub status_id: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountListKind {
    Blocks,
    Mutes,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountListOptions {
    pub max_id: Option<i64>,
    pub since_id: Option<i64>,
    pub limit: i64,
}

impl Default for AccountListOptions {
    fn default() -> Self {
        Self {
            max_id: None,
            since_id: None,
            limit: 40,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountListEntryProjection {
    pub account: AccountProjection,
    pub mute_expires_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountListPage {
    pub entries: Vec<AccountListEntryProjection>,
    pub first_cursor: Option<i64>,
    pub last_cursor: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct RestAccountListRow {
    pub cursor_id: i64,
    pub account_id: i64,
    pub mute_expires_at: Option<NaiveDateTime>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FollowCollectionKind {
    Followers,
    Following,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FollowCollectionOptions {
    pub max_id: Option<i64>,
    pub since_id: Option<i64>,
    pub limit: i64,
}

impl Default for FollowCollectionOptions {
    fn default() -> Self {
        Self {
            max_id: None,
            since_id: None,
            limit: 40,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FollowCollectionPage {
    pub accounts: Vec<AccountProjection>,
    pub first_cursor: Option<i64>,
    pub last_cursor: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct RestFollowCollectionRow {
    pub follow_id: i64,
    pub account_id: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusContextProjection {
    pub ancestors: Vec<StatusProjection>,
    pub descendants: Vec<StatusProjection>,
}

impl Default for AccountStatusesOptions {
    fn default() -> Self {
        Self {
            max_id: None,
            min_id: None,
            since_id: None,
            limit: 20,
            pinned: false,
            tagged: None,
            only_media: false,
            exclude_replies: false,
            exclude_reblogs: false,
            exclude_direct: false,
        }
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct RestMentionRow {
    pub status_id: i64,
    pub account_id: i64,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct RestStatusTagRow {
    pub status_id: i64,
    pub id: i64,
    pub name: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct RestPollVoteRow {
    pub poll_id: i64,
    pub choice: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountFieldProjection {
    pub name: String,
    pub value: String,
    pub verified_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountRoleProjection {
    pub id: i64,
    pub name: String,
    pub color: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomEmojiProjection {
    pub id: i64,
    pub shortcode: String,
    pub domain: Option<String>,
    pub file_name: String,
    pub storage_schema_version: Option<i32>,
    pub visible_in_picker: bool,
    pub category: Option<String>,
    pub featured: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct RestCustomEmojiRow {
    pub id: i64,
    pub shortcode: String,
    pub domain: Option<String>,
    pub image_file_name: String,
    pub image_storage_schema_version: Option<i32>,
    pub visible_in_picker: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct RestListedCustomEmojiRow {
    pub id: i64,
    pub shortcode: String,
    pub domain: Option<String>,
    pub image_file_name: String,
    pub image_storage_schema_version: Option<i32>,
    pub visible_in_picker: bool,
    pub category: Option<String>,
    pub featured: bool,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct RestPreviewCardRow {
    pub status_id: i64,
    pub original_url: Option<String>,
    pub id: i64,
    pub url: String,
    pub title: String,
    pub description: String,
    pub language: Option<String>,
    pub card_type: i32,
    pub author_name: String,
    pub author_url: String,
    pub author_account_id: Option<i64>,
    pub unverified_author_account_id: Option<i64>,
    pub provider_name: String,
    pub provider_url: String,
    pub html: String,
    pub width: i32,
    pub height: i32,
    pub image_file_name: Option<String>,
    pub image_storage_schema_version: Option<i32>,
    pub image_description: String,
    pub embed_url: String,
    pub blurhash: Option<String>,
    pub published_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewCardProjection {
    pub original_url: Option<String>,
    pub id: i64,
    pub url: String,
    pub title: String,
    pub description: String,
    pub language: Option<String>,
    pub card_type: i32,
    pub author_name: String,
    pub author_url: String,
    pub author_account: Option<AccountProjection>,
    pub unverified_author_account_id: Option<i64>,
    pub provider_name: String,
    pub provider_url: String,
    pub html: String,
    pub width: i32,
    pub height: i32,
    pub image_file_name: Option<String>,
    pub image_storage_schema_version: Option<i32>,
    pub image_description: String,
    pub embed_url: String,
    pub blurhash: Option<String>,
    pub published_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AccountProjection {
    pub id: i64,
    pub username: String,
    pub domain: Option<String>,
    pub actor_type: Option<String>,
    pub id_scheme: Option<AccountIdScheme>,
    pub display_name: String,
    pub note: String,
    pub stored_uri: String,
    pub stored_url: Option<String>,
    pub locked: bool,
    pub discoverable: Option<bool>,
    pub indexable: bool,
    pub memorial: bool,
    pub moved: Option<Box<AccountProjection>>,
    pub suspended: bool,
    pub limited: bool,
    pub sensitized: bool,
    pub created_at: NaiveDateTime,
    pub avatar_file_name: Option<String>,
    pub avatar_content_type: Option<String>,
    pub avatar_storage_schema_version: Option<i32>,
    pub avatar_description: String,
    pub header_file_name: Option<String>,
    pub header_content_type: Option<String>,
    pub header_storage_schema_version: Option<i32>,
    pub header_description: String,
    pub followers_count: i64,
    pub following_count: i64,
    pub statuses_count: i64,
    pub last_status_at: Option<NaiveDateTime>,
    pub hide_collections: Option<bool>,
    pub show_media: bool,
    pub show_media_replies: bool,
    pub show_featured: bool,
    pub noindex: Option<bool>,
    pub feature_automatic: Vec<String>,
    pub feature_manual: Vec<String>,
    pub feature_current_user: String,
    pub email_subscriptions: Option<bool>,
    pub roles: Option<Vec<AccountRoleProjection>>,
    pub emojis: Vec<CustomEmojiProjection>,
    pub fields: Vec<AccountFieldProjection>,
    pub profile_mentions: Vec<AccountProjection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConversationProjection {
    pub id: i64,
    pub unread: bool,
    pub participant_accounts: Vec<AccountProjection>,
    pub last_status: Option<StatusProjection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NotificationRequestProjection {
    pub id: i64,
    pub account: AccountProjection,
    pub last_status: Option<StatusProjection>,
    pub notifications_count: i64,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl AccountProjection {
    #[must_use]
    pub fn local(&self) -> bool {
        self.domain.is_none()
    }

    #[must_use]
    pub fn mention_target<'a>(&'a self, url: &'a str) -> MentionTarget<'a> {
        MentionTarget {
            username: &self.username,
            domain: self.domain.as_deref(),
            url,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialRoleProjection {
    pub id: i64,
    pub name: String,
    pub permissions: i64,
    pub color: String,
    pub highlighted: bool,
    pub collection_limit: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CredentialAccountProjection {
    pub account: AccountProjection,
    pub privacy: String,
    pub sensitive: bool,
    pub language: Option<String>,
    pub follow_requests_count: i64,
    pub attribution_domains: Option<Vec<String>>,
    pub quote_policy: String,
    pub role: CredentialRoleProjection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreferencesProjection {
    pub posting_default_visibility: String,
    pub posting_default_sensitive: bool,
    pub posting_default_language: String,
    pub posting_default_quote_policy: String,
    pub reading_default_sensitive_media: String,
    pub reading_default_sensitive_text: bool,
    pub reading_autoplay_gifs: bool,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AccountRelationshipProjection {
    pub target_account_id: i64,
    pub following: bool,
    pub showing_reblogs: bool,
    pub notifying: bool,
    pub languages: Option<Vec<String>>,
    pub followed_by: bool,
    pub blocking: bool,
    pub blocked_by: bool,
    pub muting: bool,
    pub muting_notifications: bool,
    pub muting_expires_at: Option<NaiveDateTime>,
    pub requested: bool,
    pub requested_by: bool,
    pub domain_blocking: bool,
    pub endorsed: bool,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaAttachmentProjection {
    pub id: i64,
    pub media_type: i32,
    pub processing: Option<i32>,
    pub remote_url: String,
    pub file_content_type: Option<String>,
    pub file_name: Option<String>,
    pub file_storage_schema_version: Option<i32>,
    pub thumbnail_file_name: Option<String>,
    pub thumbnail_storage_schema_version: Option<i32>,
    pub thumbnail_remote_url: Option<String>,
    pub shortcode: Option<String>,
    pub meta: Option<Value>,
    pub description: Option<String>,
    pub blurhash: Option<String>,
    pub discarded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MentionProjection {
    pub account: AccountProjection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagProjection {
    pub id: i64,
    pub name: String,
    pub display_name: Option<String>,
    pub history: Vec<TagHistoryProjection>,
    pub following: Option<bool>,
    pub featuring: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagHistoryProjection {
    pub day: String,
    pub accounts: String,
    pub uses: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PollProjection {
    pub id: i64,
    pub expires_at: Option<NaiveDateTime>,
    pub multiple: bool,
    pub votes_count: i64,
    pub voters_count: Option<i64>,
    pub options: Vec<PollOptionProjection>,
    pub emojis: Vec<CustomEmojiProjection>,
    pub voted: Option<bool>,
    pub own_votes: Option<Vec<i32>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollOptionProjection {
    pub title: String,
    pub votes_count: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CollectionItemProjection {
    pub id: i64,
    pub account_id: Option<i64>,
    pub state: i32,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CollectionProjection {
    pub id: i64,
    pub account: AccountProjection,
    pub name: String,
    pub description: Option<String>,
    pub description_html: Option<String>,
    pub local: bool,
    pub sensitive: bool,
    pub discoverable: bool,
    pub language: Option<String>,
    pub stored_uri: Option<String>,
    pub stored_url: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub tag: Option<TagProjection>,
    pub items: Vec<CollectionItemProjection>,
    pub item_accounts: Vec<AccountProjection>,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct RestTaggedCollectionRow {
    pub status_id: i64,
    pub collection_id: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilterProjection {
    pub id: i64,
    pub title: String,
    pub context: Vec<String>,
    pub expires_at: Option<NaiveDateTime>,
    pub action: i32,
    pub keywords: Vec<FilterKeywordProjection>,
    pub statuses: Vec<FilterStatusProjection>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilterKeywordProjection {
    pub id: i64,
    pub keyword: String,
    pub whole_word: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilterStatusProjection {
    pub id: i64,
    pub status_id: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilterResultProjection {
    pub filter: FilterProjection,
    pub keyword_matches: Option<Vec<String>>,
    pub status_matches: Option<Vec<i64>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarkerProjection {
    pub timeline: String,
    pub last_read_id: i64,
    pub version: i32,
    pub updated_at: NaiveDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListProjection {
    pub id: i64,
    pub title: String,
    pub replies_policy: i32,
    pub exclusive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FollowedTagsOptions {
    pub max_id: Option<i64>,
    pub min_id: Option<i64>,
    pub since_id: Option<i64>,
    pub limit: i64,
}

impl Default for FollowedTagsOptions {
    fn default() -> Self {
        Self {
            max_id: None,
            min_id: None,
            since_id: None,
            limit: 100,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FollowedTagsPage {
    pub tags: Vec<TagProjection>,
    pub first_cursor: Option<i64>,
    pub last_cursor: Option<i64>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct RestFeaturedTagRow {
    pub id: i64,
    pub name: Option<String>,
    pub tag_name: String,
    pub tag_display_name: Option<String>,
    pub statuses_count: i64,
    pub last_status_at: Option<NaiveDateTime>,
    pub username: String,
    pub domain: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct RestTagSuggestionRow {
    pub id: i64,
    pub name: String,
    pub display_name: Option<String>,
    pub following: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub(crate) struct RestFollowedTagRow {
    pub tag_follow_id: i64,
    pub id: i64,
    pub name: String,
    pub display_name: Option<String>,
    pub featuring: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeaturedTagProjection {
    pub id: i64,
    pub name: String,
    pub tag_name: String,
    pub statuses_count: i64,
    pub last_status_at: Option<NaiveDateTime>,
    pub username: String,
    pub domain: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct RestNotificationTargetRow {
    pub notification_id: i64,
    pub status_id: Option<i64>,
    pub collection_id: Option<i64>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct RestNotificationGroupRow {
    pub group_key: String,
    pub most_recent_notification_id: i64,
    pub sample_account_ids: Vec<i64>,
    pub notifications_count: i64,
    pub page_min_id: i64,
    pub latest_page_notification_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NotificationProjection {
    pub id: i64,
    pub notification_type: crate::mastodon::NotificationType,
    pub created_at: NaiveDateTime,
    pub group_key: Option<String>,
    pub filtered: bool,
    pub account: AccountProjection,
    pub status: Option<StatusProjection>,
    pub collection: Option<CollectionProjection>,
    pub report: Option<ReportProjection>,
    pub event: Option<SeveranceEventProjection>,
    pub moderation_warning: Option<AccountWarningProjection>,
    pub annual_report_year: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NotificationGroupProjection {
    pub notification: NotificationProjection,
    pub group_key: String,
    pub sample_accounts: Vec<AccountProjection>,
    pub notifications_count: i64,
    pub most_recent_notification_id: i64,
    pub page_min_id: Option<i64>,
    pub page_max_id: Option<i64>,
    pub latest_page_notification_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GroupedNotificationsProjection {
    pub accounts: Vec<AccountProjection>,
    pub statuses: Vec<StatusProjection>,
    pub groups: Vec<NotificationGroupProjection>,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct RestInstanceCountsRow {
    pub user_count: i64,
    pub status_count: i64,
    pub domain_count: i64,
    pub everyone_permissions: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct RestRuleRow {
    pub id: i64,
    pub text: String,
    pub hint: String,
    pub language: Option<String>,
    pub translated_text: Option<String>,
    pub translated_hint: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuleProjection {
    pub id: i64,
    pub text: String,
    pub hint: String,
    pub translations: BTreeMap<String, (String, String)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceRuntimeConfig {
    pub domain: String,
    pub version: String,
    pub source_url: String,
    pub streaming_api: String,
    pub vapid_public_key: Option<String>,
    pub thumbnail_url: String,
    pub thumbnail_description: String,
    pub thumbnail_blurhash: Option<String>,
    pub thumbnail_versions: Option<(String, String)>,
    pub icons: Vec<(String, String)>,
    pub languages: Vec<String>,
    pub active_month: i64,
    pub active_halfyear: i64,
    pub translation_enabled: bool,
    pub limited_federation: bool,
    pub single_user_mode: bool,
    pub terms_of_service_url: Option<String>,
    pub sso_signup_url: Option<String>,
    pub wrapstodon: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstanceProjection {
    pub runtime: InstanceRuntimeConfig,
    pub title: String,
    pub short_description: String,
    pub legacy_description: String,
    pub contact_email: String,
    pub status_page_url: Option<String>,
    pub user_count: i64,
    pub status_count: i64,
    pub domain_count: i64,
    pub registrations_mode: String,
    pub require_invite_text: bool,
    pub closed_registrations_message: Option<String>,
    pub min_age: Option<i32>,
    pub invites_enabled: bool,
    pub local_live_feed_access: String,
    pub remote_live_feed_access: String,
    pub local_topic_feed_access: String,
    pub remote_topic_feed_access: String,
    pub contact_account: Option<AccountProjection>,
    pub rules: Vec<RuleProjection>,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct RestSeveranceEventRow {
    pub id: i64,
    pub event_type: i32,
    pub purged: bool,
    pub target_name: String,
    pub followers_count: i32,
    pub following_count: i32,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeveranceEventProjection {
    pub id: i64,
    pub event_type: i32,
    pub purged: bool,
    pub target_name: String,
    pub followers_count: i32,
    pub following_count: i32,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct RestAccountWarningRow {
    pub id: i64,
    pub action: i32,
    pub text: String,
    pub status_ids: Option<Vec<String>>,
    pub created_at: NaiveDateTime,
    pub target_account_id: Option<i64>,
    pub appeal_text: Option<String>,
    pub appeal_approved_at: Option<NaiveDateTime>,
    pub appeal_rejected_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppealProjection {
    pub text: String,
    pub approved: bool,
    pub rejected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountWarningProjection {
    pub id: i64,
    pub action: i32,
    pub text: String,
    pub status_ids: Option<Vec<i64>>,
    pub created_at: NaiveDateTime,
    pub target_account: Option<AccountProjection>,
    pub appeal: Option<AppealProjection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReportProjection {
    pub id: i64,
    pub action_taken_at: Option<NaiveDateTime>,
    pub category: i32,
    pub comment: String,
    pub forwarded: Option<bool>,
    pub created_at: NaiveDateTime,
    pub status_ids: Vec<i64>,
    pub rule_ids: Option<Vec<i64>>,
    pub collection_ids: Vec<i64>,
    pub target_account: AccountProjection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusApplicationProjection {
    pub name: String,
    pub website: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuoteTargetAccess {
    Visible,
    Deleted,
    Unauthorized,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuoteProjection {
    pub state: String,
    pub accepted: bool,
    pub quoted_status_id: Option<i64>,
    pub target_access: QuoteTargetAccess,
    pub target_serializable: bool,
    pub target_link: Option<QuoteTargetLinkProjection>,
    pub quoted_status: Option<Box<StatusProjection>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuoteTargetLinkProjection {
    pub id: i64,
    pub account: AccountProjection,
    pub local: bool,
    pub stored_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusViewerProjection {
    pub viewer_account_id: i64,
    pub favourited: bool,
    pub reblogged: bool,
    pub muted: bool,
    pub bookmarked: bool,
    pub pinned: Option<bool>,
    pub filtered: Vec<FilterResultProjection>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusProjection {
    pub id: i64,
    pub account: AccountProjection,
    pub text: String,
    pub spoiler_text: String,
    pub visibility: i32,
    pub local: bool,
    pub stored_uri: Option<String>,
    pub stored_url: Option<String>,
    pub language: Option<String>,
    pub sensitive: bool,
    pub in_reply_to_id: Option<i64>,
    pub in_reply_to_account_id: Option<i64>,
    pub replies_count: i64,
    pub reblogs_count: i64,
    pub favourites_count: i64,
    pub quotes_count: i64,
    pub edited_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
    pub viewer: Option<StatusViewerProjection>,
    pub reblog: Option<Box<StatusProjection>>,
    pub show_application: bool,
    pub application: Option<StatusApplicationProjection>,
    pub media_attachments: Vec<MediaAttachmentProjection>,
    pub mentions: Vec<MentionProjection>,
    pub tags: Vec<TagProjection>,
    pub emojis: Vec<CustomEmojiProjection>,
    pub tagged_collections: Vec<CollectionProjection>,
    pub quote: Option<QuoteProjection>,
    pub card: Option<PreviewCardProjection>,
    pub poll: Option<PollProjection>,
    pub quote_automatic: Vec<String>,
    pub quote_manual: Vec<String>,
    pub quote_current_user: String,
}

impl StatusProjection {
    pub(crate) fn without_status_relationships(mut self) -> Self {
        self.clear_status_relationships();
        self
    }

    fn clear_status_relationships(&mut self) {
        if let Some(viewer) = self.viewer.as_mut() {
            viewer.favourited = false;
            viewer.reblogged = false;
            viewer.muted = false;
            viewer.bookmarked = false;
            viewer.pinned = viewer.pinned.map(|_| false);
            viewer.filtered.clear();
        }
        if let Some(quote) = self.quote.as_mut()
            && quote.accepted
            && quote.target_access == QuoteTargetAccess::Visible
        {
            "accepted".clone_into(&mut quote.state);
        }
        if let Some(reblog) = self.reblog.as_mut() {
            reblog.clear_status_relationships();
        }
        if let Some(quote) = self.quote.as_mut()
            && let Some(quoted_status) = quote.quoted_status.as_mut()
        {
            quoted_status.clear_status_relationships();
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusEditProjection {
    pub account: AccountProjection,
    pub text: String,
    pub spoiler_text: String,
    pub sensitive: Option<bool>,
    pub created_at: NaiveDateTime,
    pub media_attachments: Vec<MediaAttachmentProjection>,
    pub emojis: Vec<CustomEmojiProjection>,
    pub quote: Option<QuoteProjection>,
    pub poll_options: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::{
        GROUPABLE_NOTIFICATION_TYPES, KNOWN_NOTIFICATION_TYPES, grouped_notification_types,
        notification_type_filter, notification_type_filter_with_exclusions,
    };

    #[test]
    fn notification_type_filter_matches_mastodons_known_type_intersection() {
        assert_eq!(notification_type_filter(&[]), None);
        assert_eq!(
            notification_type_filter(&["future_event".to_owned()]),
            Some(Vec::new())
        );
        assert_eq!(
            notification_type_filter(&["mention".to_owned(), "future_event".to_owned()]),
            Some(vec!["mention".to_owned()])
        );
        assert_eq!(
            notification_type_filter(
                &KNOWN_NOTIFICATION_TYPES
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            ),
            None
        );
    }

    #[test]
    fn notification_type_exclusions_preserve_legacy_unfiltered_rows() {
        assert_eq!(
            notification_type_filter_with_exclusions(&[], &["future_event".to_owned()]),
            None
        );
        let filtered = notification_type_filter_with_exclusions(&[], &["favourite".to_owned()])
            .expect("known exclusion should create an allow-list");
        assert_eq!(filtered.len(), KNOWN_NOTIFICATION_TYPES.len() - 1);
        assert!(!filtered.iter().any(|kind| kind == "favourite"));
    }

    #[test]
    fn grouped_notification_types_intersect_only_groupable_types() {
        assert_eq!(
            grouped_notification_types(&[]),
            GROUPABLE_NOTIFICATION_TYPES
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            grouped_notification_types(&[
                "follow".to_owned(),
                "mention".to_owned(),
                "future_event".to_owned(),
            ]),
            vec!["follow".to_owned()]
        );
    }
}
