use chrono::NaiveDateTime;
use ipnetwork::IpNetwork;
use serde_json::Value;

use super::settings::{RawJsonText, RawYamlText};
use super::types::{
    AccountIdScheme, AccountKind, NotificationType, PermissionBits, RawI32, RawString, SecretText,
    StatusVisibility,
};

#[derive(Clone, PartialEq, sqlx::FromRow)]
#[allow(clippy::struct_excessive_bools)]
pub struct Account {
    pub id: i64,
    pub username: String,
    pub domain: Option<String>,
    pub actor_type: Option<RawString>,
    pub display_name: String,
    pub note: String,
    pub uri: String,
    pub url: Option<String>,
    pub also_known_as: Option<Vec<String>>,
    pub attribution_domains: Option<Vec<String>>,
    pub fields: Option<Value>,
    pub avatar_content_type: Option<String>,
    pub avatar_description: String,
    pub avatar_file_name: Option<String>,
    pub avatar_file_size: Option<i32>,
    pub avatar_remote_url: Option<String>,
    pub avatar_storage_schema_version: Option<i32>,
    pub avatar_updated_at: Option<NaiveDateTime>,
    pub collections_url: Option<String>,
    pub discoverable: Option<bool>,
    pub feature_approval_policy: RawI32,
    pub featured_collection_url: Option<String>,
    pub followers_url: String,
    pub following_url: String,
    pub header_content_type: Option<String>,
    pub header_description: String,
    pub header_file_name: Option<String>,
    pub header_file_size: Option<i32>,
    pub header_remote_url: String,
    pub header_storage_schema_version: Option<i32>,
    pub header_updated_at: Option<NaiveDateTime>,
    pub hide_collections: Option<bool>,
    pub id_scheme: Option<AccountIdScheme>,
    pub inbox_url: String,
    pub indexable: bool,
    pub locked: bool,
    pub memorial: bool,
    pub moved_to_account_id: Option<i64>,
    pub outbox_url: String,
    pub protocol: RawI32,
    pub public_key: String,
    pub private_key: Option<SecretText>,
    pub sensitized_at: Option<NaiveDateTime>,
    pub shared_inbox_url: String,
    pub show_featured: bool,
    pub show_media: bool,
    pub show_media_replies: bool,
    pub silenced_at: Option<NaiveDateTime>,
    pub suspended_at: Option<NaiveDateTime>,
    pub suspension_origin: Option<RawI32>,
    pub trendable: Option<bool>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub has_user: bool,
    pub login_capable_user: bool,
    pub has_pending_user: bool,
    pub has_unconfirmed_user: bool,
}

impl Account {
    #[must_use]
    pub fn kind(&self) -> AccountKind {
        AccountKind::classify(
            self.domain.as_deref(),
            self.has_user,
            self.login_capable_user,
        )
    }
}

