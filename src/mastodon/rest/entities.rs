use std::fmt;

use chrono::{Datelike, NaiveDateTime};
use serde::Serialize;
use serde::ser::Serializer;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecimalId(i64);

impl DecimalId {
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }
}

impl fmt::Display for DecimalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for DecimalId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiDateTime(NaiveDateTime);

impl ApiDateTime {
    #[must_use]
    pub const fn new(value: NaiveDateTime) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> NaiveDateTime {
        self.0
    }
}

impl Serialize for ApiDateTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!(
            "{}.{:03}Z",
            self.0.format("%Y-%m-%dT%H:%M:%S"),
            self.0.and_utc().timestamp_subsec_millis()
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiSecondDateTime(NaiveDateTime);

impl ApiSecondDateTime {
    #[must_use]
    pub const fn new(value: NaiveDateTime) -> Self {
        Self(value)
    }
}

impl Serialize for ApiSecondDateTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{}Z", self.0.format("%Y-%m-%dT%H:%M:%S")))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiDate(NaiveDateTime);

impl ApiDate {
    #[must_use]
    pub const fn new(value: NaiveDateTime) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RestStatusContext {
    pub ancestors: Vec<RestStatus>,
    pub descendants: Vec<RestStatus>,
}

impl Serialize for ApiDate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!(
            "{:04}-{:02}-{:02}",
            self.0.year(),
            self.0.month(),
            self.0.day()
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestInstanceV1 {
    pub uri: String,
    pub title: String,
    pub short_description: String,
    pub description: String,
    pub email: String,
    pub version: String,
    pub urls: Value,
    pub stats: Value,
    pub thumbnail: Option<String>,
    pub languages: Vec<String>,
    pub registrations: bool,
    pub approval_required: bool,
    pub invites_enabled: bool,
    pub configuration: Value,
    pub contact_account: Option<RestAccount>,
    pub rules: Vec<RestRule>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestInstanceV2 {
    pub domain: String,
    pub title: String,
    pub version: String,
    pub source_url: String,
    pub description: String,
    pub usage: Value,
    pub thumbnail: Value,
    pub icon: Vec<Value>,
    pub languages: Vec<String>,
    pub configuration: Value,
    pub registrations: Value,
    pub api_versions: Value,
    pub wrapstodon: Option<i32>,
    pub contact: Value,
    pub rules: Vec<RestRule>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestRule {
    pub id: DecimalId,
    pub text: String,
    pub hint: String,
    pub translations: Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestAccountRole {
    pub id: DecimalId,
    pub name: String,
    pub color: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestRole {
    pub id: DecimalId,
    pub name: String,
    pub permissions: String,
    pub color: String,
    pub highlighted: bool,
    pub collection_limit: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestAccountField {
    pub name: String,
    pub value: String,
    pub verified_at: Option<ApiDateTime>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestFeatureApproval {
    pub automatic: Vec<String>,
    pub manual: Vec<String>,
    pub current_user: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct RestAccount {
    pub id: DecimalId,
    pub username: String,
    pub acct: String,
    pub display_name: String,
    pub locked: bool,
    pub bot: bool,
    pub discoverable: Option<bool>,
    pub indexable: bool,
    pub group: bool,
    pub created_at: ApiDateTime,
    pub note: String,
    pub url: String,
    pub uri: String,
    pub avatar: String,
    pub avatar_static: String,
    pub avatar_description: String,
    pub header: String,
    pub header_static: String,
    pub header_description: String,
    pub followers_count: i64,
    pub following_count: i64,
    pub statuses_count: i64,
    pub last_status_at: Option<ApiDate>,
    pub hide_collections: Option<bool>,
    pub show_media: bool,
    pub show_media_replies: bool,
    pub show_featured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moved: Option<Box<RestAccount>>,
    pub emojis: Vec<RestCustomEmoji>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspended: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noindex: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memorial: Option<bool>,
    pub feature_approval: RestFeatureApproval,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_subscriptions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<RestAccountRole>>,
    pub fields: Vec<RestAccountField>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestMutedAccount {
    #[serde(flatten)]
    pub account: RestAccount,
    pub mute_expires_at: Option<ApiSecondDateTime>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestCredentialSource {
    pub privacy: String,
    pub sensitive: bool,
    pub language: Option<String>,
    pub note: String,
    pub fields: Vec<RestAccountField>,
    pub follow_requests_count: i64,
    pub hide_collections: Option<bool>,
    pub discoverable: Option<bool>,
    pub indexable: bool,
    pub attribution_domains: Option<Vec<String>>,
    pub quote_policy: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestCredentialAccount {
    #[serde(flatten)]
    pub account: RestAccount,
    pub source: RestCredentialSource,
    pub role: RestRole,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct RestRelationship {
    pub id: DecimalId,
    pub following: bool,
    pub showing_reblogs: bool,
    pub notifying: bool,
    pub languages: Option<Vec<String>>,
    pub followed_by: bool,
    pub blocking: bool,
    pub blocked_by: bool,
    pub muting: bool,
    pub muting_notifications: bool,
    pub muting_expires_at: Option<ApiSecondDateTime>,
    pub requested: bool,
    pub requested_by: bool,
    pub domain_blocking: bool,
    pub endorsed: bool,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestApplication {
    pub name: String,
    pub website: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestMediaAttachment {
    pub id: DecimalId,
    #[serde(rename = "type")]
    pub media_type: String,
    pub url: Option<String>,
    pub preview_url: Option<String>,
    pub remote_url: Option<String>,
    pub preview_remote_url: Option<String>,
    pub text_url: Option<String>,
    pub meta: Option<Value>,
    pub description: Option<String>,
    pub blurhash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestMention {
    pub id: DecimalId,
    pub username: String,
    pub url: String,
    pub acct: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestShallowTag {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestTagHistory {
    pub day: String,
    pub accounts: String,
    pub uses: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestTag {
    pub id: DecimalId,
    pub name: String,
    pub url: String,
    pub history: Vec<RestTagHistory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub following: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub featuring: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestCustomEmoji {
    pub shortcode: String,
    pub url: String,
    pub static_url: String,
    pub visible_in_picker: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub featured: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestPollOption {
    pub title: String,
    pub votes_count: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestPoll {
    pub id: DecimalId,
    pub expires_at: Option<ApiDateTime>,
    pub expired: bool,
    pub multiple: bool,
    pub votes_count: i64,
    pub voters_count: Option<i64>,
    pub options: Vec<RestPollOption>,
    pub emojis: Vec<RestCustomEmoji>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub own_votes: Option<Vec<i32>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestPreviewCardAuthor {
    pub name: String,
    pub url: String,
    pub account: Option<RestAccount>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestPreviewCard {
    pub url: String,
    pub title: String,
    pub description: String,
    pub language: Option<String>,
    #[serde(rename = "type")]
    pub card_type: String,
    pub author_name: String,
    pub author_url: String,
    pub provider_name: String,
    pub provider_url: String,
    pub html: String,
    pub width: i32,
    pub height: i32,
    pub image: Option<String>,
    pub image_description: String,
    pub embed_url: String,
    pub blurhash: Option<String>,
    pub published_at: Option<ApiDateTime>,
    pub authors: Vec<RestPreviewCardAuthor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing_attribution: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestQuoteApproval {
    pub automatic: Vec<String>,
    pub manual: Vec<String>,
    pub current_user: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestQuote {
    pub state: String,
    pub quoted_status: Option<Box<RestStatus>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestShallowQuote {
    pub state: String,
    pub quoted_status_id: Option<DecimalId>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RestQuotePayload {
    Full(RestQuote),
    Shallow(RestShallowQuote),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestFilterKeyword {
    pub id: DecimalId,
    pub keyword: String,
    pub whole_word: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestFilterStatus {
    pub id: DecimalId,
    pub status_id: DecimalId,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestFilter {
    pub id: DecimalId,
    pub title: String,
    pub context: Vec<String>,
    pub expires_at: Option<ApiDateTime>,
    pub filter_action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<RestFilterKeyword>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statuses: Option<Vec<RestFilterStatus>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestFilterResult {
    pub filter: RestFilter,
    pub keyword_matches: Option<Vec<String>>,
    pub status_matches: Option<Vec<DecimalId>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestList {
    pub id: DecimalId,
    pub title: String,
    pub replies_policy: String,
    pub exclusive: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestFeaturedTag {
    pub id: DecimalId,
    pub name: String,
    pub url: String,
    pub statuses_count: String,
    pub last_status_at: Option<ApiDate>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestCollectionItem {
    pub id: DecimalId,
    pub state: String,
    pub created_at: ApiDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<DecimalId>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestCollection {
    pub id: DecimalId,
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub account_id: DecimalId,
    pub local: bool,
    pub sensitive: bool,
    pub discoverable: bool,
    pub url: Option<String>,
    pub item_count: usize,
    pub created_at: ApiDateTime,
    pub updated_at: ApiDateTime,
    pub tag: Option<RestShallowTag>,
    pub items: Vec<RestCollectionItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestCollectionWithAccounts {
    pub collection: RestCollection,
    pub accounts: Vec<RestAccount>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestStatusSource {
    pub id: DecimalId,
    pub text: String,
    pub spoiler_text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestStatusEditPollOption {
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestStatusEditPoll {
    pub options: Vec<RestStatusEditPollOption>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestStatusEdit {
    pub account: RestAccount,
    pub content: String,
    pub spoiler_text: String,
    pub sensitive: Option<bool>,
    pub created_at: ApiDateTime,
    pub media_attachments: Vec<RestMediaAttachment>,
    pub emojis: Vec<RestCustomEmoji>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<RestQuotePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll: Option<RestStatusEditPoll>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestStatus {
    pub id: DecimalId,
    pub created_at: ApiDateTime,
    pub in_reply_to_id: Option<DecimalId>,
    pub in_reply_to_account_id: Option<DecimalId>,
    pub sensitive: bool,
    pub spoiler_text: String,
    pub visibility: String,
    pub language: Option<String>,
    pub uri: String,
    pub url: Option<String>,
    pub replies_count: i64,
    pub reblogs_count: i64,
    pub favourites_count: i64,
    pub quotes_count: i64,
    pub edited_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favourited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reblogged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filtered: Option<Vec<RestFilterResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub reblog: Option<Box<RestStatus>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<Option<RestApplication>>,
    pub account: RestAccount,
    pub media_attachments: Vec<RestMediaAttachment>,
    pub mentions: Vec<RestMention>,
    pub tags: Vec<RestShallowTag>,
    pub emojis: Vec<RestCustomEmoji>,
    pub tagged_collections: Vec<RestCollection>,
    pub quote: Option<RestQuotePayload>,
    pub card: Option<RestPreviewCard>,
    pub poll: Option<RestPoll>,
    pub quote_approval: RestQuoteApproval,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestFallback {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestSeveranceEvent {
    pub id: DecimalId,
    #[serde(rename = "type")]
    pub event_type: String,
    pub purged: bool,
    pub target_name: String,
    pub followers_count: i32,
    pub following_count: i32,
    pub created_at: ApiDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestAppeal {
    pub text: String,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestAccountWarning {
    pub id: DecimalId,
    pub action: String,
    pub text: String,
    pub status_ids: Option<Vec<DecimalId>>,
    pub created_at: ApiDateTime,
    pub target_account: Option<RestAccount>,
    pub appeal: Option<RestAppeal>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestReport {
    pub id: DecimalId,
    pub action_taken: bool,
    pub action_taken_at: Option<ApiDateTime>,
    pub category: String,
    pub comment: String,
    pub forwarded: Option<bool>,
    pub created_at: ApiDateTime,
    pub status_ids: Vec<DecimalId>,
    pub rule_ids: Option<Vec<DecimalId>>,
    pub collection_ids: Vec<DecimalId>,
    pub target_account: RestAccount,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestAnnualReport {
    pub year: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestNotification {
    pub id: DecimalId,
    #[serde(rename = "type")]
    pub notification_type: String,
    pub created_at: ApiDateTime,
    pub group_key: String,
    pub account: RestAccount,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filtered: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<RestFallback>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Option<Box<RestStatus>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<RestReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<RestSeveranceEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moderation_warning: Option<RestAccountWarning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<Option<RestCollection>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestNotificationGroup {
    pub group_key: String,
    pub notifications_count: i64,
    #[serde(rename = "type")]
    pub notification_type: String,
    pub most_recent_notification_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_min_id: Option<DecimalId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_max_id: Option<DecimalId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_page_notification_at: Option<ApiDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<RestFallback>,
    pub sample_account_ids: Vec<DecimalId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_id: Option<Option<DecimalId>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<RestReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<RestSeveranceEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moderation_warning: Option<RestAccountWarning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annual_report: Option<RestAnnualReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<Option<RestCollection>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestPartialAccount {
    pub id: DecimalId,
    pub acct: String,
    pub locked: bool,
    pub bot: bool,
    pub url: String,
    pub avatar: String,
    pub avatar_static: String,
    pub avatar_description: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestGroupedNotifications {
    pub accounts: Vec<RestAccount>,
    pub statuses: Vec<RestStatus>,
    pub notification_groups: Vec<RestNotificationGroup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_accounts: Option<Vec<RestPartialAccount>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RestMarker {
    pub last_read_id: DecimalId,
    pub version: i32,
    pub updated_at: ApiDateTime,
}
