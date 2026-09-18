pub mod local_uploads;

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::time::Duration;
#[cfg(feature = "test-support")]
use std::{
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bcrypt::{DEFAULT_COST, hash};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, Utc};
use futures_util::future::try_join_all;
use hmac::{Hmac, Mac};
use ipnetwork::IpNetwork;
use pbkdf2::pbkdf2_hmac;
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::pkcs8::EncodePublicKey;
use rsa::rand_core::{OsRng, RngCore};
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde_json::{Map, Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool, PgPoolOptions};
use sqlx::{Connection, Postgres, Transaction};
use unicode_segmentation::UnicodeSegmentation;
use url::Url;

use super::auth::{
    TwoFactorVerification, random_backup_code, valid_totp_secret, verify_password,
    verify_two_factor,
};
use super::oauth::{
    AuthenticatedBearer, RequiredScopes, WRITE_ACCOUNTS, WRITE_BLOCKS, WRITE_BOOKMARKS,
    WRITE_CONVERSATIONS, WRITE_FOLLOWS, WRITE_MEDIA, WRITE_MUTES, WRITE_NOTIFICATIONS,
    WRITE_REPORTS, WRITE_STATUSES,
};
use super::policy::{
    AuthenticatedViewerFacts, AuthorRestriction, QuotePolicy, QuotePolicyFacts,
    QuotePolicyViewerFacts, StatusAccessFacts, StatusAvailability, ViewerFacts,
    direct_quote_allowed, global_domain_policy, quote_post_has_content, quote_post_visibility,
    quote_target_visibility_allowed, status_access, status_favourite_access, status_quote_policy,
    status_reblog_access,
};
use super::records::{BrowserLoginUser, DomainBlock, Marker, MediaAttachment, NotificationPolicy};
use super::repository::normalize_hashtag;
use super::rest::{HtmlFormatter, RenderedHtml};
use super::types::{AccountIdScheme, SecretText, StatusVisibility};
use crate::crypto::ActiveRecordEncryptionConfig;
use crate::jobs::{
    ACCOUNT_DELETION_DELAY_DAYS, ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND,
    ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND, ACTIVITYPUB_DELIVERY_JOB_KIND,
    ACTIVITYPUB_EMOJI_FETCH_JOB_KIND, ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
    ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND, ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND, JobError,
    JobSpec, LOCAL_MEDIA_CLEANUP_JOB_KIND, Lane, MASTODON_ACCOUNT_PURGE_JOB_KIND,
    MASTODON_DOMAIN_BLOCK_JOB_KIND, MASTODON_DOMAIN_PURGE_JOB_KIND, NOTIFICATION_CLEANUP_JOB_KIND,
    NOTIFICATION_CREATE_JOB_KIND, NOTIFICATION_UNFILTER_JOB_KIND, PendingStreamEvent,
    PollExpirationEffectOutcome, PollExpirationIntentKind, flush_staged_stream_events_in,
    flush_stream_events_in, pending_stream_event, poll_expiration_activation_in,
    poll_expiration_effect_in, poll_expiration_generation, poll_expiration_is_historical,
    poll_expiration_job, record_outbox_in, record_outbox_once_in, record_poll_expiration_effect_in,
    record_stream_event_in, stage_stream_events_if_large_in,
};
use crate::mail::report_job;
use crate::media::media_format;
use crate::paperclip::{PaperclipAttachment, PaperclipMetadata, rails_blank};
use crate::remote::{RemoteActor, canonical_remote_domain, canonical_remote_host};
use crate::streaming::{
    STATUS_UPDATE_NOTIFICATION_EVENT, SYSTEM_KILL_EVENT, TOKEN_KILL_EVENT, TimelineListRoute,
    TimelineRouteSnapshot, event_logical_key, global_event_logical_key, media_event_logical_key,
};

use super::activitypub;
use super::activitypub_inbox::parse_note_emojis;
use super::equals_or_includes;
const MODERATION_PERMISSION_MASK: i64 = (1_i64 << 2)
    | (1_i64 << 3)
    | (1_i64 << 4)
    | (1_i64 << 5)
    | (1_i64 << 7)
    | (1_i64 << 8)
    | (1_i64 << 9)
    | (1_i64 << 10)
    | (1_i64 << 11)
    | (1_i64 << 18)
    | (1_i64 << 19)
    | (1_i64 << 20);
// Keep group continuity outside public notification rows so dismissal does not reset it.
const NOTIFICATION_GROUP_MARKER_KIND: &str = "notification_group";
const MUTE_EXPIRY_JOB_KIND: &str = "rustodon.mastodon.delete_mute";
const RELATIONSHIP_TOMBSTONE_SCOPE: &str = "rustodon.activitypub.relationship";
const RELATIONSHIP_TOMBSTONE_TTL: &str = "6 hours";
pub const STATUS_NOTIFICATION_JOB_KIND: &str = "rustodon.mastodon.notify_status";
const STATUS_NOTIFICATION_BATCH_SIZE: i64 = 100;
pub const REPORT_RATE_LIMIT: i64 = 400;
const REPORT_RATE_LIMIT_COUNT_SQL: &str = "SELECT count(*) FROM reports WHERE account_id = $1 \
     AND created_at >= date_trunc('day', timezone('UTC', clock_timestamp()))";
const OOB_REDIRECT_URI: &str = "urn:ietf:wg:oauth:2.0:oob";
const NOTIFICATION_FAVOURITE: &str = "favourite";
const NOTIFICATION_FOLLOW: &str = "follow";
const NOTIFICATION_FOLLOW_REQUEST: &str = "follow_request";
const NOTIFICATION_MENTION: &str = "mention";
const NOTIFICATION_QUOTE: &str = "quote";
const NOTIFICATION_REBLOG: &str = "reblog";
const NOTIFICATION_QUOTED_UPDATE: &str = "quoted_update";
const NOTIFICATION_UPDATE: &str = "update";
const DEFAULT_USER_ACTIVE_DAYS: i32 = 7;
pub const OAUTH_CONFIGURED_SCOPES: &[&str] = &[
    "read",
    "profile",
    "write",
    "write:accounts",
    "write:blocks",
    "write:bookmarks",
    "write:collections",
    "write:conversations",
    "write:favourites",
    "write:filters",
    "write:follows",
    "write:lists",
    "write:media",
    "write:mutes",
    "write:notifications",
    "write:reports",
    "write:statuses",
    "read:accounts",
    "read:blocks",
    "read:bookmarks",
    "read:collections",
    "read:favourites",
    "read:filters",
    "read:follows",
    "read:lists",
    "read:mutes",
    "read:notifications",
    "read:search",
    "read:statuses",
    "follow",
    "push",
    "admin:read",
    "admin:read:accounts",
    "admin:read:reports",
    "admin:read:domain_allows",
    "admin:read:domain_blocks",
    "admin:read:ip_blocks",
    "admin:read:email_domain_blocks",
    "admin:read:canonical_email_blocks",
    "admin:write",
    "admin:write:accounts",
    "admin:write:reports",
    "admin:write:domain_allows",
    "admin:write:domain_blocks",
    "admin:write:ip_blocks",
    "admin:write:email_domain_blocks",
    "admin:write:canonical_email_blocks",
];
const DUMMY_BCRYPT_PASSWORD: &str = "$2a$04$eYtbMaSJeYOgS7ENqTs6vezGAQVltj68iGTiHiIsdst5LyirUp5JC";
const MAX_TWO_FACTOR_ATTEMPTS_PER_HOUR: i64 = 10;
const TWO_FACTOR_BACKUP_CODE_COUNT: usize = 10;
const MEDIA_ATTACHMENT_COLUMNS: &str = "media.id, media.account_id, media.status_id, media.type AS media_type, \
     media.processing, media.description, media.remote_url, media.file_content_type, \
     media.file_file_name, media.file_file_size, media.file_meta, \
     media.file_storage_schema_version, media.file_updated_at, media.scheduled_status_id, \
     media.shortcode, media.thumbnail_content_type, media.thumbnail_file_name, \
     media.thumbnail_file_size, media.thumbnail_remote_url, \
     media.thumbnail_storage_schema_version, media.thumbnail_updated_at, media.blurhash, \
     media.created_at, media.updated_at";

#[derive(Clone, Copy, Debug)]
pub struct IdempotencyKey<'a> {
    pub scope: &'a str,
    pub key: &'a str,
    pub fingerprint: [u8; 32],
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug)]
pub enum WriteOutcome<T> {
    Applied(T),
    Replayed(T),
}

#[derive(Default)]
pub struct WriteOptions<'a> {
    pub idempotency: Option<IdempotencyKey<'a>>,
    pub outbox: Option<&'a JobSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountFieldUpdate {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AccountProfileValue<T> {
    #[default]
    Unchanged,
    Null,
    Value(T),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum AccountMediaUpdate {
    #[default]
    Unchanged,
    Remove,
    Replace {
        file_name: String,
        content_type: String,
        file_size: i32,
        storage_schema_version: i32,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccountSourceUpdate {
    pub privacy: AccountProfileValue<String>,
    pub sensitive: AccountProfileValue<bool>,
    pub language: AccountProfileValue<String>,
    pub quote_policy: AccountProfileValue<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccountProfileUpdate {
    pub display_name: Option<String>,
    pub note: Option<String>,
    pub avatar_description: Option<String>,
    pub header_description: Option<String>,
    pub avatar: AccountMediaUpdate,
    pub header: AccountMediaUpdate,
    pub bot: AccountProfileValue<bool>,
    pub locked: Option<bool>,
    pub discoverable: AccountProfileValue<bool>,
    pub hide_collections: AccountProfileValue<bool>,
    pub indexable: Option<bool>,
    pub attribution_domains: Option<Vec<String>>,
    pub fields: Option<Vec<AccountFieldUpdate>>,
    pub source: Option<AccountSourceUpdate>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MediaFocus {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug)]
pub struct MediaAttachmentCreate {
    pub media_type: i32,
    pub file_name: String,
    pub content_type: String,
    pub file_size: i32,
    pub file_meta: Value,
    pub blurhash: Option<String>,
    pub description: Option<String>,
    pub focus: AccountProfileValue<MediaFocus>,
}

#[derive(Clone, Debug, Default)]
pub struct MediaAttachmentUpdate {
    pub description: AccountProfileValue<String>,
    pub focus: AccountProfileValue<MediaFocus>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationActivity {
    Mention { id: i64 },
    Status { id: i64 },
    Reblog { id: i64 },
    Follow { id: i64 },
    FollowRequest { id: i64 },
    Favourite { id: i64 },
    Poll { id: i64 },
    Update { id: i64 },
    SeveredRelationships { id: i64 },
    ModerationWarning { id: i64 },
    AnnualReport { id: i64 },
    AdminSignUp { id: i64 },
    AdminReport { id: i64 },
    Quote { id: i64 },
    QuotedUpdate { id: i64 },
    AddedToCollection { id: i64 },
    CollectionUpdate { id: i64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotificationCreate {
    pub recipient_account_id: i64,
    pub activity: NotificationActivity,
    pub silenced: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NotificationPolicyUpdate {
    pub for_bots: Option<i32>,
    pub for_limited_accounts: Option<i32>,
    pub for_new_accounts: Option<i32>,
    pub for_not_followers: Option<i32>,
    pub for_not_following: Option<i32>,
    pub for_private_mentions: Option<i32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthApplicationRegistration {
    pub name: String,
    pub redirect_uri: String,
    pub scopes: String,
    pub website: Option<String>,
}

pub struct OAuthApplicationRegistrationResult {
    pub application: super::records::OAuthApplication,
    pub client_secret: String,
}

#[derive(Clone, PartialEq)]
pub struct OAuthClientCredentialsToken {
    pub access_token: String,
    pub scopes: String,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, PartialEq)]
pub struct OAuthAuthorizationCodeToken {
    pub access_token: String,
    pub scopes: String,
    pub created_at: NaiveDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthAuthorizationGrant {
    pub code: String,
    pub scopes: String,
}

#[derive(Debug)]
pub enum OAuthClientCredentialsError {
    InvalidClient,
    InvalidScope,
    Database(sqlx::Error),
}

#[derive(Debug)]
pub enum OAuthTokenRevocationError {
    InvalidClient,
    UnauthorizedClient,
    Database(sqlx::Error),
}

#[derive(Debug)]
pub enum OAuthAuthorizationCodeError {
    InvalidClient,
    InvalidGrant,
    Database(sqlx::Error),
}

#[derive(Debug)]
pub enum OAuthAuthorizationGrantError {
    InvalidClient,
    InvalidRedirectUri,
    InvalidScope,
    InvalidCodeChallenge,
    Database(sqlx::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserAuthenticationMethod {
    Password,
    Totp,
    BackupCode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserAuthentication {
    pub user_id: i64,
    pub account_id: i64,
    pub method: BrowserAuthenticationMethod,
    password: VerifiedPassword,
}

/// A password check tied to the exact stored credential, never caller-supplied authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedPassword {
    user_id: i64,
    encrypted_password: super::types::SecretText,
}

#[derive(Debug)]
pub enum BrowserAuthenticationError {
    InvalidCredentials,
    Unconfirmed,
    PendingApproval,
    Memorialized,
    TwoFactorRequired,
    InvalidTwoFactor,
    RateLimited,
    Database(sqlx::Error),
}

impl fmt::Display for BrowserAuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCredentials => "invalid credentials",
            Self::Unconfirmed => "the login is not confirmed",
            Self::PendingApproval => "the login is pending approval",
            Self::Memorialized => "the account is memorialized",
            Self::TwoFactorRequired => "two-factor authentication is required",
            Self::InvalidTwoFactor => "the two-factor code is invalid",
            Self::RateLimited => "too many two-factor attempts",
            Self::Database(_) => "browser authentication database operation failed",
        })
    }
}

impl std::error::Error for BrowserAuthenticationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::InvalidCredentials
            | Self::Unconfirmed
            | Self::PendingApproval
            | Self::Memorialized
            | Self::TwoFactorRequired
            | Self::InvalidTwoFactor
            | Self::RateLimited => None,
        }
    }
}

impl From<sqlx::Error> for BrowserAuthenticationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<sqlx::Error> for OAuthClientCredentialsError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<sqlx::Error> for OAuthTokenRevocationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<sqlx::Error> for OAuthAuthorizationCodeError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<sqlx::Error> for OAuthAuthorizationGrantError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationCreateOutcome {
    Created { id: i64, filtered: bool },
    Existing { id: i64, filtered: bool },
    Dropped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BookmarkWriteOutcome {
    pub status_id: i64,
    pub removed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FavouriteWriteOutcome {
    pub status_id: i64,
    pub activity_id: Option<i64>,
    pub recipient_account_id: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReblogWriteOutcome {
    pub status_id: i64,
    pub target_status_id: i64,
    pub recipient_account_id: i64,
    pub created: bool,
    pub removed: bool,
    pub account_statuses_count_before_removal: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FollowWriteOutcome {
    pub activity_id: Option<i64>,
    pub recipient_account_id: i64,
    pub request: bool,
    pub activity_uri: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteFollowWriteOutcome {
    pub activity_id: i64,
    pub recipient_account_id: i64,
    pub request: bool,
    pub created: bool,
    pub silenced: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteFollowOutcome {
    Applied(RemoteFollowWriteOutcome),
    Rejected { recipient_account_id: i64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatusWriteOutcome {
    pub status_id: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollCreate {
    pub options: Vec<String>,
    pub expires_in: i64,
    pub multiple: bool,
    pub hide_totals: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemotePollVoteOutcome {
    Consumed,
    NotPollVote,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PollVoteWriteOutcome {
    pub poll_id: i64,
    pub status_id: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteNoteWriteOutcome {
    pub status_id: i64,
    pub mention_ids: Vec<(i64, i64)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteInteractionWriteOutcome {
    pub activity_id: i64,
    pub recipient_account_id: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteUndoReferenceKind {
    Follow,
    Block,
    Announce,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccountPurgeOutcome {
    Skipped,
    Purged,
    AlreadyPurged,
}

#[derive(Clone, Debug, Default)]
pub struct StatusUpdate {
    pub text: Option<String>,
    pub spoiler_text: Option<String>,
    pub sensitive: Option<bool>,
    pub language: Option<String>,
    pub media_ids: Option<Vec<i64>>,
    pub media_attributes: Option<Vec<StatusMediaAttributeUpdate>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusMediaAttributeUpdate {
    pub id: i64,
    pub description: AccountProfileValue<String>,
    pub focus: AccountProfileValue<MediaFocus>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatedLocalUser {
    pub account_id: i64,
    pub user_id: i64,
    pub confirmed: bool,
}

#[derive(Debug)]
pub enum WriteError {
    Sqlx(sqlx::Error),
    Job(JobError),
    Filesystem(io::Error),
    Conflict,
    InvalidInput(&'static str),
    NotFound,
    Unauthorized,
    Forbidden,
    RateLimited,
    Validation(&'static str),
}

impl fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlx(_) => formatter.write_str("PostgreSQL rejected a Mastodon write"),
            Self::Job(_) => formatter.write_str("Mastodon write outbox metadata was invalid"),
            Self::Filesystem(_) => {
                formatter.write_str("Mastodon media filesystem operation failed")
            }
            Self::Conflict => formatter.write_str("Mastodon rejected a stale write"),
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::NotFound => formatter.write_str("Mastodon write target was not found"),
            Self::Unauthorized => formatter.write_str("Mastodon write authorization is required"),
            Self::Forbidden => formatter.write_str("This action is not allowed"),
            Self::RateLimited => formatter.write_str("Mastodon report rate limit exceeded"),
            Self::Validation(message) => write!(formatter, "Validation failed: {message}"),
        }
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlx(error) => Some(error),
            Self::Job(error) => Some(error),
            Self::Filesystem(error) => Some(error),
            Self::Conflict
            | Self::InvalidInput(_)
            | Self::NotFound
            | Self::Unauthorized
            | Self::Forbidden
            | Self::RateLimited
            | Self::Validation(_) => None,
        }
    }
}

impl From<sqlx::Error> for WriteError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

impl From<JobError> for WriteError {
    fn from(error: JobError) -> Self {
        Self::Job(error)
    }
}

impl From<io::Error> for WriteError {
    fn from(error: io::Error) -> Self {
        Self::Filesystem(error)
    }
}

#[derive(Clone)]
pub struct WriteRepository {
    pool: PgPool,
    local_domain: Option<String>,
    active_record_encryption: Option<ActiveRecordEncryptionConfig>,
    #[cfg(feature = "test-support")]
    local_media_cleanup_intent_fault: Option<Arc<AtomicBool>>,
    #[cfg(feature = "test-support")]
    relationship_write_barrier: Option<Arc<tokio::sync::Barrier>>,
}

fn two_factor_attempt_is_rate_limited(failures: i64) -> bool {
    failures.saturating_add(1) >= MAX_TWO_FACTOR_ATTEMPTS_PER_HOUR
}

/// Validates and normalizes a local poll definition.
///
/// # Errors
///
/// Returns an error when option counts, lengths, or expiration bounds are invalid.
pub fn prepare_local_poll(
    options: &[String],
    expires_in: i64,
    multiple: bool,
    hide_totals: bool,
) -> Result<PollCreate, &'static str> {
    let options = options
        .iter()
        .map(|option| option.trim())
        .filter(|option| !option.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if options.len() < 2 {
        return Err("Options must have more than one item");
    }
    if options.len() > 4 {
        return Err("Options can't contain more than 4 items");
    }
    if options
        .iter()
        .any(|option| option.graphemes(true).count() > 50)
    {
        return Err("Options cannot be longer than 50 characters each");
    }
    if options
        .iter()
        .enumerate()
        .any(|(index, option)| options[..index].iter().any(|candidate| candidate == option))
    {
        return Err("Options contain duplicate items");
    }
    if expires_in < 300 {
        return Err("Expires at is too soon");
    }
    if expires_in > 2_629_746 {
        return Err("Expires at is too far into the future");
    }
    Ok(PollCreate {
        options,
        expires_in,
        multiple,
        hide_totals,
    })
}

#[allow(clippy::missing_errors_doc)]
impl WriteRepository {
    pub async fn connect(database_url: &str) -> sqlx::Result<Self> {
        let options = database_url.parse::<PgConnectOptions>()?;
        Self::connect_with(options).await
    }

    pub async fn connect_with(options: PgConnectOptions) -> sqlx::Result<Self> {
        Self::connect_with_pool_size(options, 5).await
    }

    pub async fn connect_with_pool_size(
        options: PgConnectOptions,
        pool_size: u32,
    ) -> sqlx::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(pool_size.max(1))
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    for setting in [
                        "SET TIME ZONE 'UTC'",
                        "SET search_path TO pg_catalog, public, pg_temp",
                        "SET lock_timeout TO '10s'",
                        "SET statement_timeout TO '60s'",
                    ] {
                        sqlx::query(setting).execute(&mut *connection).await?;
                    }
                    Ok(())
                })
            })
            .connect_with(options)
            .await?;
        Ok(Self {
            pool,
            local_domain: None,
            active_record_encryption: None,
            #[cfg(feature = "test-support")]
            local_media_cleanup_intent_fault: None,
            #[cfg(feature = "test-support")]
            relationship_write_barrier: None,
        })
    }

    #[must_use]
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            local_domain: None,
            active_record_encryption: None,
            #[cfg(feature = "test-support")]
            local_media_cleanup_intent_fault: None,
            #[cfg(feature = "test-support")]
            relationship_write_barrier: None,
        }
    }

    /// Configures the account domain used to resolve qualified local status mentions.
    #[must_use]
    pub fn with_local_domain(mut self, local_domain: impl Into<String>) -> Self {
        self.local_domain = Some(local_domain.into());
        self
    }

    #[must_use]
    pub fn with_active_record_encryption(
        mut self,
        encryption: ActiveRecordEncryptionConfig,
    ) -> Self {
        self.active_record_encryption = Some(encryption);
        self
    }

    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn with_local_media_cleanup_intent_fault(mut self) -> Self {
        self.local_media_cleanup_intent_fault = Some(Arc::new(AtomicBool::new(true)));
        self
    }

    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn with_relationship_write_barrier_for_test(mut self, parties: usize) -> Self {
        self.relationship_write_barrier = Some(Arc::new(tokio::sync::Barrier::new(parties)));
        self
    }

    #[cfg(feature = "test-support")]
    pub async fn apply_remote_follow_for_test(
        &self,
        source_account_id: i64,
        follow_uri: &str,
        object_uri: &str,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<Option<RemoteFollowOutcome>, WriteError> {
        self.apply_remote_follow(
            source_account_id,
            follow_uri,
            object_uri,
            origin,
            delivery_target_account_id,
        )
        .await
    }

    #[cfg(feature = "test-support")]
    pub async fn apply_remote_undo_follow_for_test(
        &self,
        source_account_id: i64,
        follow_uri: &str,
        target_uri: Option<&str>,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        self.apply_remote_undo_follow(
            source_account_id,
            follow_uri,
            target_uri,
            origin,
            delivery_target_account_id,
        )
        .await
    }

    #[cfg(feature = "test-support")]
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_remote_follow_decision_for_test(
        &self,
        source_account_id: i64,
        follow_uri: &str,
        target_uri: Option<&str>,
        local_actor_uri: Option<&str>,
        accepted: bool,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        self.apply_remote_follow_decision(
            source_account_id,
            follow_uri,
            target_uri,
            local_actor_uri,
            accepted,
            origin,
            delivery_target_account_id,
        )
        .await
    }

    #[cfg(feature = "test-support")]
    pub async fn apply_remote_block_for_test(
        &self,
        source_account_id: i64,
        block_uri: &str,
        object_uri: &str,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        self.apply_remote_block(
            source_account_id,
            block_uri,
            object_uri,
            origin,
            delivery_target_account_id,
        )
        .await
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn decrypt_otp_secret(&self, secret: Option<SecretText>) -> sqlx::Result<Option<SecretText>> {
        let Some(secret) = secret else {
            return Ok(None);
        };
        let Some(encryption) = self.active_record_encryption.as_ref() else {
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

    fn encrypt_otp_secret(&self, secret: &str) -> Result<String, WriteError> {
        self.active_record_encryption.as_ref().map_or_else(
            || Ok(secret.to_owned()),
            |encryption| {
                encryption
                    .encrypt_string(secret)
                    .map_err(|_| WriteError::Validation("two-factor secret could not be stored"))
            },
        )
    }

    /// Runs an operation while holding the canonical lock for one domain and its remote writes.
    ///
    /// The lock lives in a short-lived transaction on a dedicated pool connection so it is
    /// released even if the operation returns an error. The operation itself may use the rest of
    /// this repository's pool, which lets callers hold the lock across filesystem work without
    /// coupling filesystem state to a `PostgreSQL` transaction.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lock cannot be acquired or released, or the operation's
    /// error.
    pub(crate) async fn with_domain_lock<F, Fut, T>(
        &self,
        domain: &str,
        operation: F,
    ) -> Result<T, WriteError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, WriteError>>,
    {
        let mut connection =
            PgConnection::connect_with(self.pool.connect_options().as_ref()).await?;
        sqlx::query("SET lock_timeout TO '10s'")
            .execute(&mut connection)
            .await?;
        let mut lock_transaction = Connection::begin(&mut connection).await?;
        lock_domain_scope(&mut lock_transaction, domain).await?;
        let result = operation().await;
        let rollback = lock_transaction.rollback().await;
        match (result, rollback) {
            (Err(error), _) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    /// Runs an operation while holding the canonical lock for one account lifecycle.
    ///
    /// The lock lives in a short-lived transaction on a dedicated pool connection so it remains
    /// held across database and filesystem work without coupling filesystem state to a database
    /// transaction.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lock cannot be acquired or released, or the operation's
    /// error.
    pub(crate) async fn with_account_lock<F, Fut, T>(
        &self,
        account_id: i64,
        operation: F,
    ) -> Result<T, WriteError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, WriteError>>,
    {
        let mut connection =
            PgConnection::connect_with(self.pool.connect_options().as_ref()).await?;
        sqlx::query("SET lock_timeout TO '10s'")
            .execute(&mut connection)
            .await?;
        let mut lock_transaction = Connection::begin(&mut connection).await?;
        lock_account_scope(&mut lock_transaction, account_id).await?;
        let result = operation().await;
        let rollback = lock_transaction.rollback().await;
        match (result, rollback) {
            (Err(error), _) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    /// Verifies that a local account can still perform a write after authentication.
    ///
    /// The account and user rows are locked for the duration of this check. Callers that need to
    /// perform filesystem work must hold [`Self::with_account_lock`] around this method and the
    /// complete operation.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the account is no longer functional.
    pub(crate) async fn ensure_account_write_allowed(
        &self,
        account_id: i64,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        ensure_account_write_allowed_in(&mut transaction, account_id).await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn preheal_relationship_account_stats(
        &self,
        account_id: i64,
        target_account_id: i64,
    ) -> Result<(), WriteError> {
        // Initialize missing FK-backed stats rows one account transaction at a
        // time before a local reciprocal write takes either account-row lock.
        self.repair_account_stats_for_accounts(&[account_id, target_account_id], false)
            .await?;
        Ok(())
    }

    async fn local_activitypub_account_id_before_relationship_locks(
        &self,
        object_uri: &str,
        origin: &str,
    ) -> Result<Option<i64>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let account_id = local_activitypub_account_id(&mut transaction, object_uri, origin).await?;
        transaction.commit().await?;
        Ok(account_id)
    }

    async fn begin_relationship_account_write(
        &self,
        account_id: i64,
        target_account_id: i64,
    ) -> Result<Transaction<'_, Postgres>, WriteError> {
        self.preheal_relationship_account_stats(account_id, target_account_id)
            .await?;
        #[cfg(feature = "test-support")]
        if let Some(barrier) = &self.relationship_write_barrier {
            barrier.wait().await;
        }
        let mut transaction = self.pool.begin().await?;
        // Serialize the unordered pair before locking the authenticated source.
        // The winner can then acquire FK KEY SHARE on the other account without
        // a reciprocal writer retaining that account while waiting on this lock.
        lock_relationship(&mut transaction, account_id, target_account_id).await?;
        ensure_account_write_allowed_in(&mut transaction, account_id).await?;
        Ok(transaction)
    }

    async fn begin_account_write(
        &self,
        authenticated: &AuthenticatedBearer,
        scopes: RequiredScopes,
    ) -> Result<(i64, Transaction<'_, Postgres>), WriteError> {
        let account_id = write_account(authenticated, scopes)?;
        let mut transaction = self.pool.begin().await?;
        ensure_account_write_allowed_in(&mut transaction, account_id).await?;
        Ok((account_id, transaction))
    }

    /// Runs an operation while holding all suffix locks needed for a remote host.
    ///
    /// A domain block for `example.test` takes the `example.test` lock, while an actor on
    /// `relay.example.test` takes both `example.test` and `relay.example.test`. This prevents an
    /// actor upsert or media write from racing a parent-domain moderation operation.
    ///
    /// # Errors
    ///
    /// Returns a database error when the locks cannot be acquired or released, or the operation's
    /// error.
    pub(crate) async fn with_remote_domain_locks<F, Fut, T>(
        &self,
        domain: &str,
        operation: F,
    ) -> Result<T, WriteError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, WriteError>>,
    {
        self.with_remote_domains_locks(&[domain], operation).await
    }

    pub(crate) async fn with_remote_domains_locks<F, Fut, T>(
        &self,
        domains: &[&str],
        operation: F,
    ) -> Result<T, WriteError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, WriteError>>,
    {
        let mut connection =
            PgConnection::connect_with(self.pool.connect_options().as_ref()).await?;
        sqlx::query("SET lock_timeout TO '10s'")
            .execute(&mut connection)
            .await?;
        let mut lock_transaction = Connection::begin(&mut connection).await?;
        let scopes = domains
            .iter()
            .flat_map(|domain| remote_domain_lock_scopes(domain))
            .collect::<std::collections::BTreeSet<_>>();
        for scope in scopes {
            lock_domain_scope(&mut lock_transaction, &scope).await?;
        }
        let result = operation().await;
        let rollback = lock_transaction.rollback().await;
        match (result, rollback) {
            (Err(error), _) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    /// Evaluates remote media policy using the caller's write transaction.
    ///
    /// The caller must hold the corresponding remote-domain advisory locks while evaluating and
    /// applying the policy decision.
    ///
    /// # Errors
    ///
    /// Returns a database error when the policy rows cannot be read.
    pub(crate) async fn remote_media_allowed_in_transaction(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        domain: &str,
        limited_federation: bool,
    ) -> Result<bool, WriteError> {
        remote_media_allowed_in_transaction(transaction, domain, limited_federation).await
    }

    /// Collects frontend reconciliation events in the transaction that installs remote media.
    pub(crate) async fn collect_remote_media_installed_stream_events_in(
        transaction: &mut Transaction<'_, Postgres>,
        pending: &mut Vec<PendingStreamEvent>,
        status_id: i64,
        media_id: i64,
    ) -> Result<(), WriteError> {
        let key = StreamEventLogicalKey::Media(media_id);
        let after = status_timeline_snapshot(transaction, status_id).await?;
        let mut before = after.clone();
        before.had_media = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM media_attachments \
             WHERE status_id = $1 AND id <> $2)",
        )
        .bind(status_id)
        .bind(media_id)
        .fetch_one(&mut **transaction)
        .await?;
        collect_status_stream_transition(
            transaction,
            pending,
            status_id,
            "status.update",
            key,
            Some(before),
            Some(after),
        )
        .await?;
        collect_status_update_notification_stream_events_with_key(
            transaction,
            pending,
            status_id,
            key,
        )
        .await?;
        let wrapper_ids = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses
              WHERE reblog_of_id = $1 AND deleted_at IS NULL
              ORDER BY id",
        )
        .bind(status_id)
        .fetch_all(&mut **transaction)
        .await?;
        for wrapper_id in wrapper_ids {
            collect_status_stream_events_with_key(
                transaction,
                pending,
                wrapper_id,
                "status.update",
                key,
            )
            .await?;
        }
        Ok(())
    }

    /// Replace one user's complete web-client snapshot, never their posting defaults.
    /// The unique user index makes concurrent first saves atomic; the last writer wins.
    pub async fn update_web_settings(
        &self,
        user_id: i64,
        account_id: i64,
        data: &Value,
    ) -> Result<(), WriteError> {
        if !data.is_object() {
            return Err(WriteError::InvalidInput("data must be an object"));
        }
        let mut transaction = self.pool.begin().await?;
        lock_account_scope(&mut transaction, account_id).await?;
        ensure_account_write_allowed_in(&mut transaction, account_id).await?;
        let result = sqlx::query(
            "INSERT INTO web_settings (user_id, data, created_at, updated_at) \
             SELECT id, $3, clock_timestamp(), clock_timestamp() FROM users \
             WHERE id = $1 AND account_id = $2 \
             ON CONFLICT (user_id) DO UPDATE \
             SET data = EXCLUDED.data, updated_at = EXCLUDED.updated_at",
        )
        .bind(user_id)
        .bind(account_id)
        .bind(data)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(WriteError::NotFound);
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Creates one user report and queues the first staff notification atomically.
    ///
    /// # Errors
    ///
    /// Returns a write error when the authenticated account, target, or attached records are not
    /// valid, or when `PostgreSQL` rejects the transaction.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn create_report(
        &self,
        authenticated: &AuthenticatedBearer,
        target_account_id: i64,
        comment: &str,
        category: Option<&str>,
        status_ids: &[i64],
        collection_ids: &[i64],
        rule_ids: &[i64],
        forward: Option<bool>,
        forward_to_domains: Option<&[String]>,
        origin: &str,
        report_mail_enabled: bool,
    ) -> Result<i64, WriteError> {
        let comment = if comment.trim().is_empty() {
            ""
        } else {
            comment
        };
        if comment.chars().count() > 1_000 {
            return Err(WriteError::Validation("comment is too long"));
        }
        let category = report_category_value(category, !rule_ids.is_empty())?;
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_REPORTS)
            .await?;
        let Some((target_remote, target_unavailable, source_local, target_domain)) =
            sqlx::query_as::<_, (bool, bool, bool, Option<String>)>(
                "SELECT target.domain IS NOT NULL, \
                        target.suspended_at IS NOT NULL AND target.id <> -99, \
                        source.domain IS NULL, target.domain \
                 FROM accounts source JOIN accounts target ON target.id = $2 \
                 WHERE source.id = $1 FOR UPDATE OF target",
            )
            .bind(account_id)
            .bind(target_account_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Err(WriteError::NotFound);
        };
        if target_unavailable {
            return Err(WriteError::NotFound);
        }
        let forward_to_domains = forward_to_domains.map_or_else(
            || target_domain.iter().cloned().collect(),
            ToOwned::to_owned,
        );

        let invalid_status = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM unnest($1::bigint[]) requested(id) \
                WHERE NOT EXISTS ( \
                    SELECT 1 FROM statuses status \
                    WHERE status.id = requested.id AND status.account_id = $2 \
                      AND NOT EXISTS ( \
                    SELECT 1 FROM blocks blocked \
                        WHERE blocked.account_id = $2 AND blocked.target_account_id = $3) \
                      AND ( \
                        status.reblog_of_id IS NULL OR NOT EXISTS ( \
                            SELECT 1 \
                              FROM statuses original \
                              JOIN accounts original_account ON original_account.id = original.account_id \
                             WHERE original.id = status.reblog_of_id \
                               AND ( \
                                 EXISTS ( \
                                     SELECT 1 FROM blocks blocked_original \
                                      WHERE blocked_original.account_id = $3 \
                                        AND blocked_original.target_account_id = original.account_id) \
                                 OR EXISTS ( \
                                     SELECT 1 FROM blocks blocking_original \
                                      WHERE blocking_original.account_id = original.account_id \
                                        AND blocking_original.target_account_id = $3) \
                                 OR EXISTS ( \
                                     SELECT 1 FROM mutes muted_original \
                                      WHERE muted_original.account_id = $3 \
                                        AND muted_original.target_account_id = original.account_id) \
                                 OR EXISTS ( \
                                     SELECT 1 FROM account_domain_blocks blocked_domain \
                                      WHERE blocked_domain.account_id = $3 \
                                        AND original_account.domain IS NOT NULL \
                                        AND lower(blocked_domain.domain) = lower(original_account.domain))))) \
                      AND ( \
                        $3 = $2 OR status.visibility IN (0, 1) \
                        OR (status.visibility = 2 AND EXISTS ( \
                            SELECT 1 FROM follows follow \
                            WHERE follow.account_id = $3 AND follow.target_account_id = $2)) \
                        OR (status.visibility IN (2, 3, 4) AND EXISTS ( \
                            SELECT 1 FROM mentions mention \
                            WHERE mention.status_id = status.id AND mention.account_id = $3)))))",
        )
        .bind(status_ids)
        .bind(target_account_id)
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if invalid_status {
            return Err(WriteError::NotFound);
        }

        let invalid_collection = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM unnest($1::bigint[]) requested(id) \
                WHERE NOT EXISTS ( \
                    SELECT 1 FROM collections collection \
                    WHERE collection.id = requested.id AND collection.account_id = $2))",
        )
        .bind(collection_ids)
        .bind(target_account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if invalid_collection {
            return Err(WriteError::NotFound);
        }
        if !rule_ids.is_empty() {
            if rule_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != rule_ids.len()
            {
                return Err(WriteError::Validation("invalid rules"));
            }
            let invalid_rule = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS ( \
                    SELECT 1 FROM unnest($1::bigint[]) requested(id) \
                    WHERE NOT EXISTS (SELECT 1 FROM rules rule WHERE rule.id = requested.id))",
            )
            .bind(rule_ids)
            .fetch_one(&mut *transaction)
            .await?;
            if invalid_rule {
                return Err(WriteError::Validation("invalid rules"));
            }
        }

        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock( \
                 pg_catalog.hashtextextended($1, 0))",
        )
        .bind(format!("rustodon.report_rate_limit:{account_id}"))
        .execute(&mut *transaction)
        .await?;
        let report_count: i64 = sqlx::query_scalar(REPORT_RATE_LIMIT_COUNT_SQL)
            .bind(account_id)
            .fetch_one(&mut *transaction)
            .await?;
        if report_count >= REPORT_RATE_LIMIT {
            return Err(WriteError::RateLimited);
        }

        let unresolved_sibling = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM reports report \
                WHERE report.target_account_id = $1 AND report.action_taken_at IS NULL)",
        )
        .bind(target_account_id)
        .fetch_one(&mut *transaction)
        .await?;
        let report_uri = source_local.then(|| {
            format!(
                "{}/payloads/{}",
                origin.trim_end_matches('/'),
                random_uuid()
            )
        });
        let forwarded = forward.map(|requested| {
            requested
                && target_remote
                && target_domain.as_deref().is_some_and(|domain| {
                    forward_to_domains
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(domain))
                })
        });
        let report_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO reports ( \
                account_id, target_account_id, application_id, category, comment, forwarded, \
                rule_ids, status_ids, uri, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, \
                     clock_timestamp(), clock_timestamp()) RETURNING id",
        )
        .bind(account_id)
        .bind(target_account_id)
        .bind(authenticated.application_id())
        .bind(category)
        .bind(comment)
        .bind(forwarded)
        .bind((!rule_ids.is_empty()).then_some(rule_ids.to_vec()))
        .bind(status_ids)
        .bind(report_uri)
        .fetch_one(&mut *transaction)
        .await?;

        for collection_id in collection_ids {
            sqlx::query(
                "INSERT INTO collection_reports \
                    (collection_id, report_id, created_at, updated_at) \
                 VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
            )
            .bind(collection_id)
            .bind(report_id)
            .execute(&mut *transaction)
            .await?;
        }

        if !unresolved_sibling {
            let (reporter_username, reporter_domain, target_username, target_domain) =
                sqlx::query_as::<_, (String, Option<String>, String, Option<String>)>(
                    "SELECT reporter.username, reporter.domain, target.username, target.domain \
                     FROM accounts reporter JOIN accounts target ON target.id = $2 \
                     WHERE reporter.id = $1",
                )
                .bind(account_id)
                .bind(target_account_id)
                .fetch_one(&mut *transaction)
                .await?;
            let target_label = report_account_label(&target_username, target_domain.as_deref());
            let reporter_label =
                report_account_label(&reporter_username, reporter_domain.as_deref());
            let staff_accounts = report_staff_accounts(&mut transaction).await?;
            for (staff_account_id, email, settings) in staff_accounts {
                record_outbox_in(
                    &mut transaction,
                    &notification_job(staff_account_id, "admin.report", report_id),
                )
                .await?;
                if report_mail_enabled && report_email_enabled(settings.as_deref()) {
                    let mail = report_job(
                        &email,
                        origin,
                        report_id,
                        &target_label,
                        &reporter_label,
                        staff_account_id,
                    );
                    record_outbox_in(&mut transaction, &mail).await?;
                }
            }
        }
        if target_remote && forward.unwrap_or(false) {
            record_report_forwarding(&mut transaction, report_id, origin, &forward_to_domains)
                .await?;
        }
        transaction.commit().await?;
        Ok(report_id)
    }

    /// Creates a report received from a remote `ActivityPub` actor.
    ///
    /// Returns `Ok(None)` when the source domain rejects reports or none of the
    /// Flag objects identify a known target account.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn create_remote_report(
        &self,
        source_account_id: i64,
        object_uris: &[String],
        comment: &str,
        report_uri: Option<&str>,
        origin: &str,
        local_domain: &str,
        report_mail_enabled: bool,
    ) -> Result<Option<i64>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_account_scope(&mut transaction, source_account_id).await?;
        let Some((source_domain, source_suspended)) = sqlx::query_as::<_, (String, bool)>(
            "SELECT domain, suspended_at IS NOT NULL
               FROM accounts
              WHERE id = $1 AND domain IS NOT NULL",
        )
        .bind(source_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Err(WriteError::NotFound);
        };
        if source_suspended {
            return Ok(None);
        }
        for scope in remote_domain_lock_scopes(&source_domain) {
            lock_domain_scope(&mut transaction, &scope).await?;
        }
        let source_suspended = sqlx::query_scalar::<_, bool>(
            "SELECT suspended_at IS NOT NULL FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(source_account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if source_suspended {
            return Ok(None);
        }
        let policy_domain = domain_policy_hostname(&source_domain);
        let rejects_reports = sqlx::query_scalar::<_, bool>(
            "SELECT COALESCE((
                 SELECT reject_reports
                   FROM domain_blocks
                  WHERE lower(domain) = lower(trim(trailing '.' FROM $1))
                     OR lower(trim(trailing '.' FROM $1)) LIKE '%.' || lower(domain)
                  ORDER BY char_length(domain) DESC
                  LIMIT 1
               ), false)",
        )
        .bind(&policy_domain)
        .fetch_one(&mut *transaction)
        .await?;
        if rejects_reports {
            return Ok(None);
        }

        let mut target_account_ids = Vec::new();
        for object_uri in object_uris {
            if let Some(account_id) =
                local_activitypub_account_id(&mut transaction, object_uri, origin).await?
                && !target_account_ids.contains(&account_id)
            {
                target_account_ids.push(account_id);
            }
        }
        if target_account_ids.is_empty() {
            return Ok(None);
        }
        target_account_ids.sort_unstable();
        let comment = comment.chars().take(5_000).collect::<String>();
        let report_uri = report_uri.filter(|uri| report_uri_matches_domain(uri, &source_domain));
        let mut last_report_id = None;
        for target_account_id in target_account_ids {
            lock_account_scope(&mut transaction, target_account_id).await?;
            let unavailable = sqlx::query_scalar::<_, bool>(
                "SELECT suspended_at IS NOT NULL FROM accounts WHERE id = $1 FOR UPDATE",
            )
            .bind(target_account_id)
            .fetch_one(&mut *transaction)
            .await?;
            if unavailable {
                continue;
            }
            let (status_ids, collection_ids) = remote_report_target_content(
                &mut transaction,
                target_account_id,
                &source_domain,
                origin,
                local_domain,
                object_uris,
            )
            .await?;
            let unresolved_sibling = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                     SELECT 1 FROM reports report
                      WHERE report.target_account_id = $1 AND report.action_taken_at IS NULL
                 )",
            )
            .bind(target_account_id)
            .fetch_one(&mut *transaction)
            .await?;
            let report_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO reports (
                     account_id, target_account_id, application_id, category, comment, forwarded,
                     rule_ids, status_ids, uri, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                         clock_timestamp(), clock_timestamp())
                 RETURNING id",
            )
            .bind(source_account_id)
            .bind(target_account_id)
            .bind(Option::<i64>::None)
            .bind(0_i32)
            .bind(&comment)
            .bind(false)
            .bind(Option::<Vec<i64>>::None)
            .bind(&status_ids)
            .bind(report_uri)
            .fetch_one(&mut *transaction)
            .await?;

            for collection_id in collection_ids {
                sqlx::query(
                    "INSERT INTO collection_reports
                        (collection_id, report_id, created_at, updated_at)
                     VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
                )
                .bind(collection_id)
                .bind(report_id)
                .execute(&mut *transaction)
                .await?;
            }

            if !unresolved_sibling {
                let (reporter_username, reporter_domain, target_username, target_domain) =
                    sqlx::query_as::<_, (String, Option<String>, String, Option<String>)>(
                        "SELECT reporter.username, reporter.domain, target.username, target.domain \
                         FROM accounts reporter JOIN accounts target ON target.id = $2 \
                         WHERE reporter.id = $1",
                    )
                    .bind(source_account_id)
                    .bind(target_account_id)
                    .fetch_one(&mut *transaction)
                    .await?;
                let target_label = report_account_label(&target_username, target_domain.as_deref());
                let reporter_label =
                    report_account_label(&reporter_username, reporter_domain.as_deref());
                for (staff_account_id, email, settings) in
                    report_staff_accounts(&mut transaction).await?
                {
                    record_outbox_in(
                        &mut transaction,
                        &notification_job(staff_account_id, "admin.report", report_id),
                    )
                    .await?;
                    if report_mail_enabled && report_email_enabled(settings.as_deref()) {
                        let mail = report_job(
                            &email,
                            origin,
                            report_id,
                            &target_label,
                            &reporter_label,
                            staff_account_id,
                        );
                        record_outbox_in(&mut transaction, &mail).await?;
                    }
                }
            }
            last_report_id = Some(report_id);
        }
        transaction.commit().await?;
        Ok(last_report_id)
    }

    /// Returns the number of reports created by an account in the current Mastodon rate window.
    ///
    /// # Errors
    ///
    /// Returns a database error when the report window cannot be inspected.
    pub async fn report_rate_limit_count(&self, account_id: i64) -> Result<i64, WriteError> {
        Ok(sqlx::query_scalar(REPORT_RATE_LIMIT_COUNT_SQL)
            .bind(account_id)
            .fetch_one(&self.pool)
            .await?)
    }

    /// Resolves or reopens a report for an authorized moderation account.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account cannot manage reports,
    /// [`WriteError::NotFound`] when the report does not exist, or a database error when the
    /// report and audit log cannot be updated atomically.
    pub async fn set_report_resolution(
        &self,
        acting_account_id: i64,
        report_id: i64,
        resolved: bool,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let can_manage_reports = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                 SELECT 1 FROM users account_user \
                 JOIN accounts account ON account.id = account_user.account_id \
                 JOIN user_roles role ON role.id = COALESCE(account_user.role_id, -99) \
                 LEFT JOIN user_roles everyone ON everyone.id = -99 \
                 WHERE account.id = $1 AND account.domain IS NULL \
                   AND account.suspended_at IS NULL \
                   AND account_user.confirmed_at IS NOT NULL \
                   AND account_user.approved = true \
                   AND account_user.disabled = false \
                   AND (role.permissions & 1 <> 0 OR \
                        ((role.permissions | COALESCE(everyone.permissions, 0)) & $2 <> 0)) \
             )",
        )
        .bind(acting_account_id)
        .bind(1_i64 << 4)
        .fetch_one(&mut *transaction)
        .await?;
        if !can_manage_reports {
            return Err(WriteError::Unauthorized);
        }

        let target_account_id =
            sqlx::query_scalar::<_, i64>("SELECT target_account_id FROM reports WHERE id = $1")
                .bind(report_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(WriteError::NotFound)?;
        sqlx::query("SELECT id FROM accounts WHERE id = $1 FOR UPDATE")
            .bind(target_account_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(WriteError::NotFound)?;
        let report_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM reports WHERE id = $1 FOR UPDATE)",
        )
        .bind(report_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !report_exists {
            return Err(WriteError::NotFound);
        }

        if resolved {
            sqlx::query(
                "UPDATE reports SET action_taken_at = clock_timestamp(), \
                        action_taken_by_account_id = $2, updated_at = clock_timestamp() \
                 WHERE id = $1",
            )
            .bind(report_id)
            .bind(acting_account_id)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query(
                "UPDATE reports SET action_taken_at = NULL, action_taken_by_account_id = NULL, \
                        updated_at = clock_timestamp() \
                 WHERE id = $1",
            )
            .bind(report_id)
            .execute(&mut *transaction)
            .await?;
        }

        let action = if resolved { "resolve" } else { "reopen" };
        sqlx::query(
            "INSERT INTO admin_action_logs ( \
                 account_id, action, created_at, human_identifier, route_param, target_id, \
                 target_type, updated_at) \
             VALUES ($1, $2, clock_timestamp(), $3, NULL, $4, 'Report', clock_timestamp())",
        )
        .bind(acting_account_id)
        .bind(action)
        .bind(report_id.to_string())
        .bind(report_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Suspends or unsuspends an account for an authorized moderation account.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account cannot perform the requested
    /// action, [`WriteError::NotFound`] for an unknown or instance account, or a database error
    /// when the account, audit record, and local actor update cannot commit atomically.
    #[allow(clippy::too_many_lines)]
    pub async fn set_account_suspension(
        &self,
        acting_account_id: i64,
        account_id: i64,
        suspended: bool,
        origin: &str,
    ) -> Result<(), WriteError> {
        let permission_mask = if suspended {
            (1_i64 << 4) | (1_i64 << 10)
        } else {
            1_i64 << 10
        };
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_account_scope(&mut transaction, account_id).await?;
        let Some(actor_position) =
            authorized_admin_account(&mut transaction, acting_account_id, permission_mask).await?
        else {
            return Err(WriteError::Unauthorized);
        };
        let target = sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                Option<NaiveDateTime>,
                Option<i32>,
                String,
                Option<i32>,
            ),
        >(
            "SELECT username, domain, suspended_at, suspension_origin, uri, id_scheme FROM accounts \
             WHERE id = $1 AND id <> -99 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if !suspended && target.1.is_some() {
            return Err(WriteError::InvalidInput(
                "remote account unsuspension requires a fresh remote account resolution",
            ));
        }
        if suspended {
            let target_position = sqlx::query_scalar::<_, i32>(
                "SELECT target_role.position FROM accounts target \
                 LEFT JOIN users target_user ON target_user.account_id = target.id \
                 JOIN user_roles target_role ON target_role.id = COALESCE(target_user.role_id, -99) \
                 WHERE target.id = $1",
            )
            .bind(account_id)
            .fetch_one(&mut *transaction)
            .await?;
            if actor_position <= target_position {
                return Err(WriteError::Unauthorized);
            }
        }
        if suspended {
            if target.2.is_some() {
                return Err(WriteError::Validation("account is already suspended"));
            }
        } else {
            if target.2.is_none() {
                return Err(WriteError::Validation("account is not suspended"));
            }
            if target.3 != Some(0) {
                return Err(WriteError::Unauthorized);
            }
        }

        let human_identifier = target.1.as_deref().map_or_else(
            || target.0.clone(),
            |domain| format!("{}@{domain}", target.0),
        );
        let user_email = if target.1.is_none() {
            sqlx::query_scalar::<_, String>(
                "SELECT email FROM users WHERE account_id = $1 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            None
        };
        let warning_id = if suspended {
            sqlx::query(
                "INSERT INTO account_deletion_requests (account_id, created_at, updated_at) \
                 SELECT $1, clock_timestamp(), clock_timestamp() \
                 WHERE NOT EXISTS (SELECT 1 FROM account_deletion_requests WHERE account_id = $1)",
            )
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
            let (deletion_request_id, deletion_created_at) =
                sqlx::query_as::<_, (i64, NaiveDateTime)>(
                    "SELECT id, created_at FROM account_deletion_requests \
                 WHERE account_id = $1 ORDER BY id LIMIT 1",
                )
                .bind(account_id)
                .fetch_one(&mut *transaction)
                .await?;
            if let Some(email) = user_email.as_deref() {
                sqlx::query(
                    "INSERT INTO canonical_email_blocks \
                         (canonical_email_hash, reference_account_id, created_at, updated_at) \
                     VALUES ($1, $2, clock_timestamp(), clock_timestamp()) \
                     ON CONFLICT (canonical_email_hash) DO NOTHING",
                )
                .bind(canonical_email_hash(email))
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
            }
            let warning_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO account_warnings ( \
                     account_id, action, created_at, report_id, status_ids, target_account_id, text, updated_at) \
                 VALUES ($1, 4000, clock_timestamp(), NULL, NULL, $2, '', clock_timestamp()) \
                 RETURNING id",
            )
            .bind(acting_account_id)
            .bind(account_id)
            .fetch_one(&mut *transaction)
            .await?;
            let report_ids = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM reports \
                 WHERE target_account_id = $1 AND action_taken_at IS NULL \
                 ORDER BY id FOR UPDATE",
            )
            .bind(account_id)
            .fetch_all(&mut *transaction)
            .await?;
            for report_id in report_ids {
                sqlx::query(
                    "UPDATE reports SET action_taken_at = clock_timestamp(), \
                            action_taken_by_account_id = $2, updated_at = clock_timestamp() \
                     WHERE id = $1",
                )
                .bind(report_id)
                .bind(acting_account_id)
                .execute(&mut *transaction)
                .await?;
                insert_admin_action_log(
                    &mut transaction,
                    acting_account_id,
                    "resolve",
                    report_id,
                    "Report",
                    report_id.to_string(),
                    None,
                )
                .await?;
            }
            let purge_run_at =
                deletion_created_at.and_utc() + ChronoDuration::days(ACCOUNT_DELETION_DELAY_DAYS);
            if target.1.is_none() {
                let actor_path = if target.5 == Some(AccountIdScheme::Numeric.raw()) {
                    format!("ap/users/{account_id}")
                } else {
                    format!("users/{}", target.0)
                };
                let actor_uri = format!("{}/{actor_path}", origin.trim_end_matches('/'));
                record_outbox_in(
                    &mut transaction,
                    &account_delete_job(account_id, &actor_uri).run_at(purge_run_at),
                )
                .await?;
            }
            record_outbox_in(
                &mut transaction,
                &account_purge_job(
                    account_id,
                    deletion_request_id,
                    deletion_created_at,
                    target.1.is_some().then_some(origin),
                ),
            )
            .await?;
            Some(warning_id)
        } else {
            sqlx::query("DELETE FROM account_deletion_requests WHERE account_id = $1")
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
            cancel_pending_account_job(
                &mut transaction,
                ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND,
                account_id,
            )
            .await?;
            cancel_pending_account_job(
                &mut transaction,
                MASTODON_ACCOUNT_PURGE_JOB_KIND,
                account_id,
            )
            .await?;
            cancel_activitypub_delivery(&mut transaction, &format!("{}#delete", target.4)).await?;
            if target.1.is_none() {
                sqlx::query("DELETE FROM canonical_email_blocks WHERE reference_account_id = $1")
                    .bind(account_id)
                    .execute(&mut *transaction)
                    .await?;
            }
            None
        };

        let transition_at =
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut *transaction)
                .await?;
        if suspended {
            collect_account_timeline_transition(
                &mut transaction,
                &mut pending_stream_events,
                account_id,
                "delete",
                transition_at.and_utc().timestamp_micros(),
            )
            .await?;
        }
        let (domain, updated_at) = if suspended {
            sqlx::query_as::<_, (Option<String>, NaiveDateTime)>(
                "UPDATE accounts SET suspended_at = $2, suspension_origin = 0, \
                    updated_at = $2 WHERE id = $1 RETURNING domain, updated_at",
            )
            .bind(account_id)
            .bind(transition_at)
            .fetch_one(&mut *transaction)
            .await?
        } else {
            sqlx::query_as::<_, (Option<String>, NaiveDateTime)>(
                "UPDATE accounts SET suspended_at = NULL, suspension_origin = NULL, \
                    updated_at = $2 WHERE id = $1 RETURNING domain, updated_at",
            )
            .bind(account_id)
            .bind(transition_at)
            .fetch_one(&mut *transaction)
            .await?
        };
        if !suspended {
            collect_account_timeline_transition(
                &mut transaction,
                &mut pending_stream_events,
                account_id,
                "update",
                transition_at.and_utc().timestamp_micros(),
            )
            .await?;
        }
        if domain.is_none() {
            if suspended {
                collect_account_kill_stream_event(
                    &mut pending_stream_events,
                    account_id,
                    updated_at,
                )?;
            }
            record_outbox_in(
                &mut transaction,
                &account_update_job(account_id, updated_at),
            )
            .await?;
            if let Some(warning_id) = warning_id {
                record_outbox_in(
                    &mut transaction,
                    &notification_job(account_id, "AccountWarning", warning_id),
                )
                .await?;
            }
        } else if suspended {
            reject_remote_account_follows(&mut transaction, account_id, origin).await?;
        }
        let action = if suspended { "suspend" } else { "unsuspend" };
        insert_admin_action_log(
            &mut transaction,
            acting_account_id,
            action,
            account_id,
            "Account",
            human_identifier,
            None,
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Starts self-service deletion for a local account and queues its actor deletion fan-out.
    ///
    /// The browser layer verifies the account challenge before calling this method. The account
    /// suspension, deletion request, stale actor-update cancellation, and durable deletion job
    /// are committed together so a successful response cannot lose the federation intent.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::NotFound`] when the account has no local user, [`WriteError::Validation`]
    /// when the account is already unavailable or has a pending request, or a database error when
    /// the transaction cannot commit.
    pub async fn request_account_deletion(
        &self,
        account_id: i64,
        actor_uri: &str,
    ) -> Result<(), WriteError> {
        if actor_uri.is_empty() {
            return Err(WriteError::InvalidInput("account actor URI is required"));
        }
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_account_scope(&mut transaction, account_id).await?;
        let Some((domain, suspended_at)) =
            sqlx::query_as::<_, (Option<String>, Option<NaiveDateTime>)>(
                "SELECT domain, suspended_at FROM accounts WHERE id = $1 AND id <> -99 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Err(WriteError::NotFound);
        };
        if domain.is_some() {
            return Err(WriteError::InvalidInput(
                "remote accounts cannot be deleted locally",
            ));
        }
        if suspended_at.is_some() {
            return Err(WriteError::Validation("account is already unavailable"));
        }
        let user_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM users WHERE account_id = $1)",
        )
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !user_exists {
            return Err(WriteError::NotFound);
        }
        let deletion_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM account_deletion_requests WHERE account_id = $1)",
        )
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if deletion_exists {
            return Err(WriteError::Validation(
                "account deletion is already pending",
            ));
        }
        let (deletion_request_id, deletion_created_at) = sqlx::query_as::<_, (i64, NaiveDateTime)>(
            "INSERT INTO account_deletion_requests (account_id, created_at, updated_at) \
             VALUES ($1, clock_timestamp(), clock_timestamp()) RETURNING id, created_at",
        )
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        let transition_at =
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut *transaction)
                .await?;
        collect_account_timeline_transition(
            &mut transaction,
            &mut pending_stream_events,
            account_id,
            "delete",
            transition_at.and_utc().timestamp_micros(),
        )
        .await?;
        let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
            "UPDATE accounts SET suspended_at = $2, suspension_origin = 0, \
             updated_at = $2 WHERE id = $1 RETURNING updated_at",
        )
        .bind(account_id)
        .bind(transition_at)
        .fetch_one(&mut *transaction)
        .await?;
        collect_account_kill_stream_event(&mut pending_stream_events, account_id, updated_at)?;
        sqlx::query(
            "DELETE FROM rustodon.outbox_events \
             WHERE kind = $1 AND dispatched_at IS NULL \
               AND payload -> 'arguments' ->> 'account_id' = $2",
        )
        .bind(ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND)
        .bind(account_id.to_string())
        .execute(&mut *transaction)
        .await?;
        record_outbox_in(&mut transaction, &account_delete_job(account_id, actor_uri)).await?;
        record_outbox_in(
            &mut transaction,
            &account_purge_job(account_id, deletion_request_id, deletion_created_at, None),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Purges the local content of a due self-service deletion while retaining its actor identity.
    ///
    /// The caller must hold the account lifecycle lock while collecting the filesystem manifest,
    /// invoking this method, and removing the manifest. A missing request after a suspended
    /// account has already been purged is reported separately from a canceled or not-yet-due
    /// request so stale leased jobs cannot remove media after unsuspension.
    ///
    /// # Errors
    ///
    /// Returns a database error when cleanup cannot commit atomically.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn purge_account_after_deletion(
        &self,
        account_id: i64,
        expected_deletion_request_id: Option<i64>,
        expected_deletion_created_at: Option<NaiveDateTime>,
        origin: Option<&str>,
    ) -> Result<AccountPurgeOutcome, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        let Some((domain, suspended_at)) =
            sqlx::query_as::<_, (Option<String>, Option<NaiveDateTime>)>(
                "SELECT domain, suspended_at FROM accounts WHERE id = $1 AND id <> -99 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Ok(AccountPurgeOutcome::AlreadyPurged);
        };
        if suspended_at.is_none() {
            return Ok(AccountPurgeOutcome::Skipped);
        }
        let deletion_request = sqlx::query_as::<_, (i64, NaiveDateTime, bool)>(
            "SELECT id, created_at,
                    created_at <= clock_timestamp() - interval '30 days'
               FROM account_deletion_requests
              WHERE account_id = $1
              ORDER BY id LIMIT 1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if expected_deletion_request_id.is_some_and(|expected| {
            deletion_request
                .as_ref()
                .is_some_and(|(request_id, _, _)| *request_id != expected)
        }) || (expected_deletion_request_id.is_none()
            && expected_deletion_created_at.is_some_and(|expected| {
                deletion_request
                    .as_ref()
                    .is_some_and(|(_, created_at, _)| *created_at != expected)
            }))
        {
            return Ok(AccountPurgeOutcome::Skipped);
        }
        let Some((_, _, due)) = deletion_request else {
            return Ok(AccountPurgeOutcome::AlreadyPurged);
        };
        if !due {
            return Ok(AccountPurgeOutcome::Skipped);
        }
        if domain.is_some() && origin.is_none() {
            return Err(WriteError::InvalidInput(
                "remote account purge requires the instance origin",
            ));
        }

        if let Some(origin) = origin {
            reject_remote_account_follows(&mut transaction, account_id, origin).await?;
            undo_remote_account_follows(&mut transaction, account_id, origin).await?;
        }
        let protected_status_ids = protected_status_ids(&mut transaction, account_id).await?;
        purge_account_user(&mut transaction, account_id).await?;
        purge_account_profile(&mut transaction, account_id).await?;
        purge_account_statuses(
            &mut transaction,
            &mut pending_stream_events,
            account_id,
            &protected_status_ids,
            true,
        )
        .await?;
        purge_account_mentions(&mut transaction, account_id, &protected_status_ids).await?;
        purge_account_media(&mut transaction, account_id, &protected_status_ids).await?;
        purge_account_relationships(&mut transaction, account_id).await?;
        purge_account_notifications(&mut transaction, account_id).await?;
        purge_account_associations(&mut transaction, account_id).await?;
        ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        sqlx::query(
            "UPDATE account_stats SET statuses_count = 0, following_count = 0,
                followers_count = 0, last_status_at = NULL, updated_at = clock_timestamp()
              WHERE account_id = $1",
        )
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM account_deletion_requests WHERE account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        flush_staged_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(AccountPurgeOutcome::Purged)
    }

    /// Collects the Paperclip metadata for the current due account purge.
    ///
    /// The caller must hold the account lifecycle lock until the returned paths have been
    /// persisted and the purge transaction has committed.
    ///
    /// # Errors
    ///
    /// Returns a database error when the account or media metadata cannot be inspected.
    pub(crate) async fn account_purge_media_metadata(
        &self,
        account_id: i64,
        expected_deletion_request_id: Option<i64>,
        expected_deletion_created_at: Option<NaiveDateTime>,
    ) -> Result<Vec<PaperclipMetadata>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let Some((domain, suspended_at)) =
            sqlx::query_as::<_, (Option<String>, Option<NaiveDateTime>)>(
                "SELECT domain, suspended_at FROM accounts WHERE id = $1 AND id <> -99 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Ok(Vec::new());
        };
        if suspended_at.is_none() {
            return Ok(Vec::new());
        }
        let deletion_request = sqlx::query_as::<_, (i64, NaiveDateTime, bool)>(
            "SELECT id, created_at,
                    created_at <= clock_timestamp() - interval '30 days'
               FROM account_deletion_requests
              WHERE account_id = $1
              ORDER BY id LIMIT 1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if expected_deletion_request_id.is_some_and(|expected| {
            deletion_request
                .as_ref()
                .is_some_and(|(request_id, _, _)| *request_id != expected)
        }) || (expected_deletion_request_id.is_none()
            && expected_deletion_created_at.is_some_and(|expected| {
                deletion_request
                    .as_ref()
                    .is_some_and(|(_, created_at, _)| *created_at != expected)
            }))
        {
            return Ok(Vec::new());
        }
        let Some((_, _, due)) = deletion_request else {
            return Ok(Vec::new());
        };
        if !due {
            return Ok(Vec::new());
        }
        let protected_status_ids = protected_status_ids(&mut transaction, account_id).await?;
        let metadata = account_media_metadata_for_cleanup(
            &mut transaction,
            account_id,
            domain.is_some(),
            &protected_status_ids,
        )
        .await?;
        transaction.commit().await?;
        Ok(metadata)
    }

    /// Creates or updates a global domain block for an authorized federation moderator.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account lacks `manage_federation`,
    /// [`WriteError::InvalidInput`] for an invalid domain or severity, or a database error when
    /// the block and audit record cannot commit atomically.
    #[allow(clippy::too_many_lines)]
    pub async fn set_domain_block(
        &self,
        acting_account_id: i64,
        domain: &str,
        severity: i32,
        reject_media: bool,
        reject_reports: bool,
        origin: &str,
    ) -> Result<i64, WriteError> {
        if !matches!(severity, 0..=2) {
            return Err(WriteError::InvalidInput("domain block severity is invalid"));
        }
        let domain = normalize_domain_block_domain(domain)?;
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        if authorized_admin_account(&mut transaction, acting_account_id, 1_i64 << 5)
            .await?
            .is_none()
        {
            return Err(WriteError::Unauthorized);
        }
        lock_domain_scope(&mut transaction, &domain).await?;
        let existing = sqlx::query_as::<_, (i64, String, Option<i32>, bool, bool, NaiveDateTime)>(
            "SELECT id, domain, severity, reject_media, reject_reports, created_at \
             FROM domain_blocks \
             WHERE lower(domain) = lower($1) OR lower($1) LIKE '%.' || lower(domain) \
             ORDER BY length(domain) DESC, id DESC FOR UPDATE",
        )
        .bind(&domain)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some((_, _, existing_severity, existing_reject_media, existing_reject_reports, _)) =
            existing.as_ref()
            && !domain_block_is_stricter(
                severity,
                reject_media,
                reject_reports,
                *existing_severity,
                *existing_reject_media,
                *existing_reject_reports,
            )
        {
            return Err(WriteError::Validation(
                "domain block would downgrade an existing rule",
            ));
        }
        let exact_existing = existing.filter(|(_, existing_domain, _, _, _, _)| {
            existing_domain.eq_ignore_ascii_case(&domain)
        });
        let policy_account_ids = if matches!(severity, 0 | 1) {
            let policy_domain = domain_policy_hostname(&domain);
            sqlx::query_scalar::<_, i64>(
                "SELECT id FROM accounts WHERE domain IS NOT NULL \
                   AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                            THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
                     OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                            THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
                 ORDER BY id",
            )
            .bind(&policy_domain)
            .fetch_all(&mut *transaction)
            .await?
        } else {
            Vec::new()
        };
        let policy_status_ids =
            timeline_status_ids_for_accounts(&mut transaction, &policy_account_ids).await?;
        let policy_before = status_timeline_snapshots(&mut transaction, &policy_status_ids).await?;
        let (domain_block_id, created_at, updated_at, action) = if let Some((
            id,
            _,
            _,
            _,
            _,
            created_at,
        )) = exact_existing
        {
            if severity != 1 {
                clear_domain_owned_account_restrictions(&mut transaction, &domain, created_at)
                    .await?;
            }
            let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
                "UPDATE domain_blocks SET severity = $2, reject_media = $3, reject_reports = $4, \
                    updated_at = clock_timestamp() WHERE id = $1 RETURNING updated_at",
            )
            .bind(id)
            .bind(severity)
            .bind(reject_media)
            .bind(reject_reports)
            .fetch_one(&mut *transaction)
            .await?;
            (id, created_at, updated_at, "update")
        } else {
            let (id, created_at, updated_at) = sqlx::query_as::<_, (i64, NaiveDateTime, NaiveDateTime)>(
                "INSERT INTO domain_blocks (domain, severity, reject_media, reject_reports, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, clock_timestamp(), clock_timestamp()) RETURNING id, created_at, updated_at",
            )
            .bind(&domain)
            .bind(severity)
            .bind(reject_media)
            .bind(reject_reports)
            .fetch_one(&mut *transaction)
            .await?;
            (id, created_at, updated_at, "create")
        };
        apply_domain_account_restrictions(&mut transaction, &domain, severity, created_at).await?;
        if !policy_status_ids.is_empty() {
            let policy_after =
                status_timeline_snapshots(&mut transaction, &policy_status_ids).await?;
            collect_timeline_snapshot_transitions(
                &mut transaction,
                &mut pending_stream_events,
                "status.update",
                &format!("domain-block:{domain_block_id}"),
                updated_at.and_utc().timestamp_micros(),
                &policy_before,
                &policy_after,
            )
            .await?;
        }
        let severance_event_id = if severity == 1 {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO relationship_severance_events
                         (type, target_name, purged, created_at, updated_at)
                     VALUES (0, $1, false, clock_timestamp(), clock_timestamp())
                     RETURNING id",
                )
                .bind(&domain)
                .fetch_one(&mut *transaction)
                .await?,
            )
        } else {
            None
        };
        if severity == 1 || reject_media {
            record_outbox_in(
                &mut transaction,
                &domain_block_job(domain_block_id, severance_event_id, updated_at, origin),
            )
            .await?;
        }
        insert_admin_action_log(
            &mut transaction,
            acting_account_id,
            action,
            domain_block_id,
            "DomainBlock",
            domain.clone(),
            None,
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(domain_block_id)
    }

    /// Queues a full purge of remote accounts and custom emoji for a domain.
    ///
    /// The request is durable and idempotent by domain. The maintenance worker performs the
    /// destructive work separately so the administrative command does not hold a public-schema
    /// transaction while removing potentially large remote datasets.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account lacks `manage_federation`,
    /// [`WriteError::InvalidInput`] for an invalid domain, or a database error when the outbox and
    /// audit record cannot commit atomically.
    pub async fn request_domain_purge(
        &self,
        acting_account_id: i64,
        domain: &str,
    ) -> Result<(), WriteError> {
        let domain = normalize_domain_block_domain(domain)?;
        let mut transaction = self.pool.begin().await?;
        if authorized_admin_account(&mut transaction, acting_account_id, 1_i64 << 5)
            .await?
            .is_none()
        {
            return Err(WriteError::Unauthorized);
        }
        lock_domain_scope(&mut transaction, &domain).await?;
        record_outbox_in(&mut transaction, &domain_purge_job(&domain)).await?;
        insert_instance_admin_action_log(&mut transaction, acting_account_id, &domain).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Permanently removes remote accounts and custom emoji for one domain.
    ///
    /// This is the worker-side equivalent of Rails `PurgeDomainService`: it does not retain remote
    /// account tombstones or emit `ActivityPub` side effects. The domain lock and idempotent deletes
    /// make a retried maintenance job safe.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::InvalidInput`] for an invalid domain or a database error when cleanup
    /// cannot commit atomically.
    pub(crate) async fn process_domain_purge_job(&self, domain: &str) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        sqlx::query(
            "UPDATE relationship_severance_events
                 SET purged = true, updated_at = clock_timestamp()
               WHERE type IN (0, 1) AND target_name = $1",
        )
        .bind(domain)
        .execute(&mut *transaction)
        .await?;
        let account_ids = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM accounts
               WHERE domain IS NOT NULL AND lower(domain) = lower($1)
               ORDER BY id FOR UPDATE",
        )
        .bind(domain)
        .fetch_all(&mut *transaction)
        .await?;
        for account_id in account_ids {
            purge_remote_account(&mut transaction, &mut pending_stream_events, account_id).await?;
        }
        sqlx::query(
            "DELETE FROM custom_emojis WHERE domain IS NOT NULL AND lower(domain) = lower($1)",
        )
        .bind(domain)
        .execute(&mut *transaction)
        .await?;
        flush_staged_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        sqlx::query("SELECT public.rustodon_refresh_instances()")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Purges remote accounts newly suspended by a global suspend-level domain block.
    ///
    /// # Errors
    ///
    /// Returns a database error when the block or any account purge cannot be read or committed.
    pub(crate) async fn process_domain_block_job(
        &self,
        domain_block_id: i64,
        severance_event_id: Option<i64>,
        origin: &str,
    ) -> Result<(), WriteError> {
        if origin.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "domain block processing requires the instance origin",
            ));
        }
        let domain =
            sqlx::query_scalar::<_, String>("SELECT domain FROM domain_blocks WHERE id = $1")
                .bind(domain_block_id)
                .fetch_optional(&self.pool)
                .await?;
        if let Some(domain) = domain {
            return self
                .with_domain_lock(&domain, || async {
                    self.process_domain_block_job_locked(
                        domain_block_id,
                        severance_event_id,
                        origin,
                    )
                    .await
                })
                .await;
        }
        self.process_domain_block_job_locked(domain_block_id, severance_event_id, origin)
            .await
    }

    pub(crate) async fn domain_block_domain(
        &self,
        domain_block_id: i64,
    ) -> Result<Option<String>, WriteError> {
        Ok(
            sqlx::query_scalar::<_, String>("SELECT domain FROM domain_blocks WHERE id = $1")
                .bind(domain_block_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub(crate) async fn process_domain_block_job_locked(
        &self,
        domain_block_id: i64,
        severance_event_id: Option<i64>,
        origin: &str,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let Some((domain, severity, reject_media, created_at)) =
            sqlx::query_as::<_, (String, Option<i32>, bool, NaiveDateTime)>(
                "SELECT domain, severity, reject_media, created_at
                   FROM domain_blocks WHERE id = $1 FOR UPDATE",
            )
            .bind(domain_block_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            if let Some(severance_event_id) = severance_event_id {
                sqlx::query(
                    "UPDATE relationship_severance_events
                        SET purged = true, updated_at = clock_timestamp()
                      WHERE id = $1
                        AND NOT EXISTS (
                            SELECT 1 FROM severed_relationships
                             WHERE relationship_severance_event_id = $1
                        )",
                )
                .bind(severance_event_id)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await?;
            return Ok(());
        };
        if severity != Some(1) {
            if reject_media {
                clear_domain_media(&mut transaction, &domain).await?;
            }
            if let Some(severance_event_id) = severance_event_id {
                sqlx::query(
                    "UPDATE relationship_severance_events
                        SET purged = true, updated_at = clock_timestamp()
                      WHERE id = $1",
                )
                .bind(severance_event_id)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await?;
            return Ok(());
        }
        let Some(severance_event_id) = severance_event_id else {
            return Err(WriteError::InvalidInput(
                "suspend domain blocks require a severance event",
            ));
        };
        let domain = domain_policy_hostname(&domain);
        let accounts = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, uri FROM accounts
              WHERE domain IS NOT NULL
                AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                         THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1)
                  OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                         THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1))
                AND suspended_at = $2
              ORDER BY id",
        )
        .bind(&domain)
        .bind(created_at)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        for (account_id, actor_uri) in accounts {
            if !actor_uri.is_empty() {
                self.apply_remote_actor_delete(
                    account_id,
                    &actor_uri,
                    origin,
                    Some(severance_event_id),
                    Some(created_at),
                )
                .await?;
            }
        }
        let mut cleanup_transaction = self.pool.begin().await?;
        clear_domain_media(&mut cleanup_transaction, &domain).await?;
        cleanup_transaction.commit().await?;
        for (account_id, event_id) in self
            .create_domain_severance_events(severance_event_id)
            .await?
        {
            self.create_notification(NotificationCreate {
                recipient_account_id: account_id,
                activity: NotificationActivity::SeveredRelationships { id: event_id },
                silenced: false,
            })
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn domain_purge_media_metadata(
        &self,
        domain: &str,
    ) -> Result<Vec<PaperclipMetadata>, WriteError> {
        let domain = normalize_domain_block_domain(domain)?;
        domain_media_metadata(&self.pool, &domain, false).await
    }

    pub(crate) async fn remote_actor_media_metadata(
        &self,
        account_id: i64,
        actor_uri: &str,
    ) -> Result<Vec<PaperclipMetadata>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let Some((domain, current_uri)) = sqlx::query_as::<_, (Option<String>, String)>(
            "SELECT domain, uri FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(Vec::new());
        };
        if domain.is_none() || current_uri != actor_uri {
            return Ok(Vec::new());
        }
        let metadata =
            account_media_metadata_for_cleanup(&mut transaction, account_id, true, &[]).await?;
        transaction.commit().await?;
        Ok(metadata)
    }

    pub(crate) async fn domain_block_media_metadata(
        &self,
        domain_block_id: i64,
    ) -> Result<Vec<PaperclipMetadata>, WriteError> {
        let Some((domain, severity, reject_media)) =
            sqlx::query_as::<_, (String, Option<i32>, bool)>(
                "SELECT domain, severity, reject_media FROM domain_blocks WHERE id = $1",
            )
            .bind(domain_block_id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(Vec::new());
        };
        if severity != Some(1) && !reject_media {
            return Ok(Vec::new());
        }
        domain_media_metadata(&self.pool, &domain_policy_hostname(&domain), true).await
    }

    pub(crate) async fn remote_note_media_metadata(
        &self,
        account_id: i64,
        actor_uri: &str,
        object_uri: &str,
        atom_uri: Option<&str>,
    ) -> Result<Vec<PaperclipMetadata>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_remote_note(&mut transaction, object_uri).await?;
        let account = sqlx::query_as::<_, (Option<String>, String)>(
            "SELECT domain, uri FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((domain, current_actor_uri)) = account else {
            return Ok(Vec::new());
        };
        if domain.is_none() || current_actor_uri != actor_uri {
            return Ok(Vec::new());
        }
        if !same_remote_note_host(actor_uri, object_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Delete URI does not match its actor host",
            ));
        }
        if let Some(atom_uri) = atom_uri
            && !same_remote_note_host(actor_uri, atom_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote Delete atom URI does not match its actor host",
            ));
        }
        let Some(status_id) =
            remote_note_status_id_for_account(&mut transaction, account_id, object_uri, atom_uri)
                .await?
        else {
            transaction.commit().await?;
            return Ok(Vec::new());
        };
        let metadata = sqlx::query_as::<
            _,
            (
                i64,
                Option<i32>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<i32>,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            "SELECT id, file_storage_schema_version, file_file_name, file_content_type,
                    remote_url, thumbnail_storage_schema_version, thumbnail_file_name,
                    thumbnail_content_type, thumbnail_remote_url
               FROM media_attachments WHERE status_id = $1 ORDER BY id",
        )
        .bind(status_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let mut metadata_result = Vec::new();
        for (
            media_id,
            file_storage_schema_version,
            file_file_name,
            file_content_type,
            remote_url,
            thumbnail_storage_schema_version,
            thumbnail_file_name,
            thumbnail_content_type,
            thumbnail_remote_url,
        ) in metadata
        {
            append_media_attachment_metadata(
                &mut metadata_result,
                media_id,
                file_storage_schema_version,
                file_file_name,
                file_content_type,
                remote_url,
                thumbnail_storage_schema_version,
                thumbnail_file_name,
                thumbnail_content_type,
                thumbnail_remote_url,
            );
        }
        Ok(metadata_result)
    }

    async fn create_domain_severance_events(
        &self,
        severance_event_id: i64,
    ) -> Result<Vec<(i64, i64)>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let events = sqlx::query_as::<_, (i64, i64)>(
            "INSERT INTO account_relationship_severance_events
                 (account_id, relationship_severance_event_id, followers_count,
                  following_count, created_at, updated_at)
             SELECT local_account_id, $1,
                    count(*) FILTER (WHERE direction = 0)::integer,
                    count(*) FILTER (WHERE direction = 1)::integer,
                    clock_timestamp(), clock_timestamp()
               FROM severed_relationships
              WHERE relationship_severance_event_id = $1
              GROUP BY local_account_id
             ON CONFLICT (account_id, relationship_severance_event_id) DO UPDATE
                 SET followers_count = EXCLUDED.followers_count,
                     following_count = EXCLUDED.following_count,
                     updated_at = clock_timestamp()
             RETURNING account_id, id",
        )
        .bind(severance_event_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(events)
    }

    /// Removes a global domain block for an authorized federation moderator.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account lacks `manage_federation`,
    /// [`WriteError::NotFound`] when the domain has no block, or a database error when the block
    /// and audit record cannot commit atomically.
    pub async fn unblock_domain(
        &self,
        acting_account_id: i64,
        domain: &str,
    ) -> Result<(), WriteError> {
        let domain = normalize_domain_block_domain(domain)?;
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        if authorized_admin_account(&mut transaction, acting_account_id, 1_i64 << 5)
            .await?
            .is_none()
        {
            return Err(WriteError::Unauthorized);
        }
        lock_domain_scope(&mut transaction, &domain).await?;
        let (domain_block_id, created_at) = sqlx::query_as::<_, (i64, NaiveDateTime)>(
            "SELECT id, created_at FROM domain_blocks \
             WHERE lower(domain) = lower($1) FOR UPDATE",
        )
        .bind(&domain)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let policy_domain = domain_policy_hostname(&domain);
        let account_ids = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM accounts WHERE domain IS NOT NULL \
               AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                        THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
                 OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                        THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
               AND (silenced_at = $2 OR suspended_at = $2) ORDER BY id",
        )
        .bind(&policy_domain)
        .bind(created_at)
        .fetch_all(&mut *transaction)
        .await?;
        let status_ids = timeline_status_ids_for_accounts(&mut transaction, &account_ids).await?;
        let before = status_timeline_snapshots(&mut transaction, &status_ids).await?;
        clear_domain_owned_account_restrictions(&mut transaction, &domain, created_at).await?;
        let after = status_timeline_snapshots(&mut transaction, &status_ids).await?;
        let transition_at =
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut *transaction)
                .await?;
        collect_timeline_snapshot_transitions(
            &mut transaction,
            &mut pending_stream_events,
            "status.update",
            &format!("domain-unblock:{domain_block_id}"),
            transition_at.and_utc().timestamp_micros(),
            &before,
            &after,
        )
        .await?;
        sqlx::query("DELETE FROM domain_blocks WHERE id = $1")
            .bind(domain_block_id)
            .execute(&mut *transaction)
            .await?;
        insert_admin_action_log(
            &mut transaction,
            acting_account_id,
            "destroy",
            domain_block_id,
            "DomainBlock",
            domain,
            None,
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Inserts reconciled counter rows for every account whose stats row is absent.
    ///
    /// Existing stats rows remain authoritative and are never rewritten. Repairs at
    /// most one account on the startup critical path and reports whether
    /// a background pass is needed. Each transaction has short lock and statement
    /// deadlines; startup invokes only a fixed number of background batches.
    pub async fn repair_missing_account_stats_startup_batch(
        &self,
    ) -> Result<(u64, bool), WriteError> {
        const BATCH_SIZE: usize = 1;
        const QUERY_LIMIT: i64 = 2;
        let mut account_ids = sqlx::query_scalar::<_, i64>(
            "SELECT account.id FROM accounts account \
             LEFT JOIN account_stats stats ON stats.account_id = account.id \
             WHERE stats.id IS NULL \
             ORDER BY (account.domain IS NOT NULL), account.id LIMIT $1",
        )
        .bind(QUERY_LIMIT)
        .fetch_all(&self.pool)
        .await?;
        let may_have_more = account_ids.len() > BATCH_SIZE;
        account_ids.truncate(BATCH_SIZE);
        let repaired = self
            .repair_account_stats_for_accounts(&account_ids, true)
            .await?;
        Ok((repaired, may_have_more))
    }

    pub async fn repair_missing_account_stats(&self) -> Result<u64, WriteError> {
        const BATCH_SIZE: i64 = 100;
        let mut after_account_id = None;
        let mut repaired = 0_u64;
        loop {
            let account_ids = sqlx::query_scalar::<_, i64>(
                "SELECT account.id FROM accounts account \
                 LEFT JOIN account_stats stats ON stats.account_id = account.id \
                 WHERE stats.id IS NULL AND ($1::bigint IS NULL OR account.id > $1) \
                 ORDER BY account.id LIMIT $2",
            )
            .bind(after_account_id)
            .bind(BATCH_SIZE)
            .fetch_all(&self.pool)
            .await?;
            let Some(last_account_id) = account_ids.last().copied() else {
                break;
            };
            repaired += self
                .repair_account_stats_for_accounts(&account_ids, false)
                .await?;
            after_account_id = Some(last_account_id);
        }
        Ok(repaired)
    }

    async fn repair_account_stats_for_accounts(
        &self,
        account_ids: &[i64],
        startup_deadline: bool,
    ) -> Result<u64, WriteError> {
        let account_ids = sqlx::query_scalar::<_, i64>(
            "SELECT account.id FROM accounts account \
             LEFT JOIN account_stats stats ON stats.account_id = account.id \
             WHERE account.id = ANY($1) AND stats.id IS NULL ORDER BY account.id",
        )
        .bind(account_ids)
        .fetch_all(&self.pool)
        .await?;
        let mut repaired = 0_u64;
        for account_id in account_ids {
            // Keep each counter row lock in its own short transaction. Locking the
            // account first matches ordinary account writes and prevents an FK /
            // unique-key lock inversion with a concurrent initializer.
            let mut transaction = self.pool.begin().await?;
            if startup_deadline {
                sqlx::query("SET LOCAL lock_timeout = '1s'")
                    .execute(&mut *transaction)
                    .await?;
                sqlx::query("SET LOCAL statement_timeout = '2s'")
                    .execute(&mut *transaction)
                    .await?;
            }
            let account_exists =
                sqlx::query_scalar::<_, i64>("SELECT id FROM accounts WHERE id = $1 FOR UPDATE")
                    .bind(account_id)
                    .fetch_optional(&mut *transaction)
                    .await?
                    .is_some();
            if !account_exists {
                transaction.rollback().await?;
                continue;
            }
            repaired += u64::from(
                initialize_account_stats_if_missing(&mut transaction, account_id)
                    .await?
                    .inserted,
            );
            transaction.commit().await?;
        }
        Ok(repaired)
    }

    /// Recomputes an account's denormalized status and relationship counters.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account lacks effective
    /// `manage_users` permission, [`WriteError::NotFound`] for an unknown target account, or a
    /// database error when the repair and audit record cannot commit atomically.
    pub async fn reconcile_account_stats(
        &self,
        acting_account_id: i64,
        account_id: i64,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let can_manage_users = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                 SELECT 1 FROM users account_user \
                 JOIN accounts account ON account.id = account_user.account_id \
                 JOIN user_roles role ON role.id = COALESCE(account_user.role_id, -99) \
                 LEFT JOIN user_roles everyone ON everyone.id = -99 \
                 WHERE account.id = $1 AND account.domain IS NULL \
                   AND account.suspended_at IS NULL \
                   AND account_user.confirmed_at IS NOT NULL \
                   AND account_user.approved = true \
                   AND account_user.disabled = false \
                   AND (role.permissions & 1 <> 0 OR \
                        ((role.permissions | COALESCE(everyone.permissions, 0)) & $2 <> 0)) \
             )",
        )
        .bind(acting_account_id)
        .bind(1_i64 << 10)
        .fetch_one(&mut *transaction)
        .await?;
        if !can_manage_users {
            return Err(WriteError::Unauthorized);
        }

        let (account_username, account_domain) = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT username, domain FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let account_human_identifier = account_domain.as_deref().map_or_else(
            || account_username.clone(),
            |domain| format!("{account_username}@{domain}"),
        );

        let counts = account_stats_snapshot(&mut transaction, account_id).await?;
        sqlx::query(
            "UPDATE status_stats stats SET replies_count = ( \
                 SELECT count(*) FROM statuses reply \
                 WHERE reply.in_reply_to_id = stats.status_id AND reply.deleted_at IS NULL), \
                 updated_at = clock_timestamp() \
              FROM statuses status \
              WHERE status.id = stats.status_id AND status.account_id = $1",
        )
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO account_stats (account_id, followers_count, following_count, last_status_at, statuses_count, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, clock_timestamp(), clock_timestamp()) \
             ON CONFLICT (account_id) DO UPDATE SET \
                 followers_count = EXCLUDED.followers_count,\
                 following_count = EXCLUDED.following_count,\
                 last_status_at = EXCLUDED.last_status_at,\
                 statuses_count = EXCLUDED.statuses_count,\
                 updated_at = clock_timestamp()",
        )
        .bind(account_id)
        .bind(counts.2)
        .bind(counts.1)
        .bind(counts.3)
        .bind(counts.0)
        .execute(&mut *transaction)
        .await?;
        insert_admin_action_log(
            &mut transaction,
            acting_account_id,
            "reconcile_account_stats",
            account_id,
            "Account",
            account_human_identifier,
            None,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn update_marker(
        &self,
        authenticated: &AuthenticatedBearer,
        timeline: &str,
        last_read_id: i64,
        expected_lock_version: Option<i32>,
    ) -> Result<Marker, WriteError> {
        let outcome = self
            .update_marker_with_options(
                authenticated,
                timeline,
                last_read_id,
                expected_lock_version,
                WriteOptions::default(),
            )
            .await?;
        Ok(match outcome {
            WriteOutcome::Applied(marker) | WriteOutcome::Replayed(marker) => marker,
        })
    }

    pub async fn update_markers(
        &self,
        authenticated: &AuthenticatedBearer,
        updates: &[(String, Option<i64>)],
    ) -> Result<Vec<Marker>, WriteError> {
        let (_, mut transaction) = self
            .begin_account_write(authenticated, WRITE_STATUSES)
            .await?;
        let user_id = authenticated
            .require_user()
            .map_err(|_| WriteError::Unauthorized)?
            .user_id();
        for (timeline, _) in updates {
            if !matches!(timeline.as_str(), "home" | "notifications") {
                return Err(WriteError::InvalidInput("unknown marker timeline"));
            }
        }
        let mut markers = Vec::with_capacity(updates.len());
        for (timeline, last_read_id) in updates {
            let marker = match last_read_id {
                Some(last_read_id) => {
                    update_marker_in(&mut transaction, user_id, timeline, *last_read_id, None)
                        .await?
                }
                None => {
                    if let Some(marker) = select_marker(&mut transaction, user_id, timeline).await?
                    {
                        marker
                    } else {
                        update_marker_in(&mut transaction, user_id, timeline, 0, None).await?
                    }
                }
            };
            markers.push(marker);
        }
        transaction.commit().await?;
        Ok(markers)
    }

    #[allow(clippy::too_many_lines)]
    pub async fn update_account_profile(
        &self,
        authenticated: &AuthenticatedBearer,
        update: &AccountProfileUpdate,
    ) -> Result<(), WriteError> {
        let account_id = write_account(authenticated, WRITE_ACCOUNTS)?;
        self.with_account_lock(account_id, || async {
            self.update_account_profile_locked(authenticated, update)
                .await
        })
        .await
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn update_account_profile_locked(
        &self,
        authenticated: &AuthenticatedBearer,
        update: &AccountProfileUpdate,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_ACCOUNTS)
            .await?;
        let user_id = authenticated
            .require_user()
            .map(super::oauth::OAuthResourceOwner::user_id)
            .map_err(|_| WriteError::Unauthorized)?;
        validate_account_profile_update(update)?;
        let current_note =
            sqlx::query_scalar::<_, String>("SELECT note FROM accounts WHERE id = $1 FOR UPDATE")
                .bind(account_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(WriteError::NotFound)?;
        let fields = match &update.fields {
            Some(fields) => Some(account_fields_json(&mut transaction, account_id, fields).await?),
            None => None,
        };
        let attribution_domains = update
            .attribution_domains
            .as_deref()
            .map(normalize_attribution_domains);
        let actor_type = match update.bot {
            AccountProfileValue::Unchanged => None,
            AccountProfileValue::Null | AccountProfileValue::Value(false) => Some("Person"),
            AccountProfileValue::Value(true) => Some("Service"),
        };
        let (discoverable_set, discoverable) = match update.discoverable {
            AccountProfileValue::Unchanged => (false, None),
            AccountProfileValue::Null => (true, None),
            AccountProfileValue::Value(value) => (true, Some(value)),
        };
        let (hide_collections_set, hide_collections) = match update.hide_collections {
            AccountProfileValue::Unchanged => (false, None),
            AccountProfileValue::Null => (true, None),
            AccountProfileValue::Value(value) => (true, Some(value)),
        };
        let has_account_update = update.display_name.is_some()
            || update.note.is_some()
            || update.avatar_description.is_some()
            || update.header_description.is_some()
            || !matches!(update.avatar, AccountMediaUpdate::Unchanged)
            || !matches!(update.header, AccountMediaUpdate::Unchanged)
            || !matches!(update.bot, AccountProfileValue::Unchanged)
            || update.locked.is_some()
            || !matches!(update.discoverable, AccountProfileValue::Unchanged)
            || !matches!(update.hide_collections, AccountProfileValue::Unchanged)
            || update.indexable.is_some()
            || update.attribution_domains.is_some()
            || update.fields.is_some();
        let account_updated_at = if has_account_update {
            Some(
                sqlx::query_scalar::<_, NaiveDateTime>(
                "UPDATE accounts SET \
                   display_name = COALESCE($2, display_name), \
                   note = COALESCE($3, note), \
                   actor_type = COALESCE($4, actor_type), \
                   locked = COALESCE($5, locked), \
                   discoverable = CASE WHEN $6 THEN $7::boolean ELSE discoverable END, \
                   hide_collections = CASE WHEN $8 THEN $9::boolean ELSE hide_collections END, \
                   indexable = COALESCE($10, indexable), \
                   fields = COALESCE($11::jsonb, fields), \
                   attribution_domains = COALESCE($12::text[], attribution_domains), \
                   avatar_content_type = CASE WHEN $13 THEN $14::varchar ELSE avatar_content_type END, \
                   avatar_file_name = CASE WHEN $13 THEN $15::varchar ELSE avatar_file_name END, \
                   avatar_file_size = CASE WHEN $13 THEN $16::integer ELSE avatar_file_size END, \
                   avatar_remote_url = CASE WHEN $13 THEN NULL::varchar ELSE avatar_remote_url END, \
                   avatar_storage_schema_version = CASE WHEN $13 THEN $17::integer ELSE avatar_storage_schema_version END, \
                   avatar_updated_at = CASE WHEN $13 THEN CASE WHEN $14::varchar IS NULL THEN NULL::timestamp ELSE clock_timestamp() END ELSE avatar_updated_at END, \
                   avatar_description = COALESCE($18, avatar_description), \
                   header_content_type = CASE WHEN $19 THEN $20::varchar ELSE header_content_type END, \
                   header_file_name = CASE WHEN $19 THEN $21::varchar ELSE header_file_name END, \
                   header_file_size = CASE WHEN $19 THEN $22::integer ELSE header_file_size END, \
                   header_remote_url = CASE WHEN $19 THEN '' ELSE header_remote_url END, \
                   header_storage_schema_version = CASE WHEN $19 THEN $23::integer ELSE header_storage_schema_version END, \
                   header_updated_at = CASE WHEN $19 THEN CASE WHEN $20::varchar IS NULL THEN NULL::timestamp ELSE clock_timestamp() END ELSE header_updated_at END, \
                   header_description = COALESCE($24, header_description), \
                   updated_at = clock_timestamp() \
                  WHERE id = $1 RETURNING updated_at",
                )
             .bind(account_id)
            .bind(update.display_name.as_deref())
            .bind(update.note.as_deref())
            .bind(actor_type)
            .bind(update.locked)
            .bind(discoverable_set)
            .bind(discoverable)
            .bind(hide_collections_set)
            .bind(hide_collections)
            .bind(update.indexable)
            .bind(fields)
            .bind(attribution_domains)
            .bind(!matches!(update.avatar, AccountMediaUpdate::Unchanged))
            .bind(account_media_content_type(&update.avatar))
            .bind(account_media_file_name(&update.avatar))
            .bind(account_media_file_size(&update.avatar))
            .bind(account_media_storage_schema_version(&update.avatar))
            .bind(update.avatar_description.as_deref())
            .bind(!matches!(update.header, AccountMediaUpdate::Unchanged))
            .bind(account_media_content_type(&update.header))
            .bind(account_media_file_name(&update.header))
            .bind(account_media_file_size(&update.header))
            .bind(account_media_storage_schema_version(&update.header))
                .bind(update.header_description.as_deref())
                .fetch_one(&mut *transaction)
                .await?,
            )
        } else {
            None
        };
        let note = update.note.as_deref().unwrap_or(&current_note);
        update_account_tags(&mut transaction, account_id, note).await?;
        if let Some(source) = &update.source {
            update_user_settings(&mut transaction, account_id, user_id, source).await?;
        }
        if let Some(updated_at) = account_updated_at {
            let update_job = JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
                json!({
                    "account_id": account_id,
                    "updated_at_micros": updated_at.and_utc().timestamp_micros()
                }),
            )
            .logical_key(format!(
                "activitypub:account:{account_id}:update:{}",
                updated_at.and_utc().timestamp_micros()
            ));
            record_outbox_in(&mut transaction, &update_job).await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn remote_note_is_relevant(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let note = RemoteNoteData::parse(object, actor_uri)?;
        if delivery_target_account_id.is_some() {
            return Ok(true);
        }
        let mut transaction = self.pool.begin().await?;
        let followers_url = sqlx::query_scalar::<_, String>(
            "SELECT followers_url FROM accounts WHERE id = $1 AND domain IS NOT NULL",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let addressed = {
            let audience = note.audience.to.iter().chain(&note.audience.cc);
            let mut addressed = false;
            for uri in audience {
                if local_activitypub_account_id(&mut transaction, uri, origin)
                    .await?
                    .is_some()
                {
                    addressed = true;
                    break;
                }
            }
            addressed
        };
        let followed = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM follows follow
                JOIN accounts local_account ON local_account.id = follow.account_id
                                             AND local_account.domain IS NULL
                WHERE follow.target_account_id = $1)",
        )
        .bind(account_id)
        .fetch_one(&mut *transaction)
        .await?;
        let (_, parent_account_id, _) = remote_note_thread(&mut transaction, &note, origin).await?;
        let parent_relevant = if let Some(parent_account_id) = parent_account_id {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                    SELECT 1 FROM accounts parent
                    WHERE parent.id = $1 AND parent.domain IS NULL
                ) OR EXISTS (
                    SELECT 1 FROM follows follow
                    JOIN accounts local_account ON local_account.id = follow.account_id
                                                 AND local_account.domain IS NULL
                    WHERE follow.target_account_id = $1)",
            )
            .bind(parent_account_id)
            .fetch_one(&mut *transaction)
            .await?
        } else {
            false
        };
        let visibility = remote_note_visibility(&note.audience, &followers_url);
        let relevant = match visibility {
            0 | 1 => addressed || followed || parent_relevant,
            2 => addressed || followed,
            _ => addressed,
        };
        transaction.commit().await?;
        Ok(relevant)
    }

    pub(crate) async fn remote_quote_target_is_local(
        &self,
        target_uri: &str,
        origin: &str,
    ) -> Result<Option<bool>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let target = resolve_quote_target(&mut transaction, target_uri, origin)
            .await?
            .map(|(_, _, local, _)| local);
        transaction.commit().await?;
        Ok(target)
    }

    pub(crate) async fn remote_domain_allowed_in_transaction(
        transaction: &mut Transaction<'_, Postgres>,
        domain: &str,
        limited_federation: bool,
    ) -> Result<bool, WriteError> {
        remote_domain_allowed_in_transaction(transaction, domain, limited_federation).await
    }

    pub(crate) async fn remote_quote_target_matches_status(
        &self,
        status_id: i64,
        target_uri: &str,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let matches =
            quote_target_matches_uri(&mut transaction, status_id, target_uri, origin).await?;
        transaction.commit().await?;
        Ok(matches)
    }

    pub(crate) async fn remote_note_exists(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
    ) -> Result<bool, WriteError> {
        let note = RemoteNoteData::parse(object, actor_uri)?;
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM statuses
                WHERE account_id = $1
                  AND (uri = $2 OR ($3::text IS NOT NULL AND uri = $3)))",
        )
        .bind(account_id)
        .bind(&note.uri)
        .bind(&note.atom_uri)
        .fetch_one(&self.pool)
        .await?)
    }

    pub(crate) async fn remote_note_reference_is_resolved(
        &self,
        account_id: i64,
        actor_uri: &str,
        object_uri: &str,
    ) -> Result<bool, WriteError> {
        if !same_remote_note_host(actor_uri, object_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Note URI does not match its actor host",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        lock_remote_note(&mut transaction, object_uri).await?;
        let actor_matches = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM accounts
                WHERE id = $1 AND domain IS NOT NULL AND uri = $2)",
        )
        .bind(account_id)
        .bind(actor_uri)
        .fetch_one(&mut *transaction)
        .await?;
        if !actor_matches {
            transaction.commit().await?;
            return Ok(true);
        }
        let resolved = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM statuses WHERE account_id = $1 AND uri = $2
                UNION ALL
                SELECT 1 FROM tombstones WHERE account_id = $1 AND uri = $2)",
        )
        .bind(account_id)
        .bind(object_uri)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(resolved)
    }

    pub(crate) async fn ensure_remote_note_reference_delivery(
        &self,
        account_id: i64,
        actor_uri: &str,
        object_uri: &str,
        delivery_target_account_id: i64,
    ) -> Result<(), WriteError> {
        if !same_remote_note_host(actor_uri, object_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Note URI does not match its actor host",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        lock_remote_note(&mut transaction, object_uri).await?;
        let status_id = sqlx::query_scalar::<_, i64>(
            "SELECT status.id FROM statuses status
               JOIN accounts actor ON actor.id = status.account_id
                                  AND actor.id = $1 AND actor.uri = $2
                                  AND actor.domain IS NOT NULL
              WHERE status.uri = $3 AND status.deleted_at IS NULL
              ORDER BY status.id LIMIT 1",
        )
        .bind(account_id)
        .bind(actor_uri)
        .bind(object_uri)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(status_id) = status_id {
            ensure_remote_note_delivery_target(
                &mut transaction,
                status_id,
                delivery_target_account_id,
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn remote_announce_target_exists(
        &self,
        object_uri: &str,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let exists = announce_interaction_target(&mut transaction, object_uri, origin)
            .await?
            .is_some();
        transaction.commit().await?;
        Ok(exists)
    }

    pub(crate) async fn remote_announce_is_relevant(
        &self,
        account_id: i64,
        delivery_target_account_id: Option<i64>,
    ) -> Result<bool, WriteError> {
        if delivery_target_account_id.is_some() {
            return Ok(true);
        }
        let mut transaction = self.pool.begin().await?;
        let relevant = remote_announce_is_relevant(&mut transaction, account_id).await?;
        transaction.commit().await?;
        Ok(relevant)
    }

    pub(crate) async fn remote_announce_is_tombstoned(
        &self,
        account_id: i64,
        activity_uri: &str,
    ) -> Result<bool, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let tombstoned =
            remote_interaction_tombstoned(&mut transaction, account_id, activity_uri).await?;
        transaction.commit().await?;
        Ok(tombstoned)
    }

    pub(crate) async fn upsert_remote_actor(
        &self,
        username: &str,
        domain: &str,
        limited_federation: bool,
        actor: &RemoteActor,
    ) -> Result<i64, WriteError> {
        if username.trim().is_empty()
            || domain.trim().is_empty()
            || !actor.username.eq_ignore_ascii_case(username)
        {
            return Err(WriteError::InvalidInput("remote actor handle is invalid"));
        }
        self.with_remote_domain_locks(domain, || async {
            self.upsert_remote_actor_locked(username, domain, limited_federation, actor, None)
                .await
        })
        .await
    }

    /// Refresh only an existing, still-identical remote row; operator recovery must
    /// never create an account or replace its authentication keys.
    pub(crate) async fn refresh_remote_actor(
        &self,
        account_id: i64,
        username: &str,
        domain: &str,
        limited_federation: bool,
        actor: &RemoteActor,
    ) -> Result<i64, WriteError> {
        if username.trim().is_empty() || !actor.username.eq_ignore_ascii_case(username) {
            return Err(WriteError::Validation("remote refresh handle changed"));
        }
        self.with_remote_domain_locks(domain, || async {
            self.upsert_remote_actor_locked(
                username,
                domain,
                limited_federation,
                actor,
                Some(account_id),
            )
            .await
        })
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn upsert_remote_actor_locked(
        &self,
        username: &str,
        domain: &str,
        limited_federation: bool,
        actor: &RemoteActor,
        existing_id: Option<i64>,
    ) -> Result<i64, WriteError> {
        let mut transaction = self.pool.begin().await?;
        if !remote_domain_allowed_in_transaction(&mut transaction, domain, limited_federation)
            .await?
        {
            return Err(WriteError::Validation("remote actor domain is not allowed"));
        }
        let uri = actor.id.as_str();
        let profile_url = actor.profile_url.as_ref().map_or(uri, Url::as_str);

        // Mastodon does not make accounts.uri unique, so serialize remote writes by actor URI.
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock(
                pg_catalog.hashtextextended($1, 0)
             )",
        )
        .bind(format!("rustodon:actor:{uri}"))
        .execute(&mut *transaction)
        .await?;

        let uri_account_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM accounts
             WHERE uri = $1
             ORDER BY id
             LIMIT 1
             FOR UPDATE",
        )
        .bind(uri)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(expected_id) = existing_id {
            let same_remote: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = $1 AND domain = $2 AND uri = $3 AND lower(username) = lower($4))"
            ).bind(expected_id).bind(domain).bind(uri).bind(username).fetch_one(&mut *transaction).await?;
            if uri_account_id != Some(expected_id) || !same_remote {
                return Err(WriteError::Validation("remote refresh identity changed"));
            }
        }
        let handle_account = sqlx::query_as::<_, (i64, Option<String>)>(
            "SELECT id, uri FROM accounts
             WHERE lower(username) = lower($1) AND lower(domain) = lower($2)
             ORDER BY id
             LIMIT 1
             FOR UPDATE",
        )
        .bind(username)
        .bind(domain)
        .fetch_optional(&mut *transaction)
        .await?;
        let inserted_account_id = if uri_account_id.is_none() && handle_account.is_none() {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO accounts (
                        username, domain, actor_type, display_name, note, uri, url,
                        inbox_url, shared_inbox_url, protocol, public_key, last_webfingered_at,
                        created_at, updated_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 1, '',
                               clock_timestamp(), clock_timestamp(), clock_timestamp())
                     ON CONFLICT DO NOTHING
                  RETURNING id, created_at",
            )
            .bind(username)
            .bind(domain)
            .bind(&actor.actor_type)
            .bind(&actor.display_name)
            .bind(&actor.note)
            .bind(uri)
            .bind(profile_url)
            .bind(actor.inbox.as_str())
            .bind(actor.shared_inbox.as_ref().map_or("", Url::as_str))
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            None
        };
        let concurrent_handle_account = if inserted_account_id.is_none()
            && uri_account_id.is_none()
            && handle_account.is_none()
        {
            sqlx::query_as::<_, (i64, Option<String>)>(
                "SELECT id, uri FROM accounts
                 WHERE lower(username) = lower($1) AND lower(domain) = lower($2)
                 ORDER BY id
                 LIMIT 1
                 FOR UPDATE",
            )
            .bind(username)
            .bind(domain)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            None
        };
        let account_id = remote_actor_account_id(
            uri_account_id,
            handle_account.as_ref(),
            inserted_account_id,
            concurrent_handle_account,
            uri,
        )?;
        sqlx::query(
            "UPDATE accounts SET username = $2, domain = $3, actor_type = $4,
                display_name = $5, note = $6, uri = $7, url = $8, inbox_url = $9,
                 shared_inbox_url = $10, protocol = 1,
                 public_key = CASE WHEN $13 THEN public_key ELSE '' END,
                followers_url = COALESCE($11, followers_url),
                following_url = COALESCE($12, following_url),
                last_webfingered_at = clock_timestamp(),
                updated_at = clock_timestamp()
             WHERE id = $1",
        )
        .bind(account_id)
        .bind(username)
        .bind(domain)
        .bind(&actor.actor_type)
        .bind(&actor.display_name)
        .bind(&actor.note)
        .bind(uri)
        .bind(profile_url)
        .bind(actor.inbox.as_str())
        .bind(actor.shared_inbox.as_ref().map_or("", Url::as_str))
        .bind(actor.followers.as_ref().map(Url::as_str))
        .bind(actor.following.as_ref().map(Url::as_str))
        .bind(existing_id.is_some())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE accounts SET
                suspended_at = CASE
                    WHEN $2 THEN COALESCE(suspended_at, clock_timestamp())
                    WHEN suspension_origin = 1 THEN NULL
                    ELSE suspended_at
                END,
                suspension_origin = CASE
                    WHEN $2 AND suspension_origin IS DISTINCT FROM 0 THEN 1
                    WHEN NOT $2 AND suspension_origin = 1 THEN NULL
                    ELSE suspension_origin
                END,
                updated_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(account_id)
        .bind(actor.suspended)
        .execute(&mut *transaction)
        .await?;
        if existing_id.is_none() {
            reconcile_remote_actor_keypairs(&mut transaction, account_id, actor).await?;
        }
        super::profile_media::persist_images(
            &mut transaction,
            account_id,
            actor.avatar.as_ref(),
            actor.header.as_ref(),
            true,
        )
        .await?;
        ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        transaction.commit().await?;
        Ok(account_id)
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_actor_update(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
    ) -> Result<(), WriteError> {
        let object_uri = remote_actor_object_uri(object, "id")?
            .ok_or(WriteError::InvalidInput("remote actor update has no ID"))?;
        if object_uri != actor_uri {
            return Err(WriteError::InvalidInput(
                "remote actor update identity does not match its signer",
            ));
        }
        let username = remote_actor_text(object, "preferredUsername")?;
        let display_name = remote_actor_text(object, "name")?;
        let note = remote_actor_text(object, "summary")?;
        let url = remote_actor_object_uri(object, "url")?;
        let inbox_url = remote_actor_object_uri(object, "inbox")?;
        let outbox_url = remote_actor_object_uri(object, "outbox")?;
        let followers_url = remote_actor_object_uri(object, "followers")?;
        let following_url = remote_actor_object_uri(object, "following")?;
        let (shared_inbox_set, shared_inbox_url) = match object.get("endpoints") {
            None => (false, None),
            Some(Value::Object(endpoints)) => (
                true,
                remote_actor_object_uri(&Value::Object(endpoints.clone()), "sharedInbox")?,
            ),
            Some(_) => {
                return Err(WriteError::InvalidInput(
                    "remote actor endpoints are invalid",
                ));
            }
        };
        let actor_type = object
            .get("type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let locked = object
            .get("manuallyApprovesFollowers")
            .map(|value| {
                value.as_bool().ok_or(WriteError::InvalidInput(
                    "remote actor approval policy is invalid",
                ))
            })
            .transpose()?;
        let discoverable = remote_actor_bool(object, "discoverable")?;
        let indexable = remote_actor_bool(object, "indexable")?;
        let suspended = remote_actor_bool(object, "suspended")?.unwrap_or(false);
        let fields = remote_actor_fields(object)?;
        let also_known_as = remote_actor_aliases(object)?;
        let avatar_set = object.get("icon").is_some();
        let avatar_remote_url = super::activitypub_inbox::actor_image_uri(object.get("icon"))
            .map_err(|_| WriteError::InvalidInput("remote actor image URI is invalid"))?;
        let header_set = object.get("image").is_some();
        let header_remote_url = super::activitypub_inbox::actor_image_uri(object.get("image"))
            .map_err(|_| WriteError::InvalidInput("remote actor image URI is invalid"))?;

        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        let account =
            sqlx::query_as::<_, (Option<String>, String, Option<NaiveDateTime>, Option<i32>)>(
                "SELECT domain, uri, suspended_at, suspension_origin
               FROM accounts WHERE id = $1 FOR UPDATE",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?;
        let Some((domain, current_uri, suspended_at, suspension_origin)) = account else {
            return Ok(());
        };
        if domain.is_none()
            || current_uri != actor_uri
            || suspension_origin == Some(0)
            || (suspended_at.is_some() && suspension_origin != Some(1))
        {
            return Ok(());
        }
        let route_status_ids = if suspended == suspended_at.is_some() {
            Vec::new()
        } else {
            account_timeline_status_ids(&mut transaction, account_id).await?
        };
        let route_before = status_timeline_snapshots(&mut transaction, &route_status_ids).await?;
        let route_version = if route_status_ids.is_empty() {
            None
        } else {
            Some(
                sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                    .fetch_one(&mut *transaction)
                    .await?
                    .and_utc()
                    .timestamp_micros(),
            )
        };
        if suspended && let Some(version) = route_version {
            for status_id in &route_status_ids {
                collect_status_lifecycle_recipient_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    *status_id,
                    "delete",
                    StreamEventLogicalKey::Version(version),
                )
                .await?;
            }
        }
        upsert_remote_emojis(
            &mut transaction,
            domain.as_deref().expect("remote account has a domain"),
            actor_uri,
            object,
        )
        .await?;
        if let Some(note) = note.as_deref() {
            update_account_tags(&mut transaction, account_id, note).await?;
        }
        sqlx::query(
            "UPDATE accounts SET
                username = COALESCE($2, username),
                actor_type = COALESCE($3, actor_type),
                display_name = COALESCE($4, display_name),
                note = COALESCE($5, note),
                url = COALESCE($6, url),
                inbox_url = COALESCE($7, inbox_url),
                outbox_url = COALESCE($8, outbox_url),
                followers_url = COALESCE($9, followers_url),
                following_url = COALESCE($10, following_url),
                shared_inbox_url = CASE WHEN $11 THEN COALESCE($12, '') ELSE shared_inbox_url END,
                locked = COALESCE($13, locked),
                discoverable = COALESCE($14, discoverable),
                indexable = COALESCE($15, indexable),
                fields = COALESCE($16, fields),
                also_known_as = COALESCE($17, also_known_as),
                updated_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(account_id)
        .bind(username)
        .bind(actor_type)
        .bind(display_name)
        .bind(note)
        .bind(url)
        .bind(inbox_url)
        .bind(outbox_url)
        .bind(followers_url)
        .bind(following_url)
        .bind(shared_inbox_set)
        .bind(shared_inbox_url)
        .bind(locked)
        .bind(discoverable)
        .bind(indexable)
        .bind(fields)
        .bind(also_known_as)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE accounts SET
                suspended_at = CASE
                    WHEN $2 THEN COALESCE(suspended_at, clock_timestamp())
                    WHEN suspension_origin = 1 THEN NULL
                    ELSE suspended_at
                END,
                suspension_origin = CASE
                    WHEN $2 AND suspension_origin IS DISTINCT FROM 0 THEN 1
                    WHEN NOT $2 AND suspension_origin = 1 THEN NULL
                    ELSE suspension_origin
                END,
                updated_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(account_id)
        .bind(suspended)
        .execute(&mut *transaction)
        .await?;
        super::profile_media::persist_images(
            &mut transaction,
            account_id,
            avatar_set.then_some(&avatar_remote_url),
            header_set.then_some(&header_remote_url),
            false,
        )
        .await?;
        if let Some(route_version) = route_version {
            let route_after =
                status_timeline_snapshots(&mut transaction, &route_status_ids).await?;
            if !suspended {
                for status_id in &route_status_ids {
                    collect_status_lifecycle_recipient_stream_events(
                        &mut transaction,
                        &mut pending_stream_events,
                        *status_id,
                        "update",
                        StreamEventLogicalKey::Version(route_version),
                    )
                    .await?;
                }
            }
            collect_timeline_snapshot_transitions(
                &mut transaction,
                &mut pending_stream_events,
                "status.update",
                &format!("remote-actor:{account_id}"),
                route_version,
                &route_before,
                &route_after,
            )
            .await?;
        }
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_actor_delete(
        &self,
        account_id: i64,
        actor_uri: &str,
        origin: &str,
        severance_event_id: Option<i64>,
        expected_suspended_at: Option<NaiveDateTime>,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        let account = sqlx::query_as::<_, (Option<String>, String, Option<NaiveDateTime>)>(
            "SELECT domain, uri, suspended_at FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((domain, current_uri, suspended_at)) = account else {
            return Ok(());
        };
        if domain.is_none()
            || current_uri != actor_uri
            || expected_suspended_at.is_some_and(|expected| suspended_at != Some(expected))
        {
            return Ok(());
        }
        let route_status_ids = account_timeline_status_ids(&mut transaction, account_id).await?;
        let mut route_snapshots =
            status_timeline_snapshots(&mut transaction, &route_status_ids).await?;

        let follows = sqlx::query_as::<
            _,
            (
                i64,
                i64,
                i64,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<bool>,
                Option<bool>,
                Option<Vec<String>>,
            ),
        >(
            "SELECT follow.id, follow.account_id, follow.target_account_id, follow.uri,
                    source.domain, target.domain, follow.show_reblogs, follow.notify,
                    follow.languages
               FROM follows follow
               JOIN accounts source ON source.id = follow.account_id
               JOIN accounts target ON target.id = follow.target_account_id
              WHERE follow.account_id = $1 OR follow.target_account_id = $1
              ORDER BY follow.account_id, follow.target_account_id FOR UPDATE OF follow",
        )
        .bind(account_id)
        .fetch_all(&mut *transaction)
        .await?;
        for status_id in &route_status_ids {
            collect_status_lifecycle_recipient_stream_events(
                &mut transaction,
                &mut pending_stream_events,
                *status_id,
                "delete",
                StreamEventLogicalKey::Version(0),
            )
            .await?;
        }
        let requests = sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, target_account_id FROM follow_requests
              WHERE account_id = $1 OR target_account_id = $1
              ORDER BY account_id, target_account_id FOR UPDATE",
        )
        .bind(account_id)
        .fetch_all(&mut *transaction)
        .await?;
        for (
            follow_id,
            follow_account_id,
            follow_target_account_id,
            follow_uri,
            source_domain,
            target_domain,
            show_reblogs,
            notify,
            languages,
        ) in &follows
        {
            if let Some(severance_event_id) = severance_event_id
                && ((*follow_account_id == account_id && target_domain.is_none())
                    || (*follow_target_account_id == account_id && source_domain.is_none()))
            {
                let (local_account_id, direction) = if *follow_account_id == account_id {
                    (*follow_target_account_id, 0_i32)
                } else {
                    (*follow_account_id, 1_i32)
                };
                sqlx::query(
                    "INSERT INTO severed_relationships
                         (relationship_severance_event_id, local_account_id,
                          remote_account_id, direction, show_reblogs, notify, languages,
                          created_at, updated_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, clock_timestamp(), clock_timestamp())
                     ON CONFLICT (relationship_severance_event_id, local_account_id, direction, remote_account_id)
                     DO NOTHING",
                )
                .bind(severance_event_id)
                .bind(local_account_id)
                .bind(account_id)
                .bind(direction)
                .bind(show_reblogs)
                .bind(notify)
                .bind(languages)
                .execute(&mut *transaction)
                .await?;
            }
            let Some(follow_uri) = follow_uri.as_deref().filter(|uri| !uri.is_empty()) else {
                continue;
            };
            if *follow_account_id == account_id
                && target_domain.is_none()
                && let Some(remote_delivery) = remote_relationship_delivery(
                    &mut transaction,
                    *follow_target_account_id,
                    account_id,
                    origin,
                )
                .await?
            {
                record_remote_reject_delivery(
                    &mut transaction,
                    *follow_target_account_id,
                    &remote_delivery,
                    *follow_id,
                    follow_uri,
                )
                .await?;
            } else if *follow_target_account_id == account_id
                && source_domain.is_none()
                && let Some(remote_delivery) = remote_relationship_delivery(
                    &mut transaction,
                    *follow_account_id,
                    account_id,
                    origin,
                )
                .await?
            {
                cancel_activitypub_delivery(&mut transaction, follow_uri).await?;
                record_remote_undo_follow_delivery(
                    &mut transaction,
                    *follow_account_id,
                    &remote_delivery,
                    follow_uri,
                    origin,
                )
                .await?;
            }
        }
        sqlx::query("DELETE FROM follows WHERE account_id = $1 OR target_account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 OR target_account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        let mut relationship_deltas = HashMap::new();
        for (_, source_account_id, target_account_id, ..) in &follows {
            add_account_stats_delta(
                &mut relationship_deltas,
                *source_account_id,
                AccountStatsDelta {
                    following: -1,
                    ..AccountStatsDelta::default()
                },
            );
            add_account_stats_delta(
                &mut relationship_deltas,
                *target_account_id,
                AccountStatsDelta {
                    followers: -1,
                    ..AccountStatsDelta::default()
                },
            );
        }
        apply_account_stats_deltas(&mut transaction, relationship_deltas).await?;
        for (follow_id, _, target_account_id, ..) in &follows {
            delete_activity_notifications(
                &mut transaction,
                *target_account_id,
                *follow_id,
                "Follow",
            )
            .await?;
        }
        for (request_id, target_account_id) in requests {
            delete_activity_notifications(
                &mut transaction,
                target_account_id,
                request_id,
                "FollowRequest",
            )
            .await?;
        }
        let owned_statuses = sqlx::query_as::<_, (i64, i64, Option<i64>, Option<i64>, i32)>(
            "SELECT id, account_id, reblog_of_id, in_reply_to_id, visibility
               FROM statuses
              WHERE account_id = $1 AND deleted_at IS NULL
              ORDER BY id FOR UPDATE",
        )
        .bind(account_id)
        .fetch_all(&mut *transaction)
        .await?;
        let original_status_ids = owned_statuses
            .iter()
            .filter(|(_, _, reblog_of_id, _, _)| reblog_of_id.is_none())
            .map(|(status_id, _, _, _, _)| *status_id)
            .collect::<Vec<_>>();
        let dependent_reblogs = if original_status_ids.is_empty() {
            Vec::new()
        } else {
            sqlx::query_as::<_, (i64, i64, Option<i64>, Option<i64>, i32)>(
                "SELECT id, account_id, reblog_of_id, in_reply_to_id, visibility
                   FROM statuses
                  WHERE reblog_of_id = ANY($1) AND deleted_at IS NULL
                  ORDER BY id FOR UPDATE",
            )
            .bind(&original_status_ids)
            .fetch_all(&mut *transaction)
            .await?
        };
        let mut affected_statuses = owned_statuses;
        for status in dependent_reblogs {
            if !affected_statuses
                .iter()
                .any(|(status_id, _, _, _, _)| *status_id == status.0)
            {
                affected_statuses.push(status);
            }
        }
        let affected_status_ids = affected_statuses
            .iter()
            .map(|(status_id, _, _, _, _)| *status_id)
            .collect::<Vec<_>>();
        delete_remote_status_notifications(&mut transaction, &affected_status_ids).await?;
        remove_favourites_for_account_and_statuses(
            &mut transaction,
            account_id,
            &affected_status_ids,
        )
        .await?;
        remove_poll_data_for_account_and_statuses(
            &mut transaction,
            account_id,
            &affected_status_ids,
            &[],
        )
        .await?;
        for status_id in &affected_status_ids {
            if let Some(snapshot) = route_snapshots.remove(status_id) {
                collect_status_delete_stream_events_with_snapshot(
                    &mut transaction,
                    &mut pending_stream_events,
                    *status_id,
                    snapshot,
                )
                .await?;
            }
        }
        if !affected_status_ids.is_empty() {
            sqlx::query(
                "UPDATE statuses SET deleted_at = COALESCE(deleted_at, clock_timestamp()),
                    updated_at = clock_timestamp() WHERE id = ANY($1)",
            )
            .bind(&affected_status_ids)
            .execute(&mut *transaction)
            .await?;
        }
        let mut status_deltas = HashMap::new();
        for (_, status_account_id, reblog_of_id, in_reply_to_id, visibility) in &affected_statuses {
            if let Some(reblog_of_id) = reblog_of_id {
                decrement_reblog_count(&mut transaction, *reblog_of_id).await?;
                if *status_account_id != account_id && *visibility != 3 {
                    add_account_stats_delta(
                        &mut status_deltas,
                        *status_account_id,
                        AccountStatsDelta {
                            statuses: -1,
                            ..AccountStatsDelta::default()
                        },
                    );
                }
            } else if *status_account_id == account_id
                && *visibility < 2
                && let Some(in_reply_to_id) = in_reply_to_id
            {
                decrement_reply_count(&mut transaction, *in_reply_to_id).await?;
            }
        }
        apply_account_stats_deltas(&mut transaction, status_deltas).await?;
        if !affected_status_ids.is_empty() {
            remove_statuses_from_account_conversations(&mut transaction, &affected_status_ids)
                .await?;
            sqlx::query(
                "DELETE FROM media_attachments
                  WHERE account_id = $1 OR status_id = ANY($2)",
            )
            .bind(account_id)
            .bind(&affected_status_ids)
            .execute(&mut *transaction)
            .await?;
            sqlx::query("DELETE FROM mentions WHERE account_id = $1 OR status_id = ANY($2)")
                .bind(account_id)
                .bind(&affected_status_ids)
                .execute(&mut *transaction)
                .await?;
            sqlx::query("DELETE FROM status_pins WHERE account_id = $1 OR status_id = ANY($2)")
                .bind(account_id)
                .bind(&affected_status_ids)
                .execute(&mut *transaction)
                .await?;
            sqlx::query("DELETE FROM bookmarks WHERE account_id = $1 OR status_id = ANY($2)")
                .bind(account_id)
                .bind(&affected_status_ids)
                .execute(&mut *transaction)
                .await?;
        }
        for query in [
            "DELETE FROM blocks WHERE account_id = $1 OR target_account_id = $1",
            "DELETE FROM mutes WHERE account_id = $1 OR target_account_id = $1",
            "DELETE FROM notification_permissions WHERE account_id = $1 OR from_account_id = $1",
            "DELETE FROM notifications WHERE from_account_id = $1",
            "DELETE FROM notification_requests WHERE from_account_id = $1",
            "DELETE FROM account_deletion_requests WHERE account_id = $1",
        ] {
            sqlx::query(query)
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query(
            "UPDATE accounts SET silenced_at = NULL,
                 suspended_at = COALESCE(suspended_at, clock_timestamp()),
                 suspension_origin = NULL, locked = false, memorial = false,
                 discoverable = false, trendable = false, display_name = '', note = '',
                 fields = '[]'::jsonb, also_known_as = ARRAY[]::text[], url = NULL,
                 inbox_url = '', outbox_url = '', followers_url = '', following_url = '',
                 shared_inbox_url = '', avatar_content_type = NULL, avatar_file_name = NULL,
                 avatar_file_size = NULL, avatar_remote_url = NULL,
                 avatar_storage_schema_version = NULL, avatar_updated_at = NULL,
                 header_content_type = NULL, header_file_name = NULL, header_file_size = NULL,
                 header_remote_url = '', header_storage_schema_version = NULL,
                 header_updated_at = NULL,
                 updated_at = clock_timestamp()
               WHERE id = $1",
        )
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        sqlx::query(
            "UPDATE account_stats SET statuses_count = 0, following_count = 0,
                followers_count = 0, updated_at = clock_timestamp()
              WHERE account_id = $1",
        )
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn apply_remote_note_create(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        self.apply_remote_note_create_with_quote_guard(
            account_id,
            actor_uri,
            object,
            delivery_target_account_id,
            origin,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn apply_remote_quote_request_instrument(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
        request_uri: &str,
        quoted_status_uri: &str,
        instrument_uri: &str,
        expected_target_status_id: i64,
        expected_target_account_id: i64,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        let guard = RemoteQuoteImportGuard {
            request_uri,
            quoted_status_uri,
            instrument_uri,
            expected_target_status_id,
            expected_target_account_id,
        };
        self.apply_remote_note_create_with_quote_guard(
            account_id,
            actor_uri,
            object,
            delivery_target_account_id,
            origin,
            Some(&guard),
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn apply_remote_note_create_with_quote_guard(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
        quote_guard: Option<&RemoteQuoteImportGuard<'_>>,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        let mut note = RemoteNoteData::parse(object, actor_uri)?;
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_note(&mut transaction, &note.uri).await?;
        if let Some(guard) = quote_guard {
            if !same_remote_note_host(actor_uri, guard.request_uri)?
                || !same_remote_note_host(actor_uri, guard.instrument_uri)?
            {
                return Err(WriteError::InvalidInput(
                    "remote QuoteRequest identifiers do not match its actor host",
                ));
            }
            lock_remote_interaction(&mut transaction, guard.request_uri).await?;
            if remote_quote_request_decision_in(
                &mut transaction,
                guard.request_uri,
                actor_uri,
                guard.quoted_status_uri,
                guard.instrument_uri,
            )
            .await?
            .is_some()
            {
                transaction.commit().await?;
                return Ok(None);
            }
            let target =
                resolve_quote_target(&mut transaction, guard.quoted_status_uri, origin).await?;
            if target
                .as_ref()
                .is_none_or(|(status_id, account_id, local, _)| {
                    *status_id != guard.expected_target_status_id
                        || *account_id != guard.expected_target_account_id
                        || !*local
                        || delivery_target_account_id.is_some_and(|id| id != *account_id)
                })
            {
                transaction.commit().await?;
                return Ok(None);
            }
            let existing_status_id = remote_note_status_id_for_account(
                &mut transaction,
                account_id,
                &note.uri,
                note.atom_uri.as_deref(),
            )
            .await?;
            let mut guarded_status_ids = existing_status_id.into_iter().collect::<Vec<_>>();
            guarded_status_ids.push(guard.expected_target_status_id);
            lock_statuses_in_order(&mut transaction, &guarded_status_ids).await?;
            match writable_quote_target(
                &mut transaction,
                account_id,
                guard.expected_target_status_id,
            )
            .await
            {
                Ok(_) => {}
                Err(WriteError::NotFound | WriteError::Forbidden) => {
                    transaction.commit().await?;
                    return Ok(None);
                }
                Err(error) => return Err(error),
            }
        }
        let account = sqlx::query_as::<
            _,
            (
                Option<String>,
                String,
                String,
                String,
                Option<NaiveDateTime>,
            ),
        >(
            "SELECT domain, uri, followers_url, following_url, suspended_at FROM accounts
             WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((domain, current_actor_uri, followers_url, following_url, suspended_at)) = account
        else {
            return Ok(None);
        };
        if domain.is_none() || current_actor_uri != actor_uri || suspended_at.is_some() {
            return Ok(None);
        }
        note.quote_approval_policy = remote_quote_approval_policy_with_collections(
            object
                .as_object()
                .ok_or(WriteError::InvalidInput("remote Note is not an object"))?,
            actor_uri,
            &followers_url,
            &following_url,
        )?;
        if !same_remote_note_host(actor_uri, &note.uri)? {
            return Err(WriteError::InvalidInput(
                "remote Note URI does not match its actor host",
            ));
        }
        let tombstoned = remote_note_tombstoned(
            &mut transaction,
            account_id,
            &note.uri,
            note.atom_uri.as_deref(),
        )
        .await?;
        if tombstoned {
            transaction.commit().await?;
            return Ok(None);
        }
        let existing_status_id = remote_note_status_id_for_account(
            &mut transaction,
            account_id,
            &note.uri,
            note.atom_uri.as_deref(),
        )
        .await?;
        prelock_remote_note_quote_targets(&mut transaction, existing_status_id, &note, origin)
            .await?;
        let existing =
            remote_note_status(&mut transaction, &note.uri, note.atom_uri.as_deref()).await?;
        if let Some((existing_status_id, existing_account_id, _, _, _)) = existing {
            if existing_account_id != account_id {
                return Err(WriteError::Conflict);
            }
            if let Some(delivery_target_account_id) = delivery_target_account_id {
                ensure_remote_note_delivery_target(
                    &mut transaction,
                    existing_status_id,
                    delivery_target_account_id,
                )
                .await?;
            }
            transaction.commit().await?;
            return Ok(None);
        }
        upsert_remote_emojis(
            &mut transaction,
            domain.as_deref().expect("remote accounts have a domain"),
            actor_uri,
            object,
        )
        .await?;
        let visibility = remote_note_visibility(&note.audience, &followers_url);
        let (in_reply_to_id, in_reply_to_account_id, conversation_id) =
            remote_note_thread(&mut transaction, &note, origin).await?;
        let status_id = insert_remote_note(
            &mut transaction,
            account_id,
            &note,
            visibility,
            in_reply_to_id,
            in_reply_to_account_id,
            conversation_id,
        )
        .await?;
        if let Some(poll) = note.poll.as_ref() {
            upsert_remote_poll(
                &mut transaction,
                status_id,
                account_id,
                poll,
                true,
                false,
                false,
            )
            .await?;
        }
        if visibility < 2
            && let Some(in_reply_to_id) = in_reply_to_id
        {
            increment_reply_count(&mut transaction, in_reply_to_id).await?;
        }
        let conversation_id = ensure_remote_note_conversation(
            &mut transaction,
            status_id,
            account_id,
            in_reply_to_id,
            in_reply_to_account_id,
            conversation_id,
            note.conversation_uri.as_deref(),
            note.published_at,
        )
        .await?;
        if conversation_id.is_some() {
            sqlx::query("UPDATE statuses SET conversation_id = $1 WHERE id = $2")
                .bind(conversation_id)
                .bind(status_id)
                .execute(&mut *transaction)
                .await?;
        }
        let media_ids =
            insert_remote_note_media(&mut transaction, status_id, account_id, &note.attachments)
                .await?;
        sqlx::query("UPDATE statuses SET ordered_media_attachment_ids = $2 WHERE id = $1")
            .bind(status_id)
            .bind(&media_ids)
            .execute(&mut *transaction)
            .await?;
        let mention_ids = insert_remote_note_mentions(
            &mut transaction,
            status_id,
            &note,
            delivery_target_account_id,
            origin,
        )
        .await?;
        reconcile_remote_note_quote(&mut transaction, status_id, account_id, &note, origin).await?;
        // Classify only after resolving mentions, including an implicit inbox recipient.
        // An explicitly mentioned Note is direct only when no silent recipient was added.
        let visibility = if visibility == 4
            && remote_note_has_only_explicit_recipients(&mut transaction, status_id, &note).await?
        {
            sqlx::query("UPDATE statuses SET visibility = 3 WHERE id = $1")
                .bind(status_id)
                .execute(&mut *transaction)
                .await?;
            3
        } else {
            visibility
        };
        update_remote_note_tags(&mut transaction, status_id, &note.hashtags).await?;
        insert_remote_note_stats(&mut transaction, status_id, &note).await?;
        if visibility == 3 {
            ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        } else {
            increment_account_status_count(&mut transaction, account_id, note.published_at).await?;
        }
        if note.in_reply_to_uri.is_some() && in_reply_to_id.is_none() {
            let thread_job = JobSpec::new(
                Lane::Pull,
                ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND,
                json!({
                    "child_status_id": status_id,
                    "parent_url": note.in_reply_to_uri.as_deref()
                }),
            )
            .logical_key(format!("activitypub:thread:{status_id}"))
            .max_attempts(4);
            record_outbox_once_in(&mut transaction, &thread_job).await?;
        }
        for (mention_id, recipient_account_id) in &mention_ids {
            record_outbox_in(
                &mut transaction,
                &notification_job(*recipient_account_id, NOTIFICATION_MENTION, *mention_id),
            )
            .await?;
        }
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "update",
            note.published_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(Some(RemoteNoteWriteOutcome {
            status_id,
            mention_ids,
        }))
    }

    pub(crate) async fn record_remote_note_forwarding(
        &self,
        actor_uri: &str,
        object: &Value,
        activity: &Value,
    ) -> Result<(), WriteError> {
        let note = RemoteNoteData::parse(object, actor_uri)?;
        self.record_remote_activity_forwarding(
            actor_uri,
            &note.uri,
            note.atom_uri.as_deref(),
            activity,
        )
        .await
    }

    pub(crate) async fn record_remote_note_reference_forwarding(
        &self,
        actor_uri: &str,
        object_uri: &str,
        activity: &Value,
    ) -> Result<(), WriteError> {
        self.record_remote_activity_forwarding(actor_uri, object_uri, None, activity)
            .await
    }

    pub(crate) async fn record_remote_note_delete_forwarding(
        &self,
        actor_uri: &str,
        object_uri: &str,
        atom_uri: Option<&str>,
        activity: &Value,
    ) -> Result<(), WriteError> {
        self.record_remote_activity_forwarding(actor_uri, object_uri, atom_uri, activity)
            .await
    }

    #[allow(clippy::too_many_lines)]
    async fn record_remote_activity_forwarding(
        &self,
        actor_uri: &str,
        object_uri: &str,
        atom_uri: Option<&str>,
        activity: &Value,
    ) -> Result<(), WriteError> {
        let activity_uri = activity
            .get("id")
            .and_then(Value::as_str)
            .filter(|uri| !uri.trim().is_empty())
            .ok_or(WriteError::InvalidInput("signed remote activity has no ID"))?;
        if !same_remote_note_host(actor_uri, object_uri)? {
            return Err(WriteError::InvalidInput(
                "remote activity object URI does not match its actor host",
            ));
        }
        if let Some(atom_uri) = atom_uri
            && !same_remote_note_host(actor_uri, atom_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote activity atom URI does not match its actor host",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let Some((status_id, parent_account_id, source_inbox)) =
            sqlx::query_as::<_, (i64, Option<i64>, String)>(
                "SELECT status.id, parent.id,
                        COALESCE(NULLIF(source.shared_inbox_url, ''), source.inbox_url)
                   FROM statuses status
                   JOIN accounts source ON source.id = status.account_id
                                        AND source.uri = $2
                                         AND source.domain IS NOT NULL
              LEFT JOIN statuses parent_status ON parent_status.id = status.in_reply_to_id
                                               AND parent_status.deleted_at IS NULL
              LEFT JOIN accounts parent ON parent.id = parent_status.account_id
                                        AND parent.domain IS NULL
                  WHERE (status.uri = $1
                         OR ($3::text IS NOT NULL AND status.uri = $3))
                    AND status.deleted_at IS NULL
                    AND status.visibility IN (0, 1)
                  LIMIT 1",
            )
            .bind(object_uri)
            .bind(actor_uri)
            .bind(atom_uri)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        record_remote_activity_forwarding_for_status_in(
            &mut transaction,
            status_id,
            parent_account_id,
            &source_inbox,
            activity_uri,
            activity,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn resolve_remote_note_thread(
        &self,
        child_status_id: i64,
        parent_uri: &str,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let parsed_parent_url = Url::parse(parent_uri)
            .map_err(|_| WriteError::InvalidInput("remote reply parent URI is invalid"))?;
        if !matches!(parsed_parent_url.scheme(), "http" | "https")
            || parsed_parent_url.host_str().is_none()
        {
            return Err(WriteError::InvalidInput(
                "remote reply parent URI is invalid",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let child = sqlx::query_as::<_, (i64, i32, Option<i64>, Option<NaiveDateTime>)>(
            "SELECT account_id, visibility, in_reply_to_id, deleted_at
               FROM statuses WHERE id = $1 FOR UPDATE",
        )
        .bind(child_status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((child_account_id, child_visibility, current_parent_id, child_deleted_at)) = child
        else {
            transaction.commit().await?;
            return Ok(true);
        };
        if child_deleted_at.is_some() || current_parent_id.is_some() {
            transaction.commit().await?;
            return Ok(true);
        }
        let parent = sqlx::query_as::<
            _,
            (
                i64,
                i64,
                Option<i64>,
                bool,
                Option<i64>,
                Option<NaiveDateTime>,
            ),
        >(
            "SELECT status.id, status.account_id, status.conversation_id, status.reply,
                    status.in_reply_to_account_id, status.deleted_at
               FROM statuses status
               JOIN accounts author ON author.id = status.account_id
              WHERE status.deleted_at IS NULL
                AND (
                  status.uri = $1
                  OR status.url = $1
                  OR (author.domain IS NULL AND (
                    $1 = $2 || '/actor/statuses/' || status.id::text
                         || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                    OR $1 = $2 || '/@' || author.username || '/' || status.id::text
                         || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                    OR $1 = $2 || '/users/' || author.username || '/statuses/' || status.id::text
                         || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                    OR $1 = $2 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text
                         || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                  ))
                )
              ORDER BY status.id
              LIMIT 1 FOR UPDATE",
        )
        .bind(parent_uri)
        .bind(origin.trim_end_matches('/'))
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((
            parent_id,
            parent_account_id,
            parent_conversation_id,
            parent_reply,
            parent_reply_account_id,
            parent_deleted_at,
        )) = parent
        else {
            transaction.commit().await?;
            return Ok(false);
        };
        if parent_deleted_at.is_some() {
            transaction.commit().await?;
            return Ok(true);
        }
        if parent_id == child_status_id {
            return Err(WriteError::InvalidInput(
                "remote reply parent cannot be the child status",
            ));
        }
        let carried_reply_account_id = if parent_reply && parent_account_id == child_account_id {
            parent_reply_account_id.or(Some(parent_account_id))
        } else {
            Some(parent_account_id)
        };
        sqlx::query(
            "UPDATE statuses SET reply = true, in_reply_to_id = $2,
                in_reply_to_account_id = $3,
                conversation_id = COALESCE(conversation_id, $4),
                updated_at = clock_timestamp()
              WHERE id = $1 AND in_reply_to_id IS NULL",
        )
        .bind(child_status_id)
        .bind(parent_id)
        .bind(carried_reply_account_id)
        .bind(parent_conversation_id)
        .execute(&mut *transaction)
        .await?;
        if child_visibility < 2 {
            increment_reply_count(&mut transaction, parent_id).await?;
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub(crate) async fn apply_remote_note_update(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        self.apply_remote_note_update_with_authority(
            account_id,
            actor_uri,
            object,
            delivery_target_account_id,
            origin,
            RemoteUpdateAuthority::Inbox,
            None,
        )
        .await
    }

    pub(crate) async fn apply_signed_remote_poll_refresh(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        origin: &str,
        expected_poll_id: i64,
        expected_poll_lock_version: i32,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        self.apply_remote_note_update_with_authority(
            account_id,
            actor_uri,
            object,
            None,
            origin,
            RemoteUpdateAuthority::SignedRefresh,
            Some((expected_poll_id, expected_poll_lock_version)),
        )
        .await
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub async fn apply_signed_remote_poll_refresh_for_test(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        origin: &str,
        expected_poll_id: i64,
        expected_poll_lock_version: i32,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        self.apply_signed_remote_poll_refresh(
            account_id,
            actor_uri,
            object,
            origin,
            expected_poll_id,
            expected_poll_lock_version,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    async fn apply_remote_note_update_with_authority(
        &self,
        account_id: i64,
        actor_uri: &str,
        object: &Value,
        delivery_target_account_id: Option<i64>,
        origin: &str,
        authority: RemoteUpdateAuthority,
        expected_poll: Option<(i64, i32)>,
    ) -> Result<Option<RemoteNoteWriteOutcome>, WriteError> {
        let mut note = RemoteNoteData::parse(object, actor_uri)?;
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_note(&mut transaction, &note.uri).await?;
        if !same_remote_note_host(actor_uri, &note.uri)? {
            return Err(WriteError::InvalidInput(
                "remote Note URI does not match its actor host",
            ));
        }
        let account = sqlx::query_as::<
            _,
            (
                Option<String>,
                String,
                String,
                String,
                Option<NaiveDateTime>,
            ),
        >(
            "SELECT domain, uri, followers_url, following_url, suspended_at FROM accounts
             WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((domain, current_actor_uri, followers_url, following_url, suspended_at)) = account
        else {
            return Ok(None);
        };
        if domain.is_none() || current_actor_uri != actor_uri || suspended_at.is_some() {
            return Ok(None);
        }
        note.quote_approval_policy = remote_quote_approval_policy_with_collections(
            object
                .as_object()
                .ok_or(WriteError::InvalidInput("remote Note is not an object"))?,
            actor_uri,
            &followers_url,
            &following_url,
        )?;
        let existing_status_id = remote_note_status_id_for_account(
            &mut transaction,
            account_id,
            &note.uri,
            note.atom_uri.as_deref(),
        )
        .await?;
        prelock_remote_note_quote_targets(&mut transaction, existing_status_id, &note, origin)
            .await?;
        let Some((
            status_id,
            existing_account_id,
            deleted_at,
            current_edited_at,
            current_created_at,
        )) = remote_note_status(&mut transaction, &note.uri, note.atom_uri.as_deref()).await?
        else {
            let tombstoned = remote_note_tombstoned(
                &mut transaction,
                account_id,
                &note.uri,
                note.atom_uri.as_deref(),
            )
            .await?;
            transaction.commit().await?;
            if tombstoned {
                return Ok(None);
            }
            if remote_note_object_is_too_old(object, note.published_at, Utc::now().naive_utc()) {
                return Ok(None);
            }
            return self
                .apply_remote_note_create(
                    account_id,
                    actor_uri,
                    object,
                    delivery_target_account_id,
                    origin,
                )
                .await;
        };
        if existing_account_id != account_id {
            return Err(WriteError::Conflict);
        }
        if deleted_at.is_some() {
            transaction.commit().await?;
            return Ok(None);
        }
        if let Some((expected_poll_id, expected_lock_version)) = expected_poll {
            let matching_poll_id = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM polls \
                 WHERE id = $1 AND status_id = $2 AND lock_version = $3 FOR UPDATE",
            )
            .bind(expected_poll_id)
            .bind(status_id)
            .bind(expected_lock_version)
            .fetch_optional(&mut *transaction)
            .await?;
            if matching_poll_id != Some(expected_poll_id) {
                transaction.commit().await?;
                return Err(WriteError::Conflict);
            }
        }
        if note.edited_at.is_none() {
            // Unsolicited inbox objects cannot roll an explicitly versioned status back. A
            // directly signed poll refresh is authoritative only for unchanged poll shape/tallies.
            if current_edited_at.is_some() && authority == RemoteUpdateAuthority::Inbox {
                transaction.commit().await?;
                return Ok(None);
            }
            let quote_changed =
                reconcile_remote_note_quote(&mut transaction, status_id, account_id, &note, origin)
                    .await?;
            let poll_reconcile =
                if authority == RemoteUpdateAuthority::SignedRefresh && note.poll.is_none() {
                    RemotePollReconcile::Unchanged
                } else {
                    reconcile_remote_poll(
                        &mut transaction,
                        status_id,
                        account_id,
                        note.poll.as_ref(),
                        false,
                        authority.rejects_tally_regression(),
                        authority.claims_freshness(),
                    )
                    .await?
                };
            update_remote_note_stats(&mut transaction, status_id, &note).await?;
            if quote_changed {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    note.updated_at.and_utc().timestamp_micros(),
                )
                .await?;
            }
            if let RemotePollReconcile::Tally(updated_at) = poll_reconcile {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    updated_at.and_utc().timestamp_micros(),
                )
                .await?;
            }
            flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
            transaction.commit().await?;
            return Ok(None);
        }
        let current_version = current_edited_at.unwrap_or(current_created_at);
        if note.updated_at < current_version {
            transaction.commit().await?;
            return Ok(None);
        }
        if note.updated_at == current_version {
            let quote_changed =
                reconcile_remote_note_quote(&mut transaction, status_id, account_id, &note, origin)
                    .await?;
            let poll_reconcile =
                if authority == RemoteUpdateAuthority::SignedRefresh && note.poll.is_none() {
                    RemotePollReconcile::Unchanged
                } else {
                    reconcile_remote_poll(
                        &mut transaction,
                        status_id,
                        account_id,
                        note.poll.as_ref(),
                        false,
                        authority.rejects_tally_regression(),
                        authority.claims_freshness(),
                    )
                    .await?
                };
            update_remote_note_stats(&mut transaction, status_id, &note).await?;
            if quote_changed {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    note.updated_at.and_utc().timestamp_micros(),
                )
                .await?;
            }
            if let RemotePollReconcile::Tally(updated_at) = poll_reconcile {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    updated_at.and_utc().timestamp_micros(),
                )
                .await?;
            }
            flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
            transaction.commit().await?;
            return Ok(None);
        }
        let poll_reconcile = reconcile_remote_poll(
            &mut transaction,
            status_id,
            account_id,
            note.poll.as_ref(),
            true,
            authority.rejects_tally_regression(),
            authority.claims_freshness(),
        )
        .await?;
        let html_origin =
            Url::parse(origin).map_err(|_| WriteError::InvalidInput("local origin is invalid"))?;
        let formatter =
            HtmlFormatter::new(&html_origin, html_origin.host_str().unwrap_or_default());
        let route_before = status_timeline_snapshot(&mut transaction, status_id).await?;
        let before = remote_note_edit_projection(&mut transaction, status_id, &formatter).await?;
        upsert_remote_emojis(
            &mut transaction,
            domain.as_deref().expect("remote accounts have a domain"),
            actor_uri,
            object,
        )
        .await?;
        sqlx::query(
            "UPDATE statuses SET text = $2, spoiler_text = $3, sensitive = $4,
                language = $5, quote_approval_policy = $6, updated_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(status_id)
        .bind(&note.content)
        .bind(&note.summary)
        .bind(note.sensitive)
        .bind(&note.language)
        .bind(note.quote_approval_policy)
        .execute(&mut *transaction)
        .await?;
        remove_remote_note_media_not_in(&mut transaction, status_id, &note.attachments).await?;
        let old_mentions = sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, account_id FROM mentions WHERE status_id = $1 ORDER BY account_id, id",
        )
        .bind(status_id)
        .fetch_all(&mut *transaction)
        .await?;
        for (mention_id, recipient_account_id) in &old_mentions {
            cancel_pending_notification_jobs(&mut transaction, *recipient_account_id, *mention_id)
                .await?;
        }
        sqlx::query(
            "UPDATE mentions SET silent = true, updated_at = clock_timestamp() \
             WHERE status_id = $1",
        )
        .bind(status_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM statuses_tags WHERE status_id = $1")
            .bind(status_id)
            .execute(&mut *transaction)
            .await?;
        let media_ids =
            insert_remote_note_media(&mut transaction, status_id, account_id, &note.attachments)
                .await?;
        sqlx::query("UPDATE statuses SET ordered_media_attachment_ids = $2 WHERE id = $1")
            .bind(status_id)
            .bind(&media_ids)
            .execute(&mut *transaction)
            .await?;
        let mention_ids = insert_remote_note_mentions(
            &mut transaction,
            status_id,
            &note,
            delivery_target_account_id,
            origin,
        )
        .await?;
        let quote_changed =
            reconcile_remote_note_quote(&mut transaction, status_id, account_id, &note, origin)
                .await?;
        update_remote_note_tags(&mut transaction, status_id, &note.hashtags).await?;
        update_remote_note_stats(&mut transaction, status_id, &note).await?;
        for (mention_id, recipient_account_id) in &mention_ids {
            record_outbox_in(
                &mut transaction,
                &notification_job(*recipient_account_id, NOTIFICATION_MENTION, *mention_id),
            )
            .await?;
        }
        let projection_changed =
            remote_note_edit_projection(&mut transaction, status_id, &formatter).await? != before;
        let route_after = status_timeline_snapshot(&mut transaction, status_id).await?;
        // Route-only edits (for example hashtag or language changes) still need a timeline
        // transition even when the rendered status projection is byte-for-byte unchanged.
        let meaningful_update = quote_changed
            || projection_changed
            || route_after != route_before
            || poll_reconcile == RemotePollReconcile::Significant;
        if !meaningful_update {
            if let RemotePollReconcile::Tally(updated_at) = poll_reconcile {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    updated_at.and_utc().timestamp_micros(),
                )
                .await?;
                flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
            }
            transaction.commit().await?;
            return Ok(None);
        }
        sqlx::query("UPDATE statuses SET edited_at = $2 WHERE id = $1")
            .bind(status_id)
            .bind(note.updated_at)
            .execute(&mut *transaction)
            .await?;
        record_status_update_notifications(
            &mut transaction,
            status_id,
            note.updated_at.and_utc().timestamp_micros(),
        )
        .await?;
        collect_status_stream_transition(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "status.update",
            StreamEventLogicalKey::Version(note.updated_at.and_utc().timestamp_micros()),
            Some(route_before),
            Some(route_after),
        )
        .await?;
        collect_status_update_notification_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            note.updated_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(Some(RemoteNoteWriteOutcome {
            status_id,
            mention_ids,
        }))
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_note_delete(
        &self,
        account_id: i64,
        actor_uri: &str,
        object_uri: &str,
        atom_uri: Option<&str>,
        origin: &str,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_quote_status_deletion(&mut transaction).await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_note(&mut transaction, object_uri).await?;
        let account = sqlx::query_as::<_, (Option<String>, String, Option<NaiveDateTime>)>(
            "SELECT domain, uri, suspended_at FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((domain, current_actor_uri, _)) = account else {
            return Ok(());
        };
        if domain.is_none() || current_actor_uri != actor_uri {
            return Ok(());
        }
        if !same_remote_note_host(actor_uri, object_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Delete URI does not match its actor host",
            ));
        }
        if let Some(atom_uri) = atom_uri
            && !same_remote_note_host(actor_uri, atom_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote Delete atom URI does not match its actor host",
            ));
        }
        let status_id =
            remote_note_status_id_for_account(&mut transaction, account_id, object_uri, atom_uri)
                .await?;
        if let Some(status_id) = status_id {
            let mut quote_lifecycle_status_ids = sqlx::query_scalar::<_, i64>(
                "SELECT quote.status_id FROM quotes quote
                   JOIN statuses quoting ON quoting.id = quote.status_id
                  WHERE quote.quoted_status_id = $1 AND quoting.deleted_at IS NULL
                  ORDER BY quote.status_id",
            )
            .bind(status_id)
            .fetch_all(&mut *transaction)
            .await?;
            quote_lifecycle_status_ids.push(status_id);
            lock_statuses_in_order(&mut transaction, &quote_lifecycle_status_ids).await?;
            let status = sqlx::query_as::<_, (i64, Option<NaiveDateTime>, i32, Option<i64>)>(
                "SELECT account_id, deleted_at, visibility, in_reply_to_id
                   FROM statuses WHERE id = $1 FOR UPDATE",
            )
            .bind(status_id)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some((existing_account_id, deleted_at, visibility, in_reply_to_id)) = status {
                if existing_account_id != account_id {
                    return Err(WriteError::Conflict);
                }
                let owned_quote = sqlx::query_as::<
                    _,
                    (i64, Option<i64>, Option<i64>, i32, Option<String>, bool),
                >(
                    "SELECT quote.id, quote.quoted_status_id, quote.quoted_account_id,
                                quote.state, quote.activity_uri,
                                COALESCE(target.domain IS NULL, false)
                           FROM quotes quote
                      LEFT JOIN accounts target ON target.id = quote.quoted_account_id
                          WHERE quote.status_id = $1 FOR UPDATE OF quote",
                )
                .bind(status_id)
                .fetch_optional(&mut *transaction)
                .await?;
                let quoting_quotes = sqlx::query_as::<_, (i64, i64, Option<String>)>(
                    "SELECT quote.id, quote.status_id, quote.activity_uri FROM quotes quote \
                       JOIN statuses quoting ON quoting.id = quote.status_id \
                      WHERE quote.quoted_status_id = $1 AND quoting.deleted_at IS NULL \
                      ORDER BY quote.id FOR UPDATE OF quote",
                )
                .bind(status_id)
                .fetch_all(&mut *transaction)
                .await?;
                if !quoting_quotes.is_empty() {
                    sqlx::query(
                        "UPDATE quotes SET quoted_status_id = NULL, approval_uri = NULL, \
                                updated_at = clock_timestamp() \
                         WHERE quoted_status_id = $1",
                    )
                    .bind(status_id)
                    .execute(&mut *transaction)
                    .await?;
                    for (quote_id, quoting_status_id, request_uri) in quoting_quotes {
                        cancel_quote_request_outbox(
                            &mut transaction,
                            quote_id,
                            request_uri.as_deref(),
                        )
                        .await?;
                        let version = if sqlx::query_scalar::<_, bool>(
                            "SELECT account.domain IS NULL FROM statuses quoting \
                             JOIN accounts account ON account.id = quoting.account_id \
                             WHERE quoting.id = $1",
                        )
                        .bind(quoting_status_id)
                        .fetch_one(&mut *transaction)
                        .await?
                        {
                            record_quote_status_update(&mut transaction, quoting_status_id)
                                .await?
                                .and_utc()
                                .timestamp_micros()
                        } else {
                            Utc::now().timestamp_micros()
                        };
                        collect_status_stream_events(
                            &mut transaction,
                            &mut pending_stream_events,
                            quoting_status_id,
                            "status.update",
                            version,
                        )
                        .await?;
                    }
                }
                let reblogs = sqlx::query_as::<_, (i64, i64, i32)>(
                    "SELECT id, account_id, visibility FROM statuses
                      WHERE reblog_of_id = $1 AND deleted_at IS NULL
                      ORDER BY id FOR UPDATE",
                )
                .bind(status_id)
                .fetch_all(&mut *transaction)
                .await?;
                let reblog_ids = reblogs.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
                let local_reblog_ids = sqlx::query_scalar::<_, i64>(
                    "SELECT status.id FROM statuses status
                       JOIN accounts account ON account.id = status.account_id
                      WHERE status.id = ANY($1) AND status.local IS TRUE
                        AND account.domain IS NULL
                      ORDER BY status.id",
                )
                .bind(&reblog_ids)
                .fetch_all(&mut *transaction)
                .await?;
                let mut status_deltas = HashMap::new();
                add_account_stats_delta(
                    &mut status_deltas,
                    account_id,
                    AccountStatsDelta::default(),
                );
                if !reblogs.is_empty() {
                    sqlx::query(
                        "UPDATE statuses SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                          WHERE id = ANY($1)",
                    )
                    .bind(&reblog_ids)
                    .execute(&mut *transaction)
                    .await?;
                    for (_, reblog_account_id, reblog_visibility) in &reblogs {
                        if *reblog_visibility != 3 {
                            add_account_stats_delta(
                                &mut status_deltas,
                                *reblog_account_id,
                                AccountStatsDelta {
                                    statuses: -1,
                                    ..AccountStatsDelta::default()
                                },
                            );
                        }
                        decrement_reblog_count(&mut transaction, status_id).await?;
                    }
                    for (reblog_id, _, _) in &reblogs {
                        collect_status_delete_stream_events(
                            &mut transaction,
                            &mut pending_stream_events,
                            *reblog_id,
                        )
                        .await?;
                    }
                }
                for reblog_id in local_reblog_ids {
                    record_status_delete_distribution(&mut transaction, reblog_id, &[]).await?;
                }
                if deleted_at.is_none() {
                    if let Some((
                        quote_id,
                        quoted_status_id,
                        quoted_account_id,
                        state,
                        request_uri,
                        quoted_account_local,
                    )) = owned_quote
                    {
                        if state == 1
                            && let Some(quoted_status_id) = quoted_status_id
                        {
                            decrement_quote_count(&mut transaction, quoted_status_id).await?;
                            if quoted_account_local
                                && let Some(quoted_account_id) = quoted_account_id
                            {
                                record_quote_authorization_delete(
                                    &mut transaction,
                                    quote_id,
                                    status_id,
                                    quoted_status_id,
                                    quoted_account_id,
                                    origin,
                                )
                                .await?;
                            }
                        }
                        if let Some(quoted_account_id) = quoted_account_id {
                            delete_activity_notifications(
                                &mut transaction,
                                quoted_account_id,
                                quote_id,
                                "Quote",
                            )
                            .await?;
                        }
                        cancel_quote_request_outbox(
                            &mut transaction,
                            quote_id,
                            request_uri.as_deref(),
                        )
                        .await?;
                    }
                    sqlx::query(
                        "UPDATE statuses SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                         WHERE id = $1",
                    )
                    .bind(status_id)
                    .execute(&mut *transaction)
                    .await?;
                    if visibility != 3 {
                        add_account_stats_delta(
                            &mut status_deltas,
                            account_id,
                            AccountStatsDelta {
                                statuses: -1,
                                ..AccountStatsDelta::default()
                            },
                        );
                    }
                    if visibility < 2
                        && let Some(in_reply_to_id) = in_reply_to_id
                    {
                        decrement_reply_count(&mut transaction, in_reply_to_id).await?;
                    }
                }
                apply_account_stats_deltas(&mut transaction, status_deltas).await?;
                collect_status_delete_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                )
                .await?;
            }
            let mut affected_status_ids = vec![status_id];
            affected_status_ids.extend(
                sqlx::query_scalar::<_, i64>(
                    "SELECT id FROM statuses WHERE reblog_of_id = $1 ORDER BY id",
                )
                .bind(status_id)
                .fetch_all(&mut *transaction)
                .await?,
            );
            delete_remote_status_notifications(&mut transaction, &affected_status_ids).await?;
            remove_favourites_for_statuses(&mut transaction, &affected_status_ids).await?;
            remove_poll_data_for_statuses(&mut transaction, &affected_status_ids).await?;
            remove_statuses_from_account_conversations(&mut transaction, &affected_status_ids)
                .await?;
            sqlx::query("DELETE FROM media_attachments WHERE status_id = ANY($1::bigint[])")
                .bind(&affected_status_ids)
                .execute(&mut *transaction)
                .await?;
        }
        insert_remote_note_tombstone(&mut transaction, account_id, object_uri).await?;
        if let Some(atom_uri) = atom_uri.filter(|value| *value != object_uri) {
            insert_remote_note_tombstone(&mut transaction, account_id, atom_uri).await?;
        }
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn remote_undo_reference_kind(
        &self,
        account_id: i64,
        activity_uri: &str,
    ) -> Result<RemoteUndoReferenceKind, WriteError> {
        let kind = sqlx::query_scalar::<_, i32>(
            "SELECT CASE
                WHEN EXISTS (
                    SELECT 1 FROM statuses
                     WHERE account_id = $1 AND uri = $2
                       AND reblog_of_id IS NOT NULL AND deleted_at IS NULL
                ) THEN 2
                WHEN EXISTS (
                    SELECT 1 FROM follows WHERE account_id = $1 AND uri = $2
                ) OR EXISTS (
                    SELECT 1 FROM follow_requests WHERE account_id = $1 AND uri = $2
                ) THEN 0
                WHEN EXISTS (
                    SELECT 1 FROM blocks WHERE account_id = $1 AND uri = $2
                ) THEN 1
                ELSE 3
             END",
        )
        .bind(account_id)
        .bind(activity_uri)
        .fetch_one(&self.pool)
        .await?;
        Ok(match kind {
            0 => RemoteUndoReferenceKind::Follow,
            1 => RemoteUndoReferenceKind::Block,
            2 => RemoteUndoReferenceKind::Announce,
            _ => RemoteUndoReferenceKind::Unknown,
        })
    }

    pub(crate) async fn apply_remote_like(
        &self,
        account_id: i64,
        actor_uri: &str,
        activity_uri: &str,
        object_uri: &str,
        origin: &str,
    ) -> Result<Option<RemoteInteractionWriteOutcome>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_remote_interaction(&mut transaction, activity_uri).await?;
        if !same_remote_note_host(actor_uri, activity_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Like URI does not match its actor host",
            ));
        }
        if !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, true).await? {
            return Ok(None);
        }
        if remote_interaction_tombstoned(&mut transaction, account_id, activity_uri).await? {
            transaction.commit().await?;
            return Ok(None);
        }
        let Some((status_id, recipient_account_id, _)) =
            local_interaction_target(&mut transaction, object_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        let favourite_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO favourites (account_id, status_id, created_at, updated_at)
             VALUES ($1, $2, clock_timestamp(), clock_timestamp())
             ON CONFLICT (account_id, status_id) DO NOTHING RETURNING id",
        )
        .bind(account_id)
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(favourite_id) = favourite_id else {
            let favourite_id = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM favourites WHERE account_id = $1 AND status_id = $2",
            )
            .bind(account_id)
            .bind(status_id)
            .fetch_one(&mut *transaction)
            .await?;
            return Ok(Some(RemoteInteractionWriteOutcome {
                activity_id: favourite_id,
                recipient_account_id,
            }));
        };
        increment_favourite_count(&mut transaction, status_id).await?;
        record_outbox_in(
            &mut transaction,
            &notification_job(recipient_account_id, NOTIFICATION_FAVOURITE, favourite_id),
        )
        .await?;
        transaction.commit().await?;
        Ok(Some(RemoteInteractionWriteOutcome {
            activity_id: favourite_id,
            recipient_account_id,
        }))
    }

    pub(crate) async fn apply_remote_undo_like(
        &self,
        account_id: i64,
        actor_uri: &str,
        activity_uri: &str,
        object_uri: &str,
        origin: &str,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_remote_interaction(&mut transaction, activity_uri).await?;
        if !same_remote_note_host(actor_uri, activity_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Undo Like URI does not match its actor host",
            ));
        }
        if !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, false).await?
        {
            return Ok(());
        }
        let Some((status_id, _, _)) =
            local_interaction_target(&mut transaction, object_uri, origin).await?
        else {
            insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
            transaction.commit().await?;
            return Ok(());
        };
        let Some((favourite_id, recipient_account_id)) = sqlx::query_as::<_, (i64, i64)>(
            "DELETE FROM favourites favourite USING statuses status
              WHERE favourite.account_id = $1 AND favourite.status_id = status.id
                AND status.id = $2 RETURNING favourite.id, status.account_id",
        )
        .bind(account_id)
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
            transaction.commit().await?;
            return Ok(());
        };
        decrement_favourite_count(&mut transaction, status_id).await?;
        delete_activity_notifications(
            &mut transaction,
            recipient_account_id,
            favourite_id,
            "Favourite",
        )
        .await?;
        insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) async fn apply_remote_announce(
        &self,
        account_id: i64,
        actor_uri: &str,
        activity_uri: &str,
        object_uri: &str,
        to: &[String],
        cc: &[String],
        published_at: Option<&str>,
        origin: &str,
    ) -> Result<Option<RemoteInteractionWriteOutcome>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, activity_uri).await?;
        if !same_remote_note_host(actor_uri, activity_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Announce URI does not match its actor host",
            ));
        }
        if !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, true).await? {
            return Ok(None);
        }
        if remote_interaction_tombstoned(&mut transaction, account_id, activity_uri).await? {
            transaction.commit().await?;
            return Ok(None);
        }
        let Some(outer_status_target) =
            announce_interaction_target(&mut transaction, object_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        let (
            target_status_id,
            recipient_account_id,
            target_visibility,
            target_account_is_local,
            original_account_is_local,
        ) = outer_status_target;
        if !target_account_is_local
            && !remote_announce_is_relevant(&mut transaction, account_id).await?
        {
            transaction.commit().await?;
            return Ok(None);
        }
        if let Some((existing_boost_id, existing_account_id, existing_target_id)) =
            sqlx::query_as::<_, (i64, i64, Option<i64>)>(
                "SELECT id, account_id, reblog_of_id FROM statuses WHERE uri = $1 FOR UPDATE",
            )
            .bind(activity_uri)
            .fetch_optional(&mut *transaction)
            .await?
        {
            if existing_account_id != account_id || existing_target_id != Some(target_status_id) {
                return Err(WriteError::Conflict);
            }
            transaction.commit().await?;
            return Ok(Some(RemoteInteractionWriteOutcome {
                activity_id: existing_boost_id,
                recipient_account_id,
            }));
        }
        if let Some(existing_boost_id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses
             WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL
             ORDER BY id DESC LIMIT 1 FOR UPDATE",
        )
        .bind(account_id)
        .bind(target_status_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            transaction.commit().await?;
            return Ok(Some(RemoteInteractionWriteOutcome {
                activity_id: existing_boost_id,
                recipient_account_id,
            }));
        }
        if target_visibility > 1 && recipient_account_id != account_id {
            return Err(WriteError::InvalidInput(
                "remote Announce target is not distributable",
            ));
        }
        let followers_url = sqlx::query_scalar::<_, String>(
            "SELECT followers_url FROM accounts WHERE id = $1 AND uri = $2 FOR UPDATE",
        )
        .bind(account_id)
        .bind(actor_uri)
        .fetch_one(&mut *transaction)
        .await?;
        let visibility = remote_interaction_visibility(to, cc, &followers_url);
        if visibility > 2 {
            return Err(WriteError::InvalidInput(
                "remote Announce visibility is not distributable",
            ));
        }
        let created_at = remote_interaction_timestamp(published_at)?;
        let boost_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO statuses (
                account_id, text, spoiler_text, visibility, local, sensitive, reply,
                reblog_of_id, uri, url, created_at, updated_at
             ) VALUES ($1, '', '', $4, false, false, false, $2, $3, NULL,
                         $5, clock_timestamp()) RETURNING id",
        )
        .bind(account_id)
        .bind(target_status_id)
        .bind(activity_uri)
        .bind(visibility)
        .bind(created_at)
        .fetch_one(&mut *transaction)
        .await?;
        let conversation_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO conversations (created_at, parent_account_id, parent_status_id, updated_at, uri)
             VALUES (clock_timestamp(), $1, $2, clock_timestamp(), NULL)
             RETURNING id",
        )
        .bind(account_id)
        .bind(boost_id)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("UPDATE statuses SET conversation_id = $1 WHERE id = $2")
            .bind(conversation_id)
            .bind(boost_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO status_stats (status_id, created_at, updated_at)
             VALUES ($1, clock_timestamp(), clock_timestamp())",
        )
        .bind(boost_id)
        .execute(&mut *transaction)
        .await?;
        if visibility == 3 {
            ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        } else {
            increment_account_status_count(&mut transaction, account_id, created_at).await?;
        }
        increment_reblog_count(&mut transaction, target_status_id).await?;
        if original_account_is_local
            && !remote_announce_notification_suppressed(
                &mut transaction,
                account_id,
                recipient_account_id,
            )
            .await?
        {
            record_outbox_in(
                &mut transaction,
                &notification_job(recipient_account_id, NOTIFICATION_REBLOG, boost_id),
            )
            .await?;
        }
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            boost_id,
            "update",
            created_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(Some(RemoteInteractionWriteOutcome {
            activity_id: boost_id,
            recipient_account_id,
        }))
    }

    pub(crate) async fn apply_remote_undo_announce(
        &self,
        account_id: i64,
        actor_uri: &str,
        activity_uri: &str,
        object_uri: &str,
        origin: &str,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, activity_uri).await?;
        if !same_remote_note_host(actor_uri, activity_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Undo Announce URI does not match its actor host",
            ));
        }
        if !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, false).await?
        {
            return Ok(());
        }
        let Some((target_status_id, _, _, _, _)) =
            announce_interaction_target(&mut transaction, object_uri, origin).await?
        else {
            insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
            transaction.commit().await?;
            return Ok(());
        };
        if let Some((boost_id, target_status_id, visibility, recipient_account_id)) =
            sqlx::query_as::<_, (i64, i64, i32, i64)>(
                "SELECT boost.id, boost.reblog_of_id, boost.visibility, target.account_id
              FROM statuses boost
              JOIN statuses target ON target.id = boost.reblog_of_id
              WHERE boost.account_id = $1 AND boost.uri = $2
                AND target.id = $3 AND boost.deleted_at IS NULL
              LIMIT 1 FOR UPDATE",
            )
            .bind(account_id)
            .bind(activity_uri)
            .bind(target_status_id)
            .fetch_optional(&mut *transaction)
            .await?
        {
            sqlx::query(
                "UPDATE statuses SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                 WHERE id = $1",
            )
            .bind(boost_id)
            .execute(&mut *transaction)
            .await?;
            if visibility == 3 {
                ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
            } else {
                decrement_account_status_count(&mut transaction, account_id).await?;
            }
            decrement_reblog_count(&mut transaction, target_status_id).await?;
            delete_activity_notifications(
                &mut transaction,
                recipient_account_id,
                boost_id,
                "Status",
            )
            .await?;
            collect_status_delete_stream_events(
                &mut transaction,
                &mut pending_stream_events,
                boost_id,
            )
            .await?;
        }
        insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn apply_remote_undo_announce_reference(
        &self,
        account_id: i64,
        actor_uri: &str,
        activity_uri: &str,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, activity_uri).await?;
        if !same_remote_note_host(actor_uri, activity_uri)? {
            return Err(WriteError::InvalidInput(
                "remote Undo reference URI does not match its actor host",
            ));
        }
        if !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, false).await?
        {
            return Ok(());
        }
        if let Some((boost_id, target_status_id, visibility, recipient_account_id)) =
            sqlx::query_as::<_, (i64, i64, i32, i64)>(
                "SELECT boost.id, boost.reblog_of_id, boost.visibility, target.account_id
                 FROM statuses boost
                 JOIN statuses target ON target.id = boost.reblog_of_id
                 WHERE boost.account_id = $1 AND boost.uri = $2
                   AND boost.deleted_at IS NULL AND target.deleted_at IS NULL
                 LIMIT 1",
            )
            .bind(account_id)
            .bind(activity_uri)
            .fetch_optional(&mut *transaction)
            .await?
        {
            let target_is_live = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM statuses WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
            )
            .bind(target_status_id)
            .fetch_optional(&mut *transaction)
            .await?
            .is_some();
            let boost_is_live = target_is_live
                && sqlx::query_scalar::<_, i64>(
                    "SELECT id FROM statuses WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
                )
                .bind(boost_id)
                .fetch_optional(&mut *transaction)
                .await?
                .is_some();
            if !boost_is_live {
                insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
                transaction.commit().await?;
                return Ok(());
            }
            sqlx::query(
                "UPDATE statuses SET deleted_at = clock_timestamp(), updated_at = clock_timestamp()
                 WHERE id = $1",
            )
            .bind(boost_id)
            .execute(&mut *transaction)
            .await?;
            if visibility == 3 {
                ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
            } else {
                decrement_account_status_count(&mut transaction, account_id).await?;
            }
            decrement_reblog_count(&mut transaction, target_status_id).await?;
            delete_activity_notifications(
                &mut transaction,
                recipient_account_id,
                boost_id,
                "Status",
            )
            .await?;
            collect_status_delete_stream_events(
                &mut transaction,
                &mut pending_stream_events,
                boost_id,
            )
            .await?;
        }
        insert_remote_note_tombstone(&mut transaction, account_id, activity_uri).await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn stage_media_attachment(
        &self,
        authenticated: &AuthenticatedBearer,
        create: &MediaAttachmentCreate,
    ) -> Result<i64, WriteError> {
        let account_id = write_account(authenticated, WRITE_MEDIA)?;
        self.with_account_lock(account_id, || async {
            self.stage_media_attachment_locked(authenticated, create)
                .await
        })
        .await
    }

    pub(crate) async fn stage_media_attachment_locked(
        &self,
        authenticated: &AuthenticatedBearer,
        create: &MediaAttachmentCreate,
    ) -> Result<i64, WriteError> {
        let (account_id, mut transaction) =
            self.begin_account_write(authenticated, WRITE_MEDIA).await?;
        validate_media_attachment_create(create)?;
        let file_meta = media_meta_with_focus(create.file_meta.clone(), &create.focus)?;
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO media_attachments (
               account_id, type, processing, description, remote_url,
               file_content_type, file_file_name, file_file_size, file_meta,
               file_storage_schema_version, file_updated_at, blurhash, created_at, updated_at
             ) VALUES ($1, $2, 0, $3, '', NULL, NULL, NULL, $4::json, NULL,
                       NULL, $5, clock_timestamp(), clock_timestamp())
             RETURNING id",
        )
        .bind(account_id)
        .bind(create.media_type)
        .bind(create.description.as_deref())
        .bind(file_meta)
        .bind(&create.blurhash)
        .fetch_one(&mut *transaction)
        .await?;
        record_outbox_in(
            &mut transaction,
            &local_media_create_cleanup_job(account_id, id, create)?,
        )
        .await?;
        transaction.commit().await?;
        Ok(id)
    }

    pub(crate) async fn publish_media_attachment_locked(
        &self,
        authenticated: &AuthenticatedBearer,
        id: i64,
        create: &MediaAttachmentCreate,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) =
            self.begin_account_write(authenticated, WRITE_MEDIA).await?;
        validate_media_attachment_create(create)?;
        let updated = sqlx::query(
            "UPDATE media_attachments SET processing = 2, file_content_type = $3,
                 file_file_name = $4, file_file_size = $5, file_storage_schema_version = 1,
                 file_updated_at = clock_timestamp(), updated_at = clock_timestamp()
               WHERE id = $1 AND account_id = $2 AND status_id IS NULL
                 AND remote_url = '' AND file_file_name IS NULL",
        )
        .bind(id)
        .bind(account_id)
        .bind(&create.content_type)
        .bind(&create.file_name)
        .bind(create.file_size)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(WriteError::NotFound);
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn publish_media_attachment(
        &self,
        authenticated: &AuthenticatedBearer,
        id: i64,
        create: &MediaAttachmentCreate,
    ) -> Result<(), WriteError> {
        let account_id = write_account(authenticated, WRITE_MEDIA)?;
        self.with_account_lock(account_id, || async {
            self.publish_media_attachment_locked(authenticated, id, create)
                .await
        })
        .await
    }

    pub async fn update_media_attachment(
        &self,
        authenticated: &AuthenticatedBearer,
        id: i64,
        update: &MediaAttachmentUpdate,
    ) -> Result<(), WriteError> {
        let account_id = write_account(authenticated, WRITE_MEDIA)?;
        self.with_account_lock(account_id, || async {
            self.update_media_attachment_locked(authenticated, id, update)
                .await
        })
        .await
    }

    pub(crate) async fn update_media_attachment_locked(
        &self,
        authenticated: &AuthenticatedBearer,
        id: i64,
        update: &MediaAttachmentUpdate,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) =
            self.begin_account_write(authenticated, WRITE_MEDIA).await?;
        validate_media_attachment_update(update)?;
        // Mastodon 4.6.5 MediaController#update permits pending/in-progress metadata
        // updates. Keep unpublished Rust staging rows and failed processing excluded.
        let current_meta = sqlx::query_scalar::<_, Option<Value>>(
            "SELECT file_meta FROM media_attachments
             WHERE id = $1 AND account_id = $2 AND status_id IS NULL
               AND processing IN (0, 1, 2) AND file_file_name IS NOT NULL
             FOR UPDATE",
        )
        .bind(id)
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let (description_set, description) = match &update.description {
            AccountProfileValue::Unchanged => (false, None),
            AccountProfileValue::Null => (true, None),
            AccountProfileValue::Value(value) => (true, Some(value.as_str())),
        };
        let (focus_set, file_meta) = match update.focus {
            AccountProfileValue::Unchanged => (false, None),
            AccountProfileValue::Null | AccountProfileValue::Value(_) => (
                true,
                Some(media_meta_with_focus(
                    current_meta.unwrap_or_else(|| json!({})),
                    &update.focus,
                )?),
            ),
        };
        sqlx::query(
            "UPDATE media_attachments SET
               description = CASE WHEN $3 THEN $4::text ELSE description END,
               file_meta = CASE WHEN $5 THEN $6::json ELSE file_meta END,
               updated_at = clock_timestamp()
             WHERE id = $1 AND account_id = $2 AND status_id IS NULL",
        )
        .bind(id)
        .bind(account_id)
        .bind(description_set)
        .bind(description)
        .bind(focus_set)
        .bind(file_meta)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn delete_media_attachment(
        &self,
        authenticated: &AuthenticatedBearer,
        id: i64,
    ) -> Result<MediaAttachment, WriteError> {
        let account_id = write_account(authenticated, WRITE_MEDIA)?;
        self.with_account_lock(account_id, || async {
            self.delete_media_attachment_locked(authenticated, id).await
        })
        .await
    }

    pub(crate) async fn delete_media_attachment_locked(
        &self,
        authenticated: &AuthenticatedBearer,
        id: i64,
    ) -> Result<MediaAttachment, WriteError> {
        let (account_id, mut transaction) =
            self.begin_account_write(authenticated, WRITE_MEDIA).await?;
        let query = format!(
            "SELECT {MEDIA_ATTACHMENT_COLUMNS} FROM media_attachments media
             WHERE media.id = $1 AND media.account_id = $2 FOR UPDATE"
        );
        let media = sqlx::query_as::<_, MediaAttachment>(&query)
            .bind(id)
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(WriteError::NotFound)?;
        if media.status_id.is_some() {
            return Err(WriteError::Validation(
                "Media attachment is currently used by a status",
            ));
        }
        let paths = local_media_deletion_paths(&media);
        if paths.is_empty() {
            return Err(WriteError::NotFound);
        }
        #[cfg(feature = "test-support")]
        if self
            .local_media_cleanup_intent_fault
            .as_ref()
            .is_some_and(|fault| fault.swap(false, Ordering::AcqRel))
        {
            return Err(WriteError::InvalidInput(
                "injected local media cleanup intent failure",
            ));
        }
        record_outbox_in(
            &mut transaction,
            &local_media_cleanup_job(account_id, id, "delete", &paths),
        )
        .await?;
        let deleted = sqlx::query(
            "DELETE FROM media_attachments
              WHERE id = $1 AND account_id = $2 AND status_id IS NULL",
        )
        .bind(id)
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        if deleted.rows_affected() != 1 {
            return Err(WriteError::Conflict);
        }
        transaction.commit().await?;
        Ok(media)
    }

    pub async fn update_conversation_unread(
        &self,
        authenticated: &AuthenticatedBearer,
        account_conversation_id: i64,
        unread: bool,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_CONVERSATIONS)
            .await?;
        let (status_ids, lock_version) = sqlx::query_as::<_, (Vec<i64>, i32)>(
            "SELECT status_ids, lock_version FROM account_conversations \
             WHERE account_id = $1 AND id = $2",
        )
        .bind(account_id)
        .bind(account_conversation_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        update_conversation_unread_in(
            &mut transaction,
            account_id,
            account_conversation_id,
            unread,
            status_ids,
            lock_version,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn update_conversation_unread_with_lock_version(
        &self,
        authenticated: &AuthenticatedBearer,
        account_conversation_id: i64,
        unread: bool,
        expected_lock_version: i32,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_CONVERSATIONS)
            .await?;
        let status_ids = sqlx::query_scalar::<_, Vec<i64>>(
            "SELECT status_ids FROM account_conversations \
             WHERE account_id = $1 AND id = $2",
        )
        .bind(account_id)
        .bind(account_conversation_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        update_conversation_unread_in(
            &mut transaction,
            account_id,
            account_conversation_id,
            unread,
            status_ids,
            expected_lock_version,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn delete_conversation(
        &self,
        authenticated: &AuthenticatedBearer,
        account_conversation_id: i64,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_CONVERSATIONS)
            .await?;
        let lock_version = sqlx::query_scalar::<_, i32>(
            "SELECT lock_version FROM account_conversations \
             WHERE account_id = $1 AND id = $2",
        )
        .bind(account_id)
        .bind(account_conversation_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let lock_version = lock_version.ok_or(WriteError::NotFound)?;
        let deleted = sqlx::query(
            "DELETE FROM account_conversations \
             WHERE account_id = $1 AND id = $2 AND lock_version = $3",
        )
        .bind(account_id)
        .bind(account_conversation_id)
        .bind(lock_version)
        .execute(&mut *transaction)
        .await?;
        if deleted.rows_affected() != 1 {
            return Err(WriteError::Conflict);
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_bookmark(
        &self,
        authenticated: &AuthenticatedBearer,
        requested_status_id: i64,
        bookmarked: bool,
    ) -> Result<BookmarkWriteOutcome, WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_BOOKMARKS)
            .await?;
        let status_id = writable_status_id(&mut transaction, requested_status_id).await?;
        let removed = if bookmarked {
            sqlx::query(
                "INSERT INTO bookmarks (account_id, status_id, created_at, updated_at) \
                 VALUES ($1, $2, clock_timestamp(), clock_timestamp()) \
                 ON CONFLICT (account_id, status_id) DO NOTHING",
            )
            .bind(account_id)
            .bind(status_id)
            .execute(&mut *transaction)
            .await?;
            false
        } else {
            sqlx::query("DELETE FROM bookmarks WHERE account_id = $1 AND status_id = $2")
                .bind(account_id)
                .bind(status_id)
                .execute(&mut *transaction)
                .await?
                .rows_affected()
                == 1
        };
        transaction.commit().await?;
        Ok(BookmarkWriteOutcome { status_id, removed })
    }

    pub async fn set_status_mute(
        &self,
        authenticated: &AuthenticatedBearer,
        status_id: i64,
        muted: bool,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) =
            self.begin_account_write(authenticated, WRITE_MUTES).await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        let conversation_id = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT conversation_id FROM statuses \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?
        .ok_or(WriteError::Validation("Mastodon::ValidationError"))?;
        if muted {
            sqlx::query(
                "INSERT INTO conversation_mutes (account_id, conversation_id) \
                 VALUES ($1, $2) ON CONFLICT (account_id, conversation_id) DO NOTHING",
            )
            .bind(account_id)
            .bind(conversation_id)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query(
                "DELETE FROM conversation_mutes \
                 WHERE account_id = $1 AND conversation_id = $2",
            )
            .bind(account_id)
            .bind(conversation_id)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_status_pin(
        &self,
        authenticated: &AuthenticatedBearer,
        status_id: i64,
        pinned: bool,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, super::oauth::WRITE_ACCOUNTS)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        let (status_account_id, visibility, reblog_of_id) =
            sqlx::query_as::<_, (i64, i32, Option<i64>)>(
                "SELECT account_id, visibility, reblog_of_id FROM statuses \
             WHERE id = $1 AND deleted_at IS NULL",
            )
            .bind(status_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(WriteError::NotFound)?;
        if status_account_id != account_id || reblog_of_id.is_some() || visibility == 3 {
            return Err(WriteError::Validation("Mastodon::ValidationError"));
        }
        if pinned {
            let pin_count = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM status_pins WHERE account_id = $1",
            )
            .bind(account_id)
            .fetch_one(&mut *transaction)
            .await?;
            if pin_count >= 5 {
                return Err(WriteError::Validation("Mastodon::ValidationError"));
            }
            sqlx::query(
                "INSERT INTO status_pins (account_id, status_id, created_at, updated_at) \
                 VALUES ($1, $2, clock_timestamp(), clock_timestamp()) \
                 ON CONFLICT (account_id, status_id) DO NOTHING",
            )
            .bind(account_id)
            .bind(status_id)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query("DELETE FROM status_pins WHERE account_id = $1 AND status_id = $2")
                .bind(account_id)
                .bind(status_id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_favourite(
        &self,
        authenticated: &AuthenticatedBearer,
        requested_status_id: i64,
        favourited: bool,
    ) -> Result<FavouriteWriteOutcome, WriteError> {
        self.set_favourite_with_origin(authenticated, requested_status_id, favourited, None, false)
            .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn set_favourite_with_origin(
        &self,
        authenticated: &AuthenticatedBearer,
        requested_status_id: i64,
        favourited: bool,
        origin: Option<&str>,
        limited_federation: bool,
    ) -> Result<FavouriteWriteOutcome, WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, super::oauth::WRITE_FAVOURITES)
            .await?;
        let (status_id, recipient_account_id) =
            writable_status_target(&mut transaction, requested_status_id).await?;
        if favourited {
            let viewer_blocks_author = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM blocks \
                 WHERE account_id = $1 AND target_account_id = $2)",
            )
            .bind(account_id)
            .bind(recipient_account_id)
            .fetch_one(&mut *transaction)
            .await?;
            if !status_favourite_access(viewer_blocks_author).is_allowed() {
                return Err(WriteError::NotFound);
            }
        }
        let remote_delivery = match origin {
            Some(origin) => {
                remote_status_delivery(&mut transaction, account_id, status_id, origin).await?
            }
            None => None,
        };
        if favourited
            && let Some(remote_delivery) = remote_delivery.as_ref()
            && !remote_domain_allowed_in_transaction(
                &mut transaction,
                &remote_delivery.domain,
                limited_federation,
            )
            .await?
        {
            return Err(WriteError::NotFound);
        }
        let activity_id = if favourited {
            let activity_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO favourites (account_id, status_id, created_at, updated_at) \
                 VALUES ($1, $2, clock_timestamp(), clock_timestamp()) \
                 ON CONFLICT (account_id, status_id) DO NOTHING \
                 RETURNING id",
            )
            .bind(account_id)
            .bind(status_id)
            .fetch_optional(&mut *transaction)
            .await?;
            if activity_id.is_some() {
                increment_favourite_count(&mut transaction, status_id).await?;
            }
            activity_id
        } else {
            let activity_id = sqlx::query_scalar::<_, i64>(
                "DELETE FROM favourites WHERE account_id = $1 AND status_id = $2 RETURNING id",
            )
            .bind(account_id)
            .bind(status_id)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some(activity_id) = activity_id {
                decrement_favourite_count(&mut transaction, status_id).await?;
                delete_activity_notifications(
                    &mut transaction,
                    recipient_account_id,
                    activity_id,
                    "Favourite",
                )
                .await?;
            }
            activity_id
        };
        if let Some(activity_id) = activity_id
            && let (Some(_), Some(remote_delivery)) = (origin, remote_delivery.as_ref())
        {
            if favourited {
                record_remote_like_delivery(
                    &mut transaction,
                    account_id,
                    remote_delivery,
                    activity_id,
                )
                .await?;
            } else {
                let like_uri = local_like_activity_uri(&remote_delivery.source_uri, activity_id);
                cancel_activitypub_delivery(&mut transaction, &like_uri).await?;
                record_remote_undo_like_delivery(
                    &mut transaction,
                    account_id,
                    remote_delivery,
                    activity_id,
                )
                .await?;
            }
        }
        if favourited && let Some(activity_id) = activity_id {
            record_outbox_in(
                &mut transaction,
                &notification_job(recipient_account_id, NOTIFICATION_FAVOURITE, activity_id),
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(FavouriteWriteOutcome {
            status_id,
            activity_id,
            recipient_account_id,
        })
    }

    pub async fn set_reblog(
        &self,
        authenticated: &AuthenticatedBearer,
        requested_status_id: i64,
        visibility: Option<&str>,
        reblogged: bool,
    ) -> Result<ReblogWriteOutcome, WriteError> {
        self.set_reblog_with_origin(
            authenticated,
            requested_status_id,
            visibility,
            reblogged,
            None,
            false,
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn remove_reblog_with_origin(
        &self,
        account_id: i64,
        requested_status_id: i64,
        origin: Option<&str>,
    ) -> Result<ReblogWriteOutcome, WriteError> {
        let mut transaction = self.pool.begin().await?;
        ensure_account_write_allowed_in(&mut transaction, account_id).await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock( \
               hashtextextended($1::text || ':' || $2::text, 0))",
        )
        .bind(account_id)
        .bind(requested_status_id)
        .execute(&mut *transaction)
        .await?;
        let existing_status = sqlx::query_as::<_, (i64, NaiveDateTime, i32, i64)>(
            "SELECT boost.id, boost.created_at, boost.visibility, target.account_id \
               FROM statuses boost \
               JOIN statuses target ON target.id = boost.reblog_of_id \
              WHERE boost.account_id = $1 AND boost.reblog_of_id = $2 \
                AND boost.deleted_at IS NULL AND target.deleted_at IS NULL \
              ORDER BY boost.id DESC LIMIT 1 FOR UPDATE",
        )
        .bind(account_id)
        .bind(requested_status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((status_id, created_at, visibility, recipient_account_id)) = existing_status
        else {
            let recipient_account_id = sqlx::query_scalar::<_, i64>(
                "SELECT account_id FROM statuses WHERE id = $1 AND deleted_at IS NULL",
            )
            .bind(requested_status_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(WriteError::NotFound)?;
            transaction.commit().await?;
            return Ok(ReblogWriteOutcome {
                status_id: requested_status_id,
                target_status_id: requested_status_id,
                recipient_account_id,
                created: false,
                removed: false,
                account_statuses_count_before_removal: None,
            });
        };
        let account_statuses_count_before_removal =
            lock_account_statuses_count(&mut transaction, account_id).await?;
        let remote_delivery = match origin {
            Some(origin) => {
                remote_status_delivery(&mut transaction, account_id, requested_status_id, origin)
                    .await?
            }
            None => None,
        };
        sqlx::query(
            "UPDATE statuses SET deleted_at = clock_timestamp(), updated_at = clock_timestamp() \
               WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(status_id)
        .execute(&mut *transaction)
        .await?;
        decrement_account_status_count(&mut transaction, account_id).await?;
        decrement_reblog_count(&mut transaction, requested_status_id).await?;
        delete_activity_notifications(&mut transaction, recipient_account_id, status_id, "Status")
            .await?;
        cancel_status_outbox(&mut transaction, status_id).await?;
        let status_distribution_job = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
            json!({
                "status_id": status_id,
                "activity_type": "Delete"
            }),
        )
        .logical_key(format!("activitypub:status:{status_id}:delete"));
        record_outbox_in(&mut transaction, &status_distribution_job).await?;
        if let Some(remote_delivery) = remote_delivery.as_ref() {
            let announce_uri = local_status_activity_uri(&remote_delivery.source_uri, status_id);
            cancel_activitypub_delivery(&mut transaction, &announce_uri).await?;
            record_remote_undo_announce_delivery(
                &mut transaction,
                account_id,
                remote_delivery,
                status_id,
                created_at,
                visibility,
            )
            .await?;
        }
        record_status_delete_stream_events(&mut transaction, status_id).await?;
        transaction.commit().await?;
        Ok(ReblogWriteOutcome {
            status_id,
            target_status_id: requested_status_id,
            recipient_account_id,
            created: false,
            removed: true,
            account_statuses_count_before_removal: Some(account_statuses_count_before_removal),
        })
    }

    #[allow(clippy::too_many_lines)]
    pub async fn set_reblog_with_origin(
        &self,
        authenticated: &AuthenticatedBearer,
        requested_status_id: i64,
        visibility: Option<&str>,
        reblogged: bool,
        origin: Option<&str>,
        limited_federation: bool,
    ) -> Result<ReblogWriteOutcome, WriteError> {
        let account_id = write_account(authenticated, WRITE_STATUSES)?;
        if !reblogged {
            return self
                .remove_reblog_with_origin(account_id, requested_status_id, origin)
                .await;
        }
        let (_, mut transaction) = self
            .begin_account_write(authenticated, WRITE_STATUSES)
            .await?;
        let mut pending_stream_events = Vec::new();
        let (target_status_id, recipient_account_id, target_visibility) =
            reblog_target(&mut transaction, requested_status_id).await?;
        let (viewer_blocks_author, author_blocks_viewer) = sqlx::query_as::<_, (bool, bool)>(
            "SELECT EXISTS (SELECT 1 FROM blocks \
                WHERE account_id = $1 AND target_account_id = $2), \
                    EXISTS (SELECT 1 FROM blocks \
                WHERE account_id = $2 AND target_account_id = $1)",
        )
        .bind(account_id)
        .bind(recipient_account_id)
        .fetch_one(&mut *transaction)
        .await?;
        if author_blocks_viewer
            || !status_reblog_access(
                StatusVisibility::from(target_visibility),
                account_id == recipient_account_id,
                viewer_blocks_author,
            )
            .is_allowed()
        {
            return Err(WriteError::NotFound);
        }
        let remote_delivery = match origin {
            Some(origin) => {
                remote_status_delivery(&mut transaction, account_id, target_status_id, origin)
                    .await?
            }
            None => None,
        };
        if let Some(remote_delivery) = remote_delivery.as_ref()
            && !remote_domain_allowed_in_transaction(
                &mut transaction,
                &remote_delivery.domain,
                limited_federation,
            )
            .await?
        {
            return Err(WriteError::NotFound);
        }
        sqlx::query(
            "SELECT pg_advisory_xact_lock( \
               hashtextextended($1::text || ':' || $2::text, 0))",
        )
        .bind(account_id)
        .bind(target_status_id)
        .execute(&mut *transaction)
        .await?;
        let existing_status = sqlx::query_as::<_, (i64, NaiveDateTime, i32)>(
            "SELECT id, created_at, visibility FROM statuses \
              WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL \
              ORDER BY id DESC LIMIT 1 FOR UPDATE",
        )
        .bind(account_id)
        .bind(target_status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let existing_status_id = existing_status.as_ref().map(|status| status.0);

        if let Some(status_id) = existing_status_id {
            transaction.commit().await?;
            return Ok(ReblogWriteOutcome {
                status_id,
                target_status_id,
                recipient_account_id,
                created: false,
                removed: false,
                account_statuses_count_before_removal: None,
            });
        }

        let visibility = if target_visibility > 1 {
            target_visibility
        } else if let Some(visibility) = visibility {
            parse_reblog_visibility(visibility)?
        } else {
            default_reblog_visibility(&mut transaction, account_id).await?
        };
        if !matches!(visibility, 0..=2) {
            return Err(WriteError::InvalidInput("invalid reblog visibility"));
        }
        // Initialize an imported/fresh account's missing stats before inserting
        // the wrapper, so the normal increment counts it exactly once.
        lock_account_statuses_count(&mut transaction, account_id).await?;
        let (status_id, created_at) = sqlx::query_as::<_, (i64, NaiveDateTime)>(
            "INSERT INTO statuses ( \
               account_id, text, spoiler_text, visibility, local, uri, url, language, \
               sensitive, reply, ordered_media_attachment_ids, reblog_of_id, \
                created_at, updated_at) \
              VALUES ($1, '', '', $2, true, NULL, NULL, NULL, false, false, NULL, $3, \
                      clock_timestamp(), clock_timestamp()) \
              RETURNING id, created_at",
        )
        .bind(account_id)
        .bind(visibility)
        .bind(target_status_id)
        .fetch_one(&mut *transaction)
        .await?;
        let conversation_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO conversations (created_at, parent_account_id, parent_status_id, updated_at, uri) \
             VALUES (clock_timestamp(), $1, $2, clock_timestamp(), NULL) \
             RETURNING id",
        )
        .bind(account_id)
        .bind(status_id)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("UPDATE statuses SET conversation_id = $1 WHERE id = $2")
            .bind(conversation_id)
            .bind(status_id)
            .execute(&mut *transaction)
            .await?;
        increment_account_status_count(&mut transaction, account_id, created_at).await?;
        increment_reblog_count(&mut transaction, target_status_id).await?;
        let status_distribution_job = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
            json!({
                "status_id": status_id,
                "activity_type": "Create"
            }),
        )
        .logical_key(format!("activitypub:status:{status_id}"));
        record_outbox_in(&mut transaction, &status_distribution_job).await?;
        if let Some(remote_delivery) = remote_delivery.as_ref()
            && origin.is_some()
        {
            record_remote_announce_delivery(
                &mut transaction,
                account_id,
                remote_delivery,
                status_id,
                created_at,
                visibility,
            )
            .await?;
        }
        record_outbox_in(
            &mut transaction,
            &notification_job(recipient_account_id, NOTIFICATION_REBLOG, status_id),
        )
        .await?;
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "update",
            created_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(ReblogWriteOutcome {
            status_id,
            target_status_id,
            recipient_account_id,
            created: true,
            removed: false,
            account_statuses_count_before_removal: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_status(
        &self,
        authenticated: &AuthenticatedBearer,
        text: &str,
        media_ids: &[i64],
        spoiler_text: Option<&str>,
        sensitive: Option<bool>,
        visibility: Option<&str>,
        language: Option<&str>,
        in_reply_to_id: Option<i64>,
        idempotency: Option<IdempotencyKey<'_>>,
    ) -> Result<StatusWriteOutcome, WriteError> {
        self.create_status_with_quote_policy(
            authenticated,
            text,
            media_ids,
            spoiler_text,
            sensitive,
            visibility,
            language,
            None,
            in_reply_to_id,
            None,
            None,
            None,
            false,
            idempotency,
        )
        .await
    }

    /// Creates a status with an optional poll using the same transactional write path as the REST API.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_status_with_poll(
        &self,
        authenticated: &AuthenticatedBearer,
        text: &str,
        media_ids: &[i64],
        spoiler_text: Option<&str>,
        sensitive: Option<bool>,
        visibility: Option<&str>,
        language: Option<&str>,
        in_reply_to_id: Option<i64>,
        poll: Option<&PollCreate>,
        idempotency: Option<IdempotencyKey<'_>>,
    ) -> Result<StatusWriteOutcome, WriteError> {
        self.create_status_with_quote_policy(
            authenticated,
            text,
            media_ids,
            spoiler_text,
            sensitive,
            visibility,
            language,
            None,
            in_reply_to_id,
            None,
            poll,
            None,
            false,
            idempotency,
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) async fn create_status_with_quote_policy(
        &self,
        authenticated: &AuthenticatedBearer,
        text: &str,
        media_ids: &[i64],
        spoiler_text: Option<&str>,
        sensitive: Option<bool>,
        visibility: Option<&str>,
        language: Option<&str>,
        quote_approval_policy: Option<&str>,
        in_reply_to_id: Option<i64>,
        quoted_status_id: Option<i64>,
        poll: Option<&PollCreate>,
        origin: Option<&str>,
        limited_federation: bool,
        idempotency: Option<IdempotencyKey<'_>>,
    ) -> Result<StatusWriteOutcome, WriteError> {
        let poll = poll
            .map(|poll| {
                prepare_local_poll(
                    &poll.options,
                    poll.expires_in,
                    poll.multiple,
                    poll.hide_totals,
                )
                .map_err(WriteError::Validation)
            })
            .transpose()?;
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_STATUSES)
            .await?;
        let mut pending_stream_events = Vec::new();
        let application_id = authenticated.application_id();
        let media_ids = unique_media_ids(media_ids);
        validate_idempotency(idempotency)?;
        let text = text.trim();
        if !quote_post_has_content(
            !text.is_empty(),
            !media_ids.is_empty(),
            quoted_status_id.is_some(),
        ) {
            return Err(WriteError::Validation("status must not be empty"));
        }
        let spoiler_text = spoiler_text.unwrap_or("").trim();
        if let Some(idempotency) = idempotency
            && claim_idempotency(&mut transaction, idempotency).await?
        {
            let status_id = sqlx::query_scalar::<_, i64>(
                "SELECT (result ->> 'status_id')::bigint FROM rustodon.idempotency_keys \
                 WHERE scope = $1 AND key = $2 AND result ? 'status_id'",
            )
            .bind(idempotency.scope)
            .bind(idempotency.key)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(WriteError::Conflict)?;
            transaction.commit().await?;
            return Ok(StatusWriteOutcome { status_id });
        }
        let defaults = sqlx::query_as::<_, (String, bool, Option<String>, String)>(
            "SELECT \
               COALESCE(NULLIF(account_user.settings, '')::jsonb ->> 'default_privacy', \
                        CASE WHEN account.locked THEN 'private' ELSE 'public' END), \
               COALESCE((NULLIF(account_user.settings, '')::jsonb ->> 'default_sensitive')::boolean, false), \
               COALESCE(NULLIF(account_user.settings, '')::jsonb ->> 'default_language', 'en'), \
               COALESCE(NULLIF(account_user.settings, '')::jsonb ->> 'default_quote_policy', 'public') \
             FROM users account_user \
             JOIN accounts account ON account.id = account_user.account_id \
             WHERE account_user.account_id = $1 ORDER BY account_user.id LIMIT 1",
        )
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .unwrap_or_else(|| ("public".to_owned(), false, Some("en".to_owned()), "public".to_owned()));
        let mut visibility = parse_status_visibility(visibility.unwrap_or(&defaults.0))?;
        let quote_target = if let Some(quoted_status_id) = quoted_status_id {
            let target =
                writable_quote_target(&mut transaction, account_id, quoted_status_id).await?;
            visibility = quote_post_visibility(
                StatusVisibility::from(visibility),
                StatusVisibility::from(target.visibility),
            )
            .raw();
            Some(target)
        } else {
            None
        };
        let quote_delivery = match quote_target.as_ref() {
            Some(target) if !target.local => {
                let origin = origin.ok_or(WriteError::NotFound)?;
                Some(
                    remote_status_delivery(&mut transaction, account_id, target.status_id, origin)
                        .await?
                        .ok_or(WriteError::NotFound)?,
                )
            }
            Some(_) | None => None,
        };
        if let Some(delivery) = quote_delivery.as_ref()
            && !remote_domain_allowed_in_transaction(
                &mut transaction,
                &delivery.domain,
                limited_federation,
            )
            .await?
        {
            return Err(WriteError::Validation(
                "remote status domain is not allowed",
            ));
        }
        let sensitive = sensitive.unwrap_or(defaults.1) || !spoiler_text.is_empty();
        let requested_language = language
            .filter(|value| !value.trim().is_empty())
            .and_then(normalize_status_language)
            .or_else(|| defaults.2.as_deref().and_then(normalize_status_language));
        let reply_target = if let Some(in_reply_to_id) = in_reply_to_id {
            Some(writable_reply_target(&mut transaction, account_id, in_reply_to_id).await?)
        } else {
            None
        };
        let language = requested_language
            .or_else(|| reply_target.as_ref().and_then(|target| target.2.clone()));
        validate_status_media(&mut transaction, account_id, &media_ids).await?;
        let quote_approval_policy =
            quote_approval_policy_for_status(visibility, quote_approval_policy, &defaults.3)?;
        let (status_id, status_created_at) = sqlx::query_as::<_, (i64, NaiveDateTime)>(
            "INSERT INTO statuses ( \
               account_id, application_id, text, spoiler_text, visibility, local, language, \
               sensitive, reply, ordered_media_attachment_ids, in_reply_to_id, \
               in_reply_to_account_id, conversation_id, quote_approval_policy, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, true, $6, $7, $8, $9, $10, $11, $12, $13, \
                      clock_timestamp(), clock_timestamp()) \
             RETURNING id, created_at",
        )
        .bind(account_id)
        .bind(application_id)
        .bind(text)
        .bind(spoiler_text)
        .bind(visibility)
        .bind(language.as_deref())
        .bind(sensitive)
        .bind(reply_target.is_some())
        .bind(media_ids.clone())
        .bind(in_reply_to_id)
        .bind(reply_target.as_ref().map(|target| target.0))
        .bind(reply_target.as_ref().and_then(|target| target.1))
        .bind(quote_approval_policy)
        .fetch_one(&mut *transaction)
        .await?;
        if let Some(poll) = poll {
            let (poll_id, expires_at) = sqlx::query_as::<_, (i64, NaiveDateTime)>(
                "INSERT INTO polls (account_id, status_id, options, cached_tallies, votes_count, \
                    voters_count, multiple, hide_totals, expires_at, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, 0, 0, $5, $6, \
                    clock_timestamp() + make_interval(secs => $7::double precision), \
                    clock_timestamp(), clock_timestamp()) RETURNING id, expires_at",
            )
            .bind(account_id)
            .bind(status_id)
            .bind(&poll.options)
            .bind(vec![0_i64; poll.options.len()])
            .bind(poll.multiple)
            .bind(poll.hide_totals)
            .bind(poll.expires_in)
            .fetch_one(&mut *transaction)
            .await?;
            sqlx::query("UPDATE statuses SET poll_id = $1 WHERE id = $2")
                .bind(poll_id)
                .bind(status_id)
                .execute(&mut *transaction)
                .await?;
            let expires_at = expires_at.and_utc();
            let expiration_job = poll_expiration_job(
                poll_id,
                expires_at,
                PollExpirationIntentKind::Initial,
                expires_at,
            );
            record_outbox_once_in(&mut transaction, &expiration_job).await?;
        }
        if reply_target.is_none() {
            let conversation_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO conversations (created_at, parent_account_id, parent_status_id, updated_at, uri) \
                 VALUES (clock_timestamp(), $1, $2, clock_timestamp(), NULL) RETURNING id",
            )
            .bind(account_id)
            .bind(status_id)
            .fetch_one(&mut *transaction)
            .await?;
            sqlx::query("UPDATE statuses SET conversation_id = $1 WHERE id = $2")
                .bind(conversation_id)
                .bind(status_id)
                .execute(&mut *transaction)
                .await?;
        }
        if !media_ids.is_empty() {
            sqlx::query(
                "UPDATE media_attachments SET status_id = $1, updated_at = clock_timestamp() \
                 WHERE account_id = $2 AND status_id IS NULL AND id = ANY($3)",
            )
            .bind(status_id)
            .bind(account_id)
            .bind(media_ids)
            .execute(&mut *transaction)
            .await?;
        }
        update_status_tags(
            &mut transaction,
            status_id,
            account_id,
            visibility,
            status_created_at,
            text,
            &[],
        )
        .await?;
        let mention_targets = insert_status_mentions(
            &mut transaction,
            status_id,
            account_id,
            text,
            self.local_domain.as_deref(),
        )
        .await?;
        let quote = if let Some(target) = quote_target.as_ref() {
            let explicitly_mentions_target = mention_targets
                .iter()
                .any(|(_, recipient_account_id)| *recipient_account_id == target.account_id);
            if !direct_quote_allowed(
                StatusVisibility::from(visibility),
                target.account_id == account_id,
                explicitly_mentions_target,
            ) {
                return Err(WriteError::Validation(
                    "direct quote posts must mention the quoted account",
                ));
            }
            let accepted = target.local;
            let activity_uri = quote_delivery.as_ref().map(|delivery| {
                format!(
                    "{}/quote_requests/{}",
                    delivery.source_uri.trim_end_matches('/'),
                    random_uuid()
                )
            });
            let quote_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO quotes (account_id, activity_uri, approval_uri, created_at, legacy, \
                   quoted_account_id, quoted_status_id, state, status_id, updated_at) \
                 VALUES ($1, $2, NULL, clock_timestamp(), false, $3, $4, $5, $6, clock_timestamp()) \
                 RETURNING id",
            )
            .bind(account_id)
            .bind(&activity_uri)
            .bind(target.account_id)
            .bind(target.status_id)
            .bind(i32::from(accepted))
            .bind(status_id)
            .fetch_one(&mut *transaction)
            .await?;
            sqlx::query(
                "INSERT INTO mentions (id, account_id, created_at, silent, status_id, updated_at) \
                 VALUES (nextval('mentions_id_seq'), $1, clock_timestamp(), true, $2, clock_timestamp()) \
                 ON CONFLICT (account_id, status_id) DO NOTHING",
            )
            .bind(target.account_id)
            .bind(status_id)
            .execute(&mut *transaction)
            .await?;
            if accepted {
                increment_quote_count(&mut transaction, target.status_id).await?;
            }
            Some((quote_id, activity_uri, accepted))
        } else {
            None
        };
        if visibility == 3
            && let Some((conversation_id, lock_version)) =
                upsert_notification_conversation(&mut transaction, account_id, Some(status_id))
                    .await?
        {
            record_conversation_stream_event_in(
                &mut transaction,
                account_id,
                conversation_id,
                lock_version,
            )
            .await?;
        }
        sqlx::query(
            "INSERT INTO status_stats (status_id, created_at, updated_at) \
             VALUES ($1, clock_timestamp(), clock_timestamp())",
        )
        .bind(status_id)
        .execute(&mut *transaction)
        .await?;
        if visibility == 3 {
            ensure_account_stats_after_mutation(&mut transaction, account_id).await?;
        } else {
            increment_account_status_count(&mut transaction, account_id, status_created_at).await?;
        }
        if matches!(visibility, 0 | 1)
            && let Some(in_reply_to_id) = in_reply_to_id
        {
            increment_reply_count(&mut transaction, in_reply_to_id).await?;
        }
        if let Some(idempotency) = idempotency {
            complete_status_idempotency(&mut transaction, idempotency, status_id).await?;
        }
        let status_notification_job = JobSpec::new(
            Lane::Core,
            STATUS_NOTIFICATION_JOB_KIND,
            json!({"status_id": status_id}),
        )
        .logical_key(format!("status-notifications:{status_id}"));
        record_outbox_in(&mut transaction, &status_notification_job).await?;
        for (mention_id, recipient_account_id) in &mention_targets {
            record_outbox_in(
                &mut transaction,
                &notification_job(*recipient_account_id, NOTIFICATION_MENTION, *mention_id),
            )
            .await?;
        }
        if let (Some(target), Some((quote_id, activity_uri, accepted))) =
            (quote_target.as_ref(), quote.as_ref())
        {
            if *accepted {
                record_outbox_in(
                    &mut transaction,
                    &notification_job(target.account_id, NOTIFICATION_QUOTE, *quote_id),
                )
                .await?;
            } else if !target.local {
                let delivery = quote_delivery.as_ref().ok_or(WriteError::NotFound)?;
                let request_uri = activity_uri.as_deref().ok_or(WriteError::NotFound)?;
                let quote_request_job = JobSpec::new(
                    Lane::Push,
                    ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                    json!({
                        "status_id": status_id,
                        "activity_type": "QuoteRequest",
                        "quote_id": quote_id,
                        "quote_request_uri": request_uri,
                        "quoting_status_id": status_id,
                        "quoted_status_id": target.status_id,
                        "quoted_status_uri": delivery.target_uri,
                        "quoted_status_url": delivery.target_url,
                        "quoted_account_id": target.account_id
                    }),
                )
                .logical_key(format!("activitypub:quote-request:{quote_id}"));
                record_outbox_once_in(&mut transaction, &quote_request_job).await?;
            }
        }
        let status_distribution_job = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
            json!({"status_id": status_id}),
        )
        .logical_key(format!("activitypub:status:{status_id}"));
        record_outbox_in(&mut transaction, &status_distribution_job).await?;
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "update",
            status_created_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(StatusWriteOutcome { status_id })
    }

    /// Casts one all-or-nothing ballot while holding a transaction-scoped voter lock.
    #[allow(clippy::similar_names, clippy::too_many_lines)]
    pub async fn vote_poll(
        &self,
        authenticated: &AuthenticatedBearer,
        poll_id: i64,
        choices: &[i32],
        origin: &str,
    ) -> Result<PollVoteWriteOutcome, WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_STATUSES)
            .await?;
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock( \
               pg_catalog.hashtext('rustodon:poll_vote:' || $1::text || ':' || $2::text))",
        )
        .bind(poll_id)
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        let Some((status_id, poll_account_id)) = sqlx::query_as::<_, (i64, i64)>(
            "SELECT status.id, poll.account_id FROM polls poll \
             JOIN statuses status ON status.id = poll.status_id AND status.deleted_at IS NULL \
             WHERE poll.id = $1 FOR SHARE OF status",
        )
        .bind(poll_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Err(WriteError::NotFound);
        };
        authorize_poll_vote(&mut transaction, account_id, status_id, poll_account_id).await?;
        let Some((
            status_id,
            poll_account_id,
            options,
            mut tallies,
            votes_count,
            voters_count,
            multiple,
            hide_totals,
            expires_at,
            author_domain,
        )) = sqlx::query_as::<
            _,
            (
                i64,
                i64,
                Vec<String>,
                Vec<i64>,
                i64,
                Option<i64>,
                bool,
                bool,
                Option<NaiveDateTime>,
                Option<String>,
            ),
        >(
            "SELECT poll.status_id, poll.account_id, poll.options, poll.cached_tallies, \
                    poll.votes_count, poll.voters_count, poll.multiple, poll.hide_totals, \
                    poll.expires_at, author.domain \
                 FROM polls poll \
                 JOIN statuses status ON status.id = poll.status_id AND status.deleted_at IS NULL \
                 JOIN accounts author ON author.id = poll.account_id \
                 WHERE poll.id = $1 AND poll.status_id = $2 FOR UPDATE OF poll",
        )
        .bind(poll_id)
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Err(WriteError::NotFound);
        };
        if choices.is_empty() {
            transaction.commit().await?;
            return Ok(PollVoteWriteOutcome { poll_id, status_id });
        }
        let now = sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()")
            .fetch_one(&mut *transaction)
            .await?;
        if expires_at.is_some_and(|expires_at| expires_at <= now) {
            return Err(WriteError::Validation("The poll has already ended"));
        }
        let mut existing = sqlx::query_scalar::<_, i32>(
            "SELECT choice FROM poll_votes WHERE poll_id = $1 AND account_id = $2 ORDER BY id",
        )
        .bind(poll_id)
        .bind(account_id)
        .fetch_all(&mut *transaction)
        .await?;
        let first_ballot = existing.is_empty();
        for &choice in choices {
            let Ok(index) = usize::try_from(choice) else {
                return Err(WriteError::Validation(
                    "The chosen vote option does not exist",
                ));
            };
            if index >= options.len() {
                return Err(WriteError::Validation(
                    "The chosen vote option does not exist",
                ));
            }
            if account_id == poll_account_id {
                return Err(WriteError::Validation("You cannot vote in your own polls"));
            }
            if (!multiple && !existing.is_empty()) || (multiple && existing.contains(&choice)) {
                return Err(WriteError::Validation(
                    "You have already voted on this poll",
                ));
            }
            existing.push(choice);
        }
        let remote_delivery = if author_domain.is_some() {
            remote_status_delivery(&mut transaction, account_id, status_id, origin).await?
        } else {
            None
        };
        if tallies.len() < options.len() {
            tallies.resize(options.len(), 0);
        }
        let mut inserted_votes = Vec::with_capacity(choices.len());
        for &choice in choices {
            let vote_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO poll_votes (account_id, poll_id, choice, uri, created_at, updated_at) \
                 VALUES ($1, $2, $3, NULL, clock_timestamp(), clock_timestamp()) RETURNING id",
            )
            .bind(account_id)
            .bind(poll_id)
            .bind(choice)
            .fetch_one(&mut *transaction)
            .await?;
            inserted_votes.push((vote_id, choice));
            let index = usize::try_from(choice)
                .map_err(|_| WriteError::Validation("The chosen vote option does not exist"))?;
            tallies[index] = tallies[index].saturating_add(1);
        }
        let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
            "UPDATE polls SET cached_tallies = $2, votes_count = $3, voters_count = $4, \
                lock_version = lock_version + 1, updated_at = clock_timestamp() \
             WHERE id = $1 RETURNING updated_at",
        )
        .bind(poll_id)
        .bind(&tallies)
        .bind(votes_count.saturating_add(i64::try_from(choices.len()).unwrap_or(i64::MAX)))
        .bind(voters_count.map(|count| count.saturating_add(i64::from(first_ballot))))
        .fetch_one(&mut *transaction)
        .await?;
        if let Some(delivery) = remote_delivery.as_ref() {
            for &(vote_id, choice) in &inserted_votes {
                let option = &options[usize::try_from(choice).map_err(|_| {
                    WriteError::Validation("The chosen vote option does not exist")
                })?];
                record_remote_poll_vote_delivery(
                    &mut transaction,
                    account_id,
                    delivery,
                    vote_id,
                    option,
                )
                .await?;
            }
            if let Some(expires_at) = expires_at {
                let expires_at = expires_at.and_utc();
                let expiration_job = poll_expiration_job(
                    poll_id,
                    expires_at,
                    PollExpirationIntentKind::Reschedule,
                    expires_at + ChronoDuration::minutes(5),
                );
                record_outbox_once_in(&mut transaction, &expiration_job).await?;
            }
        }
        if author_domain.is_none() && !hide_totals {
            record_poll_update_distribution(&mut transaction, poll_id, status_id, updated_at)
                .await?;
        }
        let mut pending_stream_events = Vec::new();
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "status.update",
            updated_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(PollVoteWriteOutcome { poll_id, status_id })
    }

    /// Applies an `ActivityPub` Note-shaped vote to a locally authored Question.
    ///
    /// Returns `NotPollVote` when the reply is not addressed to a local poll. Parsed vote
    /// candidates that fail signer or poll authorization checks are consumed fail-closed.
    /// Expired matching votes are also consumed without creating a status or vote.
    #[allow(clippy::similar_names, clippy::too_many_lines)]
    pub async fn apply_remote_poll_vote(
        &self,
        account_id: i64,
        actor_uri: &str,
        vote_uri: &str,
        question_uri: &str,
        option: &str,
        origin: &str,
    ) -> Result<RemotePollVoteOutcome, WriteError> {
        let mut transaction = self.pool.begin().await?;
        if !same_remote_note_host(actor_uri, vote_uri)?
            || !remote_interaction_actor_matches(&mut transaction, account_id, actor_uri, true)
                .await?
        {
            transaction.commit().await?;
            return Ok(RemotePollVoteOutcome::Consumed);
        }
        let origin = origin.trim_end_matches('/');
        let Some((poll_id, status_id, poll_account_id)) =
            sqlx::query_as::<_, (i64, i64, i64)>(
                "SELECT poll.id, status.id, poll.account_id FROM polls poll \
                 JOIN statuses status ON status.id = poll.status_id AND status.deleted_at IS NULL \
                 JOIN accounts author ON author.id = status.account_id AND author.domain IS NULL \
                 WHERE status.uri = $1 OR status.url = $1 \
                    OR $1 = $2 || '/actor/statuses/' || status.id::text \
                    OR $1 = $2 || '/@' || author.username || '/' || status.id::text \
                    OR $1 = $2 || '/users/' || author.username || '/statuses/' || status.id::text \
                    OR $1 = $2 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text \
                 ORDER BY status.id LIMIT 1",
            )
            .bind(question_uri)
            .bind(origin)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            transaction.commit().await?;
            return Ok(RemotePollVoteOutcome::NotPollVote);
        };
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock( \
               pg_catalog.hashtext('rustodon:poll_vote:' || $1::text || ':' || $2::text))",
        )
        .bind(poll_id)
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        let target_still_active = sqlx::query_scalar::<_, i64>(
            "SELECT status.id FROM polls poll \
             JOIN statuses status ON status.id = poll.status_id AND status.deleted_at IS NULL \
             WHERE poll.id = $1 AND status.id = $2 AND poll.account_id = $3 \
             FOR SHARE OF status",
        )
        .bind(poll_id)
        .bind(status_id)
        .bind(poll_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .is_some();
        if !target_still_active {
            transaction.commit().await?;
            return Ok(RemotePollVoteOutcome::Consumed);
        }
        if let Err(error) =
            authorize_poll_vote(&mut transaction, account_id, status_id, poll_account_id).await
        {
            return match error {
                WriteError::NotFound | WriteError::Forbidden => {
                    transaction.commit().await?;
                    Ok(RemotePollVoteOutcome::Consumed)
                }
                error => Err(error),
            };
        }
        let Some((options, mut tallies, votes_count, voters_count, multiple, hide_totals, expires_at)) =
            sqlx::query_as::<
                _,
                (
                    Vec<String>,
                    Vec<i64>,
                    i64,
                    Option<i64>,
                    bool,
                    bool,
                    Option<NaiveDateTime>,
                ),
            >(
                "SELECT options, cached_tallies, votes_count, voters_count, multiple, hide_totals, expires_at \
                 FROM polls WHERE id = $1 FOR UPDATE",
            )
            .bind(poll_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            transaction.commit().await?;
            return Ok(RemotePollVoteOutcome::Consumed);
        };
        let Some(choice) = options.iter().position(|candidate| candidate == option) else {
            transaction.commit().await?;
            return Ok(RemotePollVoteOutcome::NotPollVote);
        };
        let choice = i32::try_from(choice)
            .map_err(|_| WriteError::InvalidInput("remote poll choice is invalid"))?;
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM poll_votes WHERE poll_id = $1 AND account_id = $2 AND uri = $3)",
        )
        .bind(poll_id)
        .bind(account_id)
        .bind(vote_uri)
        .fetch_one(&mut *transaction)
        .await?
        {
            transaction.commit().await?;
            return Ok(RemotePollVoteOutcome::Consumed);
        }
        let now = sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()")
            .fetch_one(&mut *transaction)
            .await?;
        if expires_at.is_some_and(|expires_at| expires_at <= now) {
            transaction.commit().await?;
            return Ok(RemotePollVoteOutcome::Consumed);
        }
        let existing = sqlx::query_scalar::<_, i32>(
            "SELECT choice FROM poll_votes WHERE poll_id = $1 AND account_id = $2 ORDER BY id",
        )
        .bind(poll_id)
        .bind(account_id)
        .fetch_all(&mut *transaction)
        .await?;
        if (!multiple && !existing.is_empty()) || existing.contains(&choice) {
            transaction.commit().await?;
            return Ok(RemotePollVoteOutcome::Consumed);
        }
        if tallies.len() < options.len() {
            tallies.resize(options.len(), 0);
        }
        sqlx::query(
            "INSERT INTO poll_votes (account_id, poll_id, choice, uri, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, clock_timestamp(), clock_timestamp())",
        )
        .bind(account_id)
        .bind(poll_id)
        .bind(choice)
        .bind(vote_uri)
        .execute(&mut *transaction)
        .await?;
        let index = usize::try_from(choice)
            .map_err(|_| WriteError::InvalidInput("remote poll choice is invalid"))?;
        tallies[index] = tallies[index].saturating_add(1);
        let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
            "UPDATE polls SET cached_tallies = $2, votes_count = $3, voters_count = $4, \
                lock_version = lock_version + 1, updated_at = clock_timestamp() \
             WHERE id = $1 RETURNING updated_at",
        )
        .bind(poll_id)
        .bind(&tallies)
        .bind(votes_count.saturating_add(1))
        .bind(voters_count.map(|count| count.saturating_add(i64::from(existing.is_empty()))))
        .fetch_one(&mut *transaction)
        .await?;
        if !hide_totals {
            record_poll_update_distribution(&mut transaction, poll_id, status_id, updated_at)
                .await?;
        }
        let mut pending_stream_events = Vec::new();
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "status.update",
            updated_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(RemotePollVoteOutcome::Consumed)
    }

    /// Expires the poll's current generation. Intended for direct administrative and test use;
    /// durable workers should pass their scheduled generation to [`Self::expire_poll_generation`].
    pub async fn expire_poll(&self, poll_id: i64) -> Result<(), WriteError> {
        self.expire_poll_generation(poll_id, None).await
    }

    /// Updates one existing remote poll expiration through the production reconciliation path in
    /// disposable fixtures.
    ///
    /// # Errors
    ///
    /// Returns an error when the poll is absent or the transactional update fails.
    #[cfg(feature = "test-support")]
    pub async fn update_remote_poll_expiration_for_test(
        &self,
        status_id: i64,
        expires_at: Option<DateTime<Utc>>,
        replacement_tallies: Option<Vec<i64>>,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let (account_id, options, tallies, voters_count, multiple) =
            sqlx::query_as::<_, (i64, Vec<String>, Vec<i64>, Option<i64>, bool)>(
                "SELECT account_id, options, cached_tallies, voters_count, multiple \
             FROM polls WHERE status_id = $1",
            )
            .bind(status_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(WriteError::NotFound)?;
        let poll = RemotePollData {
            options,
            tallies: replacement_tallies.unwrap_or(tallies),
            multiple,
            expires_at: expires_at.map(|expires_at| expires_at.naive_utc()),
            voters_count,
        };
        upsert_remote_poll(
            &mut transaction,
            status_id,
            account_id,
            &poll,
            true,
            false,
            false,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn expire_poll_generation(
        &self,
        poll_id: i64,
        expected_generation: Option<i64>,
    ) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let Some(status_id) = sqlx::query_scalar::<_, i64>(
            "SELECT status.id FROM polls poll \
             JOIN statuses status ON status.id = poll.status_id AND status.deleted_at IS NULL \
             WHERE poll.id = $1 FOR SHARE OF status",
        )
        .bind(poll_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        let Some((owner_id, owner_domain, expires_at, _updated_at)) =
            sqlx::query_as::<_, (i64, Option<String>, Option<NaiveDateTime>, NaiveDateTime)>(
                "SELECT poll.account_id, account.domain, poll.expires_at, poll.updated_at \
                 FROM polls poll JOIN accounts account ON account.id = poll.account_id \
                 WHERE poll.id = $1 AND poll.status_id = $2 FOR UPDATE OF poll",
            )
            .bind(poll_id)
            .bind(status_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        let Some(expires_at) = expires_at.map(|value| value.and_utc()) else {
            transaction.commit().await?;
            return Ok(());
        };
        let generation = poll_expiration_generation(expires_at);
        if expected_generation.is_some_and(|expected| expected != generation) {
            transaction.commit().await?;
            return Ok(());
        }
        let activation = poll_expiration_activation_in(&mut transaction).await?;
        finalize_poll_expiration_generation_in(
            &mut transaction,
            poll_id,
            status_id,
            owner_id,
            owner_domain.is_none(),
            expires_at,
            activation,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub async fn update_status(
        &self,
        authenticated: &AuthenticatedBearer,
        status_id: i64,
        update: &StatusUpdate,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_STATUSES)
            .await?;
        let mut pending_stream_events = Vec::new();
        let (
            current_text,
            current_spoiler_text,
            current_sensitive,
            current_language,
            current_visibility,
            current_ordered_media_ids,
            created_at,
        ) = sqlx::query_as::<
            _,
            (
                String,
                String,
                bool,
                Option<String>,
                i32,
                Option<Vec<i64>>,
                NaiveDateTime,
            ),
        >(
            "SELECT text, spoiler_text, sensitive, language, visibility, \
                    ordered_media_attachment_ids, created_at \
                 FROM statuses \
                 WHERE id = $1 AND account_id = $2 AND deleted_at IS NULL \
                 FOR UPDATE",
        )
        .bind(status_id)
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let timeline_before = status_timeline_snapshot(&mut transaction, status_id).await?;
        let quote_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM quotes WHERE status_id = $1 ORDER BY id LIMIT 1",
        )
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let current_media_ids = match current_ordered_media_ids {
            Some(ids) => ids,
            None => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT id FROM media_attachments WHERE status_id = $1 ORDER BY id",
                )
                .bind(status_id)
                .fetch_all(&mut *transaction)
                .await?
            }
        };
        let snapshot_media_descriptions =
            media_descriptions(&mut transaction, status_id, &current_media_ids).await?;
        let has_existing_edits = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM status_edits WHERE status_id = $1)",
        )
        .bind(status_id)
        .fetch_one(&mut *transaction)
        .await?;
        let previous_tag_ids = sqlx::query_scalar::<_, i64>(
            "SELECT tag_id FROM statuses_tags WHERE status_id = $1 ORDER BY tag_id",
        )
        .bind(status_id)
        .fetch_all(&mut *transaction)
        .await?;
        let next_text = update
            .text
            .as_deref()
            .map_or(current_text.as_str(), str::trim)
            .to_owned();
        let next_spoiler_text = update
            .spoiler_text
            .as_deref()
            .map_or(current_spoiler_text.as_str(), str::trim)
            .to_owned();
        let next_sensitive = if update.sensitive.is_some() || update.spoiler_text.is_some() {
            update.sensitive.unwrap_or(false) || !next_spoiler_text.is_empty()
        } else {
            current_sensitive
        };
        let next_language = update
            .language
            .as_deref()
            .map(str::trim)
            .and_then(|language| {
                (!language.is_empty())
                    .then_some(language)
                    .and_then(normalize_status_language)
            });
        let next_language = next_language
            .or_else(|| current_language.clone())
            .or_else(|| update.language.as_ref().map(|_| "en".to_owned()));
        let next_media_ids = update
            .media_ids
            .as_deref()
            .map_or_else(|| current_media_ids.clone(), unique_media_ids);
        validate_status_media_update(&mut transaction, account_id, status_id, &next_media_ids)
            .await?;
        let media_attributes_changed = update_status_media_attributes(
            &mut transaction,
            account_id,
            status_id,
            &next_media_ids,
            update.media_attributes.as_deref().unwrap_or(&[]),
        )
        .await?;
        let media_changed = current_media_ids != next_media_ids;
        let significant_changes = current_text != next_text
            || current_spoiler_text != next_spoiler_text
            || current_sensitive != next_sensitive
            || current_language != next_language
            || media_changed
            || media_attributes_changed;
        if !significant_changes {
            transaction.commit().await?;
            return Ok(());
        }
        if !has_existing_edits {
            insert_status_edit(
                &mut transaction,
                status_id,
                account_id,
                &current_text,
                &current_spoiler_text,
                current_sensitive,
                &current_media_ids,
                &snapshot_media_descriptions,
                quote_id,
                created_at,
            )
            .await?;
        }
        let old_mentions = if current_text == next_text {
            Vec::new()
        } else {
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT id, account_id FROM mentions WHERE status_id = $1 ORDER BY account_id, id",
            )
            .bind(status_id)
            .fetch_all(&mut *transaction)
            .await?
        };
        if current_text != next_text {
            for (mention_id, recipient_account_id) in &old_mentions {
                cancel_pending_notification_jobs(
                    &mut transaction,
                    *recipient_account_id,
                    *mention_id,
                )
                .await?;
            }
            sqlx::query(
                "UPDATE mentions SET silent = true, updated_at = clock_timestamp() \
                 WHERE status_id = $1",
            )
            .bind(status_id)
            .execute(&mut *transaction)
            .await?;
            sqlx::query("DELETE FROM statuses_tags WHERE status_id = $1")
                .bind(status_id)
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query(
            "UPDATE media_attachments SET status_id = $1, updated_at = clock_timestamp() \
             WHERE account_id = $2 AND (status_id IS NULL OR status_id = $1) AND id = ANY($3)",
        )
        .bind(status_id)
        .bind(account_id)
        .bind(&next_media_ids)
        .execute(&mut *transaction)
        .await?;
        let edited_at = sqlx::query_scalar::<_, NaiveDateTime>(
            "UPDATE statuses SET text = $2, spoiler_text = $3, sensitive = $4, language = $5, \
                ordered_media_attachment_ids = $6, edited_at = clock_timestamp(), \
                updated_at = clock_timestamp() WHERE id = $1 RETURNING edited_at",
        )
        .bind(status_id)
        .bind(&next_text)
        .bind(&next_spoiler_text)
        .bind(next_sensitive)
        .bind(&next_language)
        .bind(&next_media_ids)
        .fetch_one(&mut *transaction)
        .await?;
        if current_text != next_text {
            update_status_tags(
                &mut transaction,
                status_id,
                account_id,
                current_visibility,
                created_at,
                &next_text,
                &previous_tag_ids,
            )
            .await?;
        }
        let mention_targets = if current_text == next_text {
            Vec::new()
        } else {
            insert_status_mentions(
                &mut transaction,
                status_id,
                account_id,
                &next_text,
                self.local_domain.as_deref(),
            )
            .await?
        };
        let next_media_descriptions =
            media_descriptions(&mut transaction, status_id, &next_media_ids).await?;
        insert_status_edit(
            &mut transaction,
            status_id,
            account_id,
            &next_text,
            &next_spoiler_text,
            next_sensitive,
            &next_media_ids,
            &next_media_descriptions,
            quote_id,
            edited_at,
        )
        .await?;
        for (mention_id, recipient_account_id) in &mention_targets {
            record_outbox_in(
                &mut transaction,
                &notification_job(*recipient_account_id, NOTIFICATION_MENTION, *mention_id),
            )
            .await?;
        }
        record_status_update_notifications(
            &mut transaction,
            status_id,
            edited_at.and_utc().timestamp_micros(),
        )
        .await?;
        let poll_updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
            "SELECT updated_at FROM polls WHERE status_id = $1 ORDER BY id LIMIT 1",
        )
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let status_update_job = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
            json!({
                "status_id": status_id,
                "activity_type": "Update",
                "update_kind": "status",
                "update_version_micros": edited_at.and_utc().timestamp_micros(),
                "edited_at_micros": edited_at.and_utc().timestamp_micros(),
                "poll_updated_at_micros": poll_updated_at.map(|value| value.and_utc().timestamp_micros())
            }),
        )
        .logical_key(format!(
            "activitypub:status:{status_id}:update:{}",
            edited_at.and_utc().timestamp_micros()
        ));
        record_outbox_in(&mut transaction, &status_update_job).await?;
        let timeline_after = status_timeline_snapshot(&mut transaction, status_id).await?;
        collect_status_stream_transition(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "status.update",
            StreamEventLogicalKey::Version(edited_at.and_utc().timestamp_micros()),
            Some(timeline_before),
            Some(timeline_after),
        )
        .await?;
        collect_status_update_notification_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            edited_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn update_status_interaction_policy(
        &self,
        authenticated: &AuthenticatedBearer,
        status_id: i64,
        requested_policy: Option<&str>,
    ) -> Result<StatusWriteOutcome, WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_STATUSES)
            .await?;
        let mut pending_stream_events = Vec::new();
        let (owner_id, visibility, reblog_of_id, current_policy) =
            sqlx::query_as::<_, (i64, i32, Option<i64>, i32)>(
                "SELECT account_id, visibility, reblog_of_id, quote_approval_policy
                   FROM statuses
                  WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
            )
            .bind(status_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(WriteError::NotFound)?;
        if owner_id != account_id {
            return Err(WriteError::Forbidden);
        }
        let default_policy = if requested_policy.is_none() {
            sqlx::query_scalar::<_, String>(
                "SELECT COALESCE(NULLIF(account_user.settings, '')::jsonb ->> 'default_quote_policy', 'public')
                   FROM users account_user
                  WHERE account_user.account_id = $1
                  ORDER BY account_user.id LIMIT 1",
            )
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?
            .unwrap_or_else(|| "public".to_owned())
        } else {
            "public".to_owned()
        };
        let requested_policy =
            quote_approval_policy_for_status(visibility, requested_policy, &default_policy)
                .map_err(|_| WriteError::Validation("Quote approval policy is invalid"))?;
        let next_policy = if reblog_of_id.is_some() {
            0
        } else {
            requested_policy
        };
        if next_policy == current_policy {
            transaction.commit().await?;
            return Ok(StatusWriteOutcome { status_id });
        }
        let timeline_before = status_timeline_snapshot(&mut transaction, status_id).await?;
        let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
            "UPDATE statuses
                SET quote_approval_policy = $2, updated_at = clock_timestamp()
              WHERE id = $1
              RETURNING updated_at",
        )
        .bind(status_id)
        .bind(next_policy)
        .fetch_one(&mut *transaction)
        .await?;
        let poll_updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
            "SELECT updated_at FROM polls WHERE status_id = $1 ORDER BY id LIMIT 1",
        )
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let version = updated_at.and_utc().timestamp_micros();
        let update = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
            json!({
                "status_id": status_id,
                "activity_type": "Update",
                "update_kind": "interaction_policy",
                "update_version_micros": version,
                "edited_at_micros": version,
                "poll_updated_at_micros": poll_updated_at.map(|value| value.and_utc().timestamp_micros()),
                "skip_notifications": true
            }),
        )
        .logical_key(format!(
            "activitypub:status:{status_id}:interaction-policy:{version}"
        ));
        record_outbox_once_in(&mut transaction, &update).await?;
        let timeline_after = status_timeline_snapshot(&mut transaction, status_id).await?;
        collect_status_stream_transition(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "status.update",
            StreamEventLogicalKey::Version(version),
            Some(timeline_before),
            Some(timeline_after),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(StatusWriteOutcome { status_id })
    }

    #[allow(clippy::too_many_lines)]
    pub async fn revoke_quote(
        &self,
        authenticated: &AuthenticatedBearer,
        quoted_status_id: i64,
        quoting_status_id: i64,
        origin: &str,
    ) -> Result<StatusWriteOutcome, WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_STATUSES)
            .await?;
        let mut pending_stream_events = Vec::new();
        lock_statuses_in_order(&mut transaction, &[quoted_status_id, quoting_status_id]).await?;
        let target_owner = sqlx::query_scalar::<_, i64>(
            "SELECT account_id FROM statuses \
             WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(quoted_status_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if target_owner != account_id {
            return Err(WriteError::Forbidden);
        }
        let quote = sqlx::query_as::<_, (i64, i32, bool, Option<String>)>(
            "SELECT quote.id, quote.state, quote.legacy, quote.activity_uri FROM quotes quote \
               JOIN statuses quoting ON quoting.id = quote.status_id \
              WHERE quote.quoted_status_id = $1 AND quote.status_id = $2 \
                AND quote.quoted_account_id = $3 AND quoting.deleted_at IS NULL \
              FOR UPDATE OF quote, quoting",
        )
        .bind(quoted_status_id)
        .bind(quoting_status_id)
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let (quote_id, old_state, legacy, request_uri) = quote;
        cancel_quote_request_outbox(&mut transaction, quote_id, request_uri.as_deref()).await?;
        let next_state = if matches!(old_state, 1 | 3) { 3 } else { 2 };
        if next_state != old_state {
            sqlx::query(
                "UPDATE quotes SET state = $2, approval_uri = NULL, updated_at = clock_timestamp() \
                 WHERE id = $1",
            )
            .bind(quote_id)
            .bind(next_state)
            .execute(&mut *transaction)
            .await?;
            if quote_state_update_counter_delta(legacy, old_state, next_state) < 0 {
                decrement_quote_count(&mut transaction, quoted_status_id).await?;
            }
            delete_activity_notifications(&mut transaction, account_id, quote_id, "Quote").await?;
            let quoting_local = sqlx::query_scalar::<_, bool>(
                "SELECT account.domain IS NULL FROM statuses status \
                 JOIN accounts account ON account.id = status.account_id WHERE status.id = $1",
            )
            .bind(quoting_status_id)
            .fetch_one(&mut *transaction)
            .await?;
            let update_at = if quoting_local {
                record_quote_status_update(&mut transaction, quoting_status_id).await?
            } else {
                sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                    .fetch_one(&mut *transaction)
                    .await?
            };
            collect_status_stream_events(
                &mut transaction,
                &mut pending_stream_events,
                quoting_status_id,
                "status.update",
                update_at.and_utc().timestamp_micros(),
            )
            .await?;
            record_quote_authorization_delete(
                &mut transaction,
                quote_id,
                quoting_status_id,
                quoted_status_id,
                account_id,
                origin,
            )
            .await?;
        }
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(StatusWriteOutcome {
            status_id: quoting_status_id,
        })
    }

    #[allow(clippy::too_many_lines)]
    pub async fn delete_status(
        &self,
        authenticated: &AuthenticatedBearer,
        status_id: i64,
        delete_media: bool,
    ) -> Result<Vec<MediaAttachment>, WriteError> {
        self.delete_status_with_origin(authenticated, status_id, delete_media, None)
            .await
    }

    pub async fn delete_status_with_origin(
        &self,
        authenticated: &AuthenticatedBearer,
        status_id: i64,
        delete_media: bool,
        origin: Option<&str>,
    ) -> Result<Vec<MediaAttachment>, WriteError> {
        let account_id = write_account(authenticated, WRITE_STATUSES)?;
        self.delete_status_for_owner(account_id, status_id, delete_media, None, true, origin)
            .await
    }

    /// Deletes a local status on behalf of an authorized moderation account.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::Unauthorized`] when the acting account lacks the
    /// `manage_reports` permission, or the same transactional errors as [`Self::delete_status`].
    pub async fn delete_status_as_moderator(
        &self,
        acting_account_id: i64,
        status_id: i64,
        delete_media: bool,
    ) -> Result<Vec<MediaAttachment>, WriteError> {
        let can_delete_status = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                 SELECT 1 FROM users account_user \
                 JOIN accounts account ON account.id = account_user.account_id \
                 JOIN user_roles role ON role.id = COALESCE(account_user.role_id, -99) \
                 LEFT JOIN user_roles everyone ON everyone.id = -99 \
                 WHERE account.id = $1 AND account.domain IS NULL \
                   AND account.suspended_at IS NULL \
                   AND account_user.confirmed_at IS NOT NULL \
                   AND account_user.approved = true \
                   AND account_user.disabled = false \
                   AND (role.permissions & 1 <> 0 OR \
                        ((role.permissions | COALESCE(everyone.permissions, 0)) & $2 <> 0)) \
             )",
        )
        .bind(acting_account_id)
        .bind(1_i64 << 4)
        .fetch_one(&self.pool)
        .await?;
        if !can_delete_status {
            return Err(WriteError::Unauthorized);
        }
        let owner_account_id = sqlx::query_scalar::<_, i64>(
            "SELECT account_id FROM statuses WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(status_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(WriteError::NotFound)?;
        self.delete_status_for_owner(
            owner_account_id,
            status_id,
            delete_media,
            Some(acting_account_id),
            false,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn delete_status_for_owner(
        &self,
        account_id: i64,
        status_id: i64,
        delete_media: bool,
        audit_account_id: Option<i64>,
        require_active_owner: bool,
        origin: Option<&str>,
    ) -> Result<Vec<MediaAttachment>, WriteError> {
        let mut transaction = self.pool.begin().await?;
        lock_quote_status_deletion(&mut transaction).await?;
        let mut pending_stream_events = Vec::new();
        if require_active_owner {
            ensure_account_write_allowed_in(&mut transaction, account_id).await?;
        }
        let mut quote_lifecycle_status_ids = sqlx::query_scalar::<_, i64>(
            "SELECT quote.status_id FROM quotes quote
               JOIN statuses quoting ON quoting.id = quote.status_id
              WHERE quote.quoted_status_id = $1 AND quoting.deleted_at IS NULL
              ORDER BY quote.status_id",
        )
        .bind(status_id)
        .fetch_all(&mut *transaction)
        .await?;
        quote_lifecycle_status_ids.push(status_id);
        lock_statuses_in_order(&mut transaction, &quote_lifecycle_status_ids).await?;
        let (reblog_of_id, in_reply_to_id, visibility, reported, human_identifier) =
            sqlx::query_as::<_, (Option<i64>, Option<i64>, i32, bool, String)>(
            "SELECT status.reblog_of_id, status.in_reply_to_id, status.visibility, ( \
                 EXISTS (SELECT 1 FROM reports \
                         WHERE target_account_id = $2 AND action_taken_at IS NULL \
                           AND $1 = ANY(status_ids)) \
                 OR EXISTS (SELECT 1 FROM account_warnings \
                            WHERE target_account_id = $2 AND overruled_at IS NULL \
                              AND $1::text = ANY(status_ids)) \
             ) AS reported, CASE WHEN account.domain IS NULL THEN account.username \
                                 ELSE account.username || '@' || account.domain END \
             FROM statuses status JOIN accounts account ON account.id = status.account_id \
             WHERE status.id = $1 AND status.account_id = $2 AND status.deleted_at IS NULL FOR UPDATE",
        )
        .bind(status_id)
        .bind(account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let owned_quote =
            sqlx::query_as::<_, (i64, Option<i64>, Option<i64>, i32, Option<String>, bool)>(
                "SELECT quote.id, quote.quoted_status_id, quote.quoted_account_id, quote.state, \
                    quote.activity_uri, COALESCE(target.domain IS NULL, false) \
               FROM quotes quote LEFT JOIN accounts target ON target.id = quote.quoted_account_id \
              WHERE quote.status_id = $1 FOR UPDATE OF quote",
            )
            .bind(status_id)
            .fetch_optional(&mut *transaction)
            .await?;
        let quoted_by_statuses = sqlx::query_as::<_, (i64, i64, Option<String>)>(
            "SELECT quote.id, quote.status_id, quote.activity_uri FROM quotes quote \
               JOIN statuses quoting ON quoting.id = quote.status_id \
              WHERE quote.quoted_status_id = $1 AND quoting.deleted_at IS NULL \
              ORDER BY quote.id FOR UPDATE OF quote",
        )
        .bind(status_id)
        .fetch_all(&mut *transaction)
        .await?;
        let removed_attachments = if delete_media && !reported {
            let query = format!(
                "SELECT {MEDIA_ATTACHMENT_COLUMNS} FROM media_attachments media
                  WHERE media.status_id = $1 FOR UPDATE"
            );
            sqlx::query_as::<_, MediaAttachment>(&query)
                .bind(status_id)
                .fetch_all(&mut *transaction)
                .await?
        } else {
            Vec::new()
        };
        let reblogs = if reblog_of_id.is_none() {
            sqlx::query_as::<_, (i64, i64, i32)>(
                "SELECT id, account_id, visibility FROM statuses \
                  WHERE reblog_of_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
            )
            .bind(status_id)
            .fetch_all(&mut *transaction)
            .await?
        } else {
            Vec::new()
        };
        let timeline_before = status_timeline_snapshot(&mut transaction, status_id).await?;
        let mut reblog_timeline_before = HashMap::new();
        for (reblog_id, _, _) in &reblogs {
            reblog_timeline_before.insert(
                *reblog_id,
                status_timeline_snapshot(&mut transaction, *reblog_id).await?,
            );
        }
        let reblog_ids = reblogs
            .iter()
            .map(|(reblog_id, _, _)| *reblog_id)
            .collect::<Vec<_>>();
        let local_reblog_ids = sqlx::query_scalar::<_, i64>(
            "SELECT status.id FROM statuses status
               JOIN accounts account ON account.id = status.account_id
              WHERE status.id = ANY($1) AND status.local IS TRUE
                AND account.domain IS NULL
              ORDER BY status.id",
        )
        .bind(&reblog_ids)
        .fetch_all(&mut *transaction)
        .await?;
        let mut remote_recipient_ids = if reblog_ids.is_empty() {
            Vec::new()
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT status.account_id FROM statuses status
                   JOIN accounts account ON account.id = status.account_id
                  WHERE status.id = ANY($1) AND account.domain IS NOT NULL
                    AND account.protocol = 1
                  ORDER BY status.account_id",
            )
            .bind(&reblog_ids)
            .fetch_all(&mut *transaction)
            .await?
        };
        remote_recipient_ids.extend(
            sqlx::query_scalar::<_, i64>(
                "SELECT DISTINCT mention.account_id FROM mentions mention
               JOIN accounts account ON account.id = mention.account_id
              WHERE mention.status_id = $1
                 AND account.domain IS NOT NULL AND account.protocol = 1
               ORDER BY mention.account_id",
            )
            .bind(status_id)
            .fetch_all(&mut *transaction)
            .await?,
        );
        remote_recipient_ids.sort_unstable();
        remote_recipient_ids.dedup();
        let deleted_at =
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut *transaction)
                .await?;
        if let Some((
            quote_id,
            quoted_status_id,
            quoted_account_id,
            state,
            request_uri,
            quoted_account_local,
        )) = owned_quote
        {
            if state == 1
                && let Some(quoted_status_id) = quoted_status_id
            {
                decrement_quote_count(&mut transaction, quoted_status_id).await?;
                if quoted_account_local
                    && let (Some(quoted_account_id), Some(origin)) = (quoted_account_id, origin)
                {
                    record_quote_authorization_delete(
                        &mut transaction,
                        quote_id,
                        status_id,
                        quoted_status_id,
                        quoted_account_id,
                        origin,
                    )
                    .await?;
                }
            }
            if let Some(quoted_account_id) = quoted_account_id {
                delete_activity_notifications(
                    &mut transaction,
                    quoted_account_id,
                    quote_id,
                    "Quote",
                )
                .await?;
            }
            cancel_quote_request_outbox(&mut transaction, quote_id, request_uri.as_deref()).await?;
            // The status is soft-deleted below, so its quote becomes unreachable without
            // requiring DELETE on the Mastodon-owned quotes table.
        }
        if !quoted_by_statuses.is_empty() {
            let quoting_status_ids = quoted_by_statuses
                .iter()
                .map(|(_, quoting_status_id, _)| *quoting_status_id)
                .collect::<Vec<_>>();
            for (quote_id, _, request_uri) in &quoted_by_statuses {
                cancel_quote_request_outbox(&mut transaction, *quote_id, request_uri.as_deref())
                    .await?;
            }
            sqlx::query(
                "UPDATE quotes SET quoted_status_id = NULL, approval_uri = NULL, \
                        updated_at = clock_timestamp() WHERE id = ANY($1::bigint[])",
            )
            .bind(
                quoted_by_statuses
                    .iter()
                    .map(|(quote_id, _, _)| *quote_id)
                    .collect::<Vec<_>>(),
            )
            .execute(&mut *transaction)
            .await?;
            for quoting_status_id in quoting_status_ids {
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    quoting_status_id,
                    "status.update",
                    deleted_at.and_utc().timestamp_micros(),
                )
                .await?;
                if sqlx::query_scalar::<_, bool>(
                    "SELECT account.domain IS NULL FROM statuses status \
                     JOIN accounts account ON account.id = status.account_id \
                     WHERE status.id = $1 AND status.deleted_at IS NULL",
                )
                .bind(quoting_status_id)
                .fetch_optional(&mut *transaction)
                .await?
                .unwrap_or(false)
                {
                    record_quote_status_update(&mut transaction, quoting_status_id).await?;
                }
            }
        }
        cancel_quote_decision_outbox_for_target(&mut transaction, status_id).await?;
        if reblog_of_id.is_none() {
            sqlx::query(
                "UPDATE statuses SET deleted_at = $2, updated_at = $2 \
                 WHERE reblog_of_id = $1 AND deleted_at IS NULL",
            )
            .bind(status_id)
            .bind(deleted_at)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            "UPDATE statuses SET deleted_at = $2, updated_at = $2 \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(status_id)
        .bind(deleted_at)
        .execute(&mut *transaction)
        .await?;
        if delete_media && !reported {
            let cleanup_jobs = removed_attachments
                .iter()
                .filter_map(|media| {
                    let paths = local_media_deletion_paths(media);
                    (!paths.is_empty())
                        .then(|| local_media_cleanup_job(account_id, media.id, "delete", &paths))
                })
                .collect::<Vec<_>>();
            #[cfg(feature = "test-support")]
            if !cleanup_jobs.is_empty()
                && self
                    .local_media_cleanup_intent_fault
                    .as_ref()
                    .is_some_and(|fault| fault.swap(false, Ordering::AcqRel))
            {
                return Err(WriteError::InvalidInput(
                    "injected local media cleanup intent failure",
                ));
            }
            for cleanup_job in &cleanup_jobs {
                record_outbox_in(&mut transaction, cleanup_job).await?;
            }
            sqlx::query("DELETE FROM media_attachments WHERE status_id = $1")
                .bind(status_id)
                .execute(&mut *transaction)
                .await?;
        } else if !reported {
            sqlx::query(
                "UPDATE media_attachments SET status_id = NULL, updated_at = clock_timestamp() \
                 WHERE status_id = $1",
            )
            .bind(status_id)
            .execute(&mut *transaction)
            .await?;
        }
        let mut discarded_status_ids = Vec::with_capacity(reblogs.len() + 1);
        discarded_status_ids.push(status_id);
        discarded_status_ids.extend(reblogs.iter().map(|(reblog_id, _, _)| *reblog_id));
        remove_statuses_from_account_conversations(&mut transaction, &discarded_status_ids).await?;
        sqlx::query("DELETE FROM status_pins WHERE status_id = ANY($1)")
            .bind(&discarded_status_ids)
            .execute(&mut *transaction)
            .await?;
        let mut status_deltas = HashMap::new();
        add_account_stats_delta(&mut status_deltas, account_id, AccountStatsDelta::default());
        if visibility != 3 {
            add_account_stats_delta(
                &mut status_deltas,
                account_id,
                AccountStatsDelta {
                    statuses: -1,
                    ..AccountStatsDelta::default()
                },
            );
            if matches!(visibility, 0 | 1)
                && let Some(in_reply_to_id) = in_reply_to_id
            {
                decrement_reply_count(&mut transaction, in_reply_to_id).await?;
            }
        }
        if let Some(reblog_of_id) = reblog_of_id {
            decrement_reblog_count(&mut transaction, reblog_of_id).await?;
        } else {
            for (_, reblog_account_id, reblog_visibility) in &reblogs {
                if *reblog_visibility != 3 {
                    add_account_stats_delta(
                        &mut status_deltas,
                        *reblog_account_id,
                        AccountStatsDelta {
                            statuses: -1,
                            ..AccountStatsDelta::default()
                        },
                    );
                }
                decrement_reblog_count(&mut transaction, status_id).await?;
            }
        }
        apply_account_stats_deltas(&mut transaction, status_deltas).await?;
        for reblog_id in local_reblog_ids {
            record_status_delete_distribution(&mut transaction, reblog_id, &[]).await?;
        }
        record_status_delete_distribution(&mut transaction, status_id, &remote_recipient_ids)
            .await?;
        for (reblog_id, _, _) in &reblogs {
            collect_status_delete_stream_events_with_snapshot(
                &mut transaction,
                &mut pending_stream_events,
                *reblog_id,
                reblog_timeline_before
                    .remove(reblog_id)
                    .expect("locked reblog has a route snapshot"),
            )
            .await?;
        }
        collect_status_delete_stream_events_with_snapshot(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            timeline_before,
        )
        .await?;
        if let Some(audit_account_id) = audit_account_id {
            sqlx::query(
                 "INSERT INTO admin_action_logs ( \
                     account_id, action, created_at, human_identifier, route_param, target_id, \
                     target_type, updated_at) \
                  VALUES ($1, 'destroy', clock_timestamp(), $2, NULL, $3, 'Status', clock_timestamp())",
            )
            .bind(audit_account_id)
            .bind(human_identifier)
            .bind(status_id)
            .execute(&mut *transaction)
            .await?;
        }
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(removed_attachments)
    }

    #[allow(clippy::too_many_lines)]
    pub async fn set_follow(
        &self,
        authenticated: &AuthenticatedBearer,
        target_account_id: i64,
        following: bool,
        reblogs: Option<bool>,
        notify: Option<bool>,
        languages: Option<Vec<String>>,
    ) -> Result<FollowWriteOutcome, WriteError> {
        self.set_follow_with_origin(
            authenticated,
            target_account_id,
            following,
            reblogs,
            notify,
            languages,
            None,
            false,
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn set_follow_with_origin(
        &self,
        authenticated: &AuthenticatedBearer,
        target_account_id: i64,
        following: bool,
        reblogs: Option<bool>,
        notify: Option<bool>,
        languages: Option<Vec<String>>,
        origin: Option<&str>,
        limited_federation: bool,
    ) -> Result<FollowWriteOutcome, WriteError> {
        let account_id = write_account(authenticated, WRITE_FOLLOWS)?;
        if account_id == target_account_id {
            return Err(WriteError::NotFound);
        }
        let mut transaction = self
            .begin_relationship_account_write(account_id, target_account_id)
            .await?;
        let (target_local, target_locked, target_unavailable, source_silenced) =
            relationship_target(&mut transaction, account_id, target_account_id).await?;
        if target_unavailable {
            return Err(WriteError::NotFound);
        }
        if following
            && (relationship_is_blocked(&mut transaction, account_id, target_account_id).await?
                || account_domain_is_blocked(&mut transaction, account_id, target_account_id)
                    .await?)
        {
            return Err(WriteError::NotFound);
        }
        let mut activity_id = None;
        let mut request_follow = false;
        let mut activity_uri = None;
        let remote_delivery = match (origin, target_local) {
            (Some(origin), false) => {
                remote_relationship_delivery(
                    &mut transaction,
                    account_id,
                    target_account_id,
                    origin,
                )
                .await?
            }
            _ => None,
        };
        if let Some(remote_delivery) = remote_delivery.as_ref()
            && !remote_domain_allowed_in_transaction(
                &mut transaction,
                &remote_delivery.domain,
                limited_federation,
            )
            .await?
        {
            return Err(WriteError::NotFound);
        }

        if following {
            let existing_follow = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM follows \
                 WHERE account_id = $1 AND target_account_id = $2 FOR UPDATE",
            )
            .bind(account_id)
            .bind(target_account_id)
            .fetch_optional(&mut *transaction)
            .await?;
            let request =
                existing_follow.is_none() && (!target_local || target_locked || source_silenced);
            request_follow = request;
            if request {
                let existing = sqlx::query_scalar::<_, i64>(
                    "SELECT id FROM follow_requests \
                     WHERE account_id = $1 AND target_account_id = $2 FOR UPDATE",
                )
                .bind(account_id)
                .bind(target_account_id)
                .fetch_optional(&mut *transaction)
                .await?;
                if existing.is_some() {
                    update_follow_options(
                        &mut transaction,
                        "follow_requests",
                        account_id,
                        target_account_id,
                        reblogs,
                        notify,
                        languages,
                    )
                    .await?;
                } else {
                    let new_activity_id = sqlx::query_scalar::<_, i64>(
                        "INSERT INTO follow_requests ( \
                           account_id, target_account_id, show_reblogs, notify, languages, \
                           uri, created_at, updated_at) \
                         VALUES ($1, $2, COALESCE($3, true), COALESCE($4, false), $5, NULL, \
                                 clock_timestamp(), clock_timestamp()) RETURNING id",
                    )
                    .bind(account_id)
                    .bind(target_account_id)
                    .bind(reblogs)
                    .bind(notify)
                    .bind(languages)
                    .fetch_one(&mut *transaction)
                    .await?;
                    activity_id = Some(new_activity_id);
                    if let (Some(origin), Some(remote_delivery)) = (origin, &remote_delivery) {
                        let follow_uri = local_follow_activity_uri(
                            origin,
                            account_id,
                            target_account_id,
                            new_activity_id,
                            true,
                        );
                        sqlx::query(
                            "UPDATE follow_requests SET uri = $3, updated_at = clock_timestamp() \
                             WHERE account_id = $1 AND target_account_id = $2",
                        )
                        .bind(account_id)
                        .bind(target_account_id)
                        .bind(&follow_uri)
                        .execute(&mut *transaction)
                        .await?;
                        record_remote_follow_delivery(
                            &mut transaction,
                            account_id,
                            remote_delivery,
                            &follow_uri,
                        )
                        .await?;
                        activity_uri = Some(follow_uri);
                    }
                }
            } else if existing_follow.is_some() {
                update_follow_options(
                    &mut transaction,
                    "follows",
                    account_id,
                    target_account_id,
                    reblogs,
                    notify,
                    languages,
                )
                .await?;
            } else {
                let new_activity_id = sqlx::query_scalar::<_, i64>(
                    "INSERT INTO follows ( \
                       account_id, target_account_id, show_reblogs, notify, languages, \
                       uri, created_at, updated_at) \
                     VALUES ($1, $2, COALESCE($3, true), COALESCE($4, false), $5, NULL, \
                             clock_timestamp(), clock_timestamp()) RETURNING id",
                )
                .bind(account_id)
                .bind(target_account_id)
                .bind(reblogs)
                .bind(notify)
                .bind(languages)
                .fetch_one(&mut *transaction)
                .await?;
                activity_id = Some(new_activity_id);
                increment_follow_counts(&mut transaction, account_id, target_account_id).await?;
                if let (Some(origin), Some(remote_delivery)) = (origin, &remote_delivery) {
                    let follow_uri = local_follow_activity_uri(
                        origin,
                        account_id,
                        target_account_id,
                        new_activity_id,
                        false,
                    );
                    sqlx::query(
                        "UPDATE follows SET uri = $3, updated_at = clock_timestamp() \
                         WHERE account_id = $1 AND target_account_id = $2",
                    )
                    .bind(account_id)
                    .bind(target_account_id)
                    .bind(&follow_uri)
                    .execute(&mut *transaction)
                    .await?;
                    record_remote_follow_delivery(
                        &mut transaction,
                        account_id,
                        remote_delivery,
                        &follow_uri,
                    )
                    .await?;
                    activity_uri = Some(follow_uri);
                }
            }
        } else {
            let deleted_follow = sqlx::query_as::<_, (i64, Option<String>)>(
                "DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2 \
                 RETURNING id, uri",
            )
            .bind(account_id)
            .bind(target_account_id)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some((follow_id, follow_uri)) = deleted_follow {
                decrement_follow_counts(&mut transaction, account_id, target_account_id).await?;
                delete_activity_notifications(
                    &mut transaction,
                    target_account_id,
                    follow_id,
                    "Follow",
                )
                .await?;
                if let (Some(origin), Some(remote_delivery)) = (origin, &remote_delivery) {
                    let follow_uri = follow_uri.unwrap_or_else(|| {
                        local_follow_activity_uri(
                            origin,
                            account_id,
                            target_account_id,
                            follow_id,
                            false,
                        )
                    });
                    cancel_activitypub_delivery(&mut transaction, &follow_uri).await?;
                    record_remote_undo_follow_delivery(
                        &mut transaction,
                        account_id,
                        remote_delivery,
                        &follow_uri,
                        origin,
                    )
                    .await?;
                    activity_uri = Some(follow_uri);
                }
            } else {
                let request = sqlx::query_as::<_, (i64, Option<String>)>(
                    "DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2 \
                     RETURNING id, uri",
                )
                .bind(account_id)
                .bind(target_account_id)
                .fetch_optional(&mut *transaction)
                .await?;
                if let Some((request_id, follow_uri)) = request {
                    delete_activity_notifications(
                        &mut transaction,
                        target_account_id,
                        request_id,
                        "FollowRequest",
                    )
                    .await?;
                    if let (Some(origin), Some(remote_delivery)) = (origin, &remote_delivery) {
                        let follow_uri = follow_uri.unwrap_or_else(|| {
                            local_follow_activity_uri(
                                origin,
                                account_id,
                                target_account_id,
                                request_id,
                                true,
                            )
                        });
                        cancel_activitypub_delivery(&mut transaction, &follow_uri).await?;
                        record_remote_undo_follow_delivery(
                            &mut transaction,
                            account_id,
                            remote_delivery,
                            &follow_uri,
                            origin,
                        )
                        .await?;
                        activity_uri = Some(follow_uri);
                    }
                }
            }
        }
        if following && let Some(activity_id) = activity_id {
            let activity_type = if request_follow {
                "follow_request"
            } else {
                "follow"
            };
            record_outbox_in(
                &mut transaction,
                &notification_job(target_account_id, activity_type, activity_id),
            )
            .await?;
        }
        ensure_relationship_account_stats(&mut transaction, account_id, target_account_id).await?;
        transaction.commit().await?;
        Ok(FollowWriteOutcome {
            activity_id,
            recipient_account_id: target_account_id,
            request: request_follow,
            activity_uri,
        })
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) async fn apply_remote_quote_decision(
        &self,
        source_account_id: i64,
        actor_uri: &str,
        request_uri: &str,
        request_actor_uri: Option<&str>,
        quoted_status_uri: Option<&str>,
        instrument_uri: Option<&str>,
        result_uri: Option<&str>,
        accepted: bool,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<bool, WriteError> {
        if accepted {
            let result_uri = result_uri.ok_or(WriteError::InvalidInput(
                "quote Accept has no authorization result",
            ))?;
            if !same_remote_note_host(actor_uri, result_uri)? {
                return Err(WriteError::InvalidInput(
                    "quote authorization host does not match its actor",
                ));
            }
        }
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, request_uri).await?;
        if accepted {
            lock_remote_interaction(
                &mut transaction,
                result_uri.expect("accepted quote decision has a result"),
            )
            .await?;
        }
        if !remote_interaction_actor_matches(&mut transaction, source_account_id, actor_uri, true)
            .await?
        {
            transaction.commit().await?;
            return Ok(false);
        }
        let quote = sqlx::query_as::<_, (i64, i64, i64, i64, i32, Option<String>, bool)>(
            "SELECT quote.id, quote.status_id, quote.account_id, quote.quoted_status_id, \
                    quote.state, quote.approval_uri, quote.legacy \
               FROM quotes quote \
               JOIN statuses instrument ON instrument.id = quote.status_id \
               JOIN accounts quoter ON quoter.id = quote.account_id \
              WHERE quote.activity_uri = $1 AND quote.quoted_account_id = $2 \
                AND instrument.local IS TRUE AND instrument.deleted_at IS NULL \
              ORDER BY quote.id LIMIT 1 FOR UPDATE OF quote, instrument",
        )
        .bind(request_uri)
        .bind(source_account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((
            quote_id,
            status_id,
            quoting_account_id,
            quoted_status_id,
            old_state,
            old_approval_uri,
            legacy,
        )) = quote
        else {
            transaction.commit().await?;
            return Ok(false);
        };
        if delivery_target_account_id.is_some_and(|id| id != quoting_account_id) {
            transaction.commit().await?;
            return Ok(true);
        }
        if let Some(request_actor_uri) = request_actor_uri {
            let expected =
                local_actor_uri_for_account(&mut transaction, quoting_account_id, origin).await?;
            if request_actor_uri != expected {
                return Err(WriteError::InvalidInput(
                    "embedded QuoteRequest actor does not match the quote author",
                ));
            }
        }
        if let Some(instrument_uri) = instrument_uri
            && !quote_target_matches_uri(&mut transaction, status_id, instrument_uri, origin)
                .await?
        {
            return Err(WriteError::InvalidInput(
                "embedded QuoteRequest instrument does not match the quote",
            ));
        }
        if let Some(quoted_status_uri) = quoted_status_uri
            && !quote_target_matches_uri(
                &mut transaction,
                quoted_status_id,
                quoted_status_uri,
                origin,
            )
            .await?
        {
            return Err(WriteError::InvalidInput(
                "embedded QuoteRequest object does not match the quote target",
            ));
        }
        if accepted
            && remote_interaction_tombstoned(
                &mut transaction,
                source_account_id,
                result_uri.expect("accepted quote decision has a result"),
            )
            .await?
        {
            let next_state = match old_state {
                0 => 2,
                1 => 3,
                _ => old_state,
            };
            if next_state != old_state || old_approval_uri.is_some() {
                sqlx::query(
                    "UPDATE quotes SET state = $2, approval_uri = NULL, updated_at = clock_timestamp() \
                     WHERE id = $1",
                )
                .bind(quote_id)
                .bind(next_state)
                .execute(&mut *transaction)
                .await?;
                if quote_state_update_counter_delta(legacy, old_state, next_state) < 0 {
                    decrement_quote_count(&mut transaction, quoted_status_id).await?;
                }
                delete_activity_notifications(
                    &mut transaction,
                    source_account_id,
                    quote_id,
                    "Quote",
                )
                .await?;
                let edited_at = record_quote_status_update(&mut transaction, status_id).await?;
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    status_id,
                    "status.update",
                    edited_at.and_utc().timestamp_micros(),
                )
                .await?;
            }
            cancel_quote_request_outbox(&mut transaction, quote_id, Some(request_uri)).await?;
            flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
            transaction.commit().await?;
            return Ok(true);
        }
        if accepted && !matches!(old_state, 0 | 1) {
            return Err(WriteError::Conflict);
        }
        let next_state = if accepted {
            if old_state == 0 { 1 } else { old_state }
        } else if matches!(old_state, 1 | 3) {
            3
        } else {
            2
        };
        let next_approval_uri = if accepted && next_state == 1 {
            result_uri.map(ToOwned::to_owned)
        } else {
            None
        };
        if accepted && old_state == 1 && old_approval_uri != next_approval_uri {
            return Err(WriteError::Conflict);
        }
        if next_state != old_state || next_approval_uri != old_approval_uri {
            sqlx::query(
                "UPDATE quotes SET state = $2, approval_uri = $3, updated_at = clock_timestamp() \
                 WHERE id = $1",
            )
            .bind(quote_id)
            .bind(next_state)
            .bind(&next_approval_uri)
            .execute(&mut *transaction)
            .await?;
            if old_state != 1 && next_state == 1 {
                increment_quote_count(&mut transaction, quoted_status_id).await?;
            } else if old_state == 1 && next_state != 1 {
                decrement_quote_count(&mut transaction, quoted_status_id).await?;
            }
            if next_state == 1 {
                let quoted_account_local = sqlx::query_scalar::<_, bool>(
                    "SELECT domain IS NULL FROM accounts WHERE id = $1",
                )
                .bind(source_account_id)
                .fetch_one(&mut *transaction)
                .await?;
                if quoted_account_local {
                    record_outbox_in(
                        &mut transaction,
                        &notification_job(source_account_id, NOTIFICATION_QUOTE, quote_id),
                    )
                    .await?;
                }
            } else {
                delete_activity_notifications(
                    &mut transaction,
                    source_account_id,
                    quote_id,
                    "Quote",
                )
                .await?;
            }
            let edited_at = record_quote_status_update(&mut transaction, status_id).await?;
            collect_status_stream_events(
                &mut transaction,
                &mut pending_stream_events,
                status_id,
                "status.update",
                edited_at.and_utc().timestamp_micros(),
            )
            .await?;
        }
        cancel_quote_request_outbox(&mut transaction, quote_id, Some(request_uri)).await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(true)
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_quote_authorization(
        &self,
        quoting_account_id: i64,
        quoting_uri: &str,
        approval_uri: &str,
        document: &Value,
        origin: &str,
    ) -> Result<bool, WriteError> {
        let authorization = remote_quote_authorization_data(document)?;
        if authorization.uri != approval_uri || !authorization.typed {
            return Err(WriteError::InvalidInput(
                "remote QuoteAuthorization identity or type is invalid",
            ));
        }
        let attributed_to =
            authorization
                .attributed_to
                .as_deref()
                .ok_or(WriteError::InvalidInput(
                    "remote QuoteAuthorization has no attributed actor",
                ))?;
        if !same_remote_note_host(attributed_to, approval_uri)? {
            return Err(WriteError::InvalidInput(
                "remote QuoteAuthorization host does not match its actor",
            ));
        }
        let interacting_object =
            authorization
                .interacting_object
                .as_deref()
                .ok_or(WriteError::InvalidInput(
                    "remote QuoteAuthorization has no interacting object",
                ))?;
        let interaction_target =
            authorization
                .interaction_target
                .as_deref()
                .ok_or(WriteError::InvalidInput(
                    "remote QuoteAuthorization has no interaction target",
                ))?;
        let mut transaction = self.pool.begin().await?;
        lock_remote_interaction(&mut transaction, approval_uri).await?;
        let status_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses \
             WHERE account_id = $2 AND deleted_at IS NULL AND (uri = $1 OR url = $1) \
             ORDER BY id LIMIT 1",
        )
        .bind(quoting_uri)
        .bind(quoting_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let quoted_status_id = sqlx::query_scalar::<_, i64>(
            "SELECT quoted_status_id FROM quotes \
             WHERE status_id = $1 AND quoted_status_id IS NOT NULL",
        )
        .bind(status_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind(vec![status_id, quoted_status_id])
        .fetch_all(&mut *transaction)
        .await?;
        let (
            quote_id,
            state,
            old_approval_uri,
            quoted_account_id,
            quoted_actor_uri,
            legacy,
            request_uri,
        ) = sqlx::query_as::<_, (i64, i32, Option<String>, i64, String, bool, Option<String>)>(
            "SELECT quote.id, quote.state, quote.approval_uri, quote.quoted_account_id, \
                        quoted_account.uri, quote.legacy, quote.activity_uri \
                   FROM quotes quote \
                   JOIN statuses quoting ON quoting.id = quote.status_id \
                   JOIN statuses quoted ON quoted.id = quote.quoted_status_id \
                   JOIN accounts quoted_account ON quoted_account.id = quote.quoted_account_id \
                  WHERE quote.status_id = $1 AND quote.quoted_status_id = $2 \
                    AND quoting.deleted_at IS NULL AND quoted.deleted_at IS NULL \
                    AND quoted_account.domain IS NOT NULL \
                  FOR UPDATE OF quote",
        )
        .bind(status_id)
        .bind(quoted_status_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if attributed_to != quoted_actor_uri
            || !quote_target_matches_uri(&mut transaction, status_id, interacting_object, origin)
                .await?
            || !quote_target_matches_uri(
                &mut transaction,
                quoted_status_id,
                interaction_target,
                origin,
            )
            .await?
        {
            return Err(WriteError::InvalidInput(
                "remote QuoteAuthorization does not match its quote",
            ));
        }
        if remote_interaction_tombstoned(&mut transaction, quoted_account_id, approval_uri).await? {
            transaction.commit().await?;
            return Ok(false);
        }
        cancel_quote_request_outbox(&mut transaction, quote_id, request_uri.as_deref()).await?;
        if state == 1 {
            let replay = old_approval_uri.as_deref() == Some(approval_uri);
            transaction.commit().await?;
            return Ok(replay);
        }
        if state != 0 {
            transaction.commit().await?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE quotes SET state = 1, approval_uri = $2, updated_at = clock_timestamp() \
             WHERE id = $1 AND state = 0",
        )
        .bind(quote_id)
        .bind(approval_uri)
        .execute(&mut *transaction)
        .await?;
        if quote_state_update_counter_delta(legacy, state, 1) > 0 {
            increment_quote_count(&mut transaction, quoted_status_id).await?;
        }
        if sqlx::query_scalar::<_, bool>("SELECT domain IS NULL FROM accounts WHERE id = $1")
            .bind(quoted_account_id)
            .fetch_one(&mut *transaction)
            .await?
        {
            record_outbox_in(
                &mut transaction,
                &notification_job(quoted_account_id, NOTIFICATION_QUOTE, quote_id),
            )
            .await?;
        }
        let mut pending_stream_events = Vec::new();
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "status.update",
            Utc::now().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub(crate) async fn apply_remote_quote_authorization_delete(
        &self,
        source_account_id: i64,
        actor_uri: &str,
        authorization_uri: &str,
        forwarding_activity: Option<&Value>,
    ) -> Result<bool, WriteError> {
        if !same_remote_note_host(actor_uri, authorization_uri)? {
            return Err(WriteError::InvalidInput(
                "QuoteAuthorization Delete host does not match its actor",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, authorization_uri).await?;
        if !remote_interaction_actor_matches(&mut transaction, source_account_id, actor_uri, false)
            .await?
        {
            transaction.commit().await?;
            return Ok(false);
        }
        insert_remote_note_tombstone(&mut transaction, source_account_id, authorization_uri)
            .await?;
        let quote = sqlx::query_as::<_, (i64, i64, i64, i32, bool, Option<String>)>(
            "SELECT quote.id, quote.status_id, quote.quoted_status_id, quote.state, quote.legacy, \
                    quote.activity_uri \
               FROM quotes quote \
               JOIN statuses status ON status.id = quote.status_id \
              WHERE quote.approval_uri = $1 AND quote.quoted_account_id = $2 \
                AND quote.state IN (0, 1) AND status.deleted_at IS NULL \
              ORDER BY quote.id LIMIT 1 FOR UPDATE OF quote, status",
        )
        .bind(authorization_uri)
        .bind(source_account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((quote_id, status_id, quoted_status_id, old_state, legacy, request_uri)) = quote
        else {
            transaction.commit().await?;
            return Ok(false);
        };
        cancel_quote_request_outbox(&mut transaction, quote_id, request_uri.as_deref()).await?;
        if let Some(activity) = forwarding_activity {
            record_remote_quote_authorization_forwarding_in(
                &mut transaction,
                source_account_id,
                actor_uri,
                status_id,
                activity,
            )
            .await?;
        }
        let next_state = if old_state == 1 { 3 } else { 2 };
        sqlx::query(
            "UPDATE quotes SET state = $2, approval_uri = NULL, updated_at = clock_timestamp() \
             WHERE id = $1",
        )
        .bind(quote_id)
        .bind(next_state)
        .execute(&mut *transaction)
        .await?;
        if quote_state_update_counter_delta(legacy, old_state, next_state) < 0 {
            decrement_quote_count(&mut transaction, quoted_status_id).await?;
        }
        delete_activity_notifications(&mut transaction, source_account_id, quote_id, "Quote")
            .await?;
        let status_is_local = sqlx::query_scalar::<_, bool>(
            "SELECT account.domain IS NULL FROM statuses status \
             JOIN accounts account ON account.id = status.account_id WHERE status.id = $1",
        )
        .bind(status_id)
        .fetch_one(&mut *transaction)
        .await?;
        let updated_at = if status_is_local {
            record_quote_status_update(&mut transaction, status_id).await?
        } else {
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut *transaction)
                .await?
        };
        collect_status_stream_events(
            &mut transaction,
            &mut pending_stream_events,
            status_id,
            "status.update",
            updated_at.and_utc().timestamp_micros(),
        )
        .await?;
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn remote_quote_request_may_import(
        &self,
        source_account_id: i64,
        request_uri: &str,
        actor_uri: &str,
        quoted_status_uri: &str,
        instrument_uri: &str,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<Option<(i64, i64)>, WriteError> {
        if !same_remote_note_host(actor_uri, request_uri)?
            || !same_remote_note_host(actor_uri, instrument_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote QuoteRequest identifiers do not match its actor host",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        lock_remote_interaction(&mut transaction, request_uri).await?;
        if !remote_interaction_actor_matches(&mut transaction, source_account_id, actor_uri, true)
            .await?
        {
            transaction.commit().await?;
            return Ok(None);
        }
        if remote_quote_request_decision_in(
            &mut transaction,
            request_uri,
            actor_uri,
            quoted_status_uri,
            instrument_uri,
        )
        .await?
        .is_some()
        {
            transaction.commit().await?;
            return Ok(None);
        }
        let Some((target_status_id, target_account_id, target_local, _)) =
            resolve_quote_target(&mut transaction, quoted_status_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        if !target_local || delivery_target_account_id.is_some_and(|id| id != target_account_id) {
            transaction.commit().await?;
            return Ok(None);
        }
        match writable_quote_target(&mut transaction, source_account_id, target_status_id).await {
            Ok(_) => {
                transaction.commit().await?;
                Ok(Some((target_status_id, target_account_id)))
            }
            Err(WriteError::NotFound | WriteError::Forbidden) => {
                transaction.commit().await?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) async fn apply_remote_quote_request(
        &self,
        source_account_id: i64,
        request_uri: &str,
        actor_uri: &str,
        quoted_status_uri: &str,
        instrument_uri: &str,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        if !same_remote_note_host(actor_uri, request_uri)?
            || !same_remote_note_host(actor_uri, instrument_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote QuoteRequest identifiers do not match its actor host",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let mut pending_stream_events = Vec::new();
        lock_remote_interaction(&mut transaction, request_uri).await?;
        if !remote_interaction_actor_matches(&mut transaction, source_account_id, actor_uri, true)
            .await?
        {
            transaction.commit().await?;
            return Ok(());
        }
        if remote_quote_request_decision_in(
            &mut transaction,
            request_uri,
            actor_uri,
            quoted_status_uri,
            instrument_uri,
        )
        .await?
        .is_some()
        {
            transaction.commit().await?;
            return Ok(());
        }
        let Some((target_status_id, target_account_id, target_local, target_actor_uri)) =
            resolve_quote_target(&mut transaction, quoted_status_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        if !target_local || delivery_target_account_id.is_some_and(|id| id != target_account_id) {
            transaction.commit().await?;
            return Ok(());
        }
        let decision_allowed =
            writable_quote_target(&mut transaction, source_account_id, target_status_id)
                .await
                .is_ok();
        let source_delivery = sqlx::query_as::<_, (String, String)>(
            "SELECT inbox_url, domain FROM accounts \
             WHERE id = $1 AND domain IS NOT NULL AND uri = $2 AND protocol = 1 \
               AND suspended_at IS NULL FOR UPDATE",
        )
        .bind(source_account_id)
        .bind(actor_uri)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((inbox_url, remote_domain)) = source_delivery else {
            transaction.commit().await?;
            return Ok(());
        };
        let quote = sqlx::query_as::<_, (i64, i64, i32, Option<String>, bool)>(
            "SELECT quote.id, quote.status_id, quote.state, quote.activity_uri, quote.legacy \
               FROM quotes quote \
               JOIN statuses instrument ON instrument.id = quote.status_id \
              WHERE instrument.account_id = $1 AND instrument.deleted_at IS NULL \
                AND (instrument.uri = $2 OR instrument.url = $2) \
                AND quote.quoted_status_id = $3 \
              ORDER BY quote.id LIMIT 1 FOR UPDATE OF quote",
        )
        .bind(source_account_id)
        .bind(instrument_uri)
        .bind(target_status_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let decision_quote_identity = quote
            .as_ref()
            .map(|(quote_id, quoting_status_id, _, _, _)| (*quote_id, *quoting_status_id));
        let mut accepted = false;
        if let Some((quote_id, quoting_status_id, state, activity_uri, legacy)) = quote.as_ref() {
            if activity_uri
                .as_deref()
                .is_some_and(|uri| uri != request_uri)
            {
                return Err(WriteError::Conflict);
            }
            if decision_allowed && matches!(*state, 0 | 1) {
                accepted = true;
                if *state == 0 {
                    sqlx::query(
                        "UPDATE quotes SET state = 1, activity_uri = $2, approval_uri = NULL, \
                                updated_at = clock_timestamp() WHERE id = $1 AND state = 0",
                    )
                    .bind(quote_id)
                    .bind(request_uri)
                    .execute(&mut *transaction)
                    .await?;
                    if quote_state_update_counter_delta(*legacy, *state, 1) > 0 {
                        increment_quote_count(&mut transaction, target_status_id).await?;
                    }
                    record_outbox_in(
                        &mut transaction,
                        &notification_job(target_account_id, NOTIFICATION_QUOTE, *quote_id),
                    )
                    .await?;
                    collect_status_stream_events(
                        &mut transaction,
                        &mut pending_stream_events,
                        *quoting_status_id,
                        "status.update",
                        Utc::now().timestamp_micros(),
                    )
                    .await?;
                } else if activity_uri.is_none() {
                    sqlx::query(
                        "UPDATE quotes SET activity_uri = $2, updated_at = clock_timestamp() \
                         WHERE id = $1 AND activity_uri IS NULL",
                    )
                    .bind(quote_id)
                    .bind(request_uri)
                    .execute(&mut *transaction)
                    .await?;
                }
            } else if !decision_allowed && matches!(*state, 0 | 1) {
                let next_state = if *state == 1 { 3 } else { 2 };
                sqlx::query(
                    "UPDATE quotes SET state = $2, activity_uri = $3, approval_uri = NULL, \
                            updated_at = clock_timestamp() WHERE id = $1",
                )
                .bind(quote_id)
                .bind(next_state)
                .bind(request_uri)
                .execute(&mut *transaction)
                .await?;
                if quote_state_update_counter_delta(*legacy, *state, next_state) < 0 {
                    decrement_quote_count(&mut transaction, target_status_id).await?;
                }
                delete_activity_notifications(
                    &mut transaction,
                    target_account_id,
                    *quote_id,
                    "Quote",
                )
                .await?;
                collect_status_stream_events(
                    &mut transaction,
                    &mut pending_stream_events,
                    *quoting_status_id,
                    "status.update",
                    Utc::now().timestamp_micros(),
                )
                .await?;
            }
        }
        let quote_id = if accepted {
            decision_quote_identity
                .map(|(quote_id, _)| quote_id)
                .expect("accepted QuoteRequest has a persisted quote")
        } else {
            activitypub::quote_request_rejection_id(request_uri)
        };
        let authorization_uri = accepted.then(|| {
            format!(
                "{}/quote_authorizations/{quote_id}",
                target_actor_uri.trim_end_matches('/')
            )
        });
        let body = activitypub::quote_decision_with_uris(
            &target_actor_uri,
            quote_id,
            request_uri,
            actor_uri,
            quoted_status_uri,
            instrument_uri,
            authorization_uri.as_deref(),
            accepted,
        );
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "source_account_id": target_account_id,
                "inbox_url": inbox_url,
                "remote_domain": remote_domain,
                "body": body,
                "quote_delivery_kind": if accepted { "accept" } else { "reject" },
                "quote_request_uri": request_uri,
                "quote_id": decision_quote_identity.map(|(quote_id, _)| quote_id),
                "quoting_status_id": decision_quote_identity.map(|(_, status_id)| status_id),
                "quoted_status_id": target_status_id
            }),
        )
        .logical_key(activitypub::quote_request_decision_logical_key(request_uri));
        if !record_outbox_once_in(&mut transaction, &delivery).await? {
            return Err(WriteError::Conflict);
        }
        flush_stream_events_in(&mut transaction, &mut pending_stream_events).await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_follow(
        &self,
        source_account_id: i64,
        follow_uri: &str,
        object_uri: &str,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<Option<RemoteFollowOutcome>, WriteError> {
        if follow_uri.trim().is_empty() || object_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote Follow is missing its activity or object URI",
            ));
        }
        let preheal_target = self
            .local_activitypub_account_id_before_relationship_locks(object_uri, origin)
            .await?;
        if let Some(target_account_id) = preheal_target.filter(|id| *id != -99)
            && delivery_target_account_id.is_none_or(|id| id == -99 || id == target_account_id)
        {
            self.preheal_relationship_account_stats(source_account_id, target_account_id)
                .await?;
        }
        let tombstone_key = follow_tombstone_key(source_account_id, follow_uri);
        let mut transaction = self.pool.begin().await?;
        lock_follow_tombstone(&mut transaction, &tombstone_key).await?;
        let tombstoned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM rustodon.idempotency_keys
                  WHERE scope = $1 AND key = $2 AND expires_at > clock_timestamp())",
        )
        .bind(RELATIONSHIP_TOMBSTONE_SCOPE)
        .bind(&tombstone_key)
        .fetch_one(&mut *transaction)
        .await?;
        if tombstoned {
            transaction.commit().await?;
            return Ok(None);
        }
        if remote_interaction_tombstoned(&mut transaction, source_account_id, follow_uri).await? {
            transaction.commit().await?;
            return Ok(None);
        }

        let Some(target_account_id) =
            local_activitypub_account_id(&mut transaction, object_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        if delivery_target_account_id.is_some_and(|id| id != -99 && id != target_account_id) {
            transaction.commit().await?;
            return Ok(None);
        }
        if target_account_id == -99 {
            transaction.commit().await?;
            return Ok(Some(RemoteFollowOutcome::Rejected {
                recipient_account_id: target_account_id,
            }));
        }
        lock_relationship(&mut transaction, source_account_id, target_account_id).await?;
        let Some((
            target_locked,
            target_unavailable,
            source_suspended,
            source_silenced,
            domain_blocked,
        )) = sqlx::query_as::<_, (bool, bool, bool, bool, bool)>(
            "SELECT target.locked,
                        target.suspended_at IS NOT NULL OR target.moved_to_account_id IS NOT NULL,
                        source.suspended_at IS NOT NULL,
                        source.silenced_at IS NOT NULL,
                        EXISTS (
                          SELECT 1 FROM account_domain_blocks domain_block
                           WHERE domain_block.account_id = target.id
                             AND domain_block.domain = source.domain)
                   FROM accounts source
                   JOIN accounts target ON target.id = $2
                  WHERE source.id = $1 AND source.domain IS NOT NULL
                  FOR UPDATE",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        if source_suspended {
            transaction.commit().await?;
            return Ok(None);
        }

        if let Some(existing_id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM follow_requests
               WHERE account_id = $1 AND target_account_id = $2 FOR UPDATE",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            update_follow_uri(
                &mut transaction,
                "follow_requests",
                source_account_id,
                target_account_id,
                follow_uri,
            )
            .await?;
            ensure_relationship_account_stats(
                &mut transaction,
                source_account_id,
                target_account_id,
            )
            .await?;
            transaction.commit().await?;
            return Ok(Some(RemoteFollowOutcome::Applied(
                RemoteFollowWriteOutcome {
                    activity_id: existing_id,
                    recipient_account_id: target_account_id,
                    request: true,
                    created: false,
                    silenced: source_silenced,
                },
            )));
        }

        if target_unavailable
            || domain_blocked
            || relationship_is_blocked(&mut transaction, source_account_id, target_account_id)
                .await?
        {
            transaction.commit().await?;
            return Ok(Some(RemoteFollowOutcome::Rejected {
                recipient_account_id: target_account_id,
            }));
        }

        if let Some(existing_id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM follows
               WHERE account_id = $1 AND target_account_id = $2 FOR UPDATE",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            update_follow_uri(
                &mut transaction,
                "follows",
                source_account_id,
                target_account_id,
                follow_uri,
            )
            .await?;
            ensure_relationship_account_stats(
                &mut transaction,
                source_account_id,
                target_account_id,
            )
            .await?;
            transaction.commit().await?;
            return Ok(Some(RemoteFollowOutcome::Applied(
                RemoteFollowWriteOutcome {
                    activity_id: existing_id,
                    recipient_account_id: target_account_id,
                    request: false,
                    created: false,
                    silenced: source_silenced,
                },
            )));
        }

        let request = target_locked || source_silenced;
        let table = if request {
            "follow_requests"
        } else {
            "follows"
        };

        let activity_id = sqlx::query_scalar::<_, i64>(&format!(
            "INSERT INTO {table} (
                 account_id, target_account_id, show_reblogs, notify, languages, uri,
                 created_at, updated_at)
             VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())
             RETURNING id"
        ))
        .bind(source_account_id)
        .bind(target_account_id)
        .bind(follow_uri)
        .fetch_one(&mut *transaction)
        .await?;
        if !request {
            increment_follow_counts(&mut transaction, source_account_id, target_account_id).await?;
        }
        let activity_type = if request {
            NOTIFICATION_FOLLOW_REQUEST
        } else {
            NOTIFICATION_FOLLOW
        };
        record_outbox_in(
            &mut transaction,
            &notification_job_with_silenced(
                target_account_id,
                activity_type,
                activity_id,
                source_silenced,
            ),
        )
        .await?;
        ensure_relationship_account_stats(&mut transaction, source_account_id, target_account_id)
            .await?;
        transaction.commit().await?;
        Ok(Some(RemoteFollowOutcome::Applied(
            RemoteFollowWriteOutcome {
                activity_id,
                recipient_account_id: target_account_id,
                request,
                created: true,
                silenced: source_silenced,
            },
        )))
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_undo_follow(
        &self,
        source_account_id: i64,
        follow_uri: &str,
        target_uri: Option<&str>,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        if follow_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote Undo object is missing its URI",
            ));
        }
        let preheal_target = if let Some(target_uri) = target_uri {
            self.local_activitypub_account_id_before_relationship_locks(target_uri, origin)
                .await?
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT target_account_id FROM follows \
                  WHERE account_id = $1 AND uri = $2 \
                 UNION ALL \
                SELECT target_account_id FROM follow_requests \
                  WHERE account_id = $1 AND uri = $2 \
                 LIMIT 1",
            )
            .bind(source_account_id)
            .bind(follow_uri)
            .fetch_optional(&self.pool)
            .await?
        };
        if let Some(target_account_id) = preheal_target.filter(|id| *id != -99)
            && delivery_target_account_id.is_none_or(|id| id == -99 || id == target_account_id)
        {
            self.preheal_relationship_account_stats(source_account_id, target_account_id)
                .await?;
        }
        let tombstone_key = follow_tombstone_key(source_account_id, follow_uri);
        let mut transaction = self.pool.begin().await?;
        lock_follow_tombstone(&mut transaction, &tombstone_key).await?;
        let source_is_remote =
            sqlx::query_scalar::<_, bool>("SELECT domain IS NOT NULL FROM accounts WHERE id = $1")
                .bind(source_account_id)
                .fetch_optional(&mut *transaction)
                .await?
                .unwrap_or(false);
        if !source_is_remote {
            transaction.commit().await?;
            return Ok(());
        }

        let target_from_object = match target_uri {
            Some(target_uri) => {
                let Some(target_account_id) =
                    local_activitypub_account_id(&mut transaction, target_uri, origin).await?
                else {
                    transaction.commit().await?;
                    return Ok(());
                };
                Some(target_account_id)
            }
            None => None,
        };
        let target_account_id = if target_from_object.is_some() {
            target_from_object
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT target_account_id FROM follows
                  WHERE account_id = $1 AND uri = $2
                 UNION ALL
                SELECT target_account_id FROM follow_requests
                  WHERE account_id = $1 AND uri = $2
                 LIMIT 1",
            )
            .bind(source_account_id)
            .bind(follow_uri)
            .fetch_optional(&mut *transaction)
            .await?
        };

        if delivery_target_account_id
            .is_some_and(|id| id != -99 && target_account_id.is_some_and(|target| target != id))
        {
            transaction.commit().await?;
            return Ok(());
        }

        if let Some(target_account_id) = target_account_id.filter(|id| *id != -99) {
            lock_relationship(&mut transaction, source_account_id, target_account_id).await?;
            let removed_follow = sqlx::query_scalar::<_, i64>(
                "DELETE FROM follows
                  WHERE account_id = $1 AND target_account_id = $2 AND uri = $3
                  RETURNING id",
            )
            .bind(source_account_id)
            .bind(target_account_id)
            .bind(follow_uri)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some(follow_id) = removed_follow {
                decrement_follow_counts(&mut transaction, source_account_id, target_account_id)
                    .await?;
                delete_activity_notifications(
                    &mut transaction,
                    target_account_id,
                    follow_id,
                    "Follow",
                )
                .await?;
            } else {
                let removed_request = sqlx::query_scalar::<_, i64>(
                    "DELETE FROM follow_requests
                      WHERE account_id = $1 AND target_account_id = $2 AND uri = $3
                      RETURNING id",
                )
                .bind(source_account_id)
                .bind(target_account_id)
                .bind(follow_uri)
                .fetch_optional(&mut *transaction)
                .await?;
                if let Some(request_id) = removed_request {
                    delete_follow_request_notifications(
                        &mut transaction,
                        target_account_id,
                        request_id,
                    )
                    .await?;
                }
            }
            ensure_relationship_account_stats(
                &mut transaction,
                source_account_id,
                target_account_id,
            )
            .await?;
        }
        insert_relationship_tombstone(&mut transaction, &tombstone_key, follow_uri).await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_block(
        &self,
        source_account_id: i64,
        block_uri: &str,
        object_uri: &str,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        if block_uri.trim().is_empty() || object_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote Block is missing its activity or object URI",
            ));
        }
        let preheal_target = self
            .local_activitypub_account_id_before_relationship_locks(object_uri, origin)
            .await?;
        if let Some(target_account_id) = preheal_target.filter(|id| *id != -99)
            && delivery_target_account_id.is_none_or(|id| id == -99 || id == target_account_id)
        {
            self.preheal_relationship_account_stats(source_account_id, target_account_id)
                .await?;
        }
        let tombstone_key = block_tombstone_key(source_account_id, block_uri);
        let mut transaction = self.pool.begin().await?;
        lock_follow_tombstone(&mut transaction, &tombstone_key).await?;
        let tombstoned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM rustodon.idempotency_keys
                  WHERE scope = $1 AND key = $2 AND expires_at > clock_timestamp())",
        )
        .bind(RELATIONSHIP_TOMBSTONE_SCOPE)
        .bind(&tombstone_key)
        .fetch_one(&mut *transaction)
        .await?;
        if tombstoned {
            transaction.commit().await?;
            return Ok(());
        }
        if remote_interaction_tombstoned(&mut transaction, source_account_id, block_uri).await? {
            transaction.commit().await?;
            return Ok(());
        }

        let Some(target_account_id) =
            local_activitypub_account_id(&mut transaction, object_uri, origin).await?
        else {
            transaction.commit().await?;
            return Ok(());
        };
        if target_account_id == -99
            || delivery_target_account_id.is_some_and(|id| id != -99 && id != target_account_id)
        {
            transaction.commit().await?;
            return Ok(());
        }
        let source_is_remote =
            sqlx::query_scalar::<_, bool>("SELECT domain IS NOT NULL FROM accounts WHERE id = $1")
                .bind(source_account_id)
                .fetch_optional(&mut *transaction)
                .await?
                .unwrap_or(false);
        if !source_is_remote {
            transaction.commit().await?;
            return Ok(());
        }

        lock_relationship(&mut transaction, source_account_id, target_account_id).await?;
        sqlx::query(
            "DELETE FROM notification_permissions
              WHERE account_id = $1 AND from_account_id = $2",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .execute(&mut *transaction)
        .await?;
        remove_follow_relationships(&mut transaction, source_account_id, target_account_id).await?;
        sqlx::query(
            "INSERT INTO blocks (account_id, created_at, target_account_id, updated_at, uri)
             VALUES ($1, clock_timestamp(), $2, clock_timestamp(), $3)
             ON CONFLICT (account_id, target_account_id) DO UPDATE SET
               uri = EXCLUDED.uri, updated_at = clock_timestamp()",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .bind(block_uri)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn apply_remote_undo_block(
        &self,
        source_account_id: i64,
        block_uri: &str,
        target_uri: Option<&str>,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        if block_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote Undo Block is missing its URI",
            ));
        }
        let tombstone_key = block_tombstone_key(source_account_id, block_uri);
        let mut transaction = self.pool.begin().await?;
        lock_follow_tombstone(&mut transaction, &tombstone_key).await?;
        let source_is_remote =
            sqlx::query_scalar::<_, bool>("SELECT domain IS NOT NULL FROM accounts WHERE id = $1")
                .bind(source_account_id)
                .fetch_optional(&mut *transaction)
                .await?
                .unwrap_or(false);
        if !source_is_remote {
            transaction.commit().await?;
            return Ok(());
        }

        let target_from_object = match target_uri {
            Some(target_uri) => {
                let Some(target_account_id) =
                    local_activitypub_account_id(&mut transaction, target_uri, origin).await?
                else {
                    transaction.commit().await?;
                    return Ok(());
                };
                Some(target_account_id)
            }
            None => None,
        };
        let target_account_id = if target_from_object.is_some() {
            target_from_object
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT target_account_id FROM blocks
                  WHERE account_id = $1 AND uri = $2
                  LIMIT 1",
            )
            .bind(source_account_id)
            .bind(block_uri)
            .fetch_optional(&mut *transaction)
            .await?
        };
        if delivery_target_account_id
            .is_some_and(|id| id != -99 && target_account_id.is_some_and(|target| target != id))
        {
            transaction.commit().await?;
            return Ok(());
        }

        if let Some(target_account_id) = target_account_id.filter(|id| *id != -99) {
            lock_relationship(&mut transaction, source_account_id, target_account_id).await?;
            sqlx::query(
                "DELETE FROM blocks
                  WHERE account_id = $1 AND target_account_id = $2 AND uri = $3",
            )
            .bind(source_account_id)
            .bind(target_account_id)
            .bind(block_uri)
            .execute(&mut *transaction)
            .await?;
        }
        insert_relationship_tombstone(&mut transaction, &tombstone_key, block_uri).await?;
        transaction.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) async fn apply_remote_follow_decision(
        &self,
        source_account_id: i64,
        follow_uri: &str,
        target_uri: Option<&str>,
        local_actor_uri: Option<&str>,
        accepted: bool,
        origin: &str,
        delivery_target_account_id: Option<i64>,
    ) -> Result<(), WriteError> {
        if follow_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote Follow decision is missing its Follow URI",
            ));
        }
        let preheal_local_account_id = if let Some(local_actor_uri) = local_actor_uri {
            self.local_activitypub_account_id_before_relationship_locks(local_actor_uri, origin)
                .await?
        } else if let Some(delivery_target_account_id) =
            delivery_target_account_id.filter(|id| *id != -99)
        {
            Some(delivery_target_account_id)
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT account_id FROM follows \
                  WHERE target_account_id = $1 AND uri = $2 \
                 UNION ALL \
                SELECT account_id FROM follow_requests \
                  WHERE target_account_id = $1 AND uri = $2 \
                 LIMIT 1",
            )
            .bind(source_account_id)
            .bind(follow_uri)
            .fetch_optional(&self.pool)
            .await?
        };
        if let Some(local_account_id) = preheal_local_account_id.filter(|id| *id != -99)
            && delivery_target_account_id.is_none_or(|id| id == -99 || id == local_account_id)
        {
            self.preheal_relationship_account_stats(local_account_id, source_account_id)
                .await?;
        }
        let mut transaction = self.pool.begin().await?;
        let source_uri = sqlx::query_scalar::<_, String>(
            "SELECT uri FROM accounts WHERE id = $1 AND domain IS NOT NULL",
        )
        .bind(source_account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(source_uri) = source_uri else {
            transaction.commit().await?;
            return Ok(());
        };
        if target_uri.is_some_and(|target_uri| target_uri != source_uri) {
            transaction.commit().await?;
            return Ok(());
        }

        let local_account_id = if let Some(local_actor_uri) = local_actor_uri {
            let local_account_id =
                local_activitypub_account_id(&mut transaction, local_actor_uri, origin).await?;
            if local_account_id.is_none() {
                transaction.commit().await?;
                return Ok(());
            }
            local_account_id
        } else if let Some(delivery_target_account_id) =
            delivery_target_account_id.filter(|id| *id != -99)
        {
            Some(delivery_target_account_id)
        } else {
            sqlx::query_scalar::<_, i64>(
                "SELECT account_id FROM follows
                  WHERE target_account_id = $1 AND uri = $2
                 UNION ALL
                SELECT account_id FROM follow_requests
                  WHERE target_account_id = $1 AND uri = $2
                 LIMIT 1",
            )
            .bind(source_account_id)
            .bind(follow_uri)
            .fetch_optional(&mut *transaction)
            .await?
        };
        let Some(local_account_id) = local_account_id.filter(|id| *id != -99) else {
            transaction.commit().await?;
            return Ok(());
        };
        if delivery_target_account_id.is_some_and(|id| id != -99 && id != local_account_id) {
            transaction.commit().await?;
            return Ok(());
        }
        lock_relationship(&mut transaction, local_account_id, source_account_id).await?;

        let request = sqlx::query_as::<_, (i64, bool, bool, Option<Vec<String>>, Option<String>)>(
            "SELECT id, show_reblogs, notify, languages, uri
               FROM follow_requests
               WHERE account_id = $1 AND target_account_id = $2
                 AND uri = $3
               FOR UPDATE",
        )
        .bind(local_account_id)
        .bind(source_account_id)
        .bind(follow_uri)
        .fetch_optional(&mut *transaction)
        .await?;
        let follow_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM follows
              WHERE account_id = $1 AND target_account_id = $2
                AND uri = $3
              FOR UPDATE",
        )
        .bind(local_account_id)
        .bind(source_account_id)
        .bind(follow_uri)
        .fetch_optional(&mut *transaction)
        .await?;
        if accepted {
            if let Some(request) = request {
                if follow_id.is_none() {
                    sqlx::query(
                        "INSERT INTO follows (
                           account_id, target_account_id, show_reblogs, notify, languages, uri,
                           created_at, updated_at)
                         VALUES ($1, $2, $3, $4, $5, $6, clock_timestamp(), clock_timestamp())",
                    )
                    .bind(local_account_id)
                    .bind(source_account_id)
                    .bind(request.1)
                    .bind(request.2)
                    .bind(request.3)
                    .bind(follow_uri)
                    .execute(&mut *transaction)
                    .await?;
                    increment_follow_counts(&mut transaction, local_account_id, source_account_id)
                        .await?;
                } else {
                    sqlx::query(
                        "UPDATE follows SET uri = $3, updated_at = clock_timestamp()
                          WHERE account_id = $1 AND target_account_id = $2",
                    )
                    .bind(local_account_id)
                    .bind(source_account_id)
                    .bind(follow_uri)
                    .execute(&mut *transaction)
                    .await?;
                }
                sqlx::query(
                    "DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2",
                )
                .bind(local_account_id)
                .bind(source_account_id)
                .execute(&mut *transaction)
                .await?;
                delete_follow_request_notifications(&mut transaction, local_account_id, request.0)
                    .await?;
            } else if follow_id.is_some() {
                sqlx::query(
                    "UPDATE follows SET uri = $3, updated_at = clock_timestamp()
                      WHERE account_id = $1 AND target_account_id = $2",
                )
                .bind(local_account_id)
                .bind(source_account_id)
                .bind(follow_uri)
                .execute(&mut *transaction)
                .await?;
            }
        } else {
            if let Some(request) = request {
                sqlx::query(
                    "DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2",
                )
                .bind(local_account_id)
                .bind(source_account_id)
                .execute(&mut *transaction)
                .await?;
                delete_follow_request_notifications(&mut transaction, local_account_id, request.0)
                    .await?;
            }
            if let Some(follow_id) = follow_id {
                sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
                    .bind(local_account_id)
                    .bind(source_account_id)
                    .execute(&mut *transaction)
                    .await?;
                decrement_follow_counts(&mut transaction, local_account_id, source_account_id)
                    .await?;
                delete_activity_notifications(
                    &mut transaction,
                    source_account_id,
                    follow_id,
                    "Follow",
                )
                .await?;
            }
        }
        ensure_relationship_account_stats(&mut transaction, local_account_id, source_account_id)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn authorize_follow_request(
        &self,
        authenticated: &AuthenticatedBearer,
        source_account_id: i64,
    ) -> Result<FollowWriteOutcome, WriteError> {
        self.authorize_follow_request_with_origin(authenticated, source_account_id, None)
            .await
    }

    #[allow(clippy::too_many_lines)]
    pub async fn authorize_follow_request_with_origin(
        &self,
        authenticated: &AuthenticatedBearer,
        source_account_id: i64,
        origin: Option<&str>,
    ) -> Result<FollowWriteOutcome, WriteError> {
        let target_account_id = write_account(authenticated, WRITE_FOLLOWS)?;
        if source_account_id == target_account_id {
            return Err(WriteError::NotFound);
        }
        let mut transaction = self
            .begin_relationship_account_write(target_account_id, source_account_id)
            .await?;
        let request = sqlx::query_as::<_, (i64, bool, bool, Option<Vec<String>>, Option<String>)>(
            "SELECT id, show_reblogs, notify, languages, uri FROM follow_requests \
             WHERE account_id = $1 AND target_account_id = $2 FOR UPDATE",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        let follow_id = if let Some(follow_id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM follows WHERE account_id = $1 AND target_account_id = $2 FOR UPDATE",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            update_follow_options(
                &mut transaction,
                "follows",
                source_account_id,
                target_account_id,
                Some(request.1),
                Some(request.2),
                request.3.clone(),
            )
            .await?;
            follow_id
        } else {
            let follow_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO follows ( \
                   account_id, target_account_id, show_reblogs, notify, languages, uri, \
                   created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, clock_timestamp(), clock_timestamp()) \
                 RETURNING id",
            )
            .bind(source_account_id)
            .bind(target_account_id)
            .bind(request.1)
            .bind(request.2)
            .bind(request.3)
            .bind(request.4.clone())
            .fetch_one(&mut *transaction)
            .await?;
            increment_follow_counts(&mut transaction, source_account_id, target_account_id).await?;
            follow_id
        };
        delete_follow_request_notifications(&mut transaction, target_account_id, request.0).await?;
        sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
            .bind(source_account_id)
            .bind(target_account_id)
            .execute(&mut *transaction)
            .await?;
        if let Some(origin) = origin
            && let Some(follow_uri) = request.4.as_deref()
        {
            let source = sqlx::query_as::<_, (String, String, String, bool, Option<String>)>(
                "SELECT source.uri, source.inbox_url, source.shared_inbox_url,
                        source.domain IS NOT NULL AND target.domain IS NULL, source.domain
                 FROM accounts source
                  JOIN accounts target ON target.id = $2
                  WHERE source.id = $1 AND source.protocol = 1",
            )
            .bind(source_account_id)
            .bind(target_account_id)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some((source_uri, inbox_url, shared_inbox_url, remote_to_local, remote_domain)) =
                source
                && remote_to_local
            {
                let inbox_url = if inbox_url.is_empty() {
                    shared_inbox_url
                } else {
                    inbox_url
                };
                if !source_uri.is_empty() && !inbox_url.is_empty() {
                    let target_uri = if target_account_id == -99 {
                        format!("{}/actor", origin.trim_end_matches('/'))
                    } else {
                        sqlx::query_scalar::<_, String>(
                            "SELECT CASE WHEN id_scheme = 1
                                      THEN $1 || '/ap/users/' || id::text
                                      ELSE $1 || '/users/' || username END
                               FROM accounts WHERE id = $2",
                        )
                        .bind(origin.trim_end_matches('/'))
                        .bind(target_account_id)
                        .fetch_one(&mut *transaction)
                        .await?
                    };
                    let body = activitypub::accept_with_uris(
                        &target_uri,
                        follow_id,
                        follow_uri,
                        &source_uri,
                    );
                    let delivery = JobSpec::new(
                        Lane::Push,
                        ACTIVITYPUB_DELIVERY_JOB_KIND,
                        json!({
                            "source_account_id": target_account_id,
                            "inbox_url": inbox_url,
                            "remote_domain": remote_domain,
                            "body": body
                        }),
                    )
                    .logical_key(activitypub::accept_delivery_logical_key(
                        follow_id, follow_uri, &inbox_url,
                    ));
                    record_outbox_once_in(&mut transaction, &delivery).await?;
                }
            }
        }
        record_outbox_in(
            &mut transaction,
            &notification_job(target_account_id, "follow", follow_id),
        )
        .await?;
        ensure_relationship_account_stats(&mut transaction, source_account_id, target_account_id)
            .await?;
        transaction.commit().await?;
        Ok(FollowWriteOutcome {
            activity_id: Some(follow_id),
            recipient_account_id: target_account_id,
            request: false,
            activity_uri: None,
        })
    }

    pub async fn reject_follow_request(
        &self,
        authenticated: &AuthenticatedBearer,
        source_account_id: i64,
    ) -> Result<(), WriteError> {
        self.reject_follow_request_with_origin(authenticated, source_account_id, None)
            .await
    }

    pub async fn reject_follow_request_with_origin(
        &self,
        authenticated: &AuthenticatedBearer,
        source_account_id: i64,
        origin: Option<&str>,
    ) -> Result<(), WriteError> {
        let target_account_id = write_account(authenticated, WRITE_FOLLOWS)?;
        if source_account_id == target_account_id {
            return Err(WriteError::NotFound);
        }
        let mut transaction = self
            .begin_relationship_account_write(target_account_id, source_account_id)
            .await?;
        let (request_id, follow_uri) = sqlx::query_as::<_, (i64, Option<String>)>(
            "DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2 \
             RETURNING id, uri",
        )
        .bind(source_account_id)
        .bind(target_account_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        delete_follow_request_notifications(&mut transaction, target_account_id, request_id)
            .await?;
        if let (Some(origin), Some(follow_uri)) = (origin, follow_uri)
            && let Some(remote_delivery) = remote_relationship_delivery(
                &mut transaction,
                target_account_id,
                source_account_id,
                origin,
            )
            .await?
        {
            record_remote_reject_delivery(
                &mut transaction,
                target_account_id,
                &remote_delivery,
                request_id,
                &follow_uri,
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn remove_follower(
        &self,
        authenticated: &AuthenticatedBearer,
        follower_account_id: i64,
    ) -> Result<(), WriteError> {
        self.remove_follower_with_origin(authenticated, follower_account_id, None)
            .await
    }

    pub async fn remove_follower_with_origin(
        &self,
        authenticated: &AuthenticatedBearer,
        follower_account_id: i64,
        origin: Option<&str>,
    ) -> Result<(), WriteError> {
        let target_account_id = write_account(authenticated, WRITE_FOLLOWS)?;
        if follower_account_id == target_account_id {
            return Err(WriteError::NotFound);
        }
        let mut transaction = self
            .begin_relationship_account_write(target_account_id, follower_account_id)
            .await?;
        let (_, _, follower_unavailable, _) =
            relationship_target(&mut transaction, target_account_id, follower_account_id).await?;
        if follower_unavailable {
            return Err(WriteError::NotFound);
        }
        let removed = sqlx::query_as::<_, (i64, Option<String>)>(
            "DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2 \
             RETURNING id, uri",
        )
        .bind(follower_account_id)
        .bind(target_account_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some((follow_id, follow_uri)) = removed {
            decrement_follow_counts(&mut transaction, follower_account_id, target_account_id)
                .await?;
            delete_activity_notifications(&mut transaction, target_account_id, follow_id, "Follow")
                .await?;
            if let (Some(origin), Some(follow_uri)) = (origin, follow_uri)
                && let Some(remote_delivery) = remote_relationship_delivery(
                    &mut transaction,
                    target_account_id,
                    follower_account_id,
                    origin,
                )
                .await?
            {
                record_remote_reject_delivery(
                    &mut transaction,
                    target_account_id,
                    &remote_delivery,
                    follow_id,
                    &follow_uri,
                )
                .await?;
            }
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_block(
        &self,
        authenticated: &AuthenticatedBearer,
        target_account_id: i64,
        blocking: bool,
    ) -> Result<(), WriteError> {
        self.set_block_with_origin(authenticated, target_account_id, blocking, None)
            .await
    }

    #[allow(clippy::too_many_lines)]
    pub async fn set_block_with_origin(
        &self,
        authenticated: &AuthenticatedBearer,
        target_account_id: i64,
        blocking: bool,
        origin: Option<&str>,
    ) -> Result<(), WriteError> {
        let account_id = write_account(authenticated, WRITE_BLOCKS)?;
        if account_id == target_account_id {
            return Ok(());
        }
        let mut transaction = self
            .begin_relationship_account_write(account_id, target_account_id)
            .await?;
        let (target_local, _, target_unavailable, _) =
            relationship_target(&mut transaction, account_id, target_account_id).await?;
        if target_unavailable {
            return Err(WriteError::NotFound);
        }
        let remote_delivery = match (origin, target_local) {
            (Some(origin), false) => {
                remote_relationship_delivery(
                    &mut transaction,
                    account_id,
                    target_account_id,
                    origin,
                )
                .await?
            }
            _ => None,
        };
        if blocking {
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
            let mut outgoing_follow_uris = Vec::new();
            let mut incoming_follow_uris = Vec::new();
            let mut incoming_request_uris = Vec::new();
            if remote_delivery.is_some() {
                outgoing_follow_uris = sqlx::query_scalar::<_, String>(
                    "SELECT uri FROM follows
                      WHERE account_id = $1 AND target_account_id = $2 AND uri IS NOT NULL",
                )
                .bind(account_id)
                .bind(target_account_id)
                .fetch_all(&mut *transaction)
                .await?;
                incoming_follow_uris = sqlx::query_as::<_, (i64, String)>(
                    "SELECT id, uri FROM follows
                      WHERE account_id = $1 AND target_account_id = $2 AND uri IS NOT NULL",
                )
                .bind(target_account_id)
                .bind(account_id)
                .fetch_all(&mut *transaction)
                .await?;
                incoming_request_uris = sqlx::query_as::<_, (i64, String)>(
                    "SELECT id, uri FROM follow_requests
                      WHERE account_id = $1 AND target_account_id = $2 AND uri IS NOT NULL",
                )
                .bind(target_account_id)
                .bind(account_id)
                .fetch_all(&mut *transaction)
                .await?;
            }
            sqlx::query(
                "DELETE FROM notification_permissions \
                 WHERE account_id = $1 AND from_account_id = $2",
            )
            .bind(account_id)
            .bind(target_account_id)
            .execute(&mut *transaction)
            .await?;
            let block = sqlx::query_as::<_, (i64, Option<String>)>(
                "INSERT INTO blocks (account_id, created_at, target_account_id, updated_at, uri) \
                 VALUES ($1, clock_timestamp(), $2, clock_timestamp(), NULL) \
                 ON CONFLICT (account_id, target_account_id) DO NOTHING \
                 RETURNING id, uri",
            )
            .bind(account_id)
            .bind(target_account_id)
            .fetch_optional(&mut *transaction)
            .await?;
            let (block_id, block_uri) = if let Some(block) = block {
                block
            } else {
                sqlx::query_as::<_, (i64, Option<String>)>(
                    "SELECT id, uri FROM blocks
                      WHERE account_id = $1 AND target_account_id = $2
                      FOR UPDATE",
                )
                .bind(account_id)
                .bind(target_account_id)
                .fetch_one(&mut *transaction)
                .await?
            };
            let mut block_uri_for_delivery = None;
            if let (Some(origin), Some(_)) = (origin, remote_delivery.as_ref()) {
                let block_uri = block_uri.unwrap_or_else(|| {
                    local_block_activity_uri(origin, account_id, target_account_id, block_id)
                });
                sqlx::query(
                    "UPDATE blocks SET uri = $3, updated_at = clock_timestamp() \
                     WHERE account_id = $1 AND target_account_id = $2",
                )
                .bind(account_id)
                .bind(target_account_id)
                .bind(&block_uri)
                .execute(&mut *transaction)
                .await?;
                block_uri_for_delivery = Some(block_uri);
            }
            remove_follow_relationships(&mut transaction, account_id, target_account_id).await?;
            if let (Some(origin), Some(remote_delivery)) = (origin, remote_delivery.as_ref()) {
                for follow_uri in outgoing_follow_uris {
                    cancel_activitypub_delivery(&mut transaction, &follow_uri).await?;
                    record_remote_undo_follow_delivery(
                        &mut transaction,
                        account_id,
                        remote_delivery,
                        &follow_uri,
                        origin,
                    )
                    .await?;
                }
                for (follow_id, follow_uri) in incoming_follow_uris
                    .into_iter()
                    .chain(incoming_request_uris)
                {
                    record_remote_reject_delivery(
                        &mut transaction,
                        account_id,
                        remote_delivery,
                        follow_id,
                        &follow_uri,
                    )
                    .await?;
                }
                if let Some(block_uri) = block_uri_for_delivery {
                    record_remote_block_delivery(
                        &mut transaction,
                        account_id,
                        remote_delivery,
                        &block_uri,
                    )
                    .await?;
                }
            }
            clear_account_interactions(&mut transaction, account_id, target_account_id).await?;
        } else {
            let block = sqlx::query_as::<_, (i64, Option<String>)>(
                "DELETE FROM blocks
                  WHERE account_id = $1 AND target_account_id = $2
                  RETURNING id, uri",
            )
            .bind(account_id)
            .bind(target_account_id)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some((block_id, block_uri)) = block
                && let (Some(origin), Some(remote_delivery)) = (origin, remote_delivery.as_ref())
            {
                let block_uri = block_uri.unwrap_or_else(|| {
                    local_block_activity_uri(origin, account_id, target_account_id, block_id)
                });
                cancel_activitypub_delivery(&mut transaction, &block_uri).await?;
                record_remote_undo_block_delivery(
                    &mut transaction,
                    account_id,
                    remote_delivery,
                    &block_uri,
                    origin,
                )
                .await?;
            }
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn set_mute(
        &self,
        authenticated: &AuthenticatedBearer,
        target_account_id: i64,
        muting: bool,
        hide_notifications: Option<bool>,
        duration: Option<i64>,
    ) -> Result<(), WriteError> {
        let account_id = write_account(authenticated, WRITE_MUTES)?;
        if account_id == target_account_id {
            return Err(WriteError::NotFound);
        }
        let mut transaction = self
            .begin_relationship_account_write(account_id, target_account_id)
            .await?;
        let (_, _, target_unavailable, _) =
            relationship_target(&mut transaction, account_id, target_account_id).await?;
        if target_unavailable {
            return Err(WriteError::NotFound);
        }
        let hide_notifications = hide_notifications.unwrap_or(true);
        if muting && hide_notifications {
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(account_id)
                .execute(&mut *transaction)
                .await?;
        }
        if muting {
            let mute_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO mutes (account_id, created_at, expires_at, hide_notifications, \
                                  target_account_id, updated_at) \
                  VALUES ($1, clock_timestamp(), \
                         CASE WHEN COALESCE($4, 0) > 0 \
                              THEN clock_timestamp() + (COALESCE($4, 0)::double precision * interval '1 second') \
                              ELSE NULL END, \
                         COALESCE($3, true), $2, clock_timestamp()) \
                  ON CONFLICT (account_id, target_account_id) DO UPDATE SET \
                    expires_at = EXCLUDED.expires_at, \
                    hide_notifications = EXCLUDED.hide_notifications, \
                    updated_at = clock_timestamp() \
                  RETURNING id, created_at",
            )
            .bind(account_id)
            .bind(target_account_id)
            .bind(hide_notifications)
            .bind(duration)
            .fetch_one(&mut *transaction)
            .await?;
            if hide_notifications {
                clear_account_interactions(&mut transaction, account_id, target_account_id).await?;
            }
            cancel_pending_mute_expiry_events(&mut transaction, mute_id).await?;
            if let Some(duration) = duration.filter(|duration| *duration > 0) {
                let run_at = Utc::now()
                    .checked_add_signed(ChronoDuration::seconds(duration))
                    .ok_or(WriteError::Validation("mute duration is too large"))?;
                let spec = JobSpec::new(
                    Lane::Maintenance,
                    MUTE_EXPIRY_JOB_KIND,
                    json!({"mute_id": mute_id}),
                )
                .logical_key(format!(
                    "mute-expiry:{mute_id}:{}",
                    run_at.timestamp_millis()
                ))
                .run_at(run_at);
                record_outbox_in(&mut transaction, &spec).await?;
            }
        } else {
            let mute_id = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM mutes WHERE account_id = $1 AND target_account_id = $2 FOR UPDATE",
            )
            .bind(account_id)
            .bind(target_account_id)
            .fetch_optional(&mut *transaction)
            .await?;
            sqlx::query("DELETE FROM mutes WHERE account_id = $1 AND target_account_id = $2")
                .bind(account_id)
                .bind(target_account_id)
                .execute(&mut *transaction)
                .await?;
            if let Some(mute_id) = mute_id {
                cancel_pending_mute_expiry_events(&mut transaction, mute_id).await?;
            }
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn update_marker_with_options(
        &self,
        authenticated: &AuthenticatedBearer,
        timeline: &str,
        last_read_id: i64,
        expected_lock_version: Option<i32>,
        options: WriteOptions<'_>,
    ) -> Result<WriteOutcome<Marker>, WriteError> {
        let (_, mut transaction) = self
            .begin_account_write(authenticated, WRITE_STATUSES)
            .await?;
        let user_id = authenticated
            .require_user()
            .map_err(|_| WriteError::Unauthorized)?
            .user_id();
        if !matches!(timeline, "home" | "notifications") {
            return Err(WriteError::InvalidInput("unknown marker timeline"));
        }
        validate_idempotency(options.idempotency)?;

        if let Some(idempotency) = options.idempotency
            && claim_idempotency(&mut transaction, idempotency).await?
        {
            let marker = select_marker(&mut transaction, user_id, timeline)
                .await?
                .ok_or(WriteError::Conflict)?;
            transaction.commit().await?;
            return Ok(WriteOutcome::Replayed(marker));
        }

        let marker = update_marker_in(
            &mut transaction,
            user_id,
            timeline,
            last_read_id,
            expected_lock_version,
        )
        .await?;
        if let Some(outbox) = options.outbox {
            record_outbox_in(&mut transaction, outbox).await?;
        }
        if let Some(idempotency) = options.idempotency {
            complete_idempotency(&mut transaction, idempotency, &marker).await?;
        }
        transaction.commit().await?;
        Ok(WriteOutcome::Applied(marker))
    }

    pub async fn clear_notifications(
        &self,
        authenticated: &AuthenticatedBearer,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_NOTIFICATIONS)
            .await?;
        sqlx::query("DELETE FROM notifications WHERE account_id = $1")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn dismiss_notification(
        &self,
        authenticated: &AuthenticatedBearer,
        notification_id: i64,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_NOTIFICATIONS)
            .await?;
        let from_account_id = sqlx::query_scalar::<_, i64>(
            "SELECT from_account_id FROM notifications \
             WHERE account_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(account_id)
        .bind(notification_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        sqlx::query("DELETE FROM notifications WHERE account_id = $1 AND id = $2")
            .bind(account_id)
            .bind(notification_id)
            .execute(&mut *transaction)
            .await?;
        reconcile_notification_requests(&mut transaction, account_id, &[from_account_id]).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn dismiss_notification_group(
        &self,
        authenticated: &AuthenticatedBearer,
        group_key: &str,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_NOTIFICATIONS)
            .await?;
        let ungrouped_id = group_key
            .strip_prefix("ungrouped-")
            .and_then(|id| id.parse::<i64>().ok());
        let notification_rows = if let Some(notification_id) = ungrouped_id {
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT id, from_account_id FROM notifications \
                 WHERE account_id = $1 AND id = $2 FOR UPDATE",
            )
            .bind(account_id)
            .bind(notification_id)
            .fetch_all(&mut *transaction)
            .await?
        } else {
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT id, from_account_id FROM notifications \
                 WHERE account_id = $1 AND group_key = $2 FOR UPDATE",
            )
            .bind(account_id)
            .bind(group_key)
            .fetch_all(&mut *transaction)
            .await?
        };
        let notification_ids = notification_rows
            .iter()
            .map(|(notification_id, _)| *notification_id)
            .collect::<Vec<_>>();
        let from_account_ids = notification_rows
            .iter()
            .map(|(_, from_account_id)| *from_account_id)
            .collect::<Vec<_>>();
        if !notification_ids.is_empty() {
            sqlx::query("DELETE FROM notifications WHERE account_id = $1 AND id = ANY($2)")
                .bind(account_id)
                .bind(&notification_ids)
                .execute(&mut *transaction)
                .await?;
        }
        reconcile_notification_requests(&mut transaction, account_id, &from_account_ids).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn accept_notification_request(
        &self,
        authenticated: &AuthenticatedBearer,
        request_id: i64,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_NOTIFICATIONS)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        let from_account_id = sqlx::query_scalar::<_, i64>(
            "SELECT from_account_id FROM notification_requests \
             WHERE account_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(account_id)
        .bind(request_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        sqlx::query(
            "INSERT INTO notification_permissions \
             (account_id, from_account_id, created_at, updated_at) \
             VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
        )
        .bind(account_id)
        .bind(from_account_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM notification_requests WHERE account_id = $1 AND id = $2")
            .bind(account_id)
            .bind(request_id)
            .execute(&mut *transaction)
            .await?;
        record_outbox_once_in(
            &mut transaction,
            &notification_unfilter_job(account_id, from_account_id),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn dismiss_notification_request(
        &self,
        authenticated: &AuthenticatedBearer,
        request_id: i64,
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_NOTIFICATIONS)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        let from_account_id = sqlx::query_scalar::<_, i64>(
            "DELETE FROM notification_requests WHERE account_id = $1 AND id = $2
             RETURNING from_account_id",
        )
        .bind(account_id)
        .bind(request_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        record_outbox_once_in(
            &mut transaction,
            &notification_cleanup_job(account_id, from_account_id, request_id),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn accept_notification_requests(
        &self,
        authenticated: &AuthenticatedBearer,
        request_ids: &[i64],
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_NOTIFICATIONS)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        let requests = sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, from_account_id FROM notification_requests \
             WHERE account_id = $1 AND id = ANY($2) FOR UPDATE",
        )
        .bind(account_id)
        .bind(request_ids)
        .fetch_all(&mut *transaction)
        .await?;
        let from_account_ids = requests
            .iter()
            .map(|(_, from_account_id)| *from_account_id)
            .collect::<Vec<_>>();
        for (_, from_account_id) in requests {
            sqlx::query(
                "INSERT INTO notification_permissions \
                 (account_id, from_account_id, created_at, updated_at) \
                 VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
            )
            .bind(account_id)
            .bind(from_account_id)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query("DELETE FROM notification_requests WHERE account_id = $1 AND id = ANY($2)")
            .bind(account_id)
            .bind(request_ids)
            .execute(&mut *transaction)
            .await?;
        for from_account_id in from_account_ids {
            record_outbox_once_in(
                &mut transaction,
                &notification_unfilter_job(account_id, from_account_id),
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn dismiss_notification_requests(
        &self,
        authenticated: &AuthenticatedBearer,
        request_ids: &[i64],
    ) -> Result<(), WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_NOTIFICATIONS)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM notification_requests WHERE account_id = $1 AND id = ANY($2)")
            .bind(account_id)
            .bind(request_ids)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn update_notification_policy(
        &self,
        authenticated: &AuthenticatedBearer,
        update: NotificationPolicyUpdate,
    ) -> Result<NotificationPolicy, WriteError> {
        let (account_id, mut transaction) = self
            .begin_account_write(authenticated, WRITE_NOTIFICATIONS)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
        let policy = sqlx::query_as::<_, NotificationPolicy>(
            "INSERT INTO notification_policies ( \
               account_id, for_bots, for_limited_accounts, for_new_accounts, \
               for_not_followers, for_not_following, for_private_mentions, \
               created_at, updated_at) \
             VALUES ($1, COALESCE($2, 0), COALESCE($3, 1), COALESCE($4, 0), \
                     COALESCE($5, 0), COALESCE($6, 0), COALESCE($7, 1), \
                     clock_timestamp(), clock_timestamp()) \
             ON CONFLICT (account_id) DO UPDATE SET \
               for_bots = COALESCE($2, notification_policies.for_bots), \
               for_limited_accounts = COALESCE($3, notification_policies.for_limited_accounts), \
               for_new_accounts = COALESCE($4, notification_policies.for_new_accounts), \
               for_not_followers = COALESCE($5, notification_policies.for_not_followers), \
               for_not_following = COALESCE($6, notification_policies.for_not_following), \
               for_private_mentions = COALESCE($7, notification_policies.for_private_mentions), \
               updated_at = clock_timestamp() \
             RETURNING id, account_id, for_bots, for_limited_accounts, for_new_accounts, \
                       for_not_followers, for_not_following, for_private_mentions",
        )
        .bind(account_id)
        .bind(update.for_bots)
        .bind(update.for_limited_accounts)
        .bind(update.for_new_accounts)
        .bind(update.for_not_followers)
        .bind(update.for_not_following)
        .bind(update.for_private_mentions)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(policy)
    }

    pub async fn register_oauth_application(
        &self,
        registration: &OAuthApplicationRegistration,
    ) -> Result<OAuthApplicationRegistrationResult, WriteError> {
        if validate_oauth_application_registration(registration).is_err() {
            return Err(WriteError::Validation("Validation failed"));
        }
        let uid = random_urlsafe_base64(32);
        let client_secret = random_urlsafe_base64(32);
        let scopes = canonical_oauth_scopes(&registration.scopes);
        let mut transaction = self.pool.begin().await?;
        let application = sqlx::query_as::<_, super::records::OAuthApplication>(
            "INSERT INTO oauth_applications ( \
               name, uid, secret, redirect_uri, scopes, confidential, website, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, true, $6, clock_timestamp(), clock_timestamp()) \
             RETURNING id, name, uid, secret, redirect_uri, scopes, confidential, owner_id, owner_type, website",
        )
        .bind(&registration.name)
        .bind(&uid)
        .bind(&client_secret)
        .bind(&registration.redirect_uri)
        .bind(&scopes)
        .bind(&registration.website)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(OAuthApplicationRegistrationResult {
            application,
            client_secret,
        })
    }

    pub async fn create_oauth_authorization_grant(
        &self,
        client_id: &str,
        resource_owner_id: i64,
        redirect_uri: &str,
        requested_scopes: Option<&str>,
        code_challenge: Option<&str>,
        code_challenge_method: Option<&str>,
    ) -> Result<OAuthAuthorizationGrant, OAuthAuthorizationGrantError> {
        let mut transaction = self.pool.begin().await?;
        let Some((application_id, application_redirect_uri, application_scopes, confidential)) =
            sqlx::query_as::<_, (i64, String, String, bool)>(
                "SELECT id, redirect_uri, scopes, confidential \
                 FROM oauth_applications WHERE uid = $1",
            )
            .bind(client_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Err(OAuthAuthorizationGrantError::InvalidClient);
        };
        if !application_redirect_uri
            .split_whitespace()
            .any(|registered| registered == redirect_uri)
        {
            return Err(OAuthAuthorizationGrantError::InvalidRedirectUri);
        }
        if !oauth_grant_pkce_is_valid(confidential, code_challenge, code_challenge_method) {
            return Err(OAuthAuthorizationGrantError::InvalidCodeChallenge);
        }
        let scopes = requested_scopes
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| "read".to_owned(), canonical_oauth_scopes);
        let application_scopes = application_scopes.split_whitespace().collect::<Vec<_>>();
        if scopes.split_whitespace().any(|scope| {
            !application_scopes.contains(&scope) || !OAUTH_CONFIGURED_SCOPES.contains(&scope)
        }) {
            return Err(OAuthAuthorizationGrantError::InvalidScope);
        }
        let code = random_urlsafe_base64(32);
        let (code, scopes) = sqlx::query_as::<_, (String, String)>(
            "INSERT INTO oauth_access_grants ( \
                application_id, code_challenge, code_challenge_method, created_at, expires_in, \
                redirect_uri, resource_owner_id, revoked_at, scopes, token) \
             VALUES ($1, $2, $3, clock_timestamp(), 600, $4, $5, NULL, $6, $7) \
             RETURNING token, scopes",
        )
        .bind(application_id)
        .bind(code_challenge)
        .bind(code_challenge_method)
        .bind(redirect_uri)
        .bind(resource_owner_id)
        .bind(&scopes)
        .bind(&code)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(OAuthAuthorizationGrant { code, scopes })
    }

    pub async fn issue_oauth_client_credentials_token(
        &self,
        client_id: &str,
        client_secret: &str,
        requested_scopes: Option<&str>,
    ) -> Result<OAuthClientCredentialsToken, OAuthClientCredentialsError> {
        if client_id.trim().is_empty() || client_secret.is_empty() {
            return Err(OAuthClientCredentialsError::InvalidClient);
        }
        let mut transaction = self.pool.begin().await?;
        let Some((application_id, stored_secret, application_scopes, confidential)) =
            sqlx::query_as::<_, (i64, String, String, bool)>(
                "SELECT id, secret, scopes, confidential \
                 FROM oauth_applications WHERE uid = $1",
            )
            .bind(client_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Err(OAuthClientCredentialsError::InvalidClient);
        };
        if !confidential || !constant_time_string_equal(&stored_secret, client_secret) {
            return Err(OAuthClientCredentialsError::InvalidClient);
        }
        let scopes = requested_scopes
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| "read".to_owned(), canonical_oauth_scopes);
        let application_scopes = application_scopes.split_whitespace().collect::<Vec<_>>();
        if scopes.split_whitespace().any(|scope| {
            !application_scopes.contains(&scope) || !OAUTH_CONFIGURED_SCOPES.contains(&scope)
        }) {
            return Err(OAuthClientCredentialsError::InvalidScope);
        }
        if let Some((access_token, created_at)) = sqlx::query_as::<_, (String, NaiveDateTime)>(
            "SELECT token, created_at FROM oauth_access_tokens \
             WHERE application_id = $1 AND resource_owner_id IS NULL \
               AND scopes = $2 AND revoked_at IS NULL \
               AND (expires_in IS NULL OR created_at + expires_in * INTERVAL '1 second' > clock_timestamp()) \
             ORDER BY id LIMIT 1 FOR UPDATE",
        )
        .bind(application_id)
        .bind(&scopes)
        .fetch_optional(&mut *transaction)
        .await?
        {
            transaction.commit().await?;
            return Ok(OAuthClientCredentialsToken {
                access_token,
                scopes,
                created_at,
            });
        }
        let access_token = random_urlsafe_base64(32);
        let (access_token, created_at) = sqlx::query_as::<_, (String, NaiveDateTime)>(
            "INSERT INTO oauth_access_tokens ( \
               resource_owner_id, application_id, token, refresh_token, scopes, \
               expires_in, created_at, revoked_at, last_used_at, last_used_ip) \
             VALUES (NULL, $1, $2, NULL, $3, NULL, clock_timestamp(), NULL, NULL, NULL) \
             RETURNING token, created_at",
        )
        .bind(application_id)
        .bind(access_token)
        .bind(&scopes)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(OAuthClientCredentialsToken {
            access_token,
            scopes,
            created_at,
        })
    }

    #[allow(clippy::too_many_lines)]
    pub async fn issue_oauth_authorization_code_token(
        &self,
        client_id: &str,
        client_secret: Option<&str>,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> Result<OAuthAuthorizationCodeToken, OAuthAuthorizationCodeError> {
        if client_id.trim().is_empty() || code.is_empty() || redirect_uri.is_empty() {
            return Err(OAuthAuthorizationCodeError::InvalidGrant);
        }
        let mut transaction = self.pool.begin().await?;
        let Some((
            grant_id,
            application_id,
            stored_secret,
            confidential,
            grant_challenge,
            grant_challenge_method,
            grant_expires_in,
            grant_created_at,
            grant_redirect_uri,
            resource_owner_id,
            grant_revoked_at,
            grant_scopes,
        )) = sqlx::query_as::<
            _,
            (
                i64,
                i64,
                String,
                bool,
                Option<String>,
                Option<String>,
                i32,
                NaiveDateTime,
                String,
                i64,
                Option<NaiveDateTime>,
                Option<String>,
            ),
        >(
            "SELECT access_grant.id, access_grant.application_id, application.secret, \
                    application.confidential, access_grant.code_challenge, \
                    access_grant.code_challenge_method, access_grant.expires_in, \
                    access_grant.created_at, access_grant.redirect_uri, \
                    access_grant.resource_owner_id, access_grant.revoked_at, access_grant.scopes \
             FROM oauth_access_grants access_grant \
             JOIN oauth_applications application ON application.id = access_grant.application_id \
             WHERE application.uid = $1 AND access_grant.token = $2 \
             FOR UPDATE OF access_grant",
        )
        .bind(client_id)
        .bind(code)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Err(OAuthAuthorizationCodeError::InvalidGrant);
        };
        if confidential
            && !constant_time_string_equal(&stored_secret, client_secret.unwrap_or_default())
        {
            return Err(OAuthAuthorizationCodeError::InvalidClient);
        }
        if grant_redirect_uri != redirect_uri
            || grant_revoked_at.is_some()
            || grant_created_at
                .checked_add_signed(ChronoDuration::seconds(i64::from(grant_expires_in)))
                .is_none_or(|expires_at| expires_at <= Utc::now().naive_utc())
            || resource_owner_id <= 0
        {
            return Err(OAuthAuthorizationCodeError::InvalidGrant);
        }
        if !oauth_pkce_matches(
            confidential,
            grant_challenge.as_deref(),
            grant_challenge_method.as_deref(),
            code_verifier,
        ) {
            return Err(OAuthAuthorizationCodeError::InvalidGrant);
        }
        sqlx::query("UPDATE oauth_access_grants SET revoked_at = clock_timestamp() WHERE id = $1")
            .bind(grant_id)
            .execute(&mut *transaction)
            .await?;
        let scopes = grant_scopes.unwrap_or_default();
        let access_token = random_urlsafe_base64(32);
        let (access_token, created_at) = sqlx::query_as::<_, (String, NaiveDateTime)>(
            "INSERT INTO oauth_access_tokens ( \
                resource_owner_id, application_id, token, refresh_token, scopes, expires_in, \
                created_at, revoked_at, last_used_at, last_used_ip) \
             VALUES ($1, $2, $3, NULL, $4, NULL, clock_timestamp(), NULL, NULL, NULL) \
             RETURNING token, created_at",
        )
        .bind(resource_owner_id)
        .bind(application_id)
        .bind(access_token)
        .bind(&scopes)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(OAuthAuthorizationCodeToken {
            access_token,
            scopes,
            created_at,
        })
    }

    /// Authenticates a persisted Mastodon login and records the result.
    ///
    /// # Errors
    ///
    /// Returns a stable authentication failure or a database error.
    ///
    /// # Panics
    ///
    /// Panics only if the current Unix TOTP timestep cannot fit the persisted
    /// `PostgreSQL` integer column.
    #[allow(clippy::too_many_lines)]
    pub async fn authenticate_browser_user(
        &self,
        email: &str,
        password: &str,
        otp_attempt: Option<&str>,
        timestamp: i64,
        ip: IpNetwork,
        user_agent: &str,
    ) -> Result<BrowserAuthentication, BrowserAuthenticationError> {
        let mut transaction = self.pool.begin().await?;
        let user = sqlx::query_as::<_, BrowserLoginUser>(
            "SELECT u.id, u.account_id, u.encrypted_password, \
                    u.otp_backup_codes::text[] AS otp_backup_codes, \
                    u.otp_required_for_login, u.otp_secret, u.consumed_timestep, \
                    u.approved, u.confirmed_at, \
                    COALESCE(role.require_2fa, false) AS role_requires_2fa, \
                    EXISTS (SELECT 1 FROM webauthn_credentials credential \
                            WHERE credential.user_id = u.id) AS has_webauthn_credentials, \
                    account.memorial AS account_memorial \
             FROM users u \
             JOIN accounts account ON account.id = u.account_id \
             LEFT JOIN user_roles role ON role.id = u.role_id \
             WHERE lower(u.email) = lower($1) \
             FOR UPDATE OF u",
        )
        .bind(email)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(mut user) = user else {
            let _ = verify_password(password, DUMMY_BCRYPT_PASSWORD);
            transaction.commit().await?;
            return Err(BrowserAuthenticationError::InvalidCredentials);
        };
        if !verify_password(password, user.encrypted_password.as_str()) {
            record_login_activity(
                &mut transaction,
                user.id,
                "password",
                Some("invalid_password"),
                false,
                ip,
                user_agent,
            )
            .await?;
            transaction.commit().await?;
            return Err(BrowserAuthenticationError::InvalidCredentials);
        }
        if user.confirmed_at.is_none() {
            record_login_activity(
                &mut transaction,
                user.id,
                "password",
                Some("unconfirmed"),
                false,
                ip,
                user_agent,
            )
            .await?;
            transaction.commit().await?;
            return Err(BrowserAuthenticationError::Unconfirmed);
        }
        if !user.approved {
            record_login_activity(
                &mut transaction,
                user.id,
                "password",
                Some("pending_approval"),
                false,
                ip,
                user_agent,
            )
            .await?;
            transaction.commit().await?;
            return Err(BrowserAuthenticationError::PendingApproval);
        }
        if user.account_memorial {
            record_login_activity(
                &mut transaction,
                user.id,
                "password",
                Some("inactive"),
                false,
                ip,
                user_agent,
            )
            .await?;
            transaction.commit().await?;
            return Err(BrowserAuthenticationError::Memorialized);
        }

        let otp_secret = self.decrypt_otp_secret(user.otp_secret.take())?;
        let requires_two_factor = user.otp_required_for_login || user.has_webauthn_credentials;
        let (method, two_factor_update) = if requires_two_factor {
            let Some(otp_attempt) = otp_attempt.filter(|attempt| !attempt.trim().is_empty()) else {
                transaction.commit().await?;
                return Err(BrowserAuthenticationError::TwoFactorRequired);
            };
            let failures = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM login_activities activity \
                 WHERE activity.user_id = $1 AND activity.success = false \
                   AND activity.authentication_method IN ('otp', 'backup_code') \
                   AND activity.created_at > clock_timestamp()::timestamp - INTERVAL '1 hour' \
                   AND activity.created_at > COALESCE( \
                       (SELECT MAX(success_activity.created_at) \
                          FROM login_activities success_activity \
                         WHERE success_activity.user_id = $1 \
                           AND success_activity.success = true), \
                       clock_timestamp()::timestamp - INTERVAL '1 hour')",
            )
            .bind(user.id)
            .fetch_one(&mut *transaction)
            .await?;
            if two_factor_attempt_is_rate_limited(failures) {
                transaction.commit().await?;
                return Err(BrowserAuthenticationError::RateLimited);
            }
            let backup_codes = user
                .otp_backup_codes
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|code| code.as_str().to_owned())
                .collect::<Vec<_>>();
            match verify_two_factor(
                otp_secret.as_ref().map(super::types::SecretText::as_str),
                &backup_codes,
                otp_attempt,
                timestamp,
                user.consumed_timestep.map(i64::from),
            ) {
                TwoFactorVerification::Totp(timestep) => (
                    BrowserAuthenticationMethod::Totp,
                    Some((
                        Some(
                            i32::try_from(timestep)
                                .expect("current TOTP timestep fits PostgreSQL integer"),
                        ),
                        None,
                    )),
                ),
                TwoFactorVerification::BackupCode(index) => {
                    let mut remaining = backup_codes;
                    remaining.remove(index);
                    (
                        BrowserAuthenticationMethod::BackupCode,
                        Some((None, Some(remaining))),
                    )
                }
                TwoFactorVerification::Invalid => {
                    record_login_activity(
                        &mut transaction,
                        user.id,
                        "otp",
                        Some("invalid_otp_token"),
                        false,
                        ip,
                        user_agent,
                    )
                    .await?;
                    transaction.commit().await?;
                    return Err(BrowserAuthenticationError::InvalidTwoFactor);
                }
            }
        } else {
            (BrowserAuthenticationMethod::Password, None)
        };

        if let Some((consumed_timestep, backup_codes)) = two_factor_update {
            if let Some(consumed_timestep) = consumed_timestep {
                sqlx::query(
                    "UPDATE users SET consumed_timestep = $1, updated_at = clock_timestamp() \
                     WHERE id = $2",
                )
                .bind(consumed_timestep)
                .bind(user.id)
                .execute(&mut *transaction)
                .await?;
            } else if let Some(backup_codes) = backup_codes {
                sqlx::query(
                    "UPDATE users SET otp_backup_codes = $1, updated_at = clock_timestamp() \
                     WHERE id = $2",
                )
                .bind(backup_codes)
                .bind(user.id)
                .execute(&mut *transaction)
                .await?;
            }
        }
        sqlx::query(
            "UPDATE users SET last_sign_in_at = current_sign_in_at, \
                    current_sign_in_at = clock_timestamp(), sign_in_count = sign_in_count + 1, \
                    updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;
        record_login_activity(
            &mut transaction,
            user.id,
            match method {
                BrowserAuthenticationMethod::Password => "password",
                BrowserAuthenticationMethod::Totp | BrowserAuthenticationMethod::BackupCode => {
                    "otp"
                }
            },
            None,
            true,
            ip,
            user_agent,
        )
        .await?;
        transaction.commit().await?;
        Ok(BrowserAuthentication {
            user_id: user.id,
            account_id: user.account_id,
            method,
            password: VerifiedPassword {
                user_id: user.id,
                encrypted_password: user.encrypted_password,
            },
        })
    }

    /// Verifies a current password; authorized writes must fence this proof in their transaction.
    pub async fn verify_current_password(
        &self,
        user_id: i64,
        password: &str,
    ) -> Result<VerifiedPassword, WriteError> {
        let encrypted_password = sqlx::query_scalar::<_, super::types::SecretText>(
            "SELECT encrypted_password FROM users WHERE id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(WriteError::Unauthorized)?;
        if !verify_password(password, encrypted_password.as_str()) {
            return Err(WriteError::Unauthorized);
        }
        Ok(VerifiedPassword {
            user_id,
            encrypted_password,
        })
    }

    /// Changes a password only if the checked credential is still current under the user lock.
    pub async fn change_user_password(
        &self,
        authentication: &VerifiedPassword,
        password: &str,
    ) -> Result<(), WriteError> {
        validate_local_password(password)?;
        let mut transaction = self.pool.begin().await?;
        let account_id = lock_verified_password_in(&mut transaction, authentication).await?;
        replace_user_password_in(
            &mut transaction,
            authentication.user_id,
            account_id,
            password,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Enables TOTP authentication and returns the one-time backup codes.
    ///
    /// The database stores only bcrypt hashes of the returned codes. The caller must
    /// present the codes to the user immediately because they cannot be recovered later.
    pub async fn enable_two_factor_authentication(
        &self,
        user_id: i64,
        otp_secret: &str,
    ) -> Result<Vec<String>, WriteError> {
        if user_id <= 0 || !valid_totp_secret(otp_secret) {
            return Err(WriteError::InvalidInput("invalid two-factor setup"));
        }
        let stored_otp_secret = self.encrypt_otp_secret(otp_secret)?;
        let mut transaction = self.pool.begin().await?;
        let enabled = sqlx::query_scalar::<_, bool>(
            "SELECT otp_required_for_login FROM users WHERE id = $1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if enabled {
            return Err(WriteError::Validation(
                "two-factor authentication is already enabled",
            ));
        }
        let (backup_codes, encrypted_backup_codes) = generate_backup_codes().await?;
        sqlx::query(
            "UPDATE users SET otp_secret = $1, otp_backup_codes = $2, \
                    otp_required_for_login = true, consumed_timestep = NULL, \
                    updated_at = clock_timestamp() WHERE id = $3",
        )
        .bind(stored_otp_secret)
        .bind(encrypted_backup_codes)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(backup_codes)
    }

    /// Disables TOTP authentication after verifying the user's current password.
    pub async fn disable_two_factor_authentication(
        &self,
        user_id: i64,
        current_password: &str,
    ) -> Result<(), WriteError> {
        if user_id <= 0 || current_password.is_empty() {
            return Err(WriteError::InvalidInput(
                "user ID and current password must not be empty",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let (encrypted_password, role_requires_2fa) = sqlx::query_as::<_, (String, bool)>(
            "SELECT user_record.encrypted_password, COALESCE(role.require_2fa, false) \
               FROM users user_record \
               LEFT JOIN user_roles role ON role.id = user_record.role_id \
              WHERE user_record.id = $1 \
              FOR UPDATE OF user_record",
        )
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if !verify_password(current_password, &encrypted_password) {
            return Err(WriteError::Unauthorized);
        }
        if role_requires_2fa {
            return Err(WriteError::Validation(
                "two-factor authentication is required by the user's role",
            ));
        }
        sqlx::query(
            "UPDATE users SET otp_required_for_login = false, otp_secret = NULL, \
                    otp_backup_codes = ARRAY[]::text[], consumed_timestep = NULL, \
                    updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM webauthn_credentials WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Replaces the stored backup codes after verifying the user's current password.
    pub async fn regenerate_two_factor_backup_codes(
        &self,
        user_id: i64,
        current_password: &str,
    ) -> Result<Vec<String>, WriteError> {
        if user_id <= 0 || current_password.is_empty() {
            return Err(WriteError::InvalidInput(
                "user ID and current password must not be empty",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let user = sqlx::query_as::<_, (String, bool)>(
            "SELECT encrypted_password, otp_required_for_login \
               FROM users WHERE id = $1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(WriteError::NotFound)?;
        if !verify_password(current_password, &user.0) {
            return Err(WriteError::Unauthorized);
        }
        if !user.1 {
            return Err(WriteError::Validation(
                "two-factor authentication is not enabled",
            ));
        }
        let (backup_codes, encrypted_backup_codes) = generate_backup_codes().await?;
        sqlx::query(
            "UPDATE users SET otp_backup_codes = $1, updated_at = clock_timestamp() \
             WHERE id = $2",
        )
        .bind(encrypted_backup_codes)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(backup_codes)
    }

    /// Replaces an existing user's password and invalidates active sessions and tokens.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error when the password cannot be stored.
    pub async fn reset_user_password_by_email(
        &self,
        email: &str,
        password: &str,
    ) -> Result<bool, WriteError> {
        if email.trim().is_empty() || password.is_empty() {
            return Err(WriteError::InvalidInput(
                "email and password must not be empty",
            ));
        }
        validate_local_password(password)?;
        let mut transaction = self.pool.begin().await?;
        let user_id = sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, account_id FROM users \
              WHERE lower(email) = lower($1) AND COALESCE(encrypted_password, '') <> '' \
              FOR UPDATE",
        )
        .bind(email)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((user_id, account_id)) = user_id else {
            transaction.commit().await?;
            return Ok(false);
        };
        replace_user_password_in(&mut transaction, user_id, account_id, password).await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Creates a single-use password-reset token for an existing user.
    ///
    /// The returned value is the only copy of the token. This compatibility
    /// helper stores the legacy SHA-256 digest.
    pub async fn create_password_reset_token(
        &self,
        email: &str,
    ) -> Result<Option<String>, WriteError> {
        if email.trim().is_empty() {
            return Err(WriteError::InvalidInput("email must not be empty"));
        }
        let token = random_urlsafe_base64(32);
        let digest = password_reset_digest(&token);
        let result = sqlx::query(
            "UPDATE users SET reset_password_token = $1, reset_password_sent_at = clock_timestamp(), \
                    updated_at = clock_timestamp() \
             WHERE lower(email) = lower($2) AND COALESCE(encrypted_password, '') <> ''",
        )
        .bind(digest)
        .bind(email)
        .execute(&self.pool)
        .await?;
        Ok((result.rows_affected() == 1).then_some(token))
    }

    /// Creates a password-reset token and records its mail job atomically.
    ///
    /// # Errors
    ///
    /// Returns a validation, job, or database error when the token or outbox event cannot be
    /// stored. The plaintext token is only passed to the in-memory job factory.
    pub async fn create_password_reset_token_with_job<F>(
        &self,
        email: &str,
        token_digest_secret: Option<&str>,
        job_factory: F,
    ) -> Result<Option<String>, WriteError>
    where
        F: FnOnce(&str) -> Result<JobSpec, WriteError>,
    {
        if email.trim().is_empty() {
            return Err(WriteError::InvalidInput("email must not be empty"));
        }
        let mut transaction = self.pool.begin().await?;
        let user_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM users \
             WHERE lower(email) = lower($1) AND COALESCE(encrypted_password, '') <> '' \
             FOR UPDATE",
        )
        .bind(email)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(user_id) = user_id else {
            transaction.commit().await?;
            return Ok(None);
        };
        let token = random_urlsafe_base64(32);
        let digest = devise_token_digest("reset_password_token", &token, token_digest_secret);
        let job = job_factory(&token)?;
        sqlx::query(
            "UPDATE users SET reset_password_token = $1, reset_password_sent_at = clock_timestamp(), \
                    updated_at = clock_timestamp() WHERE id = $2",
        )
        .bind(digest)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        record_outbox_in(&mut transaction, &job).await?;
        transaction.commit().await?;
        Ok(Some(token))
    }

    /// Creates a local user as a confirmed account without invoking registration or invites.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error when the account cannot be created.
    pub async fn create_local_user(
        &self,
        email: &str,
        username: &str,
        password: &str,
    ) -> Result<CreatedLocalUser, WriteError> {
        self.insert_local_user(email, username, password, None)
            .await
    }

    /// Creates an unconfirmed local user and records its confirmation mail atomically.
    ///
    /// # Errors
    ///
    /// Returns a validation, job, or database error when the account or confirmation event cannot
    /// be created.
    pub async fn create_local_user_with_confirmation(
        &self,
        email: &str,
        username: &str,
        password: &str,
        confirmation_token: &str,
        token_digest_secret: Option<&str>,
        confirmation_job: &JobSpec,
    ) -> Result<CreatedLocalUser, WriteError> {
        if confirmation_token.is_empty() {
            return Err(WriteError::InvalidInput(
                "confirmation token must not be empty",
            ));
        }
        self.insert_local_user(
            email,
            username,
            password,
            Some((confirmation_token, token_digest_secret, confirmation_job)),
        )
        .await
    }

    /// Confirms an account using a non-expired single-use confirmation token.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error when confirmation cannot be checked.
    pub async fn confirm_user_with_token(&self, token: &str) -> Result<bool, WriteError> {
        self.confirm_user_with_token_and_optional_secret(token, None)
            .await
    }

    /// Confirms an account using the Rails secret key base, with a legacy digest fallback.
    pub async fn confirm_user_with_token_and_secret(
        &self,
        token: &str,
        token_digest_secret: &str,
    ) -> Result<bool, WriteError> {
        self.confirm_user_with_token_and_optional_secret(token, Some(token_digest_secret))
            .await
    }

    async fn confirm_user_with_token_and_optional_secret(
        &self,
        token: &str,
        token_digest_secret: Option<&str>,
    ) -> Result<bool, WriteError> {
        if token.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "confirmation token must not be empty",
            ));
        }
        let digest = devise_token_digest("confirmation_token", token, token_digest_secret);
        let legacy_digest = password_reset_digest(token);
        let mut transaction = self.pool.begin().await?;
        let user_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM users \
             WHERE confirmed_at IS NULL AND confirmation_token IN ($1, $2, $3) \
               AND confirmation_sent_at > clock_timestamp() - INTERVAL '2 days' \
             FOR UPDATE",
        )
        .bind(digest)
        .bind(legacy_digest)
        .bind(token)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(user_id) = user_id else {
            transaction.commit().await?;
            return Ok(false);
        };
        sqlx::query(
            "UPDATE users SET confirmed_at = clock_timestamp(), confirmation_token = NULL, \
                    confirmation_sent_at = NULL, updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    async fn insert_local_user(
        &self,
        email: &str,
        username: &str,
        password: &str,
        confirmation: Option<(&str, Option<&str>, &JobSpec)>,
    ) -> Result<CreatedLocalUser, WriteError> {
        let email = normalize_local_email(email)?;
        let username = normalize_local_username(username)?;
        validate_local_password(password)?;
        let encrypted_password =
            hash(password, DEFAULT_COST).map_err(|_| WriteError::Validation("invalid password"))?;
        let (private_key, public_key) = local_signing_keys()?;
        let (confirmed, confirmation_token) =
            confirmation.map_or((true, None), |(token, token_digest_secret, _)| {
                (
                    false,
                    Some(devise_token_digest(
                        "confirmation_token",
                        token,
                        token_digest_secret,
                    )),
                )
            });
        let mut transaction = self.pool.begin().await?;
        let account_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO accounts (username, private_key, public_key, created_at, updated_at) \
             VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp()) RETURNING id",
        )
        .bind(&username)
        .bind(private_key)
        .bind(public_key)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO account_stats (account_id, created_at, updated_at) \
             VALUES ($1, clock_timestamp(), clock_timestamp())",
        )
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
        let user_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO users (account_id, email, encrypted_password, approved, confirmed_at, \
                    confirmation_token, confirmation_sent_at, created_at, updated_at) \
             VALUES ($1, $2, $3, true, CASE WHEN $4 THEN clock_timestamp() ELSE NULL END, \
                    $5, CASE WHEN $4 THEN NULL ELSE clock_timestamp() END, \
                    clock_timestamp(), clock_timestamp()) RETURNING id",
        )
        .bind(account_id)
        .bind(&email)
        .bind(encrypted_password)
        .bind(confirmed)
        .bind(confirmation_token)
        .fetch_one(&mut *transaction)
        .await?;
        if let Some((_, _, confirmation_job)) = confirmation {
            record_outbox_in(&mut transaction, confirmation_job).await?;
        }
        transaction.commit().await?;
        Ok(CreatedLocalUser {
            account_id,
            user_id,
            confirmed,
        })
    }

    /// Consumes a non-expired password-reset token and revokes prior access.
    pub async fn reset_password_with_token(
        &self,
        token: &str,
        password: &str,
    ) -> Result<bool, WriteError> {
        self.reset_password_with_token_and_optional_secret(token, password, None)
            .await
    }

    /// Consumes a password-reset token using the Rails secret key base, with a legacy digest fallback.
    pub async fn reset_password_with_token_and_secret(
        &self,
        token: &str,
        password: &str,
        token_digest_secret: &str,
    ) -> Result<bool, WriteError> {
        self.reset_password_with_token_and_optional_secret(
            token,
            password,
            Some(token_digest_secret),
        )
        .await
    }

    async fn reset_password_with_token_and_optional_secret(
        &self,
        token: &str,
        password: &str,
        token_digest_secret: Option<&str>,
    ) -> Result<bool, WriteError> {
        if token.trim().is_empty() || password.is_empty() {
            return Err(WriteError::InvalidInput(
                "reset token and password must not be empty",
            ));
        }
        validate_local_password(password)?;
        let digest = devise_token_digest("reset_password_token", token, token_digest_secret);
        let legacy_digest = password_reset_digest(token);
        let mut transaction = self.pool.begin().await?;
        let user_id = sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, account_id FROM users \
              WHERE reset_password_token IN ($1, $2) \
                AND COALESCE(encrypted_password, '') <> '' \
               AND reset_password_sent_at > clock_timestamp() - INTERVAL '6 hours' \
             FOR UPDATE",
        )
        .bind(digest)
        .bind(legacy_digest)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((user_id, account_id)) = user_id else {
            transaction.commit().await?;
            return Ok(false);
        };
        replace_user_password_in(&mut transaction, user_id, account_id, password).await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn create_browser_session(
        &self,
        authentication: &BrowserAuthentication,
        ip: IpNetwork,
        user_agent: &str,
    ) -> Result<String, WriteError> {
        let session_id = random_urlsafe_base64(32);
        let access_token = random_urlsafe_base64(32);
        let mut transaction = self.pool.begin().await?;
        lock_verified_password_in(&mut transaction, &authentication.password).await?;
        let user_id = authentication.password.user_id;
        let application_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM oauth_applications WHERE superapp = true ORDER BY id LIMIT 1",
        )
        .fetch_optional(&mut *transaction)
        .await?;
        let access_token_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO oauth_access_tokens ( \
                application_id, created_at, expires_in, last_used_at, last_used_ip, \
                refresh_token, resource_owner_id, revoked_at, scopes, token) \
             VALUES ($1, clock_timestamp(), NULL, NULL, NULL, NULL, $2, NULL, \
                     'read write follow', $3) \
             RETURNING id",
        )
        .bind(application_id)
        .bind(user_id)
        .bind(access_token)
        .fetch_one(&mut *transaction)
        .await?;
        let session_id = sqlx::query_scalar::<_, String>(
            "INSERT INTO session_activations ( \
                access_token_id, created_at, ip, session_id, updated_at, user_agent, user_id, \
                web_push_subscription_id) \
             VALUES ($1, clock_timestamp(), $2, $3, clock_timestamp(), $4, $5, NULL) \
             RETURNING session_id",
        )
        .bind(access_token_id)
        .bind(ip)
        .bind(session_id)
        .bind(user_agent)
        .bind(user_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(session_id)
    }

    pub async fn touch_browser_session(&self, session_id: &str) -> sqlx::Result<bool> {
        let result = sqlx::query(
            "UPDATE session_activations SET updated_at = clock_timestamp() \
             WHERE session_id = $1 AND updated_at > clock_timestamp() - INTERVAL '30 days'",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn delete_browser_session(&self, session_id: &str) -> Result<(), WriteError> {
        let mut transaction = self.pool.begin().await?;
        let token_row = sqlx::query_as::<_, (Option<i64>, Option<i64>, i64)>(
            "SELECT session.access_token_id, session.web_push_subscription_id, user_record.account_id
               FROM session_activations session
               JOIN users user_record ON user_record.id = session.user_id
              WHERE session.session_id = $1 FOR UPDATE",
        )
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM session_activations WHERE session_id = $1")
            .bind(session_id)
            .execute(&mut *transaction)
            .await?;
        if let Some((access_token_id, web_push_subscription_id, account_id)) = token_row {
            if let Some(web_push_subscription_id) = web_push_subscription_id {
                sqlx::query("DELETE FROM web_push_subscriptions WHERE id = $1")
                    .bind(web_push_subscription_id)
                    .execute(&mut *transaction)
                    .await?;
            }
            if let Some(access_token_id) = access_token_id {
                sqlx::query("DELETE FROM oauth_access_tokens WHERE id = $1")
                    .bind(access_token_id)
                    .execute(&mut *transaction)
                    .await?;
                record_token_kill_stream_event(&mut transaction, account_id, access_token_id)
                    .await?;
            }
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn revoke_oauth_token(
        &self,
        client_id: &str,
        client_secret: &str,
        token: Option<&str>,
        token_type_hint: Option<&str>,
    ) -> Result<(), OAuthTokenRevocationError> {
        if client_id.trim().is_empty() {
            return Err(OAuthTokenRevocationError::InvalidClient);
        }
        let mut transaction = self.pool.begin().await?;
        let Some((application_id, stored_secret, confidential)) =
            sqlx::query_as::<_, (i64, String, bool)>(
                "SELECT id, secret, confidential FROM oauth_applications WHERE uid = $1",
            )
            .bind(client_id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            return Err(OAuthTokenRevocationError::InvalidClient);
        };
        if confidential
            && (client_secret.is_empty()
                || !constant_time_string_equal(&stored_secret, client_secret))
        {
            return Err(OAuthTokenRevocationError::InvalidClient);
        }
        let Some(token) = token.filter(|token| !token.is_empty()) else {
            transaction.commit().await?;
            return Ok(());
        };
        let token_row = if token_type_hint == Some("refresh_token") {
            sqlx::query_as::<_, (i64, Option<i64>, Option<i64>)>(
                "SELECT access_token.id, access_token.application_id, token_user.account_id \
                   FROM oauth_access_tokens access_token \
                   LEFT JOIN users token_user ON token_user.id = access_token.resource_owner_id \
                  WHERE access_token.refresh_token = $1 \
                  LIMIT 1 FOR UPDATE OF access_token",
            )
            .bind(token)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            sqlx::query_as::<_, (i64, Option<i64>, Option<i64>)>(
                "SELECT access_token.id, access_token.application_id, token_user.account_id \
                   FROM oauth_access_tokens access_token \
                   LEFT JOIN users token_user ON token_user.id = access_token.resource_owner_id \
                  WHERE access_token.token = $1 \
                 LIMIT 1 FOR UPDATE OF access_token",
            )
            .bind(token)
            .fetch_optional(&mut *transaction)
            .await?
            .or(sqlx::query_as::<_, (i64, Option<i64>, Option<i64>)>(
                "SELECT access_token.id, access_token.application_id, token_user.account_id \
                   FROM oauth_access_tokens access_token \
                   LEFT JOIN users token_user ON token_user.id = access_token.resource_owner_id \
                  WHERE access_token.refresh_token = $1 \
                     LIMIT 1 FOR UPDATE OF access_token",
            )
            .bind(token)
            .fetch_optional(&mut *transaction)
            .await?)
        };
        let Some((token_id, token_application_id, account_id)) = token_row else {
            transaction.commit().await?;
            return Ok(());
        };
        if token_application_id != Some(application_id) {
            return Err(OAuthTokenRevocationError::UnauthorizedClient);
        }
        sqlx::query(
            "UPDATE oauth_access_tokens SET revoked_at = COALESCE(revoked_at, clock_timestamp()) \
             WHERE id = $1",
        )
        .bind(token_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM web_push_subscriptions WHERE access_token_id = $1")
            .bind(token_id)
            .execute(&mut *transaction)
            .await?;
        if let Some(account_id) = account_id {
            record_token_kill_stream_event(&mut transaction, account_id, token_id).await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Creates idempotent status notifications for active local followers who enabled them.
    ///
    /// The caller is expected to invoke this from the post-commit core worker. A missing or
    /// deleted status is treated as settled work so retries cannot resurrect notifications.
    pub async fn notify_status_followers(&self, status_id: i64) -> Result<u64, WriteError> {
        let Some((author_id, visibility, in_reply_to_account_id, language)) =
            sqlx::query_as::<_, (i64, i32, Option<i64>, Option<String>)>(
                "SELECT status.account_id, status.visibility, status.in_reply_to_account_id \
                        , status.language \
                 FROM statuses status \
                 JOIN accounts author ON author.id = status.account_id \
                   AND author.domain IS NULL AND author.suspended_at IS NULL \
                 WHERE status.id = $1 AND status.local IS TRUE AND status.deleted_at IS NULL",
            )
            .bind(status_id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(0);
        };
        if !matches!(visibility, 0..=4)
            || in_reply_to_account_id.is_some_and(|account_id| account_id != author_id)
        {
            return Ok(0);
        }
        let mut created = 0_u64;
        let mut after_account_id = None;
        loop {
            let recipients = sqlx::query_scalar::<_, i64>(
                "SELECT follow.account_id \
                 FROM follows follow \
                 JOIN accounts follower ON follower.id = follow.account_id \
                 JOIN users follower_user ON follower_user.account_id = follower.id \
                 WHERE follow.target_account_id = $1 AND follow.notify IS TRUE \
                   AND follower.domain IS NULL AND follower.suspended_at IS NULL \
                    AND follower_user.current_sign_in_at >= clock_timestamp() \
                      - make_interval(days => $7) \
                   AND (follow.languages IS NULL OR cardinality(follow.languages) = 0 \
                        OR $4::text IS NULL OR $4 = ANY(follow.languages)) \
                   AND ( \
                     $2 IN (0, 1, 2) \
                     OR EXISTS ( \
                       SELECT 1 FROM mentions mention \
                       WHERE mention.status_id = $3 \
                         AND mention.account_id = follow.account_id \
                         AND mention.silent IS FALSE)) \
                   AND NOT EXISTS ( \
                     SELECT 1 FROM blocks blocked_by \
                     WHERE blocked_by.account_id = $1 \
                       AND blocked_by.target_account_id = follow.account_id) \
                   AND NOT EXISTS ( \
                     SELECT 1 FROM mentions mention \
                     WHERE mention.status_id = $3 AND mention.silent IS FALSE \
                       AND ( \
                         EXISTS ( \
                           SELECT 1 FROM blocks block \
                           WHERE block.account_id = follow.account_id \
                             AND block.target_account_id = mention.account_id) \
                         OR EXISTS ( \
                           SELECT 1 FROM mutes mute \
                           WHERE mute.account_id = follow.account_id \
                             AND mute.target_account_id = mention.account_id \
                             AND mute.hide_notifications IS TRUE \
                             AND (mute.expires_at IS NULL OR mute.expires_at > clock_timestamp())))) \
                   AND ($5::bigint IS NULL OR follow.account_id > $5) \
                 ORDER BY follow.account_id LIMIT $6",
            )
            .bind(author_id)
            .bind(visibility)
            .bind(status_id)
            .bind(&language)
            .bind(after_account_id)
            .bind(STATUS_NOTIFICATION_BATCH_SIZE)
            .bind(configured_user_active_days())
            .fetch_all(&self.pool)
            .await?;
            let Some(last_account_id) = recipients.last().copied() else {
                break;
            };
            after_account_id = Some(last_account_id);
            for recipient_account_id in recipients {
                if matches!(
                    self.create_notification(NotificationCreate {
                        recipient_account_id,
                        activity: NotificationActivity::Status { id: status_id },
                        silenced: false,
                    })
                    .await?,
                    NotificationCreateOutcome::Created { .. }
                ) {
                    created = created.saturating_add(1);
                }
            }
        }
        Ok(created)
    }

    #[allow(clippy::too_many_lines)]
    pub async fn create_notification(
        &self,
        request: NotificationCreate,
    ) -> Result<NotificationCreateOutcome, WriteError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(request.recipient_account_id)
            .execute(&mut *transaction)
            .await?;
        let Some(activity) = resolve_notification_activity(
            &mut transaction,
            request.activity,
            request.recipient_account_id,
        )
        .await?
        else {
            transaction.commit().await?;
            return Ok(NotificationCreateOutcome::Dropped);
        };
        if notification_requires_active_sender(activity.notification_type) {
            let sender_available = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM accounts \
                   WHERE id = $1 AND suspended_at IS NULL)",
            )
            .bind(activity.from_account_id)
            .fetch_one(&mut *transaction)
            .await?;
            if !sender_available {
                transaction.commit().await?;
                return Ok(NotificationCreateOutcome::Dropped);
            }
        }
        if activity
            .target_account_id
            .is_some_and(|account_id| account_id != request.recipient_account_id)
        {
            transaction.commit().await?;
            return Ok(NotificationCreateOutcome::Dropped);
        }
        let replaces_existing = matches!(
            activity.notification_type,
            "update" | "quoted_update" | "collection_update"
        );
        if replaces_existing {
            sqlx::query(
                "DELETE FROM notifications WHERE account_id = $1 AND activity_id = $2 \
                 AND activity_type = $3 AND type = $4",
            )
            .bind(request.recipient_account_id)
            .bind(activity.activity_id)
            .bind(activity.activity_type)
            .bind(activity.notification_type)
            .execute(&mut *transaction)
            .await?;
        }
        if !replaces_existing
            && let Some(existing) = sqlx::query_as::<_, (i64, bool)>(
                "SELECT id, filtered FROM notifications \
                 WHERE account_id = $1 AND activity_id = $2 \
                   AND activity_type = $3 AND type = $4 ORDER BY id LIMIT 1 \
                 FOR UPDATE",
            )
            .bind(request.recipient_account_id)
            .bind(activity.activity_id)
            .bind(activity.activity_type)
            .bind(activity.notification_type)
            .fetch_optional(&mut *transaction)
            .await?
        {
            transaction.commit().await?;
            return Ok(NotificationCreateOutcome::Existing {
                id: existing.0,
                filtered: existing.1,
            });
        }
        let recipient_available = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM users WHERE account_id = $1) \
                AND ($2 OR NOT EXISTS (SELECT 1 FROM accounts WHERE id = $1 \
                  AND suspended_at IS NOT NULL))",
        )
        .bind(request.recipient_account_id)
        .bind(activity.notification_type == "moderation_warning")
        .fetch_one(&mut *transaction)
        .await?;
        if !recipient_available {
            transaction.commit().await?;
            return Ok(NotificationCreateOutcome::Dropped);
        }
        if activity.from_account_id == request.recipient_account_id
            && !matches!(
                activity.notification_type,
                "poll" | "severed_relationships" | "moderation_warning" | "annual_report"
            )
        {
            transaction.commit().await?;
            return Ok(NotificationCreateOutcome::Dropped);
        }
        let policy = notification_policy_facts(
            &mut transaction,
            request.recipient_account_id,
            activity.from_account_id,
            activity.target_status_id,
            activity.notification_type,
            request.silenced,
        )
        .await?;
        let blocked = if policy.staff_bypass && activity.notification_type == "mention" {
            false
        } else {
            sqlx::query_scalar::<_, bool>(
                r"SELECT
                     EXISTS (
                       SELECT 1 FROM blocks
                       WHERE account_id = $1 AND target_account_id = $2
                     )
                  OR EXISTS (
                       SELECT 1 FROM mutes
                       WHERE account_id = $1 AND target_account_id = $2
                         AND hide_notifications = true
                         AND (expires_at IS NULL OR expires_at > clock_timestamp())
                     )
                  OR EXISTS (
                       SELECT 1
                       FROM account_domain_blocks domain_block
                       JOIN accounts sender ON sender.id = $2
                       WHERE domain_block.account_id = $1
                         AND domain_block.domain = sender.domain
                         AND NOT EXISTS (
                           SELECT 1 FROM follows follow
                           WHERE follow.account_id = $1 AND follow.target_account_id = $2
                         )
                     )
                  OR EXISTS (
                       SELECT 1
                       FROM statuses status
                       JOIN conversation_mutes mute ON mute.conversation_id = status.conversation_id
                       WHERE status.id = $3 AND mute.account_id = $1
                     )
                  OR EXISTS (
                       SELECT 1
                       FROM statuses target
                       WHERE target.id = $3 AND $4 = 'mention'
                         AND (
                           EXISTS (
                             SELECT 1
                             FROM mentions mention
                             JOIN blocks block ON block.account_id = $1
                               AND block.target_account_id = mention.account_id
                             WHERE mention.status_id = target.id AND mention.silent = false
                           )
                           OR EXISTS (
                             SELECT 1
                             FROM mentions mention
                             JOIN mutes mute ON mute.account_id = $1
                               AND mute.target_account_id = mention.account_id
                               AND mute.hide_notifications = true
                               AND (mute.expires_at IS NULL OR mute.expires_at > clock_timestamp())
                             WHERE mention.status_id = target.id AND mention.silent = false
                           )
                           OR (
                             target.in_reply_to_account_id IS NOT NULL
                             AND (
                               EXISTS (
                                 SELECT 1 FROM blocks block
                                 WHERE block.account_id = $1
                                   AND block.target_account_id = target.in_reply_to_account_id
                               )
                               OR EXISTS (
                                 SELECT 1 FROM mutes mute
                                 WHERE mute.account_id = $1
                                   AND mute.target_account_id = target.in_reply_to_account_id
                                   AND mute.hide_notifications = true
                                   AND (mute.expires_at IS NULL OR mute.expires_at > clock_timestamp())
                               )
                             )
                           )
                         )
                     )",
            )
            .bind(request.recipient_account_id)
            .bind(activity.from_account_id)
            .bind(activity.target_status_id)
            .bind(activity.notification_type)
            .fetch_one(&mut *transaction)
            .await?
        };
        if blocked {
            transaction.commit().await?;
            return Ok(NotificationCreateOutcome::Dropped);
        }

        let filtered =
            match notification_policy_decision_for_type(&policy, activity.notification_type) {
                NotificationPolicyDecision::Drop => {
                    transaction.commit().await?;
                    return Ok(NotificationCreateOutcome::Dropped);
                }
                NotificationPolicyDecision::Filter => true,
                NotificationPolicyDecision::Accept => false,
            };

        let group_key = notification_group_key(
            &mut transaction,
            request.recipient_account_id,
            &activity,
            filtered,
        )
        .await?;
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO notifications ( \
               account_id, activity_id, activity_type, from_account_id, type, \
               group_key, filtered, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, clock_timestamp(), clock_timestamp()) \
             RETURNING id",
        )
        .bind(request.recipient_account_id)
        .bind(activity.activity_id)
        .bind(activity.activity_type)
        .bind(activity.from_account_id)
        .bind(activity.notification_type)
        .bind(group_key)
        .bind(filtered)
        .fetch_one(&mut *transaction)
        .await?;
        if !filtered {
            record_stream_event_in(
                &mut transaction,
                request.recipient_account_id,
                "notification",
                id,
                &event_logical_key(request.recipient_account_id, "notification", id, 0),
            )
            .await?;
            if let Some((conversation_id, lock_version)) = upsert_notification_conversation(
                &mut transaction,
                request.recipient_account_id,
                activity.target_status_id,
            )
            .await?
            {
                record_conversation_stream_event_in(
                    &mut transaction,
                    request.recipient_account_id,
                    conversation_id,
                    lock_version,
                )
                .await?;
            }
        }
        if filtered && matches!(activity.notification_type, "mention" | "quote") {
            upsert_notification_request(&mut transaction, &activity, request.recipient_account_id)
                .await?;
        }
        transaction.commit().await?;
        Ok(NotificationCreateOutcome::Created { id, filtered })
    }
}

fn notification_requires_active_sender(notification_type: &str) -> bool {
    matches!(
        notification_type,
        "mention"
            | "status"
            | "reblog"
            | "follow"
            | "follow_request"
            | "favourite"
            | "poll"
            | "update"
            | "quoted_update"
            | "quote"
            | "added_to_collection"
            | "collection_update"
    )
}

async fn upsert_notification_conversation(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    target_status_id: Option<i64>,
) -> Result<Option<(i64, i32)>, WriteError> {
    let Some(target_status_id) = target_status_id else {
        return Ok(None);
    };
    Ok(sqlx::query_as::<_, (i64, i32)>(
        "WITH direct AS ( \
           SELECT status.id AS status_id, status.conversation_id, status.account_id AS author_id, \
                  ARRAY( \
                    SELECT DISTINCT participant.account_id \
                      FROM ( \
                        SELECT mention.account_id \
                          FROM mentions mention \
                         WHERE mention.status_id = status.id AND mention.silent IS FALSE \
                        UNION ALL \
                        SELECT status.account_id \
                      ) participant \
                     WHERE participant.account_id <> $1 \
                     ORDER BY participant.account_id \
                  ) AS participant_account_ids \
             FROM statuses status \
            WHERE status.id = $2 AND status.visibility = 3 \
              AND status.deleted_at IS NULL AND status.conversation_id IS NOT NULL \
         ) \
         INSERT INTO account_conversations ( \
           account_id, conversation_id, last_status_id, participant_account_ids, status_ids, unread) \
          SELECT $1, conversation_id, status_id, participant_account_ids, ARRAY[status_id], author_id <> $1 \
            FROM direct \
         ON CONFLICT (account_id, conversation_id, participant_account_ids) \
         DO UPDATE SET \
           last_status_id = (SELECT max(status_id) \
                               FROM unnest(account_conversations.status_ids || EXCLUDED.status_ids) ids(status_id)), \
           status_ids = (SELECT ARRAY(SELECT DISTINCT status_id \
                                        FROM unnest(account_conversations.status_ids || EXCLUDED.status_ids) ids(status_id) \
                                       ORDER BY status_id)), \
           unread = EXCLUDED.unread, \
           lock_version = account_conversations.lock_version + 1 \
         WHERE NOT (EXCLUDED.status_ids <@ account_conversations.status_ids) \
         RETURNING id, lock_version",
    )
    .bind(account_id)
    .bind(target_status_id)
     .fetch_optional(&mut **transaction)
     .await?)
}

async fn record_conversation_stream_event_in(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    conversation_id: i64,
    lock_version: i32,
) -> Result<(), WriteError> {
    record_stream_event_in(
        transaction,
        account_id,
        "conversation",
        conversation_id,
        &event_logical_key(
            account_id,
            "conversation",
            conversation_id,
            i64::from(lock_version),
        ),
    )
    .await
    .map(|_| ())
    .map_err(WriteError::from)
}

async fn remove_favourites_for_account_and_statuses(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    status_ids: &[i64],
) -> Result<(), WriteError> {
    let favourite_status_ids = sqlx::query_scalar::<_, i64>(
        "SELECT favourite.status_id FROM favourites favourite
          WHERE favourite.account_id = $1 OR favourite.status_id = ANY($2::bigint[])
          ORDER BY favourite.id FOR UPDATE",
    )
    .bind(account_id)
    .bind(status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    for status_id in favourite_status_ids {
        decrement_favourite_count(transaction, status_id).await?;
    }
    sqlx::query(
        "DELETE FROM favourites
          WHERE account_id = $1 OR status_id = ANY($2::bigint[])",
    )
    .bind(account_id)
    .bind(status_ids)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn remove_favourites_for_statuses(
    transaction: &mut Transaction<'_, Postgres>,
    status_ids: &[i64],
) -> Result<(), WriteError> {
    if status_ids.is_empty() {
        return Ok(());
    }
    let favourite_status_ids = sqlx::query_scalar::<_, i64>(
        "SELECT favourite.status_id FROM favourites favourite
          WHERE favourite.status_id = ANY($1::bigint[])
          ORDER BY favourite.id FOR UPDATE",
    )
    .bind(status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    for status_id in favourite_status_ids {
        decrement_favourite_count(transaction, status_id).await?;
    }
    sqlx::query("DELETE FROM favourites WHERE status_id = ANY($1::bigint[])")
        .bind(status_ids)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn remove_poll_data_for_statuses(
    transaction: &mut Transaction<'_, Postgres>,
    status_ids: &[i64],
) -> Result<(), WriteError> {
    if status_ids.is_empty() {
        return Ok(());
    }
    let poll_ids = sqlx::query_scalar::<_, i64>(
        "SELECT poll.id FROM polls poll
          WHERE poll.status_id = ANY($1::bigint[])
          ORDER BY poll.id FOR UPDATE",
    )
    .bind(status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    if poll_ids.is_empty() {
        return Ok(());
    }
    sqlx::query("UPDATE statuses SET poll_id = NULL WHERE poll_id = ANY($1::bigint[])")
        .bind(&poll_ids)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM polls WHERE id = ANY($1::bigint[])")
        .bind(&poll_ids)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn remove_poll_data_for_account_and_statuses(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    status_ids: &[i64],
    protected_status_ids: &[i64],
) -> Result<(), WriteError> {
    let poll_ids = sqlx::query_scalar::<_, i64>(
        "SELECT poll.id FROM polls poll
          WHERE (poll.account_id = $1 OR poll.status_id = ANY($2::bigint[]))
            AND poll.status_id <> ALL($3::bigint[])
          ORDER BY poll.id FOR UPDATE",
    )
    .bind(account_id)
    .bind(status_ids)
    .bind(protected_status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let vote_rows = sqlx::query_as::<_, (i64, i32)>(
        "SELECT vote.poll_id, vote.choice FROM poll_votes vote
          WHERE vote.account_id = $1 ORDER BY vote.poll_id, vote.id",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mut votes_by_poll = HashMap::<i64, Vec<i32>>::new();
    for (poll_id, choice) in vote_rows {
        votes_by_poll.entry(poll_id).or_default().push(choice);
    }
    for (poll_id, choices) in votes_by_poll {
        let Some((mut cached_tallies, voter_total, ballot_total)) =
            sqlx::query_as::<_, (Vec<i64>, Option<i64>, i64)>(
                "SELECT cached_tallies, voters_count, votes_count FROM polls
                  WHERE id = $1 FOR UPDATE",
            )
            .bind(poll_id)
            .fetch_optional(&mut **transaction)
            .await?
        else {
            continue;
        };
        for choice in choices.iter().copied() {
            if let Some(tally) = choice
                .try_into()
                .ok()
                .and_then(|index: usize| cached_tallies.get_mut(index))
            {
                *tally = (*tally - 1).max(0);
            }
        }
        sqlx::query(
            "UPDATE polls SET cached_tallies = $2,
                votes_count = GREATEST($3 - $4, 0),
                voters_count = CASE WHEN $5 IS NULL THEN NULL ELSE GREATEST($5 - 1, 0) END,
                updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(poll_id)
        .bind(cached_tallies)
        .bind(ballot_total)
        .bind(i64::try_from(choices.len()).unwrap_or(i64::MAX))
        .bind(voter_total)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query("DELETE FROM poll_votes WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    if !poll_ids.is_empty() {
        sqlx::query("UPDATE statuses SET poll_id = NULL WHERE poll_id = ANY($1::bigint[])")
            .bind(&poll_ids)
            .execute(&mut **transaction)
            .await?;
        sqlx::query("DELETE FROM polls WHERE id = ANY($1::bigint[])")
            .bind(&poll_ids)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

async fn remove_statuses_from_account_conversations(
    transaction: &mut Transaction<'_, Postgres>,
    status_ids: &[i64],
) -> Result<(), WriteError> {
    if status_ids.is_empty() {
        return Ok(());
    }
    let conversations = sqlx::query_as::<_, (i64, i64, Vec<i64>)>(
        "SELECT id, account_id, status_ids FROM account_conversations
          WHERE status_ids && $1::bigint[] ORDER BY id FOR UPDATE",
    )
    .bind(status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    for (account_conversation_id, account_id, current_status_ids) in conversations {
        let mut remaining_status_ids = current_status_ids
            .into_iter()
            .filter(|current_status_id| !status_ids.contains(current_status_id))
            .collect::<Vec<_>>();
        remaining_status_ids.sort_unstable();
        if remaining_status_ids.is_empty() {
            sqlx::query("DELETE FROM account_conversations WHERE id = $1")
                .bind(account_conversation_id)
                .execute(&mut **transaction)
                .await?;
            continue;
        }
        let Some(last_status_id) = remaining_status_ids.last().copied() else {
            continue;
        };
        let lock_version = sqlx::query_scalar::<_, i32>(
            "UPDATE account_conversations SET status_ids = $2, last_status_id = $3,
                lock_version = lock_version + 1
              WHERE id = $1 RETURNING lock_version",
        )
        .bind(account_conversation_id)
        .bind(&remaining_status_ids)
        .bind(last_status_id)
        .fetch_one(&mut **transaction)
        .await?;
        record_conversation_stream_event_in(
            transaction,
            account_id,
            account_conversation_id,
            lock_version,
        )
        .await?;
    }
    Ok(())
}

async fn clear_account_interactions(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    target_account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "DELETE FROM notification_requests
          WHERE account_id = $1 AND from_account_id = $2",
    )
    .bind(account_id)
    .bind(target_account_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM notifications
          WHERE account_id = $1 AND from_account_id = $2",
    )
    .bind(account_id)
    .bind(target_account_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM account_conversations
          WHERE account_id = $1 AND $2 = ANY(participant_account_ids)",
    )
    .bind(account_id)
    .bind(target_account_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn reconcile_remote_actor_keypairs(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    actor: &RemoteActor,
) -> Result<(), WriteError> {
    let mut keys = actor.public_keys.iter().collect::<Vec<_>>();
    keys.sort_unstable_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let key_uris = keys
        .iter()
        .map(|key| key.id.as_str().to_owned())
        .collect::<Vec<_>>();

    for key in &keys {
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock(
                pg_catalog.hashtextextended($1, 0)
             )",
        )
        .bind(key.id.as_str())
        .execute(&mut **transaction)
        .await?;
        if let Some(owner) = sqlx::query_scalar::<_, i64>(
            "SELECT account_id FROM keypairs WHERE uri = $1 FOR UPDATE",
        )
        .bind(key.id.as_str())
        .fetch_optional(&mut **transaction)
        .await?
            && owner != account_id
        {
            return Err(WriteError::Conflict);
        }
    }

    if actor.key_set_complete {
        sqlx::query(
            "DELETE FROM keypairs
             WHERE account_id = $1 AND uri <> ALL($2)
               AND revoked = false
               AND (expires_at IS NULL OR expires_at > clock_timestamp())",
        )
        .bind(account_id)
        .bind(&key_uris)
        .execute(&mut **transaction)
        .await?;
    }

    for key in keys {
        sqlx::query(
            "INSERT INTO keypairs (
                account_id, type, uri, public_key, private_key, revoked, expires_at,
                created_at, updated_at
             ) VALUES ($1, 0, $2, $3, NULL, false, NULL, clock_timestamp(), clock_timestamp())
             ON CONFLICT (uri) DO UPDATE SET
                account_id = EXCLUDED.account_id,
                type = EXCLUDED.type,
                public_key = EXCLUDED.public_key,
                updated_at = clock_timestamp()",
        )
        .bind(account_id)
        .bind(key.id.as_str())
        .bind(&key.pem)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

fn remote_actor_text(object: &Value, field: &str) -> Result<Option<String>, WriteError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(WriteError::InvalidInput(
            "remote actor text field is invalid",
        )),
    }
}

fn remote_actor_bool(object: &Value, field: &str) -> Result<Option<bool>, WriteError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(WriteError::InvalidInput(
            "remote actor boolean field is invalid",
        )),
    }
}

fn remote_actor_object_uri(object: &Value, field: &str) -> Result<Option<String>, WriteError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    let value = match value {
        Value::String(value) => Some(value.clone()),
        Value::Object(object) => object
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        Value::Null => None,
        _ => {
            return Err(WriteError::InvalidInput(
                "remote actor URI field is invalid",
            ));
        }
    };
    let Some(value) = value else {
        return Ok(None);
    };
    let url = Url::parse(&value)
        .map_err(|_| WriteError::InvalidInput("remote actor URI field is invalid"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(WriteError::InvalidInput(
            "remote actor URI field is invalid",
        ));
    }
    Ok(Some(value))
}

fn remote_actor_fields(object: &Value) -> Result<Option<Value>, WriteError> {
    let Some(attachments) = object.get("attachment") else {
        return Ok(None);
    };
    let Value::Array(attachments) = attachments else {
        return Err(WriteError::InvalidInput("remote actor fields are invalid"));
    };
    let mut fields = Vec::new();
    for attachment in attachments.iter().take(20) {
        let Value::Object(attachment) = attachment else {
            continue;
        };
        if attachment.get("type").and_then(Value::as_str) != Some("PropertyValue") {
            continue;
        }
        let (Some(name), Some(value)) = (
            attachment.get("name").and_then(Value::as_str),
            attachment.get("value").and_then(Value::as_str),
        ) else {
            continue;
        };
        if name.chars().count() > 255 || value.chars().count() > 2048 {
            return Err(WriteError::InvalidInput("remote actor field is too long"));
        }
        fields.push(json!({"name": name, "value": value}));
    }
    Ok(Some(Value::Array(fields)))
}

fn remote_actor_aliases(object: &Value) -> Result<Option<Vec<String>>, WriteError> {
    let Some(aliases) = object.get("alsoKnownAs") else {
        return Ok(None);
    };
    let Value::Array(aliases) = aliases else {
        return Err(WriteError::InvalidInput("remote actor aliases are invalid"));
    };
    let mut values = Vec::new();
    for alias in aliases.iter().take(20) {
        let Some(alias) = alias.as_str() else {
            return Err(WriteError::InvalidInput("remote actor alias is invalid"));
        };
        let url = Url::parse(alias)
            .map_err(|_| WriteError::InvalidInput("remote actor alias is invalid"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(WriteError::InvalidInput("remote actor alias is invalid"));
        }
        values.push(alias.to_owned());
    }
    Ok(Some(values))
}

fn remote_actor_account_id(
    uri_account_id: Option<i64>,
    handle_account: Option<&(i64, Option<String>)>,
    inserted_account_id: Option<i64>,
    concurrent_handle_account: Option<(i64, Option<String>)>,
    uri: &str,
) -> Result<i64, WriteError> {
    if let (Some(uri_account_id), Some((handle_account_id, _))) = (uri_account_id, handle_account)
        && uri_account_id != *handle_account_id
    {
        return Err(WriteError::Conflict);
    }
    if let Some(uri_account_id) = uri_account_id {
        return Ok(uri_account_id);
    }
    if handle_account.is_some() {
        return Err(WriteError::Conflict);
    }
    if let Some(inserted_account_id) = inserted_account_id {
        return Ok(inserted_account_id);
    }
    concurrent_handle_account
        .filter(|(_, existing_uri)| existing_uri.as_deref() == Some(uri))
        .map(|(account_id, _)| account_id)
        .ok_or(WriteError::Conflict)
}

async fn record_quote_authorization_delete(
    transaction: &mut Transaction<'_, Postgres>,
    quote_id: i64,
    quoting_status_id: i64,
    quoted_status_id: i64,
    quoted_account_id: i64,
    origin: &str,
) -> Result<(), WriteError> {
    let actor_uri = local_actor_uri_for_account(transaction, quoted_account_id, origin).await?;
    let authorization_uri = format!(
        "{}/quote_authorizations/{quote_id}",
        actor_uri.trim_end_matches('/')
    );
    let body = activitypub::delete_quote_authorization_with_uris(&actor_uri, &authorization_uri);
    let destinations = sqlx::query_as::<_, (String, String)>(
        "WITH reached(account_id) AS ( \
           SELECT quoting.account_id FROM statuses quoting WHERE quoting.id = $1 \
           UNION SELECT follow.account_id FROM follows follow \
            WHERE follow.target_account_id IN ( \
              SELECT account_id FROM statuses WHERE id IN ($1, $2)) \
           UNION SELECT mention.account_id FROM mentions mention \
            WHERE mention.status_id IN ($1, $2) \
         ) SELECT DISTINCT COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url), \
                  account.domain \
             FROM reached JOIN accounts account ON account.id = reached.account_id \
            WHERE account.domain IS NOT NULL AND account.protocol = 1 \
              AND account.suspended_at IS NULL \
              AND COALESCE(NULLIF(account.shared_inbox_url, ''), account.inbox_url) <> '' \
            ORDER BY 1, 2",
    )
    .bind(quoting_status_id)
    .bind(quoted_status_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (inbox_url, remote_domain) in destinations {
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "source_account_id": quoted_account_id,
                "inbox_url": inbox_url,
                "remote_domain": remote_domain,
                "body": body.clone()
            }),
        )
        .logical_key(format!(
            "activitypub:quote-authorization-delete:{quote_id}:{inbox_url}"
        ));
        record_outbox_once_in(transaction, &delivery).await?;
    }
    Ok(())
}

async fn cancel_quote_request_outbox(
    transaction: &mut Transaction<'_, Postgres>,
    quote_id: i64,
    request_uri: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM rustodon.outbox_events WHERE dispatched_at IS NULL AND ( \
           (kind = $1 AND logical_key = $2) OR \
           (kind = $3 AND $4::text IS NOT NULL AND ( \
             (payload -> 'arguments' ->> 'quote_request_uri' = $4 \
               AND payload -> 'arguments' ->> 'quote_id' = $5::text) \
             OR payload #>> '{arguments,body,id}' = $4 \
             OR payload #>> '{arguments,body,object,id}' = $4)))",
    )
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(format!("activitypub:quote-request:{quote_id}"))
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(request_uri)
    .bind(quote_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.durable_jobs WHERE dead_at IS NULL \
           AND (lease_owner IS NULL OR lease_expires_at <= clock_timestamp()) AND ( \
           (kind = $1 AND logical_key = $2) OR \
           (kind = $3 AND $4::text IS NOT NULL AND ( \
             (arguments ->> 'quote_request_uri' = $4 \
               AND arguments ->> 'quote_id' = $5::text) \
             OR arguments #>> '{body,id}' = $4 \
             OR arguments #>> '{body,object,id}' = $4)))",
    )
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(format!("activitypub:quote-request:{quote_id}"))
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(request_uri)
    .bind(quote_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn cancel_quote_decision_outbox_for_target(
    transaction: &mut Transaction<'_, Postgres>,
    quoted_status_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
          WHERE dispatched_at IS NULL AND kind = $1 \
            AND payload -> 'arguments' ->> 'quote_delivery_kind' IN ('accept', 'reject') \
            AND payload -> 'arguments' ->> 'quoted_status_id' = $2::text",
    )
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(quoted_status_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.durable_jobs \
          WHERE dead_at IS NULL \
            AND (lease_owner IS NULL OR lease_expires_at <= clock_timestamp()) \
            AND kind = $1 \
            AND arguments ->> 'quote_delivery_kind' IN ('accept', 'reject') \
            AND arguments ->> 'quoted_status_id' = $2::text",
    )
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(quoted_status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn cancel_status_outbox(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
         WHERE kind = $1 AND logical_key = $2 AND dispatched_at IS NULL",
    )
    .bind(STATUS_NOTIFICATION_JOB_KIND)
    .bind(format!("status-notifications:{status_id}"))
    .execute(&mut **transaction)
    .await?;
    let distribution_key = format!("activitypub:status:{status_id}");
    let distribution_key_prefix = format!("{distribution_key}:%");
    let delivery_key_prefix = format!("{distribution_key}:%");
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
          WHERE dispatched_at IS NULL AND ( \
           (kind = $1 AND (logical_key = $2 OR logical_key LIKE $3)) OR \
           (kind = $4 AND logical_key LIKE $5))",
    )
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(&distribution_key)
    .bind(&distribution_key_prefix)
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(&delivery_key_prefix)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn record_status_delete_distribution(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    recipient_account_ids: &[i64],
) -> Result<(), WriteError> {
    cancel_status_outbox(transaction, status_id).await?;
    let status_delete_job = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
        json!({
            "status_id": status_id,
            "activity_type": "Delete",
            "recipient_account_ids": recipient_account_ids
        }),
    )
    .logical_key(format!("activitypub:status:{status_id}:delete"));
    record_outbox_in(transaction, &status_delete_job).await?;
    Ok(())
}

async fn update_conversation_unread_in(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    account_conversation_id: i64,
    unread: bool,
    status_ids: Vec<i64>,
    expected_lock_version: i32,
) -> Result<(), WriteError> {
    let Some(last_status_id) = status_ids.iter().max().copied() else {
        return Err(WriteError::Validation("Last status must exist"));
    };
    let last_status_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM statuses WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(last_status_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !last_status_exists {
        return Err(WriteError::Validation("Last status must exist"));
    }
    let updated_lock_version = sqlx::query_scalar::<_, i32>(
        "UPDATE account_conversations SET unread = $1, last_status_id = $2, \
         lock_version = lock_version + 1 \
         WHERE account_id = $3 AND id = $4 AND lock_version = $5 \
         RETURNING lock_version",
    )
    .bind(unread)
    .bind(last_status_id)
    .bind(account_id)
    .bind(account_conversation_id)
    .bind(expected_lock_version)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(updated_lock_version) = updated_lock_version else {
        return Err(WriteError::Conflict);
    };
    record_conversation_stream_event_in(
        transaction,
        account_id,
        account_conversation_id,
        updated_lock_version,
    )
    .await?;
    Ok(())
}

struct ResolvedNotificationActivity {
    activity_id: i64,
    activity_type: &'static str,
    notification_type: &'static str,
    from_account_id: i64,
    target_account_id: Option<i64>,
    target_status_id: Option<i64>,
    created_at: NaiveDateTime,
    group: Option<NotificationGroup>,
}

#[derive(Clone, Copy)]
enum NotificationGroup {
    Status {
        prefix: &'static str,
        status_id: i64,
    },
    Account {
        prefix: &'static str,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NotificationPolicyDecision {
    Accept,
    Filter,
    Drop,
}

#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)]
struct NotificationPolicyFacts {
    filterable: bool,
    permission: bool,
    staff_bypass: bool,
    not_following: bool,
    not_follower: bool,
    new_account: bool,
    limited: bool,
    bot: bool,
    private_mention: bool,
    for_not_following: i32,
    for_not_followers: i32,
    for_new_accounts: i32,
    for_limited_accounts: i32,
    for_bots: i32,
    for_private_mentions: i32,
}

fn notification_policy_decision(facts: &NotificationPolicyFacts) -> NotificationPolicyDecision {
    if !facts.filterable || facts.permission {
        return NotificationPolicyDecision::Accept;
    }
    let actions = [
        (facts.not_following, facts.for_not_following),
        (facts.not_follower, facts.for_not_followers),
        (
            facts.new_account && facts.not_following,
            facts.for_new_accounts,
        ),
        (
            facts.limited && facts.not_following,
            facts.for_limited_accounts,
        ),
        (facts.bot && facts.not_following, facts.for_bots),
        (
            facts.private_mention && facts.not_following,
            facts.for_private_mentions,
        ),
    ];
    if actions
        .iter()
        .any(|(condition, action)| *condition && *action == 2)
    {
        NotificationPolicyDecision::Drop
    } else if actions
        .iter()
        .any(|(condition, action)| *condition && *action == 1)
    {
        NotificationPolicyDecision::Filter
    } else {
        NotificationPolicyDecision::Accept
    }
}

fn notification_policy_decision_for_type(
    facts: &NotificationPolicyFacts,
    notification_type: &str,
) -> NotificationPolicyDecision {
    if notification_type == "mention" && facts.staff_bypass {
        NotificationPolicyDecision::Accept
    } else {
        notification_policy_decision(facts)
    }
}

#[allow(clippy::too_many_lines)]
async fn notification_policy_facts(
    transaction: &mut Transaction<'_, Postgres>,
    recipient_account_id: i64,
    sender_account_id: i64,
    target_status_id: Option<i64>,
    notification_type: &str,
    silenced: bool,
) -> Result<NotificationPolicyFacts, WriteError> {
    let row = sqlx::query_as::<
        _,
        (
             i32,
            i32,
            i32,
            i32,
            i32,
            i32,
             bool,
             bool,
            bool,
            bool,
            bool,
            bool,
            bool,
            bool,
        ),
    >(
        "SELECT COALESCE(policy.for_not_following, 0), \
                COALESCE(policy.for_not_followers, 0), \
                COALESCE(policy.for_new_accounts, 0), \
                COALESCE(policy.for_limited_accounts, 1), \
                COALESCE(policy.for_bots, 0), \
                COALESCE(policy.for_private_mentions, 1), \
                EXISTS (SELECT 1 FROM notification_permissions permission \
                        WHERE permission.account_id = $1 AND permission.from_account_id = $2), \
                NOT EXISTS (SELECT 1 FROM follows follow \
                            WHERE follow.account_id = $1 AND follow.target_account_id = $2), \
                NOT EXISTS (SELECT 1 FROM follows follow \
                            WHERE follow.account_id = $2 AND follow.target_account_id = $1 \
                              AND follow.created_at <= clock_timestamp() - interval '3 days'), \
                sender.created_at > clock_timestamp() - interval '30 days', \
                 sender.silenced_at IS NOT NULL OR $5, \
                 sender.actor_type IN ('Application', 'Service'), \
                 EXISTS ( \
                   SELECT 1 FROM users sender_user \
                   JOIN user_roles sender_role ON sender_role.id = sender_user.role_id \
                   LEFT JOIN user_roles everyone_role ON everyone_role.id = -99 \
                   LEFT JOIN users recipient_user ON recipient_user.account_id = $1 \
                   LEFT JOIN user_roles recipient_role ON recipient_role.id = recipient_user.role_id \
                   WHERE sender_user.account_id = $2 AND sender.domain IS NULL \
                     AND sender_role.highlighted \
                      AND (sender_role.permissions & 1 <> 0 OR \
                           ((sender_role.permissions | COALESCE(everyone_role.permissions, 0)) & $6 <> 0)) \
                     AND (recipient_role.id IS NULL OR sender_role.position > recipient_role.position)), \
                 $4 = 'mention' AND $3::bigint IS NOT NULL AND EXISTS ( \
                   SELECT 1 FROM statuses target WHERE target.id = $3 AND target.visibility = 3 \
                     AND NOT EXISTS ( \
                       WITH RECURSIVE ancestors(id, in_reply_to_id, mention_id, path, depth) AS ( \
                         SELECT status.id, status.in_reply_to_id, mention.id, ARRAY[status.id], 0 \
                         FROM statuses status \
                         LEFT JOIN mentions mention ON mention.silent = false \
                           AND mention.account_id = $2 AND mention.status_id = status.id \
                         WHERE status.id = target.in_reply_to_id \
                         UNION ALL \
                         SELECT status.id, status.in_reply_to_id, mention.id, \
                           ancestors.path || status.id, ancestors.depth + 1 \
                         FROM ancestors \
                         JOIN statuses status ON status.id = ancestors.in_reply_to_id \
                         LEFT JOIN mentions mention ON mention.silent = false \
                           AND mention.account_id = $2 AND mention.status_id = status.id \
                           AND status.account_id = $1 \
                         WHERE ancestors.mention_id IS NULL \
                           AND NOT status.id = ANY(ancestors.path) AND ancestors.depth < 100) \
                       SELECT 1 FROM ancestors \
                       JOIN statuses ancestor_status ON ancestor_status.id = ancestors.id \
                       WHERE ancestors.mention_id IS NOT NULL \
                         AND ancestor_status.account_id = $1 AND ancestor_status.visibility = 3)) \
          FROM accounts sender \
         LEFT JOIN notification_policies policy ON policy.account_id = $1 \
         WHERE sender.id = $2",
    )
    .bind(recipient_account_id)
    .bind(sender_account_id)
    .bind(target_status_id)
    .bind(notification_type)
    .bind(silenced)
    .bind(MODERATION_PERMISSION_MASK)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(NotificationPolicyFacts {
        filterable: matches!(
            notification_type,
            "mention"
                | "reblog"
                | "follow"
                | "follow_request"
                | "favourite"
                | "quote"
                | "added_to_collection"
        ),
        permission: row.6,
        staff_bypass: row.12,
        not_following: row.7,
        not_follower: row.8,
        new_account: row.9,
        limited: row.10,
        bot: row.11,
        private_mention: row.13,
        for_not_following: row.0,
        for_not_followers: row.1,
        for_new_accounts: row.2,
        for_limited_accounts: row.3,
        for_bots: row.4,
        for_private_mentions: row.5,
    })
}

#[allow(clippy::too_many_lines)]
async fn resolve_notification_activity(
    transaction: &mut Transaction<'_, Postgres>,
    activity: NotificationActivity,
    recipient_account_id: i64,
) -> Result<Option<ResolvedNotificationActivity>, WriteError> {
    let resolved = match activity {
        NotificationActivity::Mention { id } => sqlx::query_as::<_, (i64, i64, i64, NaiveDateTime)>(
            "SELECT status.account_id, mention.status_id, mention.account_id, mention.created_at FROM mentions mention \
             JOIN statuses status ON status.id = mention.status_id \
             WHERE mention.id = $1 AND mention.silent = false AND status.deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(|(from_account_id, target_status_id, target_account_id, created_at)| {
            ResolvedNotificationActivity {
                activity_id: id,
                activity_type: "Mention",
                notification_type: "mention",
                from_account_id,
                target_account_id: Some(target_account_id),
                target_status_id: Some(target_status_id),
                created_at,
                group: None,
            }
        }),
        NotificationActivity::Status { id } => sqlx::query_as::<_, (i64, NaiveDateTime)>(
            "SELECT account_id, created_at FROM statuses WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(|(from_account_id, created_at)| ResolvedNotificationActivity {
            activity_id: id,
            activity_type: "Status",
            notification_type: "status",
            from_account_id,
            target_account_id: None,
            target_status_id: Some(id),
            created_at,
            group: None,
        }),
        NotificationActivity::Reblog { id } =>
            sqlx::query_as::<_, (i64, i64, i64, NaiveDateTime)>(
                "SELECT activity.account_id, target.account_id, activity.reblog_of_id, activity.created_at \
                 FROM statuses activity JOIN statuses target ON target.id = activity.reblog_of_id \
                 WHERE activity.id = $1 AND activity.deleted_at IS NULL AND target.deleted_at IS NULL",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(from_account_id, target_account_id, target_status_id, created_at)| {
                ResolvedNotificationActivity {
                    activity_id: id,
                    activity_type: "Status",
                    notification_type: "reblog",
                    from_account_id,
                    target_account_id: Some(target_account_id),
                    target_status_id: Some(target_status_id),
                    created_at,
                    group: Some(NotificationGroup::Status {
                        prefix: "reblog",
                        status_id: target_status_id,
                    }),
                }
            }),
        NotificationActivity::Follow { id } => {
            sqlx::query_as::<_, (i64, i64, NaiveDateTime)>(
                "SELECT account_id, target_account_id, created_at FROM follows WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(from_account_id, target_account_id, created_at)| {
                ResolvedNotificationActivity {
                    activity_id: id,
                    activity_type: "Follow",
                    notification_type: "follow",
                    from_account_id,
                    target_account_id: Some(target_account_id),
                    target_status_id: None,
                    created_at,
                    group: Some(NotificationGroup::Account { prefix: "follow" }),
                }
            })
        }
        NotificationActivity::FollowRequest { id } =>
            sqlx::query_as::<_, (i64, i64, NaiveDateTime)>(
                "SELECT account_id, target_account_id, created_at FROM follow_requests WHERE id = $1",
            )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(
            |(from_account_id, target_account_id, created_at)| ResolvedNotificationActivity {
                activity_id: id,
                activity_type: "FollowRequest",
                notification_type: "follow_request",
                from_account_id,
                target_account_id: Some(target_account_id),
                target_status_id: None,
                created_at,
                group: None,
            },
        ),
        NotificationActivity::Favourite { id } => {
            sqlx::query_as::<_, (i64, i64, i64, NaiveDateTime)>(
                "SELECT favourite.account_id, status.account_id, favourite.status_id, favourite.created_at \
                 FROM favourites favourite JOIN statuses status ON status.id = favourite.status_id \
                 WHERE favourite.id = $1 AND status.deleted_at IS NULL",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(from_account_id, target_account_id, target_status_id, created_at)| {
                ResolvedNotificationActivity {
                    activity_id: id,
                    activity_type: "Favourite",
                    notification_type: "favourite",
                    from_account_id,
                    target_account_id: Some(target_account_id),
                    target_status_id: Some(target_status_id),
                    created_at,
                    group: Some(NotificationGroup::Status {
                        prefix: "favourite",
                        status_id: target_status_id,
                    }),
                }
            })
        }
        NotificationActivity::Poll { id } => sqlx::query_as::<_, (i64, i64, NaiveDateTime)>(
            "SELECT poll.account_id, poll.status_id, poll.created_at FROM polls poll \
             JOIN statuses status ON status.id = poll.status_id \
             WHERE poll.id = $1 AND status.deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(
            |(from_account_id, target_status_id, created_at)| ResolvedNotificationActivity {
                activity_id: id,
                activity_type: "Poll",
                notification_type: "poll",
                from_account_id,
                target_account_id: None,
                target_status_id: Some(target_status_id),
                created_at,
                group: None,
            },
        ),
        NotificationActivity::Update { id } => {
            resolve_status_activity(transaction, id, "update").await?
        }
        NotificationActivity::QuotedUpdate { id } => {
            resolve_quoted_update_activity(transaction, id).await?
        }
        NotificationActivity::Quote { id } => {
            sqlx::query_as::<_, (i64, i64, i64, NaiveDateTime)>(
                "SELECT quote.account_id, quote.quoted_account_id, quote.status_id, quote.created_at \
                 FROM quotes quote JOIN statuses status ON status.id = quote.status_id \
                 WHERE quote.id = $1 AND quote.quoted_account_id IS NOT NULL \
                   AND quote.state = 1 AND status.deleted_at IS NULL",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(from_account_id, target_account_id, target_status_id, created_at)| {
                ResolvedNotificationActivity {
                    activity_id: id,
                    activity_type: "Quote",
                    notification_type: "quote",
                    from_account_id,
                    target_account_id: Some(target_account_id),
                    target_status_id: Some(target_status_id),
                    created_at,
                    group: None,
                }
            })
        }
        NotificationActivity::SeveredRelationships { id } => {
            sqlx::query_as::<_, (i64, NaiveDateTime)>(
                "SELECT account_id, created_at FROM account_relationship_severance_events WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(account_id, created_at)| ResolvedNotificationActivity {
                activity_id: id,
                activity_type: "AccountRelationshipSeveranceEvent",
                notification_type: "severed_relationships",
                from_account_id: account_id,
                target_account_id: Some(account_id),
                target_status_id: None,
                created_at,
                group: None,
            })
        }
        NotificationActivity::ModerationWarning { id } => {
            sqlx::query_as::<_, (i64, NaiveDateTime)>(
                "SELECT target_account_id, created_at FROM account_warnings \
                 WHERE id = $1 AND target_account_id IS NOT NULL",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(account_id, created_at)| ResolvedNotificationActivity {
                activity_id: id,
                activity_type: "AccountWarning",
                notification_type: "moderation_warning",
                from_account_id: account_id,
                target_account_id: Some(account_id),
                target_status_id: None,
                created_at,
                group: None,
            })
        }
        NotificationActivity::AnnualReport { id } => {
            sqlx::query_as::<_, (i64, NaiveDateTime)>(
                "SELECT account_id, created_at FROM generated_annual_reports WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(account_id, created_at)| ResolvedNotificationActivity {
                activity_id: id,
                activity_type: "GeneratedAnnualReport",
                notification_type: "annual_report",
                from_account_id: account_id,
                target_account_id: Some(account_id),
                target_status_id: None,
                created_at,
                group: None,
            })
        }
        NotificationActivity::AdminSignUp { id } => {
            sqlx::query_as::<_, (i64, NaiveDateTime)>(
                "SELECT id, created_at FROM accounts WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(account_id, created_at)| ResolvedNotificationActivity {
                activity_id: id,
                activity_type: "Account",
                notification_type: "admin.sign_up",
                from_account_id: account_id,
                target_account_id: None,
                target_status_id: None,
                created_at,
                group: Some(NotificationGroup::Account {
                    prefix: "admin.sign_up",
                }),
            })
        }
        NotificationActivity::AdminReport { id } => {
            sqlx::query_as::<_, (i64, NaiveDateTime)>(
                "SELECT account_id, created_at FROM reports WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(from_account_id, created_at)| ResolvedNotificationActivity {
                activity_id: id,
                activity_type: "Report",
                notification_type: "admin.report",
                from_account_id,
                target_account_id: None,
                target_status_id: None,
                created_at,
                group: None,
            })
        }
        NotificationActivity::AddedToCollection { id } => {
            sqlx::query_as::<_, (i64, i64, NaiveDateTime)>(
                "SELECT collection.account_id, item.account_id, item.created_at \
                 FROM collection_items item JOIN collections collection ON collection.id = item.collection_id \
                 WHERE item.id = $1 AND item.account_id IS NOT NULL",
            )
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(from_account_id, target_account_id, created_at)| {
                ResolvedNotificationActivity {
                    activity_id: id,
                    activity_type: "CollectionItem",
                    notification_type: "added_to_collection",
                    from_account_id,
                    target_account_id: Some(target_account_id),
                    target_status_id: None,
                    created_at,
                    group: None,
                }
            })
        }
        NotificationActivity::CollectionUpdate { id } => {
            sqlx::query_as::<_, (i64, i64, NaiveDateTime)>(
                "SELECT collection.account_id, item.account_id, collection.updated_at \
                 FROM collections collection JOIN collection_items item ON item.collection_id = collection.id \
                 WHERE collection.id = $1 AND item.account_id = $2 AND item.state = 1 \
                 ORDER BY item.id LIMIT 1",
            )
            .bind(id)
            .bind(recipient_account_id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(|(from_account_id, target_account_id, created_at)| {
                ResolvedNotificationActivity {
                    activity_id: id,
                    activity_type: "Collection",
                    notification_type: "collection_update",
                    from_account_id,
                    target_account_id: Some(target_account_id),
                    target_status_id: None,
                    created_at,
                    group: None,
                }
            })
        }
    };
    Ok(resolved)
}

async fn resolve_status_activity(
    transaction: &mut Transaction<'_, Postgres>,
    id: i64,
    notification_type: &'static str,
) -> Result<Option<ResolvedNotificationActivity>, WriteError> {
    Ok(sqlx::query_as::<_, (i64, NaiveDateTime)>(
        "SELECT account_id, created_at FROM statuses WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?
    .map(
        |(from_account_id, created_at)| ResolvedNotificationActivity {
            activity_id: id,
            activity_type: "Status",
            notification_type,
            from_account_id,
            target_account_id: None,
            target_status_id: Some(id),
            created_at,
            group: None,
        },
    ))
}

async fn resolve_quoted_update_activity(
    transaction: &mut Transaction<'_, Postgres>,
    id: i64,
) -> Result<Option<ResolvedNotificationActivity>, WriteError> {
    Ok(sqlx::query_as::<_, (i64, i64, NaiveDateTime)>(
        "SELECT quote.quoted_account_id, quote.account_id, status.created_at \
         FROM statuses status JOIN quotes quote ON quote.status_id = status.id \
         WHERE status.id = $1 AND status.deleted_at IS NULL \
           AND quote.quoted_account_id IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?
    .map(
        |(from_account_id, target_account_id, created_at)| ResolvedNotificationActivity {
            activity_id: id,
            activity_type: "Status",
            notification_type: "quoted_update",
            from_account_id,
            target_account_id: Some(target_account_id),
            target_status_id: Some(id),
            created_at,
            group: None,
        },
    ))
}

async fn notification_group_key(
    transaction: &mut Transaction<'_, Postgres>,
    recipient_account_id: i64,
    activity: &ResolvedNotificationActivity,
    filtered: bool,
) -> Result<Option<String>, WriteError> {
    if filtered {
        return Ok(None);
    }
    let Some(group) = activity.group else {
        return Ok(None);
    };
    let (prefix, group_type) = match group {
        NotificationGroup::Status { prefix, status_id } => {
            (format!("{prefix}-{status_id}"), activity.notification_type)
        }
        NotificationGroup::Account { prefix } => (prefix.to_owned(), activity.notification_type),
    };
    let current_bucket = activity
        .created_at
        .and_utc()
        .timestamp()
        .div_euclid(60 * 60);
    let key_hash = notification_group_marker_key(recipient_account_id, group_type, &prefix);
    let previous_group_key = sqlx::query_scalar::<_, String>(
        "SELECT payload ->> 'group_key' FROM rustodon.ordering_markers \
          WHERE kind = $1 AND key_hash = $2 AND expires_at > clock_timestamp() \
          FOR UPDATE",
    )
    .bind(NOTIFICATION_GROUP_MARKER_KIND)
    .bind(key_hash.as_slice())
    .fetch_optional(&mut **transaction)
    .await?
    .and_then(|group_key| {
        group_key
            .rsplit('-')
            .next()
            .and_then(|bucket| bucket.parse::<i64>().ok())
    })
    .filter(|bucket| current_bucket < bucket.saturating_add(12));
    let bucket = previous_group_key.unwrap_or(current_bucket);
    let group_key = format!("{prefix}-{bucket}");
    sqlx::query(
        "INSERT INTO rustodon.ordering_markers \
           (kind, key_hash, ordering_at, payload, created_at, expires_at) \
         VALUES ($1, $2, $3, jsonb_build_object('group_key', $4), \
                 clock_timestamp(), clock_timestamp() + interval '12 hours') \
         ON CONFLICT (kind, key_hash) DO UPDATE SET \
           ordering_at = EXCLUDED.ordering_at, payload = EXCLUDED.payload, \
           expires_at = EXCLUDED.expires_at",
    )
    .bind(NOTIFICATION_GROUP_MARKER_KIND)
    .bind(key_hash.as_slice())
    .bind(activity.created_at.and_utc())
    .bind(&group_key)
    .execute(&mut **transaction)
    .await?;
    Ok(Some(group_key))
}

fn notification_group_marker_key(
    recipient_account_id: i64,
    group_type: &str,
    prefix: &str,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(NOTIFICATION_GROUP_MARKER_KIND.as_bytes());
    digest.update([0]);
    digest.update(recipient_account_id.to_be_bytes());
    digest.update([0]);
    digest.update(group_type.as_bytes());
    digest.update([0]);
    digest.update(prefix.as_bytes());
    digest.finalize().into()
}

async fn upsert_notification_request(
    transaction: &mut Transaction<'_, Postgres>,
    activity: &ResolvedNotificationActivity,
    recipient_account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO notification_requests ( \
           account_id, from_account_id, last_status_id, notifications_count, created_at, updated_at) \
         VALUES ($1, $2, $3, ( \
           SELECT count(*) FROM ( \
             SELECT 1 FROM notifications notification \
             WHERE notification.account_id = $1 AND notification.from_account_id = $2 \
               AND notification.filtered = true \
               AND notification.type IN ('mention', 'quote') LIMIT 100 \
           ) meaningful), clock_timestamp(), clock_timestamp()) \
         ON CONFLICT (account_id, from_account_id) DO UPDATE SET \
           last_status_id = EXCLUDED.last_status_id, \
           notifications_count = EXCLUDED.notifications_count, \
           updated_at = clock_timestamp()",
    )
    .bind(recipient_account_id)
    .bind(activity.from_account_id)
    .bind(activity.target_status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn write_account(
    authenticated: &AuthenticatedBearer,
    scopes: super::oauth::RequiredScopes,
) -> Result<i64, WriteError> {
    if !authenticated.scopes().permits(scopes) {
        return Err(WriteError::Unauthorized);
    }
    authenticated
        .require_user()
        .map(super::oauth::OAuthResourceOwner::account_id)
        .map_err(|_| WriteError::Unauthorized)
}

async fn record_login_activity(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: i64,
    authentication_method: &str,
    failure_reason: Option<&str>,
    success: bool,
    ip: IpNetwork,
    user_agent: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO login_activities ( \
            authentication_method, created_at, failure_reason, ip, provider, success, user_agent, user_id) \
         VALUES ($1, clock_timestamp(), $2, $3, NULL, $4, $5, $6)",
    )
    .bind(authentication_method)
    .bind(failure_reason)
    .bind(ip)
    .bind(success)
    .bind(user_agent)
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

// The inbound 4.6.5 edit contract concerns rendered text, CW, and ordered
// media identity/descriptions. Sensitivity, language, tags, counts, and media
// cache metadata still reconcile, but are not standalone edit signals. Poll
// editing is not supported here; do not infer it from ignored Question fields.
#[derive(Eq, PartialEq)]
struct RemoteNoteEditProjection {
    content: RenderedHtml,
    spoiler_text: String,
    media: Vec<(i64, Option<String>)>,
}

async fn remote_note_edit_projection(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    formatter: &HtmlFormatter<'_>,
) -> Result<RemoteNoteEditProjection, WriteError> {
    let (text, spoiler_text) = sqlx::query_as::<_, (String, String)>(
        "SELECT text, spoiler_text FROM statuses WHERE id = $1",
    )
    .bind(status_id)
    .fetch_one(&mut **transaction)
    .await?;
    // Match the visible-media projection: explicit ordering when present,
    // otherwise attachment ID order. Cache installation is not an edit.
    let media = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT media.id, media.description FROM statuses status
         CROSS JOIN LATERAL unnest(COALESCE(status.ordered_media_attachment_ids,
             ARRAY(SELECT fallback.id FROM media_attachments fallback
                   WHERE fallback.status_id = status.id ORDER BY fallback.id)))
             WITH ORDINALITY ordering(media_id, position)
         JOIN media_attachments media ON media.id = ordering.media_id
             AND media.status_id = status.id
         WHERE status.id = $1 ORDER BY ordering.position LIMIT 4",
    )
    .bind(status_id)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(RemoteNoteEditProjection {
        content: formatter.remote_fragment(&text),
        spoiler_text,
        media,
    })
}

struct RemoteNoteData {
    uri: String,
    atom_uri: Option<String>,
    url: Option<String>,
    content: String,
    summary: String,
    language: Option<String>,
    sensitive: bool,
    quote_approval_policy: i32,
    published_at: NaiveDateTime,
    updated_at: NaiveDateTime,
    edited_at: Option<NaiveDateTime>,
    in_reply_to_uri: Option<String>,
    conversation_uri: Option<String>,
    audience: RemoteNoteAudience,
    mentions: Vec<String>,
    hashtags: Vec<String>,
    attachments: Vec<RemoteNoteAttachment>,
    favourites_count: Option<i64>,
    reblogs_count: Option<i64>,
    poll: Option<RemotePollData>,
    quote: Option<RemoteQuoteData>,
}

struct RemoteQuoteImportGuard<'a> {
    request_uri: &'a str,
    quoted_status_uri: &'a str,
    instrument_uri: &'a str,
    expected_target_status_id: i64,
    expected_target_account_id: i64,
}

struct RemoteQuoteData {
    target_uri: Option<String>,
    authorization_uri: Option<String>,
    legacy: bool,
    deleted: bool,
}

struct RemoteQuoteAuthorizationData {
    uri: String,
    attributed_to: Option<String>,
    interacting_object: Option<String>,
    interaction_target: Option<String>,
    typed: bool,
}

struct RemotePollData {
    options: Vec<String>,
    tallies: Vec<i64>,
    multiple: bool,
    expires_at: Option<NaiveDateTime>,
    voters_count: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemoteUpdateAuthority {
    Inbox,
    SignedRefresh,
}

impl RemoteUpdateAuthority {
    const fn rejects_tally_regression(self) -> bool {
        matches!(self, Self::Inbox)
    }

    const fn claims_freshness(self) -> bool {
        matches!(self, Self::SignedRefresh)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemotePollReconcile {
    Unchanged,
    Tally(NaiveDateTime),
    Significant,
}

struct RemoteNoteAudience {
    to: Vec<String>,
    cc: Vec<String>,
}

struct RemoteNoteAttachment {
    remote_url: String,
    thumbnail_remote_url: Option<String>,
    content_type: Option<String>,
    description: Option<String>,
    blurhash: Option<String>,
    file_meta: Value,
}

impl RemoteNoteData {
    fn parse(object: &Value, actor_uri: &str) -> Result<Self, WriteError> {
        let object = object
            .as_object()
            .ok_or(WriteError::InvalidInput("remote Note object is invalid"))?;
        let is_note = equals_or_includes(object.get("type"), "Note");
        let is_question = equals_or_includes(object.get("type"), "Question");
        if !is_note && !is_question {
            return Err(WriteError::InvalidInput(
                "remote object is not a Note or Question",
            ));
        }
        let uri = remote_note_uri(object.get("id"))?
            .ok_or(WriteError::InvalidInput("remote Note object has no ID"))?;
        let attributed_to = remote_note_attributed_to(object.get("attributedTo"))?
            .ok_or(WriteError::InvalidInput("remote Note has no author"))?;
        if attributed_to != actor_uri {
            return Err(WriteError::InvalidInput(
                "remote Note author does not match its signer",
            ));
        }
        let content = object
            .get("content")
            .and_then(Value::as_str)
            .or_else(|| {
                object
                    .get("contentMap")
                    .and_then(Value::as_object)
                    .and_then(|values| values.values().find_map(Value::as_str))
            })
            .filter(|value| value.chars().count() <= 20 * 1024)
            .ok_or(WriteError::InvalidInput("remote Note content is invalid"))?
            .to_owned();
        let content_map = object.get("contentMap").and_then(Value::as_object);
        let language = object
            .get("language")
            .and_then(Value::as_str)
            .or_else(|| content_map.and_then(|values| values.keys().next().map(String::as_str)))
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned);
        let published_at = remote_note_timestamp(object, "published", Utc::now().naive_utc())?;
        let edited_at = object
            .get("updated")
            .map(|_| remote_note_timestamp(object, "updated", published_at))
            .transpose()?;
        let updated_at = edited_at.unwrap_or(published_at);
        let audience = RemoteNoteAudience {
            to: remote_note_uri_array(object.get("to"))?,
            cc: remote_note_uri_array(object.get("cc"))?,
        };
        let (mentions, hashtags) = remote_note_tags(object.get("tag"))?;
        let atom_uri = super::activitypub_inbox::optional_atom_uri(object.get("atomUri"))
            .map_err(|_| WriteError::InvalidInput("remote Note atom URI is invalid"))?;
        if let Some(atom_uri) = atom_uri.as_deref()
            && !same_remote_note_host(actor_uri, atom_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote Note atom URI does not match its actor host",
            ));
        }
        Ok(Self {
            uri,
            atom_uri,
            url: remote_note_optional_uri(object.get("url"))?,
            content,
            summary: object
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .chars()
                .take(20 * 1024)
                .collect(),
            language,
            sensitive: object
                .get("sensitive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            quote_approval_policy: remote_quote_approval_policy(object, actor_uri)?,
            published_at,
            updated_at,
            edited_at,
            in_reply_to_uri: remote_note_optional_uri(object.get("inReplyTo"))?,
            conversation_uri: remote_note_optional_conversation_uri(object.get("conversation"))?,
            audience,
            mentions,
            hashtags,
            attachments: remote_note_attachments(object.get("attachment")),
            favourites_count: remote_note_interaction_count(object, "likes", "favouritesCount")?,
            reblogs_count: remote_note_interaction_count(object, "shares", "reblogsCount")?,
            poll: is_question.then(|| remote_poll_data(object)).transpose()?,
            quote: remote_quote_data(object)?,
        })
    }
}

fn remote_quote_approval_policy(
    object: &serde_json::Map<String, Value>,
    actor_uri: &str,
) -> Result<i32, WriteError> {
    let actor_uri = actor_uri.trim_end_matches('/');
    remote_quote_approval_policy_with_collections(
        object,
        actor_uri,
        &format!("{actor_uri}/followers"),
        &format!("{actor_uri}/following"),
    )
}

fn remote_quote_approval_policy_with_collections(
    object: &serde_json::Map<String, Value>,
    actor_uri: &str,
    followers_uri: &str,
    following_uri: &str,
) -> Result<i32, WriteError> {
    let Some(policy) = object
        .get("interactionPolicy")
        .and_then(Value::as_object)
        .and_then(|policy| policy.get("canQuote"))
        .and_then(Value::as_object)
    else {
        return Ok(0);
    };
    let automatic = remote_quote_subpolicy(
        policy.get("automaticApproval"),
        actor_uri,
        followers_uri,
        following_uri,
    )?;
    let manual = remote_quote_subpolicy(
        policy.get("manualApproval"),
        actor_uri,
        followers_uri,
        following_uri,
    )?;
    Ok((automatic << 16) | manual)
}

fn remote_quote_subpolicy(
    value: Option<&Value>,
    actor_uri: &str,
    followers_uri: &str,
    following_uri: &str,
) -> Result<i32, WriteError> {
    let values = match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) if values.len() <= 100 => values.iter().collect(),
        Some(Value::Array(_)) => {
            return Err(WriteError::InvalidInput(
                "remote quote interaction policy is too large",
            ));
        }
        Some(value) => vec![value],
    };
    let actor_uri = actor_uri.trim_end_matches('/');
    Ok(values.into_iter().fold(0, |flags, value| {
        let uri = value
            .as_str()
            .or_else(|| value.as_object()?.get("id")?.as_str());
        flags
            | match uri {
                Some("as:Public" | "Public" | "https://www.w3.org/ns/activitystreams#Public") => 2,
                Some(uri) if uri == followers_uri => 4,
                Some(uri) if uri == following_uri => 8,
                Some(uri) if uri == actor_uri => 0,
                _ => 1,
            }
    }))
}

fn remote_quote_authorization_data(
    value: &Value,
) -> Result<RemoteQuoteAuthorizationData, WriteError> {
    let uri = remote_note_uri(Some(value))?.ok_or(WriteError::InvalidInput(
        "remote quote authorization has no ID",
    ))?;
    let embedded = value.as_object();
    Ok(RemoteQuoteAuthorizationData {
        uri,
        attributed_to: embedded
            .map(|value| remote_note_optional_uri(value.get("attributedTo")))
            .transpose()?
            .flatten(),
        interacting_object: embedded
            .map(|value| remote_note_optional_uri(value.get("interactingObject")))
            .transpose()?
            .flatten(),
        interaction_target: embedded
            .map(|value| remote_note_optional_uri(value.get("interactionTarget")))
            .transpose()?
            .flatten(),
        typed: embedded
            .is_some_and(|value| equals_or_includes(value.get("type"), "QuoteAuthorization")),
    })
}

fn remote_quote_data(
    object: &serde_json::Map<String, Value>,
) -> Result<Option<RemoteQuoteData>, WriteError> {
    let Some((field, value)) = ["quote", "_misskey_quote", "quoteUrl", "quoteUri"]
        .into_iter()
        .find_map(|field| object.get(field).map(|value| (field, value)))
    else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let legacy = field != "quote";
    let deleted = value
        .as_object()
        .is_some_and(|quote| equals_or_includes(quote.get("type"), "Tombstone"));
    if let Some(quote) = value.as_object()
        && !deleted
        && !equals_or_includes(quote.get("type"), "Note")
        && !equals_or_includes(quote.get("type"), "Question")
    {
        return Err(WriteError::InvalidInput(
            "remote quote object type is invalid",
        ));
    }
    let target_uri = if deleted
        && value
            .as_object()
            .is_some_and(|quote| quote.get("id").or_else(|| quote.get("href")).is_none())
    {
        None
    } else {
        remote_note_uri(Some(value))?
    };
    if target_uri.is_none() && !deleted {
        return Err(WriteError::InvalidInput("remote quote has no target"));
    }
    let authorization = object
        .get("quoteAuthorization")
        .and_then(|value| {
            value
                .as_array()
                .and_then(|values| values.first())
                .or(Some(value))
        })
        .filter(|value| !value.is_null())
        .map(remote_quote_authorization_data)
        .transpose()?;
    Ok(Some(RemoteQuoteData {
        target_uri,
        authorization_uri: authorization.map(|authorization| authorization.uri),
        legacy,
        deleted,
    }))
}

fn remote_poll_data(object: &serde_json::Map<String, Value>) -> Result<RemotePollData, WriteError> {
    let (multiple, values) = if let Some(Value::Array(values)) = object.get("anyOf") {
        (true, values)
    } else if let Some(Value::Array(values)) = object.get("oneOf") {
        (false, values)
    } else {
        return Err(WriteError::InvalidInput(
            "remote Question options are invalid",
        ));
    };
    let mut options = Vec::new();
    let mut tallies = Vec::new();
    for value in values.iter().take(500) {
        let option = value.as_object().ok_or(WriteError::InvalidInput(
            "remote Question option is invalid",
        ))?;
        if let Some(title) = option
            .get("name")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .or_else(|| {
                option
                    .get("content")
                    .and_then(Value::as_str)
                    .filter(|title| !title.trim().is_empty())
            })
        {
            options.push(title.to_owned());
        }
        tallies.push(
            option
                .get("replies")
                .and_then(Value::as_object)
                .and_then(|replies| replies.get("totalItems"))
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .max(0),
        );
    }
    if options.is_empty() {
        return Err(WriteError::InvalidInput(
            "remote Question options are empty",
        ));
    }
    let closed = object.get("closed");
    let expires_at = match closed {
        Some(Value::String(value)) => DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.naive_utc()),
        Some(Value::Bool(true) | Value::Number(_) | Value::Array(_) | Value::Object(_)) => {
            Some(Utc::now().naive_utc())
        }
        None | Some(Value::Null | Value::Bool(false)) => object
            .get("endTime")
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.naive_utc()),
    };
    Ok(RemotePollData {
        options,
        tallies,
        multiple,
        expires_at,
        voters_count: object
            .get("votersCount")
            .and_then(Value::as_i64)
            .map(|count| count.max(0)),
    })
}

async fn upsert_remote_emojis(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
    actor_uri: &str,
    object: &Value,
) -> Result<(), WriteError> {
    let domain = canonical_remote_domain(domain)
        .map_err(|_| WriteError::InvalidInput("remote emoji domain is invalid"))?;
    for emoji in parse_note_emojis(object, actor_uri) {
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock(
                 pg_catalog.hashtextextended($1 || ':' || $2, 0)
             )",
        )
        .bind(&domain)
        .bind(&emoji.shortcode)
        .execute(&mut **transaction)
        .await?;
        let existing = sqlx::query_as::<_, (i64, Option<String>, Option<String>, NaiveDateTime)>(
            "SELECT id, image_remote_url, image_file_name, updated_at
             FROM custom_emojis WHERE shortcode = $1 AND domain = $2 FOR UPDATE",
        )
        .bind(&emoji.shortcode)
        .bind(&domain)
        .fetch_optional(&mut **transaction)
        .await?;
        let (emoji_id, should_fetch) =
            if let Some((id, remote_url, file_name, updated_at)) = existing {
                let changed_url = remote_url.as_deref() != Some(emoji.image_url.as_str());
                let fresh = emoji
                    .updated_at
                    .is_some_and(|updated| updated >= updated_at);
                let (update_metadata, should_fetch) =
                    remote_emoji_update_decision(changed_url, fresh, file_name.is_some());
                if !update_metadata && !should_fetch {
                    continue;
                }
                if update_metadata {
                    sqlx::query(
                        "UPDATE custom_emojis SET image_remote_url = $2,
                         uri = COALESCE($3, uri), updated_at = clock_timestamp()
                     WHERE id = $1",
                    )
                    .bind(id)
                    .bind(&emoji.image_url)
                    .bind(&emoji.uri)
                    .execute(&mut **transaction)
                    .await?;
                }
                (id, should_fetch)
            } else {
                let id = sqlx::query_scalar::<_, i64>(
                    "INSERT INTO custom_emojis
                    (shortcode, domain, uri, image_remote_url, disabled, visible_in_picker,
                     created_at, updated_at)
                 VALUES ($1, $2, $3, $4, false, true, clock_timestamp(), clock_timestamp())
                 ON CONFLICT (shortcode, domain) DO UPDATE SET updated_at = custom_emojis.updated_at
                 RETURNING id",
                )
                .bind(&emoji.shortcode)
                .bind(&domain)
                .bind(&emoji.uri)
                .bind(&emoji.image_url)
                .fetch_one(&mut **transaction)
                .await?;
                (id, true)
            };
        if should_fetch {
            let digest = Sha256::digest(emoji.image_url.as_bytes());
            let job = JobSpec::new(
                Lane::Pull,
                ACTIVITYPUB_EMOJI_FETCH_JOB_KIND,
                json!({
                    "emoji_id": emoji_id,
                    "remote_url": emoji.image_url,
                    "media_type": emoji.media_type,
                    "domain": domain
                }),
            )
            .logical_key(format!("activitypub:emoji:{emoji_id}:{digest:x}"))
            .max_attempts(4);
            record_outbox_in(transaction, &job).await?;
        }
    }
    Ok(())
}

fn remote_emoji_update_decision(
    changed_url: bool,
    fresh_timestamp: bool,
    has_file: bool,
) -> (bool, bool) {
    // Mastodon accepts fresher metadata at the same URL without downloading an installed file again.
    (changed_url || fresh_timestamp, changed_url || !has_file)
}

async fn lock_remote_note(
    transaction: &mut Transaction<'_, Postgres>,
    uri: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "SELECT pg_catalog.pg_advisory_xact_lock(
            pg_catalog.hashtextextended($1, 0)
         )",
    )
    .bind(uri)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn lock_remote_interaction(
    transaction: &mut Transaction<'_, Postgres>,
    activity_uri: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "SELECT pg_catalog.pg_advisory_xact_lock(
            pg_catalog.hashtextextended($1, 0)
         )",
    )
    .bind(activity_uri)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn remote_quote_request_decision_in(
    transaction: &mut Transaction<'_, Postgres>,
    request_uri: &str,
    actor_uri: &str,
    quoted_status_uri: &str,
    instrument_uri: &str,
) -> Result<Option<bool>, WriteError> {
    let logical_key = activitypub::quote_request_decision_logical_key(request_uri);
    let body = sqlx::query_scalar::<_, Value>(
        "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events \
         WHERE kind = $1 AND logical_key = $2",
    )
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(logical_key)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(body) = body else {
        return Ok(None);
    };
    let accepted = match body.get("type").and_then(Value::as_str) {
        Some("Accept") => true,
        Some("Reject") => false,
        _ => return Err(WriteError::Conflict),
    };
    let request = body
        .get("object")
        .and_then(Value::as_object)
        .ok_or(WriteError::Conflict)?;
    if request.get("id").and_then(Value::as_str) != Some(request_uri)
        || request.get("actor").and_then(Value::as_str) != Some(actor_uri)
        || request.get("object").and_then(Value::as_str) != Some(quoted_status_uri)
        || request.get("instrument").and_then(Value::as_str) != Some(instrument_uri)
    {
        return Err(WriteError::Conflict);
    }
    Ok(Some(accepted))
}

async fn remote_interaction_actor_matches(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    actor_uri: &str,
    require_active: bool,
) -> Result<bool, WriteError> {
    let Some((domain, current_uri, suspended_at)) =
        sqlx::query_as::<_, (Option<String>, String, Option<NaiveDateTime>)>(
            "SELECT domain, uri, suspended_at FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(false);
    };
    Ok(domain.is_some() && current_uri == actor_uri && (!require_active || suspended_at.is_none()))
}

async fn local_interaction_target(
    transaction: &mut Transaction<'_, Postgres>,
    object_uri: &str,
    origin: &str,
) -> Result<Option<(i64, i64, i32)>, WriteError> {
    let origin = origin.trim_end_matches('/');
    Ok(sqlx::query_as::<_, (i64, i64, i32)>(
        "SELECT status.id, status.account_id, status.visibility
           FROM statuses status
           JOIN accounts author ON author.id = status.account_id
                              AND author.domain IS NULL
          WHERE status.deleted_at IS NULL
            AND (
              status.uri = $1
              OR status.url = $1
              OR $1 = $2 || '/actor/statuses/' || status.id::text
                   || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
              OR $1 = $2 || '/@' || author.username || '/' || status.id::text
                   || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
              OR $1 = $2 || '/users/' || author.username || '/statuses/' || status.id::text
                   || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
              OR $1 = $2 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text
                   || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
            )
          ORDER BY status.id
          LIMIT 1 FOR UPDATE",
    )
    .bind(object_uri)
    .bind(origin)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn announce_interaction_target(
    transaction: &mut Transaction<'_, Postgres>,
    object_uri: &str,
    origin: &str,
) -> Result<Option<(i64, i64, i32, bool, bool)>, WriteError> {
    if let Some((status_id, _, _)) =
        local_interaction_target(transaction, object_uri, origin).await?
    {
        return announce_target_from_status(transaction, status_id, true).await;
    }

    let matched_status_id = sqlx::query_scalar::<_, i64>(
        "SELECT status.id
           FROM statuses status
           JOIN accounts author ON author.id = status.account_id
                              AND author.domain IS NOT NULL
           WHERE status.deleted_at IS NULL
             AND (status.uri = $1 OR status.url = $1)
           ORDER BY status.id
           LIMIT 1",
    )
    .bind(object_uri)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(matched_status_id) = matched_status_id else {
        return Ok(None);
    };
    announce_target_from_status(transaction, matched_status_id, false).await
}

async fn announce_target_from_status(
    transaction: &mut Transaction<'_, Postgres>,
    matched_status_id: i64,
    matched_account_is_local: bool,
) -> Result<Option<(i64, i64, i32, bool, bool)>, WriteError> {
    let target = sqlx::query_as::<_, (i64, i64, i32, bool)>(
        "SELECT target.id, target.account_id, target.visibility, target_author.domain IS NULL
           FROM statuses matched
           JOIN statuses target ON target.id = COALESCE(matched.reblog_of_id, matched.id)
           JOIN accounts target_author ON target_author.id = target.account_id
          WHERE matched.id = $1 AND matched.deleted_at IS NULL AND target.deleted_at IS NULL
          FOR UPDATE OF target",
    )
    .bind(matched_status_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if target.is_some()
        && sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(matched_status_id)
        .fetch_optional(&mut **transaction)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    Ok(target.map(
        |(target_status_id, recipient_account_id, target_visibility, original_account_is_local)| {
            (
                target_status_id,
                recipient_account_id,
                target_visibility,
                matched_account_is_local,
                original_account_is_local,
            )
        },
    ))
}

async fn remote_announce_is_relevant(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM follows follow
            JOIN accounts local_account ON local_account.id = follow.account_id
                                         AND local_account.domain IS NULL
            WHERE follow.target_account_id = $1
        )",
    )
    .bind(account_id)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn remote_announce_notification_suppressed(
    transaction: &mut Transaction<'_, Postgres>,
    reblogger_account_id: i64,
    original_author_account_id: i64,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1
              FROM accounts reblogger
              JOIN follows follow ON follow.target_account_id = reblogger.id
             WHERE reblogger.id = $1
               AND reblogger.actor_type = 'Group'
               AND follow.account_id = $2
        )",
    )
    .bind(reblogger_account_id)
    .bind(original_author_account_id)
    .fetch_one(&mut **transaction)
    .await?)
}

fn remote_interaction_visibility(to: &[String], cc: &[String], followers_url: &str) -> i32 {
    if to.iter().any(|uri| activitypub::is_public_address(uri)) {
        return 0;
    }
    if cc.iter().any(|uri| activitypub::is_public_address(uri)) {
        return 1;
    }
    if to
        .iter()
        .any(|uri| uri == followers_url && !followers_url.is_empty())
    {
        return 2;
    }
    3
}

fn remote_interaction_timestamp(published_at: Option<&str>) -> Result<NaiveDateTime, WriteError> {
    let Some(published_at) = published_at else {
        return Ok(Utc::now().naive_utc());
    };
    let timestamp = DateTime::parse_from_rfc3339(published_at)
        .map(|timestamp| timestamp.naive_utc())
        .map_err(|_| WriteError::InvalidInput("remote Announce timestamp is invalid"))?;
    if timestamp > Utc::now().naive_utc() + ChronoDuration::hours(24) {
        return Err(WriteError::InvalidInput(
            "remote Announce timestamp is too far in the future",
        ));
    }
    Ok(timestamp)
}

async fn remote_interaction_tombstoned(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    activity_uri: &str,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tombstones WHERE account_id = $1 AND uri = $2)",
    )
    .bind(account_id)
    .bind(activity_uri)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn remote_note_status(
    transaction: &mut Transaction<'_, Postgres>,
    uri: &str,
    atom_uri: Option<&str>,
) -> Result<
    Option<(
        i64,
        i64,
        Option<NaiveDateTime>,
        Option<NaiveDateTime>,
        NaiveDateTime,
    )>,
    WriteError,
> {
    Ok(sqlx::query_as::<
        _,
        (
            i64,
            i64,
            Option<NaiveDateTime>,
            Option<NaiveDateTime>,
            NaiveDateTime,
        ),
    >(
        "SELECT id, account_id, deleted_at, edited_at, created_at FROM statuses
         WHERE uri = $1 OR ($2::text IS NOT NULL AND uri = $2)
         ORDER BY id LIMIT 1 FOR UPDATE",
    )
    .bind(uri)
    .bind(atom_uri)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn remote_note_status_id_for_account(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    uri: &str,
    atom_uri: Option<&str>,
) -> Result<Option<i64>, WriteError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT id FROM statuses
          WHERE account_id = $1
            AND (uri = $2 OR ($3::text IS NOT NULL AND uri = $3))
          ORDER BY id LIMIT 1",
    )
    .bind(account_id)
    .bind(uri)
    .bind(atom_uri)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn remote_note_tombstoned(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    uri: &str,
    atom_uri: Option<&str>,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tombstones
         WHERE account_id = $1 AND (uri = $2 OR ($3::text IS NOT NULL AND uri = $3)))",
    )
    .bind(account_id)
    .bind(uri)
    .bind(atom_uri)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn insert_remote_note_tombstone(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    uri: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO tombstones (account_id, uri, by_moderator, created_at, updated_at)
         SELECT $1, $2, false, clock_timestamp(), clock_timestamp()
         WHERE NOT EXISTS (SELECT 1 FROM tombstones WHERE account_id = $1 AND uri = $2)",
    )
    .bind(account_id)
    .bind(uri)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn same_remote_note_host(actor_uri: &str, object_uri: &str) -> Result<bool, WriteError> {
    let actor = Url::parse(actor_uri)
        .map_err(|_| WriteError::InvalidInput("remote actor URI is invalid"))?;
    let object = Url::parse(object_uri)
        .map_err(|_| WriteError::InvalidInput("remote object URI is invalid"))?;
    Ok(actor
        .host_str()
        .zip(object.host_str())
        .is_some_and(|(actor_host, object_host)| actor_host.eq_ignore_ascii_case(object_host)))
}

fn remote_note_visibility(audience: &RemoteNoteAudience, followers_url: &str) -> i32 {
    if audience
        .to
        .iter()
        .any(|uri| activitypub::is_public_address(uri))
    {
        return 0;
    }
    if audience
        .cc
        .iter()
        .any(|uri| activitypub::is_public_address(uri))
    {
        return 1;
    }
    if audience
        .to
        .iter()
        .any(|uri| uri == followers_url && !followers_url.is_empty())
    {
        return 2;
    }
    4
}

async fn remote_note_has_only_explicit_recipients(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    note: &RemoteNoteData,
) -> Result<bool, WriteError> {
    // Local mentions already resolve actor aliases and include the delivery target.
    // Known remote audience accounts also count, but unknown URIs and collections
    // must not manufacture silent recipients or trigger remote resolution here.
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM mentions WHERE status_id = $1 AND silent IS FALSE
            UNION ALL
            SELECT 1 FROM accounts WHERE domain IS NOT NULL AND uri = ANY($2::text[])
         ) AND NOT EXISTS (
            SELECT 1 FROM mentions WHERE status_id = $1 AND silent IS TRUE
            UNION ALL
            SELECT 1 FROM accounts
             WHERE domain IS NOT NULL
               AND (uri = ANY($3::text[]) OR uri = ANY($4::text[]))
               AND NOT (uri = ANY($2::text[]))
         )",
    )
    .bind(status_id)
    .bind(&note.mentions)
    .bind(&note.audience.to)
    .bind(&note.audience.cc)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn remote_note_thread(
    transaction: &mut Transaction<'_, Postgres>,
    note: &RemoteNoteData,
    origin: &str,
) -> Result<(Option<i64>, Option<i64>, Option<i64>), WriteError> {
    let Some(parent_uri) = note.in_reply_to_uri.as_deref() else {
        return Ok((None, None, None));
    };
    let parent = sqlx::query_as::<_, (i64, i64, Option<i64>)>(
        "SELECT status.id, status.account_id, status.conversation_id
           FROM statuses status
           JOIN accounts author ON author.id = status.account_id
          WHERE status.deleted_at IS NULL
            AND (
              status.uri = $1
              OR status.url = $1
              OR (author.domain IS NULL AND (
                $1 = $2 || '/actor/statuses/' || status.id::text
                     || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                OR $1 = $2 || '/@' || author.username || '/' || status.id::text
                     || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                OR $1 = $2 || '/users/' || author.username || '/statuses/' || status.id::text
                     || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                OR $1 = $2 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text
                     || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
              ))
            )
          ORDER BY status.id
          LIMIT 1
          FOR UPDATE",
    )
    .bind(parent_uri)
    .bind(origin.trim_end_matches('/'))
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(
        parent.map_or((None, None, None), |(id, account_id, conversation_id)| {
            (Some(id), Some(account_id), conversation_id)
        }),
    )
}

#[allow(clippy::too_many_arguments)]
async fn local_actor_uri_for_account(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    origin: &str,
) -> Result<String, WriteError> {
    sqlx::query_scalar::<_, String>(
        "SELECT CASE WHEN id = -99 THEN $2 || '/actor' \
                     WHEN id_scheme = 1 THEN $2 || '/ap/users/' || id::text \
                     ELSE $2 || '/users/' || username END \
           FROM accounts WHERE id = $1 AND domain IS NULL",
    )
    .bind(account_id)
    .bind(origin.trim_end_matches('/'))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)
}

async fn quote_target_matches_uri(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    uri: &str,
    origin: &str,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM statuses status \
           JOIN accounts author ON author.id = status.account_id \
          WHERE status.id = $1 AND status.deleted_at IS NULL AND ( \
                status.uri = $2 OR status.url = $2 OR (author.domain IS NULL AND ( \
                $2 = $3 || '/actor/statuses/' || status.id::text \
                OR $2 = $3 || '/@' || author.username || '/' || status.id::text \
                OR $2 = $3 || '/users/' || author.username || '/statuses/' || status.id::text \
                OR $2 = $3 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text))))",
    )
    .bind(status_id)
    .bind(uri)
    .bind(origin.trim_end_matches('/'))
    .fetch_one(&mut **transaction)
    .await?)
}

async fn resolve_quote_target(
    transaction: &mut Transaction<'_, Postgres>,
    target_uri: &str,
    origin: &str,
) -> Result<Option<(i64, i64, bool, String)>, WriteError> {
    Ok(sqlx::query_as::<_, (i64, i64, bool, String)>(
        "SELECT status.id, status.account_id, author.domain IS NULL, \
                CASE WHEN author.domain IS NULL THEN \
                  CASE WHEN author.id = -99 THEN $2 || '/actor' \
                       WHEN author.id_scheme = 1 THEN $2 || '/ap/users/' || author.id::text \
                       ELSE $2 || '/users/' || author.username END \
                ELSE author.uri END \
           FROM statuses status \
           JOIN accounts author ON author.id = status.account_id \
          WHERE status.deleted_at IS NULL AND status.reblog_of_id IS NULL AND ( \
                status.uri = $1 OR status.url = $1 OR (author.domain IS NULL AND ( \
                $1 = $2 || '/actor/statuses/' || status.id::text \
                OR $1 = $2 || '/@' || author.username || '/' || status.id::text \
                OR $1 = $2 || '/users/' || author.username || '/statuses/' || status.id::text \
                OR $1 = $2 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text))) \
          ORDER BY status.id LIMIT 1",
    )
    .bind(target_uri)
    .bind(origin.trim_end_matches('/'))
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn quote_target_id_for_status(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<Option<i64>, WriteError> {
    Ok(sqlx::query_scalar::<_, Option<i64>>(
        "SELECT quoted_status_id FROM quotes WHERE status_id = $1",
    )
    .bind(status_id)
    .fetch_optional(&mut **transaction)
    .await?
    .flatten())
}

async fn lock_quote_status_deletion(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), WriteError> {
    // Discovering every status in a quote component requires locking one endpoint first.
    // Serialize status deletions so two endpoint removals cannot each hold that first row
    // while waiting for the other's quote/status lock.
    sqlx::query("SELECT pg_advisory_xact_lock(7640897321347509341::bigint)")
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn lock_statuses_in_order(
    transaction: &mut Transaction<'_, Postgres>,
    status_ids: &[i64],
) -> Result<(), WriteError> {
    let mut status_ids = status_ids.to_vec();
    status_ids.sort_unstable();
    status_ids.dedup();
    if !status_ids.is_empty() {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind(status_ids)
        .fetch_all(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn record_remote_quote_authorization_forwarding_in(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    actor_uri: &str,
    status_id: i64,
    activity: &Value,
) -> Result<(), WriteError> {
    let (visibility, parent_account_id, source_inbox) =
        sqlx::query_as::<_, (i32, Option<i64>, String)>(
            "SELECT status.visibility, parent.id,
                    COALESCE(NULLIF(source.shared_inbox_url, ''), source.inbox_url)
               FROM statuses status
               JOIN accounts source ON source.id = $2 AND source.uri = $3
                                   AND source.domain IS NOT NULL
          LEFT JOIN statuses parent_status ON parent_status.id = status.in_reply_to_id
                                           AND parent_status.deleted_at IS NULL
          LEFT JOIN accounts parent ON parent.id = parent_status.account_id
                                    AND parent.domain IS NULL
              WHERE status.id = $1 AND status.deleted_at IS NULL",
        )
        .bind(status_id)
        .bind(source_account_id)
        .bind(actor_uri)
        .fetch_one(&mut **transaction)
        .await?;
    if !matches!(visibility, 0 | 1) {
        return Ok(());
    }
    let activity_uri = activity
        .get("id")
        .and_then(Value::as_str)
        .filter(|uri| !uri.trim().is_empty())
        .ok_or(WriteError::InvalidInput("signed remote activity has no ID"))?;
    record_remote_activity_forwarding_for_status_in(
        transaction,
        status_id,
        parent_account_id,
        &source_inbox,
        activity_uri,
        activity,
    )
    .await
}

async fn record_remote_activity_forwarding_for_status_in(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    parent_account_id: Option<i64>,
    source_inbox: &str,
    activity_uri: &str,
    activity: &Value,
) -> Result<(), WriteError> {
    let shared_account_ids = sqlx::query_scalar::<_, i64>(
        "SELECT shared.account_id
           FROM (
             SELECT reblog.account_id, 0 AS share_kind, reblog.id AS share_id
               FROM statuses reblog
               JOIN accounts account ON account.id = reblog.account_id
                                    AND account.domain IS NULL
              WHERE reblog.reblog_of_id = $1
                AND reblog.deleted_at IS NULL
             UNION ALL
             SELECT quote.account_id, 1 AS share_kind, quote.id AS share_id
               FROM quotes quote
               JOIN accounts account ON account.id = quote.account_id
                                    AND account.domain IS NULL
              WHERE quote.quoted_status_id = $1 AND quote.state = 1
                AND EXISTS (SELECT 1 FROM statuses quoting
                             WHERE quoting.id = quote.status_id
                               AND quoting.deleted_at IS NULL)
           ) shared
          ORDER BY shared.share_kind, shared.share_id DESC, shared.account_id",
    )
    .bind(status_id)
    .fetch_all(&mut **transaction)
    .await?;
    let source_account_id = parent_account_id.or_else(|| shared_account_ids.first().copied());
    let Some(source_account_id) = source_account_id else {
        return Ok(());
    };
    let mut target_account_ids = shared_account_ids;
    if let Some(parent_account_id) = parent_account_id {
        target_account_ids.push(parent_account_id);
    }
    target_account_ids.sort_unstable();
    target_account_ids.dedup();
    let followers = sqlx::query_as::<_, (String, String)>(
        "SELECT DISTINCT
                COALESCE(NULLIF(follower.shared_inbox_url, ''), follower.inbox_url),
                follower.domain
           FROM follows follow
           JOIN accounts follower ON follower.id = follow.account_id
                                 AND follower.domain IS NOT NULL
                                 AND follower.protocol = 1
                                 AND follower.suspended_at IS NULL
          WHERE follow.target_account_id = ANY($1)
            AND COALESCE(NULLIF(follower.shared_inbox_url, ''), follower.inbox_url) <> ''
            AND COALESCE(NULLIF(follower.shared_inbox_url, ''), follower.inbox_url) <> $2
           ORDER BY 1, 2",
    )
    .bind(&target_account_ids)
    .bind(source_inbox)
    .fetch_all(&mut **transaction)
    .await?;
    for (inbox_url, remote_domain) in followers {
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "source_account_id": source_account_id,
                "inbox_url": inbox_url,
                "remote_domain": remote_domain,
                "body": activity
            }),
        )
        .logical_key(activitypub::forward_delivery_logical_key(
            source_account_id,
            activity_uri,
            &inbox_url,
        ));
        record_outbox_once_in(transaction, &delivery).await?;
    }
    Ok(())
}

async fn locked_quote_for_status(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<
    Option<(
        i64,
        Option<i64>,
        Option<i64>,
        i32,
        Option<String>,
        bool,
        Option<String>,
    )>,
    WriteError,
> {
    Ok(sqlx::query_as::<
        _,
        (
            i64,
            Option<i64>,
            Option<i64>,
            i32,
            Option<String>,
            bool,
            Option<String>,
        ),
    >(
        "SELECT id, quoted_status_id, quoted_account_id, state, approval_uri, legacy, \
                    activity_uri \
           FROM quotes WHERE status_id = $1 FOR UPDATE",
    )
    .bind(status_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

fn quote_state_update_counter_delta(legacy: bool, old_state: i32, new_state: i32) -> i8 {
    if legacy || old_state == new_state {
        0
    } else if old_state != 1 && new_state == 1 {
        1
    } else if old_state == 1 && new_state != 1 {
        -1
    } else {
        0
    }
}

fn reconciled_remote_quote_state(
    target_changed: bool,
    old_state: i32,
    old_approval_uri: Option<&str>,
    advertised_approval_uri: Option<&str>,
    computed_state: i32,
) -> (i32, Option<String>) {
    if target_changed {
        return (computed_state, None);
    }
    if old_state == 1 && old_approval_uri.is_some() && old_approval_uri != advertised_approval_uri {
        (0, None)
    } else {
        (old_state, old_approval_uri.map(str::to_owned))
    }
}

async fn prelock_remote_note_quote_targets(
    transaction: &mut Transaction<'_, Postgres>,
    existing_status_id: Option<i64>,
    note: &RemoteNoteData,
    origin: &str,
) -> Result<(), WriteError> {
    let mut target_status_ids = existing_status_id.into_iter().collect::<Vec<_>>();
    if let Some(existing_status_id) = existing_status_id
        && let Some(target_status_id) =
            quote_target_id_for_status(transaction, existing_status_id).await?
    {
        target_status_ids.push(target_status_id);
    }
    if let Some(quote) = note.quote.as_ref()
        && !quote.deleted
        && let Some(target_uri) = quote.target_uri.as_deref()
        && let Some((target_status_id, ..)) =
            resolve_quote_target(transaction, target_uri, origin).await?
    {
        target_status_ids.push(target_status_id);
    }
    lock_statuses_in_order(transaction, &target_status_ids).await
}

#[allow(clippy::too_many_arguments)]
async fn insert_reconciled_remote_quote(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    status_id: i64,
    quoted_status_id: Option<i64>,
    quoted_account_id: Option<i64>,
    state: i32,
    approval_uri: Option<&str>,
    legacy: bool,
) -> Result<i64, WriteError> {
    let quote_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO quotes (account_id, activity_uri, approval_uri, created_at, legacy, \
             quoted_account_id, quoted_status_id, state, status_id, updated_at) \
         VALUES ($1, NULL, $2, clock_timestamp(), $3, $4, $5, $6, $7, clock_timestamp()) \
         RETURNING id",
    )
    .bind(account_id)
    .bind(approval_uri)
    .bind(legacy)
    .bind(quoted_account_id)
    .bind(quoted_status_id)
    .bind(state)
    .bind(status_id)
    .fetch_one(&mut **transaction)
    .await?;
    if let Some(quoted_account_id) = quoted_account_id {
        sqlx::query(
            "INSERT INTO mentions (id, account_id, created_at, silent, status_id, updated_at) \
             VALUES (nextval('mentions_id_seq'), $1, clock_timestamp(), true, $2, clock_timestamp()) \
             ON CONFLICT (account_id, status_id) DO NOTHING",
        )
        .bind(quoted_account_id)
        .bind(status_id)
        .execute(&mut **transaction)
        .await?;
        if state == 1 {
            if !legacy {
                increment_quote_count(
                    transaction,
                    quoted_status_id.expect("accepted quote has a target"),
                )
                .await?;
            }
            record_outbox_in(
                transaction,
                &notification_job(quoted_account_id, NOTIFICATION_QUOTE, quote_id),
            )
            .await?;
        }
    }
    Ok(quote_id)
}

#[allow(clippy::too_many_lines)]
async fn reconcile_remote_note_quote(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    note: &RemoteNoteData,
    origin: &str,
) -> Result<bool, WriteError> {
    let existing_target_id = quote_target_id_for_status(transaction, status_id).await?;
    let Some(quote) = note.quote.as_ref() else {
        if let Some(existing_target_id) = existing_target_id {
            lock_statuses_in_order(transaction, &[existing_target_id]).await?;
        }
        let existing = locked_quote_for_status(transaction, status_id).await?;
        if let Some((quote_id, quoted_status_id, quoted_account_id, state, _, _, request_uri)) =
            existing
        {
            cancel_quote_request_outbox(transaction, quote_id, request_uri.as_deref()).await?;
            if state == 1
                && let (Some(quoted_status_id), Some(quoted_account_id)) =
                    (quoted_status_id, quoted_account_id)
                && sqlx::query_scalar::<_, bool>(
                    "SELECT domain IS NULL FROM accounts WHERE id = $1",
                )
                .bind(quoted_account_id)
                .fetch_one(&mut **transaction)
                .await?
            {
                record_quote_authorization_delete(
                    transaction,
                    quote_id,
                    status_id,
                    quoted_status_id,
                    quoted_account_id,
                    origin,
                )
                .await?;
            }
            if state == 1
                && let Some(quoted_status_id) = quoted_status_id
            {
                decrement_quote_count(transaction, quoted_status_id).await?;
            }
            if let Some(quoted_account_id) = quoted_account_id {
                delete_activity_notifications(transaction, quoted_account_id, quote_id, "Quote")
                    .await?;
                sqlx::query(
                    "DELETE FROM mentions WHERE status_id = $1 AND account_id = $2 AND silent = true",
                )
                .bind(status_id)
                .bind(quoted_account_id)
                .execute(&mut **transaction)
                .await?;
            }
            sqlx::query(
                "UPDATE quotes SET quoted_status_id = NULL, quoted_account_id = NULL, \
                        approval_uri = NULL, activity_uri = NULL, state = 4, legacy = true, \
                        updated_at = clock_timestamp() WHERE id = $1",
            )
            .bind(quote_id)
            .execute(&mut **transaction)
            .await?;
            return Ok(true);
        }
        return Ok(false);
    };
    let target = if quote.deleted {
        None
    } else {
        let target_uri = quote
            .target_uri
            .as_deref()
            .ok_or(WriteError::InvalidInput("remote quote has no target"))?;
        resolve_quote_target(transaction, target_uri, origin).await?
    };
    if target.is_none() && !quote.deleted {
        // There is no safe generic quote-target fetch path. Do not dereference an arbitrary URI.
        return Ok(false);
    }
    let mut target_status_ids = target
        .as_ref()
        .map(|(target_status_id, ..)| *target_status_id)
        .into_iter()
        .collect::<Vec<_>>();
    target_status_ids.extend(existing_target_id);
    lock_statuses_in_order(transaction, &target_status_ids).await?;
    let existing = locked_quote_for_status(transaction, status_id).await?;
    let (quoted_status_id, quoted_account_id, mut state, mut approval_uri) =
        if let Some((target_status_id, target_account_id, _, _)) = target {
            let approval_uri = None;
            let accepted = target_account_id == account_id;
            (
                Some(target_status_id),
                Some(target_account_id),
                i32::from(accepted),
                approval_uri,
            )
        } else {
            (None, None, 4, None)
        };
    if let Some((
        quote_id,
        old_target_id,
        old_account_id,
        old_state,
        old_approval,
        old_legacy,
        old_request_uri,
    )) = existing
    {
        let target_changed =
            old_target_id != quoted_status_id || old_account_id != quoted_account_id;
        let (reconciled_state, reconciled_approval_uri) = reconciled_remote_quote_state(
            target_changed,
            old_state,
            old_approval.as_deref(),
            quote.authorization_uri.as_deref(),
            state,
        );
        state = reconciled_state;
        approval_uri = reconciled_approval_uri;
        if old_target_id == quoted_status_id
            && old_account_id == quoted_account_id
            && old_state == state
            && old_approval == approval_uri
            && old_legacy == quote.legacy
        {
            return Ok(false);
        }
        cancel_quote_request_outbox(transaction, quote_id, old_request_uri.as_deref()).await?;
        if target_changed
            && old_state == 1
            && let (Some(old_target_id), Some(old_account_id)) = (old_target_id, old_account_id)
            && sqlx::query_scalar::<_, bool>("SELECT domain IS NULL FROM accounts WHERE id = $1")
                .bind(old_account_id)
                .fetch_one(&mut **transaction)
                .await?
        {
            record_quote_authorization_delete(
                transaction,
                quote_id,
                status_id,
                old_target_id,
                old_account_id,
                origin,
            )
            .await?;
        }
        if ((target_changed && old_state == 1)
            || (!target_changed
                && quote_state_update_counter_delta(quote.legacy, old_state, state) < 0))
            && let Some(old_target_id) = old_target_id
        {
            decrement_quote_count(transaction, old_target_id).await?;
        }
        if let Some(old_account_id) = old_account_id {
            if target_changed || old_state != state {
                delete_activity_notifications(transaction, old_account_id, quote_id, "Quote")
                    .await?;
            }
            if target_changed && Some(old_account_id) != quoted_account_id {
                sqlx::query(
                    "DELETE FROM mentions WHERE status_id = $1 AND account_id = $2 AND silent = true",
                )
                .bind(status_id)
                .bind(old_account_id)
                .execute(&mut **transaction)
                .await?;
            }
        }
        if target_changed {
            sqlx::query(
                "UPDATE quotes SET quoted_status_id = $2, quoted_account_id = $3, state = $4, \
                        approval_uri = $5, activity_uri = NULL, legacy = $6, \
                        updated_at = clock_timestamp() WHERE id = $1",
            )
            .bind(quote_id)
            .bind(quoted_status_id)
            .bind(quoted_account_id)
            .bind(state)
            .bind(&approval_uri)
            .bind(quote.legacy)
            .execute(&mut **transaction)
            .await?;
            if let Some(quoted_account_id) = quoted_account_id {
                sqlx::query(
                    "INSERT INTO mentions (id, account_id, created_at, silent, status_id, updated_at) \
                     VALUES (nextval('mentions_id_seq'), $1, clock_timestamp(), true, $2, clock_timestamp()) \
                     ON CONFLICT (account_id, status_id) DO NOTHING",
                )
                .bind(quoted_account_id)
                .bind(status_id)
                .execute(&mut **transaction)
                .await?;
                if state == 1 {
                    if !quote.legacy {
                        increment_quote_count(
                            transaction,
                            quoted_status_id.expect("accepted quote has a target"),
                        )
                        .await?;
                    }
                    record_outbox_in(
                        transaction,
                        &notification_job(quoted_account_id, NOTIFICATION_QUOTE, quote_id),
                    )
                    .await?;
                }
            }
            return Ok(true);
        }
        sqlx::query(
            "UPDATE quotes SET state = $2, approval_uri = $3, legacy = $4, \
                    updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(quote_id)
        .bind(state)
        .bind(&approval_uri)
        .bind(quote.legacy)
        .execute(&mut **transaction)
        .await?;
        if quote_state_update_counter_delta(quote.legacy, old_state, state) > 0
            && let Some(quoted_status_id) = quoted_status_id
        {
            increment_quote_count(transaction, quoted_status_id).await?;
            if let Some(quoted_account_id) = quoted_account_id {
                record_outbox_in(
                    transaction,
                    &notification_job(quoted_account_id, NOTIFICATION_QUOTE, quote_id),
                )
                .await?;
            }
        }
    } else {
        insert_reconciled_remote_quote(
            transaction,
            account_id,
            status_id,
            quoted_status_id,
            quoted_account_id,
            state,
            approval_uri.as_deref(),
            quote.legacy,
        )
        .await?;
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
async fn ensure_remote_note_conversation(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    in_reply_to_id: Option<i64>,
    in_reply_to_account_id: Option<i64>,
    conversation_id: Option<i64>,
    conversation_uri: Option<&str>,
    created_at: NaiveDateTime,
) -> Result<Option<i64>, WriteError> {
    if conversation_id.is_some() {
        return Ok(conversation_id);
    }
    let conversation_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO conversations (
            created_at, parent_account_id, parent_status_id, updated_at, uri
         ) VALUES ($1, $2, $3, clock_timestamp(), $4)
         ON CONFLICT (uri) WHERE uri IS NOT NULL
         DO UPDATE SET updated_at = clock_timestamp()
         RETURNING id",
    )
    .bind(created_at)
    .bind(in_reply_to_account_id.or(Some(account_id)))
    .bind(in_reply_to_id.or(Some(status_id)))
    .bind(conversation_uri)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(Some(conversation_id))
}

async fn insert_remote_note(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    note: &RemoteNoteData,
    visibility: i32,
    in_reply_to_id: Option<i64>,
    in_reply_to_account_id: Option<i64>,
    conversation_id: Option<i64>,
) -> Result<i64, WriteError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO statuses (
            account_id, text, spoiler_text, visibility, local, language, sensitive, reply,
            uri, url, in_reply_to_id, in_reply_to_account_id, conversation_id,
            quote_approval_policy, created_at, updated_at, edited_at
         ) VALUES ($1, $2, $3, $4, false, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
         RETURNING id",
    )
    .bind(account_id)
    .bind(&note.content)
    .bind(&note.summary)
    .bind(visibility)
    .bind(&note.language)
    .bind(note.sensitive)
    .bind(note.in_reply_to_uri.is_some() || in_reply_to_id.is_some())
    .bind(&note.uri)
    .bind(note.url.as_deref().or(Some(note.uri.as_str())))
    .bind(in_reply_to_id)
    .bind(in_reply_to_account_id)
    .bind(conversation_id)
    .bind(note.quote_approval_policy)
    .bind(note.published_at)
    .bind(note.updated_at)
    .bind(note.edited_at)
    .fetch_one(&mut **transaction)
    .await?)
}

#[allow(clippy::too_many_lines)]
async fn finalize_poll_expiration_generation_in(
    transaction: &mut Transaction<'_, Postgres>,
    poll_id: i64,
    status_id: i64,
    owner_id: i64,
    owner_is_local: bool,
    expires_at: DateTime<Utc>,
    activation: DateTime<Utc>,
) -> Result<(), WriteError> {
    let generation = poll_expiration_generation(expires_at);
    if poll_expiration_effect_in(transaction, poll_id, generation)
        .await?
        .is_some()
    {
        return Ok(());
    }
    if poll_expiration_is_historical(expires_at, activation) {
        record_poll_expiration_effect_in(
            transaction,
            poll_id,
            generation,
            PollExpirationEffectOutcome::HistoricalBaseline,
        )
        .await?;
        return Ok(());
    }
    let now = sqlx::query_scalar::<_, DateTime<Utc>>("SELECT clock_timestamp()")
        .fetch_one(&mut **transaction)
        .await?;
    if expires_at > now {
        let expiration_job = poll_expiration_job(
            poll_id,
            expires_at,
            PollExpirationIntentKind::Reschedule,
            expires_at + ChronoDuration::minutes(5),
        );
        record_outbox_once_in(transaction, &expiration_job).await?;
        return Ok(());
    }
    sqlx::query(
        "WITH recipients AS ( \
            SELECT DISTINCT vote.account_id \
              FROM poll_votes vote \
              JOIN accounts voter ON voter.id = vote.account_id AND voter.domain IS NULL \
             WHERE vote.poll_id = $1 \
            UNION SELECT $2::bigint WHERE $3::boolean) \
         INSERT INTO rustodon.outbox_events (kind, logical_key, payload) \
         SELECT $4, format('notification:poll:%s:%s', account_id, $1), \
                jsonb_build_object( \
                    'lane', 'core', \
                    'arguments', jsonb_build_object( \
                        'recipient_account_id', account_id, \
                        'activity_type', 'poll', \
                        'activity_id', $1, \
                        'silenced', false), \
                    'run_at', $5::text, \
                    'max_attempts', 25) \
           FROM recipients \
         ON CONFLICT (kind, logical_key) WHERE logical_key IS NOT NULL DO NOTHING",
    )
    .bind(poll_id)
    .bind(owner_id)
    .bind(owner_is_local)
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(now.to_rfc3339())
    .execute(&mut **transaction)
    .await?;
    if owner_is_local {
        let final_key = format!("activitypub:poll:{poll_id}:expired");
        let already_recorded = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM rustodon.outbox_events \
             WHERE kind = $1 AND logical_key = $2)",
        )
        .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
        .bind(&final_key)
        .fetch_one(&mut **transaction)
        .await?;
        if !already_recorded {
            let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
                "UPDATE polls SET lock_version = lock_version + 1, \
                 updated_at = clock_timestamp() WHERE id = $1 RETURNING updated_at",
            )
            .bind(poll_id)
            .fetch_one(&mut **transaction)
            .await?;
            let edited_at = status_federation_version(transaction, status_id).await?;
            let update_job = JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                json!({
                    "status_id": status_id,
                    "activity_type": "Update",
                    "update_kind": "poll",
                    "update_version_micros": updated_at.and_utc().timestamp_micros(),
                    "edited_at_micros": edited_at.and_utc().timestamp_micros(),
                    "poll_updated_at_micros": updated_at.and_utc().timestamp_micros()
                }),
            )
            .logical_key(final_key);
            record_outbox_once_in(transaction, &update_job).await?;
        }
    }
    record_poll_expiration_effect_in(
        transaction,
        poll_id,
        generation,
        PollExpirationEffectOutcome::EffectsEnqueued,
    )
    .await?;
    Ok(())
}

async fn reconcile_remote_poll(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    poll: Option<&RemotePollData>,
    allow_significant_changes: bool,
    reject_tally_regression: bool,
    mark_fetched: bool,
) -> Result<RemotePollReconcile, WriteError> {
    if let Some(poll) = poll {
        return upsert_remote_poll(
            transaction,
            status_id,
            account_id,
            poll,
            allow_significant_changes,
            reject_tally_regression,
            mark_fetched,
        )
        .await;
    }
    if !allow_significant_changes {
        return Ok(RemotePollReconcile::Unchanged);
    }
    let poll_ids = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM polls WHERE status_id = $1 ORDER BY id FOR UPDATE",
    )
    .bind(status_id)
    .fetch_all(&mut **transaction)
    .await?;
    if poll_ids.is_empty() {
        return Ok(RemotePollReconcile::Unchanged);
    }
    sqlx::query("UPDATE statuses SET poll_id = NULL WHERE id = $1")
        .bind(status_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM notifications WHERE activity_type = 'Poll' AND activity_id = ANY($1)")
        .bind(&poll_ids)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM poll_votes WHERE poll_id = ANY($1)")
        .bind(&poll_ids)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM polls WHERE id = ANY($1)")
        .bind(&poll_ids)
        .execute(&mut **transaction)
        .await?;
    Ok(RemotePollReconcile::Significant)
}

fn remote_poll_votes_count(poll: &RemotePollData) -> Result<i64, WriteError> {
    poll.tallies
        .iter()
        .try_fold(0_i64, |total, tally| total.checked_add(*tally))
        .ok_or(WriteError::InvalidInput(
            "remote poll vote count is invalid",
        ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemotePollExpirationChange {
    None,
    Reschedule,
    Suppress,
}

fn remote_poll_previous_expiration_is_due_change(
    has_surviving_local_votes: bool,
    incoming: Option<NaiveDateTime>,
    previous: Option<NaiveDateTime>,
    database_now: NaiveDateTime,
) -> bool {
    has_surviving_local_votes
        && incoming != previous
        && previous.is_some_and(|expires_at| expires_at <= database_now)
}

fn remote_poll_expiration_change(
    has_surviving_local_votes: bool,
    incoming: Option<NaiveDateTime>,
    previous: Option<NaiveDateTime>,
    database_now: NaiveDateTime,
) -> RemotePollExpirationChange {
    if !has_surviving_local_votes || incoming == previous {
        return RemotePollExpirationChange::None;
    }
    if previous.is_some_and(|expires_at| expires_at <= database_now) {
        return RemotePollExpirationChange::Suppress;
    }
    if incoming.is_some() {
        RemotePollExpirationChange::Reschedule
    } else {
        RemotePollExpirationChange::None
    }
}

#[allow(clippy::similar_names)]
fn remote_poll_tallies_are_monotonic(
    cached_tallies: &[i64],
    cached_votes_count: i64,
    cached_voters_count: Option<i64>,
    poll: &RemotePollData,
    incoming_votes_count: i64,
) -> bool {
    cached_tallies.len() == poll.tallies.len()
        && cached_tallies
            .iter()
            .zip(&poll.tallies)
            .all(|(cached, incoming)| incoming >= cached)
        && incoming_votes_count >= cached_votes_count
        && match (cached_voters_count, poll.voters_count) {
            (Some(cached), Some(incoming)) => incoming >= cached,
            (Some(_), None) => false,
            _ => true,
        }
}

#[allow(clippy::similar_names, clippy::too_many_lines)]
async fn upsert_remote_poll(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    poll: &RemotePollData,
    allow_significant_changes: bool,
    reject_tally_regression: bool,
    mark_fetched: bool,
) -> Result<RemotePollReconcile, WriteError> {
    let existing = sqlx::query_as::<
        _,
        (
            i64,
            Vec<String>,
            Vec<i64>,
            i64,
            Option<i64>,
            bool,
            Option<NaiveDateTime>,
        ),
    >(
        "SELECT poll.id, poll.options, poll.cached_tallies, poll.votes_count,
                poll.voters_count, poll.multiple, poll.expires_at
           FROM polls poll WHERE poll.status_id = $1 FOR UPDATE",
    )
    .bind(status_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let mut expiration_reschedule = None;
    let mut expiration_suppression = None;
    let mut expiration_activation = None;
    let (poll_id, outcome) = if let Some((
        poll_id,
        options,
        cached_tallies,
        votes_count,
        voters_count,
        multiple,
        previous_expiry,
    )) = existing
    {
        let shape_changed = options != poll.options || multiple != poll.multiple;
        if shape_changed && !allow_significant_changes {
            return Ok(RemotePollReconcile::Unchanged);
        }
        let incoming_votes_count = remote_poll_votes_count(poll)?;
        if reject_tally_regression
            && !shape_changed
            && !remote_poll_tallies_are_monotonic(
                &cached_tallies,
                votes_count,
                voters_count,
                poll,
                incoming_votes_count,
            )
        {
            return Ok(RemotePollReconcile::Unchanged);
        }
        if !shape_changed
            && cached_tallies == poll.tallies
            && votes_count == incoming_votes_count
            && voters_count == poll.voters_count
            && previous_expiry == poll.expires_at
        {
            if mark_fetched {
                // Signed refresh freshness is transport metadata, not a semantic poll version.
                sqlx::query("UPDATE polls SET last_fetched_at = clock_timestamp() WHERE id = $1")
                    .bind(poll_id)
                    .execute(&mut **transaction)
                    .await?;
            }
            return Ok(RemotePollReconcile::Unchanged);
        }
        let expiration_clock =
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut **transaction)
                .await?;
        let has_local_votes = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM poll_votes vote \
                JOIN accounts voter ON voter.id = vote.account_id \
                WHERE vote.poll_id = $1 AND voter.domain IS NULL)",
        )
        .bind(poll_id)
        .fetch_one(&mut **transaction)
        .await?;
        let has_surviving_local_votes = has_local_votes && !shape_changed;
        if remote_poll_previous_expiration_is_due_change(
            has_surviving_local_votes,
            poll.expires_at,
            previous_expiry,
            expiration_clock,
        ) {
            let activation = poll_expiration_activation_in(transaction).await?;
            expiration_activation = Some(activation);
            let previous_expiry = previous_expiry
                .expect("a due remote poll expiration change has a previous expiration")
                .and_utc();
            finalize_poll_expiration_generation_in(
                transaction,
                poll_id,
                status_id,
                account_id,
                false,
                previous_expiry,
                activation,
            )
            .await?;
        }
        match remote_poll_expiration_change(
            has_surviving_local_votes,
            poll.expires_at,
            previous_expiry,
            expiration_clock,
        ) {
            RemotePollExpirationChange::None => {}
            RemotePollExpirationChange::Reschedule => expiration_reschedule = poll.expires_at,
            RemotePollExpirationChange::Suppress => expiration_suppression = poll.expires_at,
        }
        if shape_changed {
            sqlx::query("DELETE FROM poll_votes WHERE poll_id = $1")
                .bind(poll_id)
                .execute(&mut **transaction)
                .await?;
        }
        let reset_tallies = vec![0_i64; poll.options.len()];
        let tallies = if shape_changed {
            reset_tallies.as_slice()
        } else {
            poll.tallies.as_slice()
        };
        let next_votes_count = if shape_changed {
            0
        } else {
            incoming_votes_count
        };
        let next_voters_count = if shape_changed {
            Some(0)
        } else {
            poll.voters_count
        };
        let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
            "UPDATE polls SET options = $2, cached_tallies = $3, votes_count = $4, \
                voters_count = $5, multiple = $6, expires_at = $7, \
                last_fetched_at = CASE WHEN $8 THEN clock_timestamp() ELSE last_fetched_at END, \
                lock_version = lock_version + 1, updated_at = clock_timestamp() WHERE id = $1 \
                RETURNING updated_at",
        )
        .bind(poll_id)
        .bind(&poll.options)
        .bind(tallies)
        .bind(next_votes_count)
        .bind(next_voters_count)
        .bind(poll.multiple)
        .bind(poll.expires_at)
        .bind(mark_fetched)
        .fetch_one(&mut **transaction)
        .await?;
        let outcome = if shape_changed {
            RemotePollReconcile::Significant
        } else {
            RemotePollReconcile::Tally(updated_at)
        };
        (poll_id, outcome)
    } else {
        if !allow_significant_changes {
            return Ok(RemotePollReconcile::Unchanged);
        }
        let poll_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO polls (account_id, status_id, options, cached_tallies, votes_count, \
                voters_count, multiple, hide_totals, expires_at, last_fetched_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, false, $8, \
                CASE WHEN $9 THEN clock_timestamp() END, clock_timestamp(), clock_timestamp()) \
             RETURNING id",
        )
        .bind(account_id)
        .bind(status_id)
        .bind(&poll.options)
        .bind(&poll.tallies)
        .bind(remote_poll_votes_count(poll)?)
        .bind(poll.voters_count)
        .bind(poll.multiple)
        .bind(poll.expires_at)
        .bind(mark_fetched)
        .fetch_one(&mut **transaction)
        .await?;
        (poll_id, RemotePollReconcile::Significant)
    };
    sqlx::query("UPDATE statuses SET poll_id = $2 WHERE id = $1")
        .bind(status_id)
        .bind(poll_id)
        .execute(&mut **transaction)
        .await?;
    if let Some(expires_at) = expiration_suppression {
        let expires_at = expires_at.and_utc();
        let generation = poll_expiration_generation(expires_at);
        let activation = if let Some(activation) = expiration_activation {
            activation
        } else {
            poll_expiration_activation_in(transaction).await?
        };
        let outcome = if poll_expiration_is_historical(expires_at, activation) {
            PollExpirationEffectOutcome::HistoricalBaseline
        } else {
            PollExpirationEffectOutcome::RemotePastExpirySuppressed
        };
        if poll_expiration_effect_in(transaction, poll_id, generation)
            .await?
            .is_none()
        {
            record_poll_expiration_effect_in(transaction, poll_id, generation, outcome).await?;
        }
    }
    if let Some(expires_at) = expiration_reschedule {
        let expires_at = expires_at.and_utc();
        let expiration_job = poll_expiration_job(
            poll_id,
            expires_at,
            PollExpirationIntentKind::Reschedule,
            expires_at + ChronoDuration::minutes(5),
        );
        record_outbox_once_in(transaction, &expiration_job).await?;
    }
    Ok(outcome)
}

async fn insert_remote_note_media(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    attachments: &[RemoteNoteAttachment],
) -> Result<Vec<i64>, WriteError> {
    let mut media_ids = Vec::new();
    for attachment in attachments.iter().take(4) {
        let media = sqlx::query_as::<_, (i64, Option<i32>, Option<String>)>(
            "UPDATE media_attachments SET type = $3, description = $4,
                file_content_type = $6, file_meta = $7::json, blurhash = $8,
                thumbnail_remote_url = $9,
                processing = CASE WHEN file_file_name IS NULL THEN $10 ELSE processing END,
                updated_at = clock_timestamp()
              WHERE account_id = $1 AND status_id = $2 AND remote_url = $5
              RETURNING id, processing, file_file_name",
        )
        .bind(account_id)
        .bind(status_id)
        .bind(remote_media_type(attachment.content_type.as_deref()))
        .bind(&attachment.description)
        .bind(&attachment.remote_url)
        .bind(&attachment.content_type)
        .bind(&attachment.file_meta)
        .bind(&attachment.blurhash)
        .bind(&attachment.thumbnail_remote_url)
        .bind(remote_media_processing(attachment.content_type.as_deref()))
        .fetch_optional(&mut **transaction)
        .await?;
        let (media_id, processing, file_file_name) = if let Some(media) = media {
            media
        } else {
            let media_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO media_attachments (
                account_id, status_id, type, processing, description, remote_url,
                file_content_type, file_meta, blurhash, thumbnail_remote_url,
                created_at, updated_at
             ) VALUES ($1, $2, $3, $10, $4, $5, $6, $7::json, $8, $9,
                       clock_timestamp(), clock_timestamp())
             RETURNING id",
            )
            .bind(account_id)
            .bind(status_id)
            .bind(remote_media_type(attachment.content_type.as_deref()))
            .bind(&attachment.description)
            .bind(&attachment.remote_url)
            .bind(&attachment.content_type)
            .bind(&attachment.file_meta)
            .bind(&attachment.blurhash)
            .bind(&attachment.thumbnail_remote_url)
            .bind(remote_media_processing(attachment.content_type.as_deref()))
            .fetch_one(&mut **transaction)
            .await?;
            (
                media_id,
                Some(remote_media_processing(attachment.content_type.as_deref())),
                None,
            )
        };
        if file_file_name.is_none() && remote_media_is_fetchable(attachment.content_type.as_deref())
        {
            let media_job = JobSpec::new(
                Lane::Pull,
                ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
                json!({"media_id": media_id}),
            )
            .logical_key(remote_media_job_logical_key(
                media_id,
                &attachment.remote_url,
            ))
            .max_attempts(4);
            if processing == Some(3) {
                record_outbox_in(transaction, &media_job).await?;
            } else {
                record_outbox_once_in(transaction, &media_job).await?;
            }
        }
        media_ids.push(media_id);
    }
    Ok(media_ids)
}

fn remote_media_processing(content_type: Option<&str>) -> i32 {
    if remote_media_is_fetchable(content_type) {
        0
    } else {
        2
    }
}

fn remote_media_is_fetchable(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|content_type| {
        matches!(
            content_type.to_ascii_lowercase().as_str(),
            "image/jpeg" | "image/png" | "image/gif" | "image/webp"
        )
    })
}

fn remote_media_job_logical_key(media_id: i64, remote_url: &str) -> String {
    let digest = Sha256::digest(remote_url.as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:media:{media_id}:{digest_string}")
}

async fn remove_remote_note_media_not_in(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    attachments: &[RemoteNoteAttachment],
) -> Result<(), WriteError> {
    let remote_urls = attachments
        .iter()
        .take(4)
        .map(|attachment| attachment.remote_url.clone())
        .collect::<Vec<_>>();
    if remote_urls.is_empty() {
        sqlx::query("DELETE FROM media_attachments WHERE status_id = $1")
            .bind(status_id)
            .execute(&mut **transaction)
            .await?;
    } else {
        sqlx::query(
            "DELETE FROM media_attachments
              WHERE status_id = $1 AND NOT (remote_url = ANY($2))",
        )
        .bind(status_id)
        .bind(remote_urls)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn insert_remote_note_mentions(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    note: &RemoteNoteData,
    delivery_target_account_id: Option<i64>,
    origin: &str,
) -> Result<Vec<(i64, i64)>, WriteError> {
    let mut targets = HashMap::new();
    for uri in &note.mentions {
        targets.insert(uri.as_str(), false);
    }
    for uri in note.audience.to.iter().chain(&note.audience.cc) {
        targets.entry(uri.as_str()).or_insert(true);
    }
    let mut mention_ids = Vec::new();
    for (uri, silent) in targets {
        let Some(account_id) = local_activitypub_account_id(transaction, uri, origin).await? else {
            continue;
        };
        let mention_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO mentions (account_id, status_id, silent, created_at, updated_at)
             VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp())
             ON CONFLICT (account_id, status_id) DO UPDATE SET silent = mentions.silent AND $3
             RETURNING id",
        )
        .bind(account_id)
        .bind(status_id)
        .bind(silent)
        .fetch_one(&mut **transaction)
        .await?;
        if !silent {
            mention_ids.push((mention_id, account_id));
        }
    }
    if let Some(account_id) = delivery_target_account_id {
        sqlx::query(
            "INSERT INTO mentions (account_id, status_id, silent, created_at, updated_at)
             SELECT $1, $2, true, clock_timestamp(), clock_timestamp()
             WHERE EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND domain IS NULL)
             ON CONFLICT (account_id, status_id) DO NOTHING",
        )
        .bind(account_id)
        .bind(status_id)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(mention_ids)
}

async fn ensure_remote_note_delivery_target(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO mentions (account_id, status_id, silent, created_at, updated_at)
         SELECT $1, $2, true, clock_timestamp(), clock_timestamp()
         WHERE EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND domain IS NULL)
         ON CONFLICT (account_id, status_id) DO NOTHING",
    )
    .bind(account_id)
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn update_remote_note_tags(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    hashtags: &[String],
) -> Result<(), WriteError> {
    for hashtag in hashtags {
        let tag_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM tags WHERE lower(name) = lower($1) LIMIT 1",
        )
        .bind(hashtag)
        .fetch_optional(&mut **transaction)
        .await?
        .or(sqlx::query_scalar::<_, i64>(
            "INSERT INTO tags (name, display_name, created_at, updated_at)
                 VALUES ($1, $1, clock_timestamp(), clock_timestamp())
                 ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(hashtag)
        .fetch_optional(&mut **transaction)
        .await?)
        .or(sqlx::query_scalar::<_, i64>(
            "SELECT id FROM tags WHERE lower(name) = lower($1) LIMIT 1",
        )
        .bind(hashtag)
        .fetch_optional(&mut **transaction)
        .await?);
        let Some(tag_id) = tag_id else {
            continue;
        };
        sqlx::query(
            "INSERT INTO statuses_tags (status_id, tag_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(status_id)
        .bind(tag_id)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn insert_remote_note_stats(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    note: &RemoteNoteData,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO status_stats (
            status_id, untrusted_favourites_count, untrusted_reblogs_count,
            created_at, updated_at
         ) VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(status_id)
    .bind(note.favourites_count)
    .bind(note.reblogs_count)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn update_remote_note_stats(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    note: &RemoteNoteData,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE status_stats SET untrusted_favourites_count = COALESCE($2, untrusted_favourites_count),
            untrusted_reblogs_count = COALESCE($3, untrusted_reblogs_count),
            updated_at = clock_timestamp()
          WHERE status_id = $1",
    )
    .bind(status_id)
    .bind(note.favourites_count)
    .bind(note.reblogs_count)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn remote_note_uri(value: Option<&Value>) -> Result<Option<String>, WriteError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = match value {
        Value::String(value) => value.clone(),
        Value::Object(object) => object
            .get("id")
            .or_else(|| object.get("href"))
            .and_then(Value::as_str)
            .ok_or(WriteError::InvalidInput("remote URI object has no ID"))?
            .to_owned(),
        _ => return Err(WriteError::InvalidInput("remote URI is invalid")),
    };
    let url = Url::parse(&value).map_err(|_| WriteError::InvalidInput("remote URI is invalid"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(WriteError::InvalidInput("remote URI is invalid"));
    }
    Ok(Some(value))
}

fn remote_note_optional_uri(value: Option<&Value>) -> Result<Option<String>, WriteError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => remote_note_uri(Some(value)),
    }
}

fn remote_note_optional_conversation_uri(
    value: Option<&Value>,
) -> Result<Option<String>, WriteError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let uri = match value {
        Value::String(value) => value.clone(),
        Value::Object(object) => object
            .get("id")
            .or_else(|| object.get("href"))
            .and_then(Value::as_str)
            .ok_or(WriteError::InvalidInput(
                "remote conversation URI is invalid",
            ))?
            .to_owned(),
        _ => {
            return Err(WriteError::InvalidInput(
                "remote conversation URI is invalid",
            ));
        }
    };
    if let Some(tag_uri) = uri.strip_prefix("tag:") {
        if tag_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote conversation URI is invalid",
            ));
        }
        return Ok(Some(uri));
    }
    remote_note_uri(Some(&Value::String(uri)))
}

fn remote_note_attributed_to(value: Option<&Value>) -> Result<Option<String>, WriteError> {
    match value {
        Some(Value::Array(values)) => remote_note_uri(values.first()),
        _ => remote_note_uri(value),
    }
}

fn remote_note_uri_array(value: Option<&Value>) -> Result<Vec<String>, WriteError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let values = match value {
        Value::Array(values) => values.iter().collect::<Vec<_>>(),
        Value::String(_) | Value::Object(_) => vec![value],
        _ => return Err(WriteError::InvalidInput("remote Note audience is invalid")),
    };
    if values.len() > 100 {
        return Err(WriteError::InvalidInput(
            "remote Note audience is too large",
        ));
    }
    values
        .into_iter()
        .map(|value| {
            if value.as_str().is_some_and(activitypub::is_public_address) {
                return Ok(value.as_str().unwrap_or_default().to_owned());
            }
            remote_note_uri(Some(value)).and_then(|uri| {
                uri.ok_or(WriteError::InvalidInput(
                    "remote Note audience URI is invalid",
                ))
            })
        })
        .collect()
}

fn remote_note_timestamp(
    object: &serde_json::Map<String, Value>,
    field: &str,
    fallback: NaiveDateTime,
) -> Result<NaiveDateTime, WriteError> {
    let Some(value) = object.get(field) else {
        return Ok(fallback);
    };
    let value = value
        .as_str()
        .ok_or(WriteError::InvalidInput("remote Note timestamp is invalid"))?;
    let timestamp = DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.naive_utc())
        .map_err(|_| WriteError::InvalidInput("remote Note timestamp is invalid"))?;
    if timestamp > Utc::now().naive_utc() + ChronoDuration::hours(24) {
        return Err(WriteError::InvalidInput(
            "remote Note timestamp is too far in the future",
        ));
    }
    Ok(timestamp)
}

const MAX_REMOTE_NOTE_COUNT: i64 = 100_000_000;
const MAX_REMOTE_NOTE_COUNT_U64: u64 = 100_000_000;

fn remote_note_count(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<i64>, WriteError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    let count = value
        .as_i64()
        .map(|count| count.clamp(0, MAX_REMOTE_NOTE_COUNT))
        .or_else(|| {
            value
                .as_u64()
                .and_then(|count| i64::try_from(count.min(MAX_REMOTE_NOTE_COUNT_U64)).ok())
        })
        .ok_or(WriteError::InvalidInput("remote Note count is invalid"))?;
    Ok(Some(count))
}

fn remote_note_interaction_count(
    object: &serde_json::Map<String, Value>,
    collection_field: &str,
    legacy_field: &str,
) -> Result<Option<i64>, WriteError> {
    if let Some(collection) = object.get(collection_field) {
        let Some(collection) = collection.as_object() else {
            return Ok(None);
        };
        return remote_note_count(collection, "totalItems");
    }
    remote_note_count(object, legacy_field)
}

fn remote_note_tags(value: Option<&Value>) -> Result<(Vec<String>, Vec<String>), WriteError> {
    let Some(value) = value else {
        return Ok((Vec::new(), Vec::new()));
    };
    let values = match value {
        Value::Array(values) => values.iter().collect::<Vec<_>>(),
        Value::Object(_) | Value::String(_) => vec![value],
        _ => return Err(WriteError::InvalidInput("remote Note tags are invalid")),
    };
    if values.len() > 100 {
        return Err(WriteError::InvalidInput("remote Note tags are too large"));
    }
    let mut mentions = Vec::new();
    let mut hashtags = Vec::new();
    for value in values {
        let Some(object) = value.as_object() else {
            continue;
        };
        match object.get("type").and_then(Value::as_str) {
            Some("Mention") => {
                if let Some(uri) = remote_note_optional_uri(object.get("href"))? {
                    mentions.push(uri);
                }
            }
            Some("Hashtag") => {
                if let Some(name) = object.get("name").and_then(Value::as_str) {
                    let name = name.trim_start_matches('#').trim().to_ascii_lowercase();
                    if !name.is_empty() && name.chars().count() <= 100 && !hashtags.contains(&name)
                    {
                        hashtags.push(name);
                    }
                }
            }
            _ => {}
        }
    }
    Ok((mentions, hashtags))
}

fn remote_note_attachments(value: Option<&Value>) -> Vec<RemoteNoteAttachment> {
    let Some(value) = value else {
        return Vec::new();
    };
    let values = match value {
        Value::Array(values) => values.iter().collect::<Vec<_>>(),
        Value::Object(_) => vec![value],
        _ => return Vec::new(),
    };
    let mut attachments = Vec::new();
    for value in values {
        if attachments.len() == 4 {
            break;
        }
        let Some(object) = value.as_object() else {
            continue;
        };
        let Some(remote_url) = remote_attachment_url(object.get("url")) else {
            continue;
        };
        let thumbnail_remote_url =
            remote_attachment_url(object.get("icon").or_else(|| object.get("preview")));
        let content_type = object
            .get("mediaType")
            .and_then(Value::as_str)
            .or_else(|| remote_attachment_media_type(object.get("url")))
            .filter(|value| value.chars().count() <= 255)
            .map(ToOwned::to_owned);
        let description = object
            .get("summary")
            .and_then(Value::as_str)
            .or_else(|| object.get("name").and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(10_000).collect());
        let blurhash = remote_attachment_blurhash(object.get("blurhash"));
        let mut file_meta = Map::new();
        for field in ["width", "height"] {
            if let Some(value) = object.get(field).and_then(Value::as_i64)
                && value >= 0
            {
                file_meta.insert(field.to_owned(), json!(value));
            }
        }
        if let Some(Value::Array(values)) = object.get("focalPoint")
            && values.len() == 2
            && let (Some(x), Some(y)) = (values[0].as_f64(), values[1].as_f64())
            && x.is_finite()
            && y.is_finite()
        {
            file_meta.insert("focus".to_owned(), json!({"x": x, "y": y}));
        }
        attachments.push(RemoteNoteAttachment {
            remote_url,
            thumbnail_remote_url,
            content_type,
            description,
            blurhash,
            file_meta: Value::Object(file_meta),
        });
    }
    attachments
}

fn remote_attachment_url(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => remote_attachment_uri(value),
        Some(Value::Object(object)) => object
            .get("href")
            .or_else(|| object.get("url"))
            .or_else(|| object.get("id"))
            .and_then(|value| remote_attachment_url(Some(value))),
        Some(Value::Array(values)) => values
            .iter()
            .find_map(|value| remote_attachment_url(Some(value))),
        _ => None,
    }
}

fn remote_attachment_media_type(value: Option<&Value>) -> Option<&str> {
    match value {
        Some(Value::Object(object)) => object.get("mediaType").and_then(Value::as_str),
        Some(Value::Array(values)) => values
            .iter()
            .find_map(|value| remote_attachment_media_type(Some(value))),
        _ => None,
    }
}

fn remote_attachment_uri(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() {
        Some(url.to_string())
    } else {
        None
    }
}

fn remote_attachment_blurhash(value: Option<&Value>) -> Option<String> {
    let value = value.and_then(Value::as_str)?;
    if value.len() > 255 || !value.is_ascii() || value.len() < 6 {
        return None;
    }
    let alphabet =
        b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+-.:;=?@[]^_{|}~";
    let size_flag = alphabet
        .iter()
        .position(|character| *character == value.as_bytes()[0])?;
    let components_x = size_flag % 9 + 1;
    let components_y = size_flag / 9 + 1;
    if components_x > 5
        || components_y > 5
        || value.len() != 4 + 2 * components_x * components_y
        || blurhash::decode(value, 1, 1, 1.0).is_err()
    {
        return None;
    }
    Some(value.to_owned())
}

fn remote_media_type(content_type: Option<&str>) -> i32 {
    match content_type {
        Some(value) if value.eq_ignore_ascii_case("image/gif") => 1,
        Some(value)
            if value
                .split(';')
                .next()
                .is_some_and(|value| value.trim().to_ascii_lowercase().starts_with("video/")) =>
        {
            2
        }
        Some(value)
            if value
                .split(';')
                .next()
                .is_some_and(|value| value.trim().to_ascii_lowercase().starts_with("audio/")) =>
        {
            4
        }
        _ => 0,
    }
}

fn constant_time_string_equal(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(left.get(index).copied().unwrap_or_default())
            ^ usize::from(right.get(index).copied().unwrap_or_default());
    }
    difference == 0
}

fn oauth_grant_pkce_is_valid(
    confidential: bool,
    code_challenge: Option<&str>,
    code_challenge_method: Option<&str>,
) -> bool {
    match (code_challenge, code_challenge_method) {
        (None, None) => confidential,
        (Some(challenge), Some("S256")) => valid_s256_code_challenge(challenge),
        _ => false,
    }
}

fn oauth_pkce_matches(
    confidential: bool,
    code_challenge: Option<&str>,
    code_challenge_method: Option<&str>,
    code_verifier: Option<&str>,
) -> bool {
    match (code_challenge, code_challenge_method) {
        (None, None) => confidential,
        (Some(challenge), Some("S256")) if valid_s256_code_challenge(challenge) => {
            let Some(verifier) = code_verifier.filter(|verifier| valid_pkce_verifier(verifier))
            else {
                return false;
            };
            let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            constant_time_string_equal(challenge, &computed)
        }
        _ => false,
    }
}

fn valid_s256_code_challenge(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_pkce_verifier(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

fn random_urlsafe_base64(byte_count: usize) -> String {
    let mut bytes = vec![0_u8; byte_count];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

async fn generate_backup_codes() -> Result<(Vec<String>, Vec<String>), WriteError> {
    let backup_codes = (0..TWO_FACTOR_BACKUP_CODE_COUNT)
        .map(|_| random_backup_code())
        .collect::<Vec<_>>();
    let encrypted_backup_codes = try_join_all(
        backup_codes
            .iter()
            .cloned()
            .map(|code| tokio::task::spawn_blocking(move || hash(code, DEFAULT_COST))),
    )
    .await
    .map_err(|_| WriteError::Validation("backup codes could not be generated"))?
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .map_err(|_| WriteError::Validation("backup codes could not be generated"))?;
    Ok((backup_codes, encrypted_backup_codes))
}

fn random_uuid() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn password_reset_digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn devise_token_digest(column: &str, token: &str, secret: Option<&str>) -> String {
    let Some(secret) = secret.filter(|secret| !secret.is_empty()) else {
        return password_reset_digest(token);
    };
    let mut key = [0_u8; 64];
    pbkdf2_hmac::<Sha1>(
        secret.as_bytes(),
        format!("Devise {column}").as_bytes(),
        65_536,
        &mut key,
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(&key)
        .expect("HMAC-SHA256 accepts the fixed-size derived key");
    mac.update(token.as_bytes());
    format!("{:x}", mac.finalize().into_bytes())
}

fn normalize_local_email(email: &str) -> Result<String, WriteError> {
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty()
        || email.len() > 320
        || email.chars().any(char::is_whitespace)
        || email.parse::<lettre::Address>().is_err()
    {
        return Err(WriteError::InvalidInput("email must be a valid address"));
    }
    Ok(email)
}

fn normalize_local_username(username: &str) -> Result<String, WriteError> {
    let username = username.trim().to_ascii_lowercase();
    if !(1..=30).contains(&username.len())
        || !username
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || character == b'_')
    {
        return Err(WriteError::InvalidInput(
            "username must contain only letters, numbers, or underscores and be at most 30 bytes",
        ));
    }
    Ok(username)
}

fn validate_local_password(password: &str) -> Result<(), WriteError> {
    if !(8..=72).contains(&password.len()) {
        return Err(WriteError::InvalidInput(
            "password must contain between 8 and 72 bytes",
        ));
    }
    Ok(())
}

fn local_signing_keys() -> Result<(String, String), WriteError> {
    let private_key = RsaPrivateKey::new(&mut OsRng, 2048)
        .map_err(|_| WriteError::Validation("local signing key generation failed"))?;
    let public_key = RsaPublicKey::from(&private_key)
        .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
        .map_err(|_| WriteError::Validation("local signing key serialization failed"))?;
    let private_key = private_key
        .to_pkcs1_pem(rsa::pkcs8::LineEnding::LF)
        .map_err(|_| WriteError::Validation("local signing key serialization failed"))?
        .to_string();
    Ok((private_key, public_key))
}

fn canonical_oauth_scopes(scopes: &str) -> String {
    let mut values = Vec::new();
    for scope in scopes.split_whitespace() {
        if !values.contains(&scope) {
            values.push(scope);
        }
    }
    values.join(" ")
}

fn validate_oauth_application_registration(
    registration: &OAuthApplicationRegistration,
) -> Result<(), ()> {
    if registration.name.trim().is_empty() || registration.name.chars().count() > 60 {
        return Err(());
    }
    if registration.redirect_uri.chars().count() > 2_000 {
        return Err(());
    }
    let redirect_uris = registration
        .redirect_uri
        .split_whitespace()
        .collect::<Vec<_>>();
    if redirect_uris.is_empty()
        || redirect_uris
            .iter()
            .any(|uri| !valid_oauth_redirect_uri(uri))
    {
        return Err(());
    }
    if registration.scopes.trim().is_empty()
        || registration
            .scopes
            .split_whitespace()
            .any(|scope| !OAUTH_CONFIGURED_SCOPES.contains(&scope))
    {
        return Err(());
    }
    if registration
        .website
        .as_deref()
        .is_some_and(|website| !valid_oauth_website(website))
    {
        return Err(());
    }
    Ok(())
}

fn valid_oauth_redirect_uri(value: &str) -> bool {
    if value == OOB_REDIRECT_URI {
        return true;
    }
    let Ok(uri) = url::Url::parse(value) else {
        return false;
    };
    if uri.fragment().is_some() || uri.cannot_be_a_base() {
        return false;
    }
    let scheme = uri.scheme();
    if matches!(scheme, "data" | "vbscript" | "javascript") {
        return false;
    }
    !matches!(scheme, "http" | "https") || uri.host_str().is_some()
}

fn valid_oauth_website(value: &str) -> bool {
    if value.trim().is_empty() {
        return true;
    }
    if value.chars().count() > 2_000 {
        return false;
    }
    let Ok(uri) = url::Url::parse(value) else {
        return false;
    };
    matches!(uri.scheme(), "http" | "https") && uri.host_str().is_some()
}

fn validate_account_profile_update(update: &AccountProfileUpdate) -> Result<(), WriteError> {
    if update
        .display_name
        .as_deref()
        .is_some_and(|value| value.chars().count() > 40)
    {
        return Err(WriteError::Validation("display name is too long"));
    }
    if update
        .note
        .as_deref()
        .is_some_and(|value| value.chars().count() > 500)
    {
        return Err(WriteError::Validation("note is too long"));
    }
    if update
        .avatar_description
        .as_deref()
        .is_some_and(|value| value.chars().count() > 150)
        || update
            .header_description
            .as_deref()
            .is_some_and(|value| value.chars().count() > 150)
    {
        return Err(WriteError::Validation("media description is too long"));
    }
    for media in [&update.avatar, &update.header] {
        let AccountMediaUpdate::Replace {
            file_name,
            content_type,
            file_size,
            storage_schema_version,
        } = media
        else {
            continue;
        };
        if !matches!(
            content_type.as_str(),
            "image/jpeg" | "image/png" | "image/gif" | "image/webp"
        ) {
            return Err(WriteError::Validation("unsupported account image type"));
        }
        if !(1..(8 * 1024 * 1024)).contains(file_size)
            || *storage_schema_version != 1
            || file_name.is_empty()
            || file_name.len() > 255
            || file_name.contains(['/', '\\', '\0'])
            || file_name.chars().any(char::is_control)
        {
            return Err(WriteError::Validation("invalid account image metadata"));
        }
    }
    if let Some(fields) = &update.fields {
        let nonempty = fields
            .iter()
            .filter(|field| !(field.name.trim().is_empty() && field.value.trim().is_empty()))
            .count();
        if nonempty > 4 {
            return Err(WriteError::Validation("too many profile fields"));
        }
        if fields
            .iter()
            .any(|field| field.name.chars().count() > 255 || field.value.chars().count() > 255)
        {
            return Err(WriteError::Validation("profile field is too long"));
        }
        if fields
            .iter()
            .any(|field| field.name.trim().is_empty() && !field.value.trim().is_empty())
        {
            return Err(WriteError::Validation("profile field is missing a name"));
        }
    }
    if let Some(domains) = &update.attribution_domains {
        let domains = normalize_attribution_domains(domains);
        if domains.len() > 100 || domains.iter().any(|domain| !valid_profile_domain(domain)) {
            return Err(WriteError::Validation("invalid attribution domain"));
        }
    }
    if let Some(source) = &update.source {
        if let AccountProfileValue::Value(value) = &source.privacy
            && !matches!(value.as_str(), "public" | "unlisted" | "private")
        {
            return Err(WriteError::Validation("invalid default privacy"));
        }
        if let AccountProfileValue::Value(value) = &source.quote_policy
            && !matches!(value.as_str(), "public" | "followers" | "nobody")
        {
            return Err(WriteError::Validation("invalid default quote policy"));
        }
    }
    Ok(())
}

fn validate_media_attachment_create(create: &MediaAttachmentCreate) -> Result<(), WriteError> {
    let Some(format) = media_format(&create.content_type) else {
        return Err(WriteError::Validation("unsupported media type"));
    };
    if create.media_type != format.kind.database_type() {
        return Err(WriteError::Validation(
            "media type does not match its content type",
        ));
    }
    let Ok(file_size) = usize::try_from(create.file_size) else {
        return Err(WriteError::Validation("invalid media metadata"));
    };
    if file_size == 0
        || file_size >= format.input_size_limit
        || create.file_name.is_empty()
        || create.file_name.len() > 255
        || create.file_name.contains(['/', '\\', '\0'])
        || create.file_name.chars().any(char::is_control)
    {
        return Err(WriteError::Validation("invalid media metadata"));
    }
    if create
        .description
        .as_deref()
        .is_some_and(|value| value.chars().count() > 10_000)
    {
        return Err(WriteError::Validation("media description is too long"));
    }
    validate_media_focus(&create.focus)
}

fn local_media_create_cleanup_job(
    account_id: i64,
    media_id: i64,
    create: &MediaAttachmentCreate,
) -> Result<JobSpec, WriteError> {
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: media_id,
        remote: false,
        storage_schema_version: Some(1),
        file_name: create.file_name.clone(),
        content_type: Some(create.content_type.clone()),
        variant: None,
    };
    let paths = ["original", "small"]
        .into_iter()
        .filter_map(|style| metadata.relative_path(style))
        .collect::<Vec<_>>();
    let expected_paths = if media_format(&create.content_type)
        .and_then(|format| format.preview_content_type)
        .is_some()
    {
        2
    } else {
        1
    };
    if paths.len() != expected_paths {
        return Err(WriteError::Validation("invalid media metadata"));
    }
    Ok(local_media_cleanup_job(
        account_id,
        media_id,
        "rollback_create",
        &paths,
    ))
}

fn local_media_deletion_paths(media: &MediaAttachment) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(file_name) = media.file_file_name.as_ref() {
        let metadata = PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: media.id,
            remote: !rails_blank(&media.remote_url),
            storage_schema_version: media.file_storage_schema_version,
            file_name: file_name.clone(),
            content_type: media.file_content_type.clone(),
            variant: None,
        };
        paths.extend(
            ["original", "small"]
                .into_iter()
                .filter_map(|style| metadata.relative_path(style)),
        );
    }
    if let Some(file_name) = media.thumbnail_file_name.as_ref() {
        let metadata = PaperclipMetadata {
            attachment: PaperclipAttachment::MediaThumbnail,
            id: media.id,
            remote: !media
                .thumbnail_remote_url
                .as_deref()
                .is_none_or(rails_blank),
            storage_schema_version: media.thumbnail_storage_schema_version,
            file_name: file_name.clone(),
            content_type: media.thumbnail_content_type.clone(),
            variant: None,
        };
        paths.extend(metadata.relative_path("original"));
    }
    paths.sort_unstable();
    paths.dedup();
    paths
}

fn local_media_cleanup_job(
    account_id: i64,
    media_id: i64,
    action: &str,
    paths: &[String],
) -> JobSpec {
    JobSpec::new(
        Lane::Maintenance,
        LOCAL_MEDIA_CLEANUP_JOB_KIND,
        json!({
            "account_id": account_id,
            "media_id": media_id,
            "action": action,
            "paths": paths,
        }),
    )
    .logical_key(format!("mastodon:media:{media_id}:{action}"))
}

fn validate_media_attachment_update(update: &MediaAttachmentUpdate) -> Result<(), WriteError> {
    if let AccountProfileValue::Value(value) = &update.description
        && value.chars().count() > 10_000
    {
        return Err(WriteError::Validation("media description is too long"));
    }
    validate_media_focus(&update.focus)
}

fn validate_media_focus(focus: &AccountProfileValue<MediaFocus>) -> Result<(), WriteError> {
    if let AccountProfileValue::Value(focus) = focus
        && (!focus.x.is_finite() || !focus.y.is_finite())
    {
        return Err(WriteError::Validation("invalid media focus"));
    }
    Ok(())
}

async fn validate_status_media(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    media_ids: &[i64],
) -> Result<(), WriteError> {
    if media_ids.is_empty() {
        return Ok(());
    }
    if media_ids.len() > 4 {
        return Err(WriteError::Validation("too many media attachments"));
    }
    let media = sqlx::query_as::<_, (i64, i32, Option<i32>)>(
        "SELECT id, type, processing FROM media_attachments \
         WHERE account_id = $1 AND status_id IS NULL AND id = ANY($2) FOR UPDATE",
    )
    .bind(account_id)
    .bind(media_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let missing = media_ids
        .iter()
        .filter(|id| !media.iter().any(|candidate| candidate.0 == **id))
        .map(i64::to_string)
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(WriteError::Validation("media attachment not found"));
    }
    if media.len() > 1 && media.iter().any(|candidate| candidate.1 != 0) {
        return Err(WriteError::Validation(
            "media attachments must be images or video",
        ));
    }
    if media.iter().any(|candidate| candidate.2 != Some(2)) {
        return Err(WriteError::Validation("media attachment is not ready"));
    }
    Ok(())
}

async fn validate_status_media_update(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    status_id: i64,
    media_ids: &[i64],
) -> Result<(), WriteError> {
    if media_ids.is_empty() {
        return Ok(());
    }
    if media_ids.len() > 4 {
        return Err(WriteError::Validation("too many media attachments"));
    }
    let media = sqlx::query_as::<_, (i64, i32, Option<i32>)>(
        "SELECT id, type, processing FROM media_attachments \
         WHERE account_id = $1 AND (status_id IS NULL OR status_id = $2) \
           AND scheduled_status_id IS NULL AND id = ANY($3) FOR UPDATE",
    )
    .bind(account_id)
    .bind(status_id)
    .bind(media_ids)
    .fetch_all(&mut **transaction)
    .await?;
    if media_ids
        .iter()
        .any(|id| !media.iter().any(|candidate| candidate.0 == *id))
    {
        return Err(WriteError::Validation("media attachment not found"));
    }
    if media.len() > 1 && media.iter().any(|candidate| candidate.1 != 0) {
        return Err(WriteError::Validation(
            "media attachments must be images or video",
        ));
    }
    if media.iter().any(|candidate| candidate.2 != Some(2)) {
        return Err(WriteError::Validation("media attachment is not ready"));
    }
    Ok(())
}

async fn media_descriptions(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    media_ids: &[i64],
) -> Result<Vec<Option<String>>, WriteError> {
    if media_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT id, description FROM media_attachments \
         WHERE status_id = $1 AND id = ANY($2)",
    )
    .bind(status_id)
    .bind(media_ids)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(media_ids
        .iter()
        .map(|id| {
            rows.iter()
                .find(|(candidate_id, _)| candidate_id == id)
                .and_then(|(_, description)| description.clone())
        })
        .collect())
}

async fn update_status_media_attributes(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    status_id: i64,
    media_ids: &[i64],
    attributes: &[StatusMediaAttributeUpdate],
) -> Result<bool, WriteError> {
    let mut changed = false;
    for attribute in attributes {
        if !media_ids.contains(&attribute.id) {
            continue;
        }
        if let AccountProfileValue::Value(description) = &attribute.description
            && description.chars().count() > 10_000
        {
            return Err(WriteError::Validation("media description is too long"));
        }
        validate_media_focus(&attribute.focus)?;
        if matches!(
            (&attribute.description, &attribute.focus),
            (
                AccountProfileValue::Unchanged,
                AccountProfileValue::Unchanged
            )
        ) {
            continue;
        }
        let current = sqlx::query_as::<_, (Option<String>, Option<Value>)>(
            "SELECT description, file_meta FROM media_attachments
              WHERE id = $1 AND account_id = $2
                AND (status_id IS NULL OR status_id = $3)
              FOR UPDATE",
        )
        .bind(attribute.id)
        .bind(account_id)
        .bind(status_id)
        .fetch_optional(&mut **transaction)
        .await?;
        let Some((current_description, current_file_meta)) = current else {
            continue;
        };
        let next_description = match &attribute.description {
            AccountProfileValue::Unchanged => current_description.clone(),
            AccountProfileValue::Null => None,
            AccountProfileValue::Value(description) => Some(description.clone()),
        };
        let next_file_meta = if matches!(attribute.focus, AccountProfileValue::Unchanged) {
            None
        } else {
            Some(media_meta_with_focus(
                current_file_meta.clone().unwrap_or_else(|| json!({})),
                &attribute.focus,
            )?)
        };
        let description_changed = next_description != current_description;
        let file_meta_changed = next_file_meta.as_ref().is_some_and(|next| {
            current_file_meta
                .as_ref()
                .map_or(!next.as_object().is_some_and(Map::is_empty), |current| {
                    current != next
                })
        });
        if !description_changed && !file_meta_changed {
            continue;
        }
        sqlx::query(
            "UPDATE media_attachments SET
               description = $4,
               file_meta = CASE WHEN $5 THEN $6::json ELSE file_meta END,
               updated_at = clock_timestamp()
             WHERE id = $1 AND account_id = $2
               AND (status_id IS NULL OR status_id = $3)",
        )
        .bind(attribute.id)
        .bind(account_id)
        .bind(status_id)
        .bind(next_description)
        .bind(file_meta_changed)
        .bind(next_file_meta)
        .execute(&mut **transaction)
        .await?;
        changed = true;
    }
    Ok(changed)
}

#[allow(clippy::too_many_arguments)]
async fn insert_status_edit(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    text: &str,
    spoiler_text: &str,
    sensitive: bool,
    media_ids: &[i64],
    media_descriptions: &[Option<String>],
    quote_id: Option<i64>,
    created_at: NaiveDateTime,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO status_edits ( \
           account_id, created_at, media_descriptions, ordered_media_attachment_ids, \
           poll_options, quote_id, sensitive, spoiler_text, status_id, text, updated_at) \
         VALUES ($1, $2, $3, $4, (SELECT options FROM polls WHERE status_id = $8 ORDER BY id LIMIT 1), \
                 $5, $6, $7, $8, $9, clock_timestamp())",
    )
    .bind(account_id)
    .bind(created_at)
    .bind(media_descriptions)
    .bind(media_ids)
    .bind(quote_id)
    .bind(sensitive)
    .bind(spoiler_text)
    .bind(status_id)
    .bind(text)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn unique_media_ids(media_ids: &[i64]) -> Vec<i64> {
    let mut unique = Vec::with_capacity(media_ids.len());
    for media_id in media_ids {
        if !unique.contains(media_id) {
            unique.push(*media_id);
        }
    }
    unique
}

fn remote_note_object_is_too_old(
    object: &Value,
    published_at: NaiveDateTime,
    now: NaiveDateTime,
) -> bool {
    object.get("published").and_then(Value::as_str).is_some()
        && published_at < now - ChronoDuration::days(1)
}

const ISO_639_1_STATUS_LANGUAGES: &str = "aa ab ae af ak am an ar as av ay az ba be bg bh bi bm bn bo br bs ca ce ch co cr cs cu cv cy da de dv dz ee el en eo es et eu fa ff fi fj fo fr fy ga gd gl gu gv ha he hi ho hr ht hu hy hz ia id ie ig ii ik io is it iu ja jv ka kg ki kj kk kl km kn ko kr ks ku kv kw ky la lb lg li ln lo lt lu lv mg mh mi mk ml mn mr ms mt my na nb nd ne ng nl nn no nr nv ny oc oj om or os pa pi pl ps pt qu rm rn ro ru rw sa sc sd se sg si sk sl sn so sq sr ss st su sv sw ta te tg th ti tk tl tn to tr ts tt tw ty ug uk ur uz ve vi vo wa wo xh yi yo za zh zu";
const ISO_639_3_STATUS_LANGUAGES: &str = "ast chr ckb cnr csb gsw jbo kab ldn lfn lzz moh nds ota pdc sco sma smj szl tok vai xal xmf zba zgh";
const REGIONAL_STATUS_LANGUAGES: &[&str] = &["zh-CN", "zh-HK", "zh-TW", "zh-YUE", "nan-TW"];

fn normalize_status_language(language: &str) -> Option<String> {
    let language = language.trim();
    if language.is_empty() {
        return None;
    }
    if REGIONAL_STATUS_LANGUAGES.contains(&language) {
        return Some(language.to_owned());
    }
    let base = language.split(['-', '_']).next().unwrap_or_default();
    if ISO_639_1_STATUS_LANGUAGES
        .split_ascii_whitespace()
        .chain(ISO_639_3_STATUS_LANGUAGES.split_ascii_whitespace())
        .any(|code| code == base)
    {
        Some(base.to_owned())
    } else {
        None
    }
}

async fn update_status_tags(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    visibility: i32,
    status_created_at: NaiveDateTime,
    text: &str,
    previous_tag_ids: &[i64],
) -> Result<(), WriteError> {
    let mut current_tag_ids = Vec::new();
    for hashtag in profile_hashtags(text) {
        let tag_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO tags (created_at, display_name, name, updated_at) \
             VALUES (clock_timestamp(), $1, $1, clock_timestamp()) \
             ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(&hashtag)
        .fetch_optional(&mut **transaction)
        .await?
        .or(
            sqlx::query_scalar::<_, i64>("SELECT id FROM tags WHERE lower(name) = lower($1)")
                .bind(&hashtag)
                .fetch_optional(&mut **transaction)
                .await?,
        )
        .ok_or(WriteError::Conflict)?;
        sqlx::query(
            "INSERT INTO statuses_tags (status_id, tag_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(status_id)
        .bind(tag_id)
        .execute(&mut **transaction)
        .await?;
        current_tag_ids.push(tag_id);
    }
    if matches!(visibility, 0 | 1) {
        for tag_id in previous_tag_ids {
            if !current_tag_ids.contains(tag_id) {
                decrement_featured_tag(transaction, account_id, *tag_id).await?;
            }
        }
        for tag_id in &current_tag_ids {
            if !previous_tag_ids.contains(tag_id) {
                sqlx::query(
                    "UPDATE featured_tags SET statuses_count = statuses_count + 1, \
                     last_status_at = $1, updated_at = clock_timestamp() \
                     WHERE account_id = $2 AND tag_id = $3",
                )
                .bind(status_created_at)
                .bind(account_id)
                .bind(tag_id)
                .execute(&mut **transaction)
                .await?;
            }
        }
    }
    Ok(())
}

async fn decrement_featured_tag(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    tag_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE featured_tags featured SET \
           statuses_count = GREATEST(featured.statuses_count - 1, 0), \
           last_status_at = CASE WHEN featured.statuses_count <= 1 THEN NULL ELSE ( \
             SELECT MAX(status.created_at) FROM statuses status \
             JOIN statuses_tags status_tag ON status_tag.status_id = status.id \
             WHERE status.account_id = featured.account_id AND status_tag.tag_id = featured.tag_id \
               AND status.deleted_at IS NULL AND status.visibility IN (0, 1)) END, \
           updated_at = clock_timestamp() \
         WHERE featured.account_id = $1 AND featured.tag_id = $2",
    )
    .bind(account_id)
    .bind(tag_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_status_mentions(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    author_account_id: i64,
    text: &str,
    local_domain: Option<&str>,
) -> Result<Vec<(i64, i64)>, WriteError> {
    let mut targets = Vec::new();
    for (username, domain) in status_mention_candidates(text) {
        // Local accounts store NULL domains, even when the mention spells out LOCAL_DOMAIN.
        let domain = domain
            .as_deref()
            .filter(|domain| !local_domain.is_some_and(|local| domain.eq_ignore_ascii_case(local)));
        let account_id = sqlx::query_scalar::<_, i64>(
            "SELECT account.id FROM accounts account \
             LEFT JOIN users account_user ON account_user.account_id = account.id \
             WHERE account.username = $1 \
               AND account.domain IS NOT DISTINCT FROM $2 \
               AND account.suspended_at IS NULL \
               AND (account.domain IS NULL OR account.protocol = 1) \
               AND (account.domain IS NOT NULL OR (account_user.confirmed_at IS NOT NULL AND account_user.approved)) \
               AND NOT EXISTS (SELECT 1 FROM blocks block \
                               WHERE block.account_id = $3 AND block.target_account_id = account.id) \
               AND NOT EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                               WHERE domain_block.account_id = $3 AND domain_block.domain = account.domain) \
             LIMIT 1",
        )
        .bind(&username)
        .bind(domain)
        .bind(author_account_id)
        .fetch_optional(&mut **transaction)
        .await?;
        let Some(account_id) = account_id else {
            continue;
        };
        if targets.iter().any(|(_, target)| *target == account_id) {
            continue;
        }
        let mention_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO mentions (id, account_id, created_at, silent, status_id, updated_at) \
              VALUES (nextval('mentions_id_seq'), $1, clock_timestamp(), false, $2, clock_timestamp()) \
              ON CONFLICT (account_id, status_id) DO UPDATE SET silent = false, updated_at = clock_timestamp() \
              RETURNING id",
        )
        .bind(account_id)
        .bind(status_id)
        .fetch_one(&mut **transaction)
        .await?;
        targets.push((mention_id, account_id));
    }
    Ok(targets)
}

fn media_meta_with_focus(
    mut file_meta: Value,
    focus: &AccountProfileValue<MediaFocus>,
) -> Result<Value, WriteError> {
    let Value::Object(object) = &mut file_meta else {
        return Err(WriteError::Validation("invalid media metadata"));
    };
    match focus {
        AccountProfileValue::Unchanged => {}
        AccountProfileValue::Null => {
            object.remove("focus");
        }
        AccountProfileValue::Value(focus) => {
            object.insert("focus".to_owned(), json!({"x": focus.x, "y": focus.y}));
        }
    }
    Ok(file_meta)
}

fn account_media_content_type(update: &AccountMediaUpdate) -> Option<&str> {
    match update {
        AccountMediaUpdate::Replace { content_type, .. } => Some(content_type),
        AccountMediaUpdate::Unchanged | AccountMediaUpdate::Remove => None,
    }
}

fn account_media_file_name(update: &AccountMediaUpdate) -> Option<&str> {
    match update {
        AccountMediaUpdate::Replace { file_name, .. } => Some(file_name),
        AccountMediaUpdate::Unchanged | AccountMediaUpdate::Remove => None,
    }
}

fn account_media_file_size(update: &AccountMediaUpdate) -> Option<i32> {
    match update {
        AccountMediaUpdate::Replace { file_size, .. } => Some(*file_size),
        AccountMediaUpdate::Unchanged | AccountMediaUpdate::Remove => None,
    }
}

fn account_media_storage_schema_version(update: &AccountMediaUpdate) -> Option<i32> {
    match update {
        AccountMediaUpdate::Replace {
            storage_schema_version,
            ..
        } => Some(*storage_schema_version),
        AccountMediaUpdate::Unchanged | AccountMediaUpdate::Remove => None,
    }
}

async fn account_fields_json(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    fields: &[AccountFieldUpdate],
) -> Result<Value, WriteError> {
    let existing = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT fields FROM accounts WHERE id = $1 FOR UPDATE",
    )
    .bind(account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)?;
    let existing = existing.as_ref().and_then(Value::as_array);
    let fields = fields
        .iter()
        .filter(|field| !(field.name.trim().is_empty() && field.value.trim().is_empty()))
        .map(|field| {
            let mut value = Map::new();
            value.insert("name".to_owned(), Value::String(field.name.clone()));
            value.insert("value".to_owned(), Value::String(field.value.clone()));
            let verified_at = existing
                .and_then(|existing| {
                    existing.iter().find(|candidate| {
                        candidate
                            .as_object()
                            .and_then(|candidate| candidate.get("value"))
                            .and_then(Value::as_str)
                            == Some(field.value.as_str())
                    })
                })
                .and_then(Value::as_object)
                .and_then(|candidate| candidate.get("verified_at"))
                .cloned();
            if let Some(verified_at) = verified_at {
                value.insert("verified_at".to_owned(), verified_at);
            }
            Value::Object(value)
        })
        .collect::<Vec<_>>();
    Ok(Value::Array(fields))
}

fn normalize_attribution_domains(values: &[String]) -> Vec<String> {
    let mut domains = Vec::new();
    for value in values {
        let value = value.trim();
        let value = value
            .strip_prefix("http://")
            .or_else(|| value.strip_prefix("https://"))
            .unwrap_or(value);
        let value = value.strip_prefix("*.").unwrap_or(value);
        if !value.is_empty() && !domains.iter().any(|domain| domain == value) {
            domains.push(value.to_owned());
        }
    }
    domains
}

fn valid_profile_domain(value: &str) -> bool {
    value.len() < 256
        && value.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|character| character.is_ascii_alphanumeric() || character == b'-')
        })
}

async fn update_user_settings(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    user_id: i64,
    source: &AccountSourceUpdate,
) -> Result<(), WriteError> {
    let Some(settings) = sqlx::query_scalar::<_, Option<String>>(
        "SELECT settings FROM users WHERE id = $1 AND account_id = $2 FOR UPDATE",
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Err(WriteError::NotFound);
    };
    let locked = sqlx::query_scalar::<_, bool>("SELECT locked FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_one(&mut **transaction)
        .await?;
    let mut settings = settings
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<Value>(&value).ok())
        .and_then(|value| match value {
            Value::Object(value) => Some(value),
            _ => None,
        })
        .unwrap_or_default();
    let current_privacy = settings
        .get("default_privacy")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match &source.privacy {
        AccountProfileValue::Value(value) => {
            settings.insert("default_privacy".to_owned(), Value::String(value.clone()));
        }
        AccountProfileValue::Unchanged => {
            settings.insert(
                "default_privacy".to_owned(),
                Value::String(current_privacy.unwrap_or_else(|| {
                    if locked {
                        "private".to_owned()
                    } else {
                        "public".to_owned()
                    }
                })),
            );
        }
        AccountProfileValue::Null => {}
    }
    match source.sensitive {
        AccountProfileValue::Value(value) => {
            settings.insert("default_sensitive".to_owned(), Value::Bool(value));
        }
        AccountProfileValue::Unchanged => {
            settings.insert(
                "default_sensitive".to_owned(),
                Value::Bool(
                    settings
                        .get("default_sensitive")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                ),
            );
        }
        AccountProfileValue::Null => {}
    }
    if let AccountProfileValue::Value(value) = &source.language {
        settings.insert("default_language".to_owned(), Value::String(value.clone()));
    }
    update_default_quote_policy(&mut settings, source);
    let settings = serde_json::to_string(&Value::Object(settings))
        .map_err(|_| WriteError::InvalidInput("invalid account settings"))?;
    sqlx::query("UPDATE users SET settings = $1, updated_at = clock_timestamp() WHERE id = $2")
        .bind(settings)
        .bind(user_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

fn update_default_quote_policy(settings: &mut Map<String, Value>, source: &AccountSourceUpdate) {
    if matches!(&source.privacy, AccountProfileValue::Value(value) if value == "private") {
        settings.insert(
            "default_quote_policy".to_owned(),
            Value::String("nobody".to_owned()),
        );
        return;
    }
    match &source.quote_policy {
        AccountProfileValue::Value(value) => {
            settings.insert(
                "default_quote_policy".to_owned(),
                Value::String(value.clone()),
            );
        }
        AccountProfileValue::Unchanged => {
            settings.insert(
                "default_quote_policy".to_owned(),
                Value::String(
                    settings
                        .get("default_quote_policy")
                        .and_then(Value::as_str)
                        .unwrap_or("public")
                        .to_owned(),
                ),
            );
        }
        AccountProfileValue::Null => {}
    }
}

fn profile_hashtags(note: &str) -> Vec<String> {
    let mut hashtags = Vec::new();
    let characters = note.char_indices().collect::<Vec<_>>();
    for (index, (offset, character)) in characters.iter().enumerate() {
        if !matches!(character, '#' | '＃') || index > 0 && !characters[index - 1].1.is_whitespace()
        {
            continue;
        }
        let mut end = index + 1;
        while end < characters.len()
            && (characters[end].1.is_alphanumeric()
                || matches!(characters[end].1, '_' | '·' | '・' | '\u{200c}'))
        {
            end += 1;
        }
        if end == index + 1 {
            continue;
        }
        let value = &note[*offset + character.len_utf8()
            ..characters[end - 1].0 + characters[end - 1].1.len_utf8()];
        if !value.chars().any(char::is_alphabetic) {
            continue;
        }
        let value = value.to_lowercase();
        if !hashtags.iter().any(|hashtag| hashtag == &value) {
            hashtags.push(value);
        }
    }
    hashtags
}

fn status_mention_candidates(text: &str) -> Vec<(String, Option<String>)> {
    let bytes = text.as_bytes();
    let mut candidates: Vec<(String, Option<String>)> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'@'
            || index > 0
                && (bytes[index - 1].is_ascii_alphanumeric()
                    || matches!(bytes[index - 1], b'_' | b'-'))
        {
            index += 1;
            continue;
        }
        let username_start = index + 1;
        let mut username_end = username_start;
        while username_end < bytes.len()
            && (bytes[username_end].is_ascii_alphanumeric()
                || matches!(bytes[username_end], b'_' | b'-'))
        {
            username_end += 1;
        }
        if username_end == username_start {
            index += 1;
            continue;
        }
        let (domain, end) = if username_end < bytes.len() && bytes[username_end] == b'@' {
            let domain_start = username_end + 1;
            let mut domain_end = domain_start;
            while domain_end < bytes.len()
                && (bytes[domain_end].is_ascii_alphanumeric()
                    || matches!(bytes[domain_end], b'.' | b'-' | b':'))
            {
                domain_end += 1;
            }
            if domain_end == domain_start {
                (None, username_end)
            } else {
                (Some(&text[domain_start..domain_end]), domain_end)
            }
        } else {
            (None, username_end)
        };
        let username = text[username_start..username_end].to_ascii_lowercase();
        if !candidates.iter().any(|(candidate, candidate_domain)| {
            candidate == &username && candidate_domain.as_deref() == domain
        }) {
            candidates.push((username, domain.map(str::to_ascii_lowercase)));
        }
        index = end;
    }
    candidates
}

async fn update_account_tags(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    note: &str,
) -> Result<(), WriteError> {
    let hashtags = profile_hashtags(note);
    let mut tag_ids = Vec::with_capacity(hashtags.len());
    for hashtag in hashtags {
        let tag_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO tags (created_at, display_name, name, updated_at) \
             VALUES (clock_timestamp(), $1, $1, clock_timestamp()) \
             ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(&hashtag)
        .fetch_optional(&mut **transaction)
        .await?
        .or(
            sqlx::query_scalar::<_, i64>("SELECT id FROM tags WHERE lower(name) = lower($1)")
                .bind(&hashtag)
                .fetch_optional(&mut **transaction)
                .await?,
        )
        .ok_or(WriteError::Conflict)?;
        tag_ids.push(tag_id);
        sqlx::query(
            "INSERT INTO accounts_tags (account_id, tag_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(account_id)
        .bind(tag_id)
        .execute(&mut **transaction)
        .await?;
    }
    if tag_ids.is_empty() {
        sqlx::query("DELETE FROM accounts_tags WHERE account_id = $1")
            .bind(account_id)
            .execute(&mut **transaction)
            .await?;
    } else {
        sqlx::query("DELETE FROM accounts_tags WHERE account_id = $1 AND NOT (tag_id = ANY($2))")
            .bind(account_id)
            .bind(&tag_ids)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

async fn relationship_target(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    target_account_id: i64,
) -> Result<(bool, bool, bool, bool), WriteError> {
    sqlx::query_as::<_, (bool, bool, bool, bool)>(
        "SELECT target.domain IS NULL, target.locked, \
                target.suspended_at IS NOT NULL OR target.moved_to_account_id IS NOT NULL, \
                source.silenced_at IS NOT NULL \
         FROM accounts target \
         JOIN accounts source ON source.id = $1 \
         WHERE target.id = $2",
    )
    .bind(source_account_id)
    .bind(target_account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)
}

struct RemoteRelationshipDelivery {
    source_uri: String,
    target_uri: String,
    inbox_url: String,
    domain: String,
}

struct RemoteStatusDelivery {
    source_uri: String,
    target_uri: String,
    target_url: String,
    target_actor_uri: String,
    inbox_url: String,
    preferred_inbox_url: String,
    domain: String,
}

async fn remote_status_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    target_status_id: i64,
    origin: &str,
) -> Result<Option<RemoteStatusDelivery>, WriteError> {
    let delivery = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
        ),
    >(
        "SELECT CASE
                  WHEN source.id = -99 THEN $3 || '/actor'
                  WHEN source.id_scheme = 1 THEN $3 || '/ap/users/' || source.id::text
                  ELSE $3 || '/users/' || source.username
                END,
                  COALESCE(NULLIF(status.uri, ''), NULLIF(status.url, ''), ''),
                  COALESCE(NULLIF(status.url, ''), NULLIF(status.uri, ''), ''),
                  target.uri,
                  COALESCE(NULLIF(target.inbox_url, ''), target.shared_inbox_url),
                  COALESCE(NULLIF(target.shared_inbox_url, ''), target.inbox_url),
                  target.domain
           FROM statuses status
           JOIN accounts target ON target.id = status.account_id
           JOIN accounts source ON source.id = $1
          WHERE status.id = $2 AND status.deleted_at IS NULL
            AND source.domain IS NULL AND target.domain IS NOT NULL
            AND target.protocol = 1",
    )
    .bind(source_account_id)
    .bind(target_status_id)
    .bind(origin.trim_end_matches('/'))
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((
        source_uri,
        target_uri,
        target_web_url,
        target_actor_uri,
        inbox_url,
        preferred_inbox_url,
        Some(domain),
    )) = delivery
    else {
        return Ok(None);
    };
    if source_uri.is_empty()
        || target_uri.is_empty()
        || target_web_url.is_empty()
        || target_actor_uri.is_empty()
        || inbox_url.is_empty()
        || preferred_inbox_url.is_empty()
    {
        return Ok(None);
    }
    Ok(Some(RemoteStatusDelivery {
        source_uri,
        target_uri,
        target_url: target_web_url,
        target_actor_uri,
        inbox_url,
        preferred_inbox_url,
        domain,
    }))
}

#[allow(clippy::too_many_lines)]
async fn record_report_forwarding(
    transaction: &mut Transaction<'_, Postgres>,
    report_id: i64,
    origin: &str,
    forward_to_domains: &[String],
) -> Result<(), WriteError> {
    let Some((
        report_uri,
        target_uri,
        inbox_url,
        shared_inbox_url,
        remote_domain,
        comment,
        status_ids,
    )) = sqlx::query_as::<
        _,
        (
            Option<String>,
            String,
            String,
            String,
            String,
            String,
            Vec<i64>,
        ),
    >(
        "SELECT report.uri, target.uri, \
                    COALESCE(NULLIF(target.inbox_url, ''), target.shared_inbox_url), \
                    target.shared_inbox_url, target.domain, report.comment, \
                    COALESCE(report.status_ids, '{}') \
               FROM reports report \
               JOIN accounts target ON target.id = report.target_account_id \
                WHERE report.id = $1 AND target.domain IS NOT NULL AND target.protocol = 1",
    )
    .bind(report_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(());
    };
    let Some(report_uri) = report_uri.filter(|uri| !uri.is_empty()) else {
        return Ok(());
    };
    if target_uri.is_empty() || inbox_url.is_empty() || remote_domain.is_empty() {
        return Ok(());
    }
    let status_uris = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(NULLIF(status.uri, ''), NULLIF(status.url, '')) \
           FROM statuses status \
          WHERE status.id = ANY($1) \
            AND COALESCE(NULLIF(status.uri, ''), NULLIF(status.url, '')) IS NOT NULL \
          ORDER BY array_position($1, status.id)",
    )
    .bind(&status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let collection_uris = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(NULLIF(collection.uri, ''), NULLIF(collection.url, '')) \
           FROM collection_reports collection_report \
           JOIN collections collection ON collection.id = collection_report.collection_id \
          WHERE collection_report.report_id = $1 \
            AND COALESCE(NULLIF(collection.uri, ''), NULLIF(collection.url, '')) IS NOT NULL \
          ORDER BY collection_report.id",
    )
    .bind(report_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mut object_uris = Vec::with_capacity(1 + status_uris.len() + collection_uris.len());
    object_uris.push(target_uri);
    object_uris.extend(status_uris);
    object_uris.extend(collection_uris);
    let reply_destinations = sqlx::query_as::<_, (String, String)>(
        "SELECT DISTINCT \
                COALESCE(NULLIF(parent.inbox_url, ''), parent.shared_inbox_url), parent.domain \
           FROM statuses child \
           JOIN accounts parent ON parent.id = child.in_reply_to_account_id \
          WHERE child.id = ANY($1) AND parent.domain = ANY($2::text[]) AND parent.protocol = 1 \
            AND COALESCE(NULLIF(parent.inbox_url, ''), parent.shared_inbox_url) <> ALL($3) \
            AND COALESCE(NULLIF(parent.inbox_url, ''), parent.shared_inbox_url) <> ''",
    )
    .bind(&status_ids)
    .bind(forward_to_domains)
    .bind(vec![inbox_url.clone(), shared_inbox_url])
    .fetch_all(&mut **transaction)
    .await?;
    let mut destinations = Vec::new();
    if forward_to_domains
        .iter()
        .any(|domain| domain.eq_ignore_ascii_case(&remote_domain))
    {
        destinations.push((inbox_url, remote_domain));
    }
    destinations.extend(reply_destinations);
    let actor_uri = format!("{}/actor", origin.trim_end_matches('/'));
    let body = activitypub::flag_with_uris(&report_uri, &actor_uri, &object_uris, &comment);
    for (inbox_url, remote_domain) in destinations {
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "source_account_id": -99,
                "inbox_url": inbox_url,
                "remote_domain": remote_domain,
                "body": body.clone()
            }),
        )
        .logical_key(activitypub::flag_delivery_logical_key(
            &report_uri,
            &inbox_url,
        ));
        record_outbox_once_in(transaction, &delivery).await?;
    }
    Ok(())
}

fn local_like_activity_uri(source_uri: &str, activity_id: i64) -> String {
    format!("{source_uri}#likes/{activity_id}")
}

fn local_status_activity_uri(source_uri: &str, status_id: i64) -> String {
    format!("{source_uri}/statuses/{status_id}/activity")
}

fn local_undo_announce_activity_uri(source_uri: &str, status_id: i64) -> String {
    format!("{source_uri}#announces/{status_id}/undo")
}

async fn record_quote_status_update(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<NaiveDateTime, WriteError> {
    let (edited_at, poll_updated_at, quote_updated_at, update_at) = sqlx::query_as::<
        _,
        (
            NaiveDateTime,
            Option<NaiveDateTime>,
            NaiveDateTime,
            NaiveDateTime,
        ),
    >(
        "SELECT COALESCE(status.edited_at, status.updated_at), poll.updated_at, \
                quote.updated_at, clock_timestamp()::timestamp \
           FROM statuses status \
           JOIN accounts account ON account.id = status.account_id \
           JOIN quotes quote ON quote.status_id = status.id \
           LEFT JOIN polls poll ON poll.id = status.poll_id \
          WHERE status.id = $1 AND account.domain IS NULL AND status.deleted_at IS NULL \
          ORDER BY quote.id LIMIT 1 FOR UPDATE OF status, quote",
    )
    .bind(status_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)?;
    let version = update_at.and_utc().timestamp_micros();
    let update = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
        json!({
            "status_id": status_id,
            "activity_type": "Update",
            "update_kind": "quote",
            "update_version_micros": version,
            "edited_at_micros": edited_at.and_utc().timestamp_micros(),
            "poll_updated_at_micros": poll_updated_at.map(|value| value.and_utc().timestamp_micros()),
            "quote_updated_at_micros": quote_updated_at.and_utc().timestamp_micros()
        }),
    )
    .logical_key(format!("activitypub:status:{status_id}:quote:{version}"));
    record_outbox_once_in(transaction, &update).await?;
    Ok(update_at)
}

async fn status_federation_version(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<NaiveDateTime, WriteError> {
    sqlx::query_scalar::<_, NaiveDateTime>(
        "SELECT COALESCE(edited_at, updated_at) FROM statuses \
         WHERE id = $1 AND deleted_at IS NULL FOR SHARE",
    )
    .bind(status_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)
}

async fn record_poll_update_distribution(
    transaction: &mut Transaction<'_, Postgres>,
    poll_id: i64,
    status_id: i64,
    updated_at: NaiveDateTime,
) -> Result<(), WriteError> {
    let base_logical_key = format!("activitypub:poll:{poll_id}:update");
    let pending_outbox = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM rustodon.outbox_events
          WHERE kind = $1 AND dispatched_at IS NULL
            AND (logical_key = $2 OR logical_key LIKE $2 || ':after:%')
          ORDER BY id LIMIT 1 FOR UPDATE",
    )
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(&base_logical_key)
    .fetch_optional(&mut **transaction)
    .await?;
    if pending_outbox.is_some() {
        return Ok(());
    }
    // Lock queued work against dispatcher claiming. A queued worker will read the just-committed
    // tally. A worker that is already leased may have serialized the prior tally, so retain one
    // follow-up keyed to that durable job rather than dropping the racing vote.
    let active_jobs = sqlx::query_as::<_, (i64, bool)>(
        "SELECT id, lease_owner IS NOT NULL AND lease_expires_at > clock_timestamp()
           FROM rustodon.durable_jobs
          WHERE kind = $1 AND dead_at IS NULL
            AND (logical_key = $2 OR logical_key LIKE $2 || ':after:%')
          ORDER BY id FOR SHARE",
    )
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(&base_logical_key)
    .fetch_all(&mut **transaction)
    .await?;
    if active_jobs.iter().any(|(_, leased)| !leased) {
        return Ok(());
    }
    let logical_key = active_jobs.last().map_or_else(
        || base_logical_key.clone(),
        |(job_id, _)| format!("{base_logical_key}:after:{job_id}"),
    );
    let edited_at = status_federation_version(transaction, status_id).await?;
    let update_job = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
        json!({
            "status_id": status_id,
            "activity_type": "Update",
            "update_kind": "poll",
            "update_version_micros": updated_at.and_utc().timestamp_micros(),
            "edited_at_micros": edited_at.and_utc().timestamp_micros(),
            "poll_updated_at_micros": updated_at.and_utc().timestamp_micros()
        }),
    )
    .logical_key(logical_key)
    .run_at(Utc::now() + ChronoDuration::minutes(3));
    record_outbox_in(transaction, &update_job).await?;
    Ok(())
}

async fn record_remote_poll_vote_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    delivery_target: &RemoteStatusDelivery,
    vote_id: i64,
    option: &str,
) -> Result<(), WriteError> {
    let vote_uri = format!("{}#votes/{vote_id}", delivery_target.source_uri);
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "inbox_url": delivery_target.inbox_url,
            "remote_domain": delivery_target.domain,
            "body": activitypub::vote_with_uris(
                &vote_uri,
                &delivery_target.source_uri,
                &delivery_target.target_uri,
                &delivery_target.target_actor_uri,
                option,
            )
        }),
    )
    .logical_key(format!(
        "activitypub:vote:{vote_id}:{}",
        delivery_target.inbox_url
    ));
    record_outbox_once_in(transaction, &delivery).await?;
    Ok(())
}

async fn record_remote_like_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    delivery_target: &RemoteStatusDelivery,
    activity_id: i64,
) -> Result<(), WriteError> {
    let like_uri = local_like_activity_uri(&delivery_target.source_uri, activity_id);
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "inbox_url": delivery_target.inbox_url,
            "remote_domain": delivery_target.domain,
            "body": activitypub::like_with_uris(
                &like_uri,
                &delivery_target.source_uri,
                &delivery_target.target_uri,
            )
        }),
    )
    .logical_key(activitypub::like_delivery_logical_key(
        &like_uri,
        &delivery_target.inbox_url,
    ));
    record_outbox_once_in(transaction, &delivery).await?;
    Ok(())
}

async fn record_remote_undo_like_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    delivery_target: &RemoteStatusDelivery,
    activity_id: i64,
) -> Result<(), WriteError> {
    let like_uri = local_like_activity_uri(&delivery_target.source_uri, activity_id);
    let undo_uri = format!("{like_uri}/undo");
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "inbox_url": delivery_target.inbox_url,
            "remote_domain": delivery_target.domain,
            "body": activitypub::undo_like_with_uris(
                &undo_uri,
                &delivery_target.source_uri,
                &like_uri,
                &delivery_target.target_uri,
            )
        }),
    )
    .logical_key(activitypub::undo_like_delivery_logical_key(
        &like_uri,
        &delivery_target.inbox_url,
    ));
    record_outbox_once_in(transaction, &delivery).await?;
    Ok(())
}

async fn record_remote_announce_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    delivery_target: &RemoteStatusDelivery,
    status_id: i64,
    published_at: NaiveDateTime,
    visibility: i32,
) -> Result<(), WriteError> {
    let announce_uri = local_status_activity_uri(&delivery_target.source_uri, status_id);
    let (to, mut cc) =
        activitypub::local_announce_audience(visibility.into(), &delivery_target.source_uri);
    if let Value::Array(values) = &mut cc {
        values.push(Value::String(delivery_target.target_actor_uri.clone()));
    }
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "inbox_url": delivery_target.preferred_inbox_url,
            "remote_domain": delivery_target.domain,
            "body": activitypub::announce_with_uris(
                &announce_uri,
                &delivery_target.source_uri,
                published_at,
                &delivery_target.target_uri,
                to,
                cc,
            )
        }),
    )
    .logical_key(activitypub::status_delivery_logical_key(
        status_id,
        &delivery_target.preferred_inbox_url,
    ));
    record_outbox_once_in(transaction, &delivery).await?;
    Ok(())
}

async fn record_remote_undo_announce_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    delivery_target: &RemoteStatusDelivery,
    status_id: i64,
    published_at: NaiveDateTime,
    visibility: i32,
) -> Result<(), WriteError> {
    let announce_uri = local_status_activity_uri(&delivery_target.source_uri, status_id);
    let undo_uri = local_undo_announce_activity_uri(&delivery_target.source_uri, status_id);
    let (to, mut cc) =
        activitypub::local_announce_audience(visibility.into(), &delivery_target.source_uri);
    if let Value::Array(values) = &mut cc {
        values.push(Value::String(delivery_target.target_actor_uri.clone()));
    }
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "inbox_url": delivery_target.preferred_inbox_url,
            "remote_domain": delivery_target.domain,
            "body": activitypub::undo_announce_with_uris(
                &undo_uri,
                &delivery_target.source_uri,
                &announce_uri,
                published_at,
                &delivery_target.target_uri,
                to,
                cc,
            )
        }),
    )
    .logical_key(activitypub::status_delete_delivery_logical_key(
        status_id,
        &delivery_target.preferred_inbox_url,
    ));
    record_outbox_once_in(transaction, &delivery).await?;
    Ok(())
}

async fn remote_relationship_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    target_account_id: i64,
    origin: &str,
) -> Result<Option<RemoteRelationshipDelivery>, WriteError> {
    let delivery = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT CASE
                  WHEN source.id = -99 THEN $3 || '/actor'
                  WHEN source.id_scheme = 1 THEN $3 || '/ap/users/' || source.id::text
                  ELSE $3 || '/users/' || source.username
                END,
                target.uri,
                COALESCE(NULLIF(target.inbox_url, ''), target.shared_inbox_url),
                target.domain
           FROM accounts source
           JOIN accounts target ON target.id = $2
          WHERE source.id = $1
            AND target.domain IS NOT NULL
            AND target.protocol = 1",
    )
    .bind(source_account_id)
    .bind(target_account_id)
    .bind(origin.trim_end_matches('/'))
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((source_uri, target_uri, inbox_url, Some(domain))) = delivery else {
        return Ok(None);
    };
    if source_uri.is_empty() || target_uri.is_empty() || inbox_url.is_empty() {
        return Ok(None);
    }
    Ok(Some(RemoteRelationshipDelivery {
        source_uri,
        target_uri,
        inbox_url,
        domain,
    }))
}

fn local_follow_activity_uri(
    origin: &str,
    source_account_id: i64,
    target_account_id: i64,
    activity_id: i64,
    request: bool,
) -> String {
    let kind = if request { "request" } else { "active" };
    let digest = Sha256::digest(
        format!("follow:{kind}:{source_account_id}:{target_account_id}:{activity_id}").as_bytes(),
    );
    format!(
        "{}/payloads/follow-{:x}",
        origin.trim_end_matches('/'),
        digest
    )
}

fn local_undo_follow_activity_uri(origin: &str, follow_uri: &str) -> String {
    let digest = Sha256::digest(format!("undo-follow:{follow_uri}").as_bytes());
    format!(
        "{}/payloads/undo-follow-{:x}",
        origin.trim_end_matches('/'),
        digest
    )
}

fn local_block_activity_uri(
    origin: &str,
    source_account_id: i64,
    target_account_id: i64,
    block_id: i64,
) -> String {
    let digest = Sha256::digest(
        format!("block:{source_account_id}:{target_account_id}:{block_id}").as_bytes(),
    );
    format!(
        "{}/payloads/block-{:x}",
        origin.trim_end_matches('/'),
        digest
    )
}

fn local_undo_block_activity_uri(origin: &str, block_uri: &str) -> String {
    let digest = Sha256::digest(format!("undo-block:{block_uri}").as_bytes());
    format!(
        "{}/payloads/undo-block-{:x}",
        origin.trim_end_matches('/'),
        digest
    )
}

async fn record_remote_follow_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    delivery_target: &RemoteRelationshipDelivery,
    follow_uri: &str,
) -> Result<(), WriteError> {
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "inbox_url": delivery_target.inbox_url,
            "remote_domain": delivery_target.domain,
            "body": activitypub::follow_with_uris(
                follow_uri,
                &delivery_target.source_uri,
                &delivery_target.target_uri,
            )
        }),
    )
    .logical_key(activitypub::follow_delivery_logical_key(
        follow_uri,
        &delivery_target.inbox_url,
    ));
    record_outbox_once_in(transaction, &delivery).await?;
    Ok(())
}

async fn record_remote_undo_follow_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    delivery_target: &RemoteRelationshipDelivery,
    follow_uri: &str,
    origin: &str,
) -> Result<(), WriteError> {
    let undo_uri = local_undo_follow_activity_uri(origin, follow_uri);
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "inbox_url": delivery_target.inbox_url,
            "remote_domain": delivery_target.domain,
            "body": activitypub::undo_follow_with_uris(
                &undo_uri,
                &delivery_target.source_uri,
                follow_uri,
                &delivery_target.target_uri,
            )
        }),
    )
    .logical_key(activitypub::undo_follow_delivery_logical_key(
        follow_uri,
        &delivery_target.inbox_url,
    ));
    record_outbox_once_in(transaction, &delivery).await?;
    Ok(())
}

async fn record_remote_block_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    delivery_target: &RemoteRelationshipDelivery,
    block_uri: &str,
) -> Result<(), WriteError> {
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "inbox_url": delivery_target.inbox_url,
            "remote_domain": delivery_target.domain,
            "body": activitypub::block_with_uris(
                block_uri,
                &delivery_target.source_uri,
                &delivery_target.target_uri,
            )
        }),
    )
    .logical_key(activitypub::block_delivery_logical_key(
        block_uri,
        &delivery_target.inbox_url,
    ));
    record_outbox_once_in(transaction, &delivery).await?;
    Ok(())
}

async fn record_remote_undo_block_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    delivery_target: &RemoteRelationshipDelivery,
    block_uri: &str,
    origin: &str,
) -> Result<(), WriteError> {
    let undo_uri = local_undo_block_activity_uri(origin, block_uri);
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "inbox_url": delivery_target.inbox_url,
            "remote_domain": delivery_target.domain,
            "body": activitypub::undo_block_with_uris(
                &undo_uri,
                &delivery_target.source_uri,
                block_uri,
                &delivery_target.target_uri,
            )
        }),
    )
    .logical_key(activitypub::undo_block_delivery_logical_key(
        block_uri,
        &delivery_target.inbox_url,
    ));
    record_outbox_once_in(transaction, &delivery).await?;
    Ok(())
}

async fn record_remote_reject_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    delivery_target: &RemoteRelationshipDelivery,
    follow_id: i64,
    follow_uri: &str,
) -> Result<(), WriteError> {
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "inbox_url": delivery_target.inbox_url,
            "remote_domain": delivery_target.domain,
            "body": activitypub::reject_with_uris(
                &delivery_target.source_uri,
                Some(follow_id),
                follow_uri,
                &delivery_target.target_uri,
            )
        }),
    )
    .logical_key(activitypub::reject_delivery_logical_key(
        source_account_id,
        follow_uri,
        &delivery_target.inbox_url,
    ));
    record_outbox_once_in(transaction, &delivery).await?;
    Ok(())
}

fn notification_job(recipient_account_id: i64, activity_type: &str, activity_id: i64) -> JobSpec {
    notification_job_with_silenced(recipient_account_id, activity_type, activity_id, false)
}

fn notification_unfilter_job(account_id: i64, from_account_id: i64) -> JobSpec {
    JobSpec::new(
        Lane::Core,
        NOTIFICATION_UNFILTER_JOB_KIND,
        json!({
            "account_id": account_id,
            "from_account_id": from_account_id,
        }),
    )
    .logical_key(format!(
        "notification:unfilter:{account_id}:{from_account_id}"
    ))
}

fn notification_cleanup_job(account_id: i64, from_account_id: i64, request_id: i64) -> JobSpec {
    JobSpec::new(
        Lane::Core,
        NOTIFICATION_CLEANUP_JOB_KIND,
        json!({
            "account_id": account_id,
            "from_account_id": from_account_id,
        }),
    )
    .logical_key(format!(
        "notification:cleanup:{account_id}:{from_account_id}:{request_id}"
    ))
}

async fn record_status_update_notifications(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    version_micros: i64,
) -> Result<(), WriteError> {
    let update_recipients = sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT reblog.account_id \
           FROM statuses reblog \
           JOIN accounts recipient ON recipient.id = reblog.account_id \
            AND recipient.domain IS NULL AND recipient.suspended_at IS NULL \
           JOIN users recipient_user ON recipient_user.account_id = recipient.id \
          WHERE reblog.reblog_of_id = $1 AND reblog.deleted_at IS NULL",
    )
    .bind(status_id)
    .fetch_all(&mut **transaction)
    .await?;
    for recipient_account_id in update_recipients {
        record_outbox_in(
            transaction,
            &notification_job(recipient_account_id, NOTIFICATION_UPDATE, status_id).logical_key(
                format!(
                    "notification:{NOTIFICATION_UPDATE}:{recipient_account_id}:{status_id}:{version_micros}"
                ),
            ),
        )
        .await?;
    }
    let quoted_update_recipients = sqlx::query_as::<_, (i64, i64)>(
        "SELECT DISTINCT quote.account_id, quote.status_id \
           FROM quotes quote \
           JOIN statuses quote_status ON quote_status.id = quote.status_id \
            AND quote_status.deleted_at IS NULL \
           JOIN accounts recipient ON recipient.id = quote.account_id \
            AND recipient.domain IS NULL AND recipient.suspended_at IS NULL \
           JOIN users recipient_user ON recipient_user.account_id = recipient.id \
          WHERE quote.quoted_status_id = $1 AND quote.state = 1",
    )
    .bind(status_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (recipient_account_id, quote_status_id) in quoted_update_recipients {
        record_outbox_in(
            transaction,
            &notification_job(
                recipient_account_id,
                NOTIFICATION_QUOTED_UPDATE,
                quote_status_id,
            )
            .logical_key(format!(
                "notification:{NOTIFICATION_QUOTED_UPDATE}:{recipient_account_id}:{quote_status_id}:{version_micros}"
            )),
        )
        .await?;
    }
    Ok(())
}

fn collect_account_kill_stream_event(
    pending: &mut Vec<PendingStreamEvent>,
    account_id: i64,
    updated_at: NaiveDateTime,
) -> Result<(), WriteError> {
    let logical_key = event_logical_key(
        account_id,
        SYSTEM_KILL_EVENT,
        account_id,
        updated_at.and_utc().timestamp_micros(),
    );
    pending.push(pending_stream_event(
        account_id,
        SYSTEM_KILL_EVENT,
        account_id,
        &logical_key,
        None,
        None,
    )?);
    Ok(())
}

async fn record_token_kill_stream_event(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    token_id: i64,
) -> Result<(), sqlx::Error> {
    let logical_key = event_logical_key(account_id, TOKEN_KILL_EVENT, token_id, 0);
    record_stream_event_in(
        transaction,
        account_id,
        TOKEN_KILL_EVENT,
        token_id,
        &logical_key,
    )
    .await
    .map(|_| ())
    .map_err(|error| match error {
        JobError::Sqlx(error) => error,
        other => sqlx::Error::Protocol(other.to_string()),
    })
}

// The password hash is the credential version: bcrypt resets generate a fresh salt even
// when the plaintext is reused. Recovery and all fenced writes serialize on this row.
async fn lock_verified_password_in(
    transaction: &mut Transaction<'_, Postgres>,
    authentication: &VerifiedPassword,
) -> Result<i64, WriteError> {
    sqlx::query_scalar(
        "SELECT account_id FROM users WHERE id = $1 AND encrypted_password = $2 FOR UPDATE",
    )
    .bind(authentication.user_id)
    .bind(authentication.encrypted_password.as_str())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::Unauthorized)
}

async fn replace_user_password_in(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: i64,
    account_id: i64,
    password: &str,
) -> Result<(), WriteError> {
    let encrypted_password =
        hash(password, DEFAULT_COST).map_err(|_| WriteError::Validation("invalid password"))?;
    sqlx::query(
        "UPDATE users SET encrypted_password = $1, reset_password_token = NULL, \
                    reset_password_sent_at = NULL, sign_in_token = NULL, \
                    sign_in_token_sent_at = NULL, updated_at = clock_timestamp() \
             WHERE id = $2",
    )
    .bind(encrypted_password)
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM session_activations WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "DELETE FROM web_push_subscriptions subscription \
             USING oauth_access_tokens access_token \
             WHERE subscription.access_token_id = access_token.id \
               AND access_token.resource_owner_id = $1",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    revoke_user_access_tokens_in(transaction, user_id, account_id).await?;
    sqlx::query(
        "UPDATE oauth_access_grants SET revoked_at = COALESCE(revoked_at, clock_timestamp()) \
             WHERE resource_owner_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn revoke_user_access_tokens_in(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: i64,
    account_id: i64,
) -> Result<(), WriteError> {
    let token_ids = sqlx::query_scalar::<_, i64>(
        "UPDATE oauth_access_tokens SET revoked_at = COALESCE(revoked_at, clock_timestamp()) \
          WHERE resource_owner_id = $1 AND revoked_at IS NULL \
          RETURNING id",
    )
    .bind(user_id)
    .fetch_all(&mut **transaction)
    .await?;
    for token_id in token_ids {
        record_token_kill_stream_event(transaction, account_id, token_id).await?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum StreamEventLogicalKey {
    Version(i64),
    Media(i64),
}

impl StreamEventLogicalKey {
    fn for_recipient(self, account_id: i64, event: &str, object_id: i64) -> String {
        match self {
            Self::Version(version) => event_logical_key(account_id, event, object_id, version),
            Self::Media(media_id) => {
                media_event_logical_key(account_id, event, object_id, media_id)
            }
        }
    }

    fn for_global(self, event: &str, object_id: i64) -> String {
        match self {
            Self::Version(version) => global_event_logical_key(event, object_id, version),
            Self::Media(media_id) => {
                format!("stream:global:{event}:{object_id}:media:{media_id}")
            }
        }
    }
}

async fn account_timeline_status_ids(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<Vec<i64>, WriteError> {
    timeline_status_ids_for_accounts(transaction, &[account_id]).await
}

async fn timeline_status_ids_for_accounts(
    transaction: &mut Transaction<'_, Postgres>,
    account_ids: &[i64],
) -> Result<Vec<i64>, WriteError> {
    if account_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE affected(id) AS ( \
           SELECT id FROM statuses WHERE account_id = ANY($1::bigint[]) AND deleted_at IS NULL \
           UNION \
           SELECT child.id FROM statuses child JOIN affected parent \
             ON parent.id = child.reblog_of_id WHERE child.deleted_at IS NULL \
         ) SELECT id FROM affected ORDER BY id",
    )
    .bind(account_ids)
    .fetch_all(&mut **transaction)
    .await?)
}

async fn collect_timeline_snapshot_transitions(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    event: &str,
    context: &str,
    version: i64,
    before: &HashMap<i64, TimelineRouteSnapshot>,
    after: &HashMap<i64, TimelineRouteSnapshot>,
) -> Result<(), WriteError> {
    let mut status_ids = before
        .keys()
        .chain(after.keys())
        .copied()
        .collect::<Vec<_>>();
    status_ids.sort_unstable();
    status_ids.dedup();
    for status_id in status_ids {
        let before = before.get(&status_id);
        let after = after.get(&status_id);
        if before == after {
            continue;
        }
        let key = format!("stream:global:{event}:{status_id}:{context}:{version}");
        pending.push(pending_stream_event(
            0, event, status_id, &key, before, after,
        )?);
        stage_stream_events_if_large_in(transaction, pending).await?;
    }
    Ok(())
}

async fn collect_account_timeline_transition(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    account_id: i64,
    event: &str,
    version: i64,
) -> Result<(), WriteError> {
    let status_ids = account_timeline_status_ids(transaction, account_id).await?;
    let mut snapshots = status_timeline_snapshots(transaction, &status_ids).await?;
    for status_id in status_ids {
        let Some(snapshot) = snapshots.remove(&status_id) else {
            continue;
        };
        let (before, after) = if event == "delete" {
            (Some(&snapshot), None)
        } else {
            (None, Some(&snapshot))
        };
        let key =
            format!("stream:global:{event}:{status_id}:account:{account_id}:lifecycle:{version}");
        pending.push(pending_stream_event(
            0, event, status_id, &key, before, after,
        )?);
        stage_stream_events_if_large_in(transaction, pending).await?;
        collect_status_lifecycle_recipient_stream_events(
            transaction,
            pending,
            status_id,
            event,
            StreamEventLogicalKey::Version(version),
        )
        .await?;
    }
    Ok(())
}

async fn status_timeline_snapshot(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<TimelineRouteSnapshot, WriteError> {
    status_timeline_snapshots(transaction, &[status_id])
        .await?
        .remove(&status_id)
        .ok_or(WriteError::NotFound)
}

#[allow(clippy::too_many_lines)]
async fn status_timeline_snapshots(
    transaction: &mut Transaction<'_, Postgres>,
    status_ids: &[i64],
) -> Result<HashMap<i64, TimelineRouteSnapshot>, WriteError> {
    let mut snapshots = HashMap::new();
    for chunk in status_ids.chunks(128) {
        let rows = sqlx::query_as::<_, (i64, bool, Option<String>, bool, bool, bool)>(
            "SELECT status.id, \
                    EXISTS (SELECT 1 FROM media_attachments media WHERE media.status_id = status.id), \
                    status.language, \
                    status.visibility = 0 AND author.suspended_at IS NULL \
                      AND author.silenced_at IS NULL AND status.reblog_of_id IS NULL \
                      AND (NOT status.reply OR status.in_reply_to_account_id = status.account_id) IS TRUE, \
                    status.visibility = 0 AND author.suspended_at IS NULL \
                      AND author.silenced_at IS NULL, \
                    status.local OR status.uri IS NULL \
               FROM statuses status JOIN accounts author ON author.id = status.account_id \
              WHERE status.id = ANY($1::bigint[]) ORDER BY status.id",
        )
        .bind(chunk)
        .fetch_all(&mut **transaction)
        .await?;
        for (status_id, had_media, language, public, hashtag, local) in rows {
            snapshots.insert(
                status_id,
                TimelineRouteSnapshot {
                    public,
                    hashtag,
                    local,
                    had_media,
                    language,
                    tags: Vec::new(),
                    lists: Vec::new(),
                },
            );
        }
        let tags = sqlx::query_as::<_, (i64, String)>(
            "SELECT status_tag.status_id, tag.name FROM statuses_tags status_tag \
             JOIN tags tag ON tag.id = status_tag.tag_id \
             WHERE status_tag.status_id = ANY($1::bigint[]) \
             ORDER BY status_tag.status_id, lower(tag.name), tag.id",
        )
        .bind(chunk)
        .fetch_all(&mut **transaction)
        .await?;
        for (status_id, tag) in tags {
            if let Some(snapshot) = snapshots.get_mut(&status_id) {
                snapshot.tags.push(normalize_hashtag(&tag));
            }
        }
        let lists = sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT DISTINCT status.id, list.account_id, list.id \
               FROM statuses status \
               JOIN accounts author ON author.id = status.account_id \
               JOIN list_accounts member ON member.account_id = status.account_id \
               JOIN lists list ON list.id = member.list_id \
                 AND (member.follow_id IS NOT NULL OR status.account_id = list.account_id) \
               LEFT JOIN follows member_follow ON member_follow.id = member.follow_id \
               LEFT JOIN statuses source ON source.id = status.reblog_of_id \
                 AND source.deleted_at IS NULL \
               LEFT JOIN accounts source_author ON source_author.id = source.account_id \
               LEFT JOIN accounts viewer ON viewer.id = list.account_id \
              WHERE status.id = ANY($1::bigint[]) \
                AND author.suspended_at IS NULL \
                AND (source.id IS NULL OR source_author.suspended_at IS NULL) \
                AND status.visibility IN (0, 1, 2) \
                AND CASE WHEN status.account_id = list.account_id THEN true \
                  WHEN status.visibility = 2 THEN member_follow.id IS NOT NULL \
                  WHEN status.visibility IN (0, 1) THEN NOT EXISTS ( \
                    SELECT 1 FROM blocks author_block \
                     WHERE author_block.account_id = status.account_id \
                       AND author_block.target_account_id = list.account_id) \
                    AND (viewer.domain IS NULL OR NOT EXISTS ( \
                      SELECT 1 FROM account_domain_blocks domain_block \
                       WHERE domain_block.account_id = status.account_id \
                         AND domain_block.domain = viewer.domain)) \
                  ELSE false END \
                AND (COALESCE(cardinality(member_follow.languages), 0) = 0 \
                  OR status.language IS NULL OR status.language = ANY(member_follow.languages)) \
                AND (NOT status.reply OR (status.in_reply_to_id IS NOT NULL \
                  AND status.in_reply_to_account_id IS NOT NULL)) \
                AND (NOT status.reply OR status.in_reply_to_account_id = status.account_id \
                  OR status.in_reply_to_account_id = list.account_id \
                  OR (list.replies_policy = 0 AND EXISTS ( \
                    SELECT 1 FROM list_accounts reply_member \
                     WHERE reply_member.list_id = list.id \
                       AND reply_member.account_id = status.in_reply_to_account_id)) \
                  OR (list.replies_policy = 1 AND EXISTS ( \
                    SELECT 1 FROM follows reply_follow \
                     WHERE reply_follow.account_id = list.account_id \
                       AND reply_follow.target_account_id = status.in_reply_to_account_id))) \
                AND (status.account_id = list.account_id OR status.reblog_of_id IS NULL OR ( \
                  source.account_id IS NOT NULL AND member_follow.show_reblogs)) \
                AND (status.account_id = list.account_id OR NOT EXISTS ( \
                  SELECT 1 FROM blocks viewer_block \
                   WHERE viewer_block.account_id = list.account_id \
                     AND viewer_block.target_account_id = status.account_id)) \
                AND (status.account_id = list.account_id OR NOT EXISTS ( \
                  SELECT 1 FROM blocks author_block \
                   WHERE author_block.account_id = status.account_id \
                     AND author_block.target_account_id = list.account_id)) \
                AND (status.account_id = list.account_id OR NOT EXISTS ( \
                  SELECT 1 FROM mutes viewer_mute \
                   WHERE viewer_mute.account_id = list.account_id \
                     AND viewer_mute.target_account_id = status.account_id)) \
                AND (status.account_id = list.account_id OR NOT EXISTS ( \
                  SELECT 1 FROM mentions mention \
                   WHERE mention.status_id IN (status.id, status.reblog_of_id) \
                     AND NOT mention.silent AND (EXISTS ( \
                       SELECT 1 FROM blocks mention_block \
                        WHERE mention_block.account_id = list.account_id \
                          AND mention_block.target_account_id = mention.account_id) \
                     OR EXISTS (SELECT 1 FROM mutes mention_mute \
                        WHERE mention_mute.account_id = list.account_id \
                          AND mention_mute.target_account_id = mention.account_id)))) \
                AND (status.account_id = list.account_id OR source.account_id IS NULL OR ( \
                  NOT EXISTS (SELECT 1 FROM blocks source_block \
                    WHERE source_block.account_id = list.account_id \
                      AND source_block.target_account_id = source.account_id) \
                  AND NOT EXISTS (SELECT 1 FROM mutes source_mute \
                    WHERE source_mute.account_id = list.account_id \
                      AND source_mute.target_account_id = source.account_id) \
                  AND NOT EXISTS (SELECT 1 FROM blocks source_author_block \
                    WHERE source_author_block.account_id = source.account_id \
                      AND source_author_block.target_account_id = list.account_id) \
                  AND (source_author.domain IS NULL OR NOT EXISTS ( \
                    SELECT 1 FROM account_domain_blocks source_domain_block \
                     WHERE source_domain_block.account_id = list.account_id \
                       AND source_domain_block.domain = source_author.domain)))) \
                AND (status.account_id = list.account_id OR author.domain IS NULL OR NOT EXISTS ( \
                  SELECT 1 FROM account_domain_blocks domain_block \
                   WHERE domain_block.account_id = list.account_id \
                     AND domain_block.domain = author.domain)) \
              ORDER BY status.id, list.account_id, list.id",
        )
        .bind(chunk)
        .fetch_all(&mut **transaction)
        .await?;
        for (status_id, account_id, list_id) in lists {
            if let Some(snapshot) = snapshots.get_mut(&status_id) {
                snapshot.lists.push(TimelineListRoute {
                    account_id,
                    list_id,
                });
            }
        }
    }
    for snapshot in snapshots.values_mut() {
        snapshot.tags.sort();
        snapshot.tags.dedup();
        snapshot.lists.dedup();
    }
    Ok(snapshots)
}

async fn collect_status_stream_transition(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    status_id: i64,
    event: &str,
    key: StreamEventLogicalKey,
    before: Option<TimelineRouteSnapshot>,
    after: Option<TimelineRouteSnapshot>,
) -> Result<(), WriteError> {
    let global_key = key.for_global(event, status_id);
    pending.push(pending_stream_event(
        0,
        event,
        status_id,
        &global_key,
        before.as_ref(),
        after.as_ref(),
    )?);
    stage_stream_events_if_large_in(transaction, pending).await?;
    collect_status_recipient_stream_events(transaction, pending, status_id, event, key).await
}

async fn collect_status_stream_events(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    status_id: i64,
    event: &str,
    version: i64,
) -> Result<(), WriteError> {
    let snapshot = status_timeline_snapshot(transaction, status_id).await?;
    let (before, after) = match event {
        "update" => (None, Some(snapshot)),
        "delete" => (Some(snapshot), None),
        _ => (Some(snapshot.clone()), Some(snapshot)),
    };
    collect_status_stream_transition(
        transaction,
        pending,
        status_id,
        event,
        StreamEventLogicalKey::Version(version),
        before,
        after,
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn collect_status_stream_events_with_key(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    status_id: i64,
    event: &str,
    key: StreamEventLogicalKey,
) -> Result<(), WriteError> {
    let snapshot = status_timeline_snapshot(transaction, status_id).await?;
    let (before, after) = match event {
        "update" => (None, Some(snapshot)),
        "delete" => (Some(snapshot), None),
        _ => (Some(snapshot.clone()), Some(snapshot)),
    };
    collect_status_stream_transition(transaction, pending, status_id, event, key, before, after)
        .await
}

#[allow(clippy::too_many_lines)]
async fn collect_status_lifecycle_recipient_stream_events(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    status_id: i64,
    event: &str,
    key: StreamEventLogicalKey,
) -> Result<(), WriteError> {
    collect_status_recipient_stream_events(transaction, pending, status_id, event, key).await?;
    let recipients = sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT mention.account_id
           FROM mentions mention
           JOIN accounts recipient ON recipient.id = mention.account_id
            AND recipient.domain IS NULL AND recipient.suspended_at IS NULL
           JOIN users recipient_user ON recipient_user.account_id = recipient.id
            AND recipient_user.disabled IS FALSE
          WHERE mention.status_id = $1 AND mention.silent IS FALSE
          ORDER BY mention.account_id",
    )
    .bind(status_id)
    .fetch_all(&mut **transaction)
    .await?;
    for account_id in recipients {
        let logical_key = key.for_recipient(account_id, event, status_id);
        pending.push(pending_stream_event(
            account_id,
            event,
            status_id,
            &logical_key,
            None,
            None,
        )?);
        stage_stream_events_if_large_in(transaction, pending).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn collect_status_recipient_stream_events(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    status_id: i64,
    event: &str,
    key: StreamEventLogicalKey,
) -> Result<(), WriteError> {
    let deleting = event == "delete";
    // The tag-follow UNION mirrors rest_home_timeline_ids' hashtag branch, including
    // its policy exclusions. Delete callers may have already tombstoned the status, but
    // retain its tags; only that tombstone check is relaxed for hashtag recipients.
    let recipients = sqlx::query_scalar::<_, i64>(
        "WITH recipients AS ( \
           SELECT status.account_id \
             FROM statuses status \
             JOIN accounts recipient ON recipient.id = status.account_id \
              AND recipient.domain IS NULL AND recipient.suspended_at IS NULL \
             JOIN users recipient_user ON recipient_user.account_id = recipient.id \
              AND recipient_user.disabled IS FALSE \
            WHERE status.id = $1 \
           UNION \
             SELECT follow.account_id \
              FROM statuses status \
              JOIN accounts author ON author.id = status.account_id \
              JOIN follows follow ON follow.target_account_id = status.account_id \
               AND (($2 AND status.visibility IN (0, 1, 2)) OR status.reblog_of_id IS NULL OR follow.show_reblogs IS TRUE) \
                  AND (status.visibility IN (0, 1, 2) OR ( \
                  status.visibility IN (3, 4) AND EXISTS ( \
                    SELECT 1 FROM mentions mention \
                     WHERE mention.status_id = status.id \
                      AND mention.account_id = follow.account_id))) \
             LEFT JOIN statuses source ON source.id = status.reblog_of_id \
             LEFT JOIN accounts source_author ON source_author.id = source.account_id \
             JOIN accounts viewer ON viewer.id = follow.account_id \
             JOIN accounts recipient ON recipient.id = follow.account_id \
              AND recipient.domain IS NULL AND recipient.suspended_at IS NULL \
              JOIN users recipient_user ON recipient_user.account_id = recipient.id \
               AND recipient_user.disabled IS FALSE \
              WHERE status.id = $1 \
                AND (($2 AND status.visibility IN (0, 1, 2)) OR NOT EXISTS (SELECT 1 FROM blocks blocked_by \
                               WHERE blocked_by.account_id = status.account_id \
                               AND blocked_by.target_account_id = follow.account_id)) \
               AND (($2 AND status.visibility IN (0, 1, 2)) OR NOT EXISTS (SELECT 1 FROM blocks blocked \
                               WHERE blocked.account_id = follow.account_id \
                               AND blocked.target_account_id = status.account_id)) \
               AND (($2 AND status.visibility IN (0, 1, 2)) OR NOT EXISTS (SELECT 1 FROM mutes muted \
                               WHERE muted.account_id = follow.account_id \
                                 AND muted.target_account_id = status.account_id \
                                 AND (muted.expires_at IS NULL OR muted.expires_at > clock_timestamp()))) \
               AND (($2 AND status.visibility IN (0, 1, 2)) OR status.account_id = follow.account_id OR COALESCE(cardinality(follow.languages), 0) = 0 \
               OR status.language IS NULL OR status.language = ANY(follow.languages)) \
               AND (($2 AND status.visibility IN (0, 1, 2)) OR status.account_id = follow.account_id OR NOT status.reply \
                OR (status.in_reply_to_id IS NOT NULL AND status.in_reply_to_account_id IS NOT NULL)) \
               AND (($2 AND status.visibility IN (0, 1, 2)) OR status.account_id = follow.account_id OR NOT status.reply \
                OR status.in_reply_to_account_id = status.account_id \
                OR status.in_reply_to_account_id = follow.account_id \
                OR EXISTS (SELECT 1 FROM follows reply_follow \
                           WHERE reply_follow.account_id = follow.account_id \
                             AND reply_follow.target_account_id = status.in_reply_to_account_id)) \
               AND (($2 AND status.visibility IN (0, 1, 2)) OR status.account_id = follow.account_id OR NOT EXISTS ( \
                 SELECT 1 FROM list_accounts list_account \
                  JOIN lists exclusive_list ON exclusive_list.id = list_account.list_id \
                   WHERE exclusive_list.account_id = follow.account_id \
                   AND exclusive_list.exclusive \
                   AND list_account.account_id = status.account_id)) \
               AND (($2 AND status.visibility IN (0, 1, 2)) OR NOT EXISTS (SELECT 1 FROM mentions mention \
                                WHERE mention.status_id IN (status.id, status.reblog_of_id) \
                                 AND mention.silent IS FALSE \
                                   AND (EXISTS (SELECT 1 FROM blocks mention_block \
                                                WHERE mention_block.account_id = follow.account_id \
                                                  AND mention_block.target_account_id = mention.account_id) \
                                   OR EXISTS (SELECT 1 FROM mutes mention_mute \
                                                 WHERE mention_mute.account_id = follow.account_id \
                                                   AND mention_mute.target_account_id = mention.account_id \
                                                   AND (mention_mute.expires_at IS NULL OR mention_mute.expires_at > clock_timestamp()))))) \
               AND (($2 AND status.visibility IN (0, 1, 2)) OR status.reblog_of_id IS NULL OR ( \
                 NOT EXISTS (SELECT 1 FROM blocks source_block \
                             WHERE source_block.account_id = follow.account_id \
                               AND source_block.target_account_id = source.account_id) \
                 AND NOT EXISTS (SELECT 1 FROM mutes source_mute \
                                  WHERE source_mute.account_id = follow.account_id \
                                    AND source_mute.target_account_id = source.account_id \
                                    AND (source_mute.expires_at IS NULL OR source_mute.expires_at > clock_timestamp())) \
                AND NOT EXISTS (SELECT 1 FROM blocks source_author_block \
                   WHERE source_author_block.account_id = source.account_id \
                                 AND source_author_block.target_account_id = follow.account_id) \
                 AND (source_author.domain IS NULL OR NOT EXISTS ( \
                  SELECT 1 FROM account_domain_blocks source_domain_block \
                   WHERE source_domain_block.account_id = follow.account_id \
                    AND source_domain_block.domain = source_author.domain)))) \
               AND (($2 AND status.visibility IN (0, 1, 2)) OR author.domain IS NULL OR NOT EXISTS ( \
                  SELECT 1 FROM account_domain_blocks author_domain_block \
                  WHERE author_domain_block.account_id = follow.account_id \
                  AND author_domain_block.domain = author.domain)) \
           UNION \
             SELECT tag_follow.account_id \
               FROM statuses status \
               JOIN accounts author ON author.id = status.account_id \
               JOIN statuses_tags status_tag ON status_tag.status_id = status.id \
               JOIN tag_follows tag_follow ON tag_follow.tag_id = status_tag.tag_id \
               JOIN accounts recipient ON recipient.id = tag_follow.account_id \
                AND recipient.domain IS NULL AND recipient.suspended_at IS NULL \
               JOIN users recipient_user ON recipient_user.account_id = recipient.id \
                AND recipient_user.disabled IS FALSE \
              WHERE status.id = $1 AND ($2 OR status.deleted_at IS NULL) \
                AND status.visibility = 0 AND status.reblog_of_id IS NULL \
                AND ($2 OR (author.suspended_at IS NULL AND author.silenced_at IS NULL)) \
                AND NOT EXISTS (SELECT 1 FROM blocks blocked_by \
                  WHERE blocked_by.account_id = status.account_id \
                    AND blocked_by.target_account_id = tag_follow.account_id) \
                AND NOT EXISTS (SELECT 1 FROM blocks blocked \
                  WHERE blocked.account_id = tag_follow.account_id \
                    AND blocked.target_account_id = status.account_id) \
                AND NOT EXISTS (SELECT 1 FROM mutes muted \
                  WHERE muted.account_id = tag_follow.account_id \
                    AND muted.target_account_id = status.account_id) \
                AND NOT EXISTS (SELECT 1 FROM mentions mention \
                  WHERE mention.status_id = status.id AND NOT mention.silent \
                    AND (EXISTS (SELECT 1 FROM blocks mention_block \
                      WHERE mention_block.account_id = tag_follow.account_id \
                        AND mention_block.target_account_id = mention.account_id) \
                    OR EXISTS (SELECT 1 FROM mutes mention_mute \
                      WHERE mention_mute.account_id = tag_follow.account_id \
                        AND mention_mute.target_account_id = mention.account_id))) \
                AND (author.domain IS NULL OR NOT EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                  WHERE domain_block.account_id = tag_follow.account_id \
                    AND domain_block.domain = author.domain)) \
            ) SELECT account_id FROM recipients ORDER BY account_id",
    )
    .bind(status_id)
    .bind(deleting)
    .fetch_all(&mut **transaction)
    .await?;
    for account_id in recipients {
        let logical_key = key.for_recipient(account_id, event, status_id);
        pending.push(pending_stream_event(
            account_id,
            event,
            status_id,
            &logical_key,
            None,
            None,
        )?);
        stage_stream_events_if_large_in(transaction, pending).await?;
    }
    Ok(())
}

async fn record_status_delete_stream_events(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<(), WriteError> {
    let mut pending = Vec::new();
    collect_status_delete_stream_events(transaction, &mut pending, status_id).await?;
    flush_stream_events_in(transaction, &mut pending).await?;
    Ok(())
}

async fn collect_status_delete_stream_events(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    status_id: i64,
) -> Result<(), WriteError> {
    let before = status_timeline_snapshot(transaction, status_id).await?;
    collect_status_delete_stream_events_with_snapshot(transaction, pending, status_id, before).await
}

async fn collect_status_delete_stream_events_with_snapshot(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    status_id: i64,
    before: TimelineRouteSnapshot,
) -> Result<(), WriteError> {
    let key = StreamEventLogicalKey::Version(0);
    let global_key = key.for_global("delete", status_id);
    pending.push(pending_stream_event(
        0,
        "delete",
        status_id,
        &global_key,
        Some(&before),
        None,
    )?);
    stage_stream_events_if_large_in(transaction, pending).await?;
    collect_status_recipient_stream_events(transaction, pending, status_id, "delete", key).await?;
    let recipients = sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT mention.account_id
           FROM mentions mention
           JOIN accounts recipient ON recipient.id = mention.account_id
            AND recipient.domain IS NULL AND recipient.suspended_at IS NULL
           JOIN users recipient_user ON recipient_user.account_id = recipient.id
            AND recipient_user.disabled IS FALSE
          WHERE mention.status_id = $1 AND mention.silent IS FALSE
          ORDER BY mention.account_id",
    )
    .bind(status_id)
    .fetch_all(&mut **transaction)
    .await?;
    for account_id in recipients {
        let logical_key = event_logical_key(account_id, "delete", status_id, 0);
        pending.push(pending_stream_event(
            account_id,
            "delete",
            status_id,
            &logical_key,
            None,
            None,
        )?);
        stage_stream_events_if_large_in(transaction, pending).await?;
    }
    Ok(())
}

async fn collect_status_update_notification_stream_events(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    status_id: i64,
    version: i64,
) -> Result<(), WriteError> {
    collect_status_update_notification_stream_events_with_key(
        transaction,
        pending,
        status_id,
        StreamEventLogicalKey::Version(version),
    )
    .await
}

async fn collect_status_update_notification_stream_events_with_key(
    transaction: &mut Transaction<'_, Postgres>,
    pending: &mut Vec<PendingStreamEvent>,
    status_id: i64,
    key: StreamEventLogicalKey,
) -> Result<(), WriteError> {
    let recipients = sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT mention.account_id \
           FROM mentions mention \
           JOIN accounts recipient ON recipient.id = mention.account_id \
            AND recipient.domain IS NULL AND recipient.suspended_at IS NULL \
           JOIN users recipient_user ON recipient_user.account_id = recipient.id \
            AND recipient_user.disabled IS FALSE \
          WHERE mention.status_id = $1 AND mention.silent IS FALSE \
          ORDER BY mention.account_id",
    )
    .bind(status_id)
    .fetch_all(&mut **transaction)
    .await?;
    for account_id in recipients {
        let logical_key =
            key.for_recipient(account_id, STATUS_UPDATE_NOTIFICATION_EVENT, status_id);
        pending.push(pending_stream_event(
            account_id,
            STATUS_UPDATE_NOTIFICATION_EVENT,
            status_id,
            &logical_key,
            None,
            None,
        )?);
        stage_stream_events_if_large_in(transaction, pending).await?;
    }
    Ok(())
}

fn notification_job_with_silenced(
    recipient_account_id: i64,
    activity_type: &str,
    activity_id: i64,
    silenced: bool,
) -> JobSpec {
    JobSpec::new(
        Lane::Core,
        NOTIFICATION_CREATE_JOB_KIND,
        json!({
            "recipient_account_id": recipient_account_id,
            "activity_type": activity_type,
            "activity_id": activity_id,
            "silenced": silenced
        }),
    )
    .logical_key(format!(
        "notification:{activity_type}:{recipient_account_id}:{activity_id}"
    ))
}

async fn cancel_activitypub_delivery(
    transaction: &mut Transaction<'_, Postgres>,
    activity_uri: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE kind = $1
            AND dispatched_at IS NULL
            AND payload -> 'arguments' -> 'body' ->> 'id' = $2",
    )
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(activity_uri)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.durable_jobs
          WHERE kind = $1
            AND dead_at IS NULL
            AND arguments -> 'body' ->> 'id' = $2
            AND (lease_owner IS NULL OR lease_expires_at <= clock_timestamp())",
    )
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(activity_uri)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn cancel_pending_account_job(
    transaction: &mut Transaction<'_, Postgres>,
    kind: &str,
    account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE kind = $1
            AND dispatched_at IS NULL
            AND payload -> 'arguments' ->> 'account_id' = $2",
    )
    .bind(kind)
    .bind(account_id.to_string())
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.durable_jobs
          WHERE kind = $1
            AND dead_at IS NULL
            AND arguments ->> 'account_id' = $2
            AND (lease_owner IS NULL OR lease_expires_at <= clock_timestamp())",
    )
    .bind(kind)
    .bind(account_id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn local_activitypub_account_id(
    transaction: &mut Transaction<'_, Postgres>,
    object_uri: &str,
    origin: &str,
) -> Result<Option<i64>, WriteError> {
    let origin = origin.trim_end_matches('/');
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts
          WHERE domain IS NULL
            AND (
              uri = $1
              OR url = $1
              OR ($1 = $2 || '/actor' AND id = -99)
              OR ($1 = $2 || '/@' || username)
              OR ($1 = $2 || '/users/' || username AND id_scheme IS DISTINCT FROM 1)
              OR ($1 = $2 || '/ap/users/' || id::text AND id_scheme = 1)
            )
          ORDER BY id
          LIMIT 1",
    )
    .bind(object_uri)
    .bind(origin)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn remote_report_target_content(
    transaction: &mut Transaction<'_, Postgres>,
    target_account_id: i64,
    source_domain: &str,
    origin: &str,
    local_domain: &str,
    object_uris: &[String],
) -> Result<(Vec<i64>, Vec<i64>), WriteError> {
    let (target_username, target_id_scheme) = sqlx::query_as::<_, (String, Option<i32>)>(
        "SELECT username, id_scheme FROM accounts WHERE id = $1",
    )
    .bind(target_account_id)
    .fetch_one(&mut **transaction)
    .await?;
    let origin = origin.trim_end_matches('/');
    let account_path = if target_id_scheme == Some(AccountIdScheme::Numeric.raw()) {
        format!("ap/users/{target_account_id}")
    } else {
        format!("users/{target_username}")
    };
    let status_uri_prefix = format!("{origin}/{account_path}/statuses/");
    let status_permalink_prefix = format!("{origin}/@{target_username}/");
    let collection_uri_prefix = format!("{origin}/ap/users/{target_account_id}/collections/");
    let collection_web_uri_prefix = format!("{origin}/collections/");
    let mut status_ids = Vec::new();
    let mut collection_ids = Vec::new();
    for object_uri in object_uris {
        let tagged_status_id = local_object_tag_id(object_uri, local_domain, "Status");
        if let Some(status_id) = sqlx::query_scalar::<_, i64>(
            "SELECT status.id FROM statuses status
               WHERE status.account_id = $1
                 AND (
                   status.uri = $2
                   OR status.url = $2
                   OR $2 = $3 || status.id::text || CASE
                       WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                   OR $2 = $4 || status.id::text || CASE
                       WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                   OR ($6 IS NOT NULL AND status.id = $6)
                 )
                 AND (
                   status.visibility IN (0, 1)
                   OR (status.visibility = 2 AND EXISTS (
                       SELECT 1 FROM follows follow
                       JOIN accounts follower ON follower.id = follow.account_id
                        WHERE follow.target_account_id = status.account_id
                          AND lower(follower.domain) = lower($5)
                   ))
                   OR EXISTS (
                       SELECT 1 FROM mentions mention
                       JOIN accounts mentioned ON mentioned.id = mention.account_id
                        WHERE mention.status_id = status.id
                          AND lower(mentioned.domain) = lower($5)
                   )
                 )
               ORDER BY status.id LIMIT 1",
        )
        .bind(target_account_id)
        .bind(object_uri)
        .bind(&status_uri_prefix)
        .bind(&status_permalink_prefix)
        .bind(source_domain)
        .bind(tagged_status_id)
        .fetch_optional(&mut **transaction)
        .await?
            && !status_ids.contains(&status_id)
        {
            status_ids.push(status_id);
        }
        let tagged_collection_id = local_object_tag_id(object_uri, local_domain, "Collection");
        if let Some(collection_id) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM collections
               WHERE account_id = $1
                 AND (uri = $2 OR url = $2 OR $2 = $3 || id::text
                      OR $2 = $4 || id::text
                      OR ($5 IS NOT NULL AND id = $5))
               ORDER BY id LIMIT 1",
        )
        .bind(target_account_id)
        .bind(object_uri)
        .bind(&collection_uri_prefix)
        .bind(&collection_web_uri_prefix)
        .bind(tagged_collection_id)
        .fetch_optional(&mut **transaction)
        .await?
            && !collection_ids.contains(&collection_id)
        {
            collection_ids.push(collection_id);
        }
    }
    Ok((status_ids, collection_ids))
}

fn local_object_tag_id(uri: &str, local_domain: &str, object_type: &str) -> Option<i64> {
    let prefix = format!("tag:{local_domain}");
    let object_id = uri.strip_prefix(&prefix)?.split_once("objectId=")?.1;
    let (object_id, actual_type) = object_id.split_once(":objectType=")?;
    (actual_type == object_type && object_id.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| object_id.parse::<i64>().ok())
        .flatten()
}

fn relationship_tombstone_key(kind: &str, source_account_id: i64, activity_uri: &str) -> String {
    format!(
        "{kind}:{:x}",
        Sha256::digest(format!("{source_account_id}:{activity_uri}").as_bytes())
    )
}

fn follow_tombstone_key(source_account_id: i64, follow_uri: &str) -> String {
    relationship_tombstone_key("undo-follow", source_account_id, follow_uri)
}

fn block_tombstone_key(source_account_id: i64, block_uri: &str) -> String {
    relationship_tombstone_key("undo-block", source_account_id, block_uri)
}

async fn lock_follow_tombstone(
    transaction: &mut Transaction<'_, Postgres>,
    tombstone_key: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(
           pg_catalog.hashtextextended($1, 0))",
    )
    .bind(tombstone_key)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_relationship_tombstone(
    transaction: &mut Transaction<'_, Postgres>,
    tombstone_key: &str,
    activity_uri: &str,
) -> Result<(), WriteError> {
    let fingerprint = Sha256::digest(activity_uri.as_bytes()).to_vec();
    sqlx::query(
        "INSERT INTO rustodon.idempotency_keys
             (scope, key, fingerprint, result, expires_at)
         VALUES ($1, $2, $3, $4, clock_timestamp() + $5::interval)
         ON CONFLICT (scope, key) DO UPDATE
           SET fingerprint = EXCLUDED.fingerprint,
               result = EXCLUDED.result,
               created_at = clock_timestamp(),
               expires_at = EXCLUDED.expires_at
         WHERE idempotency_keys.expires_at <= clock_timestamp()",
    )
    .bind(RELATIONSHIP_TOMBSTONE_SCOPE)
    .bind(tombstone_key)
    .bind(fingerprint)
    .bind(json!({ "type": "Undo", "object": activity_uri }))
    .bind(RELATIONSHIP_TOMBSTONE_TTL)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn update_follow_uri(
    transaction: &mut Transaction<'_, Postgres>,
    table: &str,
    account_id: i64,
    target_account_id: i64,
    uri: &str,
) -> Result<(), WriteError> {
    let query = match table {
        "follows" => {
            "UPDATE follows SET uri = $3, updated_at = clock_timestamp()
              WHERE account_id = $1 AND target_account_id = $2"
        }
        "follow_requests" => {
            "UPDATE follow_requests SET uri = $3, updated_at = clock_timestamp()
              WHERE account_id = $1 AND target_account_id = $2"
        }
        _ => unreachable!("remote Follow URI updates use fixed tables"),
    };
    sqlx::query(query)
        .bind(account_id)
        .bind(target_account_id)
        .bind(uri)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn lock_relationship(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    target_account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock( \
           hashtextextended(LEAST($1, $2)::text || ':' || GREATEST($1, $2)::text, 0))",
    )
    .bind(account_id)
    .bind(target_account_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn relationship_is_blocked(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    target_account_id: i64,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS ( \
           SELECT 1 FROM blocks \
           WHERE (account_id = $1 AND target_account_id = $2) \
              OR (account_id = $2 AND target_account_id = $1))",
    )
    .bind(account_id)
    .bind(target_account_id)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn account_domain_is_blocked(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    target_account_id: i64,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1
             FROM account_domain_blocks domain_block
             JOIN accounts target ON target.id = $2
            WHERE domain_block.account_id = $1
              AND lower(domain_block.domain) = lower(target.domain))",
    )
    .bind(account_id)
    .bind(target_account_id)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn remote_domain_allowed_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
    limited_federation: bool,
) -> Result<bool, WriteError> {
    let domain = domain_policy_hostname(domain);
    if limited_federation {
        return Ok(sqlx::query_scalar(
            "SELECT EXISTS (
               SELECT 1 FROM domain_allows WHERE lower(domain) = lower($1))",
        )
        .bind(&domain)
        .fetch_one(&mut **transaction)
        .await?);
    }
    let blocks = matching_domain_blocks_in_transaction(transaction, &domain).await?;
    let rules = blocks
        .iter()
        .map(DomainBlock::policy_rule)
        .collect::<Vec<_>>();
    Ok(!global_domain_policy(&domain, &rules).blocks_federation())
}

async fn remote_media_allowed_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
    limited_federation: bool,
) -> Result<bool, WriteError> {
    let domain = domain_policy_hostname(domain);
    if limited_federation {
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS (
               SELECT 1 FROM domain_allows WHERE lower(domain) = lower($1))",
        )
        .bind(&domain)
        .fetch_one(&mut **transaction)
        .await?;
        if !allowed {
            return Ok(false);
        }
    }
    let blocks = matching_domain_blocks_in_transaction(transaction, &domain).await?;
    let rules = blocks
        .iter()
        .map(DomainBlock::policy_rule)
        .collect::<Vec<_>>();
    Ok(!global_domain_policy(&domain, &rules).rejects_media())
}

async fn matching_domain_blocks_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
) -> Result<Vec<DomainBlock>, WriteError> {
    Ok(sqlx::query_as::<_, DomainBlock>(
        "SELECT id, domain, severity, reject_media, reject_reports, private_comment,
                public_comment, obfuscate
           FROM domain_blocks
          WHERE lower(domain) = lower(trim(trailing '.' FROM $1))
             OR lower(trim(trailing '.' FROM $1)) LIKE '%.' || lower(domain)",
    )
    .bind(domain)
    .fetch_all(&mut **transaction)
    .await?)
}

async fn delete_follow_request_notifications(
    transaction: &mut Transaction<'_, Postgres>,
    recipient_account_id: i64,
    request_id: i64,
) -> Result<(), WriteError> {
    delete_activity_notifications(
        transaction,
        recipient_account_id,
        request_id,
        "FollowRequest",
    )
    .await
}

async fn delete_activity_notifications(
    transaction: &mut Transaction<'_, Postgres>,
    recipient_account_id: i64,
    activity_id: i64,
    activity_type: &str,
) -> Result<(), WriteError> {
    lock_notification_recipient(transaction, recipient_account_id).await?;
    cancel_pending_notification_jobs_locked(transaction, recipient_account_id, activity_id).await?;
    let from_account_ids = sqlx::query_scalar::<_, i64>(
        "DELETE FROM notifications WHERE account_id = $1 AND activity_id = $2 \
         AND activity_type = $3 RETURNING from_account_id",
    )
    .bind(recipient_account_id)
    .bind(activity_id)
    .bind(activity_type)
    .fetch_all(&mut **transaction)
    .await?;
    reconcile_notification_requests(transaction, recipient_account_id, &from_account_ids).await?;
    Ok(())
}

async fn delete_remote_status_notifications(
    transaction: &mut Transaction<'_, Postgres>,
    status_ids: &[i64],
) -> Result<(), WriteError> {
    let notification_keys = sqlx::query_as::<_, (i64, i64, String)>(
        "WITH affected_statuses AS (
             SELECT id FROM statuses
              WHERE id = ANY($1) OR reblog_of_id = ANY($1)
         ), affected_mentions AS (
             SELECT id FROM mentions WHERE status_id IN (SELECT id FROM affected_statuses)
          ), affected_quote_statuses AS (
              SELECT status_id FROM quotes
               WHERE quoted_status_id IN (SELECT id FROM affected_statuses)
          ), affected_favourites AS (
              SELECT id FROM favourites
               WHERE status_id IN (SELECT id FROM affected_statuses)
          ), affected_polls AS (
              SELECT id FROM polls
               WHERE status_id IN (SELECT id FROM affected_statuses)
          )
          SELECT DISTINCT notification.account_id, notification.activity_id, notification.activity_type
            FROM notifications notification
          WHERE (notification.activity_type = 'Status'
                 AND (notification.activity_id IN (SELECT id FROM affected_statuses)
                      OR (notification.type = 'quoted_update'
                          AND notification.activity_id IN (SELECT status_id FROM affected_quote_statuses))))
              OR (notification.activity_type = 'Mention'
                  AND notification.activity_id IN (SELECT id FROM affected_mentions))
              OR (notification.activity_type = 'Favourite'
                  AND notification.activity_id IN (SELECT id FROM affected_favourites))
              OR (notification.activity_type = 'Poll'
                  AND notification.activity_id IN (SELECT id FROM affected_polls))
           ORDER BY notification.account_id, notification.activity_type, notification.activity_id",
    )
    .bind(status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    for (recipient_account_id, activity_id, activity_type) in notification_keys {
        delete_activity_notifications(
            transaction,
            recipient_account_id,
            activity_id,
            &activity_type,
        )
        .await?;
    }
    Ok(())
}

async fn lock_notification_recipient(
    transaction: &mut Transaction<'_, Postgres>,
    recipient_account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(recipient_account_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn cancel_pending_notification_jobs(
    transaction: &mut Transaction<'_, Postgres>,
    recipient_account_id: i64,
    activity_id: i64,
) -> Result<(), WriteError> {
    lock_notification_recipient(transaction, recipient_account_id).await?;
    cancel_pending_notification_jobs_locked(transaction, recipient_account_id, activity_id).await
}

async fn cancel_pending_notification_jobs_locked(
    transaction: &mut Transaction<'_, Postgres>,
    recipient_account_id: i64,
    activity_id: i64,
) -> Result<(), WriteError> {
    // The web writer may not mutate runtime durable jobs. Dispatched jobs are fenced by the
    // recipient lock and the activity resolver; only undispatched outbox events are canceled here.
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
         WHERE kind = $1 AND dispatched_at IS NULL \
           AND payload -> 'arguments' ->> 'recipient_account_id' = $2 \
           AND payload -> 'arguments' ->> 'activity_id' = $3",
    )
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(recipient_account_id.to_string())
    .bind(activity_id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn cancel_pending_mute_expiry_events(
    transaction: &mut Transaction<'_, Postgres>,
    mute_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
         WHERE kind = $1 AND dispatched_at IS NULL \
           AND payload -> 'arguments' ->> 'mute_id' = $2",
    )
    .bind(MUTE_EXPIRY_JOB_KIND)
    .bind(mute_id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn remove_follow_relationships(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    target_account_id: i64,
) -> Result<(), WriteError> {
    let follows = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT id, account_id, target_account_id FROM follows \
          WHERE (account_id = $1 AND target_account_id = $2) \
             OR (account_id = $2 AND target_account_id = $1) \
          ORDER BY account_id, target_account_id \
          FOR UPDATE",
    )
    .bind(account_id)
    .bind(target_account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let requests = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT id, account_id, target_account_id FROM follow_requests \
          WHERE account_id = $2 AND target_account_id = $1 \
          ORDER BY account_id, target_account_id \
          FOR UPDATE",
    )
    .bind(account_id)
    .bind(target_account_id)
    .fetch_all(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM follows \
         WHERE (account_id = $1 AND target_account_id = $2) \
            OR (account_id = $2 AND target_account_id = $1)",
    )
    .bind(account_id)
    .bind(target_account_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM follow_requests \
          WHERE account_id = $2 AND target_account_id = $1",
    )
    .bind(account_id)
    .bind(target_account_id)
    .execute(&mut **transaction)
    .await?;
    let mut relationship_deltas = HashMap::new();
    for (_, source_account_id, followed_account_id) in &follows {
        add_account_stats_delta(
            &mut relationship_deltas,
            *source_account_id,
            AccountStatsDelta {
                following: -1,
                ..AccountStatsDelta::default()
            },
        );
        add_account_stats_delta(
            &mut relationship_deltas,
            *followed_account_id,
            AccountStatsDelta {
                followers: -1,
                ..AccountStatsDelta::default()
            },
        );
    }
    apply_account_stats_deltas(transaction, relationship_deltas).await?;
    for (follow_id, _, followed_account_id) in &follows {
        delete_activity_notifications(transaction, *followed_account_id, *follow_id, "Follow")
            .await?;
    }
    for (request_id, _source_account_id, followed_account_id) in requests {
        delete_activity_notifications(
            transaction,
            followed_account_id,
            request_id,
            "FollowRequest",
        )
        .await?;
    }
    Ok(())
}

async fn update_follow_options(
    transaction: &mut Transaction<'_, Postgres>,
    table: &str,
    account_id: i64,
    target_account_id: i64,
    reblogs: Option<bool>,
    notify: Option<bool>,
    languages: Option<Vec<String>>,
) -> Result<(), WriteError> {
    let query = match table {
        "follows" => {
            "UPDATE follows SET show_reblogs = COALESCE($3, show_reblogs), \
             notify = COALESCE($4, notify), languages = COALESCE($5, languages), \
             updated_at = clock_timestamp() \
             WHERE account_id = $1 AND target_account_id = $2"
        }
        "follow_requests" => {
            "UPDATE follow_requests SET show_reblogs = COALESCE($3, show_reblogs), \
             notify = COALESCE($4, notify), languages = COALESCE($5, languages), \
             updated_at = clock_timestamp() \
             WHERE account_id = $1 AND target_account_id = $2"
        }
        _ => unreachable!("follow option updates use fixed tables"),
    };
    sqlx::query(query)
        .bind(account_id)
        .bind(target_account_id)
        .bind(reblogs)
        .bind(notify)
        .bind(languages)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct AccountStatsDelta {
    statuses: i64,
    following: i64,
    followers: i64,
    last_status_at: Option<NaiveDateTime>,
}

fn add_account_stats_delta(
    deltas: &mut HashMap<i64, AccountStatsDelta>,
    account_id: i64,
    delta: AccountStatsDelta,
) {
    let total = deltas.entry(account_id).or_default();
    total.statuses += delta.statuses;
    total.following += delta.following;
    total.followers += delta.followers;
    total.last_status_at = total.last_status_at.max(delta.last_status_at);
}

async fn apply_account_stats_deltas(
    transaction: &mut Transaction<'_, Postgres>,
    deltas: HashMap<i64, AccountStatsDelta>,
) -> Result<(), WriteError> {
    let mut deltas = deltas.into_iter().collect::<Vec<_>>();
    deltas.sort_unstable_by_key(|(account_id, _)| *account_id);
    for (account_id, delta) in deltas {
        let initialization = initialize_account_stats_if_missing(transaction, account_id).await?;
        if initialization.inserted {
            // Initialization observes this transaction's post-mutation state.
            continue;
        }
        sqlx::query(
            "UPDATE account_stats SET \
               statuses_count = GREATEST(statuses_count + $2, 0), \
               following_count = GREATEST(following_count + $3, 0), \
               followers_count = GREATEST(followers_count + $4, 0), \
               last_status_at = CASE WHEN $5::timestamp IS NULL THEN last_status_at \
                 WHEN last_status_at IS NULL THEN LEAST($5, clock_timestamp()) \
                 ELSE GREATEST(last_status_at, LEAST($5, clock_timestamp())) END, \
               updated_at = clock_timestamp() \
             WHERE account_id = $1",
        )
        .bind(account_id)
        .bind(delta.statuses)
        .bind(delta.following)
        .bind(delta.followers)
        .bind(delta.last_status_at)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn ensure_relationship_account_stats(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    target_account_id: i64,
) -> Result<(), WriteError> {
    let mut deltas = HashMap::new();
    add_account_stats_delta(&mut deltas, account_id, AccountStatsDelta::default());
    add_account_stats_delta(&mut deltas, target_account_id, AccountStatsDelta::default());
    apply_account_stats_deltas(transaction, deltas).await
}

async fn increment_follow_counts(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    target_account_id: i64,
) -> Result<(), WriteError> {
    let mut deltas = HashMap::new();
    add_account_stats_delta(
        &mut deltas,
        account_id,
        AccountStatsDelta {
            following: 1,
            ..AccountStatsDelta::default()
        },
    );
    add_account_stats_delta(
        &mut deltas,
        target_account_id,
        AccountStatsDelta {
            followers: 1,
            ..AccountStatsDelta::default()
        },
    );
    apply_account_stats_deltas(transaction, deltas).await
}

async fn decrement_follow_counts(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    target_account_id: i64,
) -> Result<(), WriteError> {
    let mut deltas = HashMap::new();
    add_account_stats_delta(
        &mut deltas,
        account_id,
        AccountStatsDelta {
            following: -1,
            ..AccountStatsDelta::default()
        },
    );
    add_account_stats_delta(
        &mut deltas,
        target_account_id,
        AccountStatsDelta {
            followers: -1,
            ..AccountStatsDelta::default()
        },
    );
    apply_account_stats_deltas(transaction, deltas).await
}

async fn authorize_poll_vote(
    transaction: &mut Transaction<'_, Postgres>,
    viewer_account_id: i64,
    status_id: i64,
    poll_account_id: i64,
) -> Result<(), WriteError> {
    // Keep one authorization path for REST and verified inbox votes. Visibility intentionally
    // returns NotFound; Mastodon's PollPolicy then adds the symmetric account-block restriction.
    writable_reply_target(transaction, viewer_account_id, status_id).await?;
    let either_account_blocks_the_other = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM blocks \
           WHERE (account_id = $1 AND target_account_id = $2) \
              OR (account_id = $2 AND target_account_id = $1))",
    )
    .bind(viewer_account_id)
    .bind(poll_account_id)
    .fetch_one(&mut **transaction)
    .await?;
    if either_account_blocks_the_other {
        return Err(WriteError::Forbidden);
    }
    Ok(())
}

async fn writable_reply_target(
    transaction: &mut Transaction<'_, Postgres>,
    viewer_account_id: i64,
    status_id: i64,
) -> Result<(i64, Option<i64>, Option<String>), WriteError> {
    let (
        account_id,
        conversation_id,
        language,
        visibility,
        author_suspended,
        viewer_is_author,
        viewer_follows_author,
        viewer_is_mentioned,
        author_blocks_viewer,
        author_domain_blocks_viewer,
    ) = sqlx::query_as::<
        _,
        (
            i64,
            Option<i64>,
            Option<String>,
            i32,
            bool,
            bool,
            bool,
            bool,
            bool,
            bool,
        ),
    >(
        "SELECT status.account_id, status.conversation_id, status.language, status.visibility, \
                author.suspended_at IS NOT NULL, status.account_id = $2, \
                EXISTS (SELECT 1 FROM follows follow WHERE follow.account_id = $2 \
                  AND follow.target_account_id = status.account_id), \
                EXISTS (SELECT 1 FROM mentions mention WHERE mention.status_id = status.id \
                  AND mention.account_id = $2), \
                EXISTS (SELECT 1 FROM blocks block WHERE block.account_id = status.account_id \
                  AND block.target_account_id = $2), \
                EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                  JOIN accounts viewer ON viewer.id = $2 \
                  WHERE domain_block.account_id = status.account_id \
                    AND domain_block.domain = viewer.domain) \
           FROM statuses status JOIN accounts author ON author.id = status.account_id \
          WHERE status.id = $1 AND status.deleted_at IS NULL",
    )
    .bind(status_id)
    .bind(viewer_account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)?;
    let author_restriction = if author_blocks_viewer {
        AuthorRestriction::BlocksViewer
    } else if author_domain_blocks_viewer {
        AuthorRestriction::BlocksViewerDomain
    } else {
        AuthorRestriction::None
    };
    let access = status_access(StatusAccessFacts {
        visibility: StatusVisibility::from(visibility),
        availability: if author_suspended {
            StatusAvailability::AuthorSuspended
        } else {
            StatusAvailability::Available
        },
        viewer: ViewerFacts::Authenticated(AuthenticatedViewerFacts {
            is_author: viewer_is_author,
            follows_author: viewer_follows_author,
            is_mentioned: viewer_is_mentioned,
            author_restriction,
        }),
    });
    if !access.is_allowed() {
        return Err(WriteError::NotFound);
    }
    Ok((account_id, conversation_id, language))
}

#[derive(sqlx::FromRow)]
#[allow(clippy::struct_excessive_bools)]
struct WritableQuoteTargetRow {
    status_id: i64,
    account_id: i64,
    visibility: i32,
    quote_approval_policy: i32,
    is_reblog: bool,
    local: bool,
    author_suspended: bool,
    viewer_is_author: bool,
    viewer_follows_author: bool,
    author_follows_viewer: bool,
    viewer_is_mentioned: bool,
    author_blocks_viewer: bool,
    author_domain_blocks_viewer: bool,
    viewer_blocks_author: bool,
}

struct WritableQuoteTarget {
    status_id: i64,
    account_id: i64,
    visibility: i32,
    local: bool,
}

async fn writable_quote_target(
    transaction: &mut Transaction<'_, Postgres>,
    viewer_account_id: i64,
    requested_status_id: i64,
) -> Result<WritableQuoteTarget, WriteError> {
    let target_status_id = writable_status_id(transaction, requested_status_id).await?;
    let row = sqlx::query_as::<_, WritableQuoteTargetRow>(
        "SELECT status.id AS status_id, status.account_id, status.visibility, \
                status.quote_approval_policy, status.reblog_of_id IS NOT NULL AS is_reblog, \
                author.domain IS NULL AS local, author.suspended_at IS NOT NULL AS author_suspended, \
                status.account_id = $2 AS viewer_is_author, \
                EXISTS (SELECT 1 FROM follows follow WHERE follow.account_id = $2 \
                  AND follow.target_account_id = status.account_id) AS viewer_follows_author, \
                EXISTS (SELECT 1 FROM follows follow WHERE follow.account_id = status.account_id \
                  AND follow.target_account_id = $2) AS author_follows_viewer, \
                EXISTS (SELECT 1 FROM mentions mention WHERE mention.status_id = status.id \
                  AND mention.account_id = $2) AS viewer_is_mentioned, \
                EXISTS (SELECT 1 FROM blocks block WHERE block.account_id = status.account_id \
                  AND block.target_account_id = $2) AS author_blocks_viewer, \
                EXISTS (SELECT 1 FROM account_domain_blocks domain_block \
                  JOIN accounts viewer ON viewer.id = $2 \
                  WHERE domain_block.account_id = status.account_id \
                    AND domain_block.domain = viewer.domain) AS author_domain_blocks_viewer, \
                EXISTS (SELECT 1 FROM blocks block WHERE block.account_id = $2 \
                  AND block.target_account_id = status.account_id) AS viewer_blocks_author \
           FROM statuses status JOIN accounts author ON author.id = status.account_id \
          WHERE status.id = $1 AND status.deleted_at IS NULL FOR UPDATE OF status",
    )
    .bind(target_status_id)
    .bind(viewer_account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)?;
    let canonical_after_lock = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(reblog_of_id, id) FROM statuses \
         WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(requested_status_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)?;
    if canonical_after_lock != row.status_id {
        return Err(WriteError::NotFound);
    }
    let author_restriction = if row.author_blocks_viewer {
        AuthorRestriction::BlocksViewer
    } else if row.author_domain_blocks_viewer {
        AuthorRestriction::BlocksViewerDomain
    } else {
        AuthorRestriction::None
    };
    let policy = status_quote_policy(QuotePolicyFacts {
        visibility: StatusVisibility::from(row.visibility),
        is_reblog: row.is_reblog,
        approval_policy: row.quote_approval_policy,
        viewer: Some(QuotePolicyViewerFacts {
            is_author: row.viewer_is_author,
            follows_author: row.viewer_follows_author,
            author_follows_viewer: row.author_follows_viewer,
        }),
    });
    if !quote_target_visibility_allowed(
        row.viewer_is_author,
        StatusVisibility::from(row.visibility),
    ) || !status_access(StatusAccessFacts {
        visibility: StatusVisibility::from(row.visibility),
        availability: if row.author_suspended {
            StatusAvailability::AuthorSuspended
        } else {
            StatusAvailability::Available
        },
        viewer: ViewerFacts::Authenticated(AuthenticatedViewerFacts {
            is_author: row.viewer_is_author,
            follows_author: row.viewer_follows_author,
            is_mentioned: row.viewer_is_mentioned,
            author_restriction,
        }),
    })
    .is_allowed()
        || row.viewer_blocks_author
        || policy == QuotePolicy::Denied
    {
        return Err(WriteError::NotFound);
    }
    Ok(WritableQuoteTarget {
        status_id: row.status_id,
        account_id: row.account_id,
        visibility: row.visibility,
        local: row.local,
    })
}

async fn writable_status_id(
    transaction: &mut Transaction<'_, Postgres>,
    requested_status_id: i64,
) -> Result<i64, WriteError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(reblog_of_id, id) FROM statuses \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(requested_status_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)
}

async fn writable_status_target(
    transaction: &mut Transaction<'_, Postgres>,
    requested_status_id: i64,
) -> Result<(i64, i64), WriteError> {
    sqlx::query_as::<_, (i64, i64)>(
        "SELECT COALESCE(status.reblog_of_id, status.id), \
                 COALESCE(original.account_id, status.account_id) \
          FROM statuses status \
          LEFT JOIN statuses original ON original.id = status.reblog_of_id \
          WHERE status.id = $1 AND status.deleted_at IS NULL",
    )
    .bind(requested_status_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)
}

async fn reblog_target(
    transaction: &mut Transaction<'_, Postgres>,
    requested_status_id: i64,
) -> Result<(i64, i64, i32), WriteError> {
    let target_status_id = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(reblog_of_id, id) FROM statuses \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(requested_status_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)?;
    let target = sqlx::query_as::<_, (i64, i64, i32)>(
        "SELECT id, account_id, visibility FROM statuses \
         WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(target_status_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)?;
    let requested_target_status_id = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(reblog_of_id, id) FROM statuses \
         WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(requested_status_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)?;
    if requested_target_status_id != target_status_id {
        return Err(WriteError::NotFound);
    }
    Ok(target)
}

async fn default_reblog_visibility(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<i32, WriteError> {
    sqlx::query_scalar::<_, i32>(
        "SELECT CASE \
           WHEN account.locked THEN 2 \
           WHEN COALESCE(NULLIF(account_user.settings, '')::jsonb ->> 'default_privacy', 'public') = 'private' THEN 2 \
           WHEN COALESCE(NULLIF(account_user.settings, '')::jsonb ->> 'default_privacy', 'public') = 'unlisted' THEN 1 \
           ELSE 0 \
         END \
         FROM accounts account \
         LEFT JOIN LATERAL ( \
           SELECT settings FROM users WHERE account_id = account.id ORDER BY id LIMIT 1 \
         ) account_user ON true \
         WHERE account.id = $1",
    )
    .bind(account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)
}

fn parse_reblog_visibility(value: &str) -> Result<i32, WriteError> {
    match value {
        "public" => Ok(0),
        "unlisted" => Ok(1),
        "private" => Ok(2),
        _ => Err(WriteError::InvalidInput("invalid reblog visibility")),
    }
}

fn parse_status_visibility(value: &str) -> Result<i32, WriteError> {
    match value {
        "public" => Ok(0),
        "unlisted" => Ok(1),
        "private" => Ok(2),
        "direct" => Ok(3),
        "limited" => Ok(4),
        _ => Err(WriteError::InvalidInput("invalid status visibility")),
    }
}

fn quote_approval_policy_for_status(
    visibility: i32,
    requested_policy: Option<&str>,
    default_policy: &str,
) -> Result<i32, WriteError> {
    let policy = match requested_policy.unwrap_or(default_policy) {
        "public" => 2 << 16,
        "followers" => 4 << 16,
        "nobody" => 0,
        _ if requested_policy.is_some() => {
            return Err(WriteError::InvalidInput("invalid quote approval policy"));
        }
        _ => 2 << 16,
    };
    Ok(if matches!(visibility, 0 | 1) {
        policy
    } else {
        0
    })
}

fn report_account_label(username: &str, domain: Option<&str>) -> String {
    domain.map_or_else(
        || username.to_owned(),
        |domain| format!("{username}@{domain}"),
    )
}

fn report_uri_matches_domain(uri: &str, domain: &str) -> bool {
    let Ok(uri) = Url::parse(uri) else {
        return false;
    };
    if !matches!(uri.scheme(), "http" | "https") {
        return false;
    }
    let domain = domain_policy_hostname(domain);
    uri.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case(&domain))
}

fn domain_policy_hostname(domain: &str) -> String {
    canonical_remote_host(domain).unwrap_or_else(|_| domain.trim_end_matches('.').to_owned())
}

fn remote_domain_lock_scopes(domain: &str) -> Vec<String> {
    let host = domain_policy_hostname(domain).to_ascii_lowercase();
    if host.parse::<IpAddr>().is_ok() || host.contains(':') {
        return vec![host];
    }
    let labels = host.split('.').collect::<Vec<_>>();
    let mut scopes = if labels.is_empty() {
        Vec::new()
    } else {
        (0..labels.len())
            .map(|index| labels[index..].join("."))
            .collect::<Vec<_>>()
    };
    if scopes.is_empty() {
        scopes.push(host);
    }
    scopes.sort_unstable();
    scopes.dedup();
    scopes
}

async fn lock_domain_scope(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
) -> Result<(), WriteError> {
    let lock_name = format!(
        "rustodon:domain:{}",
        domain_policy_hostname(domain).to_ascii_lowercase()
    );
    sqlx::query(
        "SELECT pg_catalog.pg_advisory_xact_lock(
             pg_catalog.hashtextextended($1, 0))",
    )
    .bind(lock_name)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn lock_account_scope(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    let lock_name = format!("rustodon:account:{account_id}");
    sqlx::query(
        "SELECT pg_catalog.pg_advisory_xact_lock(
             pg_catalog.hashtextextended($1, 0))",
    )
    .bind(lock_name)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn ensure_account_write_allowed_in(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    let state = sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<NaiveDateTime>,
            bool,
            Option<NaiveDateTime>,
            bool,
        ),
    >(
        "SELECT account.domain, account.suspended_at, account_user.disabled,
                account_user.confirmed_at, account_user.approved
           FROM accounts account
           JOIN users account_user ON account_user.account_id = account.id
          WHERE account.id = $1
          ORDER BY account_user.id
          LIMIT 1
          FOR NO KEY UPDATE OF account, account_user",
    )
    .bind(account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)?;
    if state.0.is_some() || state.1.is_some() || state.2 || state.3.is_none() || !state.4 {
        return Err(WriteError::Unauthorized);
    }
    Ok(())
}

async fn report_staff_accounts(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<Vec<(i64, String, Option<String>)>, WriteError> {
    Ok(sqlx::query_as::<_, (i64, String, Option<String>)>(
        "SELECT account.id, account_user.email, account_user.settings FROM users account_user \
         JOIN accounts account ON account.id = account_user.account_id \
         JOIN user_roles role ON role.id = COALESCE(account_user.role_id, -99) \
         LEFT JOIN user_roles everyone ON everyone.id = -99 \
         WHERE account.domain IS NULL AND account.suspended_at IS NULL \
           AND account_user.confirmed_at IS NOT NULL AND account_user.approved = true \
           AND account_user.disabled = false \
            AND (role.permissions & 1 <> 0 OR \
                 ((role.permissions | COALESCE(everyone.permissions, 0)) & $1 <> 0))",
    )
    .bind(1_i64 << 4)
    .fetch_all(&mut **transaction)
    .await?)
}

fn report_email_enabled(settings: Option<&str>) -> bool {
    let Some(settings) = settings
        .map(str::trim)
        .filter(|settings| !settings.is_empty())
    else {
        return true;
    };
    serde_json::from_str::<Value>(settings)
        .ok()
        .and_then(|settings| {
            settings
                .get("notification_emails.report")
                .and_then(Value::as_bool)
        })
        .unwrap_or(true)
}

fn configured_user_active_days() -> i32 {
    parse_user_active_days(std::env::var("USER_ACTIVE_DAYS").ok().as_deref())
}

fn parse_user_active_days(value: Option<&str>) -> i32 {
    value
        .map(str::trim)
        .map_or(DEFAULT_USER_ACTIVE_DAYS, |value| {
            value.parse().unwrap_or_default()
        })
}

fn report_category_value(category: Option<&str>, has_rules: bool) -> Result<i32, WriteError> {
    if !has_rules && category == Some("violation") {
        return Err(WriteError::Validation("violation reports require rules"));
    }
    match if has_rules {
        "violation"
    } else {
        category.unwrap_or("other")
    } {
        "other" => Ok(0),
        "spam" => Ok(1_000),
        "legal" => Ok(1_500),
        "violation" => Ok(2_000),
        _ => Err(WriteError::InvalidInput("invalid report category")),
    }
}

async fn increment_favourite_count(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at, favourites_count) \
         VALUES ($1, clock_timestamp(), clock_timestamp(), 1) \
         ON CONFLICT (status_id) DO UPDATE SET \
           favourites_count = GREATEST(status_stats.favourites_count + 1, 0), \
           untrusted_favourites_count = CASE \
             WHEN status_stats.untrusted_favourites_count IS NULL THEN NULL \
             ELSE GREATEST(status_stats.untrusted_favourites_count + 1, 0) END, \
           updated_at = clock_timestamp()",
    )
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn decrement_favourite_count(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE status_stats SET \
           favourites_count = GREATEST(favourites_count - 1, 0), \
           untrusted_favourites_count = CASE \
             WHEN untrusted_favourites_count IS NULL THEN NULL \
             ELSE GREATEST(untrusted_favourites_count - 1, 0) END, \
           updated_at = clock_timestamp() \
         WHERE status_id = $1",
    )
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn decrement_quote_count(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE status_stats SET quotes_count = GREATEST(quotes_count - 1, 0), \
                updated_at = clock_timestamp() WHERE status_id = $1",
    )
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn increment_quote_count(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at, quotes_count) \
         VALUES ($1, clock_timestamp(), clock_timestamp(), 1) \
         ON CONFLICT (status_id) DO UPDATE SET \
           quotes_count = GREATEST(status_stats.quotes_count + 1, 0), \
           updated_at = clock_timestamp()",
    )
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn increment_reblog_count(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at, reblogs_count) \
         VALUES ($1, clock_timestamp(), clock_timestamp(), 1) \
         ON CONFLICT (status_id) DO UPDATE SET \
           reblogs_count = GREATEST(status_stats.reblogs_count + 1, 0), \
           untrusted_reblogs_count = CASE \
             WHEN status_stats.untrusted_reblogs_count IS NULL THEN NULL \
             ELSE GREATEST(status_stats.untrusted_reblogs_count + 1, 0) END, \
           updated_at = clock_timestamp()",
    )
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

struct AccountStatsInitialization {
    statuses_count: i64,
    inserted: bool,
}

const ACCOUNT_STATS_SNAPSHOT_SQL: &str = "\
    (SELECT count(*) FROM statuses \
      WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3) AS statuses_count, \
    (SELECT count(*) FROM follows WHERE account_id = $1) AS following_count, \
    (SELECT count(*) FROM follows WHERE target_account_id = $1) AS followers_count, \
    (SELECT max(LEAST(created_at, clock_timestamp())) FROM statuses \
      WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3) AS last_status_at";

async fn account_stats_snapshot(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(i64, i64, i64, Option<NaiveDateTime>), WriteError> {
    let query = format!("SELECT {ACCOUNT_STATS_SNAPSHOT_SQL}");
    Ok(sqlx::query_as(&query)
        .bind(account_id)
        .fetch_one(&mut **transaction)
        .await?)
}

// Existing counters remain authoritative. The unique account_id conflict makes
// concurrent initializers preserve the winner's snapshot and all later deltas.
async fn initialize_account_stats_if_missing(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<AccountStatsInitialization, WriteError> {
    const LOCK_COUNT: &str =
        "SELECT statuses_count FROM account_stats WHERE account_id = $1 FOR UPDATE";
    if let Some(statuses_count) = sqlx::query_scalar::<_, i64>(LOCK_COUNT)
        .bind(account_id)
        .fetch_optional(&mut **transaction)
        .await?
    {
        return Ok(AccountStatsInitialization {
            statuses_count,
            inserted: false,
        });
    }

    // Keep the live snapshot and insert in one PostgreSQL statement. This makes
    // the inserted baseline correspond to one MVCC snapshot; a concurrent winner
    // remains authoritative and its caller applies its own normal delta.
    let insert = format!(
        "INSERT INTO account_stats (account_id, statuses_count, last_status_at, \
                                     following_count, followers_count, created_at, updated_at) \
         SELECT account.id, snapshot.statuses_count, snapshot.last_status_at, \
                snapshot.following_count, snapshot.followers_count, \
                clock_timestamp(), clock_timestamp() \
         FROM accounts account \
         CROSS JOIN LATERAL (SELECT {ACCOUNT_STATS_SNAPSHOT_SQL}) snapshot \
         WHERE account.id = $1 \
         ON CONFLICT (account_id) DO NOTHING"
    );
    let inserted = sqlx::query(&insert)
        .bind(account_id)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
        == 1;
    // A concurrent initializer may have won the unique-key conflict. Read and
    // lock its actual row in a new statement; never reset it. A concurrently
    // deleted account legitimately leaves no row to lock.
    let statuses_count = sqlx::query_scalar::<_, i64>(LOCK_COUNT)
        .bind(account_id)
        .fetch_optional(&mut **transaction)
        .await?
        .unwrap_or_default();
    Ok(AccountStatsInitialization {
        statuses_count,
        inserted,
    })
}

async fn lock_account_statuses_count(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<i64, WriteError> {
    Ok(initialize_account_stats_if_missing(transaction, account_id)
        .await?
        .statuses_count)
}

async fn ensure_account_stats_after_mutation(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    let mut deltas = HashMap::new();
    add_account_stats_delta(&mut deltas, account_id, AccountStatsDelta::default());
    apply_account_stats_deltas(transaction, deltas).await
}

async fn increment_account_status_count(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    status_created_at: NaiveDateTime,
) -> Result<(), WriteError> {
    let mut deltas = HashMap::new();
    add_account_stats_delta(
        &mut deltas,
        account_id,
        AccountStatsDelta {
            statuses: 1,
            last_status_at: Some(status_created_at),
            ..AccountStatsDelta::default()
        },
    );
    apply_account_stats_deltas(transaction, deltas).await
}

async fn decrement_account_status_count(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    let mut deltas = HashMap::new();
    add_account_stats_delta(
        &mut deltas,
        account_id,
        AccountStatsDelta {
            statuses: -1,
            ..AccountStatsDelta::default()
        },
    );
    apply_account_stats_deltas(transaction, deltas).await
}

async fn increment_reply_count(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at, replies_count) \
         VALUES ($1, clock_timestamp(), clock_timestamp(), 1) \
         ON CONFLICT (status_id) DO UPDATE SET \
           replies_count = GREATEST(status_stats.replies_count + 1, 0), \
           updated_at = clock_timestamp()",
    )
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn decrement_reply_count(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE status_stats SET replies_count = GREATEST(replies_count - 1, 0),
            updated_at = clock_timestamp() WHERE status_id = $1",
    )
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn decrement_reblog_count(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE status_stats SET \
           reblogs_count = GREATEST(reblogs_count - 1, 0), \
           untrusted_reblogs_count = CASE \
             WHEN untrusted_reblogs_count IS NULL THEN NULL \
             ELSE GREATEST(untrusted_reblogs_count - 1, 0) END, \
           updated_at = clock_timestamp() \
         WHERE status_id = $1",
    )
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn reconcile_notification_requests(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    from_account_ids: &[i64],
) -> Result<(), WriteError> {
    if from_account_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "DELETE FROM notification_requests request \
         WHERE request.account_id = $1 AND request.from_account_id = ANY($2) \
           AND NOT EXISTS ( \
             SELECT 1 FROM notifications notification \
             WHERE notification.account_id = request.account_id \
               AND notification.from_account_id = request.from_account_id \
               AND notification.filtered = true \
               AND notification.type IN ('mention', 'quote'))",
    )
    .bind(account_id)
    .bind(from_account_ids)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE notification_requests request SET \
           notifications_count = ( \
             SELECT count(*) FROM ( \
               SELECT 1 FROM notifications notification \
               WHERE notification.account_id = request.account_id \
                 AND notification.from_account_id = request.from_account_id \
                 AND notification.filtered = true \
                 AND notification.type IN ('mention', 'quote') LIMIT 100 \
             ) meaningful), updated_at = clock_timestamp() \
         WHERE request.account_id = $1 AND request.from_account_id = ANY($2)",
    )
    .bind(account_id)
    .bind(from_account_ids)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn update_marker_in(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: i64,
    timeline: &str,
    last_read_id: i64,
    expected_lock_version: Option<i32>,
) -> Result<Marker, WriteError> {
    if let Some(expected_lock_version) = expected_lock_version {
        return sqlx::query_as::<_, Marker>(
            "UPDATE markers SET last_read_id = $1, lock_version = lock_version + 1, \
             updated_at = clock_timestamp() \
             WHERE user_id = $2 AND timeline = $3 AND lock_version = $4 \
             RETURNING timeline, last_read_id, lock_version, updated_at",
        )
        .bind(last_read_id)
        .bind(user_id)
        .bind(timeline)
        .bind(expected_lock_version)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(WriteError::Conflict);
    }

    if let Some(marker) = sqlx::query_as::<_, Marker>(
        "INSERT INTO markers (user_id, timeline, last_read_id, lock_version, created_at, updated_at) \
         VALUES ($1, $2, $3, 0, clock_timestamp(), clock_timestamp()) \
         ON CONFLICT (user_id, timeline) DO NOTHING \
         RETURNING timeline, last_read_id, lock_version, updated_at",
    )
    .bind(user_id)
    .bind(timeline)
    .bind(last_read_id)
    .fetch_optional(&mut **transaction)
    .await?
    {
        if last_read_id == 0 {
            return Ok(marker);
        }
        return sqlx::query_as::<_, Marker>(
            "UPDATE markers SET last_read_id = $1, lock_version = lock_version + 1, \
             updated_at = clock_timestamp() \
             WHERE user_id = $2 AND timeline = $3 AND lock_version = 0 \
             RETURNING timeline, last_read_id, lock_version, updated_at",
        )
        .bind(last_read_id)
        .bind(user_id)
        .bind(timeline)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(WriteError::Conflict);
    }

    let current_lock_version = sqlx::query_scalar::<_, i32>(
        "SELECT lock_version FROM markers \
         WHERE user_id = $1 AND timeline = $2 FOR UPDATE",
    )
    .bind(user_id)
    .bind(timeline)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::Conflict)?;
    sqlx::query_as::<_, Marker>(
        "UPDATE markers SET last_read_id = $1, lock_version = lock_version + 1, \
         updated_at = clock_timestamp() \
         WHERE user_id = $2 AND timeline = $3 AND lock_version = $4 \
         RETURNING timeline, last_read_id, lock_version, updated_at",
    )
    .bind(last_read_id)
    .bind(user_id)
    .bind(timeline)
    .bind(current_lock_version)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::Conflict)
}

async fn select_marker(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: i64,
    timeline: &str,
) -> Result<Option<Marker>, WriteError> {
    Ok(sqlx::query_as::<_, Marker>(
        "SELECT timeline, last_read_id, lock_version, updated_at FROM markers \
         WHERE user_id = $1 AND timeline = $2 FOR UPDATE",
    )
    .bind(user_id)
    .bind(timeline)
    .fetch_optional(&mut **transaction)
    .await?)
}

fn validate_idempotency(key: Option<IdempotencyKey<'_>>) -> Result<(), WriteError> {
    if let Some(key) = key {
        if !(1..=255).contains(&key.scope.len()) {
            return Err(WriteError::InvalidInput(
                "idempotency scope must contain 1-255 bytes",
            ));
        }
        if !(1..=255).contains(&key.key.len()) {
            return Err(WriteError::InvalidInput(
                "idempotency key must contain 1-255 bytes",
            ));
        }
        if key.expires_at <= Utc::now() {
            return Err(WriteError::InvalidInput(
                "idempotency key expiration must be in the future",
            ));
        }
    }
    Ok(())
}

async fn claim_idempotency(
    transaction: &mut Transaction<'_, Postgres>,
    key: IdempotencyKey<'_>,
) -> Result<bool, WriteError> {
    sqlx::query(
        "DELETE FROM rustodon.idempotency_keys \
         WHERE scope = $1 AND key = $2 AND expires_at <= clock_timestamp()",
    )
    .bind(key.scope)
    .bind(key.key)
    .execute(&mut **transaction)
    .await?;
    let inserted = sqlx::query_scalar::<_, String>(
        "INSERT INTO rustodon.idempotency_keys \
         (scope, key, fingerprint, result, expires_at) \
         VALUES ($1, $2, $3, '{}'::jsonb, $4) \
         ON CONFLICT (scope, key) DO NOTHING \
         RETURNING scope",
    )
    .bind(key.scope)
    .bind(key.key)
    .bind(key.fingerprint.as_slice())
    .bind(key.expires_at)
    .fetch_optional(&mut **transaction)
    .await?;
    if inserted.is_some() {
        return Ok(false);
    }

    let fingerprint = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT fingerprint FROM rustodon.idempotency_keys \
         WHERE scope = $1 AND key = $2 FOR UPDATE",
    )
    .bind(key.scope)
    .bind(key.key)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::Conflict)?;
    if fingerprint.as_slice() != key.fingerprint {
        return Err(WriteError::Conflict);
    }
    Ok(true)
}

async fn complete_idempotency(
    transaction: &mut Transaction<'_, Postgres>,
    key: IdempotencyKey<'_>,
    marker: &Marker,
) -> Result<(), WriteError> {
    let result = json!({
        "operation": "marker",
        "timeline": marker.timeline,
        "last_read_id": marker.last_read_id,
        "lock_version": marker.lock_version,
    });
    let updated = sqlx::query(
        "UPDATE rustodon.idempotency_keys SET result = $3 \
         WHERE scope = $1 AND key = $2",
    )
    .bind(key.scope)
    .bind(key.key)
    .bind(result)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(WriteError::Conflict);
    }
    Ok(())
}

async fn complete_status_idempotency(
    transaction: &mut Transaction<'_, Postgres>,
    key: IdempotencyKey<'_>,
    status_id: i64,
) -> Result<(), WriteError> {
    let updated = sqlx::query(
        "UPDATE rustodon.idempotency_keys SET result = jsonb_build_object('status_id', $3) \
         WHERE scope = $1 AND key = $2",
    )
    .bind(key.scope)
    .bind(key.key)
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(WriteError::Conflict);
    }
    Ok(())
}

async fn authorized_admin_account(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    permission_mask: i64,
) -> Result<Option<i32>, WriteError> {
    sqlx::query_scalar::<_, i32>(
        "SELECT role.position FROM users account_user \
         JOIN accounts account ON account.id = account_user.account_id \
         JOIN user_roles role ON role.id = COALESCE(account_user.role_id, -99) \
         JOIN user_roles everyone ON everyone.id = -99 \
         WHERE account.id = $1 AND account.domain IS NULL \
           AND account.suspended_at IS NULL \
           AND account_user.confirmed_at IS NOT NULL \
           AND account_user.approved = true \
           AND account_user.disabled = false \
            AND (role.permissions & 1 <> 0 OR \
                 ((role.permissions | everyone.permissions) & $2 <> 0))",
    )
    .bind(account_id)
    .bind(permission_mask)
    .fetch_one(&mut **transaction)
    .await
    .map(Some)
    .or_else(|error| match error {
        sqlx::Error::RowNotFound => Ok(None),
        error => Err(WriteError::Sqlx(error)),
    })
}

fn account_update_job(account_id: i64, updated_at: NaiveDateTime) -> JobSpec {
    let updated_at_micros = updated_at.and_utc().timestamp_micros();
    JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
        json!({
            "account_id": account_id,
            "updated_at_micros": updated_at_micros
        }),
    )
    .logical_key(format!(
        "activitypub:account:{account_id}:update:{updated_at_micros}"
    ))
}

fn account_delete_job(account_id: i64, actor_uri: &str) -> JobSpec {
    JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND,
        json!({
            "account_id": account_id,
            "actor_uri": actor_uri,
        }),
    )
    .logical_key(format!("activitypub:account:{account_id}:delete"))
}

fn account_purge_job(
    account_id: i64,
    deletion_request_id: i64,
    created_at: NaiveDateTime,
    origin: Option<&str>,
) -> JobSpec {
    let run_at = created_at.and_utc() + ChronoDuration::days(ACCOUNT_DELETION_DELAY_DAYS);
    let mut arguments = json!({
        "account_id": account_id,
        "deletion_request_id": deletion_request_id,
        "deletion_created_at_micros": created_at.and_utc().timestamp_micros(),
    });
    if let Some(origin) = origin {
        arguments["origin"] = json!(origin);
    }
    JobSpec::new(
        Lane::Maintenance,
        MASTODON_ACCOUNT_PURGE_JOB_KIND,
        arguments,
    )
    .run_at(run_at)
    .logical_key(format!("mastodon:account:{account_id}:purge"))
}

fn domain_block_job(
    domain_block_id: i64,
    severance_event_id: Option<i64>,
    updated_at: NaiveDateTime,
    origin: &str,
) -> JobSpec {
    let updated_at_micros = updated_at.and_utc().timestamp_micros();
    JobSpec::new(
        Lane::Maintenance,
        MASTODON_DOMAIN_BLOCK_JOB_KIND,
        json!({
            "domain_block_id": domain_block_id,
            "severance_event_id": severance_event_id,
            "origin": origin,
        }),
    )
    .logical_key(format!(
        "mastodon:domain-block:{domain_block_id}:{updated_at_micros}"
    ))
}

fn domain_purge_job(domain: &str) -> JobSpec {
    JobSpec::new(
        Lane::Maintenance,
        MASTODON_DOMAIN_PURGE_JOB_KIND,
        json!({"domain": domain}),
    )
    .logical_key(format!("mastodon:domain-purge:{domain}"))
}

async fn protected_status_ids(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<Vec<i64>, WriteError> {
    let status_ids = sqlx::query_scalar::<_, String>(
        "SELECT status_id FROM (
           SELECT unnest(report.status_ids)::text AS status_id
             FROM reports report
            WHERE report.target_account_id = $1 AND report.action_taken_at IS NULL
           UNION
           SELECT warning_status_id
             FROM account_warnings warning
             CROSS JOIN LATERAL unnest(
                 COALESCE(warning.status_ids, ARRAY[]::varchar[])
             ) AS warning_status(warning_status_id)
            WHERE warning.target_account_id = $1 AND warning.overruled_at IS NULL
         ) protected
        WHERE status_id ~ '^[0-9]+$'
        ORDER BY status_id",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(status_ids
        .into_iter()
        .filter_map(|status_id| status_id.parse::<i64>().ok())
        .collect())
}

async fn purge_remote_account(
    transaction: &mut Transaction<'_, Postgres>,
    pending_stream_events: &mut Vec<PendingStreamEvent>,
    account_id: i64,
) -> Result<(), WriteError> {
    let is_remote = sqlx::query_scalar::<_, bool>(
        "SELECT domain IS NOT NULL FROM accounts WHERE id = $1 FOR UPDATE",
    )
    .bind(account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or(false);
    if !is_remote {
        return Ok(());
    }
    cancel_pending_account_job(transaction, ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND, account_id)
        .await?;
    cancel_pending_account_job(transaction, ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND, account_id)
        .await?;
    purge_account_statuses(transaction, pending_stream_events, account_id, &[], true).await?;
    purge_account_mentions(transaction, account_id, &[]).await?;
    purge_account_media(transaction, account_id, &[]).await?;
    purge_account_relationships(transaction, account_id).await?;
    purge_account_notifications(transaction, account_id).await?;
    purge_remote_account_activity_notifications(transaction, account_id).await?;
    purge_account_associations(transaction, account_id).await?;
    purge_remote_account_non_cascading_associations(transaction, account_id).await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM accounts WHERE id = $1 AND domain IS NOT NULL")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn purge_account_user(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE users SET disabled = true, updated_at = clock_timestamp()
          WHERE account_id = $1",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM invites
          WHERE user_id IN (SELECT id FROM users WHERE account_id = $1)
            AND uses = 0",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn purge_account_profile(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE accounts SET silenced_at = NULL,
            suspended_at = COALESCE(suspended_at, clock_timestamp()),
            suspension_origin = 0, locked = false, memorial = false,
            discoverable = false, trendable = false, display_name = '', note = '',
            fields = '[]'::jsonb, also_known_as = ARRAY[]::varchar[],
            moved_to_account_id = NULL, reviewed_at = NULL, requested_review_at = NULL,
            avatar_content_type = NULL, avatar_description = '', avatar_file_name = NULL,
            avatar_file_size = NULL, avatar_remote_url = NULL,
            avatar_storage_schema_version = NULL, avatar_updated_at = NULL,
            header_content_type = NULL, header_description = '', header_file_name = NULL,
            header_file_size = NULL, header_remote_url = '',
            header_storage_schema_version = NULL, header_updated_at = NULL,
            updated_at = clock_timestamp()
          WHERE id = $1",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn account_media_metadata_for_cleanup(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    remote: bool,
    protected_status_ids: &[i64],
) -> Result<Vec<PaperclipMetadata>, WriteError> {
    let Some((
        avatar_storage_schema_version,
        avatar_file_name,
        avatar_content_type,
        header_storage_schema_version,
        header_file_name,
        header_content_type,
    )) = sqlx::query_as::<
        _,
        (
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT avatar_storage_schema_version, avatar_file_name, avatar_content_type,
                header_storage_schema_version, header_file_name, header_content_type
           FROM accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(Vec::new());
    };
    let mut metadata = Vec::new();
    if let Some(file_name) = avatar_file_name.filter(|name| !name.is_empty()) {
        metadata.push(PaperclipMetadata {
            attachment: PaperclipAttachment::AccountAvatar,
            id: account_id,
            remote,
            storage_schema_version: avatar_storage_schema_version,
            file_name,
            content_type: avatar_content_type,
            variant: None,
        });
    }
    if let Some(file_name) = header_file_name.filter(|name| !name.is_empty()) {
        metadata.push(PaperclipMetadata {
            attachment: PaperclipAttachment::AccountHeader,
            id: account_id,
            remote,
            storage_schema_version: header_storage_schema_version,
            file_name,
            content_type: header_content_type,
            variant: None,
        });
    }
    for (
        media_id,
        file_storage_schema_version,
        file_file_name,
        file_content_type,
        remote_url,
        thumbnail_storage_schema_version,
        thumbnail_file_name,
        thumbnail_content_type,
        thumbnail_remote_url,
    ) in sqlx::query_as::<
        _,
        (
            i64,
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT id, file_storage_schema_version, file_file_name, file_content_type,
                remote_url, thumbnail_storage_schema_version, thumbnail_file_name,
                thumbnail_content_type, thumbnail_remote_url
           FROM media_attachments
          WHERE account_id = $1
            AND (status_id IS NULL OR status_id <> ALL($2::bigint[]))
          ORDER BY id",
    )
    .bind(account_id)
    .bind(protected_status_ids)
    .fetch_all(&mut **transaction)
    .await?
    {
        append_media_attachment_metadata(
            &mut metadata,
            media_id,
            file_storage_schema_version,
            file_file_name,
            file_content_type,
            remote_url,
            thumbnail_storage_schema_version,
            thumbnail_file_name,
            thumbnail_content_type,
            thumbnail_remote_url,
        );
    }
    Ok(metadata)
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
fn append_media_attachment_metadata(
    metadata: &mut Vec<PaperclipMetadata>,
    media_id: i64,
    file_storage_schema_version: Option<i32>,
    file_file_name: Option<String>,
    file_content_type: Option<String>,
    remote_url: Option<String>,
    thumbnail_storage_schema_version: Option<i32>,
    thumbnail_file_name: Option<String>,
    thumbnail_content_type: Option<String>,
    thumbnail_remote_url: Option<String>,
) {
    if let Some(file_name) = file_file_name.filter(|name| !name.is_empty()) {
        metadata.push(PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: media_id,
            remote: !remote_url.as_deref().is_none_or(rails_blank),
            storage_schema_version: file_storage_schema_version,
            file_name,
            content_type: file_content_type,
            variant: None,
        });
    }
    if let Some(file_name) = thumbnail_file_name.filter(|name| !name.is_empty()) {
        metadata.push(PaperclipMetadata {
            attachment: PaperclipAttachment::MediaThumbnail,
            id: media_id,
            remote: !thumbnail_remote_url.as_deref().is_none_or(rails_blank),
            storage_schema_version: thumbnail_storage_schema_version,
            file_name,
            content_type: thumbnail_content_type,
            variant: None,
        });
    }
}

#[allow(clippy::too_many_lines)]
async fn domain_media_metadata(
    pool: &PgPool,
    domain: &str,
    include_subdomains: bool,
) -> Result<Vec<PaperclipMetadata>, WriteError> {
    let account_query = if include_subdomains {
        "SELECT id, avatar_storage_schema_version, avatar_file_name, avatar_content_type,
                header_storage_schema_version, header_file_name, header_content_type
           FROM accounts
          WHERE domain IS NOT NULL
            AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                     THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1)
              OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                     THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1))"
    } else {
        "SELECT id, avatar_storage_schema_version, avatar_file_name, avatar_content_type,
                header_storage_schema_version, header_file_name, header_content_type
           FROM accounts
          WHERE domain IS NOT NULL AND lower(domain) = lower($1)"
    };
    let mut metadata = Vec::new();
    for (
        account_id,
        avatar_storage_schema_version,
        avatar_file_name,
        avatar_content_type,
        header_storage_schema_version,
        header_file_name,
        header_content_type,
    ) in sqlx::query_as::<
        _,
        (
            i64,
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<String>,
        ),
    >(account_query)
    .bind(domain)
    .fetch_all(pool)
    .await?
    {
        if let Some(file_name) = avatar_file_name.filter(|name| !name.is_empty()) {
            metadata.push(PaperclipMetadata {
                attachment: PaperclipAttachment::AccountAvatar,
                id: account_id,
                remote: true,
                storage_schema_version: avatar_storage_schema_version,
                file_name,
                content_type: avatar_content_type,
                variant: None,
            });
        }
        if let Some(file_name) = header_file_name.filter(|name| !name.is_empty()) {
            metadata.push(PaperclipMetadata {
                attachment: PaperclipAttachment::AccountHeader,
                id: account_id,
                remote: true,
                storage_schema_version: header_storage_schema_version,
                file_name,
                content_type: header_content_type,
                variant: None,
            });
        }
    }

    let media_query = if include_subdomains {
        "SELECT media.id, media.file_storage_schema_version, media.file_file_name,
                media.file_content_type, media.thumbnail_storage_schema_version,
                media.thumbnail_file_name, media.thumbnail_content_type
           FROM media_attachments media
           JOIN accounts account ON account.id = media.account_id
          WHERE account.domain IS NOT NULL
            AND (lower(trim(trailing '.' FROM (CASE WHEN left(account.domain, 1) = '['
                     THEN split_part(account.domain, ']', 1) || ']' ELSE split_part(account.domain, ':', 1) END))) = lower($1)
              OR lower(trim(trailing '.' FROM (CASE WHEN left(account.domain, 1) = '['
                     THEN split_part(account.domain, ']', 1) || ']' ELSE split_part(account.domain, ':', 1) END))) LIKE '%.' || lower($1))"
    } else {
        "SELECT media.id, media.file_storage_schema_version, media.file_file_name,
                media.file_content_type, media.thumbnail_storage_schema_version,
                media.thumbnail_file_name, media.thumbnail_content_type
           FROM media_attachments media
           JOIN accounts account ON account.id = media.account_id
          WHERE account.domain IS NOT NULL AND lower(account.domain) = lower($1)"
    };
    for (
        media_id,
        file_storage_schema_version,
        file_file_name,
        file_content_type,
        thumbnail_storage_schema_version,
        thumbnail_file_name,
        thumbnail_content_type,
    ) in sqlx::query_as::<
        _,
        (
            i64,
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<String>,
        ),
    >(media_query)
    .bind(domain)
    .fetch_all(pool)
    .await?
    {
        if let Some(file_name) = file_file_name.filter(|name| !name.is_empty()) {
            metadata.push(PaperclipMetadata {
                attachment: PaperclipAttachment::MediaFile,
                id: media_id,
                remote: true,
                storage_schema_version: file_storage_schema_version,
                file_name,
                content_type: file_content_type,
                variant: None,
            });
        }
        if let Some(file_name) = thumbnail_file_name.filter(|name| !name.is_empty()) {
            metadata.push(PaperclipMetadata {
                attachment: PaperclipAttachment::MediaThumbnail,
                id: media_id,
                remote: true,
                storage_schema_version: thumbnail_storage_schema_version,
                file_name,
                content_type: thumbnail_content_type,
                variant: None,
            });
        }
    }

    let emoji_query = if include_subdomains {
        "SELECT id, image_storage_schema_version, image_file_name, image_content_type
           FROM custom_emojis
          WHERE domain IS NOT NULL
            AND (lower(trim(trailing '.' FROM domain)) = lower($1)
              OR lower(trim(trailing '.' FROM domain)) LIKE '%.' || lower($1))"
    } else {
        "SELECT id, image_storage_schema_version, image_file_name, image_content_type
           FROM custom_emojis
          WHERE domain IS NOT NULL AND lower(domain) = lower($1)"
    };
    for (emoji_id, storage_schema_version, file_name, content_type) in
        sqlx::query_as::<_, (i64, Option<i32>, Option<String>, Option<String>)>(emoji_query)
            .bind(domain)
            .fetch_all(pool)
            .await?
    {
        if let Some(file_name) = file_name.filter(|name| !name.is_empty()) {
            metadata.push(PaperclipMetadata {
                attachment: PaperclipAttachment::CustomEmojiImage,
                id: emoji_id,
                remote: true,
                storage_schema_version,
                file_name,
                content_type,
                variant: None,
            });
        }
    }
    Ok(metadata)
}

#[allow(clippy::too_many_lines)]
async fn purge_account_statuses(
    transaction: &mut Transaction<'_, Postgres>,
    pending_stream_events: &mut Vec<PendingStreamEvent>,
    account_id: i64,
    protected_status_ids: &[i64],
    emit_stream_events: bool,
) -> Result<(), WriteError> {
    let statuses = sqlx::query_as::<_, (i64, i64, Option<i64>, Option<i64>, i32)>(
        "WITH RECURSIVE owned AS (
           SELECT id, account_id, reblog_of_id, in_reply_to_id, visibility
             FROM statuses
            WHERE account_id = $1 AND deleted_at IS NULL
              AND id <> ALL($2::bigint[])
         ), affected_ids(id) AS (
           SELECT id FROM owned
           UNION
           SELECT child.id
             FROM statuses child
             JOIN affected_ids parent ON parent.id = child.reblog_of_id
            WHERE child.deleted_at IS NULL
         )
         SELECT status_row.id, status_row.account_id, status_row.reblog_of_id,
                status_row.in_reply_to_id, status_row.visibility
           FROM statuses status_row
           JOIN affected_ids ON affected_ids.id = status_row.id
          ORDER BY status_row.id
          FOR UPDATE OF status_row",
    )
    .bind(account_id)
    .bind(protected_status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let status_ids = statuses
        .iter()
        .map(|(status_id, ..)| *status_id)
        .collect::<Vec<_>>();
    let affected_quotes = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT id, activity_uri FROM quotes \
          WHERE status_id = ANY($1::bigint[]) OR quoted_status_id = ANY($1::bigint[]) \
          ORDER BY id FOR UPDATE",
    )
    .bind(&status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    for (quote_id, request_uri) in affected_quotes {
        cancel_quote_request_outbox(transaction, quote_id, request_uri.as_deref()).await?;
    }
    let mut timeline_snapshots = if emit_stream_events {
        status_timeline_snapshots(transaction, &status_ids).await?
    } else {
        HashMap::new()
    };
    let accepted_quote_targets = sqlx::query_scalar::<_, i64>(
        "SELECT quoted_status_id FROM quotes
           WHERE status_id = ANY($1::bigint[])
             AND state = 1
             AND quoted_status_id IS NOT NULL
             AND NOT (quoted_status_id = ANY($2::bigint[]))",
    )
    .bind(&status_ids)
    .bind(&status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let mut accepted_quote_counts = HashMap::new();
    for quoted_status_id in accepted_quote_targets {
        accepted_quote_counts
            .entry(quoted_status_id)
            .and_modify(|count: &mut i64| *count = count.saturating_add(1))
            .or_insert(1_i64);
    }

    delete_remote_status_notifications(transaction, &status_ids).await?;
    remove_favourites_for_account_and_statuses(transaction, account_id, &status_ids).await?;
    remove_poll_data_for_account_and_statuses(
        transaction,
        account_id,
        &status_ids,
        protected_status_ids,
    )
    .await?;
    remove_statuses_from_account_conversations(transaction, &status_ids).await?;
    for status_id in &status_ids {
        cancel_status_outbox(transaction, *status_id).await?;
        cancel_quote_decision_outbox_for_target(transaction, *status_id).await?;
        if emit_stream_events && let Some(snapshot) = timeline_snapshots.remove(status_id) {
            collect_status_delete_stream_events_with_snapshot(
                transaction,
                pending_stream_events,
                *status_id,
                snapshot,
            )
            .await?;
        }
    }
    if !status_ids.is_empty() {
        sqlx::query("DELETE FROM media_attachments WHERE status_id = ANY($1::bigint[])")
            .bind(&status_ids)
            .execute(&mut **transaction)
            .await?;
        sqlx::query("DELETE FROM status_pins WHERE status_id = ANY($1::bigint[])")
            .bind(&status_ids)
            .execute(&mut **transaction)
            .await?;
        sqlx::query("DELETE FROM bookmarks WHERE status_id = ANY($1::bigint[])")
            .bind(&status_ids)
            .execute(&mut **transaction)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id = ANY($1::bigint[])")
            .bind(&status_ids)
            .execute(&mut **transaction)
            .await?;
    }
    let mut status_deltas = HashMap::new();
    for (_, status_account_id, reblog_of_id, in_reply_to_id, visibility) in &statuses {
        if *visibility != 3 {
            add_account_stats_delta(
                &mut status_deltas,
                *status_account_id,
                AccountStatsDelta {
                    statuses: -1,
                    ..AccountStatsDelta::default()
                },
            );
        }
        if let Some(reblog_of_id) = reblog_of_id {
            decrement_reblog_count(transaction, *reblog_of_id).await?;
        } else if *visibility < 2
            && let Some(in_reply_to_id) = in_reply_to_id
        {
            decrement_reply_count(transaction, *in_reply_to_id).await?;
        }
    }
    apply_account_stats_deltas(transaction, status_deltas).await?;
    for (quoted_status_id, quote_count) in accepted_quote_counts {
        sqlx::query(
            "UPDATE status_stats
                SET quotes_count = GREATEST(0, quotes_count - $2),
                    updated_at = clock_timestamp()
              WHERE status_id = $1",
        )
        .bind(quoted_status_id)
        .bind(quote_count)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn purge_account_mentions(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    protected_status_ids: &[i64],
) -> Result<(), WriteError> {
    sqlx::query(
        "DELETE FROM mentions
          WHERE account_id = $1 AND status_id <> ALL($2::bigint[])",
    )
    .bind(account_id)
    .bind(protected_status_ids)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn purge_account_media(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    protected_status_ids: &[i64],
) -> Result<(), WriteError> {
    sqlx::query(
        "DELETE FROM media_attachments
          WHERE account_id = $1
            AND (status_id IS NULL OR status_id <> ALL($2::bigint[]))",
    )
    .bind(account_id)
    .bind(protected_status_ids)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn purge_account_relationships(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    let follows = sqlx::query_as::<_, (i64, i64, i64, Option<String>)>(
        "SELECT id, account_id, target_account_id, uri
           FROM follows
          WHERE account_id = $1 OR target_account_id = $1
          ORDER BY account_id, target_account_id
          FOR UPDATE",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let follow_requests = sqlx::query_as::<_, (i64, i64, Option<String>)>(
        "SELECT id, target_account_id, uri
           FROM follow_requests
          WHERE account_id = $1 OR target_account_id = $1
          ORDER BY account_id, target_account_id
          FOR UPDATE",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let blocks = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT id, uri FROM blocks
          WHERE account_id = $1 OR target_account_id = $1
          ORDER BY id FOR UPDATE",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mutes = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM mutes
          WHERE account_id = $1 OR target_account_id = $1
          ORDER BY id FOR UPDATE",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (_, _, _, uri) in &follows {
        if let Some(uri) = uri.as_deref().filter(|uri| !uri.is_empty()) {
            cancel_activitypub_delivery(transaction, uri).await?;
        }
    }
    for (_, _, uri) in &follow_requests {
        if let Some(uri) = uri.as_deref().filter(|uri| !uri.is_empty()) {
            cancel_activitypub_delivery(transaction, uri).await?;
        }
    }
    for (_, uri) in &blocks {
        if let Some(uri) = uri.as_deref().filter(|uri| !uri.is_empty()) {
            cancel_activitypub_delivery(transaction, uri).await?;
        }
    }
    for mute_id in &mutes {
        cancel_pending_mute_expiry_events(transaction, *mute_id).await?;
    }
    sqlx::query("DELETE FROM follows WHERE account_id = $1 OR target_account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 OR target_account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    let mut relationship_deltas = HashMap::new();
    for (_, source_account_id, target_account_id, _) in &follows {
        add_account_stats_delta(
            &mut relationship_deltas,
            *source_account_id,
            AccountStatsDelta {
                following: -1,
                ..AccountStatsDelta::default()
            },
        );
        add_account_stats_delta(
            &mut relationship_deltas,
            *target_account_id,
            AccountStatsDelta {
                followers: -1,
                ..AccountStatsDelta::default()
            },
        );
    }
    apply_account_stats_deltas(transaction, relationship_deltas).await?;
    for (follow_id, _, target_account_id, _) in &follows {
        delete_activity_notifications(transaction, *target_account_id, *follow_id, "Follow")
            .await?;
    }
    for (request_id, target_account_id, _) in follow_requests {
        delete_activity_notifications(transaction, target_account_id, request_id, "FollowRequest")
            .await?;
    }
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 OR target_account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM mutes WHERE account_id = $1 OR target_account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn purge_account_notifications(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    let notification_keys = sqlx::query_as::<_, (i64, i64, String)>(
        "SELECT DISTINCT notification.account_id, notification.activity_id,
                notification.activity_type
           FROM notifications notification
          WHERE notification.account_id = $1 OR notification.from_account_id = $1
          ORDER BY notification.account_id, notification.activity_id,
                   notification.activity_type",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (recipient_account_id, activity_id, activity_type) in notification_keys {
        delete_activity_notifications(
            transaction,
            recipient_account_id,
            activity_id,
            &activity_type,
        )
        .await?;
    }
    sqlx::query("DELETE FROM notifications WHERE account_id = $1 OR from_account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "DELETE FROM notification_requests
          WHERE account_id = $1 OR from_account_id = $1",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn purge_remote_account_activity_notifications(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    let notification_keys = sqlx::query_as::<_, (i64, i64, String)>(
        "WITH activities(activity_id, activity_type) AS (
             SELECT id, 'Account'::text FROM accounts WHERE id = $1
             UNION
             SELECT id, 'Status'::text FROM statuses WHERE account_id = $1
             UNION
             SELECT id, 'Mention'::text FROM mentions WHERE account_id = $1
             UNION
             SELECT id, 'Favourite'::text FROM favourites WHERE account_id = $1
             UNION
             SELECT id, 'Poll'::text FROM polls WHERE account_id = $1
             UNION
             SELECT id, 'Quote'::text FROM quotes WHERE account_id = $1
             UNION
             SELECT id, 'AccountRelationshipSeveranceEvent'::text
               FROM account_relationship_severance_events WHERE account_id = $1
             UNION
             SELECT id, 'AccountWarning'::text FROM account_warnings
              WHERE account_id = $1 OR target_account_id = $1
             UNION
             SELECT id, 'GeneratedAnnualReport'::text
               FROM generated_annual_reports WHERE account_id = $1
             UNION
             SELECT id, 'Report'::text FROM reports
              WHERE account_id = $1 OR target_account_id = $1
             UNION
             SELECT id, 'CollectionItem'::text FROM collection_items
              WHERE account_id = $1 OR collection_id IN (
                  SELECT id FROM collections WHERE account_id = $1)
             UNION
             SELECT id, 'Collection'::text FROM collections WHERE account_id = $1
         )
         SELECT DISTINCT notification.account_id, notification.activity_id,
                         notification.activity_type
           FROM notifications notification
           JOIN activities activity
             ON activity.activity_id = notification.activity_id
            AND activity.activity_type = notification.activity_type
          ORDER BY notification.account_id, notification.activity_type,
                   notification.activity_id",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (recipient_account_id, activity_id, activity_type) in notification_keys {
        delete_activity_notifications(
            transaction,
            recipient_account_id,
            activity_id,
            &activity_type,
        )
        .await?;
    }
    Ok(())
}

async fn purge_remote_account_non_cascading_associations(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    for query in [
        "DELETE FROM generated_annual_reports WHERE account_id = $1",
        "DELETE FROM fasp_follow_recommendations
           WHERE requesting_account_id = $1 OR recommended_account_id = $1",
    ] {
        sqlx::query(query)
            .bind(account_id)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

async fn purge_account_associations(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    for query in [
        "DELETE FROM account_notes WHERE account_id = $1",
        "DELETE FROM account_pins WHERE account_id = $1",
        "DELETE FROM account_aliases WHERE account_id = $1",
        "DELETE FROM account_domain_blocks WHERE account_id = $1",
        "DELETE FROM account_migrations WHERE account_id = $1",
        "DELETE FROM featured_tags WHERE account_id = $1",
        "DELETE FROM bookmarks WHERE account_id = $1",
        "DELETE FROM report_notes WHERE account_id = $1",
        "DELETE FROM scheduled_statuses WHERE account_id = $1",
        "DELETE FROM status_pins WHERE account_id = $1",
        "DELETE FROM tag_follows WHERE account_id = $1",
        "DELETE FROM accounts_tags WHERE account_id = $1",
        "DELETE FROM account_conversations WHERE account_id = $1",
        "DELETE FROM conversation_mutes WHERE account_id = $1",
        "DELETE FROM custom_filters WHERE account_id = $1",
    ] {
        sqlx::query(query)
            .bind(account_id)
            .execute(&mut **transaction)
            .await?;
    }
    sqlx::query(
        "DELETE FROM collection_items
          WHERE account_id = $1
             OR collection_id IN (SELECT id FROM collections WHERE account_id = $1)",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM collections WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "DELETE FROM list_accounts
          WHERE account_id = $1
             OR list_id IN (SELECT id FROM lists WHERE account_id = $1)",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM lists WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn reject_remote_account_follows(
    transaction: &mut Transaction<'_, Postgres>,
    remote_account_id: i64,
    origin: &str,
) -> Result<(), WriteError> {
    let follows = sqlx::query_as::<_, (i64, i64, Option<String>)>(
        "SELECT follow_row.id, follow_row.target_account_id, follow_row.uri \
         FROM follows follow_row \
         JOIN accounts target ON target.id = follow_row.target_account_id \
         WHERE follow_row.account_id = $1 AND target.domain IS NULL \
         ORDER BY follow_row.id FOR UPDATE OF follow_row",
    )
    .bind(remote_account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let follow_ids = follows
        .iter()
        .map(|(follow_id, _, _)| *follow_id)
        .collect::<Vec<_>>();
    sqlx::query("DELETE FROM follows WHERE id = ANY($1)")
        .bind(&follow_ids)
        .execute(&mut **transaction)
        .await?;
    let mut relationship_deltas = HashMap::new();
    for (_, local_account_id, _) in &follows {
        add_account_stats_delta(
            &mut relationship_deltas,
            remote_account_id,
            AccountStatsDelta {
                following: -1,
                ..AccountStatsDelta::default()
            },
        );
        add_account_stats_delta(
            &mut relationship_deltas,
            *local_account_id,
            AccountStatsDelta {
                followers: -1,
                ..AccountStatsDelta::default()
            },
        );
    }
    apply_account_stats_deltas(transaction, relationship_deltas).await?;
    for (follow_id, local_account_id, follow_uri) in follows {
        delete_activity_notifications(transaction, local_account_id, follow_id, "Follow").await?;
        if let Some(follow_uri) = follow_uri
            && let Some(remote_delivery) = remote_relationship_delivery(
                transaction,
                local_account_id,
                remote_account_id,
                origin,
            )
            .await?
        {
            cancel_activitypub_delivery(
                transaction,
                &format!("{}#accepts/follows/{follow_id}", remote_delivery.source_uri),
            )
            .await?;
            record_remote_reject_delivery(
                transaction,
                local_account_id,
                &remote_delivery,
                follow_id,
                &follow_uri,
            )
            .await?;
        }
    }
    Ok(())
}

async fn undo_remote_account_follows(
    transaction: &mut Transaction<'_, Postgres>,
    remote_account_id: i64,
    origin: &str,
) -> Result<(), WriteError> {
    let follows = sqlx::query_as::<_, (i64, i64, Option<String>)>(
        "SELECT follow_row.id, follow_row.account_id, follow_row.uri \
           FROM follows follow_row \
           JOIN accounts source ON source.id = follow_row.account_id \
          WHERE follow_row.target_account_id = $1 AND source.domain IS NULL \
          ORDER BY follow_row.id FOR UPDATE OF follow_row",
    )
    .bind(remote_account_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (_, local_account_id, follow_uri) in follows {
        let Some(follow_uri) = follow_uri.as_deref().filter(|uri| !uri.is_empty()) else {
            continue;
        };
        if let Some(remote_delivery) =
            remote_relationship_delivery(transaction, local_account_id, remote_account_id, origin)
                .await?
        {
            cancel_activitypub_delivery(transaction, follow_uri).await?;
            record_remote_undo_follow_delivery(
                transaction,
                local_account_id,
                &remote_delivery,
                follow_uri,
                origin,
            )
            .await?;
        }
    }
    Ok(())
}

async fn insert_instance_admin_action_log(
    transaction: &mut Transaction<'_, Postgres>,
    acting_account_id: i64,
    domain: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO admin_action_logs ( \
             account_id, action, created_at, human_identifier, route_param, target_id, \
             target_type, updated_at) \
         VALUES ($1, 'destroy', clock_timestamp(), $2, $2, NULL, 'Instance', clock_timestamp())",
    )
    .bind(acting_account_id)
    .bind(domain)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_admin_action_log(
    transaction: &mut Transaction<'_, Postgres>,
    acting_account_id: i64,
    action: &str,
    target_id: i64,
    target_type: &str,
    human_identifier: String,
    route_param: Option<&str>,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO admin_action_logs ( \
             account_id, action, created_at, human_identifier, route_param, target_id, \
             target_type, updated_at) \
         VALUES ($1, $2, clock_timestamp(), $3, $4, $5, $6, clock_timestamp())",
    )
    .bind(acting_account_id)
    .bind(action)
    .bind(human_identifier)
    .bind(route_param)
    .bind(target_id)
    .bind(target_type)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn normalize_domain_block_domain(domain: &str) -> Result<String, WriteError> {
    let domain = domain.trim();
    let domain = domain.strip_suffix('/').unwrap_or(domain);
    if domain.is_empty() || domain.contains('/') {
        return Err(WriteError::InvalidInput("domain block domain is invalid"));
    }
    let domain = canonical_remote_domain(domain)
        .map_err(|_| WriteError::InvalidInput("domain block domain is invalid"))?;
    if domain.contains(':') {
        return Err(WriteError::InvalidInput(
            "domain block domain must not include a port",
        ));
    }
    if domain.len() >= 256
        || domain.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(WriteError::InvalidInput("domain block domain is invalid"));
    }
    Ok(domain)
}

#[allow(clippy::fn_params_excessive_bools)]
fn domain_block_is_stricter(
    severity: i32,
    reject_media: bool,
    reject_reports: bool,
    existing_severity: Option<i32>,
    existing_reject_media: bool,
    existing_reject_reports: bool,
) -> bool {
    if severity == 1 {
        return true;
    }
    let Some(existing_severity) = existing_severity else {
        return false;
    };
    if !matches!(existing_severity, 0..=2) {
        return false;
    }
    if existing_severity == 1 && (severity == 0 || severity == 2) {
        return false;
    }
    if existing_severity == 0 && severity == 2 {
        return false;
    }
    (reject_media || !existing_reject_media) && (reject_reports || !existing_reject_reports)
}

fn canonical_email_hash(email: &str) -> String {
    let email = email.to_ascii_lowercase();
    let mut parts = email.splitn(2, '@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    let local = local.split('+').next().unwrap_or_default().replace('.', "");
    format!(
        "{:x}",
        Sha256::digest(format!("{local}@{domain}").as_bytes())
    )
}

async fn clear_domain_owned_account_restrictions(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
    created_at: NaiveDateTime,
) -> Result<(), WriteError> {
    let domain = domain_policy_hostname(domain);
    sqlx::query(
        "UPDATE accounts SET silenced_at = NULL \
         WHERE domain IS NOT NULL \
           AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                    THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
             OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                    THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
           AND silenced_at = $2",
    )
    .bind(&domain)
    .bind(created_at)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE accounts SET suspended_at = NULL, suspension_origin = NULL \
         WHERE domain IS NOT NULL \
           AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                    THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
             OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                    THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
           AND suspended_at = $2",
    )
    .bind(&domain)
    .bind(created_at)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn apply_domain_account_restrictions(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
    severity: i32,
    created_at: NaiveDateTime,
) -> Result<(), WriteError> {
    let domain = domain_policy_hostname(domain);
    match severity {
        0 => {
            sqlx::query(
                "UPDATE accounts SET silenced_at = $2 \
                 WHERE domain IS NOT NULL \
                   AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                            THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
                     OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                            THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
                   AND silenced_at IS NULL",
            )
            .bind(&domain)
            .bind(created_at)
            .execute(&mut **transaction)
            .await?;
            Ok(())
        }
        1 => {
            sqlx::query(
                "UPDATE accounts SET silenced_at = NULL, suspended_at = $2, suspension_origin = 0 \
                  WHERE domain IS NOT NULL \
                    AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                             THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
                      OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                             THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
                    AND suspended_at IS NULL",
            )
            .bind(&domain)
            .bind(created_at)
            .execute(&mut **transaction)
            .await?;
            Ok(())
        }
        2 => Ok(()),
        _ => Err(WriteError::InvalidInput("domain block severity is invalid")),
    }
}

async fn clear_domain_media(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
) -> Result<(), WriteError> {
    let domain = domain_policy_hostname(domain);
    sqlx::query(
        "UPDATE accounts SET
            avatar_file_name = NULL, avatar_content_type = NULL, avatar_file_size = NULL,
            avatar_updated_at = NULL, header_file_name = NULL, header_content_type = NULL,
            header_file_size = NULL, header_updated_at = NULL, updated_at = clock_timestamp()
          WHERE domain IS NOT NULL
            AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                     THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1)
              OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                     THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1))",
    )
    .bind(&domain)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE media_attachments SET
            file_file_name = NULL, file_content_type = NULL, file_file_size = NULL,
            file_updated_at = NULL, thumbnail_file_name = NULL, thumbnail_content_type = NULL,
            thumbnail_file_size = NULL, thumbnail_updated_at = NULL, updated_at = clock_timestamp()
          WHERE account_id IN (
            SELECT id FROM accounts
             WHERE domain IS NOT NULL
               AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                        THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1)
                   OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                        THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1))
          )",
    )
    .bind(&domain)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM custom_emojis
          WHERE domain IS NOT NULL
            AND (lower(trim(trailing '.' FROM domain)) = lower($1)
              OR lower(trim(trailing '.' FROM domain)) LIKE '%.' || lower($1))",
    )
    .bind(&domain)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        NotificationPolicyDecision, NotificationPolicyFacts, OAuthApplicationRegistration,
        RemoteNoteAudience, RemoteNoteData, RemotePollData, RemotePollExpirationChange,
        RemoteUpdateAuthority, WriteError, canonical_email_hash, canonical_oauth_scopes,
        devise_token_digest, domain_block_is_stricter, local_object_tag_id,
        normalize_domain_block_domain, normalize_status_language, notification_policy_decision,
        notification_policy_decision_for_type, oauth_grant_pkce_is_valid, oauth_pkce_matches,
        parse_user_active_days, password_reset_digest, prepare_local_poll,
        quote_approval_policy_for_status, quote_state_update_counter_delta, random_urlsafe_base64,
        reconciled_remote_quote_state, remote_actor_account_id, remote_domain_lock_scopes,
        remote_emoji_update_decision, remote_note_attachments, remote_note_object_is_too_old,
        remote_note_visibility, remote_poll_expiration_change,
        remote_poll_previous_expiration_is_due_change, remote_poll_tallies_are_monotonic,
        remote_poll_votes_count, remote_quote_approval_policy_with_collections,
        remote_quote_authorization_data, report_category_value, report_email_enabled,
        report_uri_matches_domain, status_mention_candidates, two_factor_attempt_is_rate_limited,
        validate_local_password, validate_oauth_application_registration,
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, Utc};
    use serde_json::json;
    use sha2::{Digest, Sha256};

    fn facts() -> NotificationPolicyFacts {
        NotificationPolicyFacts {
            filterable: true,
            permission: false,
            staff_bypass: false,
            not_following: true,
            not_follower: false,
            new_account: false,
            limited: false,
            bot: false,
            private_mention: false,
            for_not_following: 0,
            for_not_followers: 0,
            for_new_accounts: 0,
            for_limited_accounts: 1,
            for_bots: 0,
            for_private_mentions: 1,
        }
    }

    #[test]
    fn report_email_setting_defaults_to_enabled() {
        assert!(report_email_enabled(None));
        assert!(report_email_enabled(Some("{}")));
        assert!(report_email_enabled(Some("not json")));
    }

    #[test]
    fn remote_notes_parse_quote_aliases_tombstones_and_inline_authorizations() {
        let actor = "https://remote.example/users/alice";
        let base = || {
            json!({
                "id": "https://remote.example/statuses/9",
                "type": "Note",
                "attributedTo": actor,
                "content": "quote",
                "published": "2026-01-01T00:00:00Z"
            })
        };
        for (field, legacy) in [
            ("quote", false),
            ("_misskey_quote", true),
            ("quoteUrl", true),
            ("quoteUri", true),
        ] {
            let mut object = base();
            object[field] = json!("https://target.example/statuses/7");
            let note = RemoteNoteData::parse(&object, actor).expect("quote alias should parse");
            let quote = note.quote.expect("quote should be retained");
            assert_eq!(
                quote.target_uri.as_deref(),
                Some("https://target.example/statuses/7")
            );
            assert_eq!(quote.legacy, legacy);
            assert!(!quote.deleted);
        }

        let mut policy = base();
        policy["interactionPolicy"] = json!({
            "canQuote": {
                "automaticApproval": [
                    "https://www.w3.org/ns/activitystreams#Public",
                    "https://remote.example/users/alice/followers"
                ],
                "manualApproval": [
                    "https://remote.example/users/alice/following",
                    "https://unsupported.example/group"
                ]
            }
        });
        assert_eq!(
            RemoteNoteData::parse(&policy, actor)
                .expect("quote interaction policy should parse")
                .quote_approval_policy,
            ((2 | 4) << 16) | 8 | 1,
        );
        policy["interactionPolicy"]["canQuote"] = json!({
            "automaticApproval": "https://remote.example/custom-followers",
            "manualApproval": "https://remote.example/custom-following"
        });
        assert_eq!(
            remote_quote_approval_policy_with_collections(
                policy.as_object().expect("policy is an object"),
                actor,
                "https://remote.example/custom-followers",
                "https://remote.example/custom-following",
            )
            .expect("custom collection policy should parse"),
            (4 << 16) | 8,
        );

        let mut tombstone = base();
        tombstone["quote"] = json!({
            "id": "https://target.example/statuses/deleted",
            "type": "Tombstone"
        });
        assert!(
            RemoteNoteData::parse(&tombstone, actor)
                .expect("quote tombstone should parse")
                .quote
                .expect("quote should be retained")
                .deleted
        );
        tombstone["quote"] = json!({ "type": "Tombstone" });
        let idless_tombstone = RemoteNoteData::parse(&tombstone, actor)
            .expect("id-less quote tombstone should parse")
            .quote
            .expect("id-less quote tombstone should be retained");
        assert!(idless_tombstone.deleted);
        assert!(idless_tombstone.target_uri.is_none());

        let mut authorized = base();
        authorized["quote"] = json!("https://target.example/statuses/7");
        authorized["quoteAuthorization"] = json!({
            "id": "https://target.example/quote_authorizations/3",
            "type": "QuoteAuthorization",
            "attributedTo": "https://target.example/users/bob",
            "interactingObject": "https://remote.example/statuses/9",
            "interactionTarget": "https://target.example/statuses/7"
        });
        RemoteNoteData::parse(&authorized, actor).expect("inline authorization should parse");
        let authorization = remote_quote_authorization_data(&authorized["quoteAuthorization"])
            .expect("authorization should parse");
        assert!(authorization.typed);
        assert_eq!(
            authorization.interacting_object.as_deref(),
            Some("https://remote.example/statuses/9")
        );
    }

    #[test]
    fn legacy_quote_state_updates_do_not_change_counters() {
        assert_eq!(quote_state_update_counter_delta(false, 0, 1), 1);
        assert_eq!(quote_state_update_counter_delta(false, 1, 3), -1);
        assert_eq!(quote_state_update_counter_delta(false, 1, 1), 0);
        assert_eq!(quote_state_update_counter_delta(true, 0, 1), 0);
        assert_eq!(quote_state_update_counter_delta(true, 1, 3), 0);
    }

    #[test]
    fn remote_quote_authorization_changes_return_accepted_quotes_to_pending() {
        let old = "https://target.example/quote_authorizations/1";
        let replacement = "https://target.example/quote_authorizations/2";

        assert_eq!(
            reconciled_remote_quote_state(false, 1, Some(old), Some(old), 0),
            (1, Some(old.to_owned()))
        );
        assert_eq!(
            reconciled_remote_quote_state(false, 1, Some(old), None, 0),
            (0, None)
        );
        assert_eq!(
            reconciled_remote_quote_state(false, 1, Some(old), Some(replacement), 0),
            (0, None)
        );
        assert_eq!(
            reconciled_remote_quote_state(false, 1, None, Some(replacement), 0),
            (1, None),
            "locally accepted QuoteRequests do not depend on a remote approval URI"
        );
        assert_eq!(
            reconciled_remote_quote_state(true, 1, Some(old), Some(old), 0),
            (0, None),
            "a changed target starts a new quote lifecycle"
        );
    }

    #[test]
    fn remote_note_atom_tags_are_metadata_not_lookup_authority() {
        let actor = "https://remote.example/users/alice";
        let mut object = json!({
            "type": "Note", "id": "https://remote.example/statuses/1",
            "attributedTo": actor, "content": "hello"
        });
        for tag in [
            "tag:remote.example,2026-07-01:objectId=1:objectType=Status",
            "tag:other.example,2026-07-01:objectId=2:objectType=Status",
            "tag:",
            "TAG:malformed opaque metadata",
        ] {
            object["atomUri"] = json!(tag);
            let note = RemoteNoteData::parse(&object, actor)
                .expect("tag metadata must not reject the Note");
            assert_eq!(note.uri, "https://remote.example/statuses/1");
            assert!(
                note.atom_uri.is_none(),
                "tags must not become lookup aliases"
            );
        }
        for invalid in [
            json!(42),
            json!([]),
            json!("file:///tmp/note"),
            json!("relative"),
            json!("https://victim.example/objects/1"),
        ] {
            object["atomUri"] = invalid;
            assert!(RemoteNoteData::parse(&object, actor).is_err());
        }
        object["atomUri"] = json!("https://remote.example/objects/1");
        assert_eq!(
            RemoteNoteData::parse(&object, actor)
                .unwrap()
                .atom_uri
                .as_deref(),
            Some("https://remote.example/objects/1")
        );
        object["atomUri"] = json!("tag:remote.example,2026-07-01:opaque");
        object["id"] = json!("tag:remote.example,2026-07-01:opaque");
        assert!(
            RemoteNoteData::parse(&object, actor).is_err(),
            "tag metadata cannot rescue a non-HTTP canonical ID"
        );
        // Canonical actor-host equality is enforced later by the writers, not
        // this syntax parser; the existing provenance worker tests cover it.
    }

    #[test]
    fn remote_note_counts_follow_activitystreams_collections() {
        let object = json!({
            "type": "Note",
            "id": "https://remote.example/statuses/1",
            "attributedTo": "https://remote.example/users/alice",
            "content": "<p>Hello</p>",
            "likes": { "type": "Collection", "totalItems": 7 },
            "shares": { "type": "Collection", "totalItems": 3 }
        });
        let note = RemoteNoteData::parse(&object, "https://remote.example/users/alice")
            .expect("ActivityStreams interaction collections should parse");
        assert_eq!(note.favourites_count, Some(7));
        assert_eq!(note.reblogs_count, Some(3));

        let legacy = json!({
            "type": "Note",
            "id": "https://remote.example/statuses/2",
            "attributedTo": "https://remote.example/users/alice",
            "content": "<p>Legacy</p>",
            "favouritesCount": 4,
            "reblogsCount": 2
        });
        let note = RemoteNoteData::parse(&legacy, "https://remote.example/users/alice")
            .expect("legacy interaction counts should remain compatible");
        assert_eq!(note.favourites_count, Some(4));
        assert_eq!(note.reblogs_count, Some(2));

        let bounded = json!({
            "type": "Note",
            "id": "https://remote.example/statuses/3",
            "attributedTo": "https://remote.example/users/alice",
            "content": "<p>Bounded</p>",
            "likes": { "type": "Collection", "totalItems": -1 },
            "shares": { "type": "Collection", "totalItems": 100_000_001 },
            "favouritesCount": 100_000_001,
            "reblogsCount": -1
        });
        let note = RemoteNoteData::parse(&bounded, "https://remote.example/users/alice")
            .expect("remote interaction counts should be clamped to Mastodon's bounds");
        assert_eq!(note.favourites_count, Some(0));
        assert_eq!(note.reblogs_count, Some(100_000_000));

        let legacy_bounded = json!({
            "type": "Note",
            "id": "https://remote.example/statuses/4",
            "attributedTo": "https://remote.example/users/alice",
            "content": "<p>Legacy bounded</p>",
            "favouritesCount": -1,
            "reblogsCount": 100_000_001
        });
        let note = RemoteNoteData::parse(&legacy_bounded, "https://remote.example/users/alice")
            .expect("legacy interaction counts should use the same bounds");
        assert_eq!(note.favourites_count, Some(0));
        assert_eq!(note.reblogs_count, Some(100_000_000));
    }

    #[test]
    fn remote_emoji_refresh_decision_avoids_redundant_same_url_downloads() {
        for (changed_url, fresh_timestamp, has_file, expected) in [
            (false, true, true, (true, false)),
            (false, false, true, (false, false)),
            (false, false, false, (false, true)),
            (true, false, true, (true, true)),
            (true, true, false, (true, true)),
        ] {
            assert_eq!(
                remote_emoji_update_decision(changed_url, fresh_timestamp, has_file),
                expected
            );
        }
    }

    #[test]
    fn two_factor_rate_limit_counts_the_current_attempt() {
        assert!(!two_factor_attempt_is_rate_limited(0));
        assert!(!two_factor_attempt_is_rate_limited(8));
        assert!(two_factor_attempt_is_rate_limited(9));
        assert!(two_factor_attempt_is_rate_limited(10));
        assert!(two_factor_attempt_is_rate_limited(i64::MAX));
    }

    #[test]
    fn report_email_setting_reads_mastodon_flat_key() {
        assert!(report_email_enabled(Some(
            r#"{"notification_emails.report":true}"#
        )));
        assert!(!report_email_enabled(Some(
            r#"{"notification_emails.report":false}"#
        )));
    }

    #[test]
    fn report_uri_requires_an_http_origin_on_the_reporter_domain() {
        assert!(report_uri_matches_domain(
            "https://remote.example/activities/report",
            "remote.example"
        ));
        assert!(report_uri_matches_domain(
            "http://remote.example/activities/report",
            "remote.example"
        ));
        assert!(!report_uri_matches_domain(
            "ftp://remote.example/activities/report",
            "remote.example"
        ));
        assert!(!report_uri_matches_domain(
            "tag:remote.example,2026:report",
            "remote.example"
        ));
        assert!(!report_uri_matches_domain(
            "https://other.example/activities/report",
            "remote.example"
        ));
    }

    #[test]
    fn local_object_tags_accept_mastodons_tag_separators_and_types() {
        assert_eq!(
            local_object_tag_id(
                "tag:local.example,2026-09-07:objectId=12:objectType=Status",
                "local.example",
                "Status"
            ),
            Some(12)
        );
        assert_eq!(
            local_object_tag_id(
                "tag:local.example;objectId=13:objectType=Collection",
                "local.example",
                "Collection"
            ),
            Some(13)
        );
        assert_eq!(
            local_object_tag_id(
                "tag:other.example;objectId=14:objectType=Status",
                "local.example",
                "Status"
            ),
            None
        );
        assert_eq!(
            local_object_tag_id(
                "tag:local.example;objectId=14:objectType=Account",
                "local.example",
                "Status"
            ),
            None
        );
    }

    #[test]
    fn user_active_days_follow_mastodons_environment_default_and_conversion() {
        assert_eq!(parse_user_active_days(None), 7);
        assert_eq!(parse_user_active_days(Some("30")), 30);
        assert_eq!(parse_user_active_days(Some("-2")), -2);
        assert_eq!(parse_user_active_days(Some("not-a-number")), 0);
    }

    #[test]
    fn notification_policy_accepts_non_filterable_events() {
        let mut facts = facts();
        facts.filterable = false;
        facts.for_not_following = 2;
        assert_eq!(
            notification_policy_decision(&facts),
            NotificationPolicyDecision::Accept
        );
    }

    #[test]
    fn notification_permission_bypasses_policy_actions() {
        let mut facts = facts();
        facts.permission = true;
        facts.for_not_following = 2;
        assert_eq!(
            notification_policy_decision(&facts),
            NotificationPolicyDecision::Accept
        );
    }

    #[test]
    fn highlighted_moderators_only_bypass_mention_policy() {
        let mut facts = facts();
        facts.staff_bypass = true;
        facts.for_not_following = 2;
        assert_eq!(
            notification_policy_decision_for_type(&facts, "mention"),
            NotificationPolicyDecision::Accept
        );
        assert_eq!(
            notification_policy_decision_for_type(&facts, "favourite"),
            NotificationPolicyDecision::Drop
        );
    }

    #[test]
    fn notification_policy_drop_precedes_filter() {
        let mut facts = facts();
        facts.for_not_following = 1;
        facts.not_follower = true;
        facts.for_not_followers = 2;
        assert_eq!(
            notification_policy_decision(&facts),
            NotificationPolicyDecision::Drop
        );
    }

    #[test]
    fn notification_policy_filters_when_no_drop_applies() {
        let mut facts = facts();
        facts.for_not_following = 1;
        assert_eq!(
            notification_policy_decision(&facts),
            NotificationPolicyDecision::Filter
        );
    }

    #[test]
    fn status_mentions_preserve_local_and_remote_account_shapes() {
        assert_eq!(
            status_mention_candidates("Hello @moderator and @bob@remote.fixture.invalid"),
            vec![
                ("moderator".to_owned(), None),
                ("bob".to_owned(), Some("remote.fixture.invalid".to_owned()))
            ]
        );
        assert_eq!(
            status_mention_candidates("@moderator @moderator"),
            vec![("moderator".to_owned(), None)]
        );
    }

    #[test]
    fn status_languages_follow_mastodon_locale_cascade() {
        assert_eq!(normalize_status_language("fr-FR"), Some("fr".to_owned()));
        assert_eq!(normalize_status_language("fr_FR"), Some("fr".to_owned()));
        assert_eq!(normalize_status_language("zh-TW"), Some("zh-TW".to_owned()));
        assert_eq!(normalize_status_language("fr"), Some("fr".to_owned()));
        assert_eq!(normalize_status_language("xx-YY"), None);
        assert_eq!(normalize_status_language(""), None);
    }

    #[test]
    fn unknown_remote_note_updates_are_old_only_when_published() {
        let now =
            NaiveDateTime::parse_from_str("2026-08-29 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let old = json!({"published": "2026-08-28T11:59:59Z"});
        let boundary = json!({"published": "2026-08-28T12:00:00Z"});
        let missing = json!({});
        assert!(remote_note_object_is_too_old(
            &old,
            now - chrono::Duration::days(1) - chrono::Duration::seconds(1),
            now,
        ));
        assert!(!remote_note_object_is_too_old(
            &boundary,
            now - chrono::Duration::days(1),
            now,
        ));
        assert!(!remote_note_object_is_too_old(&missing, now, now));
    }

    #[test]
    fn remote_actor_identity_prefers_uri_and_rejects_handle_collisions() {
        let uri = "https://remote.example/users/alice";
        let same_handle = Some((7, Some(uri.to_owned())));
        let other_handle = Some((8, Some("https://other.example/users/alice".to_owned())));
        let empty_handle = Some((8, None));
        assert!(matches!(
            remote_actor_account_id(Some(7), same_handle.as_ref(), None, None, uri),
            Ok(7)
        ));
        assert!(matches!(
            remote_actor_account_id(Some(7), other_handle.as_ref(), None, None, uri,),
            Err(WriteError::Conflict)
        ));
        assert!(matches!(
            remote_actor_account_id(None, empty_handle.as_ref(), None, None, uri),
            Err(WriteError::Conflict)
        ));
        assert!(matches!(
            remote_actor_account_id(None, None, Some(9), None, uri),
            Ok(9)
        ));
        assert!(matches!(
            remote_actor_account_id(None, None, None, Some((10, Some(uri.to_owned()))), uri),
            Ok(10)
        ));
        assert!(matches!(
            remote_actor_account_id(
                None,
                None,
                None,
                Some((10, Some("https://other.example/users/alice".to_owned()))),
                uri,
            ),
            Err(WriteError::Conflict)
        ));
    }

    #[test]
    fn remote_note_specific_audiences_are_limited_not_direct() {
        let audience = RemoteNoteAudience {
            to: vec!["https://fixture.example/users/alice".to_owned()],
            cc: Vec::new(),
        };
        assert_eq!(
            remote_note_visibility(&audience, "https://fixture.example/followers"),
            4
        );
    }

    #[test]
    fn status_quote_defaults_follow_visibility_and_account_setting() {
        assert_eq!(
            quote_approval_policy_for_status(0, None, "public").unwrap(),
            131_072
        );
        assert_eq!(
            quote_approval_policy_for_status(1, Some("followers"), "public").unwrap(),
            262_144
        );
        assert_eq!(
            quote_approval_policy_for_status(0, Some("nobody"), "public").unwrap(),
            0
        );
        assert_eq!(
            quote_approval_policy_for_status(2, Some("public"), "public").unwrap(),
            0
        );
        assert_eq!(
            quote_approval_policy_for_status(3, Some("followers"), "public").unwrap(),
            0
        );
        assert_eq!(
            quote_approval_policy_for_status(0, None, "invalid").unwrap(),
            131_072
        );
        assert!(matches!(
            quote_approval_policy_for_status(0, Some("invalid"), "public"),
            Err(WriteError::InvalidInput("invalid quote approval policy"))
        ));
        assert!(matches!(
            quote_approval_policy_for_status(2, Some("invalid"), "public"),
            Err(WriteError::InvalidInput("invalid quote approval policy"))
        ));
    }

    #[test]
    fn oauth_credentials_use_doorkeeper_urlsafe_base64_shape() {
        let value = random_urlsafe_base64(32);
        assert_eq!(value.len(), 43);
        assert!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        );
    }

    #[test]
    fn public_oauth_grants_require_a_well_formed_s256_challenge() {
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(oauth_grant_pkce_is_valid(
            false,
            Some(challenge),
            Some("S256")
        ));
        assert!(!oauth_grant_pkce_is_valid(false, None, None));
        assert!(!oauth_grant_pkce_is_valid(
            false,
            Some(challenge),
            Some("plain")
        ));
        assert!(!oauth_grant_pkce_is_valid(
            false,
            Some(&"a".repeat(42)),
            Some("S256")
        ));
        assert!(!oauth_grant_pkce_is_valid(
            false,
            Some("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw.cM"),
            Some("S256")
        ));
    }

    #[test]
    fn confidential_oauth_grants_may_omit_pkce_but_not_supply_invalid_pkce() {
        assert!(oauth_grant_pkce_is_valid(true, None, None));
        assert!(!oauth_grant_pkce_is_valid(
            true,
            Some("short"),
            Some("S256")
        ));
        assert!(!oauth_grant_pkce_is_valid(true, Some("challenge"), None));
    }

    #[test]
    fn oauth_pkce_redemption_validates_the_rfc_7636_verifier() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(oauth_pkce_matches(
            false,
            Some(challenge),
            Some("S256"),
            Some(verifier)
        ));
        assert!(!oauth_pkce_matches(
            false,
            Some(challenge),
            Some("S256"),
            Some(&"a".repeat(42))
        ));
        assert!(!oauth_pkce_matches(
            false,
            Some(challenge),
            Some("S256"),
            Some(&"a".repeat(129))
        ));
        assert!(!oauth_pkce_matches(
            false,
            Some(challenge),
            Some("S256"),
            Some("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk=")
        ));

        for verifier in ["~".repeat(43), ".".repeat(128)] {
            let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            assert!(oauth_pkce_matches(
                false,
                Some(&challenge),
                Some("S256"),
                Some(&verifier)
            ));
        }
    }

    #[test]
    fn oauth_pkce_redemption_rejects_legacy_public_grants() {
        assert!(!oauth_pkce_matches(false, None, None, None));
        assert!(oauth_pkce_matches(true, None, None, None));
    }

    #[test]
    fn password_reset_tokens_are_stored_as_fixed_sha256_digests() {
        let digest = password_reset_digest("fixture-reset-token");
        assert_eq!(digest.len(), 64);
        assert_ne!(digest, "fixture-reset-token");
        assert_eq!(digest, password_reset_digest("fixture-reset-token"));
        assert_ne!(digest, password_reset_digest("different-token"));
    }

    #[test]
    fn devise_token_digests_use_the_secret_key_with_legacy_fallback() {
        assert_eq!(
            devise_token_digest(
                "reset_password_token",
                "fixture-reset-token",
                Some("fixture-secret")
            ),
            "63b969c17d9be80784b3921802ec28fb9c328e8fdb82a49cb018bdbbb69da25d"
        );
        assert_eq!(
            devise_token_digest(
                "confirmation_token",
                "fixture-reset-token",
                Some("fixture-secret")
            ),
            "333f9fba080e4ff5cd06b0d0d6c1f0275a39db7d8ddb8de5a04dc99a5232e277"
        );
        assert_ne!(
            devise_token_digest(
                "reset_password_token",
                "fixture-reset-token",
                Some("fixture-secret")
            ),
            password_reset_digest("fixture-reset-token")
        );
        assert_eq!(
            devise_token_digest("reset_password_token", "fixture-reset-token", None),
            password_reset_digest("fixture-reset-token")
        );
    }

    #[test]
    fn password_reset_validation_matches_local_user_creation_limits() {
        assert!(validate_local_password(&"p".repeat(8)).is_ok());
        assert!(validate_local_password(&"p".repeat(72)).is_ok());
        assert!(validate_local_password(&"p".repeat(7)).is_err());
        assert!(validate_local_password(&"p".repeat(73)).is_err());
    }

    #[test]
    fn domain_block_normalization_matches_mastodon_boundaries() {
        assert_eq!(
            normalize_domain_block_domain("  Example.COM/ ").unwrap(),
            "example.com"
        );
        assert_eq!(
            normalize_domain_block_domain("example.com:443").unwrap(),
            "example.com"
        );
        assert!(normalize_domain_block_domain("example.com:8443").is_err());
        assert!(normalize_domain_block_domain("example..com").is_err());
        assert!(normalize_domain_block_domain("example_com").is_err());
        assert!(normalize_domain_block_domain(&"a".repeat(64)).is_err());
        assert!(normalize_domain_block_domain("example.com//").is_err());
    }

    #[test]
    fn remote_domain_lock_scopes_cover_single_label_ancestors() {
        assert_eq!(
            remote_domain_lock_scopes("Foo.Example.COM."),
            vec!["com", "example.com", "foo.example.com"]
        );
        assert_eq!(remote_domain_lock_scopes("com"), vec!["com"]);
        assert_eq!(remote_domain_lock_scopes("192.0.2.1"), vec!["192.0.2.1"]);
    }

    #[test]
    fn domain_block_updates_never_weaken_an_effective_rule() {
        assert!(domain_block_is_stricter(
            1,
            false,
            false,
            Some(1),
            true,
            true
        ));
        assert!(domain_block_is_stricter(
            1,
            false,
            false,
            Some(0),
            false,
            false
        ));
        assert!(!domain_block_is_stricter(
            0,
            false,
            false,
            Some(1),
            false,
            false
        ));
        assert!(!domain_block_is_stricter(
            2,
            true,
            true,
            Some(0),
            false,
            false
        ));
        assert!(domain_block_is_stricter(
            0,
            true,
            false,
            Some(0),
            false,
            false
        ));
        assert!(!domain_block_is_stricter(
            0,
            false,
            false,
            Some(0),
            true,
            false
        ));
        assert!(domain_block_is_stricter(
            1, false, false, None, false, false
        ));
        assert!(!domain_block_is_stricter(0, true, true, None, false, false));
        assert!(!domain_block_is_stricter(
            2,
            true,
            true,
            Some(99),
            false,
            false
        ));
    }

    #[test]
    fn canonical_email_hash_matches_mastodon_canonicalization() {
        assert_eq!(
            canonical_email_hash("Fixture.User+tag@Example.Invalid"),
            canonical_email_hash("fixtureuser@example.invalid")
        );
        assert_ne!(
            canonical_email_hash("fixture.user@example.invalid"),
            canonical_email_hash("other@example.invalid")
        );
    }

    #[test]
    fn oauth_registration_matches_doorkeeper_validation_boundaries() {
        let mut registration = OAuthApplicationRegistration {
            name: "Fixture client".to_owned(),
            redirect_uri: "https://client.example/callback\nurn:ietf:wg:oauth:2.0:oob".to_owned(),
            scopes: "read write:statuses".to_owned(),
            website: Some("https://client.example".to_owned()),
        };
        assert!(validate_oauth_application_registration(&registration).is_ok());

        registration.name = "x".repeat(61);
        assert!(validate_oauth_application_registration(&registration).is_err());
        registration.name = "Fixture client".to_owned();

        registration.scopes = "read unknown:scope".to_owned();
        assert!(validate_oauth_application_registration(&registration).is_err());
        registration.scopes = "read write:statuses".to_owned();

        for redirect_uri in [
            "https://client.example/callback#fragment",
            "javascript:alert(1)",
            "relative/callback",
        ] {
            registration.redirect_uri = redirect_uri.to_owned();
            assert!(validate_oauth_application_registration(&registration).is_err());
        }
        registration.redirect_uri = "https://client.example/callback".to_owned();

        registration.website = Some("ftp://client.example".to_owned());
        assert!(validate_oauth_application_registration(&registration).is_err());

        assert_eq!(
            canonical_oauth_scopes("read write:statuses read"),
            "read write:statuses"
        );
    }

    #[test]
    fn remote_note_attachment_metadata_preserves_summary_and_focal_point() {
        let attachments = remote_note_attachments(Some(&json!([
            {
                "type": "Document",
                "url": "https://media.example/clip.jpg",
                "mediaType": "image/jpeg",
                "summary": "A descriptive summary",
                "focalPoint": [0.25, -0.5]
            }
        ])));

        assert_eq!(
            attachments[0].description.as_deref(),
            Some("A descriptive summary")
        );
        assert_eq!(
            attachments[0].file_meta,
            json!({"focus": {"x": 0.25, "y": -0.5}})
        );

        let linked = remote_note_attachments(Some(&json!({
            "url": [{
                "href": "https://media.example/linked.png",
                "mediaType": "image/png"
            }],
            "icon": {"url": "https://media.example/preview.png"},
            "name": "Fallback name"
        })));
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].remote_url, "https://media.example/linked.png");
        assert_eq!(
            linked[0].thumbnail_remote_url.as_deref(),
            Some("https://media.example/preview.png")
        );
        assert_eq!(linked[0].content_type.as_deref(), Some("image/png"));

        let truncated = remote_note_attachments(Some(&json!({
            "url": "https://media.example/truncated.jpg",
            "summary": "x".repeat(10_001),
            "blurhash": "not-a-blurhash"
        })));
        assert_eq!(
            truncated[0]
                .description
                .as_ref()
                .map(|value| value.chars().count()),
            Some(10_000)
        );
        assert_eq!(truncated[0].blurhash, None);
    }

    #[test]
    fn local_poll_normalization_matches_mastodon_bounds() {
        let poll = prepare_local_poll(
            &["  first  ".to_owned(), String::new(), " second".to_owned()],
            300,
            true,
            true,
        )
        .expect("bounded poll");
        assert_eq!(poll.options, ["first", "second"]);
        assert_eq!(poll.expires_in, 300);
        assert!(poll.multiple);
        assert!(poll.hide_totals);

        assert_eq!(
            prepare_local_poll(&["one".into()], 300, false, false)
                .expect_err("one option is invalid"),
            "Options must have more than one item"
        );
        assert_eq!(
            prepare_local_poll(&["same".into(), "same".into()], 300, false, false)
                .expect_err("duplicates are invalid"),
            "Options contain duplicate items"
        );
        assert_eq!(
            prepare_local_poll(&["one".into(), "two".into()], 299, false, false)
                .expect_err("short expiry is invalid"),
            "Expires at is too soon"
        );
        assert_eq!(
            prepare_local_poll(&["one".into(), "two".into()], 2_629_747, false, false)
                .expect_err("long expiry is invalid"),
            "Expires at is too far into the future"
        );
    }

    #[test]
    fn local_poll_option_limit_counts_grapheme_clusters() {
        let joined = "e\u{301}".repeat(50);
        assert!(prepare_local_poll(&[joined.clone(), "other".into()], 300, false, false).is_ok());
        assert_eq!(
            prepare_local_poll(&[format!("{joined}x"), "other".into()], 300, false, false)
                .expect_err("51 graphemes are invalid"),
            "Options cannot be longer than 50 characters each"
        );
    }

    #[test]
    fn remote_question_parser_accepts_type_arrays_and_tolerates_partial_poll_metadata() {
        let actor = "https://remote.example/users/alice";
        let object = json!({
            "id": "https://remote.example/statuses/1",
            "type": ["Question"],
            "attributedTo": actor,
            "content": "<p>Choose</p>",
            "published": "2026-08-25T12:00:00Z",
            "endTime": "not-a-timestamp",
            "oneOf": [
                {"type": "Note", "replies": {"totalItems": 4}},
                {"type": "Note", "name": "Tea", "replies": {"totalItems": 2}},
                {"type": "Note", "content": "Coffee", "replies": {"totalItems": 1}}
            ]
        });
        let note = RemoteNoteData::parse(&object, actor)
            .expect("Mastodon-compatible partial Question metadata should parse");
        let poll = note.poll.expect("Question should project a poll");
        assert_eq!(poll.options, ["Tea", "Coffee"]);
        assert_eq!(poll.tallies, [4, 2, 1]);
        assert_eq!(poll.expires_at, None);
    }

    #[test]
    fn remote_past_expiry_changes_finalize_before_suppressing_replacements() {
        let now = DateTime::<Utc>::UNIX_EPOCH.naive_utc();
        let past = now - ChronoDuration::days(1);
        let earlier = past - ChronoDuration::hours(1);
        let future = now + ChronoDuration::days(1);

        for incoming in [Some(future), Some(earlier), None] {
            assert!(remote_poll_previous_expiration_is_due_change(
                true,
                incoming,
                Some(past),
                now,
            ));
            assert_eq!(
                remote_poll_expiration_change(true, incoming, Some(past), now),
                RemotePollExpirationChange::Suppress,
                "a due previous generation is finalized before applying Mastodon's anti-retrigger suppression"
            );
        }
        assert_eq!(
            remote_poll_expiration_change(true, Some(past), Some(past), now),
            RemotePollExpirationChange::None,
            "same-generation tally refreshes do not change expiration work"
        );
        assert_eq!(
            remote_poll_expiration_change(true, Some(past), None, now),
            RemotePollExpirationChange::Reschedule
        );
        assert_eq!(
            remote_poll_expiration_change(
                true,
                Some(future - ChronoDuration::hours(1)),
                Some(future),
                now,
            ),
            RemotePollExpirationChange::Reschedule
        );
        assert_eq!(
            remote_poll_expiration_change(
                true,
                Some(future + ChronoDuration::hours(1)),
                Some(future),
                now,
            ),
            RemotePollExpirationChange::Reschedule,
            "every changed present future expiry gets an exact-generation schedule"
        );
        assert!(!remote_poll_previous_expiration_is_due_change(
            false,
            Some(future),
            Some(past),
            now,
        ));
        assert_eq!(
            remote_poll_expiration_change(false, Some(future), Some(past), now),
            RemotePollExpirationChange::None
        );
    }

    #[test]
    fn signed_refresh_is_authoritative_but_inbox_poll_tallies_are_monotonic() {
        assert!(RemoteUpdateAuthority::Inbox.rejects_tally_regression());
        assert!(!RemoteUpdateAuthority::Inbox.claims_freshness());
        assert!(!RemoteUpdateAuthority::SignedRefresh.rejects_tally_regression());
        assert!(RemoteUpdateAuthority::SignedRefresh.claims_freshness());

        let poll = |tallies, voters_count| RemotePollData {
            options: vec!["one".into(), "two".into()],
            tallies,
            multiple: false,
            expires_at: None,
            voters_count,
        };
        assert!(remote_poll_tallies_are_monotonic(
            &[2, 1],
            3,
            Some(3),
            &poll(vec![2, 2], Some(4)),
            4,
        ));
        assert!(!remote_poll_tallies_are_monotonic(
            &[2, 1],
            3,
            Some(3),
            &poll(vec![1, 3], Some(4)),
            4,
        ));
        assert!(!remote_poll_tallies_are_monotonic(
            &[2, 1, 1],
            4,
            Some(4),
            &poll(vec![3, 1], Some(4)),
            4,
        ));
        assert!(!remote_poll_tallies_are_monotonic(
            &[2, 1],
            3,
            Some(3),
            &poll(vec![2, 2], None),
            4,
        ));
    }

    #[test]
    fn remote_poll_vote_count_rejects_bigint_overflow() {
        let poll = RemotePollData {
            options: vec!["one".into(), "two".into()],
            tallies: vec![i64::MAX, 1],
            multiple: false,
            expires_at: None,
            voters_count: None,
        };
        assert!(matches!(
            remote_poll_votes_count(&poll),
            Err(WriteError::InvalidInput(
                "remote poll vote count is invalid"
            ))
        ));
    }

    #[test]
    fn report_categories_match_mastodon_and_rules_force_violation() {
        assert_eq!(report_category_value(None, false).unwrap(), 0);
        assert_eq!(report_category_value(Some("spam"), false).unwrap(), 1_000);
        assert_eq!(report_category_value(Some("legal"), false).unwrap(), 1_500);
        assert_eq!(report_category_value(Some("other"), true).unwrap(), 2_000);
        assert!(report_category_value(Some("violation"), false).is_err());
        assert!(report_category_value(Some("unknown"), false).is_err());
    }
}