impl DomainBlock {
    #[must_use]
    pub fn policy_rule(&self) -> super::policy::GlobalDomainRule {
        super::policy::GlobalDomainRule {
            domain: self.domain.clone(),
            severity: super::policy::DomainSeverity::from(self.severity.map(|value| value.0)),
            reject_media: self.reject_media,
            reject_reports: self.reject_reports,
        }
    }
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct AccountStat {
    pub id: i64,
    pub account_id: i64,
    pub statuses_count: i64,
    pub following_count: i64,
    pub followers_count: i64,
    pub last_status_at: Option<NaiveDateTime>,
}

#[derive(Clone, PartialEq, sqlx::FromRow)]
#[allow(clippy::struct_excessive_bools)]
pub struct User {
    pub id: i64,
    pub account_id: i64,
    pub email: String,
    pub encrypted_password: SecretText,
    pub chosen_languages: Option<Vec<String>>,
    pub otp_backup_codes: Option<Vec<SecretText>>,
    pub otp_required_for_login: bool,
    pub otp_secret: Option<SecretText>,
    pub settings: Option<RawJsonText>,
    pub sign_up_ip: Option<IpNetwork>,
    pub role_id: Option<i64>,
    pub approved: bool,
    pub disabled: bool,
    pub confirmed_at: Option<NaiveDateTime>,
    pub locale: Option<String>,
    pub webauthn_id: Option<String>,
    pub has_webauthn_credentials: bool,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct UserRole {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub position: i32,
    pub permissions: PermissionBits,
    pub highlighted: bool,
    pub require_2fa: bool,
    pub collection_limit: i32,
}

#[derive(Clone, PartialEq, sqlx::FromRow)]
pub struct OAuthApplication {
    pub id: i64,
    pub name: String,
    pub uid: String,
    pub secret: SecretText,
    pub redirect_uri: String,
    pub scopes: String,
    pub confidential: bool,
    pub owner_id: Option<i64>,
    pub owner_type: Option<RawString>,
    pub website: Option<String>,
}

#[derive(Clone, PartialEq, sqlx::FromRow)]
pub struct OAuthAccessToken {
    pub id: i64,
    pub resource_owner_id: Option<i64>,
    pub application_id: Option<i64>,
    pub token: SecretText,
    pub refresh_token: Option<SecretText>,
    pub scopes: Option<String>,
    pub expires_in: Option<i32>,
    pub created_at: NaiveDateTime,
    pub revoked_at: Option<NaiveDateTime>,
    pub last_used_at: Option<NaiveDateTime>,
    pub last_used_ip: Option<IpNetwork>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct OAuthBearerCandidate {
    pub token_id: i64,
    pub resource_owner_id: Option<i64>,
    pub application_id: Option<i64>,
    pub scopes: Option<String>,
    pub expires_in: Option<i32>,
    pub created_at: NaiveDateTime,
    pub revoked_at: Option<NaiveDateTime>,
    pub application_exists: bool,
    pub user_id: Option<i64>,
    pub user_account_id: Option<i64>,
    pub confirmed_at: Option<NaiveDateTime>,
    pub approved: Option<bool>,
    pub disabled: Option<bool>,
    pub otp_required_for_login: Option<bool>,
    pub role_requires_2fa: Option<bool>,
    pub has_webauthn_credentials: bool,
    pub account_id: Option<i64>,
    pub suspended_at: Option<NaiveDateTime>,
    pub has_deletion_request: bool,
    pub memorial: Option<bool>,
    pub moved_to_account_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Status {
    pub id: i64,
    pub account_id: i64,
    pub application_id: Option<i64>,
    pub text: String,
    pub spoiler_text: String,
    pub visibility: StatusVisibility,
    pub local: Option<bool>,
    pub uri: Option<String>,
    pub url: Option<String>,
    pub language: Option<String>,
    pub sensitive: bool,
    pub reply: bool,
    pub ordered_media_attachment_ids: Option<Vec<i64>>,
    pub conversation_id: Option<i64>,
    pub in_reply_to_account_id: Option<i64>,
    pub in_reply_to_id: Option<i64>,
    pub reblog_of_id: Option<i64>,
    pub poll_id: Option<i64>,
    pub quote_approval_policy: RawI32,
    pub deleted_at: Option<NaiveDateTime>,
    pub edited_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct StatusStat {
    pub id: i64,
    pub status_id: i64,
    pub replies_count: i64,
    pub reblogs_count: i64,
    pub favourites_count: i64,
    pub quotes_count: i64,
    pub untrusted_reblogs_count: Option<i64>,
    pub untrusted_favourites_count: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct StatusEdit {
    pub id: i64,
    pub status_id: i64,
    pub account_id: Option<i64>,
    pub text: String,
    pub spoiler_text: String,
    pub sensitive: Option<bool>,
    pub ordered_media_attachment_ids: Option<Vec<i64>>,
    pub media_descriptions: Option<Vec<Option<String>>>,
    pub poll_options: Option<Vec<String>>,
    pub quote_id: Option<i64>,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct MediaAttachment {
    pub id: i64,
    pub account_id: Option<i64>,
    pub status_id: Option<i64>,
    pub media_type: RawI32,
    pub processing: Option<RawI32>,
    pub description: Option<String>,
    pub remote_url: String,
    pub file_content_type: Option<String>,
    pub file_file_name: Option<String>,
    pub file_file_size: Option<i32>,
    pub file_meta: Option<Value>,
    pub file_storage_schema_version: Option<i32>,
    pub file_updated_at: Option<NaiveDateTime>,
    pub scheduled_status_id: Option<i64>,
    pub shortcode: Option<String>,
    pub thumbnail_content_type: Option<String>,
    pub thumbnail_file_name: Option<String>,
    pub thumbnail_file_size: Option<i32>,
    pub thumbnail_remote_url: Option<String>,
    pub thumbnail_storage_schema_version: Option<i32>,
    pub thumbnail_updated_at: Option<NaiveDateTime>,
    pub blurhash: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Mention {
    pub id: i64,
    pub account_id: i64,
    pub status_id: i64,
    pub silent: bool,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub display_name: Option<String>,
    pub usable: Option<bool>,
    pub trendable: Option<bool>,
    pub listable: Option<bool>,
    pub last_status_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct StatusTag {
    pub status_id: i64,
    pub tag_id: i64,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct AccountTag {
    pub account_id: i64,
    pub tag_id: i64,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct FeaturedTag {
    pub id: i64,
    pub account_id: i64,
    pub tag_id: i64,
    pub name: Option<String>,
    pub statuses_count: i64,
    pub last_status_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Conversation {
    pub id: i64,
    pub uri: Option<String>,
    pub parent_account_id: Option<i64>,
    pub parent_status_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct AccountConversation {
    pub id: i64,
    pub account_id: i64,
    pub conversation_id: i64,
    pub last_status_id: Option<i64>,
    pub participant_account_ids: Vec<i64>,
    pub status_ids: Vec<i64>,
    pub unread: bool,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct ConversationMute {
    pub id: i64,
    pub account_id: i64,
    pub conversation_id: i64,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Follow {
    pub id: i64,
    pub account_id: i64,
    pub target_account_id: i64,
    pub show_reblogs: bool,
    pub notify: bool,
    pub languages: Option<Vec<String>>,
    pub uri: Option<String>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct FollowRequest {
    pub id: i64,
    pub account_id: i64,
    pub target_account_id: i64,
    pub show_reblogs: bool,
    pub notify: bool,
    pub languages: Option<Vec<String>>,
    pub uri: Option<String>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Favourite {
    pub id: i64,
    pub account_id: i64,
    pub status_id: i64,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Bookmark {
    pub id: i64,
    pub account_id: i64,
    pub status_id: i64,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct StatusPin {
    pub id: i64,
    pub account_id: i64,
    pub status_id: i64,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Block {
    pub id: i64,
    pub account_id: i64,
    pub target_account_id: i64,
    pub uri: Option<String>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Mute {
    pub id: i64,
    pub account_id: i64,
    pub target_account_id: i64,
    pub hide_notifications: bool,
    pub expires_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct AccountDomainBlock {
    pub id: i64,
    pub account_id: i64,
    pub domain: String,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct List {
    pub id: i64,
    pub account_id: i64,
    pub title: String,
    pub replies_policy: RawI32,
    pub exclusive: bool,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct ListAccount {
    pub id: i64,
    pub list_id: i64,
    pub account_id: i64,
    pub follow_id: Option<i64>,
    pub follow_request_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct CustomFilter {
    pub id: i64,
    pub account_id: i64,
    pub phrase: String,
    pub context: Vec<String>,
    pub action: RawI32,
    pub expires_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct CustomFilterKeyword {
    pub id: i64,
    pub custom_filter_id: i64,
    pub keyword: String,
    pub whole_word: bool,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct CustomFilterStatus {
    pub id: i64,
    pub custom_filter_id: i64,
    pub status_id: i64,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Marker {
    pub timeline: String,
    pub last_read_id: i64,
    pub lock_version: i32,
    pub updated_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Notification {
    pub id: i64,
    pub account_id: i64,
    pub activity_id: i64,
    pub activity_type: RawString,
    pub from_account_id: i64,
    pub notification_type: Option<NotificationType>,
    pub group_key: Option<String>,
    pub filtered: bool,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct RelationshipSeveranceEvent {
    pub id: i64,
    pub event_type: RawI32,
    pub target_name: String,
    pub purged: bool,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct AccountRelationshipSeveranceEvent {
    pub id: i64,
    pub account_id: i64,
    pub relationship_severance_event_id: i64,
    pub followers_count: i32,
    pub following_count: i32,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct AccountWarning {
    pub id: i64,
    pub account_id: Option<i64>,
    pub target_account_id: Option<i64>,
    pub report_id: Option<i64>,
    pub action: RawI32,
    pub text: String,
    pub status_ids: Option<Vec<String>>,
    pub overruled_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct GeneratedAnnualReport {
    pub id: i64,
    pub account_id: i64,
    pub year: i32,
    pub schema_version: i32,
    pub data: Value,
    pub share_key: Option<SecretText>,
    pub viewed_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Report {
    pub id: i64,
    pub account_id: i64,
    pub target_account_id: i64,
    pub action_taken_at: Option<NaiveDateTime>,
    pub action_taken_by_account_id: Option<i64>,
    pub application_id: Option<i64>,
    pub assigned_account_id: Option<i64>,
    pub category: RawI32,
    pub comment: String,
    pub forwarded: Option<bool>,
    pub rule_ids: Option<Vec<i64>>,
    pub status_ids: Vec<i64>,
    pub uri: Option<String>,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct NotificationPolicy {
    pub id: i64,
    pub account_id: i64,
    pub for_bots: RawI32,
    pub for_limited_accounts: RawI32,
    pub for_new_accounts: RawI32,
    pub for_not_followers: RawI32,
    pub for_not_following: RawI32,
    pub for_private_mentions: RawI32,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct NotificationPermission {
    pub id: i64,
    pub account_id: i64,
    pub from_account_id: i64,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct NotificationRequest {
    pub id: i64,
    pub account_id: i64,
    pub from_account_id: i64,
    pub last_status_id: Option<i64>,
    pub notifications_count: i64,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct DomainAllow {
    pub id: i64,
    pub domain: String,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct DomainBlock {
    pub id: i64,
    pub domain: String,
    pub severity: Option<RawI32>,
    pub reject_media: bool,
    pub reject_reports: bool,
    pub private_comment: Option<String>,
    pub public_comment: Option<String>,
    pub obfuscate: bool,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Setting {
    pub id: i64,
    pub var: String,
    pub value: Option<RawYamlText>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Quote {
    pub id: i64,
    pub account_id: i64,
    pub status_id: i64,
    pub quoted_account_id: Option<i64>,
    pub quoted_status_id: Option<i64>,
    pub state: RawI32,
    pub activity_uri: Option<String>,
    pub approval_uri: Option<String>,
    pub legacy: bool,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Collection {
    pub id: i64,
    pub account_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub description_html: Option<String>,
    pub local: bool,
    pub sensitive: bool,
    pub discoverable: bool,
    pub item_count: i32,
    pub original_number_of_items: Option<i32>,
    pub language: Option<String>,
    pub uri: Option<String>,
    pub url: Option<String>,
    pub tag_id: Option<i64>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct CollectionItem {
    pub id: i64,
    pub collection_id: i64,
    pub account_id: Option<i64>,
    pub position: i32,
    pub state: RawI32,
    pub activity_uri: Option<String>,
    pub approval_uri: Option<String>,
    pub object_uri: Option<String>,
    pub uri: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Poll {
    pub id: i64,
    pub account_id: i64,
    pub status_id: i64,
    pub options: Vec<String>,
    pub cached_tallies: Vec<i64>,
    pub votes_count: i64,
    pub voters_count: Option<i64>,
    pub multiple: bool,
    pub hide_totals: bool,
    pub expires_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct PollVote {
    pub id: i64,
    pub account_id: i64,
    pub poll_id: i64,
    pub choice: i32,
    pub uri: Option<String>,
}

#[derive(Clone, PartialEq, sqlx::FromRow)]
pub struct Keypair {
    pub id: i64,
    pub account_id: i64,
    pub key_type: RawI32,
    pub uri: String,
    pub public_key: String,
    pub private_key: Option<SecretText>,
    pub revoked: bool,
    pub expires_at: Option<NaiveDateTime>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Tombstone {
    pub id: i64,
    pub account_id: i64,
    pub uri: String,
    pub by_moderator: Option<bool>,
    pub created_at: NaiveDateTime,
}
