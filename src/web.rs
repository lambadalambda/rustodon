mod browser;
use browser::*;
mod oauth;
use oauth::*;
mod federation;
use federation::*;
mod hashtag_controls;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::VecDeque;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, SystemTime};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{
    ConnectInfo, Extension, Path, Query, RawQuery, Request, State,
    ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
};
use axum::http::header::{
    ACCEPT, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE,
    ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD, AUTHORIZATION, CACHE_CONTROL,
    CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, COOKIE, HOST, IF_MODIFIED_SINCE, IF_NONE_MATCH,
    LAST_MODIFIED, LOCATION, ORIGIN, PRAGMA, RANGE, SEC_WEBSOCKET_PROTOCOL, SET_COOKIE, USER_AGENT,
    VARY, WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderValue, Method, Response, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::routing::{any, delete, get, patch, post, put};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{Duration as ChronoDuration, NaiveDateTime, SecondsFormat, Utc};
use futures_util::TryStreamExt;
use hmac::{Hmac, Mac};
use ipnetwork::IpNetwork;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower_http::services::{ServeDir, ServeFile};
use url::{Host, Url};

use crate::jobs::{ACTIVITYPUB_INBOX_JOB_KIND, JobError, JobSpec, Lane, Queue};
use crate::mail::MailConfig;
use crate::mastodon::rest::{
    AccountListKind, AccountListOptions, AccountSearchError, AccountStatusesOptions, ApiDateTime,
    ConversationProjection, DecimalId, FollowCollectionKind, FollowCollectionOptions,
    FollowedTagsOptions, InstanceActivityCounts, InstanceProjection, InstanceRuntimeConfig,
    ListProjection, NotificationOptions, PreferencesProjection, RestAccount, RestConversation,
    RestError, RestMarker, RestPreferences, RestProjectionLoader, RestRole, RestSerializer,
    SUPPORTED_MIME_TYPES, SavedStatusKind, SavedStatusesOptions, StatusShape, TagTimelineOptions,
    TimelineOptions, media_projection, notification_type_filter_with_exclusions,
};
use crate::mastodon::{
    Account, AccountFieldUpdate, AccountMediaUpdate, AccountProfileUpdate, AccountProfileValue,
    AccountSourceUpdate, AuthenticatedBearer, BearerAuthenticator, BearerToken,
    BrowserAuthenticationError, BrowserSession, HttpSignatureError, HttpSignatureKey,
    HttpSignatureRequest, HttpSignatureSigner, IdempotencyKey, MediaAttachment,
    MediaAttachmentCreate, MediaAttachmentUpdate, MediaFocus, NO_SCOPE, NotificationPolicy,
    NotificationPolicyUpdate, OAUTH_CONFIGURED_SCOPES, OAuthAuthenticationError,
    OAuthAuthorizationCodeError, OAuthAuthorizationGrantError, OAuthClientCredentialsError,
    OAuthError, OAuthResourceOwner, OAuthScopes, OAuthTokenRevocationError, PROFILE, PollCreate,
    READ_ACCOUNTS, READ_BLOCKS, READ_BOOKMARKS, READ_COLLECTIONS, READ_FAVOURITES, READ_FILTERS,
    READ_FOLLOWS, READ_LISTS, READ_MUTES, READ_NOTIFICATIONS, READ_SEARCH, READ_STATUSES,
    REPORT_RATE_LIMIT, Repository, RequiredScopes, StatusMediaAttributeUpdate, StatusUpdate,
    TwoFactorVerification, User, VERIFY_CREDENTIALS, WRITE_ACCOUNTS, WRITE_BLOCKS, WRITE_BOOKMARKS,
    WRITE_CONVERSATIONS, WRITE_FAVOURITES, WRITE_FOLLOWS, WRITE_MEDIA, WRITE_MUTES,
    WRITE_NOTIFICATIONS, WRITE_REPORTS, WRITE_STATUSES, WriteError, WriteRepository,
    activitypub::{self, ACTIVITY_JSON, JRD_JSON},
    equals_or_includes, random_auth_token, random_totp_secret, signature_key_id,
    verify_http_signature, verify_password, verify_two_factor,
};
use crate::paperclip::{
    PaperclipAttachment, PaperclipMetadata, PaperclipRoot, PreparedAccountMedia,
    PreparedMediaAttachment, parse_paperclip_path, prepare_account_media, write_prepared_media,
};
use crate::remote::{
    RemoteAccountResolver, RemoteFetchError, canonical_remote_domain,
    canonical_remote_domain_from_url, supported_activitypub_context, valid_remote_username,
};
use crate::remote::{RemoteFetchLimits, RemoteFetcher};
use crate::secret::SecretString;
use crate::streaming::{
    ClientCommand, ParsedCommand, STATUS_UPDATE_NOTIFICATION_EVENT, STREAM_EVENT_BATCH_SIZE,
    STREAM_MAX_SUBSCRIPTIONS, SYSTEM_KILL_EVENT, StreamEvent, StreamName, Subscription,
    TOKEN_KILL_EVENT, TimelineRouteSnapshot, event_message,
};
use tokio::time::{Instant, MissedTickBehavior, interval};

const CORS_METHODS: &str = "POST, PUT, DELETE, GET, PATCH, OPTIONS";
const CORS_MAX_AGE: &str = "7200";
const CORS_EXPOSE_HEADERS: &str = "Link, Mastodon-Async-Refresh, X-RateLimit-Reset, X-RateLimit-Limit, X-RateLimit-Remaining, X-Request-Id";
const PUBLIC_CACHE: &str = "max-age=300, public, stale-while-revalidate=30, stale-if-error=86400";
const ACTIVITYPUB_STATUS_PUBLIC_VARY: &str =
    "Accept, Accept-Language, Cookie, Authorization, Signature";
const ACTIVITYPUB_STATUS_AUTHORIZED_VARY: &str =
    "Accept, Accept-Language, Cookie, Signature, Authorization";
const ACTIVITYPUB_STATUS_PUBLIC_CACHE: &str = "max-age=180, public";
const ACTIVITYPUB_STATUS_PENDING_QUOTE_CACHE: &str = "max-age=5, public";
const ACTIVITYPUB_STATUS_PRIVATE_ACTIVITY_CACHE: &str = "max-age=180, private";
const ANONYMOUS_CACHE: &str = "max-age=15, public, stale-while-revalidate=30, stale-if-error=86400";
const PRIVATE_CACHE: &str = "private, no-store";
const PAPERCLIP_CACHE: &str = "public, max-age=2419200, immutable";
const PAPERCLIP_STATUS_VARY: &str = "Authorization, Cookie, Signature";
const PAPERCLIP_CSP: &str = "default-src 'none'; form-action 'none'";
const HTML_CONTENT_SECURITY_POLICY: &str = "default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:";
const MEDIA_PROXY_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MULTIPART_BOUNDARY: &str = "AaB03x";
const FRAMEWORK_ERROR_HEADER: &str = "x-rustodon-framework-error";
const RACK_BYTES_LIMIT: usize = 4 * 1024 * 1024;
const RACK_PARAMETER_LIMIT: usize = 4096;
const RACK_DEPTH_LIMIT: usize = 32;
const PUBLIC_REQUEST_BODY_LIMIT_BYTES: usize = RACK_BYTES_LIMIT;
pub const REST_BODY_LIMIT_BYTES: usize = 99 * 1024 * 1024;
const ACCOUNT_PROFILE_BODY_LIMIT_BYTES: usize = 12 * 1024 * 1024;
pub const ACTIVITYPUB_INBOX_BODY_LIMIT_BYTES: usize = 1024 * 1024;
const REQUEST_BODY_READ_TIMEOUT: StdDuration = StdDuration::from_secs(30);
const ACTIVITYPUB_INBOX_RATE_LIMIT: usize = 300;
const ACTIVITYPUB_INBOX_RATE_LIMIT_PERIOD: StdDuration = StdDuration::from_mins(5);
const MEDIA_UPLOAD_RATE_LIMIT: usize = 30;
const MEDIA_UPLOAD_RATE_LIMIT_PERIOD: StdDuration = StdDuration::from_mins(30);
const REPORT_RATE_LIMIT_PERIOD: StdDuration = StdDuration::from_hours(24);
const SIGNATURE_FETCH_COOL_OFF: StdDuration = StdDuration::from_mins(5);
const MAX_SIGNATURE_FETCH_CIRCUITS: usize = 65_536;
const FRONTEND_ANDROID_ICON_SIZES: &[u16] = &[36, 48, 72, 96, 144, 192, 256, 384, 512];
const FRONTEND_CACHE: &str = "public, max-age=31536000, immutable";
const RUSTODON_STYLESHEET_URL: &str = "/rustodon-assets/rustodon-f4604ff644e0.css";
const RUSTODON_STYLESHEET: &str = include_str!("../assets/rustodon.css");

#[derive(Clone, Copy)]
enum ActivityPubStatusDocument {
    Note { pending_quote: bool },
    Activity,
}
const FRONTEND_THEME_SELECTION: &str = r"(function (element) {
  const {colorScheme, contrast} = element.dataset;
  const colorSchemeMediaWatcher = window.matchMedia('(prefers-color-scheme: dark)');
  const contrastMediaWatcher = window.matchMedia('(prefers-contrast: more)');
  const updateColorScheme = () => {
    const useDarkMode = colorScheme === 'auto' ? colorSchemeMediaWatcher.matches : colorScheme === 'dark';
    element.dataset.colorScheme = useDarkMode ? 'dark' : 'light';
  };
  const updateContrast = () => {
    const useHighContrast = contrast === 'high' || contrastMediaWatcher.matches;
    element.dataset.contrast = useHighContrast ? 'high' : 'default';
  };
  colorSchemeMediaWatcher.addEventListener('change', updateColorScheme);
  contrastMediaWatcher.addEventListener('change', updateContrast);
  updateColorScheme();
  updateContrast();
})(document.documentElement);";

#[derive(Clone, Debug, serde::Deserialize)]
struct FrontendAsset {
    file: String,
    #[serde(default)]
    css: Vec<String>,
    #[serde(default)]
    imports: Vec<String>,
    #[serde(default)]
    integrity: Option<String>,
}

#[derive(Clone, Debug)]
struct FrontendAssets {
    root: PathBuf,
    manifest: BTreeMap<String, FrontendAsset>,
    asset_manifest: BTreeMap<String, FrontendAsset>,
}

struct FrontendAuthenticatedState {
    account: RestAccount,
    access_token: String,
    account_id: i64,
    preferences: RestPreferences,
    settings: serde_json::Value,
    role: RestRole,
}

impl FrontendAssets {
    fn load(root: PathBuf) -> io::Result<Self> {
        let manifest = load_frontend_manifest(&root.join("packs/.vite/manifest.json"))?;
        let asset_manifest =
            load_frontend_manifest(&root.join("packs/.vite/manifest-assets.json"))?;
        if manifest
            .values()
            .chain(asset_manifest.values())
            .any(|asset| {
                safe_frontend_path(&asset.file).is_none()
                    || asset
                        .css
                        .iter()
                        .any(|path| safe_frontend_path(path).is_none())
                    || asset
                        .imports
                        .iter()
                        .any(|path| safe_frontend_path(path).is_none())
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frontend manifest contains an unsafe asset path",
            ));
        }
        Ok(Self {
            root,
            manifest,
            asset_manifest,
        })
    }

    fn entry(&self, source: &str) -> Option<&FrontendAsset> {
        self.manifest.get(source)
    }

    fn asset(&self, source: &str) -> Option<&FrontendAsset> {
        self.asset_manifest.get(source)
    }

    fn file_url(file: &str) -> Option<String> {
        Some(format!("/packs/{}", safe_frontend_path(file)?))
    }

    fn entry_url(&self, source: &str) -> Option<String> {
        self.entry(source)
            .and_then(|asset| Self::file_url(&asset.file))
    }

    fn asset_url(&self, source: &str) -> Option<String> {
        self.asset(source)
            .and_then(|asset| Self::file_url(&asset.file))
    }

    fn path(&self, relative: &str) -> Option<PathBuf> {
        Some(self.root.join(safe_frontend_path(relative)?))
    }
}

fn load_frontend_manifest(path: &FsPath) -> io::Result<BTreeMap<String, FrontendAsset>> {
    let body = std::fs::read(path)?;
    serde_json::from_slice(&body).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frontend manifest {} is invalid: {error}", path.display()),
        )
    })
}

fn safe_frontend_path(path: &str) -> Option<&str> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return None;
    }
    if FsPath::new(path).components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        return None;
    }
    Some(path)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestMetadata {
    pub client_ip: IpAddr,
    pub scheme: Option<String>,
    pub host: Option<String>,
}

const MAX_RATE_LIMIT_WINDOWS: usize = 65_536;

#[derive(Clone, Copy, Debug)]
struct RateLimitExceeded {
    limit: usize,
    period: StdDuration,
}

#[derive(Clone, Copy, Debug)]
struct RateLimitStatus {
    limit: usize,
    remaining: usize,
    period: StdDuration,
}

struct RateLimitWindow {
    bucket: u64,
    attempts: usize,
    expires_at: u64,
}

#[derive(Default)]
struct AttemptLimiterState {
    windows: HashMap<String, RateLimitWindow>,
    expirations: BinaryHeap<Reverse<(u64, String, u64)>>,
}

#[derive(Clone, Default)]
struct AttemptLimiter {
    state: Arc<Mutex<AttemptLimiterState>>,
}

impl AttemptLimiter {
    fn try_allow<I>(&self, keys: I) -> Result<(), RateLimitExceeded>
    where
        I: IntoIterator<Item = (String, usize, StdDuration)>,
    {
        self.try_allow_at(keys, unix_timestamp_seconds())
    }

    #[cfg(test)]
    fn allow_at<I>(&self, keys: I, now: u64) -> bool
    where
        I: IntoIterator<Item = (String, usize, StdDuration)>,
    {
        self.try_allow_at(keys, now).is_ok()
    }

    fn try_allow_at<I>(&self, keys: I, now: u64) -> Result<(), RateLimitExceeded>
    where
        I: IntoIterator<Item = (String, usize, StdDuration)>,
    {
        let keys = keys.into_iter().collect::<Vec<_>>();
        if keys.is_empty()
            || keys
                .iter()
                .any(|(_, limit, period)| *limit == 0 || period.as_secs() == 0)
        {
            return Err(RateLimitExceeded {
                limit: 0,
                period: StdDuration::from_secs(1),
            });
        }
        let Ok(mut state) = self.state.lock() else {
            return Err(RateLimitExceeded {
                limit: keys[0].1,
                period: keys[0].2,
            });
        };
        state.purge_expired(now);
        for (key, _, period) in &keys {
            let bucket = rate_limit_bucket(now, *period);
            if state
                .windows
                .get(key)
                .is_some_and(|window| window.bucket != bucket)
            {
                state.windows.remove(key);
            }
        }
        let new_keys = keys
            .iter()
            .filter(|(key, _, _)| !state.windows.contains_key(key))
            .count();
        if state.windows.len().saturating_add(new_keys) > MAX_RATE_LIMIT_WINDOWS {
            return Err(RateLimitExceeded {
                limit: keys[0].1,
                period: keys[0].2,
            });
        }
        if let Some((_, limit, period)) = keys.iter().find(|(key, limit, _)| {
            state
                .windows
                .get(key)
                .is_some_and(|window| window.attempts >= *limit)
        }) {
            return Err(RateLimitExceeded {
                limit: *limit,
                period: *period,
            });
        }
        for (key, _, period) in keys {
            let bucket = rate_limit_bucket(now, period);
            let expires_at = bucket.saturating_add(1).saturating_mul(period.as_secs());
            if let Some(window) = state.windows.get_mut(&key) {
                window.attempts += 1;
            } else {
                state.windows.insert(
                    key.clone(),
                    RateLimitWindow {
                        bucket,
                        attempts: 1,
                        expires_at,
                    },
                );
                state.expirations.push(Reverse((expires_at, key, bucket)));
            }
        }
        Ok(())
    }
}

impl AttemptLimiterState {
    fn purge_expired(&mut self, now: u64) {
        while let Some(Reverse((expires_at, key, bucket))) = self.expirations.peek().cloned() {
            if expires_at > now {
                break;
            }
            self.expirations.pop();
            if self
                .windows
                .get(&key)
                .is_some_and(|window| window.bucket == bucket && window.expires_at <= now)
            {
                self.windows.remove(&key);
            }
        }
    }
}

fn unix_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn rate_limit_bucket(now: u64, period: StdDuration) -> u64 {
    now / period.as_secs()
}

#[derive(Clone)]
struct SharedRateLimiter {
    pool: PgPool,
    #[cfg(feature = "test-support")]
    fixed_time: Option<i64>,
}

struct SharedRateLimitKey {
    window_key: String,
    limit: usize,
    period: StdDuration,
    bucket: i64,
    expires_at: i64,
}

impl SharedRateLimiter {
    const fn new(pool: PgPool) -> Self {
        Self {
            pool,
            #[cfg(feature = "test-support")]
            fixed_time: None,
        }
    }

    #[cfg(feature = "test-support")]
    fn with_fixed_time(mut self, unix_seconds: i64) -> Self {
        self.fixed_time = Some(unix_seconds);
        self
    }

    #[allow(clippy::too_many_lines)]
    async fn try_allow<I>(&self, keys: I) -> Result<(), RateLimitExceeded>
    where
        I: IntoIterator<Item = (String, usize, StdDuration)>,
    {
        let keys = keys.into_iter().collect::<Vec<_>>();
        let Some((_, limit, period)) = keys.first() else {
            return Err(RateLimitExceeded {
                limit: 0,
                period: StdDuration::from_secs(1),
            });
        };
        let fallback = RateLimitExceeded {
            limit: *limit,
            period: *period,
        };
        if keys
            .iter()
            .any(|(_, limit, period)| *limit == 0 || period.as_secs() == 0)
        {
            return Err(fallback);
        }
        let now = unix_timestamp_seconds();
        #[cfg(feature = "test-support")]
        let now = match self.fixed_time {
            Some(now) => u64::try_from(now).map_err(|_| fallback)?,
            None => now,
        };
        let mut shared_keys = Vec::with_capacity(keys.len());
        for (window_key, limit, period) in keys {
            let Ok(period_seconds) = i64::try_from(period.as_secs()) else {
                return Err(fallback);
            };
            let Ok(bucket) = i64::try_from(now / period.as_secs()) else {
                return Err(fallback);
            };
            let Some(expires_at) = bucket
                .checked_add(1)
                .and_then(|bucket| bucket.checked_mul(period_seconds))
            else {
                return Err(fallback);
            };
            if i32::try_from(limit).is_err() {
                return Err(fallback);
            }
            shared_keys.push(SharedRateLimitKey {
                window_key,
                limit,
                period,
                bucket,
                expires_at,
            });
        }

        let mut lock_keys = shared_keys
            .iter()
            .map(|key| key.window_key.as_str())
            .collect::<Vec<_>>();
        lock_keys.sort_unstable();
        let Ok(mut transaction) = self.pool.begin().await else {
            return Err(fallback);
        };
        for window_key in lock_keys {
            if sqlx::query(
                "SELECT pg_catalog.pg_advisory_xact_lock(\
                   pg_catalog.hashtext('rustodon:rate_limit:' || $1))",
            )
            .bind(window_key)
            .execute(&mut *transaction)
            .await
            .is_err()
            {
                return Err(fallback);
            }
        }
        for key in &shared_keys {
            let delete_expired = sqlx::query(
                "DELETE FROM rustodon.rate_limit_windows \
                 WHERE window_key = $1 AND expires_at <= clock_timestamp()",
            )
            .bind(&key.window_key);
            #[cfg(feature = "test-support")]
            let delete_expired = match self.fixed_time {
                Some(now) => sqlx::query(
                    "DELETE FROM rustodon.rate_limit_windows \
                     WHERE window_key = $1 AND expires_at <= to_timestamp($2::double precision)",
                )
                .bind(&key.window_key)
                .bind(now),
                None => delete_expired,
            };
            if delete_expired.execute(&mut *transaction).await.is_err() {
                return Err(fallback);
            }
        }
        for key in &shared_keys {
            let attempts = match sqlx::query_scalar::<_, i32>(
                "SELECT attempts FROM rustodon.rate_limit_windows \
                 WHERE window_key = $1 AND bucket = $2",
            )
            .bind(&key.window_key)
            .bind(key.bucket)
            .fetch_optional(&mut *transaction)
            .await
            {
                Ok(attempts) => attempts.unwrap_or_default(),
                Err(_) => return Err(fallback),
            };
            if usize::try_from(attempts).unwrap_or(usize::MAX) >= key.limit {
                return Err(RateLimitExceeded {
                    limit: key.limit,
                    period: key.period,
                });
            }
        }
        for key in &shared_keys {
            if sqlx::query(
                "INSERT INTO rustodon.rate_limit_windows \
                   (window_key, bucket, attempts, expires_at) \
                 VALUES ($1, $2, 1, to_timestamp($3::double precision)) \
                 ON CONFLICT (window_key, bucket) DO UPDATE \
                 SET attempts = rustodon.rate_limit_windows.attempts + 1",
            )
            .bind(&key.window_key)
            .bind(key.bucket)
            .bind(key.expires_at)
            .execute(&mut *transaction)
            .await
            .is_err()
            {
                return Err(fallback);
            }
        }
        if transaction.commit().await.is_err() {
            return Err(fallback);
        }
        Ok(())
    }

    async fn circuit_open(&self, window_key: &str) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock(\
               pg_catalog.hashtext('rustodon:rate_limit:' || $1))",
        )
        .bind(window_key)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "DELETE FROM rustodon.rate_limit_windows \
             WHERE window_key = $1 AND bucket = 0 AND expires_at <= clock_timestamp()",
        )
        .bind(window_key)
        .execute(&mut *transaction)
        .await?;
        let open = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(\
               SELECT 1 FROM rustodon.rate_limit_windows \
               WHERE window_key = $1 AND bucket = 0 AND expires_at > clock_timestamp())",
        )
        .bind(window_key)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(open)
    }

    async fn record_circuit_failure(
        &self,
        window_key: &str,
        cool_off: StdDuration,
    ) -> Result<(), sqlx::Error> {
        let cool_off_seconds = i64::try_from(cool_off.as_secs())
            .map_err(|_| sqlx::Error::Protocol("signature circuit period is too large".into()))?;
        if cool_off_seconds <= 0 {
            return Err(sqlx::Error::Protocol(
                "signature circuit period must be positive".into(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock(\
               pg_catalog.hashtext('rustodon:rate_limit:' || $1))",
        )
        .bind(window_key)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "DELETE FROM rustodon.rate_limit_windows \
             WHERE window_key = $1 AND bucket = 0 AND expires_at <= clock_timestamp()",
        )
        .bind(window_key)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO rustodon.rate_limit_windows \
               (window_key, bucket, attempts, expires_at) \
             VALUES ($1, 0, 1, clock_timestamp() + ($2::double precision * INTERVAL '1 second')) \
             ON CONFLICT (window_key, bucket) DO NOTHING",
        )
        .bind(window_key)
        .bind(cool_off_seconds)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await
    }
}

async fn try_rate_limit<I>(
    local: &AttemptLimiter,
    shared: Option<&SharedRateLimiter>,
    keys: I,
) -> Result<(), RateLimitExceeded>
where
    I: IntoIterator<Item = (String, usize, StdDuration)>,
{
    let keys = keys.into_iter().collect::<Vec<_>>();
    match shared {
        Some(shared) => shared.try_allow(keys).await,
        None => local.try_allow(keys),
    }
}

#[derive(Clone, Default)]
struct PasswordResetLimiter {
    limiter: AttemptLimiter,
}

impl PasswordResetLimiter {
    fn keys(client_ip: IpAddr, email: &str) -> [(String, usize, StdDuration); 2] {
        let normalized_email = email.trim().to_ascii_lowercase();
        let email_digest = format!("{:x}", Sha256::digest(normalized_email.as_bytes()));
        [
            (
                format!("password_reset:ip:{}", attempt_ip_bucket(client_ip)),
                25,
                StdDuration::from_mins(5),
            ),
            (
                format!("password_reset:email:{email_digest}"),
                5,
                StdDuration::from_mins(30),
            ),
        ]
    }

    #[cfg(test)]
    fn check(&self, client_ip: IpAddr, email: &str) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(client_ip, email))
    }

    async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
        email: &str,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip, email)).await
    }
}

#[derive(Clone, Default)]
struct BrowserLoginLimiter {
    limiter: AttemptLimiter,
}

impl BrowserLoginLimiter {
    fn keys(client_ip: IpAddr, email: &str) -> [(String, usize, StdDuration); 2] {
        let normalized_email = email.trim().to_ascii_lowercase();
        let email_digest = format!("{:x}", Sha256::digest(normalized_email.as_bytes()));
        [
            (
                format!("browser_login:ip:{}", attempt_ip_bucket(client_ip)),
                25,
                StdDuration::from_mins(5),
            ),
            (
                format!("browser_login:email:{email_digest}"),
                25,
                StdDuration::from_hours(1),
            ),
        ]
    }

    #[cfg(test)]
    fn check(&self, client_ip: IpAddr, email: &str) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(client_ip, email))
    }

    async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
        email: &str,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip, email)).await
    }
}

#[derive(Clone, Default)]
struct BrowserReauthenticationLimiter {
    limiter: AttemptLimiter,
    #[cfg(feature = "test-support")]
    fixed_time: Option<i64>,
}

impl BrowserReauthenticationLimiter {
    fn keys(client_ip: IpAddr, user_id: i64) -> [(String, usize, StdDuration); 2] {
        [
            (
                format!(
                    "browser_reauthentication:ip:{}",
                    attempt_ip_bucket(client_ip)
                ),
                25,
                StdDuration::from_mins(5),
            ),
            (
                format!("browser_reauthentication:user:{user_id}"),
                10,
                StdDuration::from_hours(1),
            ),
        ]
    }

    async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
        user_id: i64,
    ) -> Result<(), RateLimitExceeded> {
        #[cfg(feature = "test-support")]
        if let (Some(now), Some(shared)) = (self.fixed_time, shared) {
            // Freeze only this reauthentication call, not the state's shared
            // limiter used by login, password reset, media, and other routes.
            return shared
                .clone()
                .with_fixed_time(now)
                .try_allow(Self::keys(client_ip, user_id))
                .await;
        }
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip, user_id)).await
    }
}

#[derive(Clone, Default)]
struct OAuthApplicationLimiter {
    limiter: AttemptLimiter,
}

impl OAuthApplicationLimiter {
    fn keys(client_ip: IpAddr) -> [(String, usize, StdDuration); 1] {
        [(
            format!("oauth_application:ip:{}", attempt_ip_bucket(client_ip)),
            5,
            StdDuration::from_mins(10),
        )]
    }

    #[cfg(test)]
    fn check(&self, client_ip: IpAddr) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(client_ip))
    }

    async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip)).await
    }
}

#[derive(Clone, Default)]
struct MediaProxyLimiter {
    limiter: AttemptLimiter,
}

impl MediaProxyLimiter {
    fn keys(client_ip: IpAddr) -> [(String, usize, StdDuration); 1] {
        [(
            format!("media_proxy:ip:{}", attempt_ip_bucket(client_ip)),
            30,
            StdDuration::from_mins(10),
        )]
    }

    #[cfg(test)]
    fn check(&self, client_ip: IpAddr) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(client_ip))
    }

    async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip)).await
    }
}

#[derive(Clone, Default)]
struct MediaUploadLimiter {
    limiter: AttemptLimiter,
}

impl MediaUploadLimiter {
    fn keys(user_id: i64) -> [(String, usize, StdDuration); 1] {
        [(
            format!("media_upload:user:{user_id}"),
            MEDIA_UPLOAD_RATE_LIMIT,
            MEDIA_UPLOAD_RATE_LIMIT_PERIOD,
        )]
    }

    #[cfg(test)]
    fn check(&self, user_id: i64) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(user_id))
    }

    async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        user_id: i64,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(user_id)).await
    }
}

#[derive(Clone, Default)]
struct ActivityPubInboxLimiter {
    limiter: AttemptLimiter,
}

impl ActivityPubInboxLimiter {
    fn keys(client_ip: IpAddr) -> [(String, usize, StdDuration); 1] {
        [(
            format!("activitypub_inbox:ip:{}", attempt_ip_bucket(client_ip)),
            ACTIVITYPUB_INBOX_RATE_LIMIT,
            ACTIVITYPUB_INBOX_RATE_LIMIT_PERIOD,
        )]
    }

    #[cfg(test)]
    fn check(&self, client_ip: IpAddr) -> Result<(), RateLimitExceeded> {
        self.limiter.try_allow(Self::keys(client_ip))
    }

    async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(&self.limiter, shared, Self::keys(client_ip)).await
    }
}

#[derive(Clone, Default)]
struct RemoteAccountResolutionLimiter {
    limiter: AttemptLimiter,
}

impl RemoteAccountResolutionLimiter {
    fn keys(client_ip: IpAddr, username: &str, domain: &str) -> [(String, usize, StdDuration); 2] {
        let handle = format!("{}@{}", username.trim(), domain.trim()).to_ascii_lowercase();
        let handle_digest = format!("{:x}", Sha256::digest(handle.as_bytes()));
        [
            (
                format!(
                    "remote_account_resolution:ip:{}",
                    attempt_ip_bucket(client_ip)
                ),
                10,
                StdDuration::from_mins(1),
            ),
            (
                format!("remote_account_resolution:handle:{handle_digest}"),
                1,
                StdDuration::from_mins(5),
            ),
        ]
    }

    #[cfg(test)]
    fn check(
        &self,
        client_ip: IpAddr,
        username: &str,
        domain: &str,
    ) -> Result<(), RateLimitExceeded> {
        self.limiter
            .try_allow(Self::keys(client_ip, username, domain))
    }

    async fn check_shared(
        &self,
        shared: Option<&SharedRateLimiter>,
        client_ip: IpAddr,
        username: &str,
        domain: &str,
    ) -> Result<(), RateLimitExceeded> {
        try_rate_limit(
            &self.limiter,
            shared,
            Self::keys(client_ip, username, domain),
        )
        .await
    }
}

#[derive(Clone, Default)]
struct SignatureFetchCircuit {
    state: Arc<Mutex<SignatureFetchCircuitState>>,
}

#[derive(Default)]
struct SignatureFetchCircuitState {
    failures: HashMap<IpAddr, u64>,
    expirations: BinaryHeap<Reverse<(u64, IpAddr)>>,
}

impl SignatureFetchCircuit {
    fn allow(&self, client_ip: IpAddr) -> bool {
        self.allow_at(client_ip, unix_timestamp_seconds())
    }

    fn allow_at(&self, client_ip: IpAddr, now: u64) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        state.purge_expired(now);
        !state.failures.contains_key(&client_ip)
            && state.failures.len() < MAX_SIGNATURE_FETCH_CIRCUITS
    }

    fn record_failure(&self, client_ip: IpAddr) {
        self.record_failure_at(client_ip, unix_timestamp_seconds());
    }

    fn record_failure_at(&self, client_ip: IpAddr, now: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.purge_expired(now);
        if state.failures.len() >= MAX_SIGNATURE_FETCH_CIRCUITS
            || state.failures.contains_key(&client_ip)
        {
            return;
        }
        let expires_at = now.saturating_add(SIGNATURE_FETCH_COOL_OFF.as_secs());
        state.failures.insert(client_ip, expires_at);
        state.expirations.push(Reverse((expires_at, client_ip)));
    }
}

impl SignatureFetchCircuitState {
    fn purge_expired(&mut self, now: u64) {
        while let Some(Reverse((expires_at, client_ip))) = self.expirations.peek().copied() {
            if expires_at > now {
                break;
            }
            self.expirations.pop();
            if self
                .failures
                .get(&client_ip)
                .is_some_and(|expiration| *expiration == expires_at)
            {
                self.failures.remove(&client_ip);
            }
        }
    }
}

fn attempt_ip_bucket(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => {
            let network = u128::from(ip) & (!0_u128 << 64);
            format!("{}/64", Ipv6Addr::from(network))
        }
    }
}

fn signature_fetch_circuit_key(client_ip: IpAddr) -> String {
    format!(
        "signature_fetch_circuit:ip:{}",
        attempt_ip_bucket(client_ip)
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForwardedHeaderError;

/// Resolves request metadata from trusted forwarding headers or the direct socket peer.
///
/// # Errors
///
/// Rejects malformed forwarding values supplied by a configured trusted proxy.
pub fn request_metadata(
    peer: SocketAddr,
    headers: &HeaderMap,
    trusted_proxies: &[IpNetwork],
) -> Result<RequestMetadata, ForwardedHeaderError> {
    if !trusted_proxies
        .iter()
        .any(|network| network.contains(peer.ip()))
    {
        return Ok(RequestMetadata {
            client_ip: peer.ip(),
            scheme: None,
            host: None,
        });
    }
    let forwarded_for = header_text(headers, "x-forwarded-for")?;
    let client_ip = forwarded_for.map_or(Ok(peer.ip()), |chain| {
        let addresses = chain
            .split(',')
            .map(str::trim)
            .map(str::parse::<IpAddr>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ForwardedHeaderError)?;
        if addresses.is_empty() {
            return Err(ForwardedHeaderError);
        }
        Ok(addresses
            .iter()
            .copied()
            .rev()
            .find(|address| {
                !trusted_proxies
                    .iter()
                    .any(|network| network.contains(*address))
            })
            .unwrap_or(addresses[0]))
    })?;
    let scheme = header_text(headers, "x-forwarded-proto")?
        .map(str::to_ascii_lowercase)
        .filter(|value| matches!(value.as_str(), "http" | "https"));
    if headers.contains_key("x-forwarded-proto") && scheme.is_none() {
        return Err(ForwardedHeaderError);
    }
    let host = header_text(headers, "x-forwarded-host")?.map(str::to_ascii_lowercase);
    Ok(RequestMetadata {
        client_ip,
        scheme,
        host,
    })
}

fn header_text<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> Result<Option<&'a str>, ForwardedHeaderError> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(ForwardedHeaderError);
    }
    value.to_str().map(Some).map_err(|_| ForwardedHeaderError)
}
const RAILS_PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'$')
    .remove(b'&')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b',')
    .remove(b'-')
    .remove(b'.')
    .remove(b':')
    .remove(b';')
    .remove(b'=')
    .remove(b'@')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiRouteSupport {
    Implemented,
    DisabledResponse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiMethod {
    Get,
    Post,
    Delete,
    Patch,
    Put,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiAuthentication {
    Public,
    Optional(&'static [&'static str]),
    Required(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaginationContract {
    None,
    StatusId,
    RelationshipId,
    AssociationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApiCachePolicy {
    Public,
    Anonymous,
    Private,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiRouteContract {
    pub path: &'static str,
    pub method: ApiMethod,
    pub support: ApiRouteSupport,
    pub authentication: ApiAuthentication,
    pub pagination: PaginationContract,
    cache: ApiCachePolicy,
}

macro_rules! route {
    ($path:literal, $support:ident, $authentication:expr, $pagination:ident, $cache:ident) => {
        ApiRouteContract {
            path: $path,
            method: ApiMethod::Get,
            support: ApiRouteSupport::$support,
            authentication: $authentication,
            pagination: PaginationContract::$pagination,
            cache: ApiCachePolicy::$cache,
        }
    };
}

macro_rules! post_route {
    ($path:literal, $support:ident, $authentication:expr, $pagination:ident, $cache:ident) => {
        ApiRouteContract {
            path: $path,
            method: ApiMethod::Post,
            support: ApiRouteSupport::$support,
            authentication: $authentication,
            pagination: PaginationContract::$pagination,
            cache: ApiCachePolicy::$cache,
        }
    };
}

macro_rules! delete_route {
    ($path:literal, $support:ident, $authentication:expr, $pagination:ident, $cache:ident) => {
        ApiRouteContract {
            path: $path,
            method: ApiMethod::Delete,
            support: ApiRouteSupport::$support,
            authentication: $authentication,
            pagination: PaginationContract::$pagination,
            cache: ApiCachePolicy::$cache,
        }
    };
}

macro_rules! patch_route {
    ($path:literal, $support:ident, $authentication:expr, $pagination:ident, $cache:ident) => {
        ApiRouteContract {
            path: $path,
            method: ApiMethod::Patch,
            support: ApiRouteSupport::$support,
            authentication: $authentication,
            pagination: PaginationContract::$pagination,
            cache: ApiCachePolicy::$cache,
        }
    };
}

macro_rules! put_route {
    ($path:literal, $support:ident, $authentication:expr, $pagination:ident, $cache:ident) => {
        ApiRouteContract {
            path: $path,
            method: ApiMethod::Put,
            support: ApiRouteSupport::$support,
            authentication: $authentication,
            pagination: PaginationContract::$pagination,
            cache: ApiCachePolicy::$cache,
        }
    };
}

const READ_FAMILIAR_FOLLOWERS: RequiredScopes = RequiredScopes::new(&["read", "read:follows"]);

pub const API_ROUTE_INVENTORY: &[ApiRouteContract] = &[
    put_route!(
        "/api/web/settings",
        Implemented,
        ApiAuthentication::Optional(NO_SCOPE.as_slice()),
        None,
        Private
    ),
    patch_route!(
        "/api/web/settings",
        Implemented,
        ApiAuthentication::Optional(NO_SCOPE.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/trends/tags",
        DisabledResponse,
        ApiAuthentication::Optional(NO_SCOPE.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/trends/links",
        DisabledResponse,
        ApiAuthentication::Optional(NO_SCOPE.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/trends/statuses",
        DisabledResponse,
        ApiAuthentication::Optional(NO_SCOPE.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/directory",
        DisabledResponse,
        ApiAuthentication::Optional(NO_SCOPE.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/timelines/link",
        DisabledResponse,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v2/suggestions",
        DisabledResponse,
        ApiAuthentication::Required(READ_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/domain_blocks",
        Implemented,
        ApiAuthentication::Required(READ_BLOCKS.as_slice()),
        AssociationId,
        Private
    ),
    route!(
        "/api/v1/instance/domain_blocks",
        Implemented,
        ApiAuthentication::Optional(NO_SCOPE.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/accounts/familiar_followers",
        DisabledResponse,
        ApiAuthentication::Required(READ_FAMILIAR_FOLLOWERS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/instance",
        Implemented,
        ApiAuthentication::Public,
        None,
        Public
    ),
    route!(
        "/api/v2/instance",
        Implemented,
        ApiAuthentication::Public,
        None,
        Public
    ),
    route!(
        "/api/v1/instance/extended_description",
        Implemented,
        ApiAuthentication::Public,
        None,
        Public
    ),
    route!(
        "/api/v1/instance/rules",
        Implemented,
        ApiAuthentication::Public,
        None,
        Public
    ),
    route!(
        "/api/v1/instance/translation_languages",
        DisabledResponse,
        ApiAuthentication::Public,
        None,
        Public
    ),
    route!(
        "/api/v1/custom_emojis",
        Implemented,
        ApiAuthentication::Public,
        None,
        Public
    ),
    route!(
        "/api/v1/accounts/lookup",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/accounts/search",
        Implemented,
        ApiAuthentication::Required(READ_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/announcements",
        Implemented,
        ApiAuthentication::Required(NO_SCOPE.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v2/search",
        Implemented,
        ApiAuthentication::Optional(READ_SEARCH.as_slice()),
        None,
        Anonymous
    ),
    post_route!(
        "/api/v1/apps",
        Implemented,
        ApiAuthentication::Public,
        None,
        Private
    ),
    route!(
        "/api/v1/apps/verify_credentials",
        Implemented,
        ApiAuthentication::Required(NO_SCOPE.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/markers",
        Implemented,
        ApiAuthentication::Required(READ_STATUSES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/markers",
        Implemented,
        ApiAuthentication::Required(WRITE_STATUSES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses",
        Implemented,
        ApiAuthentication::Required(WRITE_STATUSES.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/polls/{id}",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        None,
        Anonymous
    ),
    post_route!(
        "/api/v1/polls/{id}/votes",
        Implemented,
        ApiAuthentication::Required(WRITE_STATUSES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/reports",
        Implemented,
        ApiAuthentication::Required(WRITE_REPORTS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/media",
        Implemented,
        ApiAuthentication::Required(WRITE_MEDIA.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/media/{id}",
        Implemented,
        ApiAuthentication::Required(WRITE_MEDIA.as_slice()),
        None,
        Private
    ),
    patch_route!(
        "/api/v1/media/{id}",
        Implemented,
        ApiAuthentication::Required(WRITE_MEDIA.as_slice()),
        None,
        Private
    ),
    put_route!(
        "/api/v1/media/{id}",
        Implemented,
        ApiAuthentication::Required(WRITE_MEDIA.as_slice()),
        None,
        Private
    ),
    delete_route!(
        "/api/v1/media/{id}",
        Implemented,
        ApiAuthentication::Required(WRITE_MEDIA.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v2/media",
        Implemented,
        ApiAuthentication::Required(WRITE_MEDIA.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/conversations",
        Implemented,
        ApiAuthentication::Required(READ_STATUSES.as_slice()),
        StatusId,
        Private
    ),
    post_route!(
        "/api/v1/conversations/{id}/read",
        Implemented,
        ApiAuthentication::Required(WRITE_CONVERSATIONS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/conversations/{id}/unread",
        Implemented,
        ApiAuthentication::Required(WRITE_CONVERSATIONS.as_slice()),
        None,
        Private
    ),
    delete_route!(
        "/api/v1/conversations/{id}",
        Implemented,
        ApiAuthentication::Required(WRITE_CONVERSATIONS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/notifications",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        AssociationId,
        Private
    ),
    route!(
        "/api/v2/notifications",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        AssociationId,
        Private
    ),
    post_route!(
        "/api/v1/notifications/clear",
        Implemented,
        ApiAuthentication::Required(WRITE_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/notifications/{id}/dismiss",
        Implemented,
        ApiAuthentication::Required(WRITE_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v2/notifications/clear",
        Implemented,
        ApiAuthentication::Required(WRITE_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v2/notifications/{id}/dismiss",
        Implemented,
        ApiAuthentication::Required(WRITE_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/notifications/requests/{id}/accept",
        Implemented,
        ApiAuthentication::Required(WRITE_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/notifications/requests/{id}/dismiss",
        Implemented,
        ApiAuthentication::Required(WRITE_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/notifications/requests/accept",
        Implemented,
        ApiAuthentication::Required(WRITE_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/notifications/requests/dismiss",
        Implemented,
        ApiAuthentication::Required(WRITE_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/notifications/unread_count",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/notifications/{id}",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/notifications/requests",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        AssociationId,
        Private
    ),
    route!(
        "/api/v1/notifications/requests/merged",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/notifications/requests/{id}",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/notifications/policy",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    put_route!(
        "/api/v1/notifications/policy",
        Implemented,
        ApiAuthentication::Required(WRITE_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v2/notifications/unread_count",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v2/notifications/{id}",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v2/notifications/policy",
        Implemented,
        ApiAuthentication::Required(READ_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    put_route!(
        "/api/v2/notifications/policy",
        Implemented,
        ApiAuthentication::Required(WRITE_NOTIFICATIONS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v2/filters",
        Implemented,
        ApiAuthentication::Required(READ_FILTERS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/lists",
        Implemented,
        ApiAuthentication::Required(READ_LISTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/lists/{id}",
        Implemented,
        ApiAuthentication::Required(READ_LISTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/lists/{id}/accounts",
        Implemented,
        ApiAuthentication::Required(READ_LISTS.as_slice()),
        AssociationId,
        Private
    ),
    route!(
        "/api/v1/accounts/{id}/lists",
        Implemented,
        ApiAuthentication::Required(READ_LISTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/accounts/{id}/collections",
        Implemented,
        ApiAuthentication::Optional(READ_COLLECTIONS.as_slice()),
        AssociationId,
        Anonymous
    ),
    route!(
        "/api/v1/accounts/{id}/in_collections",
        Implemented,
        ApiAuthentication::Optional(READ_COLLECTIONS.as_slice()),
        AssociationId,
        Anonymous
    ),
    route!(
        "/api/v1/tags/{tag}",
        Implemented,
        ApiAuthentication::Optional(NO_SCOPE.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/featured_tags",
        Implemented,
        ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    delete_route!(
        "/api/v1/featured_tags/{id}",
        Implemented,
        ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/tags/{tag}/follow",
        Implemented,
        ApiAuthentication::Required(WRITE_FOLLOWS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/tags/{tag}/unfollow",
        Implemented,
        ApiAuthentication::Required(WRITE_FOLLOWS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/tags/{tag}/feature",
        Implemented,
        ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/tags/{tag}/unfeature",
        Implemented,
        ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/featured_tags",
        Implemented,
        ApiAuthentication::Required(READ_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/followed_tags",
        Implemented,
        ApiAuthentication::Required(READ_FOLLOWS.as_slice()),
        AssociationId,
        Private
    ),
    route!(
        "/api/v1/follow_requests",
        Implemented,
        ApiAuthentication::Required(READ_FOLLOWS.as_slice()),
        RelationshipId,
        Private
    ),
    post_route!(
        "/api/v1/follow_requests/{id}/authorize",
        Implemented,
        ApiAuthentication::Required(WRITE_FOLLOWS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/follow_requests/{id}/reject",
        Implemented,
        ApiAuthentication::Required(WRITE_FOLLOWS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/preferences",
        Implemented,
        ApiAuthentication::Required(READ_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/featured_tags/suggestions",
        Implemented,
        ApiAuthentication::Required(READ_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/accounts/{id}/featured_tags",
        Implemented,
        ApiAuthentication::Public,
        None,
        Private
    ),
    route!(
        "/api/v1/accounts/relationships",
        Implemented,
        ApiAuthentication::Required(READ_FOLLOWS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/profile",
        Implemented,
        ApiAuthentication::Required(VERIFY_CREDENTIALS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/accounts/verify_credentials",
        Implemented,
        ApiAuthentication::Required(VERIFY_CREDENTIALS.as_slice()),
        None,
        Private
    ),
    patch_route!(
        "/api/v1/accounts/update_credentials",
        Implemented,
        ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    delete_route!(
        "/api/v1/profile/avatar",
        Implemented,
        ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    delete_route!(
        "/api/v1/profile/header",
        Implemented,
        ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/accounts",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/accounts/{id}",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/collections/{id}",
        Implemented,
        ApiAuthentication::Optional(READ_COLLECTIONS.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/accounts/{id}/statuses",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        StatusId,
        Anonymous
    ),
    route!(
        "/api/v1/accounts/{id}/followers",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        RelationshipId,
        Anonymous
    ),
    route!(
        "/api/v1/accounts/{id}/following",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        RelationshipId,
        Anonymous
    ),
    route!(
        "/api/v1/statuses/{id}",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        None,
        Anonymous
    ),
    patch_route!(
        "/api/v1/statuses/{id}",
        Implemented,
        ApiAuthentication::Required(WRITE_STATUSES.as_slice()),
        None,
        Private
    ),
    put_route!(
        "/api/v1/statuses/{id}",
        Implemented,
        ApiAuthentication::Required(WRITE_STATUSES.as_slice()),
        None,
        Private
    ),
    delete_route!(
        "/api/v1/statuses/{id}",
        Implemented,
        ApiAuthentication::Required(WRITE_STATUSES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{id}/bookmark",
        Implemented,
        ApiAuthentication::Required(WRITE_BOOKMARKS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{id}/unbookmark",
        Implemented,
        ApiAuthentication::Required(WRITE_BOOKMARKS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{id}/favourite",
        Implemented,
        ApiAuthentication::Required(WRITE_FAVOURITES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{id}/unfavourite",
        Implemented,
        ApiAuthentication::Required(WRITE_FAVOURITES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{id}/reblog",
        Implemented,
        ApiAuthentication::Required(WRITE_STATUSES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{id}/unreblog",
        Implemented,
        ApiAuthentication::Required(WRITE_STATUSES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{id}/mute",
        Implemented,
        ApiAuthentication::Required(WRITE_MUTES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{id}/unmute",
        Implemented,
        ApiAuthentication::Required(WRITE_MUTES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{id}/pin",
        Implemented,
        ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{id}/unpin",
        Implemented,
        ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/accounts/{id}/follow",
        Implemented,
        ApiAuthentication::Required(WRITE_FOLLOWS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/accounts/{id}/unfollow",
        Implemented,
        ApiAuthentication::Required(WRITE_FOLLOWS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/accounts/{id}/remove_from_followers",
        Implemented,
        ApiAuthentication::Required(WRITE_FOLLOWS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/accounts/{id}/block",
        Implemented,
        ApiAuthentication::Required(WRITE_BLOCKS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/accounts/{id}/unblock",
        Implemented,
        ApiAuthentication::Required(WRITE_BLOCKS.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/accounts/{id}/mute",
        Implemented,
        ApiAuthentication::Required(WRITE_MUTES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/accounts/{id}/unmute",
        Implemented,
        ApiAuthentication::Required(WRITE_MUTES.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/statuses/{id}/source",
        Implemented,
        ApiAuthentication::Required(READ_STATUSES.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/statuses/{id}/history",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/statuses/{id}/quotes",
        Implemented,
        ApiAuthentication::Required(READ_STATUSES.as_slice()),
        AssociationId,
        Private
    ),
    put_route!(
        "/api/v1/statuses/{id}/interaction_policy",
        Implemented,
        ApiAuthentication::Required(WRITE_STATUSES.as_slice()),
        None,
        Private
    ),
    post_route!(
        "/api/v1/statuses/{quoted_status_id}/quotes/{id}/revoke",
        Implemented,
        ApiAuthentication::Required(WRITE_STATUSES.as_slice()),
        None,
        Private
    ),
    route!(
        "/api/v1/statuses/{id}/favourited_by",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        AssociationId,
        Anonymous
    ),
    route!(
        "/api/v1/statuses/{id}/reblogged_by",
        Implemented,
        ApiAuthentication::Optional(READ_ACCOUNTS.as_slice()),
        StatusId,
        Anonymous
    ),
    route!(
        "/api/v1/statuses/{id}/context",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        None,
        Anonymous
    ),
    route!(
        "/api/v1/timelines/public",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        StatusId,
        Anonymous
    ),
    route!(
        "/api/v1/timelines/tag/{hashtag}",
        Implemented,
        ApiAuthentication::Optional(READ_STATUSES.as_slice()),
        StatusId,
        Anonymous
    ),
    route!(
        "/api/v1/timelines/home",
        Implemented,
        ApiAuthentication::Required(READ_STATUSES.as_slice()),
        StatusId,
        Private
    ),
    route!(
        "/api/v1/timelines/list/{id}",
        Implemented,
        ApiAuthentication::Required(READ_LISTS.as_slice()),
        StatusId,
        Private
    ),
    route!(
        "/api/v1/favourites",
        Implemented,
        ApiAuthentication::Required(READ_FAVOURITES.as_slice()),
        AssociationId,
        Private
    ),
    route!(
        "/api/v1/bookmarks",
        Implemented,
        ApiAuthentication::Required(READ_BOOKMARKS.as_slice()),
        AssociationId,
        Private
    ),
    route!(
        "/api/v1/blocks",
        Implemented,
        ApiAuthentication::Required(READ_BLOCKS.as_slice()),
        RelationshipId,
        Private
    ),
    route!(
        "/api/v1/mutes",
        Implemented,
        ApiAuthentication::Required(READ_MUTES.as_slice()),
        RelationshipId,
        Private
    ),
];

/// The REST routes required by `docs/v1-scope.md` for ordinary v1 clients.
///
/// This is deliberately separate from `API_ROUTE_INVENTORY`: the inventory also
/// contains harmless read-only compatibility routes used by the web client.
pub const V1_REQUIRED_API_ROUTES: &[(&str, ApiMethod, ApiRouteSupport)] = &[
    (
        "/api/v1/instance",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v2/instance",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/instance/extended_description",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/instance/rules",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/instance/translation_languages",
        ApiMethod::Get,
        ApiRouteSupport::DisabledResponse,
    ),
    (
        "/api/v1/accounts/{id}",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/verify_credentials",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/update_credentials",
        ApiMethod::Patch,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/lookup",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/search",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/announcements",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v2/search",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/relationships",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/{id}/statuses",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/{id}/followers",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/{id}/following",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/polls/{id}",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/polls/{id}/votes",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/reports",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}",
        ApiMethod::Patch,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}",
        ApiMethod::Put,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}",
        ApiMethod::Delete,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/source",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/history",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/quotes",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/interaction_policy",
        ApiMethod::Put,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{quoted_status_id}/quotes/{id}/revoke",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/context",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/bookmark",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/unbookmark",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/favourite",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/unfavourite",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/reblog",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/unreblog",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/favourited_by",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/statuses/{id}/reblogged_by",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/timelines/home",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/timelines/public",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/timelines/tag/{hashtag}",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/timelines/list/{id}",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/favourites",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/bookmarks",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/blocks",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/mutes",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/follow_requests",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/follow_requests/{id}/authorize",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/follow_requests/{id}/reject",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/{id}/follow",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/{id}/unfollow",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/{id}/remove_from_followers",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/{id}/block",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/{id}/unblock",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/{id}/mute",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/accounts/{id}/unmute",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/conversations",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/conversations/{id}/read",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/conversations/{id}/unread",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/conversations/{id}",
        ApiMethod::Delete,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/notifications",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v2/notifications",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/notifications/unread_count",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v2/notifications/unread_count",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/notifications/{id}",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v2/notifications/{id}",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/notifications/clear",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v2/notifications/clear",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/notifications/{id}/dismiss",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v2/notifications/{id}/dismiss",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v2/filters",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/preferences",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/apps",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/apps/verify_credentials",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/markers",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/markers",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/media",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/media/{id}",
        ApiMethod::Get,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/media/{id}",
        ApiMethod::Patch,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/media/{id}",
        ApiMethod::Put,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v1/media/{id}",
        ApiMethod::Delete,
        ApiRouteSupport::Implemented,
    ),
    (
        "/api/v2/media",
        ApiMethod::Post,
        ApiRouteSupport::Implemented,
    ),
];

#[derive(Clone)]
pub struct WebState {
    repository: Repository,
    queue: Option<Queue>,
    write_repository: Option<WriteRepository>,
    remote_fetcher: RemoteFetcher,
    remote_account_resolver: RemoteAccountResolver,
    signature_fetch_circuit: SignatureFetchCircuit,
    shared_rate_limiter: Option<SharedRateLimiter>,
    mail_config: Option<MailConfig>,
    password_reset_limiter: PasswordResetLimiter,
    browser_login_limiter: BrowserLoginLimiter,
    browser_reauthentication_limiter: BrowserReauthenticationLimiter,
    oauth_application_limiter: OAuthApplicationLimiter,
    media_proxy_limiter: MediaProxyLimiter,
    media_upload_limiter: MediaUploadLimiter,
    activitypub_inbox_limiter: ActivityPubInboxLimiter,
    remote_account_resolution_limiter: RemoteAccountResolutionLimiter,
    authenticator: BearerAuthenticator,
    origin: Url,
    local_domain: String,
    media_root_url: String,
    media_root: PaperclipRoot,
    media_route_path: String,
    media_route_authority: Option<String>,
    instance_runtime: InstanceRuntimeConfig,
    activity_cache: crate::activity::ActivityCache,
    frontend: FrontendAssets,
    csrf_signing_key: [u8; 32],
    trusted_proxies: Vec<IpNetwork>,
    allowed_hosts: Vec<String>,
}

impl WebState {
    /// Builds web state and securely opens the configured local media root.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the media root cannot be opened without following symlinks.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repository: Repository,
        origin: Url,
        local_domain: impl Into<String>,
        media_root_url: impl Into<String>,
        media_root_path: impl Into<PathBuf>,
        instance_runtime: InstanceRuntimeConfig,
        trusted_proxies: Vec<IpNetwork>,
        allowed_hosts: Vec<String>,
    ) -> std::io::Result<Self> {
        let media_root_url = media_root_url.into();
        let (media_route_path, media_route_authority) = media_route(&media_root_url);
        let frontend =
            FrontendAssets::load(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("public"))?;
        let mut instance_runtime = instance_runtime;
        if instance_runtime
            .thumbnail_url
            .ends_with("/packs/assets/preview.png")
            && let Some(path) = frontend.asset_url("images/preview.png")
        {
            instance_runtime.thumbnail_url = origin.join(&path).map_or(path, |url| url.to_string());
        }
        if instance_runtime.icons.is_empty() {
            instance_runtime.icons = FRONTEND_ANDROID_ICON_SIZES
                .iter()
                .filter_map(|size| {
                    let source = format!("icons/android-chrome-{size}x{size}.png");
                    let path = frontend.asset_url(&source)?;
                    let url = origin.join(&path).map_or(path, |url| url.to_string());
                    Some((url, format!("{size}x{size}")))
                })
                .collect();
        }
        let remote_fetcher = RemoteFetcher::default();
        Ok(Self {
            authenticator: BearerAuthenticator::new(repository.clone()),
            repository,
            queue: None,
            write_repository: None,
            remote_fetcher: remote_fetcher.clone(),
            remote_account_resolver: RemoteAccountResolver::new(remote_fetcher),
            signature_fetch_circuit: SignatureFetchCircuit::default(),
            shared_rate_limiter: None,
            mail_config: None,
            password_reset_limiter: PasswordResetLimiter::default(),
            browser_login_limiter: BrowserLoginLimiter::default(),
            browser_reauthentication_limiter: BrowserReauthenticationLimiter::default(),
            oauth_application_limiter: OAuthApplicationLimiter::default(),
            media_proxy_limiter: MediaProxyLimiter::default(),
            media_upload_limiter: MediaUploadLimiter::default(),
            activitypub_inbox_limiter: ActivityPubInboxLimiter::default(),
            remote_account_resolution_limiter: RemoteAccountResolutionLimiter::default(),
            origin,
            local_domain: local_domain.into(),
            media_root_url,
            media_root: PaperclipRoot::open(&media_root_path.into())?,
            media_route_path,
            media_route_authority,
            instance_runtime,
            activity_cache: crate::activity::ActivityCache::default(),
            frontend,
            csrf_signing_key: derive_browser_csrf_signing_key(&random_auth_token(32)),
            trusted_proxies,
            allowed_hosts,
        })
    }

    // Only for metadata that never serializes activity counts (manifest/rules).
    async fn static_instance(&self) -> sqlx::Result<InstanceProjection> {
        self.loader(None)
            .instance(
                self.instance_runtime.clone(),
                InstanceActivityCounts::default(),
            )
            .await
    }

    async fn instance(&self) -> sqlx::Result<InstanceProjection> {
        let counts = self
            .activity_cache
            .get(self.repository.activity_pool())
            .await;
        self.loader(None)
            .instance(self.instance_runtime.clone(), counts)
            .await
    }

    #[must_use]
    pub fn with_csrf_signing_secret(mut self, secret: &SecretString) -> Self {
        self.csrf_signing_key = derive_browser_csrf_signing_key(secret.expose_secret());
        self
    }

    /// Fixes the database-backed browser reauthentication budget clock for this
    /// test instance only. Other rate limits and browser/session clocks are unchanged.
    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn with_browser_reauthentication_test_time(mut self, unix_seconds: i64) -> Self {
        self.browser_reauthentication_limiter.fixed_time = Some(unix_seconds);
        self
    }

    #[must_use]
    pub fn with_write_repository(mut self, repository: WriteRepository) -> Self {
        if self.shared_rate_limiter.is_none() {
            self.shared_rate_limiter = Some(SharedRateLimiter::new(repository.pool().clone()));
        }
        self.write_repository = Some(repository.with_local_domain(self.local_domain.clone()));
        self
    }

    #[must_use]
    pub fn with_queue(mut self, queue: Queue) -> Self {
        self.shared_rate_limiter = Some(SharedRateLimiter::new(queue.pool().clone()));
        let remote_fetcher = self
            .remote_fetcher
            .with_operational_pool(queue.pool().clone());
        self.remote_fetcher = remote_fetcher.clone();
        self.remote_account_resolver = RemoteAccountResolver::new(remote_fetcher);
        self.queue = Some(queue);
        self
    }

    #[must_use]
    pub fn with_mail_config(mut self, config: MailConfig) -> Self {
        self.mail_config = Some(config);
        self
    }

    fn loader(&self, viewer_account_id: Option<i64>) -> RestProjectionLoader {
        RestProjectionLoader::new(
            self.repository.clone(),
            viewer_account_id,
            self.local_domain.clone(),
        )
    }

    fn serializer(&self) -> RestSerializer<'_> {
        RestSerializer::new(
            &self.origin,
            &self.local_domain,
            &self.media_root_url,
            Utc::now().naive_utc(),
        )
    }
}

fn media_route(media_root_url: &str) -> (String, Option<String>) {
    Url::parse(media_root_url).map_or_else(
        |_| (media_root_url.trim_end_matches('/').to_owned(), None),
        |url| {
            let authority = url.port().map_or_else(
                || url.host_str().unwrap_or_default().to_owned(),
                |port| format!("{}:{port}", url.host_str().unwrap_or_default()),
            );
            (url.path().trim_end_matches('/').to_owned(), Some(authority))
        },
    )
}

fn media_proxy_path(path: &str) -> Option<(i64, bool)> {
    let (id, suffix) = path.split_once('/').map_or((path, ""), |value| value);
    Some((
        path_id(id)?,
        suffix == "small" || suffix.ends_with("/small"),
    ))
}

fn cached_remote_media_response(
    state: &WebState,
    media: &MediaAttachment,
    small: bool,
    method: &Method,
    headers: &HeaderMap,
) -> Option<Response<Body>> {
    if media.processing.is_some_and(|state| state.0 != 2) {
        return None;
    }
    let file_name = media.file_file_name.as_deref()?;
    let content_type = media.file_content_type.as_deref()?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: media.id,
        remote: true,
        storage_schema_version: media.file_storage_schema_version,
        file_name: file_name.to_owned(),
        content_type: Some(content_type.to_owned()),
        variant: None,
    };
    let style = if small { "small" } else { "original" };
    let relative_path = metadata.relative_path(style)?;
    let file = state
        .media_root
        .open_file(FsPath::new(&relative_path))
        .ok()?;
    let file_metadata = file.metadata().ok()?;
    let limit = if small {
        MEDIA_PROXY_MAX_RESPONSE_BYTES
    } else {
        crate::media::media_format(content_type)?.input_size_limit
    };
    if file_metadata.len() > u64::try_from(limit).ok()? {
        return None;
    }
    let content_type = metadata.media_file_content_type(style)?;
    let mut response = paperclip_file_response(
        method,
        headers,
        FsPath::new(&relative_path),
        file,
        &file_metadata,
        PRIVATE_CACHE,
        Some(PAPERCLIP_STATUS_VARY),
    );
    // Multipart has its own envelope MIME; its parts use the known cached suffix.
    if matches!(
        response.status(),
        StatusCode::OK | StatusCode::PARTIAL_CONTENT
    ) && !response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"multipart/"))
    {
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_str(content_type).ok()?);
    }
    Some(response)
}

async fn media_proxy(
    State(state): State<WebState>,
    Path(path): Path<String>,
    Extension(metadata): Extension<RequestMetadata>,
    method: Method,
    headers: HeaderMap,
) -> Response<Body> {
    let mut response = media_proxy_inner(&state, &path, metadata, &method, &headers).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(PRIVATE_CACHE));
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static(PAPERCLIP_STATUS_VARY));
    response
}

// A missing rich poster is not permission to fetch its original as an image.
fn remote_media_proxy_source(media: &MediaAttachment, small: bool) -> Option<&str> {
    if media.file_file_name.is_some() {
        return None; // An installed representation must not silently change back to its source.
    }
    if !small {
        return Some(&media.remote_url);
    }
    media
        .thumbnail_remote_url
        .as_deref()
        .filter(|url| !crate::paperclip::rails_blank(url))
        .or_else(|| {
            matches!(
                media.file_content_type.as_deref(),
                Some("image/jpeg" | "image/png" | "image/gif" | "image/webp")
            )
            .then_some(media.remote_url.as_str())
        })
}

#[allow(clippy::too_many_lines)]
async fn media_proxy_inner(
    state: &WebState,
    path: &str,
    metadata: RequestMetadata,
    method: &Method,
    headers: &HeaderMap,
) -> Response<Body> {
    if let Err(limited) = state
        .media_proxy_limiter
        .check_shared(state.shared_rate_limiter.as_ref(), metadata.client_ip)
        .await
    {
        return rate_limited_response(limited);
    }
    let viewer_account_id = match paperclip_viewer(state, headers, true).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((id, small)) = media_proxy_path(path) else {
        return not_found();
    };
    let media = match state.repository.remote_media_attachment(id).await {
        Ok(Some(media)) => media,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    let Some(status_id) = media.status_id else {
        return not_found();
    };
    match state
        .repository
        .rest_authorized_status_ids(&[status_id], viewer_account_id)
        .await
    {
        Ok(ids) if ids == [status_id] => {}
        Ok(_) => return not_found(),
        Err(_) => return internal_error(),
    }
    let Some(account_id) = media.account_id else {
        return not_found();
    };
    let account = match state.repository.account(account_id).await {
        Ok(Some(account)) => account,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    let Some(account_domain) = account.domain else {
        return not_found();
    };
    let Ok(account_domain) = canonical_remote_domain(&account_domain) else {
        return not_found();
    };
    if !state
        .repository
        .remote_media_allowed(&account_domain, state.instance_runtime.limited_federation)
        .await
        .unwrap_or(false)
    {
        return not_found();
    }
    if let Some(response) = cached_remote_media_response(state, &media, small, method, headers) {
        return response;
    }
    let Some(remote_url) = remote_media_proxy_source(&media, small) else {
        return not_found();
    };
    let Ok(remote_url) = Url::parse(remote_url) else {
        return not_found();
    };
    let response = match state
        .remote_fetcher
        .with_limits(RemoteFetchLimits {
            max_response_bytes: MEDIA_PROXY_MAX_RESPONSE_BYTES,
            ..RemoteFetchLimits::default()
        })
        .get(
            remote_url,
            if small {
                &["image/jpeg", "image/png", "image/gif", "image/webp"]
            } else {
                SUPPORTED_MIME_TYPES
            },
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return match error {
                crate::remote::RemoteFetchError::InvalidUrl
                | crate::remote::RemoteFetchError::BlockedAddress(_)
                | crate::remote::RemoteFetchError::MissingContentType
                | crate::remote::RemoteFetchError::UnsupportedContentType
                | crate::remote::RemoteFetchError::UnsupportedEncoding
                | crate::remote::RemoteFetchError::BodyTooLarge => not_found(),
                crate::remote::RemoteFetchError::UnexpectedStatus(status)
                    if status.is_client_error() =>
                {
                    not_found()
                }
                crate::remote::RemoteFetchError::DomainBudgetExceeded => error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Remote media is temporarily unavailable",
                ),
                _ => internal_error(),
            };
        }
    };
    let content_type = response
        .content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(media.file_content_type.as_deref())
        .unwrap_or("application/octet-stream");
    if small
        && (image::guess_format(&response.body)
            .ok()
            .is_none_or(|format| format.to_mime_type() != content_type)
            || (media
                .thumbnail_remote_url
                .as_deref()
                .is_none_or(crate::paperclip::rails_blank)
                && media.file_content_type.as_deref() != Some(content_type)))
    {
        return not_found();
    }
    let mut output = Response::new(Body::from(response.body));
    *output.status_mut() = StatusCode::OK;
    output.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=60"),
    );
    output.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    output.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    output
}

async fn frontend_app(State(state): State<WebState>, request: Request) -> Response<Body> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return not_found();
    }
    let path = request.uri().path();
    if !is_frontend_path(path) {
        return not_found();
    }
    frontend_html_response(&state, path, request.headers()).await
}

async fn web_fallback(State(state): State<WebState>, request: Request) -> Response<Body> {
    if is_frontend_path(request.uri().path()) {
        frontend_app(State(state), request).await
    } else {
        api_not_found()
    }
}

async fn frontend_html_response(
    state: &WebState,
    path: &str,
    headers: &HeaderMap,
) -> Response<Body> {
    let session = match request_cookie(headers, BROWSER_SESSION_COOKIE) {
        Some(session_id) => match state.repository.browser_session(session_id).await {
            Ok(session) => session,
            Err(_) => return internal_error(),
        },
        None => None,
    };
    let authenticated = if let Some(session) = session.as_ref() {
        if let Some(writer) = state.write_repository.as_ref()
            && writer
                .track_interactive_user(session.user_id)
                .await
                .is_err()
        {
            return internal_error();
        }
        let loader = state.loader(Some(session.account_id));
        let Ok(Some(credential)) = loader
            .credential_account(session.user_id, session.account_id)
            .await
        else {
            return internal_error();
        };
        let Ok(Some(preferences)) = loader
            .preferences(session.user_id, session.account_id)
            .await
        else {
            return internal_error();
        };
        let Ok(serialized) = state.serializer().credential_account(&credential) else {
            return internal_error();
        };
        let Ok(settings) = state
            .repository
            .web_settings(session.user_id, session.account_id)
            .await
        else {
            return internal_error();
        };
        Some(FrontendAuthenticatedState {
            account: serialized.account,
            access_token: session.access_token.as_str().to_owned(),
            account_id: session.account_id,
            preferences: state.serializer().preferences(&preferences),
            settings,
            role: serialized.role,
        })
    } else {
        None
    };
    let Ok(instance) = state.instance().await else {
        return internal_error();
    };
    let secure = state.origin.scheme() == "https";
    let (csrf_token, csrf_cookie) = browser_page_csrf(headers, secure, &state.csrf_signing_key);
    let csp_nonce = random_auth_token(32);
    let Some(serialized_instance) = state
        .serializer()
        .instance_v2(&instance)
        .ok()
        .and_then(|value| serde_json::to_value(value).ok())
    else {
        return internal_error();
    };
    let Some(document) = frontend_document(
        &state.frontend,
        &state.instance_runtime,
        path,
        Some(&instance),
        Some(serialized_instance),
        authenticated.as_ref(),
        &csrf_token,
        &csp_nonce,
    ) else {
        return internal_error();
    };
    let mut response = html_response(StatusCode::OK, document);
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_str(&frontend_content_security_policy(&csp_nonce))
            .expect("frontend CSP nonce is a valid header value"),
    );
    if let Some(cookie) = csrf_cookie {
        append_cookie(&mut response, &cookie);
    }
    response
}

async fn frontend_manifest(State(state): State<WebState>) -> Response<Body> {
    let Ok(instance) = state.static_instance().await else {
        return internal_error();
    };
    let Some(value) = frontend_manifest_value(&state.frontend, &instance.title) else {
        return internal_error();
    };
    let mut response = json_response(
        StatusCode::OK,
        serde_json::to_vec(&value).expect("frontend manifest is serializable"),
    );
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=180"),
    );
    response
}

async fn frontend_service_worker(State(state): State<WebState>) -> Response<Body> {
    let mut response = frontend_file_response(
        &state.frontend,
        "packs/sw.js",
        "text/javascript; charset=utf-8",
        "no-cache",
    );
    if response.status().is_success() {
        response
            .headers_mut()
            .insert("service-worker-allowed", HeaderValue::from_static("/"));
    }
    response
}

async fn frontend_favicon(State(state): State<WebState>) -> Response<Body> {
    let Some(path) = state.frontend.asset("icons/favicon-32x32.png") else {
        return not_found();
    };
    frontend_file_response(
        &state.frontend,
        &format!("packs/{}", path.file),
        "image/png",
        FRONTEND_CACHE,
    )
}

async fn frontend_android_icon(State(state): State<WebState>) -> Response<Body> {
    let Some(path) = state.frontend.asset("icons/android-chrome-192x192.png") else {
        return not_found();
    };
    frontend_file_response(
        &state.frontend,
        &format!("packs/{}", path.file),
        "image/png",
        FRONTEND_CACHE,
    )
}

fn frontend_file_response(
    frontend: &FrontendAssets,
    relative: &str,
    content_type: &str,
    cache_control: &str,
) -> Response<Body> {
    let Some(path) = frontend.path(relative) else {
        return not_found();
    };
    let Ok(body) = std::fs::read(path) else {
        return not_found();
    };
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_str(cache_control)
            .unwrap_or_else(|_| HeaderValue::from_static("no-cache")),
    );
    response
}

async fn rustodon_stylesheet() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/css; charset=utf-8")
        .body(Body::from(RUSTODON_STYLESHEET))
        .expect("static stylesheet response headers are valid")
}

fn frontend_asset_cache_control(target: &str) -> &'static str {
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    [(RUSTODON_STYLESHEET_URL, FRONTEND_CACHE)]
        .into_iter()
        .find(|(asset, _)| *asset == path)
        .map_or(FRONTEND_CACHE, |(_, policy)| policy)
}

async fn frontend_asset_headers(request: Request, next: Next) -> Response<Body> {
    let target = request
        .uri()
        .path_and_query()
        .map_or(request.uri().path(), |target| target.as_str());
    let cache_control = frontend_asset_cache_control(target);
    let mut response = next.run(request).await;
    if response.status().is_success() {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    }
    response
}

fn frontend_manifest_value(frontend: &FrontendAssets, title: &str) -> Option<serde_json::Value> {
    let icons = FRONTEND_ANDROID_ICON_SIZES
        .iter()
        .map(|size| {
            let source = format!("icons/android-chrome-{size}x{size}.png");
            let src = frontend.asset_url(&source)?;
            Some(serde_json::json!({
                "src": src,
                "sizes": format!("{size}x{size}"),
                "type": "image/png",
                "purpose": "any maskable",
            }))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(serde_json::json!({
        "instance": {
            "id": "/home",
            "name": title,
            "short_name": title,
            "icons": icons,
            "theme_color": "#191b22",
            "background_color": "#191b22",
            "display": "standalone",
            "start_url": "/",
            "scope": "/",
            "share_target": {
                "url_template": "share?title={title}&text={text}&url={url}",
                "action": "share",
                "method": "GET",
                "enctype": "application/x-www-form-urlencoded",
                "params": {"title": "title", "text": "text", "url": "url"},
            },
            "shortcuts": [
                {"name": "Compose new post", "url": "/publish"},
                {"name": "Notifications", "url": "/notifications"},
                {"name": "Explore", "url": "/explore"},
            ],
            "prefer_related_applications": true,
            "related_applications": [
                {
                    "platform": "play",
                    "url": "https://play.google.com/store/apps/details?id=org.joinmastodon.android",
                    "id": "org.joinmastodon.android",
                },
                {
                    "platform": "itunes",
                    "url": "https://apps.apple.com/us/app/mastodon-for-iphone/id1571998974",
                    "id": "id1571998974",
                },
                {
                    "platform": "f-droid",
                    "url": "https://f-droid.org/en/packages/org.joinmastodon.android/",
                    "id": "org.joinmastodon.android",
                },
            ],
        },
    }))
}

#[allow(clippy::too_many_arguments)]
fn frontend_document(
    frontend: &FrontendAssets,
    runtime: &InstanceRuntimeConfig,
    path: &str,
    instance: Option<&InstanceProjection>,
    serialized_instance: Option<serde_json::Value>,
    authenticated: Option<&FrontendAuthenticatedState>,
    csrf_token: &str,
    csp_nonce: &str,
) -> Option<String> {
    let theme = frontend.entry("styles/application.scss")?;
    let inert = frontend.entry("styles/entrypoints/inert.scss")?;
    let common = frontend.entry("entrypoints/common.ts")?;
    let application = frontend.entry("entrypoints/application.ts")?;
    let logo = frontend.asset_url("images/logo.svg")?;
    let logo_symbol = frontend.asset_url("images/logo-symbol-icon.svg")?;
    let mut initial_state = frontend_initial_state(runtime, instance, authenticated)?;
    if let Some(value) = serialized_instance {
        initial_state["instance"] = value;
    }
    let initial_state = json_script(&initial_state)?;
    let props = json_script(&serde_json::json!({"locale": "en"}))?;
    let title = instance.map_or("Mastodon", |value| value.title.as_str());
    let vapid_public_key = runtime.vapid_public_key.as_deref().unwrap_or_default();

    let mut favicon_tags = String::new();
    for size in [16_u16, 32, 48] {
        let source = format!("icons/favicon-{size}x{size}.png");
        let url = frontend.asset_url(&source)?;
        let _ = write!(
            favicon_tags,
            "<link rel=\"icon\" sizes=\"{size}x{size}\" href=\"{}\" type=\"image/png\">",
            html_escape::encode_quoted_attribute(&url),
        );
    }
    let theme_url = frontend.entry_url("styles/application.scss")?;
    let inert_url = frontend.entry_url("styles/entrypoints/inert.scss")?;
    let common_url = frontend.entry_url("entrypoints/common.ts")?;
    let application_url = frontend.entry_url("entrypoints/application.ts")?;
    let theme_integrity = integrity_attribute(theme.integrity.as_deref());
    let inert_integrity = integrity_attribute(inert.integrity.as_deref());
    let common_integrity = integrity_attribute(common.integrity.as_deref());
    let application_integrity = integrity_attribute(application.integrity.as_deref());
    let escaped_title = html_escape::encode_text(title);
    let escaped_path = html_escape::encode_quoted_attribute(path);
    let escaped_csrf = html_escape::encode_quoted_attribute(csrf_token);
    let escaped_csp_nonce = html_escape::encode_quoted_attribute(csp_nonce);
    let escaped_vapid = html_escape::encode_quoted_attribute(vapid_public_key);
    let escaped_props = html_escape::encode_quoted_attribute(&props);

    Some(format!(
        "<!doctype html><html lang=\"en\" data-contrast=\"auto\" data-color-scheme=\"auto\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover\">{favicon_tags}<link rel=\"mask-icon\" href=\"{}\" color=\"#6364FF\"><link rel=\"manifest\" href=\"/manifest\"><script nonce=\"{escaped_csp_nonce}\">{FRONTEND_THEME_SELECTION}</script><meta name=\"theme-color\" content=\"#191b22\"><meta name=\"mobile-web-app-capable\" content=\"yes\"><title>{escaped_title}</title><link rel=\"stylesheet\" href=\"{}\" media=\"all\" crossorigin=\"anonymous\"{theme_integrity}><link rel=\"stylesheet\" id=\"inert-style\" href=\"{}\" media=\"all\" crossorigin=\"anonymous\"{inert_integrity}><meta name=\"csrf-token\" content=\"{escaped_csrf}\"><meta name=\"applicationServerKey\" content=\"{escaped_vapid}\"><meta name=\"initialPath\" content=\"{escaped_path}\"><script id=\"initial-state\" type=\"application/json\" nonce=\"{escaped_csp_nonce}\">{initial_state}</script><script type=\"module\" crossorigin=\"anonymous\" src=\"{}\"{common_integrity}></script><script type=\"module\" crossorigin=\"anonymous\" src=\"{}\"{application_integrity}></script></head><body class=\"app-body\"><div class=\"notranslate app-holder\" id=\"mastodon\" data-props=\"{escaped_props}\"><noscript><img src=\"{}\" alt=\"Mastodon\"><div>JavaScript is required to use Mastodon. See <a href=\"https://joinmastodon.org/apps\">the Mastodon apps</a>.</div></noscript></div></body></html>",
        html_escape::encode_quoted_attribute(&logo_symbol),
        html_escape::encode_quoted_attribute(&theme_url),
        html_escape::encode_quoted_attribute(&inert_url),
        html_escape::encode_quoted_attribute(&common_url),
        html_escape::encode_quoted_attribute(&application_url),
        html_escape::encode_quoted_attribute(&logo),
    ))
}

fn frontend_content_security_policy(nonce: &str) -> String {
    format!(
        "default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; font-src 'self' data: https:; img-src 'self' data: blob: https:; media-src 'self' data: blob: https:; manifest-src 'self'; connect-src 'self' https: wss:; script-src 'self' 'nonce-{nonce}' 'wasm-unsafe-eval'; style-src 'self'; worker-src 'self' blob:; frame-src 'self' https:"
    )
}

fn frontend_initial_state(
    runtime: &InstanceRuntimeConfig,
    instance: Option<&InstanceProjection>,
    authenticated: Option<&FrontendAuthenticatedState>,
) -> Option<serde_json::Value> {
    let title = instance.map_or("Mastodon", |value| value.title.as_str());
    let languages = runtime
        .languages
        .iter()
        .map(|language| {
            let name = if language == "en" {
                "English"
            } else {
                language.as_str()
            };
            serde_json::json!([language, name, name])
        })
        .collect::<Vec<_>>();
    let mut initial_state = serde_json::json!({
        "accounts": {},
        "compose": {"text": ""},
        "features": [],
        "languages": languages,
        "media_attachments": {"accept_content_types": []},
        "meta": {
            "access_token": "",
            "activity_api_enabled": false,
            "admin": "",
            "auto_play_gif": true,
            "display_media": "default",
            "domain": runtime.domain,
            "landing_page": "about",
            "limited_federation_mode": runtime.limited_federation,
            "locale": "en",
            "mascot": null,
            "profile_directory": false,
            "registrations_open": instance.is_some_and(|value| value.registrations_mode != "none"),
            "reduce_motion": false,
            "repository": runtime.source_url,
            "search_enabled": false,
            "single_user_mode": runtime.single_user_mode,
            "source_url": runtime.source_url,
            "status_page_url": instance.and_then(|value| value.status_page_url.clone()),
            "streaming_api_base_url": runtime.streaming_api,
            "title": title,
            "trends_enabled": false,
            "show_trends": false,
            "use_blurhash": true,
            "version": runtime.version,
            "terms_of_service_enabled": runtime.terms_of_service_url.is_some(),
            "local_live_feed_access": instance.map_or("public", |value| value.local_live_feed_access.as_str()),
            "remote_live_feed_access": instance.map_or("public", |value| value.remote_live_feed_access.as_str()),
            "local_topic_feed_access": instance.map_or("public", |value| value.local_topic_feed_access.as_str()),
            "remote_topic_feed_access": instance.map_or("public", |value| value.remote_topic_feed_access.as_str()),
        },
        "settings": {},
    });
    if let Some(authenticated) = authenticated {
        let account = serde_json::to_value(&authenticated.account).ok()?;
        let role = serde_json::to_value(&authenticated.role).ok()?;
        let account_id = authenticated.account_id.to_string();
        initial_state["accounts"][&account_id] = account;
        initial_state["compose"]["default_language"] =
            serde_json::json!(authenticated.preferences.posting_default_language);
        initial_state["compose"]["default_privacy"] =
            serde_json::json!(authenticated.preferences.posting_default_visibility);
        initial_state["compose"]["default_quote_policy"] =
            serde_json::json!(authenticated.preferences.posting_default_quote_policy);
        initial_state["compose"]["default_sensitive"] =
            serde_json::json!(authenticated.preferences.posting_default_sensitive);
        initial_state["compose"]["me"] = serde_json::json!(account_id);
        initial_state["meta"]["access_token"] = serde_json::json!(&authenticated.access_token);
        initial_state["meta"]["me"] = serde_json::json!(account_id);
        initial_state["role"] = role;
        initial_state["settings"] = authenticated.settings.clone();
    }
    Some(initial_state)
}

fn json_script(value: &serde_json::Value) -> Option<String> {
    Some(
        serde_json::to_string(value)
            .ok()?
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
            .replace('&', "\\u0026"),
    )
}

fn integrity_attribute(integrity: Option<&str>) -> String {
    integrity.map_or_else(String::new, |value| {
        format!(
            " integrity=\"{}\"",
            html_escape::encode_quoted_attribute(value)
        )
    })
}

fn is_frontend_path(path: &str) -> bool {
    const EXACT: &[&str] = &[
        "/",
        "/about",
        "/blocks",
        "/bookmarks",
        "/collections",
        "/conversations",
        "/deck",
        "/directory",
        "/domain_blocks",
        "/explore",
        "/favourites",
        "/follow_requests",
        "/followed_tags",
        "/getting-started",
        "/home",
        "/keyboard-shortcuts",
        "/links",
        "/lists",
        "/mutes",
        "/notifications",
        "/notifications_v2",
        "/overview",
        "/overview/about",
        "/pinned",
        "/privacy-policy",
        "/profile",
        "/public",
        "/public/local",
        "/public/remote",
        "/publish",
        "/search",
        "/start",
        "/statuses",
        "/terms-of-service",
    ];
    if EXACT.contains(&path) || path.starts_with("/@") {
        return true;
    }
    [
        "/collections/",
        "/deck/",
        "/explore/",
        "/links/",
        "/lists/",
        "/notifications/",
        "/notifications_v2/",
        "/profile/",
        "/start/",
        "/statuses/",
        "/tags/",
        "/terms-of-service/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

#[allow(clippy::too_many_lines)]
pub fn router(state: WebState) -> Router {
    let media_route = format!("{}/{{*path}}", state.media_route_path.trim_end_matches('/'));
    let federation = Router::new()
        .route("/.well-known/webfinger", get(federation_webfinger))
        .route("/.well-known/host-meta", get(federation_host_meta))
        .route("/.well-known/host-meta.json", get(federation_host_meta))
        .route("/.well-known/nodeinfo", get(federation_nodeinfo_discovery))
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_metadata),
        )
        .route("/nodeinfo/2.0", get(federation_nodeinfo))
        .route("/actor", get(federation_actor_instance))
        .route("/emojis/{id}", get(federation_emoji))
        .route("/actor/inbox", post(federation_inbox_instance))
        .route("/inbox", post(federation_inbox_shared))
        .route("/users/{username}", get(federation_actor_username))
        .route("/users/{username}/inbox", post(federation_inbox_username))
        .route("/@{username}", get(federation_actor_username))
        .route("/ap/users/{id}", get(federation_actor_id))
        .route("/ap/users/{account_id}/inbox", post(federation_inbox_id))
        .route(
            "/users/{username}/quote_authorizations/{id}",
            get(federation_quote_authorization_username),
        )
        .route(
            "/ap/users/{account_id}/quote_authorizations/{id}",
            get(federation_quote_authorization_id),
        )
        .route(
            "/users/{username}/statuses/{id}",
            get(federation_note_username),
        )
        .route(
            "/users/{username}/statuses/{id}/activity",
            get(federation_status_activity_username),
        )
        .route(
            "/ap/users/{account_id}/statuses/{id}",
            get(federation_note_id),
        )
        .route(
            "/ap/users/{account_id}/statuses/{id}/activity",
            get(federation_status_activity_id),
        )
        .route(
            "/users/{username}/statuses/{status_id}/{collection}",
            get(federation_status_collection_username),
        )
        .route(
            "/ap/users/{account_id}/statuses/{status_id}/{collection}",
            get(federation_status_collection_id),
        )
        .route("/users/{username}/outbox", get(federation_outbox_username))
        .route("/ap/users/{account_id}/outbox", get(federation_outbox_id))
        .route("/actor/outbox", get(federation_outbox_instance))
        .route(
            "/users/{username}/followers",
            get(federation_followers_username),
        )
        .route(
            "/users/{username}/following",
            get(federation_following_username),
        )
        .route(
            "/ap/users/{account_id}/followers",
            get(federation_followers_id),
        )
        .route(
            "/ap/users/{account_id}/following",
            get(federation_following_id),
        );
    let api = Router::new()
        .route(
            "/api/web/settings",
            axum::routing::put(web_settings::update).patch(web_settings::update),
        )
        .route(
            "/api/web/settings/",
            axum::routing::put(web_settings::update).patch(web_settings::update),
        )
        .route(
            "/auth/sign_in",
            get(browser_sign_in_page).post(browser_sign_in),
        )
        .route("/auth/password/new", get(browser_password_reset_page))
        .route("/auth/password/edit", get(browser_password_reset_edit))
        .route("/auth/confirmation", get(browser_confirmation))
        .route(
            "/auth/password",
            post(browser_password_reset_request)
                .patch(browser_password_reset_update)
                .put(browser_password_reset_update),
        )
        .route(
            "/auth/sign_out",
            post(browser_sign_out).delete(browser_sign_out),
        )
        .route("/auth/session", get(browser_session))
        .route("/settings", get(browser_settings_index))
        .route("/settings/", get(browser_settings_index))
        .route(
            "/settings/profile",
            get(browser_profile_page).post(browser_profile_update),
        )
        .route(
            "/settings/profile/",
            get(browser_profile_page).post(browser_profile_update),
        )
        .route(
            "/settings/preferences",
            get(browser_posting_defaults_redirect),
        )
        .route(
            "/settings/preferences/",
            get(browser_posting_defaults_redirect),
        )
        .route(
            "/settings/preferences/appearance",
            get(browser_settings_appearance),
        )
        .route(
            "/settings/preferences/appearance/",
            get(browser_settings_appearance),
        )
        .route(
            "/settings/preferences/posting_defaults",
            get(browser_posting_defaults_page).post(browser_posting_defaults_update),
        )
        .route(
            "/settings/preferences/posting_defaults/",
            get(browser_posting_defaults_page).post(browser_posting_defaults_update),
        )
        .route(
            "/settings/security",
            get(browser_security_page).post(browser_security_update),
        )
        .route(
            "/settings/security/",
            get(browser_security_page).post(browser_security_update),
        )
        .route(
            "/settings/two_factor_authentication_methods",
            get(browser_two_factor_methods_page),
        )
        .route(
            "/settings/two_factor_authentication_methods/disable",
            post(browser_two_factor_disable),
        )
        .route(
            "/settings/otp_authentication",
            get(browser_otp_authentication_page).post(browser_otp_authentication_start),
        )
        .route(
            "/settings/two_factor_authentication/confirmation",
            get(browser_otp_confirmation_redirect).post(browser_otp_confirmation),
        )
        .route(
            "/settings/two_factor_authentication/recovery_codes",
            post(browser_two_factor_recovery_codes),
        )
        .route(
            "/settings/delete",
            get(browser_delete_page)
                .post(browser_delete)
                .delete(browser_delete),
        )
        .route(
            "/settings/delete/",
            get(browser_delete_page)
                .post(browser_delete)
                .delete(browser_delete),
        )
        .route(
            "/oauth/authorize",
            get(oauth_authorize).post(oauth_authorize),
        )
        .route("/oauth/userinfo", get(oauth_userinfo).post(oauth_userinfo))
        .route("/oauth/token", post(oauth_token))
        .route("/oauth/revoke", post(oauth_revoke))
        .route("/health", get(health))
        .route("/ready", get(readiness))
        .route("/api/v1/streaming", get(streaming))
        .route("/api/v1/streaming/", get(streaming))
        .route("/api/v1/streaming/user", get(streaming))
        .route("/api/v1/streaming/user/notification", get(streaming))
        .route("/api/v1/streaming/direct", get(streaming))
        .route("/api/v1/streaming/public", get(streaming))
        .route("/api/v1/streaming/public/local", get(streaming))
        .route("/api/v1/streaming/public/remote", get(streaming))
        .route("/api/v1/streaming/hashtag", get(streaming))
        .route("/api/v1/streaming/hashtag/local", get(streaming))
        .route("/api/v1/streaming/list", get(streaming))
        .route("/api/v1/instance", get(instance_v1))
        .route("/api/v2/instance", get(instance_v2))
        .route(
            "/api/v1/instance/extended_description",
            get(extended_description::show),
        )
        .route("/api/v1/instance/rules", get(instance_rules))
        .route(
            "/api/v1/instance/translation_languages",
            get(translation_languages),
        )
        .route("/api/v1/custom_emojis", get(custom_emojis))
        .route("/api/v1/accounts/lookup", get(account_lookup))
        .route("/api/v1/accounts/search", get(account_search))
        .route("/api/v1/announcements", get(announcements))
        .route("/api/v2/search", get(search_v2))
        .route("/api/v1/apps", post(app_create))
        .route(
            "/api/v1/apps/verify_credentials",
            get(app_verify_credentials),
        )
        .route("/api/v1/markers", get(markers).post(marker_update))
        .route("/api/v1/statuses", post(status_create))
        .route("/api/v1/polls/{id}", get(poll_show))
        .route("/api/v1/polls/{id}/votes", post(poll_vote))
        .route("/api/v1/reports", post(report_create))
        .route("/api/v1/media", post(media_create_v1))
        .route(
            "/api/v1/media/{id}",
            get(media_show)
                .patch(media_update)
                .put(media_update)
                .delete(media_delete),
        )
        .route("/api/v2/media", post(media_create_v2))
        .route("/api/v1/conversations", get(conversations))
        .route("/api/v1/conversations/{id}/read", post(conversation_read))
        .route(
            "/api/v1/conversations/{id}/unread",
            post(conversation_unread),
        )
        .route("/api/v1/conversations/{id}", delete(conversation_delete))
        .route("/api/v1/notifications", get(notifications))
        .route("/api/v2/notifications", get(grouped_notifications))
        .route("/api/v1/notifications/clear", post(notification_clear))
        .route(
            "/api/v1/notifications/{id}/dismiss",
            post(notification_dismiss),
        )
        .route(
            "/api/v2/notifications/clear",
            post(grouped_notification_clear),
        )
        .route(
            "/api/v2/notifications/{id}/dismiss",
            post(grouped_notification_dismiss),
        )
        .route(
            "/api/v1/notifications/requests/{id}/accept",
            post(notification_request_accept),
        )
        .route(
            "/api/v1/notifications/requests/{id}/dismiss",
            post(notification_request_dismiss),
        )
        .route(
            "/api/v1/notifications/requests/accept",
            post(notification_requests_accept),
        )
        .route(
            "/api/v1/notifications/requests/dismiss",
            post(notification_requests_dismiss),
        )
        .route(
            "/api/v1/notifications/requests/merged",
            get(notification_requests_merged),
        )
        .route(
            "/api/v1/notifications/policy",
            get(notification_policy_v1).put(notification_policy_v1_update),
        )
        .route(
            "/api/v1/notifications/unread_count",
            get(notification_unread_count),
        )
        .route("/api/v1/notifications/{id}", get(notification_show))
        .route("/api/v1/notifications/requests", get(notification_requests))
        .route(
            "/api/v1/notifications/requests/{id}",
            get(notification_request_show),
        )
        .route(
            "/api/v2/notifications/unread_count",
            get(grouped_notification_unread_count),
        )
        .route("/api/v2/notifications/{id}", get(grouped_notification_show))
        .route(
            "/api/v2/notifications/policy",
            get(notification_policy_v2).put(notification_policy_v2_update),
        )
        .route("/api/v2/filters", get(filters))
        .route("/api/v1/lists", get(lists))
        .route("/api/v1/lists/{id}", get(list_show))
        .route("/api/v1/lists/{id}/accounts", get(list_accounts))
        .route("/api/v1/accounts/{id}/lists", get(account_lists))
        .route(
            "/api/v1/accounts/{id}/collections",
            get(account_collections),
        )
        .route(
            "/api/v1/accounts/{id}/in_collections",
            get(account_in_collections),
        )
        .route(
            "/api/v1/featured_tags",
            get(featured_tags).post(hashtag_controls::create_featured),
        )
        .route(
            "/api/v1/featured_tags/{id}",
            delete(hashtag_controls::delete_featured),
        )
        .route("/api/v1/tags/{tag}", get(hashtag_controls::show))
        .route("/api/v1/tags/{tag}/follow", post(hashtag_controls::mutate))
        .route(
            "/api/v1/tags/{tag}/unfollow",
            post(hashtag_controls::mutate),
        )
        .route("/api/v1/tags/{tag}/feature", post(hashtag_controls::mutate))
        .route(
            "/api/v1/tags/{tag}/unfeature",
            post(hashtag_controls::mutate),
        )
        .route("/api/v1/followed_tags", get(followed_tags))
        .route("/api/v1/follow_requests", get(follow_requests))
        .route(
            "/api/v1/follow_requests/{id}/authorize",
            post(authorize_follow_request),
        )
        .route(
            "/api/v1/follow_requests/{id}/reject",
            post(reject_follow_request),
        )
        .route("/api/v1/preferences", get(preferences))
        .route(
            "/api/v1/featured_tags/suggestions",
            get(featured_tag_suggestions),
        )
        .route(
            "/api/v1/accounts/{id}/featured_tags",
            get(account_featured_tags),
        )
        .route("/api/v1/accounts/relationships", get(relationships))
        .route("/api/v1/profile", get(profile))
        .route("/api/v1/profile/", get(profile))
        .route(
            "/api/v1/accounts/verify_credentials",
            get(verify_credentials),
        )
        .route(
            "/api/v1/accounts/update_credentials",
            patch(update_credentials),
        )
        .route("/api/v1/profile/avatar", delete(delete_profile_avatar))
        .route("/api/v1/profile/header", delete(delete_profile_header))
        .route("/api/v1/accounts", get(accounts_index))
        .route("/api/v1/accounts/{id}", get(account_show))
        .route("/api/v1/collections/{id}", get(collection_show))
        .route("/api/v1/accounts/{id}/statuses", get(account_statuses))
        .route("/api/v1/accounts/{id}/followers", get(account_followers))
        .route("/api/v1/accounts/{id}/following", get(account_following))
        .route("/api/v1/accounts/{id}/follow", post(follow_account))
        .route("/api/v1/accounts/{id}/unfollow", post(unfollow_account))
        .route(
            "/api/v1/accounts/{id}/remove_from_followers",
            post(remove_from_followers),
        )
        .route("/api/v1/accounts/{id}/block", post(block_account))
        .route("/api/v1/accounts/{id}/unblock", post(unblock_account))
        .route("/api/v1/accounts/{id}/mute", post(mute_account))
        .route("/api/v1/accounts/{id}/unmute", post(unmute_account))
        .route(
            "/api/v1/statuses/{id}",
            get(status_show)
                .patch(status_update)
                .put(status_update)
                .delete(status_delete),
        )
        .route("/api/v1/statuses/{id}/bookmark", post(bookmark_status))
        .route("/api/v1/statuses/{id}/unbookmark", post(unbookmark_status))
        .route("/api/v1/statuses/{id}/favourite", post(favourite_status))
        .route(
            "/api/v1/statuses/{id}/unfavourite",
            post(unfavourite_status),
        )
        .route("/api/v1/statuses/{id}/reblog", post(reblog_status))
        .route("/api/v1/statuses/{id}/unreblog", post(unreblog_status))
        .route("/api/v1/statuses/{id}/mute", post(mute_status))
        .route("/api/v1/statuses/{id}/unmute", post(unmute_status))
        .route("/api/v1/statuses/{id}/pin", post(pin_status))
        .route("/api/v1/statuses/{id}/unpin", post(unpin_status))
        .route("/api/v1/statuses/{id}/source", get(status_source))
        .route("/api/v1/statuses/{id}/history", get(status_history))
        .route("/api/v1/statuses/{id}/quotes", get(status_quotes))
        .route(
            "/api/v1/statuses/{id}/interaction_policy",
            put(status_interaction_policy_update),
        )
        .route(
            "/api/v1/statuses/{quoted_status_id}/quotes/{id}/revoke",
            post(revoke_quote),
        )
        .route("/api/v1/statuses/{id}/favourited_by", get(favourited_by))
        .route("/api/v1/statuses/{id}/reblogged_by", get(reblogged_by))
        .route("/api/v1/statuses/{id}/context", get(status_context))
        .route("/api/v1/timelines/public", get(public_timeline))
        .route("/api/v1/timelines/tag/{hashtag}", get(tag_timeline))
        .route("/api/v1/timelines/home", get(home_timeline))
        .route("/api/v1/timelines/list/{id}", get(list_timeline))
        .route("/api/v1/favourites", get(favourites))
        .route("/api/v1/bookmarks", get(bookmarks))
        .route("/api/v1/blocks", get(blocks))
        .route("/api/v1/mutes", get(mutes))
        .route("/api/v1/trends/tags", get(empty_discovery_read))
        .route("/api/v1/trends/tags/", get(empty_discovery_read))
        .route("/api/v1/trends/links", get(empty_discovery_read))
        .route("/api/v1/trends/links/", get(empty_discovery_read))
        .route("/api/v1/trends/statuses", get(empty_discovery_read))
        .route("/api/v1/trends/statuses/", get(empty_discovery_read))
        .route("/api/v1/directory", get(empty_discovery_read))
        .route("/api/v1/directory/", get(empty_discovery_read))
        .route("/api/v1/timelines/link", get(empty_link_timeline))
        .route("/api/v1/timelines/link/", get(empty_link_timeline))
        .route("/api/v2/suggestions", get(empty_suggestions))
        .route("/api/v2/suggestions/", get(empty_suggestions))
        .route("/api/v1/domain_blocks", get(domain_blocks))
        .route("/api/v1/domain_blocks/", get(domain_blocks))
        .route(
            "/api/v1/instance/domain_blocks",
            get(instance_domain_blocks),
        )
        .route(
            "/api/v1/instance/domain_blocks/",
            get(instance_domain_blocks),
        )
        .route(
            "/api/v1/accounts/familiar_followers",
            get(empty_familiar_followers),
        )
        .route(
            "/api/v1/accounts/familiar_followers/",
            get(empty_familiar_followers),
        )
        .route("/api/v1/instance/", get(instance_v1))
        .route("/api/v2/instance/", get(instance_v2))
        .route(
            "/api/v1/instance/extended_description/",
            get(extended_description::show),
        )
        .route("/api/v1/instance/rules/", get(instance_rules))
        .route(
            "/api/v1/instance/translation_languages/",
            get(translation_languages),
        )
        .route("/api/v1/custom_emojis/", get(custom_emojis))
        .route("/api/v1/accounts/lookup/", get(account_lookup))
        .route("/api/v1/accounts/search/", get(account_search))
        .route("/api/v1/announcements/", get(announcements))
        .route("/api/v2/search/", get(search_v2))
        .route("/api/v1/apps/", post(app_create))
        .route(
            "/api/v1/apps/verify_credentials/",
            get(app_verify_credentials),
        )
        .route("/api/v1/markers/", get(markers).post(marker_update))
        .route("/api/v1/statuses/", post(status_create))
        .route("/api/v1/polls/{id}/", get(poll_show))
        .route("/api/v1/polls/{id}/votes/", post(poll_vote))
        .route("/api/v1/reports/", post(report_create))
        .route("/api/v1/media/", post(media_create_v1))
        .route(
            "/api/v1/media/{id}/",
            get(media_show)
                .patch(media_update)
                .put(media_update)
                .delete(media_delete),
        )
        .route("/api/v2/media/", post(media_create_v2))
        .route("/api/v1/conversations/", get(conversations))
        .route("/api/v1/conversations/{id}/read/", post(conversation_read))
        .route(
            "/api/v1/conversations/{id}/unread/",
            post(conversation_unread),
        )
        .route("/api/v1/conversations/{id}/", delete(conversation_delete))
        .route("/api/v1/notifications/", get(notifications))
        .route("/api/v2/notifications/", get(grouped_notifications))
        .route("/api/v1/notifications/clear/", post(notification_clear))
        .route(
            "/api/v1/notifications/{id}/dismiss/",
            post(notification_dismiss),
        )
        .route(
            "/api/v2/notifications/clear/",
            post(grouped_notification_clear),
        )
        .route(
            "/api/v2/notifications/{id}/dismiss/",
            post(grouped_notification_dismiss),
        )
        .route(
            "/api/v1/notifications/requests/{id}/accept/",
            post(notification_request_accept),
        )
        .route(
            "/api/v1/notifications/requests/{id}/dismiss/",
            post(notification_request_dismiss),
        )
        .route(
            "/api/v1/notifications/requests/accept/",
            post(notification_requests_accept),
        )
        .route(
            "/api/v1/notifications/requests/dismiss/",
            post(notification_requests_dismiss),
        )
        .route(
            "/api/v1/notifications/requests/merged/",
            get(notification_requests_merged),
        )
        .route(
            "/api/v1/notifications/policy/",
            get(notification_policy_v1).put(notification_policy_v1_update),
        )
        .route(
            "/api/v1/notifications/unread_count/",
            get(notification_unread_count),
        )
        .route("/api/v1/notifications/{id}/", get(notification_show))
        .route(
            "/api/v1/notifications/requests/",
            get(notification_requests),
        )
        .route(
            "/api/v1/notifications/requests/{id}/",
            get(notification_request_show),
        )
        .route(
            "/api/v2/notifications/unread_count/",
            get(grouped_notification_unread_count),
        )
        .route(
            "/api/v2/notifications/{id}/",
            get(grouped_notification_show),
        )
        .route(
            "/api/v2/notifications/policy/",
            get(notification_policy_v2).put(notification_policy_v2_update),
        )
        .route("/api/v2/filters/", get(filters))
        .route("/api/v1/lists/", get(lists))
        .route("/api/v1/lists/{id}/", get(list_show))
        .route("/api/v1/lists/{id}/accounts/", get(list_accounts))
        .route("/api/v1/accounts/{id}/lists/", get(account_lists))
        .route(
            "/api/v1/accounts/{id}/collections/",
            get(account_collections),
        )
        .route(
            "/api/v1/accounts/{id}/in_collections/",
            get(account_in_collections),
        )
        .route(
            "/api/v1/featured_tags/",
            get(featured_tags).post(hashtag_controls::create_featured),
        )
        .route(
            "/api/v1/featured_tags/{id}/",
            delete(hashtag_controls::delete_featured),
        )
        .route("/api/v1/tags/{tag}/", get(hashtag_controls::show))
        .route("/api/v1/tags/{tag}/follow/", post(hashtag_controls::mutate))
        .route(
            "/api/v1/tags/{tag}/unfollow/",
            post(hashtag_controls::mutate),
        )
        .route(
            "/api/v1/tags/{tag}/feature/",
            post(hashtag_controls::mutate),
        )
        .route(
            "/api/v1/tags/{tag}/unfeature/",
            post(hashtag_controls::mutate),
        )
        .route("/api/v1/followed_tags/", get(followed_tags))
        .route("/api/v1/follow_requests/", get(follow_requests))
        .route(
            "/api/v1/follow_requests/{id}/authorize/",
            post(authorize_follow_request),
        )
        .route(
            "/api/v1/follow_requests/{id}/reject/",
            post(reject_follow_request),
        )
        .route("/api/v1/preferences/", get(preferences))
        .route(
            "/api/v1/featured_tags/suggestions/",
            get(featured_tag_suggestions),
        )
        .route(
            "/api/v1/accounts/{id}/featured_tags/",
            get(account_featured_tags),
        )
        .route("/api/v1/accounts/relationships/", get(relationships))
        .route(
            "/api/v1/accounts/verify_credentials/",
            get(verify_credentials),
        )
        .route(
            "/api/v1/accounts/update_credentials/",
            patch(update_credentials),
        )
        .route("/api/v1/profile/avatar/", delete(delete_profile_avatar))
        .route("/api/v1/profile/header/", delete(delete_profile_header))
        .route("/api/v1/accounts/", get(accounts_index))
        .route("/api/v1/accounts/{id}/", get(account_show))
        .route("/api/v1/collections/{id}/", get(collection_show))
        .route("/api/v1/accounts/{id}/statuses/", get(account_statuses))
        .route("/api/v1/accounts/{id}/followers/", get(account_followers))
        .route("/api/v1/accounts/{id}/following/", get(account_following))
        .route("/api/v1/accounts/{id}/follow/", post(follow_account))
        .route("/api/v1/accounts/{id}/unfollow/", post(unfollow_account))
        .route(
            "/api/v1/accounts/{id}/remove_from_followers/",
            post(remove_from_followers),
        )
        .route("/api/v1/accounts/{id}/block/", post(block_account))
        .route("/api/v1/accounts/{id}/unblock/", post(unblock_account))
        .route("/api/v1/accounts/{id}/mute/", post(mute_account))
        .route("/api/v1/accounts/{id}/unmute/", post(unmute_account))
        .route(
            "/api/v1/statuses/{id}/",
            get(status_show)
                .patch(status_update)
                .put(status_update)
                .delete(status_delete),
        )
        .route("/api/v1/statuses/{id}/bookmark/", post(bookmark_status))
        .route("/api/v1/statuses/{id}/unbookmark/", post(unbookmark_status))
        .route("/api/v1/statuses/{id}/favourite/", post(favourite_status))
        .route(
            "/api/v1/statuses/{id}/unfavourite/",
            post(unfavourite_status),
        )
        .route("/api/v1/statuses/{id}/reblog/", post(reblog_status))
        .route("/api/v1/statuses/{id}/unreblog/", post(unreblog_status))
        .route("/api/v1/statuses/{id}/mute/", post(mute_status))
        .route("/api/v1/statuses/{id}/unmute/", post(unmute_status))
        .route("/api/v1/statuses/{id}/pin/", post(pin_status))
        .route("/api/v1/statuses/{id}/unpin/", post(unpin_status))
        .route("/api/v1/statuses/{id}/source/", get(status_source))
        .route("/api/v1/statuses/{id}/history/", get(status_history))
        .route("/api/v1/statuses/{id}/quotes/", get(status_quotes))
        .route(
            "/api/v1/statuses/{id}/interaction_policy/",
            put(status_interaction_policy_update),
        )
        .route(
            "/api/v1/statuses/{quoted_status_id}/quotes/{id}/revoke/",
            post(revoke_quote),
        )
        .route("/api/v1/statuses/{id}/favourited_by/", get(favourited_by))
        .route("/api/v1/statuses/{id}/reblogged_by/", get(reblogged_by))
        .route("/api/v1/statuses/{id}/context/", get(status_context))
        .route("/api/v1/timelines/public/", get(public_timeline))
        .route("/api/v1/timelines/tag/{hashtag}/", get(tag_timeline))
        .route("/api/v1/timelines/home/", get(home_timeline))
        .route("/api/v1/timelines/list/{id}/", get(list_timeline))
        .route("/api/v1/favourites/", get(favourites))
        .route("/api/v1/bookmarks/", get(bookmarks))
        .route("/api/v1/blocks/", get(blocks))
        .route("/api/v1/mutes/", get(mutes))
        .method_not_allowed_fallback(|| async { api_not_found() })
        .reset_fallback()
        .layer(middleware::from_fn_with_state(state.clone(), api_protocol));
    let media_proxy = Router::new().route("/media_proxy/{*path}", get(media_proxy));
    let frontend_assets = Router::new()
        .nest_service("/packs", ServeDir::new(state.frontend.root.join("packs")))
        .layer(middleware::from_fn(frontend_asset_headers));
    let frontend_public_assets = Router::new()
        .route(RUSTODON_STYLESHEET_URL, get(rustodon_stylesheet))
        .route_service(
            "/badge.png",
            ServeFile::new(state.frontend.root.join("badge.png")),
        )
        .route_service(
            "/loading.gif",
            ServeFile::new(state.frontend.root.join("loading.gif")),
        )
        .route_service(
            "/loading.png",
            ServeFile::new(state.frontend.root.join("loading.png")),
        )
        .route_service(
            "/oops.gif",
            ServeFile::new(state.frontend.root.join("oops.gif")),
        )
        .route_service(
            "/oops.png",
            ServeFile::new(state.frontend.root.join("oops.png")),
        )
        .route_service(
            "/web-push-icon_expand.png",
            ServeFile::new(state.frontend.root.join("web-push-icon_expand.png")),
        )
        .route_service(
            "/web-push-icon_favourite.png",
            ServeFile::new(state.frontend.root.join("web-push-icon_favourite.png")),
        )
        .route_service(
            "/web-push-icon_reblog.png",
            ServeFile::new(state.frontend.root.join("web-push-icon_reblog.png")),
        )
        .route("/android-chrome-192x192.png", get(frontend_android_icon))
        .nest_service(
            "/avatars",
            ServeDir::new(state.frontend.root.join("avatars")),
        )
        .nest_service("/emoji", ServeDir::new(state.frontend.root.join("emoji")))
        .nest_service(
            "/headers",
            ServeDir::new(state.frontend.root.join("headers")),
        )
        .nest_service("/ocr", ServeDir::new(state.frontend.root.join("ocr")))
        .nest_service("/sounds", ServeDir::new(state.frontend.root.join("sounds")))
        .layer(middleware::from_fn(frontend_asset_headers));
    Router::new()
        .route("/manifest", get(frontend_manifest))
        .route("/manifest.json", get(frontend_manifest))
        .route("/sw.js", get(frontend_service_worker))
        .route("/favicon.ico", get(frontend_favicon))
        .route(&media_route, any(paperclip_media))
        .merge(media_proxy)
        .merge(frontend_assets)
        .merge(frontend_public_assets)
        .merge(federation)
        .merge(api)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_context,
        ))
        .fallback(web_fallback)
        .with_state(state)
}

const FORWARDED_HEADERS: &[&str] = &[
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-proto",
    "x-real-ip",
    "client-ip",
];

fn accepts_activitypub(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get("accept").and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let mut best_activity = None;
    let mut best_html = None;
    for (index, entry) in value.split(',').enumerate() {
        let mut parameters = entry.split(';');
        let media_type = parameters.next().unwrap_or_default().trim();
        let is_activity = media_type.eq_ignore_ascii_case("application/activity+json")
            || media_type.eq_ignore_ascii_case("application/ld+json")
            || media_type.eq_ignore_ascii_case("application/json");
        let is_html = media_type.eq_ignore_ascii_case("text/html");
        if !is_activity && !is_html {
            continue;
        }
        let quality = parameters
            .find_map(|parameter| {
                let (name, value) = parameter.trim().split_once('=')?;
                name.trim()
                    .eq_ignore_ascii_case("q")
                    .then_some(value.trim())
            })
            .map_or(1.0, |quality| {
                quality
                    .parse::<f32>()
                    .ok()
                    .filter(|quality| quality.is_finite())
                    .unwrap_or(0.0)
            });
        if quality <= 0.0 {
            continue;
        }
        let best = if is_activity {
            &mut best_activity
        } else {
            &mut best_html
        };
        if best.is_none_or(|(current, current_index)| {
            matches!(quality.total_cmp(&current), std::cmp::Ordering::Greater)
                || (quality.total_cmp(&current) == std::cmp::Ordering::Equal
                    && index < current_index)
        }) {
            *best = Some((quality, index));
        }
    }
    best_activity.is_some_and(|(activity_quality, activity_index)| {
        best_html.is_none_or(|(html_quality, html_index)| {
            matches!(
                activity_quality.total_cmp(&html_quality),
                std::cmp::Ordering::Greater
            ) || (activity_quality.total_cmp(&html_quality) == std::cmp::Ordering::Equal
                && activity_index < html_index)
        })
    })
}

fn accepts_html(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(ACCEPT).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    value.split(',').any(|entry| {
        let mut parameters = entry.split(';');
        let media_type = parameters.next().unwrap_or_default().trim();
        if !media_type.eq_ignore_ascii_case("text/html") && !media_type.eq_ignore_ascii_case("*/*")
        {
            return false;
        }
        let quality = parameters
            .find_map(|parameter| {
                let (name, value) = parameter.trim().split_once('=')?;
                name.trim()
                    .eq_ignore_ascii_case("q")
                    .then_some(value.trim())
            })
            .map_or(1.0, |quality| {
                quality
                    .parse::<f32>()
                    .ok()
                    .filter(|quality| quality.is_finite())
                    .unwrap_or(0.0)
            });
        quality > 0.0
    })
}

fn activitypub_truthy(value: &str) -> bool {
    !value.is_empty() && !matches!(value, "0" | "f" | "F" | "false" | "FALSE" | "off" | "OFF")
}

fn html_redirect(origin: &Url, location: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header(
            LOCATION,
            origin
                .join(location)
                .expect("origin and redirect path are valid")
                .as_str(),
        )
        .header(VARY, "Origin, Accept")
        .body(Body::empty())
        .expect("HTML redirect response headers are valid")
}

#[allow(clippy::needless_pass_by_value)]
fn activity_response(
    status: StatusCode,
    content_type: &str,
    value: serde_json::Value,
) -> Response<Body> {
    raw_response(
        status,
        content_type,
        serde_json::to_vec(&value).expect("ActivityPub value is serializable"),
    )
}

fn raw_response(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .header(VARY, "Accept, Signature")
        .body(Body::from(body))
        .expect("federation response headers are valid")
}

fn request_host_authority(value: &str) -> Option<(String, Option<u16>)> {
    let url = Url::parse(&format!("http://{value}")).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = match url.host()? {
        Host::Domain(domain) => domain.to_ascii_lowercase(),
        Host::Ipv4(address) => address.to_string(),
        Host::Ipv6(address) => format!("[{address}]"),
    };
    Some((host, url.port()))
}

fn request_host_matches_allowed(host: &str, allowed_hosts: &[String]) -> bool {
    let Some((host, port)) = request_host_authority(host) else {
        return false;
    };
    allowed_hosts.iter().any(|allowed| {
        let Some((allowed_host, allowed_port)) = request_host_authority(allowed) else {
            return false;
        };
        host == allowed_host
            && (allowed_port.is_none()
                || allowed_port == port
                || port.is_none() && matches!(allowed_port, Some(80 | 443)))
    })
}

async fn request_context(
    State(state): State<WebState>,
    mut request: Request,
    next: Next,
) -> Response<Body> {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let has_peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some();
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(
            || SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0),
            |ConnectInfo(peer)| *peer,
        );
    let metadata = request_metadata(peer, request.headers(), &state.trusted_proxies);
    for name in FORWARDED_HEADERS {
        request.headers_mut().remove(*name);
    }
    let request_headers = request.headers().clone();
    let Ok(metadata) = metadata else {
        return json_response(
            StatusCode::BAD_REQUEST,
            br#"{"error":"Invalid forwarded request metadata"}"#.to_vec(),
        );
    };
    if has_peer && !matches!(request.uri().path(), "/health" | "/ready") {
        let Ok(direct_host) = header_text(request.headers(), "host") else {
            return json_response(
                StatusCode::BAD_REQUEST,
                br#"{"error":"Invalid request host header"}"#.to_vec(),
            );
        };
        let effective_host = metadata.host.as_deref().or(direct_host);
        if effective_host
            .is_none_or(|host| !request_host_matches_allowed(host, &state.allowed_hosts))
        {
            return json_response(
                StatusCode::MISDIRECTED_REQUEST,
                br#"{"error":"Unrecognized request host"}"#.to_vec(),
            );
        }
    }
    if method == Method::OPTIONS
        && let Some(response) = cors_preflight_response(&path, &request_headers)
    {
        return response;
    }
    request.extensions_mut().insert(metadata);
    let response = next.run(request).await;
    finalize_external_cors_response(&path, &method, &request_headers, response)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    const fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RangeSelection {
    Full,
    Ranges(Vec<ByteRange>),
    Unsatisfiable,
}

#[must_use]
fn paperclip_media_access_allowed(
    status_allowed: bool,
    discarded: bool,
    can_manage_reports: bool,
) -> bool {
    status_allowed || (discarded && can_manage_reports)
}

fn request_header_is_nonempty(headers: &HeaderMap, name: &str) -> bool {
    headers
        .get(name)
        .is_some_and(|value| !value.as_bytes().iter().all(u8::is_ascii_whitespace))
}

fn paperclip_response_policy(
    attachment: PaperclipAttachment,
    headers: &HeaderMap,
) -> (&'static str, Option<&'static str>) {
    if !matches!(
        attachment,
        PaperclipAttachment::MediaFile | PaperclipAttachment::MediaThumbnail
    ) {
        return (PAPERCLIP_CACHE, None);
    }
    let has_viewer_credentials = headers.contains_key(AUTHORIZATION)
        || [COOKIE.as_str(), "signature"]
            .into_iter()
            .any(|name| request_header_is_nonempty(headers, name));
    (
        if has_viewer_credentials {
            PRIVATE_CACHE
        } else {
            PAPERCLIP_CACHE
        },
        Some(PAPERCLIP_STATUS_VARY),
    )
}

// Native GET/HEAD media has no bearer header. Resolve only this route's cookie
// through the existing OAuth user/scope checks, without touching the session.
async fn paperclip_viewer(
    state: &WebState,
    headers: &HeaderMap,
    attached: bool,
) -> Result<Option<i64>, Response<Body>> {
    if headers.contains_key(AUTHORIZATION) {
        // Retain historical attached bearer semantics. Only the new unattached
        // owner grant (and limited federation) requires a functional user.
        if attached && !state.instance_runtime.limited_federation {
            return optional_viewer(state, headers, READ_STATUSES).await;
        }
        return required_viewer(state, headers, READ_STATUSES)
            .await
            .map(Some);
    }
    if let Some(session_id) = request_cookie(headers, BROWSER_SESSION_COOKIE)
        && let Some(session) = state
            .repository
            .browser_session(session_id)
            .await
            .map_err(|_| internal_error())?
    {
        let mut session_headers = HeaderMap::new();
        session_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", session.access_token.as_str()))
                .map_err(|_| internal_error())?,
        );
        let owner = required_viewer_owner(state, &session_headers, READ_STATUSES).await?;
        if session.functional
            && owner.user_id() == session.user_id
            && owner.account_id() == session.account_id
        {
            return Ok(Some(owner.account_id()));
        }
        return Err(not_found());
    }
    if state.instance_runtime.limited_federation {
        return required_viewer(state, headers, READ_STATUSES)
            .await
            .map(Some);
    }
    Ok(None)
}

async fn paperclip_status_media_access(
    state: &WebState,
    headers: &HeaderMap,
    media_id: i64,
    attachment: PaperclipAttachment,
) -> Result<bool, Response<Body>> {
    let media = state
        .repository
        .media_attachment_access(media_id)
        .await
        .map_err(|_| internal_error())?;
    let attached = media
        .as_ref()
        .is_some_and(|media| media.status_id.is_some());
    let viewer_account_id = paperclip_viewer(state, headers, attached).await?;
    let Some(media) = media else {
        return Ok(false);
    };
    // Explicit non-ready remote originals and their generated styles must agree
    // with the proxy. NULL is historical readiness, not an unfinished upload.
    // Keep separate thumbnails and the local owner/attached policy unchanged.
    if attachment == PaperclipAttachment::MediaFile
        && !media.local
        && matches!(media.processing, Some(0 | 1 | 3))
    {
        return Ok(false);
    }
    let Some(status_id) = media.status_id else {
        return Ok(media.local
            && media.processing == Some(2)
            && viewer_account_id == Some(media.account_id));
    };
    let discarded = media.discarded;
    let status_allowed = state
        .repository
        .rest_authorized_status_ids(&[status_id], viewer_account_id)
        .await
        .map_err(|_| internal_error())?
        == [status_id];
    let report_manager = if !status_allowed && discarded {
        match viewer_account_id {
            Some(account_id) => state
                .repository
                .user_can_manage_reports(account_id)
                .await
                .map_err(|_| internal_error())?,
            None => false,
        }
    } else {
        false
    };
    Ok(paperclip_media_access_allowed(
        status_allowed,
        discarded,
        report_manager,
    ))
}

async fn paperclip_media(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if !matches!(method, Method::GET | Method::HEAD) {
        return not_found();
    }
    let Some(path) = uri
        .path()
        .strip_prefix(&state.media_route_path)
        .and_then(|path| path.strip_prefix('/'))
        .and_then(parse_paperclip_path)
    else {
        return not_found();
    };
    let (cache_control, vary) = paperclip_response_policy(path.attachment(), &headers);
    let mut response = async {
        if state
            .media_route_authority
            .as_deref()
            .is_some_and(|expected| {
                metadata
                    .host
                    .as_deref()
                    .or_else(|| headers.get(HOST).and_then(|value| value.to_str().ok()))
                    .is_none_or(|actual| !actual.eq_ignore_ascii_case(expected))
            })
        {
            return finalize_api_response(uri.path(), &headers, api_not_found());
        }
        if matches!(
            path.attachment(),
            PaperclipAttachment::MediaFile | PaperclipAttachment::MediaThumbnail
        ) {
            match paperclip_status_media_access(&state, &headers, path.id(), path.attachment())
                .await
            {
                Ok(true) => {}
                Ok(false) => return not_found(),
                Err(response) => return response,
            }
        }
        match state
            .repository
            .paperclip_metadata(path.attachment(), path.id())
            .await
        {
            Ok(Some(metadata)) if path.authorizes(&metadata) => {}
            Ok(_) => return not_found(),
            Err(_) => return internal_error(),
        }
        let root = state.media_root.clone();
        let relative_path = path.relative_path().to_owned();
        let opened = tokio::task::spawn_blocking(move || {
            let file = root.open_file(&relative_path)?;
            let metadata = file.metadata()?;
            Ok::<_, std::io::Error>((file, metadata))
        })
        .await;
        let (file, file_metadata) = match opened {
            Ok(Ok(opened)) => opened,
            Ok(Err(_)) => return not_found(),
            Err(_) => return internal_error(),
        };
        paperclip_file_response(
            &method,
            &headers,
            path.relative_path(),
            file,
            &file_metadata,
            cache_control,
            vary,
        )
    }
    .await;
    // Apply after every recognized-media outcome, not just opened files. A
    // denied composer request must not be reused after upload/attachment state changes.
    let media_error = vary == Some(PAPERCLIP_STATUS_VARY)
        && (response.status().is_client_error() || response.status().is_server_error());
    if cache_control == PRIVATE_CACHE || media_error {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static(PRIVATE_CACHE));
        response
            .headers_mut()
            .insert(VARY, HeaderValue::from_static(PAPERCLIP_STATUS_VARY));
    }
    response
}

fn paperclip_file_response(
    method: &Method,
    headers: &HeaderMap,
    path: &std::path::Path,
    file: File,
    file_metadata: &std::fs::Metadata,
    cache_control: &'static str,
    vary: Option<&'static str>,
) -> Response<Body> {
    let size = file_metadata.len();
    let modified = file_metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let last_modified = httpdate::fmt_http_date(modified);
    let content_type = paperclip_content_type(path.to_string_lossy().as_ref());
    let conditional = not_modified(headers, &last_modified, modified);
    if conditional == NotModified::Exact {
        let response = paperclip_response(
            StatusCode::NOT_MODIFIED,
            content_type,
            &last_modified,
            None,
            None,
            Body::empty(),
            cache_control,
            vary,
        );
        return response;
    }

    let range_header = headers.get(RANGE).and_then(|value| value.to_str().ok());
    match paperclip_ranges(range_header, size) {
        RangeSelection::Full => {
            if conditional == NotModified::Parsed && !headers.contains_key(IF_NONE_MATCH) {
                let mut response = paperclip_response(
                    StatusCode::NOT_MODIFIED,
                    content_type,
                    &last_modified,
                    None,
                    None,
                    Body::empty(),
                    cache_control,
                    vary,
                );
                response.headers_mut().insert(
                    LAST_MODIFIED,
                    HeaderValue::from_str(&last_modified).expect("an HTTP date is a valid header"),
                );
                return response;
            }
            let body = if method == Method::HEAD {
                Body::empty()
            } else {
                file_body(file, 0, size)
            };
            paperclip_response(
                StatusCode::OK,
                content_type,
                &last_modified,
                Some(size),
                None,
                body,
                cache_control,
                vary,
            )
        }
        RangeSelection::Ranges(ranges) if ranges.len() == 1 => {
            let range = ranges[0];
            let body = if method == Method::HEAD {
                Body::empty()
            } else {
                file_body(file, range.start, range.len())
            };
            paperclip_response(
                StatusCode::PARTIAL_CONTENT,
                content_type,
                &last_modified,
                Some(range.len()),
                Some(format!("bytes {}-{}/{}", range.start, range.end, size)),
                body,
                cache_control,
                vary,
            )
        }
        RangeSelection::Ranges(ranges) => {
            let content_length = multipart_content_length(content_type, size, &ranges);
            let body = if method == Method::HEAD {
                Body::empty()
            } else {
                multipart_body(file, content_type, size, &ranges)
            };
            paperclip_response(
                StatusCode::PARTIAL_CONTENT,
                content_type,
                &last_modified,
                Some(content_length),
                None,
                body,
                cache_control,
                vary,
            )
        }
        // Rack marks its 416 as `X-Cascade: pass`, so Rails replaces it with this 404.
        RangeSelection::Unsatisfiable => framework_not_found(),
    }
}

#[allow(clippy::too_many_arguments)]
fn paperclip_response(
    status: StatusCode,
    content_type: &'static str,
    last_modified: &str,
    content_length: Option<u64>,
    content_range: Option<String>,
    body: Body,
    cache_control: &'static str,
    vary: Option<&'static str>,
) -> Response<Body> {
    let mut response = Response::builder()
        .status(status)
        .header(CACHE_CONTROL, cache_control)
        .header("content-security-policy", PAPERCLIP_CSP)
        .header("x-content-type-options", "nosniff")
        .body(body)
        .expect("static Paperclip headers are valid");
    if let Some(vary) = vary {
        response
            .headers_mut()
            .insert(VARY, HeaderValue::from_static(vary));
    }
    if status != StatusCode::NOT_MODIFIED {
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        response.headers_mut().insert(
            LAST_MODIFIED,
            HeaderValue::from_str(last_modified).expect("an HTTP date is a valid header"),
        );
        if let Some(content_length) = content_length {
            response.headers_mut().insert(
                CONTENT_LENGTH,
                HeaderValue::from_str(&content_length.to_string())
                    .expect("a decimal length is a valid header"),
            );
        }
        if let Some(content_range) = content_range {
            response.headers_mut().insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&content_range).expect("a byte range is a valid header"),
            );
        }
    }
    response
}

fn file_body(file: File, start: u64, length: u64) -> Body {
    let stream = futures_util::stream::try_unfold(
        (file, Some(start), length),
        |(mut file, start, remaining)| async move {
            if remaining == 0 {
                return Ok(None);
            }
            tokio::task::spawn_blocking(move || {
                if let Some(start) = start {
                    file.seek(SeekFrom::Start(start))?;
                }
                let capacity = usize::try_from(remaining.min(64 * 1024)).unwrap_or(64 * 1024);
                let mut buffer = vec![0_u8; capacity];
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "Paperclip file ended before its reported size",
                    ));
                }
                buffer.truncate(count);
                Ok(Some((
                    Bytes::from(buffer),
                    (file, None, remaining - count as u64),
                )))
            })
            .await
            .map_err(std::io::Error::other)?
        },
    );
    Body::from_stream(stream)
}

fn multipart_body(file: File, content_type: &str, size: u64, ranges: &[ByteRange]) -> Body {
    let mut parts = VecDeque::new();
    for range in ranges {
        parts.push_back(MultipartPart::Bytes(Bytes::from(multipart_prefix(
            content_type,
            size,
            *range,
        ))));
        parts.push_back(MultipartPart::File {
            start: range.start,
            remaining: range.len(),
        });
    }
    parts.push_back(MultipartPart::Bytes(Bytes::from(format!(
        "\r\n--{MULTIPART_BOUNDARY}--\r\n"
    ))));
    let stream =
        futures_util::stream::try_unfold((file, parts), |(mut file, mut parts)| async move {
            let Some(part) = parts.pop_front() else {
                return Ok::<_, std::io::Error>(None);
            };
            match part {
                MultipartPart::Bytes(bytes) => Ok(Some((bytes, (file, parts)))),
                MultipartPart::File { start, remaining } => {
                    tokio::task::spawn_blocking(move || {
                        file.seek(SeekFrom::Start(start))?;
                        let capacity =
                            usize::try_from(remaining.min(64 * 1024)).unwrap_or(64 * 1024);
                        let mut buffer = vec![0_u8; capacity];
                        file.read_exact(&mut buffer)?;
                        if remaining > capacity as u64 {
                            parts.push_front(MultipartPart::File {
                                start: start + capacity as u64,
                                remaining: remaining - capacity as u64,
                            });
                        }
                        Ok(Some((Bytes::from(buffer), (file, parts))))
                    })
                    .await
                    .map_err(std::io::Error::other)?
                }
            }
        });
    Body::from_stream(stream)
}

enum MultipartPart {
    Bytes(Bytes),
    File { start: u64, remaining: u64 },
}

fn multipart_content_length(content_type: &str, size: u64, ranges: &[ByteRange]) -> u64 {
    ranges
        .iter()
        .map(|range| multipart_prefix(content_type, size, *range).len() as u64 + range.len())
        .sum::<u64>()
        + format!("\r\n--{MULTIPART_BOUNDARY}--\r\n").len() as u64
}

fn multipart_prefix(content_type: &str, size: u64, range: ByteRange) -> String {
    format!(
        "\r\n--{MULTIPART_BOUNDARY}\r\ncontent-type: {content_type}\r\ncontent-range: bytes {}-{}/{size}\r\n\r\n",
        range.start, range.end
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NotModified {
    No,
    Exact,
    Parsed,
}

fn not_modified(headers: &HeaderMap, formatted: &str, modified: SystemTime) -> NotModified {
    let Some(value) = headers
        .get(IF_MODIFIED_SINCE)
        .and_then(|value| value.to_str().ok())
    else {
        return NotModified::No;
    };
    if value == formatted {
        return NotModified::Exact;
    }
    if httpdate::parse_http_date(value).is_ok_and(|requested| {
        requested
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .zip(modified.duration_since(SystemTime::UNIX_EPOCH).ok())
            .is_some_and(|(requested, modified)| requested.as_secs() >= modified.as_secs())
    }) {
        NotModified::Parsed
    } else {
        NotModified::No
    }
}

fn paperclip_ranges(header: Option<&str>, size: u64) -> RangeSelection {
    let Some(value) = header.filter(|_| size > 0) else {
        return RangeSelection::Full;
    };
    let Some((_, value)) = value.split_once("bytes=") else {
        return RangeSelection::Full;
    };
    let value = value.split(';').next().unwrap_or_default();
    if value.bytes().filter(|byte| *byte == b',').count() >= 100 {
        return RangeSelection::Full;
    }
    let mut specifications = value.split(',').collect::<Vec<_>>();
    while specifications.last().is_some_and(|value| value.is_empty()) {
        specifications.pop();
    }
    let mut ranges = Vec::new();
    for value in specifications {
        let Some((start, end)) = value.trim().split_once('-') else {
            return RangeSelection::Full;
        };
        let range = if start.is_empty() {
            let suffix = RubyUnsigned::parse(end);
            if suffix.is_zero() {
                continue;
            }
            ByteRange {
                start: suffix
                    .as_u64()
                    .map_or(0, |suffix| size.saturating_sub(suffix)),
                end: size - 1,
            }
        } else {
            let start = RubyUnsigned::parse(start);
            let explicit_end = !end.is_empty();
            let end = if explicit_end {
                RubyUnsigned::parse(end)
            } else {
                RubyUnsigned::from_u64(size - 1)
            };
            if explicit_end && end < start {
                return RangeSelection::Full;
            }
            let Some(start) = start.as_u64().filter(|start| *start < size) else {
                continue;
            };
            ByteRange {
                start,
                end: end.as_u64().map_or(size - 1, |end| end.min(size - 1)),
            }
        };
        ranges.push(range);
    }
    if ranges.is_empty() || ranges.iter().map(|range| range.len()).sum::<u64>() > size {
        RangeSelection::Unsatisfiable
    } else {
        RangeSelection::Ranges(ranges)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RubyUnsigned(String);

impl Ord for RubyUnsigned {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for RubyUnsigned {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl RubyUnsigned {
    fn parse(value: &str) -> Self {
        let value = value
            .trim_start()
            .strip_prefix('+')
            .unwrap_or(value.trim_start());
        let digits = value
            .bytes()
            .take_while(u8::is_ascii_digit)
            .collect::<Vec<_>>();
        let digits = std::str::from_utf8(&digits).unwrap_or_default();
        let digits = digits.trim_start_matches('0');
        Self(if digits.is_empty() { "0" } else { digits }.to_owned())
    }

    fn from_u64(value: u64) -> Self {
        Self(value.to_string())
    }

    fn is_zero(&self) -> bool {
        self.0 == "0"
    }

    fn as_u64(&self) -> Option<u64> {
        self.0.parse().ok()
    }
}

fn paperclip_content_type(file_name: &str) -> &'static str {
    match file_name
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension)
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "avif" => "image/avif",
        "svg" | "svgz" => "image/svg+xml",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "apng" => "image/apng",
        "btif" => "image/prs.btif",
        "cgm" => "image/cgm",
        "cmx" => "image/x-cmx",
        "djv" | "djvu" => "image/vnd.djvu",
        "dwg" => "image/vnd.dwg",
        "dxf" => "image/vnd.dxf",
        "fbs" => "image/vnd.fastbidsheet",
        "flif" => "image/flif",
        "fpx" => "image/vnd.fpx",
        "fst" => "image/vnd.fst",
        "g3" => "image/g3fax",
        "heics" => "image/heic-sequence",
        "heifs" => "image/heif-sequence",
        "ief" => "image/ief",
        "jp2" => "image/jp2",
        "jpm" => "video/jpm",
        "mdi" => "image/vnd.ms-modi",
        "mj2" => "video/mj2",
        "mmr" => "image/vnd.fujixerox.edmics-mmr",
        "npx" => "image/vnd.net-fpx",
        "pbm" => "image/x-portable-bitmap",
        "pcx" => "image/x-pcx",
        "pgm" => "image/x-portable-graymap",
        "pic" => "image/x-pict",
        "pict" => "image/pict",
        "pnm" => "image/x-portable-anymap",
        "pntg" => "image/x-macpaint",
        "ppm" => "image/x-portable-pixmap",
        "psd" => "image/vnd.adobe.photoshop",
        "qtif" => "image/x-quicktime",
        "ras" => "image/x-cmu-raster",
        "rgb" => "image/x-rgb",
        "rlc" => "image/vnd.fujixerox.edmics-rlc",
        "wbmp" => "image/vnd.wap.wbmp",
        "xbm" => "image/x-xbitmap",
        "xif" => "image/vnd.xiff",
        "xpm" => "image/x-xpixmap",
        "xwd" => "image/x-xwindowdump",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "ogg" => "application/ogg",
        "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "wav" => "audio/x-wav",
        "m4a" => "audio/mp4a-latm",
        "3gp" => "video/3gpp",
        "wma" => "audio/x-ms-wma",
        "ico" => "image/vnd.microsoft.icon",
        _ => "text/plain",
    }
}

fn api_route(path: &str) -> Option<&'static ApiRouteContract> {
    let path = path.strip_suffix('/').unwrap_or(path);
    if reserved_static_route(path) {
        return API_ROUTE_INVENTORY.iter().find(|route| route.path == path);
    }
    API_ROUTE_INVENTORY
        .iter()
        .find(|route| route_path_matches(route.path, path))
}

fn api_route_for_method(method: &Method, path: &str) -> Option<&'static ApiRouteContract> {
    let method = match method.as_str() {
        "GET" => ApiMethod::Get,
        "POST" => ApiMethod::Post,
        "DELETE" => ApiMethod::Delete,
        "PATCH" => ApiMethod::Patch,
        "PUT" => ApiMethod::Put,
        _ => return None,
    };
    let path = path.strip_suffix('/').unwrap_or(path);
    if reserved_static_route(path) {
        return API_ROUTE_INVENTORY
            .iter()
            .find(|route| route.method == method && route.path == path);
    }
    API_ROUTE_INVENTORY
        .iter()
        .find(|route| route.method == method && route_path_matches(route.path, path))
}

fn preflight_api_route(path: &str) -> Option<&'static ApiRouteContract> {
    let path = path.strip_suffix('/').unwrap_or(path);
    if reserved_static_route(path) {
        return API_ROUTE_INVENTORY.iter().find(|route| route.path == path);
    }
    API_ROUTE_INVENTORY
        .iter()
        .find(|route| preflight_route_path_matches(route.path, path))
}

fn reserved_static_route(path: &str) -> bool {
    const RESERVED_SEGMENTS: &[&str] = &[
        "lookup",
        "relationships",
        "familiar_followers",
        "search",
        "update_credentials",
        "verify_credentials",
    ];
    let segments = path.split('/').collect::<Vec<_>>();
    segments.len() == 5 && RESERVED_SEGMENTS.contains(&segments[4])
}

fn route_path_matches(pattern: &str, path: &str) -> bool {
    let mut pattern = pattern.split('/');
    let mut path = path.split('/');
    loop {
        match (pattern.next(), path.next()) {
            (None, None) => return true,
            (Some(expected), Some(actual))
                if expected == actual
                    || (expected.starts_with('{')
                        && expected.ends_with('}')
                        && !actual.is_empty()) => {}
            _ => return false,
        }
    }
}

fn preflight_route_path_matches(pattern: &str, path: &str) -> bool {
    let mut pattern = pattern.split('/');
    let mut path = path.split('/');
    loop {
        match (pattern.next(), path.next()) {
            (None, None) => return true,
            (Some(expected), Some(actual)) if expected == actual => {}
            (Some("{id}"), Some(actual)) if !actual.is_empty() => {}
            (Some(expected), Some(actual))
                if expected.starts_with('{') && expected.ends_with('}') && !actual.is_empty() => {}
            _ => return false,
        }
    }
}

async fn api_protocol(
    State(state): State<WebState>,
    request: Request,
    next: Next,
) -> Response<Body> {
    let path = request.uri().path().to_owned();
    let mut headers = request.headers().clone();
    if request.method() == Method::OPTIONS
        && let Some(response) = cors_preflight_response(&path, &headers)
    {
        return response;
    }
    inject_query_parameter_bearer(&mut headers, request.uri().query());
    let route = api_route_for_method(request.method(), &path);
    if api_request_requires_preauthentication(route, &headers, &request) {
        let Some(scopes) = required_api_scopes(route) else {
            unreachable!("pre-authentication requires a required API route");
        };
        if let Err(response) = authenticate_required_api_route(&state, &headers, scopes).await {
            return finalize_api_response(&path, &headers, response);
        }
    }
    let body_limit = api_request_body_limit(&path, route, &headers, &request);
    let body_limit = if path.trim_end_matches('/') == "/settings/profile"
        && request_body_may_exceed_public_limit(&request)
    {
        match browser_session_allows_large_body(&state, &headers).await {
            Ok(true) => ACCOUNT_PROFILE_BODY_LIMIT_BYTES,
            Ok(false) => return browser_redirect_response("/auth/sign_in"),
            Err(response) => return response,
        }
    } else {
        body_limit
    };
    if content_length_exceeds_limit(&headers, body_limit) {
        return finalize_api_response(&path, &headers, upload_body_limit_response(&path));
    }
    let mut request = match bounded_request(request, body_limit).await {
        Ok(request) => request,
        Err(response) => {
            let response = if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
                upload_body_limit_response(&path)
            } else {
                response
            };
            return finalize_api_response(&path, &headers, response);
        }
    };
    if !valid_percent_encoded(&path) {
        return malformed_request_response(&headers);
    }
    if request
        .uri()
        .query()
        .is_some_and(|query| !valid_query(query))
    {
        return malformed_request_response(&headers);
    }
    if let Err(error) = merge_request_parameters(&mut request) {
        return match error {
            RequestParameterError::BadRequest => framework_bad_request(&headers),
            RequestParameterError::InternalServer => framework_internal_error(),
        };
    }
    inject_parameter_bearer(&mut request);
    headers = request.headers().clone();
    let response = next.run(request).await;
    finalize_api_response(&path, &headers, response)
}

fn required_api_scopes(route: Option<&ApiRouteContract>) -> Option<&'static [&'static str]> {
    match route.map(|route| route.authentication) {
        Some(ApiAuthentication::Required(scopes)) => Some(scopes),
        _ => None,
    }
}

fn api_request_requires_preauthentication(
    route: Option<&ApiRouteContract>,
    headers: &HeaderMap,
    request: &Request,
) -> bool {
    required_api_scopes(route).is_some()
        && has_valid_bearer_authorization(headers)
        && request_body_may_exceed_public_limit(request)
}

fn request_body_may_exceed_public_limit(request: &Request) -> bool {
    if !request_body_can_be_large(request) {
        return false;
    }
    request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_none_or(|length| length > PUBLIC_REQUEST_BODY_LIMIT_BYTES as u64)
}

fn request_body_can_be_large(request: &Request) -> bool {
    request.method() == Method::POST
        || request.method() == Method::PUT
        || request.method() == Method::PATCH
}

fn content_length_exceeds_limit(headers: &HeaderMap, limit: usize) -> bool {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > limit as u64)
}

fn upload_body_limit_response(path: &str) -> Response<Body> {
    if path.trim_end_matches('/') == "/api/v2/media" {
        media_upload_error(crate::paperclip::MediaAttachmentError::TooLarge)
    } else {
        error_response(StatusCode::PAYLOAD_TOO_LARGE, "Payload Too Large")
    }
}

fn api_request_body_limit(
    path: &str,
    route: Option<&ApiRouteContract>,
    headers: &HeaderMap,
    request: &Request,
) -> usize {
    if required_api_scopes(route).is_some()
        && has_valid_bearer_authorization(headers)
        && request_body_can_be_large(request)
    {
        match path.trim_end_matches('/') {
            "/api/v1/accounts/update_credentials"
            | "/api/v1/profile/avatar"
            | "/api/v1/profile/header" => ACCOUNT_PROFILE_BODY_LIMIT_BYTES,
            "/api/v2/media" => REST_BODY_LIMIT_BYTES + 64 * 1024,
            _ => REST_BODY_LIMIT_BYTES,
        }
    } else {
        PUBLIC_REQUEST_BODY_LIMIT_BYTES
    }
}

async fn browser_session_allows_large_body(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<bool, Response<Body>> {
    let Some(session_id) = request_cookie(headers, BROWSER_SESSION_COOKIE) else {
        return Ok(false);
    };
    state
        .repository
        .browser_session(session_id)
        .await
        .map(|session| session.is_some())
        .map_err(|_| internal_error())
}

async fn authenticate_required_api_route(
    state: &WebState,
    headers: &HeaderMap,
    scopes: &'static [&'static str],
) -> Result<(), Response<Body>> {
    match state
        .authenticator
        .authenticate(headers, RequiredScopes::new(scopes))
        .await
    {
        Ok(_) => Ok(()),
        Err(OAuthAuthenticationError::OAuth(error)) => {
            Err(error.into_http_response().map(Body::from))
        }
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

fn inject_query_parameter_bearer(headers: &mut HeaderMap, query: Option<&str>) {
    if !parameter_bearer_fallback_allowed(headers) {
        return;
    }
    let Some(query) = query.filter(|query| valid_query(query)) else {
        return;
    };
    let Ok(parameters) = RackParameters::parse(query) else {
        return;
    };
    let Some(token) = parameter_bearer(&parameters) else {
        return;
    };
    let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) else {
        return;
    };
    headers.insert(AUTHORIZATION, value);
}

fn inject_parameter_bearer(request: &mut Request) {
    if !parameter_bearer_fallback_allowed(request.headers()) {
        return;
    }
    let Some(parameters) = request.extensions().get::<RackParameters>() else {
        return;
    };
    let Some(token) = parameter_bearer(parameters) else {
        return;
    };
    let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) else {
        return;
    };
    request.headers_mut().insert(AUTHORIZATION, value);
}

fn parameter_bearer(parameters: &RackParameters) -> Option<&str> {
    ["access_token", "bearer_token"]
        .into_iter()
        .find_map(|name| match parameters.get(name) {
            Some(RackValue::Scalar(value)) if !value.trim().is_empty() => Some(value.as_str()),
            _ => None,
        })
}

fn has_valid_bearer_authorization(headers: &HeaderMap) -> bool {
    BearerToken::from_headers(headers).is_ok()
}

fn parameter_bearer_fallback_allowed(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let Some(_) = values.next() else {
        return true;
    };
    if values.next().is_some() {
        return false;
    }
    !has_valid_bearer_authorization(headers)
}

fn cors_external_resource(path: &str, method: &Method) -> bool {
    let is_get = method.as_str().eq_ignore_ascii_case("GET");
    let is_post = method.as_str().eq_ignore_ascii_case("POST");
    match path {
        "/oauth/token" | "/oauth/revoke" => is_post,
        "/oauth/userinfo" => is_get || is_post,
        _ => {
            is_get
                && (path.starts_with("/.well-known/")
                    || path.starts_with("/nodeinfo/")
                    || path
                        .strip_prefix("/@")
                        .is_some_and(|username| !username.is_empty() && !username.contains('/'))
                    || path
                        .strip_prefix("/users/")
                        .is_some_and(|username| !username.is_empty() && !username.contains('/')))
        }
    }
}

fn cors_preflight_response(path: &str, headers: &HeaderMap) -> Option<Response<Body>> {
    let requested_method = headers.get(ACCESS_CONTROL_REQUEST_METHOD)?;
    let requested_method = Method::from_bytes(requested_method.as_bytes()).ok()?;
    if headers.get(ORIGIN).is_none()
        || (preflight_api_route(path).is_none() && !cors_external_resource(path, &requested_method))
        || !cors_method(requested_method.as_str().as_bytes())
    {
        return None;
    }
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(ACCESS_CONTROL_ALLOW_METHODS, CORS_METHODS)
        .header(ACCESS_CONTROL_MAX_AGE, CORS_MAX_AGE)
        .header(ACCESS_CONTROL_EXPOSE_HEADERS, CORS_EXPOSE_HEADERS)
        .body(Body::empty())
        .expect("static CORS response headers are valid");
    if let Some(requested_headers) = headers.get(ACCESS_CONTROL_REQUEST_HEADERS) {
        response
            .headers_mut()
            .insert(ACCESS_CONTROL_ALLOW_HEADERS, requested_headers.clone());
    }
    Some(response)
}

fn finalize_external_cors_response(
    path: &str,
    method: &Method,
    request_headers: &HeaderMap,
    mut response: Response<Body>,
) -> Response<Body> {
    if request_headers.get(ORIGIN).is_some() && cors_external_resource(path, method) {
        response
            .headers_mut()
            .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        response.headers_mut().insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(CORS_EXPOSE_HEADERS),
        );
    }
    response
}

async fn bounded_request(request: Request, limit: usize) -> Result<Request, Response<Body>> {
    bounded_request_with_timeout(request, limit, REQUEST_BODY_READ_TIMEOUT).await
}

async fn bounded_request_with_timeout(
    request: Request,
    limit: usize,
    timeout: StdDuration,
) -> Result<Request, Response<Body>> {
    let (mut parts, body) = request.into_parts();
    let mut stream = body.into_data_stream();
    let result = tokio::time::timeout(timeout, async move {
        let mut size = 0_usize;
        let mut buffer = Vec::new();
        while let Some(chunk) = stream.try_next().await.map_err(|_| internal_error())? {
            size = size.saturating_add(chunk.len());
            if size > limit {
                return Err(error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Payload Too Large",
                ));
            }
            buffer.extend_from_slice(&chunk);
        }
        let body = Bytes::from(buffer);
        parts.extensions.insert(BufferedRequestBody(body.clone()));
        Ok(Request::from_parts(parts, Body::from(body)))
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => Err(error_response(
            StatusCode::REQUEST_TIMEOUT,
            "Request Timeout",
        )),
    }
}

#[derive(Clone)]
struct BufferedRequestBody(Bytes);

#[derive(Clone, Copy, Debug)]
enum RequestParameterError {
    BadRequest,
    InternalServer,
}

fn merge_request_parameters(request: &mut Request) -> Result<(), RequestParameterError> {
    let query_parameters = RackParameters::parse(request.uri().query().unwrap_or_default())
        .map_err(RequestParameterError::from)?;
    let body = request.extensions().get::<BufferedRequestBody>();
    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim);
    let body_parameters = if body.is_none_or(|body| body.0.is_empty()) {
        RackParameters::default()
    } else {
        let Some(body) = body else {
            unreachable!("nonempty request body is present")
        };
        match content_type {
            Some(value) if content_type_is(value, "application/x-www-form-urlencoded") => {
                let body =
                    std::str::from_utf8(&body.0).map_err(|_| RequestParameterError::BadRequest)?;
                if !valid_query(body) {
                    return Err(RequestParameterError::BadRequest);
                }
                RackParameters::parse(body).map_err(RequestParameterError::from)?
            }
            Some(value) if content_type_is(value, "multipart/form-data") => {
                parse_multipart_parameters(&body.0, value)?
            }
            Some(value)
                if json_content_type(value.split(';').next().unwrap_or_default().trim()) =>
            {
                let value = serde_json::from_slice(&body.0)
                    .map_err(|_| RequestParameterError::BadRequest)?;
                let root = RackValue::from_json(&value);
                if root.json_depth() > 100 {
                    return Err(RequestParameterError::BadRequest);
                }
                RackParameters::from_json(&value)
            }
            _ => RackParameters::default(),
        }
    };
    let merged = body_parameters.merge(query_parameters);
    let query = merged.to_query();
    let path_and_query = if query.is_empty() {
        request.uri().path().to_owned()
    } else {
        format!("{}?{query}", request.uri().path())
    };
    *request.uri_mut() = path_and_query
        .parse()
        .map_err(|_| RequestParameterError::BadRequest)?;
    request.extensions_mut().insert(merged);
    Ok(())
}

fn json_content_type(value: &str) -> bool {
    [
        "application/json",
        "text/x-json",
        "application/jsonrequest",
        "application/jrd+json",
        "application/activity+json",
        "application/ld+json",
        "application/problem+json",
    ]
    .iter()
    .any(|mime| value.eq_ignore_ascii_case(mime))
}

fn content_type_is(value: &str, expected: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
}

fn parse_multipart_parameters(
    body: &[u8],
    content_type: &str,
) -> Result<RackParameters, RequestParameterError> {
    let boundary = multipart_boundary(content_type).ok_or(RequestParameterError::BadRequest)?;
    let marker = format!("--{boundary}");
    let marker = marker.as_bytes();
    if !body.starts_with(marker) {
        return Err(RequestParameterError::BadRequest);
    }
    let separator = format!("\r\n--{boundary}");
    let mut offset = marker.len();
    let mut parameters = RackParameters::default();
    let mut count = 0_usize;
    loop {
        if body.get(offset..offset + 2) == Some(b"--") {
            break;
        }
        if body.get(offset..offset + 2) != Some(b"\r\n") {
            return Err(RequestParameterError::BadRequest);
        }
        offset += 2;
        let Some(header_end) = find_bytes(&body[offset..], b"\r\n\r\n") else {
            return Err(RequestParameterError::BadRequest);
        };
        let headers = parse_multipart_headers(&body[offset..offset + header_end])?;
        offset += header_end + 4;
        let relative = find_bytes(&body[offset..], separator.as_bytes())
            .ok_or(RequestParameterError::BadRequest)?;
        let next_marker = offset + relative;
        let value = &body[offset..next_marker];
        let name = headers
            .content_disposition
            .name
            .as_deref()
            .ok_or(RequestParameterError::BadRequest)?;
        let Some((root, segments)) = rack_key(name).map_err(RequestParameterError::from)? else {
            return Err(RequestParameterError::BadRequest);
        };
        let value = if let Some(file_name) = headers.content_disposition.file_name {
            if file_name.is_empty() {
                RackValue::Null
            } else {
                if file_name.contains(['\0', '\r', '\n']) {
                    return Err(RequestParameterError::BadRequest);
                }
                if matches!(name, "avatar" | "header") && value.len() >= 8 * 1024 * 1024 {
                    return Err(RequestParameterError::BadRequest);
                }
                RackValue::Upload(UploadedFile {
                    file_name,
                    content_type: headers
                        .content_type
                        .unwrap_or_else(|| "application/octet-stream".to_owned()),
                    bytes: value.to_vec(),
                })
            }
        } else {
            RackValue::Scalar(
                String::from_utf8(value.to_vec()).map_err(|_| RequestParameterError::BadRequest)?,
            )
        };
        count += 1;
        if count > RACK_PARAMETER_LIMIT {
            return Err(RequestParameterError::InternalServer);
        }
        parameters
            .insert(root, &segments, value)
            .map_err(RequestParameterError::from)?;
        offset = next_marker + 2 + marker.len();
    }
    if body.get(offset + 2..) != Some(b"\r\n") {
        return Err(RequestParameterError::BadRequest);
    }
    Ok(parameters)
}

#[derive(Clone, Debug)]
struct UploadedFile {
    file_name: String,
    content_type: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct MultipartHeaders {
    content_disposition: MultipartContentDisposition,
    content_type: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct MultipartContentDisposition {
    name: Option<String>,
    file_name: Option<String>,
}

fn multipart_boundary(content_type: &str) -> Option<String> {
    if !content_type_is(content_type, "multipart/form-data") {
        return None;
    }
    content_type
        .split(';')
        .skip(1)
        .filter_map(|parameter| parameter.trim().split_once('='))
        .find_map(|(key, value)| {
            if !key.trim().eq_ignore_ascii_case("boundary") {
                return None;
            }
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            (!value.is_empty()
                && value.len() <= 70
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() && byte != b'"'))
            .then(|| value.to_owned())
        })
}

fn parse_multipart_headers(value: &[u8]) -> Result<MultipartHeaders, RequestParameterError> {
    let mut content_disposition = None;
    let mut content_type = None;
    for line in value.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            return Err(RequestParameterError::BadRequest);
        }
        let Some(separator) = line.iter().position(|byte| *byte == b':') else {
            return Err(RequestParameterError::BadRequest);
        };
        let (name, value) = line.split_at(separator);
        let value = &value[1..];
        let name = std::str::from_utf8(name)
            .map_err(|_| RequestParameterError::BadRequest)?
            .trim();
        let value = std::str::from_utf8(value)
            .map_err(|_| RequestParameterError::BadRequest)?
            .trim();
        if name.eq_ignore_ascii_case("content-disposition") {
            content_disposition = Some(parse_content_disposition(value)?);
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.to_owned());
        }
    }
    Ok(MultipartHeaders {
        content_disposition: content_disposition.ok_or(RequestParameterError::BadRequest)?,
        content_type,
    })
}

fn parse_content_disposition(
    value: &str,
) -> Result<MultipartContentDisposition, RequestParameterError> {
    let mut disposition = value.split(';');
    if !disposition
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("form-data"))
    {
        return Err(RequestParameterError::BadRequest);
    }
    let mut result = MultipartContentDisposition::default();
    for parameter in disposition {
        let Some((key, value)) = parameter.trim().split_once('=') else {
            return Err(RequestParameterError::BadRequest);
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .ok_or(RequestParameterError::BadRequest)?;
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => result.name = Some(value.to_owned()),
            "filename" => result.file_name = Some(value.to_owned()),
            _ => {}
        }
    }
    Ok(result)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Clone, Debug)]
enum RackValue {
    Null,
    Scalar(String),
    Number(serde_json::Number),
    Boolean(bool),
    Array(Vec<RackValue>),
    Object(BTreeMap<String, RackValue>),
    Upload(UploadedFile),
}

#[derive(Clone, Debug, Default)]
struct RackParameters(BTreeMap<String, RackValue>);

#[derive(Clone, Debug)]
enum RackKeySegment {
    Field(String),
    TrailingPush(String),
    Push,
}

#[derive(Clone, Copy, Debug)]
enum RackParseError {
    Conflict,
    Limit,
}

impl From<RackParseError> for RequestParameterError {
    fn from(error: RackParseError) -> Self {
        match error {
            RackParseError::Conflict => Self::BadRequest,
            RackParseError::Limit => Self::InternalServer,
        }
    }
}

impl RackParameters {
    fn parse(encoded: &str) -> Result<Self, RackParseError> {
        if encoded.len() > RACK_BYTES_LIMIT {
            return Err(RackParseError::Limit);
        }
        let mut parameters = Self::default();
        for (index, raw) in encoded.split('&').enumerate() {
            if index == RACK_PARAMETER_LIMIT {
                return Err(RackParseError::Limit);
            }
            let Some((name, value)) = url::form_urlencoded::parse(raw.as_bytes()).next() else {
                continue;
            };
            let Some((root, segments)) = rack_key(&name)? else {
                continue;
            };
            let value = if raw.contains('=') {
                RackValue::Scalar(value.into_owned())
            } else {
                RackValue::Null
            };
            parameters.insert(root, &segments, value)?;
        }
        Ok(parameters)
    }

    fn from_json(value: &serde_json::Value) -> Self {
        let serde_json::Value::Object(values) = value else {
            return Self(BTreeMap::from([(
                "_json".to_owned(),
                RackValue::from_json(value),
            )]));
        };
        Self(
            values
                .iter()
                .map(|(key, value)| (key.clone(), RackValue::from_json(value)))
                .collect(),
        )
    }

    fn insert(
        &mut self,
        root: String,
        segments: &[RackKeySegment],
        value: RackValue,
    ) -> Result<(), RackParseError> {
        if segments.is_empty() {
            self.0.insert(root, value);
            return Ok(());
        }
        let expected = match segments[0] {
            RackKeySegment::Field(_) => RackValue::Object(BTreeMap::new()),
            RackKeySegment::Push | RackKeySegment::TrailingPush(_) => RackValue::Array(Vec::new()),
        };
        insert_rack_value(self.0.entry(root).or_insert(expected), segments, value)
    }

    fn merge(mut self, query: Self) -> Self {
        self.0.extend(query.0);
        self
    }

    fn get(&self, name: &str) -> Option<&RackValue> {
        self.0.get(name)
    }

    fn to_query(&self) -> String {
        fn append(pairs: &mut Vec<(String, String)>, name: String, value: &RackValue) {
            match value {
                RackValue::Null | RackValue::Upload(_) => {}
                RackValue::Scalar(value) => pairs.push((name, value.clone())),
                RackValue::Number(value) => pairs.push((name, ruby_json_number(value))),
                RackValue::Boolean(value) => pairs.push((name, value.to_string())),
                RackValue::Array(values) => {
                    for value in values {
                        append(pairs, format!("{name}[]"), value);
                    }
                }
                RackValue::Object(values) => {
                    for (key, value) in values {
                        append(pairs, format!("{name}[{key}]"), value);
                    }
                }
            }
        }

        let mut pairs = Vec::new();
        for (name, value) in &self.0 {
            append(&mut pairs, name.clone(), value);
        }
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.extend_pairs(pairs);
        serializer.finish()
    }
}

impl RackValue {
    fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::String(value) => Self::Scalar(value.clone()),
            serde_json::Value::Number(value) => Self::Number(value.clone()),
            serde_json::Value::Bool(value) => Self::Boolean(*value),
            serde_json::Value::Array(values) => {
                Self::Array(values.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(values) => Self::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    fn json_depth(&self) -> usize {
        match self {
            Self::Array(values) => 1 + values.iter().map(Self::json_depth).max().unwrap_or(0),
            Self::Object(values) => 1 + values.values().map(Self::json_depth).max().unwrap_or(0),
            Self::Null | Self::Scalar(_) | Self::Number(_) | Self::Boolean(_) | Self::Upload(_) => {
                0
            }
        }
    }
}

fn rack_key(name: &str) -> Result<Option<(String, Vec<RackKeySegment>)>, RackParseError> {
    let root_end = name
        .get(1..)
        .and_then(|suffix| suffix.find('[').map(|index| index + 1))
        .unwrap_or(name.len());
    let root = &name[..root_end];
    if root.is_empty() {
        return Ok(None);
    }
    let mut segments = Vec::new();
    let mut suffix = &name[root_end..];
    while !suffix.is_empty() {
        let Some(rest) = suffix.strip_prefix('[') else {
            break;
        };
        let end = rest.find(']').unwrap_or(rest.len());
        let field = &rest[..end];
        let remaining = if end == rest.len() {
            ""
        } else {
            &rest[end + 1..]
        };
        segments.push(if field.is_empty() {
            if !remaining.is_empty() && matches!(segments.last(), Some(RackKeySegment::Push)) {
                RackKeySegment::Field("[]".to_owned())
            } else {
                RackKeySegment::Push
            }
        } else {
            RackKeySegment::Field(field.to_owned())
        });
        suffix = remaining;
        if segments.len() >= RACK_DEPTH_LIMIT {
            return Err(RackParseError::Limit);
        }
    }
    if !suffix.is_empty() {
        match segments.last() {
            Some(RackKeySegment::Field(_)) => {
                segments.push(RackKeySegment::Field(suffix.to_owned()));
            }
            Some(RackKeySegment::Push) => {
                segments.pop();
                segments.push(RackKeySegment::TrailingPush(suffix.to_owned()));
            }
            Some(RackKeySegment::TrailingPush(_)) | None => {}
        }
    }
    if segments.len() >= RACK_DEPTH_LIMIT {
        return Err(RackParseError::Limit);
    }
    Ok(Some((root.to_owned(), segments)))
}

fn insert_rack_value(
    target: &mut RackValue,
    segments: &[RackKeySegment],
    value: RackValue,
) -> Result<(), RackParseError> {
    let Some((segment, rest)) = segments.split_first() else {
        *target = value;
        return Ok(());
    };
    match (target, segment) {
        (RackValue::Object(values), RackKeySegment::Field(field)) => {
            if rest.is_empty() {
                values.insert(field.clone(), value);
                return Ok(());
            }
            let expected = match rest[0] {
                RackKeySegment::Field(_) => RackValue::Object(BTreeMap::new()),
                RackKeySegment::Push | RackKeySegment::TrailingPush(_) => {
                    RackValue::Array(Vec::new())
                }
            };
            insert_rack_value(values.entry(field.clone()).or_insert(expected), rest, value)
        }
        (RackValue::Array(values), RackKeySegment::TrailingPush(field)) => {
            let path = [RackKeySegment::Field(field.clone())];
            if let Some(RackValue::Object(last)) = values.last()
                && !matches!(rack_path_state(last, &path), RackPathState::Existing)
            {
                let last = values.last_mut().expect("last array object exists");
                return insert_rack_value(last, &path, value);
            }
            let mut nested = RackValue::Object(BTreeMap::new());
            insert_rack_value(&mut nested, &path, value)?;
            values.push(nested);
            Ok(())
        }
        (RackValue::Array(values), RackKeySegment::Push) => {
            if rest.is_empty() {
                values.push(value);
                return Ok(());
            }
            if matches!(rest.first(), Some(RackKeySegment::Field(_)))
                && let Some(RackValue::Object(last)) = values.last()
                && !matches!(rack_path_state(last, rest), RackPathState::Existing)
            {
                let last = values.last_mut().expect("last array object exists");
                return insert_rack_value(last, rest, value);
            }
            if matches!(rest, [RackKeySegment::Push])
                && matches!(values.last(), Some(RackValue::Object(_)))
            {
                return Ok(());
            }
            if matches!(rest.first(), Some(RackKeySegment::Push))
                && matches!(values.last(), Some(RackValue::Array(_)))
            {
                let last = values.last_mut().expect("last nested array exists");
                return insert_rack_value(last, rest, value);
            }
            let mut nested = match rest[0] {
                RackKeySegment::Field(_) => RackValue::Object(BTreeMap::new()),
                RackKeySegment::Push | RackKeySegment::TrailingPush(_) => {
                    RackValue::Array(Vec::new())
                }
            };
            insert_rack_value(&mut nested, rest, value)?;
            values.push(nested);
            Ok(())
        }
        _ => Err(RackParseError::Conflict),
    }
}

#[derive(Clone, Copy)]
enum RackPathState {
    Missing,
    Existing,
    Conflict,
}

fn rack_path_state(
    values: &BTreeMap<String, RackValue>,
    segments: &[RackKeySegment],
) -> RackPathState {
    let Some((segment, rest)) = segments.split_first() else {
        return RackPathState::Existing;
    };
    let RackKeySegment::Field(field) = segment else {
        return RackPathState::Conflict;
    };
    let Some(value) = values.get(field) else {
        return RackPathState::Missing;
    };
    let Some((next, _)) = rest.split_first() else {
        return RackPathState::Existing;
    };
    match (value, next) {
        (RackValue::Object(values), RackKeySegment::Field(_)) => rack_path_state(values, rest),
        (RackValue::Array(_), RackKeySegment::Push | RackKeySegment::TrailingPush(_)) => {
            RackPathState::Missing
        }
        _ => RackPathState::Conflict,
    }
}

fn cors_method(method: &[u8]) -> bool {
    ["POST", "PUT", "DELETE", "GET", "PATCH", "OPTIONS"]
        .iter()
        .any(|allowed| method.eq_ignore_ascii_case(allowed.as_bytes()))
}

fn valid_query(query: &str) -> bool {
    valid_percent_encoded(query)
}

fn valid_percent_encoded(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    percent_decode_str(value).decode_utf8().is_ok()
}

fn malformed_request_response(request_headers: &HeaderMap) -> Response<Body> {
    let mut response = Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .header(CONTENT_TYPE, "application/json; charset=UTF-8")
        .header(VARY, "Origin")
        .body(Body::from(r#"{"status":400,"error":"Bad Request"}"#))
        .expect("static malformed-query response is valid");
    if request_headers.contains_key(ORIGIN) {
        response
            .headers_mut()
            .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(CORS_METHODS),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(CORS_EXPOSE_HEADERS),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static(CORS_MAX_AGE),
        );
    }
    response
}

fn framework_bad_request(request_headers: &HeaderMap) -> Response<Body> {
    let mut response = malformed_request_response(request_headers);
    response
        .headers_mut()
        .insert(FRAMEWORK_ERROR_HEADER, HeaderValue::from_static("1"));
    response
}

fn finalize_api_response(
    path: &str,
    request_headers: &HeaderMap,
    mut response: Response<Body>,
) -> Response<Body> {
    if response
        .headers_mut()
        .remove(FRAMEWORK_ERROR_HEADER)
        .is_some()
    {
        if request_headers.contains_key(ORIGIN) {
            response
                .headers_mut()
                .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        }
        response
            .headers_mut()
            .insert(VARY, HeaderValue::from_static("Origin"));
        return response;
    }
    let Some(route) = api_route(path) else {
        return response;
    };
    let authenticated = request_headers
        .get(AUTHORIZATION)
        .is_some_and(|value| !value.as_bytes().iter().all(u8::is_ascii_whitespace));
    let success = response.status().is_success();
    let cache_control = match (success, route.cache, authenticated) {
        (true, ApiCachePolicy::Public, _) => PUBLIC_CACHE,
        (true, ApiCachePolicy::Anonymous, false) => ANONYMOUS_CACHE,
        _ => PRIVATE_CACHE,
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    let cors = request_headers.contains_key(ORIGIN);
    let public_vary = matches!(route.cache, ApiCachePolicy::Public);
    let vary = if public_vary {
        Some("Accept, Origin")
    } else {
        Some("Authorization, Origin")
    };
    if let Some(vary) = vary {
        response
            .headers_mut()
            .insert(VARY, HeaderValue::from_static(vary));
    } else {
        response.headers_mut().remove(VARY);
    }
    if cors {
        response
            .headers_mut()
            .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        response.headers_mut().insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(CORS_EXPOSE_HEADERS),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(CORS_METHODS),
        );
        response.headers_mut().insert(
            ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static(CORS_MAX_AGE),
        );
    }
    response
}

/// Serves the currently implemented Rustodon HTTP surface.
///
/// # Errors
///
/// Returns an I/O error when binding or serving the listener fails.
pub async fn serve<F>(address: SocketAddr, state: WebState, shutdown: F) -> std::io::Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}

async fn health() -> Response<Body> {
    json_response(StatusCode::OK, br#"{"status":"ok"}"#.to_vec())
}

async fn readiness(State(state): State<WebState>) -> Response<Body> {
    if state.repository.ready().await {
        json_response(StatusCode::OK, br#"{"status":"ready"}"#.to_vec())
    } else {
        json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            br#"{"status":"not_ready"}"#.to_vec(),
        )
    }
}

async fn instance_v1(State(state): State<WebState>) -> Response<Body> {
    instance_response(state, true).await
}

async fn instance_v2(State(state): State<WebState>) -> Response<Body> {
    instance_response(state, false).await
}

async fn instance_response(state: WebState, v1: bool) -> Response<Body> {
    let Ok(instance) = state.instance().await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let body = if v1 {
        serializer
            .instance_v1(&instance)
            .ok()
            .and_then(|value| serde_json::to_vec(&value).ok())
    } else {
        serializer
            .instance_v2(&instance)
            .ok()
            .and_then(|value| serde_json::to_vec(&value).ok())
    };
    body.map_or_else(internal_error, |body| json_response(StatusCode::OK, body))
}

async fn instance_rules(State(state): State<WebState>) -> Response<Body> {
    let Ok(instance) = state.static_instance().await else {
        return internal_error();
    };
    match serde_json::to_vec(&RestSerializer::rules(&instance)) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn translation_languages() -> Response<Body> {
    json_response(StatusCode::OK, b"{}".to_vec())
}

async fn custom_emojis(State(state): State<WebState>) -> Response<Body> {
    let Ok(emojis) = state.loader(None).custom_emojis().await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = emojis
        .iter()
        .map(|emoji| serializer.custom_emoji(emoji))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn accounts_index(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Ok(ids) = batch_account_ids(&rack) else {
        return framework_internal_error();
    };
    if ids.len() > 40 {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "Validation failed");
    }
    if ids.is_empty() {
        return json_response(StatusCode::OK, b"[]".to_vec());
    }
    let Ok(ids) = state.repository.rest_batch_account_ids(&ids).await else {
        return internal_error();
    };
    let Ok(accounts) = state.loader(viewer).accounts(&ids).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let Ok(values) = accounts
        .iter()
        .map(|account| serializer.account(account))
        .collect::<Result<Vec<_>, _>>()
    else {
        return internal_error();
    };
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn account_show(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    match state.repository.rest_account_showable(account_id).await {
        Ok(true) => {}
        Ok(false) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    account_response(&state, state.loader(viewer).account(account_id).await)
}

async fn collection_show(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_COLLECTIONS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, collection_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    match state
        .repository
        .rest_collection_showable(collection_id, viewer)
        .await
    {
        Ok(Some(true)) => {}
        Ok(Some(false)) => {
            return error_response(StatusCode::FORBIDDEN, "This action is not allowed");
        }
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    let collection = match state.loader(viewer).collection(collection_id).await {
        Ok(Some(collection)) => collection,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .collection_with_accounts(&collection)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn account_lookup(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let handle = match rack.get("acct") {
        None | Some(RackValue::Null) => None,
        Some(RackValue::Scalar(value)) => Some(value.clone()),
        Some(
            RackValue::Number(_)
            | RackValue::Boolean(_)
            | RackValue::Array(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => return framework_internal_error(),
    };
    let account = match handle {
        Some(handle) => {
            if let Some((_, domain)) =
                remote_account_search_handle(Some(&handle), &state.local_domain, 0)
            {
                let Ok(domain_allowed) = state
                    .repository
                    .remote_domain_allowed(&domain, state.instance_runtime.limited_federation)
                    .await
                else {
                    return internal_error();
                };
                if !domain_allowed {
                    return record_not_found();
                }
            }
            state.loader(viewer).lookup_account(&handle).await
        }
        None => Ok(None),
    };
    account_response(&state, account)
}

fn remote_account_search_handle(
    query: Option<&str>,
    local_domain: &str,
    offset: i64,
) -> Option<(String, String)> {
    if offset != 0 {
        return None;
    }
    let query = query?.trim();
    let query = query.strip_prefix('@').unwrap_or(query);
    let mut parts = query.split('@');
    let username = parts.next()?.trim();
    let domain = parts.next()?.trim();
    if parts.next().is_some() || !valid_remote_username(username) || domain.is_empty() {
        return None;
    }
    let domain = canonical_remote_domain(domain).ok()?;
    if domain.eq_ignore_ascii_case(local_domain) {
        return None;
    }
    Some((username.to_owned(), domain))
}

#[allow(clippy::too_many_lines)]
async fn account_search(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let query = match rack.get("q") {
        None | Some(RackValue::Null) => None,
        Some(RackValue::Scalar(value)) => Some(value.as_str()),
        Some(
            RackValue::Number(_)
            | RackValue::Boolean(_)
            | RackValue::Array(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => return framework_internal_error(),
    };
    let offset = match rack.get("offset") {
        None | Some(RackValue::Null) => 0,
        Some(RackValue::Scalar(value)) => ruby_integer(value),
        Some(RackValue::Number(value)) => json_number_integer(value).unwrap_or(0),
        Some(
            RackValue::Boolean(_)
            | RackValue::Array(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => {
            return framework_internal_error();
        }
    };
    if offset < 0 {
        return framework_internal_error();
    }
    let Ok(limit) = limit_parameter(&rack, 40, 80) else {
        return framework_internal_error();
    };
    match search_accounts(
        &state,
        &metadata,
        Some(owner),
        AccountSearchOptions {
            query,
            resolve: boolean_parameter(&rack, "resolve"),
            following: boolean_parameter(&rack, "following"),
            limit,
            offset,
        },
    )
    .await
    {
        Ok(accounts) => match serde_json::to_vec(&accounts) {
            Ok(body) => json_response(StatusCode::OK, body),
            Err(_) => internal_error(),
        },
        Err(response) => response,
    }
}

struct AccountSearchOptions<'a> {
    query: Option<&'a str>,
    resolve: bool,
    following: bool,
    limit: i64,
    offset: i64,
}

// Endpoint authentication and parameter contracts stay in their handlers. Sharing the
// resolver here keeps the domain policy, limits, signing and persistence identical.
#[allow(clippy::too_many_lines)]
async fn search_accounts(
    state: &WebState,
    metadata: &RequestMetadata,
    owner: Option<i64>,
    options: AccountSearchOptions<'_>,
) -> Result<Vec<RestAccount>, Response<Body>> {
    let AccountSearchOptions {
        query,
        resolve,
        following,
        limit,
        offset,
    } = options;
    if limit < 1 || (following && owner.is_none()) {
        return Ok(Vec::new());
    }
    // Optional-scope and application-only requests may read cached accounts, never resolve.
    let resolve = resolve && owner.is_some();
    let remote_handle = resolve
        .then(|| remote_account_search_handle(query, &state.local_domain, offset))
        .flatten();
    let accounts = match if let Some((username, domain)) = remote_handle.as_ref()
        && let Some(writer) = state.write_repository.as_ref()
    {
        let Ok(domain_allowed) = state
            .repository
            .remote_domain_allowed(domain, state.instance_runtime.limited_federation)
            .await
        else {
            return Err(internal_error());
        };
        if !domain_allowed {
            return Err(json_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"error":"Remote account resolution is unavailable"}"#.to_vec(),
            ));
        }
        let Ok(last_webfingered_at) = state
            .repository
            .rest_account_last_webfingered_at(username, domain)
            .await
        else {
            return Err(internal_error());
        };
        let fresh_after = Utc::now().naive_utc() - ChronoDuration::days(1);
        if last_webfingered_at.is_some_and(|value| value >= fresh_after) {
            state
                .loader(owner)
                .account_search(query, false, following, limit, offset)
                .await
        } else {
            if let Err(limited) = state
                .remote_account_resolution_limiter
                .check_shared(
                    state.shared_rate_limiter.as_ref(),
                    metadata.client_ip,
                    username,
                    domain,
                )
                .await
            {
                return Err(rate_limited_response(limited));
            }
            let Ok(instance) = state.repository.account(-99).await else {
                return Err(internal_error());
            };
            let Some(instance) = instance else {
                return Err(internal_error());
            };
            let Some(private_key) = instance.private_key.as_ref().filter(|key| key.is_present())
            else {
                return Err(json_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    br#"{"error":"Remote account resolution is unavailable"}"#.to_vec(),
                ));
            };
            let key_id = format!(
                "{}#main-key",
                activitypub::actor_url(&state.origin, &instance)
            );
            let signer = HttpSignatureSigner {
                key_id: &key_id,
                private_key_pem: private_key.as_str(),
            };
            let actor = match state
                .remote_account_resolver
                .resolve_with_signer(username, domain, Some(&signer))
                .await
            {
                Ok(actor) => actor,
                Err(RemoteFetchError::DomainBudgetExceeded) => {
                    return Err(json_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        br#"{"error":"Remote account resolution is temporarily unavailable"}"#
                            .to_vec(),
                    ));
                }
                Err(_) => {
                    return Err(json_response(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        br#"{"error":"Remote account resolution is unavailable"}"#.to_vec(),
                    ));
                }
            };
            let actor_username = actor.username.clone();
            if writer
                .upsert_remote_actor(
                    &actor_username,
                    domain,
                    state.instance_runtime.limited_federation,
                    &actor,
                )
                .await
                .is_err()
            {
                return Err(internal_error());
            }
            state
                .loader(owner)
                .account_search(query, false, following, limit, offset)
                .await
        }
    } else {
        state
            .loader(owner)
            .account_search(query, resolve, following, limit, offset)
            .await
    } {
        Ok(accounts) => accounts,
        Err(AccountSearchError::RemoteResolutionUnsupported) => {
            return Err(json_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"error":"Remote account resolution is unavailable"}"#.to_vec(),
            ));
        }
        Err(AccountSearchError::Database(_)) => return Err(internal_error()),
    };
    let serializer = state.serializer();
    accounts
        .iter()
        .map(|account| serializer.account(account))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| internal_error())
}

async fn announcements(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, NO_SCOPE).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(announcements) = state.loader(Some(owner)).announcements(owner).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    match announcements
        .iter()
        .map(|announcement| serializer.announcement(announcement))
        .collect::<Result<Vec<_>, _>>()
        .ok()
        .and_then(|announcements| serde_json::to_vec(&announcements).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

// SearchService's URL branch is exclusive and independent of text filters.
fn status_search_url<'a>(
    query: &'a str,
    resolve: bool,
    kind: Option<&str>,
    limit: i64,
    offset: i64,
) -> Option<&'a str> {
    let kind = kind.filter(|value| !value.trim().is_empty());
    let query = query.trim();
    if !resolve || limit == 0 || kind.is_some_and(|kind| kind != "statuses" || offset > 0) {
        return None;
    }
    let url = Url::parse(query).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(query)
}

async fn search_status_url(
    state: &WebState,
    viewer: i64,
    url: &str,
) -> Result<Vec<crate::mastodon::rest::RestStatus>, ()> {
    use crate::mastodon::KnownSearchStatus;
    let mut target = state
        .repository
        .known_search_status_id(url, state.origin.as_str(), viewer)
        .await
        .map_err(|_| ())?;
    if target == KnownSearchStatus::Unknown
        && let Some(writer) = state.write_repository.as_ref()
    {
        let resolved = crate::status_resolution::RemoteStatusResolver {
            repository: &state.repository,
            writer,
            fetcher: &state.remote_fetcher,
            origin: &state.origin,
            limited_federation: state.instance_runtime.limited_federation,
        }
        .resolve(viewer, url)
        .await?;
        target = state
            .repository
            .known_search_status_id(
                resolved.as_deref().unwrap_or(url),
                state.origin.as_str(),
                viewer,
            )
            .await
            .map_err(|_| ())?;
    }
    let KnownSearchStatus::Found(id) = target else {
        return Ok(Vec::new());
    };
    let status = state
        .loader(Some(viewer))
        .authorized_status(id)
        .await
        .map_err(|_| ())?;
    status
        .iter()
        .map(|status| {
            state
                .serializer()
                .status(status, StatusShape::Full)
                .map_err(|_| ())
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
async fn search_v2(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match optional_viewer(&state, &headers, READ_SEARCH).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(Some(query)) = oauth_scalar(&rack, "q") else {
        return error_response(StatusCode::BAD_REQUEST, "q is required");
    };
    if query.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "q is required");
    }
    let Ok(search_type) = oauth_scalar(&rack, "type") else {
        return error_response(StatusCode::BAD_REQUEST, "type is invalid");
    };
    let search_type = search_type.filter(|value| !value.trim().is_empty());
    let resolve = boolean_parameter(&rack, "resolve");
    if owner.is_none() && (resolve || rack.get("offset").is_some()) {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "Search resolution and pagination require authentication",
        );
    }
    let Ok(limit) = nonnegative_search_parameter(&rack, "limit", 20, Some(40)) else {
        return error_response(StatusCode::BAD_REQUEST, "limit is invalid");
    };
    let offset = if search_type.is_some() {
        match nonnegative_search_parameter(&rack, "offset", 0, None) {
            Ok(offset) => offset,
            Err(()) => return error_response(StatusCode::BAD_REQUEST, "offset is invalid"),
        }
    } else {
        0
    };
    // A URL search must never fall through to hashtag/text results.
    if resolve && (query.trim().starts_with("https://") || query.trim().starts_with("http://")) {
        let mut statuses = Vec::new();
        if let (Some(viewer), Some(url)) = (
            owner,
            status_search_url(query, resolve, search_type, limit, offset),
        ) {
            statuses = match search_status_url(&state, viewer, url).await {
                Ok(statuses) => statuses,
                Err(()) => return internal_error(),
            };
        }
        return json_response(
            StatusCode::OK,
            serde_json::to_vec(&serde_json::json!({
                "accounts": [], "statuses": statuses, "hashtags": [], "collections": [],
            }))
            .expect("search response is serializable"),
        );
    }
    let accounts = if search_type.is_none_or(|value| value == "accounts") && limit > 0 {
        match search_accounts(
            &state,
            &metadata,
            owner,
            AccountSearchOptions {
                query: Some(query),
                resolve: boolean_parameter(&rack, "resolve"),
                following: boolean_parameter(&rack, "following"),
                limit,
                offset,
            },
        )
        .await
        {
            Ok(accounts) => accounts,
            Err(response) => return response,
        }
    } else {
        Vec::new()
    };
    let exclude_unreviewed = boolean_parameter(&rack, "exclude_unreviewed");
    let tags = if search_type.is_none_or(|value| value == "hashtags") && limit > 0 {
        match state
            .loader(owner)
            .tag_search(query, limit, offset, exclude_unreviewed)
            .await
        {
            Ok(tags) => tags,
            Err(_) => return internal_error(),
        }
    } else {
        Vec::new()
    };
    let serializer = state.serializer();
    let hashtags = tags
        .iter()
        .map(|tag| serializer.tag(tag))
        .collect::<Vec<_>>();
    json_response(
        StatusCode::OK,
        serde_json::to_vec(&serde_json::json!({
            "accounts": accounts,
            "statuses": [],
            "hashtags": hashtags,
            "collections": [],
        }))
        .expect("search response is serializable"),
    )
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(left.get(index).copied().unwrap_or_default())
            ^ usize::from(right.get(index).copied().unwrap_or_default());
    }
    difference == 0
}

async fn markers(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer_owner(&state, &headers, READ_STATUSES).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let timelines = rack_array_values(&rack, "timeline");
    let Ok(markers) = state
        .loader(Some(owner.account_id()))
        .markers(owner.user_id(), &timelines)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = markers
        .iter()
        .map(|marker| (marker.timeline.clone(), serializer.marker(marker)))
        .collect::<BTreeMap<_, _>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn marker_update(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_STATUSES).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let mut updates = Vec::new();
    for timeline in ["home", "notifications"] {
        let last_read_id = match marker_last_read_id(&rack, timeline) {
            Ok(Some(MarkerParameter::Value(last_read_id))) => Some(last_read_id),
            Ok(Some(MarkerParameter::Default)) => None,
            Ok(None) => continue,
            Err(()) => return error_response(StatusCode::BAD_REQUEST, "Invalid marker parameters"),
        };
        updates.push((timeline.to_owned(), last_read_id));
    }
    let markers = match writer.update_markers(&authenticated, &updates).await {
        Ok(markers) => markers,
        Err(WriteError::Conflict) => {
            return error_response(
                StatusCode::CONFLICT,
                "Conflict during update, please try again",
            );
        }
        Err(WriteError::Unauthorized) => {
            return error_response(StatusCode::UNAUTHORIZED, "Unauthorized");
        }
        Err(WriteError::Forbidden) => {
            return error_response(StatusCode::FORBIDDEN, "This action is not allowed");
        }
        Err(WriteError::InvalidInput(_)) => {
            return error_response(StatusCode::BAD_REQUEST, "Invalid marker parameters");
        }
        Err(
            WriteError::Sqlx(_)
            | WriteError::Job(_)
            | WriteError::Filesystem(_)
            | WriteError::NotFound
            | WriteError::RateLimited
            | WriteError::Validation(_),
        ) => {
            return internal_error();
        }
    };
    let mut values = BTreeMap::new();
    for marker in markers {
        values.insert(
            marker.timeline.clone(),
            RestMarker {
                last_read_id: DecimalId::new(marker.last_read_id),
                version: marker.lock_version,
                updated_at: ApiDateTime::new(marker.updated_at),
            },
        );
    }
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

#[allow(clippy::too_many_lines)]
async fn report_create(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_REPORTS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let target_account_id = match report_id_parameter(&rack, "account_id") {
        Ok(ids) if ids.len() == 1 => ids[0],
        _ => {
            return report_response_with_rate_limit(
                writer,
                owner,
                error_response(StatusCode::BAD_REQUEST, "Invalid account_id"),
            )
            .await;
        }
    };
    let Ok(comment) = report_string_parameter(&rack, "comment") else {
        return report_response_with_rate_limit(
            writer,
            owner,
            error_response(StatusCode::BAD_REQUEST, "Invalid comment"),
        )
        .await;
    };
    let Ok(category) = report_string_parameter(&rack, "category") else {
        return report_response_with_rate_limit(
            writer,
            owner,
            error_response(StatusCode::BAD_REQUEST, "Invalid category"),
        )
        .await;
    };
    let Ok(status_ids) = report_id_parameter(&rack, "status_ids") else {
        return report_response_with_rate_limit(
            writer,
            owner,
            error_response(StatusCode::BAD_REQUEST, "Invalid status_ids"),
        )
        .await;
    };
    let Ok(collection_ids) = report_id_parameter(&rack, "collection_ids") else {
        return report_response_with_rate_limit(
            writer,
            owner,
            error_response(StatusCode::BAD_REQUEST, "Invalid collection_ids"),
        )
        .await;
    };
    let Ok(rule_ids) = report_id_parameter(&rack, "rule_ids") else {
        return report_response_with_rate_limit(
            writer,
            owner,
            error_response(StatusCode::BAD_REQUEST, "Invalid rule_ids"),
        )
        .await;
    };
    let forward = optional_boolean_parameter(&rack, "forward");
    let Ok(forward_to_domains) = report_forward_domains_parameter(&rack) else {
        return report_response_with_rate_limit(
            writer,
            owner,
            error_response(StatusCode::BAD_REQUEST, "Invalid forward_to_domains"),
        )
        .await;
    };
    let report_id = match writer
        .create_report(
            &authenticated,
            target_account_id,
            comment.as_deref().unwrap_or_default(),
            category.as_deref(),
            &status_ids,
            &collection_ids,
            &rule_ids,
            forward,
            forward_to_domains.as_deref(),
            state.origin.as_str(),
            state
                .mail_config
                .as_ref()
                .is_some_and(MailConfig::is_enabled),
        )
        .await
    {
        Ok(report_id) => report_id,
        Err(WriteError::RateLimited) => {
            return rate_limited_response(RateLimitExceeded {
                limit: usize::try_from(REPORT_RATE_LIMIT).unwrap_or_default(),
                period: REPORT_RATE_LIMIT_PERIOD,
            });
        }
        Err(error) => {
            return report_response_with_rate_limit(writer, owner, report_write_error(&error))
                .await;
        }
    };
    let report = match state.loader(Some(owner)).report(report_id).await {
        Ok(Some(report)) => report,
        Ok(None) => {
            return report_response_with_rate_limit(writer, owner, record_not_found()).await;
        }
        Err(_) => {
            return report_response_with_rate_limit(writer, owner, internal_error()).await;
        }
    };
    let Some(body) = state
        .serializer()
        .report(&report)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    else {
        return report_response_with_rate_limit(writer, owner, internal_error()).await;
    };
    report_response_with_rate_limit(writer, owner, json_response(StatusCode::OK, body)).await
}

#[allow(clippy::too_many_lines)]
async fn status_create(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_STATUSES).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let parameter = |name: &str| -> Result<Option<String>, ()> {
        match rack.get(name) {
            None | Some(RackValue::Null) => Ok(None),
            Some(RackValue::Scalar(value)) => Ok(Some(value.clone())),
            Some(RackValue::Number(value)) => Ok(Some(ruby_json_number(value))),
            Some(RackValue::Boolean(value)) => Ok(Some(value.to_string())),
            Some(RackValue::Array(_) | RackValue::Object(_) | RackValue::Upload(_)) => Err(()),
        }
    };
    let text = match parameter("status") {
        Ok(Some(value)) => value,
        Ok(None) => String::new(),
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "Invalid status"),
    };
    let Ok(media_ids) = status_media_ids(&rack) else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid media_ids");
    };
    let poll = match status_poll(&rack) {
        Ok(poll) => poll,
        Err(error) => return status_saved_write_error(&error),
    };
    let Ok(spoiler_text) = parameter("spoiler_text") else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid spoiler_text");
    };
    let Ok(visibility) = parameter("visibility") else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid visibility");
    };
    let Ok(language) = parameter("language") else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid language");
    };
    let Ok(quote_approval_policy) = parameter("quote_approval_policy") else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid quote_approval_policy");
    };
    let sensitive = optional_boolean_parameter(&rack, "sensitive");
    let Ok(in_reply_to_id) = integer_parameter(&rack, "in_reply_to_id") else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid in_reply_to_id");
    };
    let Ok(quoted_status_id) = strict_optional_id_parameter(&rack, "quoted_status_id") else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid quoted_status_id");
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let idempotency_key = match headers.get("Idempotency-Key") {
        None => None,
        Some(value) => match value.to_str() {
            Ok(value) => Some(value.to_owned()),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid Idempotency-Key"),
        },
    };
    let idempotency_fingerprint = idempotency_key.as_deref().map(|_| {
        status_idempotency_fingerprint(
            &text,
            &media_ids,
            spoiler_text.as_deref(),
            visibility.as_deref(),
            language.as_deref(),
            quote_approval_policy.as_deref(),
            sensitive,
            in_reply_to_id,
            quoted_status_id,
            poll.as_ref(),
        )
    });
    let idempotency_scope = status_idempotency_scope(owner);
    let idempotency =
        idempotency_key
            .as_deref()
            .zip(idempotency_fingerprint)
            .map(|(key, fingerprint)| IdempotencyKey {
                scope: &idempotency_scope,
                key,
                fingerprint,
                expires_at: Utc::now() + ChronoDuration::hours(1),
            });
    if let Some(in_reply_to_id) = in_reply_to_id {
        match state
            .loader(Some(owner))
            .authorized_status(in_reply_to_id)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return record_not_found(),
            Err(_) => return internal_error(),
        }
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let outcome = match writer
        .create_status_with_quote_policy(
            &authenticated,
            &text,
            &media_ids,
            spoiler_text.as_deref(),
            sensitive,
            visibility.as_deref(),
            language.as_deref(),
            quote_approval_policy.as_deref(),
            in_reply_to_id,
            quoted_status_id,
            poll.as_ref(),
            Some(state.origin.as_str()),
            state.instance_runtime.limited_federation,
            idempotency,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return status_saved_write_error(&error),
    };
    let status = match state
        .loader(Some(owner))
        .authorized_status(outcome.status_id)
        .await
    {
        Ok(Some(status)) => status,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .status(&status, StatusShape::Full)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn status_update(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_STATUSES).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let parameter = |name: &str| -> Result<Option<String>, ()> {
        match rack.get(name) {
            None | Some(RackValue::Null) => Ok(None),
            Some(RackValue::Scalar(value)) => Ok(Some(value.clone())),
            Some(RackValue::Number(value)) => Ok(Some(ruby_json_number(value))),
            Some(RackValue::Boolean(value)) => Ok(Some(value.to_string())),
            Some(RackValue::Array(_) | RackValue::Object(_) | RackValue::Upload(_)) => Err(()),
        }
    };
    let text = match parameter("status") {
        Ok(value) => Some(value.unwrap_or_default()),
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "Invalid status"),
    };
    let spoiler_text = match parameter("spoiler_text") {
        Ok(value) => Some(value.unwrap_or_default()),
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "Invalid spoiler_text"),
    };
    let language = match parameter("language") {
        Ok(value) => Some(value.unwrap_or_default()),
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "Invalid language"),
    };
    let sensitive = Some(
        optional_boolean_parameter(&rack, "sensitive").unwrap_or(false)
            || spoiler_text
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
    );
    let Ok(media_ids) = status_media_ids(&rack) else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid media_ids");
    };
    let Ok(media_attributes) = status_media_attributes(&rack) else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid media_attributes");
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let update = StatusUpdate {
        text,
        spoiler_text,
        sensitive,
        language,
        media_ids: Some(media_ids),
        media_attributes,
    };
    if let Err(error) = writer
        .update_status(&authenticated, status_id, &update)
        .await
    {
        return status_saved_write_error(&error);
    }
    let status = match state.loader(Some(owner)).authorized_status(status_id).await {
        Ok(Some(status)) => status,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .status(&status, StatusShape::Full)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

#[allow(clippy::too_many_arguments)]
fn status_idempotency_fingerprint(
    text: &str,
    media_ids: &[i64],
    spoiler_text: Option<&str>,
    visibility: Option<&str>,
    language: Option<&str>,
    quote_approval_policy: Option<&str>,
    sensitive: Option<bool>,
    in_reply_to_id: Option<i64>,
    quoted_status_id: Option<i64>,
    poll: Option<&PollCreate>,
) -> [u8; 32] {
    let reply_id = in_reply_to_id.map(|id| id.to_string());
    let quote_id = quoted_status_id.map(|id| id.to_string());
    let values = [
        Some(text.trim()),
        spoiler_text.map(str::trim),
        visibility,
        language,
        quote_approval_policy,
        sensitive.map(|value| if value { "true" } else { "false" }),
        reply_id.as_deref(),
        quote_id.as_deref(),
    ];
    let mut digest = Sha256::new();
    for value in values {
        if let Some(value) = value {
            digest.update(value.as_bytes());
        }
        digest.update([0]);
    }
    digest.update(b"media_ids");
    digest.update([0]);
    for media_id in media_ids {
        digest.update(media_id.to_string().as_bytes());
        digest.update([0]);
    }
    digest.update(b"poll");
    digest.update([0]);
    if let Some(poll) = poll {
        digest.update(poll.expires_in.to_string().as_bytes());
        digest.update([0]);
        digest.update([u8::from(poll.multiple), u8::from(poll.hide_totals)]);
        digest.update([0]);
        for option in &poll.options {
            digest.update(option.as_bytes());
            digest.update([0]);
        }
    }
    digest.finalize().into()
}

fn ruby_string_to_i(value: &str) -> i64 {
    let value = value.trim_start();
    let (negative, digits) = match value.as_bytes().first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0_i64, |number, digit| {
            if negative {
                number
                    .saturating_mul(10)
                    .saturating_sub(i64::from(digit - b'0'))
            } else {
                number
                    .saturating_mul(10)
                    .saturating_add(i64::from(digit - b'0'))
            }
        })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn ruby_json_number_to_i(value: &serde_json::Number) -> i64 {
    value.as_i64().unwrap_or_else(|| {
        value.as_u64().map_or_else(
            || {
                let value = value.as_f64().unwrap_or(0.0).trunc();
                if value >= i64::MAX as f64 {
                    i64::MAX
                } else if value <= i64::MIN as f64 {
                    i64::MIN
                } else {
                    value as i64
                }
            },
            |value| i64::try_from(value).unwrap_or(i64::MAX),
        )
    })
}

fn poll_expires_in(value: Option<&RackValue>) -> Result<i64, WriteError> {
    match value {
        None | Some(RackValue::Null) => Err(WriteError::Validation("Expires at can't be blank")),
        Some(RackValue::Scalar(value)) if value.trim().is_empty() => {
            Err(WriteError::Validation("Expires at can't be blank"))
        }
        Some(RackValue::Scalar(value)) => Ok(ruby_string_to_i(value)),
        Some(RackValue::Number(value)) => Ok(ruby_json_number_to_i(value)),
        Some(_) => Err(WriteError::InvalidInput("Invalid poll expires_in")),
    }
}

fn status_poll(parameters: &RackParameters) -> Result<Option<PollCreate>, WriteError> {
    let Some(value) = parameters.get("poll") else {
        return Ok(None);
    };
    if matches!(value, RackValue::Null) {
        return Ok(None);
    }
    let RackValue::Object(fields) = value else {
        return Err(WriteError::InvalidInput("Invalid poll"));
    };
    let options = match fields.get("options") {
        Some(RackValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                RackValue::Scalar(value) => Ok(value.clone()),
                _ => Err(WriteError::InvalidInput("Invalid poll options")),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(WriteError::InvalidInput("Invalid poll options")),
    };
    let expires_in = poll_expires_in(fields.get("expires_in"))?;
    let multiple = fields.get("multiple").is_some_and(boolean_value);
    let hide_totals = fields.get("hide_totals").is_some_and(boolean_value);
    crate::mastodon::prepare_local_poll(&options, expires_in, multiple, hide_totals)
        .map(Some)
        .map_err(WriteError::Validation)
}

fn status_media_ids(parameters: &RackParameters) -> Result<Vec<i64>, ()> {
    match parameters.get("media_ids") {
        None | Some(RackValue::Null) => Ok(Vec::new()),
        Some(RackValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                RackValue::Scalar(value) => value.parse::<i64>().map_err(|_| ()),
                RackValue::Number(value) => value.as_i64().ok_or(()),
                _ => Err(()),
            })
            .collect(),
        Some(
            RackValue::Scalar(_)
            | RackValue::Number(_)
            | RackValue::Boolean(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => Err(()),
    }
}

fn status_media_attributes(
    parameters: &RackParameters,
) -> Result<Option<Vec<StatusMediaAttributeUpdate>>, ()> {
    let Some(value) = parameters.get("media_attributes") else {
        return Ok(None);
    };
    let RackValue::Array(values) = value else {
        return Err(());
    };
    values
        .iter()
        .map(|value| {
            let RackValue::Object(fields) = value else {
                return Err(());
            };
            let id = match fields.get("id") {
                Some(RackValue::Scalar(value)) => value.parse::<i64>().map_err(|_| ())?,
                Some(RackValue::Number(value)) => value.as_i64().ok_or(())?,
                _ => return Err(()),
            };
            Ok(StatusMediaAttributeUpdate {
                id,
                description: media_description_value(fields.get("description")).map_err(|_| ())?,
                focus: media_focus_value(fields.get("focus")).map_err(|_| ())?,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn report_id_parameter(parameters: &RackParameters, name: &str) -> Result<Vec<i64>, ()> {
    match parameters.get(name) {
        None | Some(RackValue::Null) => Ok(Vec::new()),
        Some(RackValue::Array(values)) => values.iter().map(report_id_value).collect(),
        Some(value @ (RackValue::Scalar(_) | RackValue::Number(_))) => {
            Ok(vec![report_id_value(value)?])
        }
        Some(RackValue::Boolean(_) | RackValue::Object(_) | RackValue::Upload(_)) => Err(()),
    }
}

fn report_string_parameter(parameters: &RackParameters, name: &str) -> Result<Option<String>, ()> {
    match parameters.get(name) {
        None | Some(RackValue::Null) => Ok(None),
        Some(RackValue::Scalar(value)) => Ok(Some(value.clone())),
        Some(RackValue::Number(value)) => Ok(Some(ruby_json_number(value))),
        Some(
            RackValue::Boolean(_)
            | RackValue::Array(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => Err(()),
    }
}

fn report_forward_domains_parameter(
    parameters: &RackParameters,
) -> Result<Option<Vec<String>>, ()> {
    let Some(value) = parameters.get("forward_to_domains") else {
        return Ok(None);
    };
    let values = match value {
        RackValue::Array(values) => values,
        RackValue::Null => return Ok(None),
        RackValue::Scalar(_)
        | RackValue::Number(_)
        | RackValue::Boolean(_)
        | RackValue::Object(_)
        | RackValue::Upload(_) => return Err(()),
    };
    let mut domains = Vec::new();
    for value in values {
        let RackValue::Scalar(value) = value else {
            return Err(());
        };
        let domain = canonical_remote_domain(value).map_err(|_| ())?;
        if !domains
            .iter()
            .any(|candidate: &String| candidate.eq_ignore_ascii_case(&domain))
        {
            domains.push(domain);
        }
    }
    Ok(Some(domains))
}

fn report_id_value(value: &RackValue) -> Result<i64, ()> {
    match value {
        RackValue::Scalar(value) => value.parse::<i64>().map_err(|_| ()),
        RackValue::Number(value) => value.as_i64().ok_or(()),
        RackValue::Null
        | RackValue::Boolean(_)
        | RackValue::Array(_)
        | RackValue::Object(_)
        | RackValue::Upload(_) => Err(()),
    }
}

fn status_idempotency_scope(account_id: i64) -> String {
    format!("status:create:{account_id}")
}

async fn media_create_v1(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    media_create(state, rack, headers, false).await
}

async fn media_create_v2(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    media_create(state, rack, headers, true).await
}

#[allow(clippy::too_many_lines)]
async fn media_create(
    state: WebState,
    rack: RackParameters,
    headers: HeaderMap,
    rich: bool,
) -> Response<Body> {
    let rate_limit_user_id = match optional_authenticated_user_id(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if let Some(user_id) = rate_limit_user_id
        && let Err(limited) = state
            .media_upload_limiter
            .check_shared(state.shared_rate_limiter.as_ref(), user_id)
            .await
    {
        return rate_limited_response(limited);
    }
    let authenticated = match required_write_viewer(&state, &headers, WRITE_MEDIA).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner,
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Some(RackValue::Upload(upload)) = rack.get("file") else {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "File type of uploaded media could not be verified",
        );
    };
    let description = match media_description_parameter(&rack, "description") {
        Ok(AccountProfileValue::Unchanged | AccountProfileValue::Null) => None,
        Ok(AccountProfileValue::Value(value)) => Some(value),
        Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, error),
    };
    let focus = match media_focus_parameter(&rack, "focus") {
        Ok(focus) => focus,
        Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, error),
    };
    if rich
        && crate::media::media_format(&upload.content_type)
            .is_some_and(|format| format.external_processing)
    {
        return media_create_rich(
            &state,
            &authenticated,
            upload,
            MediaAttachmentUpdate {
                description: description
                    .map_or(AccountProfileValue::Null, AccountProfileValue::Value),
                focus,
            },
        )
        .await;
    }
    let prepared = match crate::paperclip::prepare_media_attachment_async(
        owner.account_id(),
        upload.file_name.clone(),
        upload.content_type.clone(),
        upload.bytes.clone(),
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => return media_upload_error(error),
    };
    let create = MediaAttachmentCreate {
        media_type: prepared.media_kind.database_type(),
        file_name: prepared.file_name.clone(),
        content_type: prepared.content_type.clone(),
        file_size: prepared.file_size,
        file_meta: prepared.file_meta.clone(),
        blurhash: prepared.blurhash.clone(),
        description,
        focus,
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let account_id = owner.account_id();
    match writer
        .with_account_lock(account_id, || async {
            let id = writer
                .stage_media_attachment_locked(&authenticated, &create)
                .await?;
            let metadata = media_metadata_from_prepared(id, &prepared);
            if let Err(error) = write_prepared_media(&state.media_root, &metadata, &prepared) {
                remove_expected_media(&state.media_root, &metadata);
                return Err(WriteError::Filesystem(error));
            }
            // A failed commit has an ambiguous outcome. Retain the files so either the
            // published row is complete or its durable rollback intent removes them.
            writer
                .publish_media_attachment_locked(&authenticated, id, &create)
                .await?;
            let response = media_response_for_id(&state, account_id, id).await;
            if !response.status().is_success() {
                let _ = cleanup_media_after_response_failure(
                    &state.media_root,
                    writer,
                    &authenticated,
                    id,
                )
                .await;
            }
            Ok(response)
        })
        .await
    {
        Ok(response) => response,
        Err(error) => media_write_error(&error),
    }
}

// Modern stills are intentionally asynchronous too: the bundled composer polls
// 206 until ready, or terminates on retained 422. No filename exists before publish.
async fn media_create_rich(
    state: &WebState,
    authenticated: &AuthenticatedBearer,
    upload: &UploadedFile,
    update: MediaAttachmentUpdate,
) -> Response<Body> {
    use crate::mastodon::local_uploads::{RawInput, raw_path};
    use sha2::{Digest, Sha256};
    let Some(format) = crate::media::media_format(&upload.content_type) else {
        return media_upload_error(crate::paperclip::MediaAttachmentError::UnsupportedContentType);
    };
    if upload.bytes.is_empty() || upload.bytes.len() >= format.input_size_limit {
        return media_upload_error(crate::paperclip::MediaAttachmentError::TooLarge);
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let account = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let bytes = upload.bytes.clone();
    let hash: [u8; 32] =
        match tokio::task::spawn_blocking(move || Sha256::digest(bytes).into()).await {
            Ok(hash) => hash,
            Err(_) => return internal_error(),
        };
    match writer
        .with_account_lock(account, || async {
            let id = writer
                .stage_local_upload_locked(
                    authenticated,
                    &RawInput {
                        mime: &upload.content_type,
                        size: i64::try_from(upload.bytes.len()).expect("bounded upload length"),
                        sha256: &hash,
                    },
                    &update,
                )
                .await?;
            // Stage commit precedes all writes. Synchronous confined write/fsync remains
            // inside the account lock, including cancellation; no detached write may
            // outlive ownership. Every ambiguous/error outcome retains its manifest.
            state
                .media_root
                .private_upload_root()?
                .write_file(FsPath::new(&raw_path(id)), &upload.bytes)?;
            writer.accept_local_upload_locked(authenticated, id).await?;
            let mut response = media_response_for_id(state, account, id.media_id).await;
            if response.status() == StatusCode::PARTIAL_CONTENT {
                *response.status_mut() = StatusCode::ACCEPTED;
            }
            Ok(response)
        })
        .await
    {
        Ok(response) => response,
        Err(error) => media_write_error(&error),
    }
}

async fn media_show(State(state): State<WebState>, uri: Uri, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, WRITE_MEDIA).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some((_, id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    media_response_for_id(&state, owner, id).await
}

async fn media_update(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_MEDIA).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner,
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Some((_, id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let current = match state
        .repository
        .media_attachment(owner.account_id(), id)
        .await
    {
        Ok(Some(media)) => media,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if current
        .processing
        .is_some_and(|processing| processing.0 == 3)
    {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Error processing thumbnail for uploaded media",
        );
    }
    let update = match media_attachment_update(&rack) {
        Ok(update) => update,
        Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, error),
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .update_media_attachment(&authenticated, id, &update)
        .await
    {
        return media_write_error(&error);
    }
    media_response_for_id(&state, owner.account_id(), id).await
}

async fn media_delete(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_MEDIA).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let account_id = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    if let Err(error) = writer
        .with_account_lock(account_id, || async {
            let media = writer
                .delete_media_attachment_locked(&authenticated, id)
                .await?;
            remove_media_files(&state.media_root, &media);
            Ok(())
        })
        .await
    {
        return media_write_error(&error);
    }
    empty_json_response()
}

async fn media_response_for_id(state: &WebState, account_id: i64, id: i64) -> Response<Body> {
    let media = match state.repository.media_attachment(account_id, id).await {
        Ok(Some(media))
            if media.file_file_name.is_some()
                || (media.remote_url.is_empty()
                    && media
                        .processing
                        .is_some_and(|state| matches!(state.0, 1 | 3))) =>
        {
            media
        }
        Ok(Some(_) | None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if media.processing.is_some_and(|processing| processing.0 == 3) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Error processing thumbnail for uploaded media",
        );
    }
    let status = if media.processing.is_some_and(|processing| processing.0 != 2) {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let projection = media_projection(&media, media.description.clone());
    match serde_json::to_vec(&state.serializer().media_attachment(&projection)) {
        Ok(body) => json_response(status, body),
        Err(_) => internal_error(),
    }
}

fn media_attachment_update(rack: &RackParameters) -> Result<MediaAttachmentUpdate, &'static str> {
    Ok(MediaAttachmentUpdate {
        description: media_description_parameter(rack, "description")?,
        focus: media_focus_parameter(rack, "focus")?,
    })
}

fn media_description_parameter(
    rack: &RackParameters,
    name: &str,
) -> Result<AccountProfileValue<String>, &'static str> {
    media_description_value(rack.get(name))
}

fn media_description_value(
    value: Option<&RackValue>,
) -> Result<AccountProfileValue<String>, &'static str> {
    match value {
        None => Ok(AccountProfileValue::Unchanged),
        Some(RackValue::Null) => Ok(AccountProfileValue::Null),
        Some(value) => profile_scalar_string(value)
            .map(AccountProfileValue::Value)
            .map_err(|()| "Invalid media description"),
    }
}

fn media_focus_parameter(
    rack: &RackParameters,
    name: &str,
) -> Result<AccountProfileValue<MediaFocus>, &'static str> {
    media_focus_value(rack.get(name))
}

fn media_focus_value(
    value: Option<&RackValue>,
) -> Result<AccountProfileValue<MediaFocus>, &'static str> {
    let Some(value) = value else {
        return Ok(AccountProfileValue::Unchanged);
    };
    if matches!(value, RackValue::Null)
        || matches!(value, RackValue::Scalar(value) if value.trim().is_empty())
    {
        return Ok(AccountProfileValue::Unchanged);
    }
    let values = match value {
        RackValue::Scalar(value) => value.split(',').map(str::to_owned).collect::<Vec<_>>(),
        RackValue::Array(values) if values.len() == 2 => values
            .iter()
            .map(profile_scalar_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|()| "Invalid media focus")?,
        RackValue::Object(values) => ["x", "y"]
            .into_iter()
            .map(|key| values.get(key).ok_or(()).and_then(profile_scalar_string))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|()| "Invalid media focus")?,
        _ => return Err("Invalid media focus"),
    };
    if values.len() != 2 {
        return Err("Invalid media focus");
    }
    let x = values[0]
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or("Invalid media focus")?;
    let y = values[1]
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or("Invalid media focus")?;
    Ok(AccountProfileValue::Value(MediaFocus { x, y }))
}

fn media_metadata_from_prepared(id: i64, prepared: &PreparedMediaAttachment) -> PaperclipMetadata {
    PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id,
        remote: false,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    }
}

fn media_metadata_from_record(
    media: &crate::mastodon::MediaAttachment,
) -> Option<PaperclipMetadata> {
    Some(PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: media.id,
        remote: !crate::paperclip::rails_blank(&media.remote_url),
        storage_schema_version: media.file_storage_schema_version,
        file_name: media.file_file_name.clone()?,
        content_type: media.file_content_type.clone(),
        variant: None,
    })
}

fn media_thumbnail_metadata_from_record(
    media: &crate::mastodon::MediaAttachment,
) -> Option<PaperclipMetadata> {
    Some(PaperclipMetadata {
        attachment: PaperclipAttachment::MediaThumbnail,
        id: media.id,
        remote: !crate::paperclip::rails_blank(&media.remote_url),
        storage_schema_version: media.thumbnail_storage_schema_version,
        file_name: media.thumbnail_file_name.clone()?,
        content_type: media.thumbnail_content_type.clone(),
        variant: None,
    })
}

fn remove_media_files(root: &PaperclipRoot, media: &crate::mastodon::MediaAttachment) {
    if let Some(metadata) = media_metadata_from_record(media) {
        for style in ["original", "small"] {
            if let Some(path) = metadata.relative_path(style) {
                let _ = root.remove_file(FsPath::new(&path));
            }
        }
    }
    if let Some(metadata) = media_thumbnail_metadata_from_record(media)
        && let Some(path) = metadata.relative_path("original")
    {
        let _ = root.remove_file(FsPath::new(&path));
    }
}

async fn cleanup_media_after_response_failure(
    root: &PaperclipRoot,
    writer: &WriteRepository,
    authenticated: &AuthenticatedBearer,
    id: i64,
) -> Result<(), WriteError> {
    let media = writer
        .delete_media_attachment_locked(authenticated, id)
        .await?;
    remove_media_files(root, &media);
    Ok(())
}

#[cfg(feature = "test-support")]
/// Exercises the same ordered cleanup used after media response serialization fails.
///
/// # Errors
///
/// Returns the metadata transaction error without unlinking any published files.
pub async fn cleanup_media_after_response_failure_for_test(
    root: &PaperclipRoot,
    writer: &WriteRepository,
    authenticated: &AuthenticatedBearer,
    id: i64,
) -> Result<(), WriteError> {
    let account_id = authenticated
        .require_user()
        .map_err(|_| WriteError::Unauthorized)?
        .account_id();
    writer
        .with_account_lock(account_id, || async {
            cleanup_media_after_response_failure(root, writer, authenticated, id).await
        })
        .await
}

fn remove_expected_media(root: &PaperclipRoot, metadata: &PaperclipMetadata) {
    for style in ["original", "small"] {
        if let Some(path) = metadata.relative_path(style) {
            let _ = root.remove_file(FsPath::new(&path));
        }
    }
}

fn media_upload_error(error: crate::paperclip::MediaAttachmentError) -> Response<Body> {
    let (status, message) = match error {
        crate::paperclip::MediaAttachmentError::TooLarge => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "File size of uploaded media is too large",
        ),
        crate::paperclip::MediaAttachmentError::ProcessingTimedOut => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Uploaded media took too long to process",
        ),
        crate::paperclip::MediaAttachmentError::ProcessingUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Media processing is temporarily unavailable",
        ),
        crate::paperclip::MediaAttachmentError::UnsupportedContentType
        | crate::paperclip::MediaAttachmentError::InvalidImage
        | crate::paperclip::MediaAttachmentError::InvalidMedia
        | crate::paperclip::MediaAttachmentError::SizeOverflow => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "File type of uploaded media could not be verified",
        ),
    };
    error_response(status, message)
}

fn media_write_error(error: &WriteError) -> Response<Body> {
    match error {
        WriteError::Unauthorized => error_response(StatusCode::UNAUTHORIZED, "Unauthorized"),
        WriteError::Forbidden => {
            error_response(StatusCode::FORBIDDEN, "This action is not allowed")
        }
        WriteError::NotFound => record_not_found(),
        WriteError::Validation(message) => {
            error_response(StatusCode::UNPROCESSABLE_ENTITY, message)
        }
        WriteError::InvalidInput(message) => error_response(StatusCode::BAD_REQUEST, message),
        WriteError::Conflict => error_response(
            StatusCode::CONFLICT,
            "Conflict during update, please try again",
        ),
        WriteError::RateLimited
        | WriteError::Sqlx(_)
        | WriteError::Job(_)
        | WriteError::Filesystem(_) => internal_error(),
    }
}

fn report_write_error(error: &WriteError) -> Response<Body> {
    match error {
        WriteError::Unauthorized => error_response(StatusCode::UNAUTHORIZED, "Unauthorized"),
        WriteError::Forbidden => {
            error_response(StatusCode::FORBIDDEN, "This action is not allowed")
        }
        WriteError::NotFound => record_not_found(),
        WriteError::Validation(message) => {
            error_response(StatusCode::UNPROCESSABLE_ENTITY, message)
        }
        WriteError::InvalidInput(message) => error_response(StatusCode::BAD_REQUEST, message),
        WriteError::Conflict => error_response(
            StatusCode::CONFLICT,
            "Conflict during update, please try again",
        ),
        WriteError::RateLimited => rate_limited_response(RateLimitExceeded {
            limit: usize::try_from(REPORT_RATE_LIMIT).unwrap_or_default(),
            period: REPORT_RATE_LIMIT_PERIOD,
        }),
        WriteError::Sqlx(_) | WriteError::Job(_) | WriteError::Filesystem(_) => internal_error(),
    }
}

async fn conversations(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_STATUSES).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let (max_id, min_id, since_id) = match cursor_triplet(&rack) {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = TimelineOptions {
        max_id,
        min_id,
        since_id,
        limit: match limit_parameter(&rack, 20, 40) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
        ..TimelineOptions::default()
    };
    let Ok(projections) = state
        .loader(Some(owner))
        .conversations(owner, &options)
        .await
    else {
        return internal_error();
    };
    let serialized = projections
        .iter()
        .map(|projection| serialize_conversation(&state, projection))
        .collect::<Result<Vec<_>, _>>();
    let Ok(values) = serialized else {
        return internal_error();
    };
    let first_id = projections
        .first()
        .and_then(|conversation| conversation.last_status.as_ref())
        .map(|status| status.id);
    let last_id = projections
        .last()
        .and_then(|conversation| conversation.last_status.as_ref())
        .map(|status| status.id);
    let mut response = match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => return internal_error(),
    };
    let parameters = parameters(query.as_deref());
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| projections.len() == limit)
        && let Some(last_id) = last_id
        && let Some(url) = pagination_url(
            &state,
            "api/v1/conversations",
            &parameters,
            "max_id",
            last_id,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_id) = first_id
        && let Some(url) = pagination_url(
            &state,
            "api/v1/conversations",
            &parameters,
            "min_id",
            first_id,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

fn serialize_conversation(
    state: &WebState,
    projection: &ConversationProjection,
) -> Result<RestConversation, RestError> {
    let serializer = state.serializer();
    Ok(RestConversation {
        id: DecimalId::new(projection.id),
        unread: projection.unread,
        accounts: projection
            .participant_accounts
            .iter()
            .map(|account| serializer.account(account))
            .collect::<Result<Vec<_>, _>>()?,
        last_status: projection
            .last_status
            .as_ref()
            .map(|status| serializer.status(status, StatusShape::Full))
            .transpose()?,
    })
}

async fn streaming(
    State(state): State<WebState>,
    websocket: WebSocketUpgrade,
    headers: HeaderMap,
    uri: Uri,
    RawQuery(query): RawQuery,
) -> Response<Body> {
    let (auth_headers, websocket_protocol) = streaming_credentials(&headers, query.as_deref());
    let authenticated = match state
        .authenticator
        .authenticate(&auth_headers, NO_SCOPE)
        .await
    {
        Ok(authenticated) => authenticated,
        Err(OAuthAuthenticationError::OAuth(error)) => {
            return error.into_http_response().map(Body::from);
        }
        Err(OAuthAuthenticationError::Repository(_)) => return internal_error(),
    };
    if let Err(error) = authenticated.require_user() {
        return error.into_http_response().map(Body::from);
    }
    let Some(queue) = state.queue.clone() else {
        return internal_error();
    };
    let Ok(cursor) = queue.stream_cursor().await else {
        return internal_error();
    };
    let authenticated = match state
        .authenticator
        .authenticate(&auth_headers, NO_SCOPE)
        .await
    {
        Ok(authenticated) => authenticated,
        Err(OAuthAuthenticationError::OAuth(error)) => {
            return error.into_http_response().map(Body::from);
        }
        Err(OAuthAuthenticationError::Repository(_)) => return internal_error(),
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner,
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let scopes = authenticated.scopes().clone();
    let only_media = streaming_query_parameter(query.as_deref(), "only_media")
        .is_some_and(|value| activitypub_truthy(&value));
    let initial_stream = streaming_query_parameter(query.as_deref(), "stream")
        .or_else(|| streaming_path_stream(uri.path(), only_media).map(str::to_owned));
    let initial_command = streaming_initial_command(initial_stream, query.as_deref());
    let websocket = if let Some(protocol) = websocket_protocol {
        websocket.protocols([protocol])
    } else {
        websocket
    };
    websocket.on_upgrade(move |socket| {
        streaming_connection(
            socket,
            state,
            queue,
            auth_headers,
            owner.user_id(),
            owner.account_id(),
            authenticated.token_id(),
            scopes,
            cursor,
            initial_command,
        )
    })
}

fn streaming_credentials(headers: &HeaderMap, query: Option<&str>) -> (HeaderMap, Option<String>) {
    if headers.contains_key(AUTHORIZATION) {
        return (headers.clone(), None);
    }
    let query_token =
        streaming_query_parameter(query, "access_token").filter(|token| !token.is_empty());
    let protocol = if query_token.is_none() {
        headers
            .get(SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    } else {
        None
    };
    let Some(token) = query_token.or_else(|| protocol.clone()) else {
        return (headers.clone(), None);
    };
    let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) else {
        return (headers.clone(), None);
    };
    let mut headers = headers.clone();
    headers.insert(AUTHORIZATION, value);
    (headers, protocol)
}

fn streaming_query_parameter(query: Option<&str>, name: &str) -> Option<String> {
    parameters(query)
        .into_iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

fn streaming_path_stream(path: &str, only_media: bool) -> Option<&'static str> {
    match path {
        "/api/v1/streaming/user" => Some("user"),
        "/api/v1/streaming/user/notification" => Some("user:notification"),
        "/api/v1/streaming/direct" => Some("direct"),
        "/api/v1/streaming/public" => Some(if only_media { "public:media" } else { "public" }),
        "/api/v1/streaming/public/local" => Some(if only_media {
            "public:local:media"
        } else {
            "public:local"
        }),
        "/api/v1/streaming/public/remote" => Some(if only_media {
            "public:remote:media"
        } else {
            "public:remote"
        }),
        "/api/v1/streaming/hashtag" => Some("hashtag"),
        "/api/v1/streaming/hashtag/local" => Some("hashtag:local"),
        "/api/v1/streaming/list" => Some("list"),
        _ => None,
    }
}

fn streaming_initial_command(stream: Option<String>, query: Option<&str>) -> ParsedCommand {
    let Some(stream) = stream else {
        return ParsedCommand::Ignore;
    };
    let mut command = serde_json::Map::from_iter([
        (
            "type".to_owned(),
            serde_json::Value::String("subscribe".to_owned()),
        ),
        ("stream".to_owned(), serde_json::Value::String(stream)),
    ]);
    for name in ["tag", "list"] {
        if let Some(value) = streaming_query_parameter(query, name) {
            command.insert(name.to_owned(), serde_json::Value::String(value));
        }
    }
    ClientCommand::parse(&serde_json::Value::Object(command).to_string())
}

#[derive(Default)]
struct StreamingSubscriptions {
    values: BTreeMap<Subscription, i64>,
}

impl StreamingSubscriptions {
    fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    fn has_capacity_for(&self, subscription: &Subscription) -> bool {
        self.values.contains_key(subscription) || self.values.len() < STREAM_MAX_SUBSCRIPTIONS
    }

    fn insert(&mut self, subscription: Subscription, baseline_cursor: i64) {
        self.values.insert(subscription, baseline_cursor);
    }

    fn remove(&mut self, subscription: &Subscription) {
        self.values.remove(subscription);
    }

    fn contains(&self, stream: StreamName) -> bool {
        self.values.contains_key(&Subscription::from(stream))
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn streaming_connection(
    mut socket: WebSocket,
    state: WebState,
    queue: Queue,
    auth_headers: HeaderMap,
    user_id: i64,
    account_id: i64,
    token_id: i64,
    scopes: OAuthScopes,
    mut cursor: i64,
    initial_command: ParsedCommand,
) {
    let live_cursor = cursor;
    let mut subscriptions = StreamingSubscriptions::default();
    let mut has_established_subscription = false;
    let mut system_cursor = cursor;
    let mut heartbeat = interval(StdDuration::from_secs(30));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let mut poll = interval(StdDuration::from_millis(100));
    poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
    poll.tick().await;
    let mut last_pong = Instant::now();

    if !streaming_handle_command(
        &mut socket,
        &state,
        &queue,
        &mut subscriptions,
        &mut has_established_subscription,
        initial_command,
        user_id,
        account_id,
        live_cursor,
        &scopes,
    )
    .await
    {
        return;
    }

    loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(message) = message else {
                    return;
                };
                let Ok(message) = message else {
                    return;
                };
                match message {
                    Message::Text(text) => {
                        let command = ClientCommand::parse(text.as_str());
                        if !streaming_handle_command(
                            &mut socket,
                            &state,
                            &queue,
                            &mut subscriptions,
                            &mut has_established_subscription,
                            command,
                            user_id,
                            account_id,
                            live_cursor,
                            &scopes,
                        ).await {
                            return;
                        }
                    }
                    Message::Binary(_) => {
                        let _ = socket
                            .send(Message::Close(Some(CloseFrame {
                                code: 1003,
                                reason: "The mastodon streaming server does not support binary messages".into(),
                            })))
                            .await;
                        return;
                    }
                    Message::Pong(_) => last_pong = Instant::now(),
                    Message::Close(_) => return,
                    Message::Ping(_) => {}
                }
            }
            _ = heartbeat.tick() => {
                if last_pong.elapsed() >= StdDuration::from_mins(1) {
                    return;
                }
                let valid = match state.authenticator.authenticate(&auth_headers, NO_SCOPE).await {
                    Ok(authenticated) => authenticated
                        .require_user()
                        .is_ok_and(|owner| owner.account_id() == account_id),
                    Err(_) => false,
                };
                if !valid {
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: 1000,
                            reason: "Invalid access token".into(),
                        })))
                        .await;
                    return;
                }
                if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                    return;
                }
            }
            _ = poll.tick() => {
                let Ok(mut kill) =
                    stream_system_events(&queue, account_id, token_id, &mut system_cursor).await
                else {
                    return;
                };
                if !kill && !subscriptions.is_empty() {
                    match stream_pending_events(
                        &mut socket,
                        &state,
                        &queue,
                        user_id,
                        account_id,
                        token_id,
                        &scopes,
                        &mut subscriptions,
                        &mut cursor,
                        live_cursor,
                    )
                    .await
                    {
                        Ok(pending_kill) => kill = pending_kill,
                        Err(()) => return,
                    }
                }
                if kill {
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: 1000,
                            reason: "Invalid access token".into(),
                        })))
                        .await;
                    return;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn streaming_handle_command(
    socket: &mut WebSocket,
    state: &WebState,
    queue: &Queue,
    subscriptions: &mut StreamingSubscriptions,
    has_established_subscription: &mut bool,
    command: ParsedCommand,
    user_id: i64,
    account_id: i64,
    connection_cursor: i64,
    scopes: &OAuthScopes,
) -> bool {
    match command {
        ParsedCommand::Command(ClientCommand::Subscribe(subscription)) => {
            streaming_subscribe(
                socket,
                state,
                queue,
                subscriptions,
                has_established_subscription,
                subscription,
                user_id,
                account_id,
                connection_cursor,
                scopes,
            )
            .await
        }
        ParsedCommand::Command(ClientCommand::Unsubscribe(subscription)) => {
            subscriptions.remove(&subscription);
            true
        }
        ParsedCommand::Reject(error) => match error.status {
            Some(status) => send_stream_error(socket, status, error.message).await,
            None => socket
                .send(Message::Text(
                    r#"{"error":"Error unsubscribing from channel"}"#.into(),
                ))
                .await
                .is_ok(),
        },
        ParsedCommand::Ignore => true,
    }
}

fn subscription_create_after(
    connection_cursor: i64,
    subscribe_cursor: i64,
    has_established_subscription: bool,
) -> i64 {
    if has_established_subscription {
        subscribe_cursor
    } else {
        connection_cursor
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn streaming_subscribe(
    socket: &mut WebSocket,
    state: &WebState,
    queue: &Queue,
    subscriptions: &mut StreamingSubscriptions,
    has_established_subscription: &mut bool,
    subscription: Subscription,
    user_id: i64,
    account_id: i64,
    connection_cursor: i64,
    scopes: &OAuthScopes,
) -> bool {
    let stream = subscription.stream();
    if !subscriptions.has_capacity_for(&subscription) {
        return send_stream_error(socket, 400, "Too many subscriptions").await;
    }
    if subscriptions.values.contains_key(&subscription) {
        return true;
    }
    let subscribe_cursor = if subscription.is_timeline() {
        let Ok(cursor) = queue.stream_cursor().await else {
            return false;
        };
        cursor
    } else {
        0
    };
    let create_after = subscription_create_after(
        connection_cursor,
        subscribe_cursor,
        *has_established_subscription,
    );
    if !stream.permits(scopes) {
        return send_stream_error(
            socket,
            401,
            "Access token does not have the required scopes",
        )
        .await;
    }
    let authorization = if stream == StreamName::List {
        let list_id = subscription
            .parameter()
            .and_then(|list| list.parse::<i64>().ok())
            .filter(|id| *id > 0);
        match list_id {
            Some(list_id) => state
                .repository
                .rest_owned_list_exists(account_id, list_id)
                .await
                .map_err(|_| ()),
            None => Ok(false),
        }
    } else if matches!(stream, StreamName::Hashtag | StreamName::HashtagLocal)
        && subscription.parameter().is_none_or(str::is_empty)
    {
        Ok(false)
    } else if stream.is_timeline() {
        stream_timeline_options(state, user_id, account_id, stream)
            .await
            .map(|options| options.is_some())
    } else {
        Ok(true)
    };
    match authorization {
        Ok(true) => {}
        Ok(false) => {
            let (status, message) = if stream == StreamName::List {
                (401, "Not authorized to stream this list")
            } else if matches!(stream, StreamName::Hashtag | StreamName::HashtagLocal) {
                (400, "Missing tag name parameter")
            } else {
                (401, "Not authorized to stream this feed")
            };
            return send_stream_error(socket, status, message).await;
        }
        Err(()) => return false,
    }
    let replay_through = if subscription.is_timeline() {
        let Ok(cursor) = queue.stream_cursor().await else {
            return false;
        };
        cursor
    } else {
        0
    };
    if subscription.is_timeline() {
        let Ok(events) = queue
            .stream_replay_events_for_subscription(
                replay_through,
                create_after,
                &subscription,
                account_id,
            )
            .await
        else {
            return false;
        };
        for event in events {
            if stream_timeline_event_for_subscription(
                socket,
                state,
                user_id,
                account_id,
                &subscription,
                &event,
                Some(create_after),
            )
            .await
            .is_err()
            {
                return false;
            }
        }
    }
    subscriptions.insert(subscription, replay_through);
    *has_established_subscription = true;
    true
}

async fn send_stream_error(socket: &mut WebSocket, status: u16, error: &str) -> bool {
    let message = format!(
        "{{\"error\":{},\"status\":{status}}}",
        serde_json::to_string(error).expect("stream error is serializable")
    );
    socket.send(Message::Text(message.into())).await.is_ok()
}

async fn stream_timeline_options(
    state: &WebState,
    user_id: i64,
    account_id: i64,
    stream: StreamName,
) -> Result<Option<TimelineOptions>, ()> {
    let mut options = TimelineOptions {
        local: matches!(
            stream,
            StreamName::PublicLocal | StreamName::PublicLocalMedia | StreamName::HashtagLocal
        ),
        remote: matches!(
            stream,
            StreamName::PublicRemote | StreamName::PublicRemoteMedia
        ),
        only_media: matches!(
            stream,
            StreamName::PublicMedia | StreamName::PublicLocalMedia | StreamName::PublicRemoteMedia
        ),
        ..TimelineOptions::default()
    };
    let settings = state.repository.settings().await.map_err(|_| ())?;
    let setting = |name: &str| {
        settings
            .iter()
            .find(|setting| setting.var == name)
            .and_then(|setting| setting.value.as_ref())
            .and_then(|value| yaml_scalar(value.raw()))
            .unwrap_or_else(|| "public".to_owned())
    };
    let topic = matches!(stream, StreamName::Hashtag | StreamName::HashtagLocal);
    let (local, remote) = if topic {
        (
            setting("local_topic_feed_access"),
            setting("remote_topic_feed_access"),
        )
    } else {
        (
            setting("local_live_feed_access"),
            setting("remote_live_feed_access"),
        )
    };
    let can_view_disabled = state
        .repository
        .user_can_view_feeds(user_id, account_id)
        .await
        .map_err(|_| ())?;
    let allowed = |setting: &str| match setting {
        "public" | "authenticated" => true,
        "disabled" => can_view_disabled,
        _ => false,
    };
    let access = FeedAccess {
        local: allowed(&local),
        remote: allowed(&remote),
    };
    Ok(apply_feed_access(&mut options, access).then_some(options))
}

#[allow(clippy::too_many_arguments)]
async fn stream_pending_events(
    socket: &mut WebSocket,
    state: &WebState,
    queue: &Queue,
    user_id: i64,
    account_id: i64,
    token_id: i64,
    scopes: &OAuthScopes,
    subscriptions: &mut StreamingSubscriptions,
    cursor: &mut i64,
    live_cursor: i64,
) -> Result<bool, ()> {
    let events = queue
        .stream_events_after(*cursor, STREAM_EVENT_BATCH_SIZE)
        .await
        .map_err(|_| ())?;
    for event in events {
        *cursor = event.id;
        if event.account_id == 0 {
            stream_timeline_event(socket, state, user_id, account_id, subscriptions, &event)
                .await?;
            continue;
        }
        if event.id <= live_cursor || event.account_id != account_id {
            continue;
        }
        if event.event == SYSTEM_KILL_EVENT
            || (event.event == TOKEN_KILL_EVENT && event.object_id == token_id)
        {
            return Ok(true);
        }
        if event.event == TOKEN_KILL_EVENT {
            continue;
        }
        let streams = stream_account_targets(&event, scopes, subscriptions);
        if streams.is_empty() {
            continue;
        }
        let Some(payload) = stream_event_payload(state, account_id, &event).await? else {
            continue;
        };
        for subscription in streams {
            socket
                .send(Message::Text(
                    event_message(&subscription, stream_protocol_event(&event.event), &payload)
                        .into(),
                ))
                .await
                .map_err(|_| ())?;
        }
    }
    Ok(false)
}

async fn stream_timeline_event(
    socket: &mut WebSocket,
    state: &WebState,
    user_id: i64,
    account_id: i64,
    subscriptions: &StreamingSubscriptions,
    event: &StreamEvent,
) -> Result<(), ()> {
    let timeline_subscriptions = subscriptions
        .values
        .iter()
        .filter(|(subscription, baseline_cursor)| {
            subscription.is_timeline() && event.id > **baseline_cursor
        })
        .map(|(subscription, _)| subscription.clone())
        .collect::<Vec<_>>();
    for subscription in timeline_subscriptions {
        stream_timeline_event_for_subscription(
            socket,
            state,
            user_id,
            account_id,
            &subscription,
            event,
            None,
        )
        .await?;
    }
    Ok(())
}

async fn stream_timeline_event_for_subscription(
    socket: &mut WebSocket,
    state: &WebState,
    user_id: i64,
    account_id: i64,
    subscription: &Subscription,
    event: &StreamEvent,
    create_after: Option<i64>,
) -> Result<(), ()> {
    if !matches!(event.event.as_str(), "update" | "status.update" | "delete") {
        return Ok(());
    }
    let before = match event.before.as_ref() {
        Some(snapshot) => {
            // The before snapshot is authoritative for route membership: the current status row
            // already reflects the after state (and may already be suspended or silenced).
            stream_timeline_delete_snapshot_contains(
                state,
                user_id,
                account_id,
                subscription,
                snapshot,
            )
            .await?
        }
        None => false,
    };
    let after = match event.after.as_ref() {
        Some(snapshot) => {
            stream_timeline_snapshot_contains(
                state,
                user_id,
                account_id,
                subscription,
                event.object_id,
                snapshot,
            )
            .await?
        }
        None => false,
    };
    let protocol_event = match create_after {
        Some(create_after) => {
            timeline_replay_protocol_event(&event.event, before, after, event.id, create_after)
        }
        None => timeline_protocol_event(&event.event, before, after),
    };
    let Some(protocol_event) = protocol_event else {
        return Ok(());
    };
    let payload = if protocol_event == "delete" {
        event.object_id.to_string()
    } else {
        let Some(payload) = stream_event_payload(state, account_id, event).await? else {
            return Ok(());
        };
        payload
    };
    socket
        .send(Message::Text(
            event_message(subscription, protocol_event, &payload).into(),
        ))
        .await
        .map_err(|_| ())
}

fn timeline_protocol_event(event: &str, before: bool, after: bool) -> Option<&'static str> {
    match (before, after) {
        (false, true) => Some("update"),
        (true, true) => Some("status.update"),
        (true, false) if event != "delete" => Some("status.update"),
        (true, false) => Some("delete"),
        (false, false) => None,
    }
}

fn timeline_replay_protocol_event(
    event: &str,
    before: bool,
    after: bool,
    event_id: i64,
    create_after: i64,
) -> Option<&'static str> {
    timeline_protocol_event(event, before, after)
        .filter(|protocol_event| *protocol_event != "update" || event_id > create_after)
}

async fn stream_timeline_delete_snapshot_contains(
    state: &WebState,
    user_id: i64,
    account_id: i64,
    subscription: &Subscription,
    snapshot: &TimelineRouteSnapshot,
) -> Result<bool, ()> {
    let stream = subscription.stream();
    if stream != StreamName::List
        && stream_timeline_options(state, user_id, account_id, stream)
            .await?
            .is_none()
    {
        return Ok(false);
    }
    let language_allowed = if matches!(
        stream,
        StreamName::Public
            | StreamName::PublicMedia
            | StreamName::PublicLocal
            | StreamName::PublicLocalMedia
            | StreamName::PublicRemote
            | StreamName::PublicRemoteMedia
    ) {
        state
            .repository
            .stream_public_language_allowed(account_id, snapshot.language.as_deref())
            .await
            .map_err(|_| ())?
    } else {
        true
    };
    Ok(timeline_delete_snapshot_matches(
        snapshot,
        subscription,
        account_id,
        language_allowed,
    ))
}

fn timeline_delete_snapshot_matches(
    snapshot: &TimelineRouteSnapshot,
    subscription: &Subscription,
    account_id: i64,
    language_allowed: bool,
) -> bool {
    let stream = subscription.stream();
    match stream {
        StreamName::Public
        | StreamName::PublicMedia
        | StreamName::PublicLocal
        | StreamName::PublicLocalMedia
        | StreamName::PublicRemote
        | StreamName::PublicRemoteMedia => {
            let locality_matches = match stream {
                StreamName::PublicLocal | StreamName::PublicLocalMedia => snapshot.local,
                StreamName::PublicRemote | StreamName::PublicRemoteMedia => !snapshot.local,
                _ => true,
            };
            let media_matches = !matches!(
                stream,
                StreamName::PublicMedia
                    | StreamName::PublicLocalMedia
                    | StreamName::PublicRemoteMedia
            ) || snapshot.had_media;
            snapshot.public && locality_matches && media_matches && language_allowed
        }
        StreamName::Hashtag | StreamName::HashtagLocal => {
            snapshot.hashtag
                && (stream != StreamName::HashtagLocal || snapshot.local)
                && subscription
                    .parameter()
                    .is_some_and(|tag| snapshot.tags.iter().any(|candidate| candidate == tag))
        }
        StreamName::List => subscription
            .parameter()
            .and_then(|list| list.parse::<i64>().ok())
            .is_some_and(|list_id| {
                snapshot
                    .lists
                    .iter()
                    .any(|route| route.account_id == account_id && route.list_id == list_id)
            }),
        StreamName::User | StreamName::UserNotification | StreamName::Direct => false,
    }
}

async fn stream_timeline_snapshot_contains(
    state: &WebState,
    user_id: i64,
    account_id: i64,
    subscription: &Subscription,
    status_id: i64,
    snapshot: &TimelineRouteSnapshot,
) -> Result<bool, ()> {
    let stream = subscription.stream();
    match stream {
        StreamName::Public
        | StreamName::PublicMedia
        | StreamName::PublicLocal
        | StreamName::PublicLocalMedia
        | StreamName::PublicRemote
        | StreamName::PublicRemoteMedia => {
            let Some(options) = stream_timeline_options(state, user_id, account_id, stream).await?
            else {
                return Ok(false);
            };
            state
                .repository
                .stream_public_timeline_contains(
                    account_id,
                    status_id,
                    options,
                    true,
                    Some(snapshot.had_media),
                    snapshot.language.as_deref(),
                    true,
                )
                .await
                .map_err(|_| ())
        }
        StreamName::Hashtag | StreamName::HashtagLocal => {
            let Some(tag) = subscription.parameter() else {
                return Ok(false);
            };
            let Some(options) = stream_timeline_options(state, user_id, account_id, stream).await?
            else {
                return Ok(false);
            };
            let tag_matches = snapshot.tags.iter().any(|candidate| candidate == tag);
            state
                .repository
                .stream_tag_timeline_contains(
                    account_id,
                    status_id,
                    tag,
                    options,
                    true,
                    Some(tag_matches),
                    Some(snapshot.had_media),
                )
                .await
                .map_err(|_| ())
        }
        StreamName::List => {
            let Some(list_id) = subscription
                .parameter()
                .and_then(|list| list.parse::<i64>().ok())
            else {
                return Ok(false);
            };
            let historically_matched = snapshot
                .lists
                .iter()
                .any(|route| route.account_id == account_id && route.list_id == list_id);
            if !historically_matched {
                return Ok(false);
            }
            state
                .repository
                .stream_list_timeline_contains(
                    account_id,
                    list_id,
                    status_id,
                    true,
                    snapshot.language.as_deref(),
                )
                .await
                .map(|current| current.unwrap_or(true))
                .map_err(|_| ())
        }
        StreamName::User | StreamName::UserNotification | StreamName::Direct => Ok(false),
    }
}

async fn stream_system_events(
    queue: &Queue,
    account_id: i64,
    token_id: i64,
    cursor: &mut i64,
) -> Result<bool, ()> {
    let events = queue
        .stream_events_after(*cursor, STREAM_EVENT_BATCH_SIZE)
        .await
        .map_err(|_| ())?;
    for event in events {
        *cursor = event.id;
        if event.account_id == account_id
            && (event.event == SYSTEM_KILL_EVENT
                || (event.event == TOKEN_KILL_EVENT && event.object_id == token_id))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn stream_account_targets(
    event: &StreamEvent,
    scopes: &OAuthScopes,
    subscriptions: &StreamingSubscriptions,
) -> Vec<Subscription> {
    if event.event == STATUS_UPDATE_NOTIFICATION_EVENT {
        let mut streams = Vec::with_capacity(2);
        if subscriptions.contains(StreamName::User)
            && StreamName::User.includes_notifications(scopes)
        {
            streams.push(StreamName::User.into());
        }
        if subscriptions.contains(StreamName::UserNotification)
            && StreamName::UserNotification.permits(scopes)
        {
            streams.push(StreamName::UserNotification.into());
        }
        streams
    } else if event.event == "conversation" {
        if subscriptions.contains(StreamName::Direct) && StreamName::Direct.permits(scopes) {
            vec![StreamName::Direct.into()]
        } else {
            Vec::new()
        }
    } else if matches!(
        event.event.as_str(),
        "notification" | "notifications_merged"
    ) {
        let mut streams = Vec::with_capacity(2);
        if subscriptions.contains(StreamName::User)
            && StreamName::User.includes_notifications(scopes)
        {
            streams.push(StreamName::User.into());
        }
        if subscriptions.contains(StreamName::UserNotification)
            && StreamName::UserNotification.permits(scopes)
        {
            streams.push(StreamName::UserNotification.into());
        }
        streams
    } else if subscriptions.contains(StreamName::User) {
        vec![StreamName::User.into()]
    } else {
        Vec::new()
    }
}

fn stream_protocol_event(event: &str) -> &str {
    if event == STATUS_UPDATE_NOTIFICATION_EVENT {
        "status.update"
    } else {
        event
    }
}

async fn stream_event_payload(
    state: &WebState,
    account_id: i64,
    event: &StreamEvent,
) -> Result<Option<String>, ()> {
    match event.event.as_str() {
        "delete" => Ok(Some(event.object_id.to_string())),
        "update" | "status.update" | STATUS_UPDATE_NOTIFICATION_EVENT => {
            let Some(status) = state
                .loader(Some(account_id))
                .authorized_status(event.object_id)
                .await
                .map_err(|_| ())?
            else {
                return Ok(None);
            };
            serde_json::to_string(
                &state
                    .serializer()
                    .status(&status, StatusShape::Full)
                    .map_err(|_| ())?,
            )
            .map(Some)
            .map_err(|_| ())
        }
        "notification" => {
            let Some(notification) = state
                .loader(Some(account_id))
                .notification(account_id, event.object_id)
                .await
                .map_err(|_| ())?
            else {
                return Ok(None);
            };
            serde_json::to_string(
                &state
                    .serializer()
                    .notification(&notification, None)
                    .map_err(|_| ())?,
            )
            .map(Some)
            .map_err(|_| ())
        }
        "notifications_merged" => Ok(Some("1".to_owned())),
        "conversation" => {
            let Some(conversation) = state
                .loader(Some(account_id))
                .conversation(account_id, event.object_id)
                .await
                .map_err(|_| ())?
            else {
                return Ok(None);
            };
            serde_json::to_string(&serialize_conversation(state, &conversation).map_err(|_| ())?)
                .map(Some)
                .map_err(|_| ())
        }
        _ => Ok(None),
    }
}

async fn conversation_read(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    conversation_unread_state(state, uri, headers, false).await
}

async fn conversation_unread(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    conversation_unread_state(state, uri, headers, true).await
}

async fn conversation_unread_state(
    state: WebState,
    uri: Uri,
    headers: HeaderMap,
    unread: bool,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_CONVERSATIONS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, conversation_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .update_conversation_unread(&authenticated, conversation_id, unread)
        .await
    {
        return conversation_write_error(&error);
    }
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(_) => return error_response(StatusCode::UNAUTHORIZED, "Unauthorized"),
    };
    let Ok(conversation) = state
        .loader(Some(owner))
        .conversation(owner, conversation_id)
        .await
    else {
        return internal_error();
    };
    let Some(conversation) = conversation else {
        return record_not_found();
    };
    match serialize_conversation(&state, &conversation)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn conversation_delete(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_CONVERSATIONS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, conversation_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .delete_conversation(&authenticated, conversation_id)
        .await
    {
        Ok(()) => empty_json_response(),
        Err(WriteError::NotFound) => record_not_found(),
        Err(_) => internal_error(),
    }
}

fn conversation_write_error(error: &WriteError) -> Response<Body> {
    match error {
        WriteError::NotFound => record_not_found(),
        WriteError::Unauthorized => error_response(StatusCode::UNAUTHORIZED, "Unauthorized"),
        WriteError::Forbidden => {
            error_response(StatusCode::FORBIDDEN, "This action is not allowed")
        }
        WriteError::InvalidInput(_) => {
            error_response(StatusCode::BAD_REQUEST, "Invalid conversation parameters")
        }
        WriteError::Validation(message) => error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("Validation failed: {message}"),
        ),
        WriteError::Conflict => error_response(
            StatusCode::CONFLICT,
            "Conflict during update, please try again",
        ),
        WriteError::RateLimited
        | WriteError::Sqlx(_)
        | WriteError::Job(_)
        | WriteError::Filesystem(_) => internal_error(),
    }
}

enum MarkerParameter {
    Default,
    Value(i64),
}

fn marker_last_read_id(
    parameters: &RackParameters,
    timeline: &str,
) -> Result<Option<MarkerParameter>, ()> {
    let Some(value) = parameters.get(timeline) else {
        return Ok(None);
    };
    let RackValue::Object(values) = value else {
        return Err(());
    };
    let Some(value) = values.get("last_read_id") else {
        return Ok(Some(MarkerParameter::Default));
    };
    match value {
        RackValue::Scalar(value) => value
            .parse::<i64>()
            .map(MarkerParameter::Value)
            .map(Some)
            .map_err(|_| ()),
        RackValue::Number(value) => value
            .as_i64()
            .map(MarkerParameter::Value)
            .map(Some)
            .ok_or(()),
        RackValue::Null
        | RackValue::Boolean(_)
        | RackValue::Array(_)
        | RackValue::Object(_)
        | RackValue::Upload(_) => Err(()),
    }
}

async fn notifications(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_NOTIFICATIONS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let options = match notification_options(&rack, false) {
        Ok(options) => options,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let supported_types = notification_supported_types(&rack);
    let Ok(projections) = state
        .loader(Some(owner))
        .notifications(owner, &options)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let ids = projections
        .iter()
        .map(|notification| notification.id)
        .collect::<Vec<_>>();
    let values = projections
        .iter()
        .map(|notification| serializer.notification(notification, supported_types.as_deref()))
        .collect::<Result<Vec<_>, _>>();
    let Some(body) = values
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    else {
        return internal_error();
    };
    notification_response(
        &state,
        "api/v1/notifications",
        &parameters(query.as_deref()),
        &[
            "limit",
            "account_id",
            "types[]",
            "exclude_types[]",
            "include_filtered",
            "supported_types[]",
        ],
        &ids,
        body,
    )
}

async fn grouped_notifications(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_NOTIFICATIONS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    if grouped_types_parameter_invalid(&rack) {
        return framework_internal_error();
    }
    let options = match notification_options(&rack, true) {
        Ok(options) => options,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let supported_types = notification_supported_types(&rack);
    let Some(partial_avatars) = expand_accounts_parameter(&rack) else {
        return invalid_expand_accounts_response(&rack);
    };
    let Ok(grouped) = state
        .loader(Some(owner))
        .grouped_notifications(owner, &options)
        .await
    else {
        return internal_error();
    };
    let Some(body) = state
        .serializer()
        .grouped_notifications(&grouped, partial_avatars, supported_types.as_deref())
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    else {
        return internal_error();
    };
    let ids = grouped
        .groups
        .iter()
        .map(|group| group.notification.id)
        .collect::<Vec<_>>();
    notification_response(
        &state,
        "api/v2/notifications",
        &parameters(query.as_deref()),
        &[
            "limit",
            "types[]",
            "exclude_types[]",
            "include_filtered",
            "grouped_types[]",
            "supported_types[]",
        ],
        &ids,
        body,
    )
}

async fn notification_unread_count(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    notification_unread_count_impl(state, rack, headers, false).await
}

async fn grouped_notification_unread_count(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    notification_unread_count_impl(state, rack, headers, true).await
}

async fn notification_unread_count_impl(
    state: WebState,
    rack: RackParameters,
    headers: HeaderMap,
    grouped: bool,
) -> Response<Body> {
    let owner = match required_viewer_owner(&state, &headers, READ_NOTIFICATIONS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    if grouped && grouped_types_parameter_invalid(&rack) {
        return framework_internal_error();
    }
    let mut options = match notification_options_with_limits(&rack, grouped, 100, 1_000, false) {
        Ok(options) => options,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let marker = match state
        .repository
        .rest_markers(owner.user_id(), &["notifications".to_owned()])
        .await
    {
        Ok(markers) => markers
            .into_iter()
            .find(|marker| marker.timeline == "notifications")
            .map(|marker| marker.last_read_id),
        Err(_) => return internal_error(),
    };
    options.max_id = None;
    options.since_id = None;
    options.min_id = marker;
    let Ok(notifications) = state
        .repository
        .rest_notifications(owner.account_id(), &options, grouped)
        .await
    else {
        return internal_error();
    };
    match serde_json::to_vec(&serde_json::json!({ "count": notifications.len() })) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn notification_show(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_NOTIFICATIONS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some((_, notification_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let notification = match state
        .loader(Some(owner))
        .notification(owner, notification_id)
        .await
    {
        Ok(Some(notification)) => notification,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    let supported_types = notification_supported_types(&rack);
    match state
        .serializer()
        .notification(&notification, supported_types.as_deref())
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn notification_requests(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_NOTIFICATIONS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let (max_id, min_id, since_id) = match cursor_triplet(&rack) {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let Ok(limit) = limit_parameter(&rack, 40, 80) else {
        return framework_internal_error();
    };
    let Ok(requests) = state
        .loader(Some(owner))
        .notification_requests(owner, max_id, since_id, min_id, limit)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let Ok(values) = requests
        .iter()
        .map(|request| serializer.notification_request(request))
        .collect::<Result<Vec<_>, _>>()
    else {
        return internal_error();
    };
    let Ok(body) = serde_json::to_vec(&values) else {
        return internal_error();
    };
    let ids = requests
        .iter()
        .map(|request| request.id)
        .collect::<Vec<_>>();
    notification_request_response(
        &state,
        "api/v1/notifications/requests",
        &parameters(query.as_deref()),
        &["limit"],
        &ids,
        body,
        limit,
    )
}

async fn notification_requests_merged(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_NOTIFICATIONS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let merged = match state.queue.as_ref() {
        Some(queue) => match queue.notification_unfilter_pending(owner).await {
            Ok(pending) => !pending,
            Err(_) => return internal_error(),
        },
        None => true,
    };
    let body = if merged {
        br#"{"merged":true}"#.to_vec()
    } else {
        br#"{"merged":false}"#.to_vec()
    };
    json_response(StatusCode::OK, body)
}

async fn notification_policy_v1(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let (policy, summary) = match notification_policy_data(&state, &headers).await {
        Ok(data) => data,
        Err(response) => return response,
    };
    notification_policy_response(policy.as_ref(), summary, false)
}

async fn notification_policy_v2(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let (policy, summary) = match notification_policy_data(&state, &headers).await {
        Ok(data) => data,
        Err(response) => return response,
    };
    notification_policy_response(policy.as_ref(), summary, true)
}

async fn notification_policy_v1_update(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let update = NotificationPolicyUpdate {
        for_bots: optional_boolean_parameter(&rack, "filter_bots").map(i32::from),
        for_limited_accounts: None,
        for_new_accounts: optional_boolean_parameter(&rack, "filter_new_accounts").map(i32::from),
        for_not_followers: optional_boolean_parameter(&rack, "filter_not_followers").map(i32::from),
        for_not_following: optional_boolean_parameter(&rack, "filter_not_following").map(i32::from),
        for_private_mentions: optional_boolean_parameter(&rack, "filter_private_mentions")
            .map(i32::from),
    };
    notification_policy_update(state, headers, update, false).await
}

async fn notification_policy_v2_update(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let Ok(update) = notification_policy_mode_update(&rack) else {
        return framework_internal_error();
    };
    notification_policy_update(state, headers, update, true).await
}

async fn notification_policy_update(
    state: WebState,
    headers: HeaderMap,
    update: NotificationPolicyUpdate,
    v2: bool,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_NOTIFICATIONS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let account_id = match authenticated.resource_owner() {
        Some(owner) => owner.account_id(),
        None => return internal_error(),
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let Ok(policy) = writer
        .update_notification_policy(&authenticated, update)
        .await
    else {
        return internal_error();
    };
    let Ok(summary) = state
        .repository
        .notification_policy_summary(account_id)
        .await
    else {
        return internal_error();
    };
    notification_policy_response(Some(&policy), summary, v2)
}

async fn notification_policy_data(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<(Option<NotificationPolicy>, (i64, i64)), Response<Body>> {
    let owner = required_viewer(state, headers, READ_NOTIFICATIONS).await?;
    let policy = state
        .repository
        .notification_policy(owner)
        .await
        .map_err(|_| internal_error())?;
    let summary = state
        .repository
        .notification_policy_summary(owner)
        .await
        .map_err(|_| internal_error())?;
    Ok((policy, summary))
}

fn notification_policy_response(
    policy: Option<&NotificationPolicy>,
    (pending_requests_count, pending_notifications_count): (i64, i64),
    v2: bool,
) -> Response<Body> {
    let [
        for_bots,
        for_limited_accounts,
        for_new_accounts,
        for_not_followers,
        for_not_following,
        for_private_mentions,
    ] = notification_policy_values(policy);
    let summary = serde_json::json!({
        "pending_requests_count": pending_requests_count,
        "pending_notifications_count": pending_notifications_count,
    });
    let value = if v2 {
        serde_json::json!({
            "for_not_following": notification_policy_mode(for_not_following),
            "for_not_followers": notification_policy_mode(for_not_followers),
            "for_new_accounts": notification_policy_mode(for_new_accounts),
            "for_private_mentions": notification_policy_mode(for_private_mentions),
            "for_limited_accounts": notification_policy_mode(for_limited_accounts),
            "for_bots": notification_policy_mode(for_bots),
            "summary": summary,
        })
    } else {
        serde_json::json!({
            "filter_not_following": for_not_following != 0,
            "filter_not_followers": for_not_followers != 0,
            "filter_new_accounts": for_new_accounts != 0,
            "filter_private_mentions": for_private_mentions != 0,
            "filter_bots": for_bots != 0,
            "summary": summary,
        })
    };
    match serde_json::to_vec(&value) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

fn notification_policy_values(policy: Option<&NotificationPolicy>) -> [i32; 6] {
    policy.map_or([0, 1, 0, 0, 0, 1], |policy| {
        [
            policy.for_bots.0,
            policy.for_limited_accounts.0,
            policy.for_new_accounts.0,
            policy.for_not_followers.0,
            policy.for_not_following.0,
            policy.for_private_mentions.0,
        ]
    })
}

fn notification_policy_mode(value: i32) -> Option<&'static str> {
    match value {
        0 => Some("accept"),
        1 => Some("filter"),
        2 => Some("drop"),
        _ => None,
    }
}

fn notification_policy_mode_update(
    parameters: &RackParameters,
) -> Result<NotificationPolicyUpdate, ()> {
    Ok(NotificationPolicyUpdate {
        for_bots: notification_policy_mode_parameter(parameters, "for_bots")?,
        for_limited_accounts: notification_policy_mode_parameter(
            parameters,
            "for_limited_accounts",
        )?,
        for_new_accounts: notification_policy_mode_parameter(parameters, "for_new_accounts")?,
        for_not_followers: notification_policy_mode_parameter(parameters, "for_not_followers")?,
        for_not_following: notification_policy_mode_parameter(parameters, "for_not_following")?,
        for_private_mentions: notification_policy_mode_parameter(
            parameters,
            "for_private_mentions",
        )?,
    })
}

fn notification_policy_mode_parameter(
    parameters: &RackParameters,
    name: &str,
) -> Result<Option<i32>, ()> {
    match parameters.get(name) {
        None | Some(RackValue::Null) => Ok(None),
        Some(RackValue::Scalar(value)) => match value.as_str() {
            "accept" => Ok(Some(0)),
            "filter" => Ok(Some(1)),
            "drop" => Ok(Some(2)),
            _ => Err(()),
        },
        Some(_) => Err(()),
    }
}

async fn notification_request_show(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_NOTIFICATIONS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some((_, request_id)) = uri_path_id(&uri, 5) else {
        return record_not_found();
    };
    let Ok(Some(request)) = state
        .loader(Some(owner))
        .notification_request(owner, request_id)
        .await
    else {
        return record_not_found();
    };
    match state.serializer().notification_request(&request) {
        Ok(value) => match serde_json::to_vec(&value) {
            Ok(body) => json_response(StatusCode::OK, body),
            Err(_) => internal_error(),
        },
        Err(_) => internal_error(),
    }
}

async fn grouped_notification_show(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_NOTIFICATIONS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some(group_key) = uri_path_segment(&uri, 4) else {
        return record_not_found();
    };
    let grouped = match state
        .loader(Some(owner))
        .grouped_notification(owner, &group_key)
        .await
    {
        Ok(Some(grouped)) => grouped,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    let supported_types = notification_supported_types(&rack);
    match state
        .serializer()
        .grouped_notifications(&grouped, false, supported_types.as_deref())
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn notification_clear(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_NOTIFICATIONS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer.clear_notifications(&authenticated).await {
        Ok(()) => empty_json_response(),
        Err(_) => internal_error(),
    }
}

async fn notification_dismiss(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_NOTIFICATIONS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, notification_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .dismiss_notification(&authenticated, notification_id)
        .await
    {
        Ok(()) => empty_json_response(),
        Err(crate::mastodon::WriteError::NotFound) => record_not_found(),
        Err(_) => internal_error(),
    }
}

async fn notification_request_accept(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_NOTIFICATIONS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, request_id)) = uri_path_id(&uri, 5) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .accept_notification_request(&authenticated, request_id)
        .await
    {
        Ok(()) => empty_json_response(),
        Err(crate::mastodon::WriteError::NotFound) => record_not_found(),
        Err(_) => internal_error(),
    }
}

async fn notification_request_dismiss(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_NOTIFICATIONS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, request_id)) = uri_path_id(&uri, 5) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .dismiss_notification_request(&authenticated, request_id)
        .await
    {
        Ok(()) => empty_json_response(),
        Err(crate::mastodon::WriteError::NotFound) => record_not_found(),
        Err(_) => internal_error(),
    }
}

async fn notification_requests_accept(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    notification_requests_write(state, rack, headers, true).await
}

async fn notification_requests_dismiss(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    notification_requests_write(state, rack, headers, false).await
}

async fn notification_requests_write(
    state: WebState,
    rack: RackParameters,
    headers: HeaderMap,
    accept: bool,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_NOTIFICATIONS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Ok(request_ids) = relationship_ids(&rack) else {
        return framework_internal_error();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let result = if accept {
        writer
            .accept_notification_requests(&authenticated, &request_ids)
            .await
    } else {
        writer
            .dismiss_notification_requests(&authenticated, &request_ids)
            .await
    };
    match result {
        Ok(()) => empty_json_response(),
        Err(_) => internal_error(),
    }
}

async fn grouped_notification_clear(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    notification_clear(State(state), headers).await
}

async fn grouped_notification_dismiss(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_NOTIFICATIONS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some(group_key) = uri_path_segment(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .dismiss_notification_group(&authenticated, &group_key)
        .await
    {
        Ok(()) => empty_json_response(),
        Err(_) => internal_error(),
    }
}

async fn filters(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_FILTERS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(filters) = state.loader(Some(owner)).filters().await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = filters
        .iter()
        .map(|filter| serializer.filter(filter))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn lists(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_LISTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(lists) = state.loader(Some(owner)).lists(owner).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = lists
        .iter()
        .map(|list| serializer.list(list))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn list_show(State(state): State<WebState>, uri: Uri, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_LISTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some((_, list_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Ok(lists) = state.loader(Some(owner)).lists(owner).await else {
        return internal_error();
    };
    let Some(list) = lists.into_iter().find(|list| list.id == list_id) else {
        return record_not_found();
    };
    match serde_json::to_vec(&state.serializer().list(&list)) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn list_accounts(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_LISTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some((list_path, list_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let is_owned = match state.loader(Some(owner)).lists(owner).await {
        Ok(lists) => lists.into_iter().any(|list| list.id == list_id),
        Err(_) => return internal_error(),
    };
    if !is_owned {
        return record_not_found();
    }
    let (max_id, since_id) = match cursor_pair(&rack, "max_id", "since_id") {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let Ok(limit) = limit_parameter(&rack, 40, 80) else {
        return framework_internal_error();
    };
    let unlimited = limit == 0;
    let Ok(account_ids) = state
        .repository
        .rest_list_account_ids(
            list_id,
            (!unlimited).then_some(max_id).flatten(),
            (!unlimited).then_some(since_id).flatten(),
            limit,
        )
        .await
    else {
        return internal_error();
    };
    let Ok(accounts) = state.loader(Some(owner)).accounts(&account_ids).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = match accounts
        .iter()
        .map(|account| serializer.account(account))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(values) => match serde_json::to_vec(&values) {
            Ok(values) => values,
            Err(_) => return internal_error(),
        },
        Err(_) => return internal_error(),
    };
    let mut response = json_response(StatusCode::OK, values);
    let parameters = parameters(query.as_deref());
    let route = format!("api/v1/lists/{list_path}/accounts");
    let mut links = Vec::new();
    if limit > 0
        && account_ids.len() == usize::try_from(limit).unwrap_or_default()
        && let Some(last_id) = account_ids.last()
        && let Some(url) =
            pagination_url(&state, &route, &parameters, "max_id", *last_id, &["limit"])
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if !unlimited
        && let Some(first_id) = account_ids.first()
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "since_id",
            *first_id,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn account_lists(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_LISTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some((_, account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let account = match state.loader(Some(owner)).account(account_id).await {
        Ok(Some(account)) => account,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if account.suspended {
        return json_response(StatusCode::OK, b"[]".to_vec());
    }
    let Ok(lists) = state.repository.rest_account_lists(owner, account_id).await else {
        return internal_error();
    };
    let values = lists
        .into_iter()
        .map(|list| {
            state.serializer().list(&ListProjection {
                id: list.id,
                title: list.title,
                replies_policy: list.replies_policy.0,
                exclusive: list.exclusive,
            })
        })
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn account_collections(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    account_collection_index(state, rack, query, uri, headers, false).await
}

async fn account_in_collections(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    account_collection_index(state, rack, query, uri, headers, true).await
}

#[allow(clippy::too_many_lines)]
async fn account_collection_index(
    state: WebState,
    rack: RackParameters,
    query: Option<String>,
    uri: Uri,
    headers: HeaderMap,
    in_collections: bool,
) -> Response<Body> {
    let viewer = if in_collections {
        if !headers.contains_key(AUTHORIZATION) {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "This method requires an authenticated user",
            );
        }
        match required_viewer_owner(&state, &headers, READ_COLLECTIONS).await {
            Ok(viewer) => Some(viewer.account_id()),
            Err(response) => return response,
        }
    } else {
        match optional_viewer(&state, &headers, READ_COLLECTIONS).await {
            Ok(viewer) => viewer,
            Err(response) => return response,
        }
    };
    let Some(account_path) = uri_path_segment(&uri, 4) else {
        return record_not_found();
    };
    let account_id = match route_path_id(&account_path) {
        Some(account_id) => account_id,
        None => match state
            .repository
            .rest_local_account_id_by_username(&account_path)
            .await
        {
            Ok(Some(account_id)) => account_id,
            Ok(None) => return record_not_found(),
            Err(_) => return internal_error(),
        },
    };
    let account = match state.loader(viewer).account(account_id).await {
        Ok(Some(account)) => account,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if in_collections && viewer != Some(account_id) {
        return error_response(StatusCode::FORBIDDEN, "This action is not allowed");
    }
    let account_route_path = account.domain.as_deref().map_or_else(
        || account.username.clone(),
        |domain| format!("{}@{domain}", account.username),
    );
    let offset = match rack.get("offset") {
        None | Some(RackValue::Null) => 0,
        Some(RackValue::Scalar(value)) => ruby_integer(value),
        Some(RackValue::Number(value)) => json_number_integer(value).unwrap_or(0),
        Some(
            RackValue::Boolean(_)
            | RackValue::Array(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => return framework_internal_error(),
    };
    if offset < 0 {
        return framework_internal_error();
    }
    let Ok(limit) = limit_parameter(&rack, 40, if in_collections { 80 } else { 100 }) else {
        return framework_internal_error();
    };
    let query_limit = if limit == 0 {
        0
    } else {
        limit.saturating_add(1)
    };
    let collection_ids = if in_collections {
        state
            .repository
            .rest_account_in_collection_ids(account_id, offset, query_limit)
            .await
    } else {
        state
            .repository
            .rest_account_collection_ids(account_id, viewer, offset, query_limit)
            .await
    };
    let Ok(mut collection_ids) = collection_ids else {
        return internal_error();
    };
    let has_next = limit > 0 && collection_ids.len() > usize::try_from(limit).unwrap_or_default();
    if has_next {
        collection_ids.truncate(usize::try_from(limit).unwrap_or_default());
    }
    let Ok(collections) = state.loader(viewer).collections(&collection_ids).await else {
        return internal_error();
    };
    let route = if in_collections {
        format!("api/v1/accounts/{account_route_path}/in_collections")
    } else {
        format!("api/v1/accounts/{account_route_path}/collections")
    };
    let parameters = parameters(query.as_deref());
    let mut links = Vec::new();
    if has_next
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "offset",
            offset.saturating_add(limit),
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if offset > 0
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "offset",
            offset.saturating_sub(limit),
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    let serializer = state.serializer();
    let Ok(values) = collections
        .iter()
        .map(|collection| serializer.collection(collection))
        .collect::<Result<Vec<_>, _>>()
    else {
        return internal_error();
    };
    collection_index_response(&state, &values, Some(links))
}

fn collection_index_response(
    _state: &WebState,
    values: &[impl serde::Serialize],
    links: Option<Vec<String>>,
) -> Response<Body> {
    let mut response = match serde_json::to_vec(&serde_json::json!({ "collections": values })) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => return internal_error(),
    };
    if let Some(links) = links {
        set_link_header(&mut response, &links);
    }
    response
}

async fn featured_tags(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(tags) = state.loader(Some(owner)).featured_tags(owner).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = tags
        .iter()
        .map(|tag| serializer.featured_tag(tag))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn followed_tags(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_FOLLOWS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let (max_id, min_id, since_id) = match cursor_triplet(&rack) {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = FollowedTagsOptions {
        max_id,
        min_id,
        since_id,
        limit: match limit_parameter(&rack, 100, 200) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let Ok(page) = state
        .loader(Some(owner))
        .followed_tags(owner, &options)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = page
        .tags
        .iter()
        .map(|tag| serializer.tag(tag))
        .collect::<Vec<_>>();
    let Ok(body) = serde_json::to_vec(&values) else {
        return internal_error();
    };
    let mut response = json_response(StatusCode::OK, body);
    let parameters = parameters(query.as_deref());
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| page.tags.len() == limit)
        && let Some(last_id) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            "api/v1/followed_tags",
            &parameters,
            "max_id",
            last_id,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_id) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            "api/v1/followed_tags",
            &parameters,
            "since_id",
            first_id,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn follow_requests(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_FOLLOWS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let (max_id, since_id) = match cursor_pair(&rack, "max_id", "since_id") {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = FollowCollectionOptions {
        max_id,
        since_id,
        limit: match limit_parameter(&rack, 40, 80) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let Ok(page) = state
        .loader(Some(owner))
        .follow_requests(owner, &options)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let accounts = page
        .accounts
        .iter()
        .map(|account| serializer.account(account))
        .collect::<Result<Vec<_>, _>>();
    let mut response = match accounts
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => return internal_error(),
    };
    let parameters = parameters(query.as_deref());
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| page.accounts.len() == limit)
        && let Some(last_cursor) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            "api/v1/follow_requests",
            &parameters,
            "max_id",
            last_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_cursor) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            "api/v1/follow_requests",
            &parameters,
            "since_id",
            first_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn preferences(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer_owner(&state, &headers, READ_ACCOUNTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(Some(preferences)) = state
        .loader(Some(owner.account_id()))
        .preferences(owner.user_id(), owner.account_id())
        .await
    else {
        return internal_error();
    };
    match serde_json::to_vec(&state.serializer().preferences(&preferences)) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn featured_tag_suggestions(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(tags) = state
        .loader(Some(owner))
        .featured_tag_suggestions(owner)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = tags
        .iter()
        .map(|tag| serializer.tag(tag))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn account_featured_tags(
    State(state): State<WebState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if let Err(response) = optional_suspension_guard(&state, &headers).await {
        return response;
    }
    let Some((_, account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let account = match state.repository.account(account_id).await {
        Ok(Some(account)) => account,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    if account.suspended_at.is_some() {
        return json_response(StatusCode::OK, b"[]".to_vec());
    }
    let Ok(tags) = state.loader(None).featured_tags(account_id).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = tags
        .iter()
        .map(|tag| serializer.featured_tag(tag))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn optional_suspension_guard(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<(), Response<Body>> {
    if !headers.contains_key(AUTHORIZATION) {
        return Ok(());
    }
    match state.authenticator.authenticate(headers, NO_SCOPE).await {
        Ok(_) => Ok(()),
        Err(OAuthAuthenticationError::OAuth(error)) => match error {
            OAuthError::UserDisabled => Err(OAuthError::UserDisabled
                .into_http_response()
                .map(Body::from)),
            _ => Ok(()),
        },
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

fn account_response(
    state: &WebState,
    account: sqlx::Result<Option<crate::mastodon::rest::AccountProjection>>,
) -> Response<Body> {
    let account = match account {
        Ok(Some(account)) => account,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .account(&account)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn follow_account(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    account_relationship_write(state, rack, uri, headers, true).await
}

async fn authorize_follow_request(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_FOLLOWS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, source_account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .authorize_follow_request_with_origin(
            &authenticated,
            source_account_id,
            Some(state.origin.as_str()),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return status_saved_write_error(&error),
    };
    relationship_response(&state, &authenticated, source_account_id).await
}

async fn reject_follow_request(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_FOLLOWS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, source_account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .reject_follow_request_with_origin(
            &authenticated,
            source_account_id,
            Some(state.origin.as_str()),
        )
        .await
    {
        return status_saved_write_error(&error);
    }
    relationship_response(&state, &authenticated, source_account_id).await
}

async fn remove_from_followers(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_FOLLOWS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, follower_account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .remove_follower_with_origin(
            &authenticated,
            follower_account_id,
            Some(state.origin.as_str()),
        )
        .await
    {
        return status_saved_write_error(&error);
    }
    relationship_response(&state, &authenticated, follower_account_id).await
}

async fn block_account(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    account_block_write(state, uri, headers, true).await
}

async fn unblock_account(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    account_block_write(state, uri, headers, false).await
}

async fn mute_account(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    account_mute_write(state, rack, uri, headers, true).await
}

async fn unmute_account(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    account_mute_write(state, RackParameters::default(), uri, headers, false).await
}

async fn unfollow_account(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    account_relationship_write(state, RackParameters::default(), uri, headers, false).await
}

async fn account_relationship_write(
    state: WebState,
    rack: RackParameters,
    uri: Uri,
    headers: HeaderMap,
    following: bool,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_FOLLOWS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, target_account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let (reblogs, notify, languages) = if following {
        let languages = match rack.get("languages") {
            None | Some(RackValue::Null) => None,
            Some(RackValue::Scalar(_) | RackValue::Array(_)) => {
                Some(rack_array_values(&rack, "languages"))
            }
            Some(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid languages"),
        };
        (
            optional_boolean_parameter(&rack, "reblogs"),
            optional_boolean_parameter(&rack, "notify"),
            languages,
        )
    } else {
        (None, None, None)
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .set_follow_with_origin(
            &authenticated,
            target_account_id,
            following,
            reblogs,
            notify,
            languages,
            Some(state.origin.as_str()),
            state.instance_runtime.limited_federation,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return status_saved_write_error(&error),
    };
    relationship_response(&state, &authenticated, target_account_id).await
}

async fn account_block_write(
    state: WebState,
    uri: Uri,
    headers: HeaderMap,
    blocking: bool,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_BLOCKS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, target_account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .set_block_with_origin(
            &authenticated,
            target_account_id,
            blocking,
            Some(state.origin.as_str()),
        )
        .await
    {
        return status_saved_write_error(&error);
    }
    relationship_response(&state, &authenticated, target_account_id).await
}

async fn account_mute_write(
    state: WebState,
    rack: RackParameters,
    uri: Uri,
    headers: HeaderMap,
    muting: bool,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_MUTES).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, target_account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let (hide_notifications, duration) = if muting {
        let duration = match integer_parameter(&rack, "duration") {
            Ok(duration) if duration.unwrap_or_default() >= 0 => duration,
            _ => return error_response(StatusCode::BAD_REQUEST, "Invalid duration"),
        };
        (optional_boolean_parameter(&rack, "notifications"), duration)
    } else {
        (None, None)
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .set_mute(
            &authenticated,
            target_account_id,
            muting,
            hide_notifications,
            duration,
        )
        .await
    {
        return status_saved_write_error(&error);
    }
    relationship_response(&state, &authenticated, target_account_id).await
}

async fn relationship_response(
    state: &WebState,
    authenticated: &AuthenticatedBearer,
    target_account_id: i64,
) -> Response<Body> {
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Ok(mut values) = state
        .loader(Some(owner))
        .relationships(&[target_account_id], false)
        .await
    else {
        return internal_error();
    };
    let Some(value) = values.pop() else {
        return record_not_found();
    };
    match serde_json::to_vec(&state.serializer().relationship(&value)) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

// These are explicit disabled collection reads, not a catch-all API fallback.
async fn empty_discovery_read(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    if let Err(response) = optional_viewer_owner(&state, &headers, NO_SCOPE).await {
        return response;
    }
    json_response(StatusCode::OK, b"[]".to_vec())
}

async fn empty_suggestions(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    if let Err(response) = required_viewer(&state, &headers, READ_ACCOUNTS).await {
        return response;
    }
    json_response(StatusCode::OK, b"[]".to_vec())
}

async fn empty_link_timeline(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(response) = timeline_viewer(
        &state,
        &headers,
        READ_STATUSES,
        &requested_feed_options(&rack),
        FeedKind::Topic,
    )
    .await
    {
        return response;
    }
    // Link discovery is disabled: unlike a real preview-card lookup, no resource
    // was looked up and found missing. Do not fabricate a preview card or status.
    json_response(StatusCode::OK, b"[]".to_vec())
}

async fn empty_familiar_followers(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(response) = required_viewer(&state, &headers, READ_FAMILIAR_FOLLOWERS).await {
        return response;
    }
    let Ok(ids) = relationship_ids(&rack) else {
        return framework_internal_error();
    };
    // The frontend caches by requested account ID; [] alone leaves it unresolved.
    // These are empty relationship placeholders, not proof that an account exists.
    let mut seen = std::collections::BTreeSet::new();
    let values = ids
        .into_iter()
        .filter(|id| seen.insert(*id))
        .map(|id| serde_json::json!({"id": id.to_string(), "accounts": []}))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn domain_blocks(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_BLOCKS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let (max_id, since_id) = match cursor_pair(&rack, "max_id", "since_id") {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let Ok(limit) =
        limit_parameter(&rack, 100, 200).and_then(|limit| usize::try_from(limit).map_err(|_| ()))
    else {
        return framework_internal_error();
    };
    let Ok(blocks) = state.repository.account_domain_blocks(owner).await else {
        return internal_error();
    };
    // The existing repository returns ascending IDs. Mastodon's max-ID pages are descending.
    let blocks = blocks
        .into_iter()
        .rev()
        .filter(|block| {
            max_id.is_none_or(|id| block.id < id) && since_id.is_none_or(|id| block.id > id)
        })
        .take(limit)
        .collect::<Vec<_>>();
    let values = blocks.iter().map(|block| &block.domain).collect::<Vec<_>>();
    let Ok(body) = serde_json::to_vec(&values) else {
        return internal_error();
    };
    let mut response = json_response(StatusCode::OK, body);
    let parameters = parameters(query.as_deref());
    let mut links = Vec::new();
    for (cursor, relation, block) in [
        (
            "max_id",
            "next",
            blocks.last().filter(|_| blocks.len() == limit),
        ),
        ("since_id", "prev", blocks.first()),
    ] {
        if let Some(block) = block
            && let Some(url) = pagination_url(
                &state,
                "api/v1/domain_blocks",
                &parameters,
                cursor,
                block.id,
                &["limit"],
            )
        {
            links.push(format!("<{url}>; rel=\"{relation}\""));
        }
    }
    set_link_header(&mut response, &links);
    response
}

async fn instance_domain_blocks(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match optional_viewer_owner(&state, &headers, NO_SCOPE).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(user_eligible) = instance_block_list_user_eligible(&state, &headers, owner).await else {
        return internal_error();
    };
    let Ok(settings) = state.repository.settings().await else {
        return internal_error();
    };
    let visible = |name| {
        settings
            .iter()
            .find(|setting| setting.var == name)
            .and_then(|setting| setting.value.as_ref())
            .and_then(|value| yaml_scalar(value.raw()))
            .is_some_and(|value| value == "all" || (value == "users" && user_eligible))
    };
    // Hidden publishing is an explicit disabled response, not an empty moderation database.
    if !visible("show_domain_blocks") {
        return json_response(StatusCode::OK, b"[]".to_vec());
    }
    let Ok(mut blocks) = state.repository.domain_blocks().await else {
        return internal_error();
    };
    blocks.retain(|block| {
        block
            .severity
            .is_some_and(|severity| matches!(severity.0, 0 | 1))
    });
    blocks.sort_by(|left, right| {
        (left.severity.map(|v| v.0), &left.domain)
            .cmp(&(right.severity.map(|v| v.0), &right.domain))
    });
    let with_comment = visible("show_domain_blocks_rationale");
    let values = blocks
        .iter()
        .map(|block| public_domain_block(block, with_comment))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

// Publishing uses User#functional_or_moved?, not the stricter require_user!.
// Reuse the OAuth facts query, avoiding Repository::user's OTP decryption.
async fn instance_block_list_user_eligible(
    state: &WebState,
    headers: &HeaderMap,
    owner: Option<OAuthResourceOwner>,
) -> sqlx::Result<bool> {
    let Some(owner) = owner else {
        return Ok(false);
    };
    // The optional authenticator above already validated this bearer header.
    let Some(token) = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.get(7..))
    else {
        return Ok(false);
    };
    let Some(user) = state.repository.oauth_bearer_candidate(token).await? else {
        return Ok(false);
    };
    Ok(user.user_id == Some(owner.user_id())
        && user.account_id == Some(owner.account_id())
        && user.confirmed_at.is_some()
        && user.approved == Some(true)
        && user.disabled == Some(false)
        && user.suspended_at.is_none()
        && user.memorial == Some(false)
        && (user.role_requires_2fa != Some(true)
            || user.otp_required_for_login == Some(true)
            || user.has_webauthn_credentials))
}

fn public_domain_block(
    block: &crate::mastodon::DomainBlock,
    with_comment: bool,
) -> serde_json::Value {
    let length = block.domain.chars().count();
    let visible = length / 4;
    let domain = block
        .domain
        .chars()
        .enumerate()
        .map(|(index, ch)| {
            if block.obfuscate && index > visible && index < length - visible && ch != '.' {
                '*'
            } else {
                ch
            }
        })
        .collect::<String>();
    serde_json::json!({
        "domain": domain,
        "digest": format!("{:x}", Sha256::digest(block.domain.as_bytes())),
        "severity": if block.severity.is_some_and(|severity| severity.0 == 1) { "suspend" } else { "silence" },
        "comment": if with_comment { block.public_comment.as_deref() } else { None },
    })
}

async fn relationships(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_FOLLOWS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(ids) = relationship_ids(&rack) else {
        return framework_internal_error();
    };
    let with_suspended = boolean_parameter(&rack, "with_suspended");
    let Ok(values) = state
        .loader(Some(owner))
        .relationships(&ids, with_suspended)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = values
        .iter()
        .map(|value| serializer.relationship(value))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&values) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn profile(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer_owner(&state, &headers, VERIFY_CREDENTIALS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let loader = state.loader(Some(owner.account_id()));
    let credential = match loader
        .credential_account(owner.user_id(), owner.account_id())
        .await
    {
        Ok(Some(credential)) => credential,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    let Ok(tags) = loader.featured_tags(owner.account_id()).await else {
        return internal_error();
    };
    match state
        .serializer()
        .profile(&credential, &tags)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn verify_credentials(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    let owner = match required_viewer_owner(&state, &headers, VERIFY_CREDENTIALS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    if let Some(writer) = state.write_repository.as_ref()
        && writer
            .track_interactive_user(owner.user_id())
            .await
            .is_err()
    {
        return internal_error();
    }
    credential_account_response(&state, owner).await
}

#[allow(clippy::too_many_lines)]
async fn update_credentials(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_ACCOUNTS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner,
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let account_id = owner.account_id();
    let mut update = match account_profile_update(&rack) {
        Ok(update) => update,
        Err(field) => return error_response(StatusCode::BAD_REQUEST, field),
    };
    let avatar = match profile_media_change(
        &rack,
        "avatar",
        PaperclipAttachment::AccountAvatar,
        owner.account_id(),
    ) {
        Ok(change) => change,
        Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, error),
    };
    let header = match profile_media_change(
        &rack,
        "header",
        PaperclipAttachment::AccountHeader,
        owner.account_id(),
    ) {
        Ok(change) => change,
        Err(error) => return error_response(StatusCode::UNPROCESSABLE_ENTITY, error),
    };
    update.avatar = avatar.update;
    update.header = header.update;
    let result = writer
        .with_account_lock(account_id, || async {
            writer.ensure_account_write_allowed(account_id).await?;
            let old_avatar = state
                .repository
                .paperclip_metadata(PaperclipAttachment::AccountAvatar, account_id)
                .await?;
            let old_header = state
                .repository
                .paperclip_metadata(PaperclipAttachment::AccountHeader, account_id)
                .await?;
            let avatar_paths = write_prepared_profile_media(
                &state.media_root,
                PaperclipAttachment::AccountAvatar,
                account_id,
                avatar.prepared.as_ref(),
            )
            .map_err(WriteError::Filesystem)?;
            let header_paths = match write_prepared_profile_media(
                &state.media_root,
                PaperclipAttachment::AccountHeader,
                account_id,
                header.prepared.as_ref(),
            ) {
                Ok(paths) => paths,
                Err(error) => {
                    remove_written_profile_media(
                        &state.media_root,
                        &avatar_paths,
                        old_avatar.as_ref(),
                    );
                    return Err(WriteError::Filesystem(error));
                }
            };
            if let Err(error) = writer
                .update_account_profile_locked(&authenticated, &update)
                .await
            {
                remove_written_profile_media(&state.media_root, &avatar_paths, old_avatar.as_ref());
                remove_written_profile_media(&state.media_root, &header_paths, old_header.as_ref());
                return Err(error);
            }
            if !matches!(&update.avatar, AccountMediaUpdate::Unchanged) {
                remove_replaced_profile_media(
                    &state.media_root,
                    old_avatar.as_ref(),
                    &avatar_paths,
                );
            }
            if !matches!(&update.header, AccountMediaUpdate::Unchanged) {
                remove_replaced_profile_media(
                    &state.media_root,
                    old_header.as_ref(),
                    &header_paths,
                );
            }
            Ok(())
        })
        .await;
    if let Err(error) = result {
        return status_saved_write_error(&error);
    }
    credential_account_response(&state, owner).await
}

async fn delete_profile_avatar(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    delete_profile_media(state, headers, PaperclipAttachment::AccountAvatar).await
}

async fn delete_profile_header(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    delete_profile_media(state, headers, PaperclipAttachment::AccountHeader).await
}

async fn delete_profile_media(
    state: WebState,
    headers: HeaderMap,
    attachment: PaperclipAttachment,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_ACCOUNTS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner,
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let mut update = AccountProfileUpdate::default();
    match attachment {
        PaperclipAttachment::AccountAvatar => update.avatar = AccountMediaUpdate::Remove,
        PaperclipAttachment::AccountHeader => update.header = AccountMediaUpdate::Remove,
        _ => return internal_error(),
    }
    let account_id = owner.account_id();
    let result = writer
        .with_account_lock(account_id, || async {
            writer.ensure_account_write_allowed(account_id).await?;
            let old = state
                .repository
                .paperclip_metadata(attachment, account_id)
                .await?;
            writer
                .update_account_profile_locked(&authenticated, &update)
                .await?;
            remove_replaced_profile_media(&state.media_root, old.as_ref(), &[]);
            Ok(())
        })
        .await;
    if let Err(error) = result {
        return status_saved_write_error(&error);
    }
    credential_account_response(&state, owner).await
}

async fn credential_account_response(
    state: &WebState,
    owner: OAuthResourceOwner,
) -> Response<Body> {
    let credential = match state
        .loader(Some(owner.account_id()))
        .credential_account(owner.user_id(), owner.account_id())
        .await
    {
        Ok(Some(credential)) => credential,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .credential_account(&credential)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

fn account_profile_update(rack: &RackParameters) -> Result<AccountProfileUpdate, &'static str> {
    let display_name = profile_text_parameter(rack, "display_name")?;
    let note = profile_text_parameter(rack, "note")?;
    let avatar_description = profile_text_parameter(rack, "avatar_description")?;
    let header_description = profile_text_parameter(rack, "header_description")?;
    let attribution_domains = profile_string_array(rack, "attribution_domains")?;
    let fields = profile_fields_parameter(rack)?;
    let source = profile_source_parameter(rack)?;
    Ok(AccountProfileUpdate {
        display_name,
        note,
        avatar_description,
        header_description,
        avatar: AccountMediaUpdate::Unchanged,
        header: AccountMediaUpdate::Unchanged,
        bot: nullable_boolean_parameter(rack, "bot"),
        locked: optional_boolean_parameter(rack, "locked"),
        discoverable: nullable_boolean_parameter(rack, "discoverable"),
        hide_collections: nullable_boolean_parameter(rack, "hide_collections"),
        indexable: optional_boolean_parameter(rack, "indexable"),
        attribution_domains,
        fields,
        source,
    })
}

struct ProfileMediaChange {
    update: AccountMediaUpdate,
    prepared: Option<PreparedAccountMedia>,
}

fn profile_media_change(
    parameters: &RackParameters,
    name: &str,
    attachment: PaperclipAttachment,
    account_id: i64,
) -> Result<ProfileMediaChange, &'static str> {
    match parameters.get(name) {
        None => Ok(ProfileMediaChange {
            update: AccountMediaUpdate::Unchanged,
            prepared: None,
        }),
        Some(RackValue::Null) => Ok(ProfileMediaChange {
            update: AccountMediaUpdate::Remove,
            prepared: None,
        }),
        Some(RackValue::Upload(upload)) => {
            let prepared = prepare_account_media(
                attachment,
                account_id,
                &upload.file_name,
                &upload.content_type,
                &upload.bytes,
            )
            .map_err(|_| "Invalid account image")?;
            Ok(ProfileMediaChange {
                update: AccountMediaUpdate::Replace {
                    file_name: prepared.file_name.clone(),
                    content_type: prepared.content_type.clone(),
                    file_size: prepared.file_size,
                    storage_schema_version: 1,
                },
                prepared: Some(prepared),
            })
        }
        Some(_) => Err("Invalid account image"),
    }
}

fn write_prepared_profile_media(
    root: &PaperclipRoot,
    attachment: PaperclipAttachment,
    account_id: i64,
    prepared: Option<&PreparedAccountMedia>,
) -> std::io::Result<Vec<String>> {
    let Some(prepared) = prepared else {
        return Ok(Vec::new());
    };
    let metadata = PaperclipMetadata {
        attachment,
        id: account_id,
        remote: false,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let original = metadata.relative_path("original").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid media path")
    })?;
    let mut paths = vec![original];
    root.write_file(FsPath::new(&paths[0]), &prepared.original_bytes)?;
    if let Some(static_bytes) = prepared.static_bytes.as_ref() {
        let Some(static_path) = metadata.relative_path("static") else {
            let _ = root.remove_file(FsPath::new(&paths[0]));
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid static media path",
            ));
        };
        if let Err(error) = root.write_file(FsPath::new(&static_path), static_bytes) {
            let _ = root.remove_file(FsPath::new(&paths[0]));
            return Err(error);
        }
        paths.push(static_path);
    }
    Ok(paths)
}

fn remove_written_profile_media(
    root: &PaperclipRoot,
    paths: &[String],
    protected: Option<&PaperclipMetadata>,
) {
    for path in paths {
        if protected.is_some_and(|metadata| {
            ["original", "static"].iter().any(|style| {
                metadata
                    .relative_path(style)
                    .is_some_and(|old_path| old_path == *path)
            })
        }) {
            continue;
        }
        let _ = root.remove_file(FsPath::new(path));
    }
}

fn remove_replaced_profile_media(
    root: &PaperclipRoot,
    old: Option<&PaperclipMetadata>,
    new_paths: &[String],
) {
    let Some(old) = old else {
        return;
    };
    for style in ["original", "static"] {
        let Some(path) = old.relative_path(style) else {
            continue;
        };
        if !new_paths.iter().any(|new_path| new_path == &path) {
            let _ = root.remove_file(FsPath::new(&path));
        }
    }
}

fn profile_text_parameter(
    parameters: &RackParameters,
    name: &str,
) -> Result<Option<String>, &'static str> {
    match parameters.get(name) {
        None => Ok(None),
        Some(RackValue::Null) => Err("Invalid profile text"),
        Some(value) => profile_scalar_string(value)
            .map(Some)
            .map_err(|()| "Invalid profile text"),
    }
}

fn profile_string_array(
    parameters: &RackParameters,
    name: &str,
) -> Result<Option<Vec<String>>, &'static str> {
    let Some(value) = parameters.get(name) else {
        return Ok(None);
    };
    let RackValue::Array(values) = value else {
        return Err("Invalid profile domain list");
    };
    values
        .iter()
        .map(|value| match value {
            RackValue::Null => Ok(String::new()),
            value => profile_scalar_string(value).map_err(|()| "Invalid profile domain list"),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn profile_fields_parameter(
    parameters: &RackParameters,
) -> Result<Option<Vec<AccountFieldUpdate>>, &'static str> {
    let Some(value) = parameters.get("fields_attributes") else {
        return Ok(None);
    };
    let values = match value {
        RackValue::Null => Vec::new(),
        RackValue::Array(values) => values.iter().collect::<Vec<_>>(),
        RackValue::Object(values)
            if values.contains_key("name") || values.contains_key("value") =>
        {
            vec![value]
        }
        RackValue::Object(values) => values.values().collect::<Vec<_>>(),
        _ => return Err("Invalid profile fields"),
    };
    values
        .into_iter()
        .map(profile_field)
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn profile_field(value: &RackValue) -> Result<AccountFieldUpdate, &'static str> {
    let RackValue::Object(values) = value else {
        return Err("Invalid profile field");
    };
    let name = profile_object_string(values.get("name"))?;
    let value = profile_object_string(values.get("value"))?;
    Ok(AccountFieldUpdate { name, value })
}

fn profile_source_parameter(
    parameters: &RackParameters,
) -> Result<Option<AccountSourceUpdate>, &'static str> {
    let Some(value) = parameters.get("source") else {
        return Ok(None);
    };
    let RackValue::Object(values) = value else {
        return if matches!(value, RackValue::Null) {
            Ok(None)
        } else {
            Err("Invalid profile source")
        };
    };
    Ok(Some(AccountSourceUpdate {
        privacy: profile_object_nullable_string(values.get("privacy"))?,
        sensitive: profile_object_boolean(values.get("sensitive"))?,
        language: profile_object_nullable_string(values.get("language"))?,
        quote_policy: profile_object_nullable_string(values.get("quote_policy"))?,
    }))
}

fn profile_object_string(value: Option<&RackValue>) -> Result<String, &'static str> {
    match value {
        None | Some(RackValue::Null) => Ok(String::new()),
        Some(value) => profile_scalar_string(value).map_err(|()| "Invalid profile field"),
    }
}

fn profile_object_nullable_string(
    value: Option<&RackValue>,
) -> Result<AccountProfileValue<String>, &'static str> {
    match value {
        None => Ok(AccountProfileValue::Unchanged),
        Some(RackValue::Null) => Ok(AccountProfileValue::Null),
        Some(value) => profile_scalar_string(value)
            .map(AccountProfileValue::Value)
            .map_err(|()| "Invalid profile source"),
    }
}

fn profile_object_boolean(
    value: Option<&RackValue>,
) -> Result<AccountProfileValue<bool>, &'static str> {
    match value {
        None => Ok(AccountProfileValue::Unchanged),
        Some(RackValue::Null) => Ok(AccountProfileValue::Null),
        Some(RackValue::Array(_) | RackValue::Object(_) | RackValue::Upload(_)) => {
            Err("Invalid profile source")
        }
        Some(value) => Ok(AccountProfileValue::Value(boolean_value(value))),
    }
}

fn profile_scalar_string(value: &RackValue) -> Result<String, ()> {
    match value {
        RackValue::Scalar(value) => Ok(value.clone()),
        RackValue::Number(value) => Ok(ruby_json_number(value)),
        RackValue::Boolean(value) => Ok(value.to_string()),
        RackValue::Null | RackValue::Array(_) | RackValue::Object(_) | RackValue::Upload(_) => {
            Err(())
        }
    }
}

fn boolean_value(value: &RackValue) -> bool {
    match value {
        RackValue::Null => false,
        RackValue::Scalar(value) => {
            !value.is_empty()
                && !matches!(
                    value.as_str(),
                    "0" | "f" | "F" | "false" | "FALSE" | "off" | "OFF"
                )
                && !value.parse::<f64>().is_ok_and(|value| value == 0.0)
        }
        RackValue::Number(value) => {
            if value.is_i64() {
                value.as_i64().is_some_and(|value| value != 0)
            } else if value.is_u64() {
                value.as_u64().is_some_and(|value| value != 0)
            } else {
                true
            }
        }
        RackValue::Array(_) | RackValue::Object(_) | RackValue::Upload(_) => true,
        RackValue::Boolean(value) => *value,
    }
}

fn nullable_boolean_parameter(
    parameters: &RackParameters,
    name: &str,
) -> AccountProfileValue<bool> {
    match parameters.get(name) {
        None => AccountProfileValue::Unchanged,
        Some(RackValue::Null) => AccountProfileValue::Null,
        Some(value) => AccountProfileValue::Value(boolean_value(value)),
    }
}

async fn account_statuses(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((account_path, account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let loader = state.loader(viewer);
    match loader.account(account_id).await {
        Ok(Some(account)) if account.suspended => return statuses_response(&state, &[]),
        Ok(Some(_)) => {}
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    let parameters = parameters(query.as_deref());
    let (max_id, min_id, since_id) = match cursor_triplet(&rack) {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = AccountStatusesOptions {
        max_id,
        min_id,
        since_id,
        limit: match limit_parameter(&rack, 20, 40) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
        pinned: boolean_parameter(&rack, "pinned"),
        tagged: tagged_parameter(&rack),
        only_media: boolean_parameter(&rack, "only_media"),
        exclude_replies: boolean_parameter(&rack, "exclude_replies"),
        exclude_reblogs: boolean_parameter(&rack, "exclude_reblogs"),
        exclude_direct: boolean_parameter(&rack, "exclude_direct"),
    };
    let Ok(statuses) = loader.account_statuses(account_id, &options).await else {
        return internal_error();
    };
    let first_id = statuses.first().map(|status| status.id);
    let last_id = statuses.last().map(|status| status.id);
    let mut response = statuses_response(&state, &statuses);
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| statuses.len() == limit)
        && let Some(last_id) = last_id
        && let Some(url) = pagination_url(
            &state,
            &format!("api/v1/accounts/{account_path}/statuses"),
            &parameters,
            "max_id",
            last_id,
            &[
                "limit",
                "pinned",
                "tagged",
                "only_media",
                "exclude_replies",
                "exclude_reblogs",
            ],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_id) = first_id
        && let Some(url) = pagination_url(
            &state,
            &format!("api/v1/accounts/{account_path}/statuses"),
            &parameters,
            "min_id",
            first_id,
            &[
                "limit",
                "pinned",
                "tagged",
                "only_media",
                "exclude_replies",
                "exclude_reblogs",
            ],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn account_followers(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    uri: Uri,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    account_follows(
        state,
        rack,
        uri,
        query,
        headers,
        FollowCollectionKind::Followers,
    )
    .await
}

async fn account_following(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    uri: Uri,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    account_follows(
        state,
        rack,
        uri,
        query,
        headers,
        FollowCollectionKind::Following,
    )
    .await
}

async fn account_follows(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    kind: FollowCollectionKind,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_ACCOUNTS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((account_path, account_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let loader = state.loader(viewer);
    match loader.account(account_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    match loader.follow_collection_hidden(account_id).await {
        Ok(true) => return json_response(StatusCode::OK, b"[]".to_vec()),
        Ok(false) => {}
        Err(_) => return internal_error(),
    }
    let parameters = parameters(query.as_deref());
    let (max_id, since_id) = match cursor_pair(&rack, "max_id", "since_id") {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = FollowCollectionOptions {
        max_id,
        since_id,
        limit: match limit_parameter(&rack, 40, 80) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let Ok(page) = loader.follow_collection(account_id, kind, &options).await else {
        return internal_error();
    };
    let serializer = state.serializer();
    let accounts = page
        .accounts
        .iter()
        .map(|account| serializer.account(account))
        .collect::<Result<Vec<_>, _>>();
    let mut response = match accounts
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    };
    let route = match kind {
        FollowCollectionKind::Followers => format!("api/v1/accounts/{account_path}/followers"),
        FollowCollectionKind::Following => format!("api/v1/accounts/{account_path}/following"),
    };
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| page.accounts.len() == limit)
        && let Some(last_cursor) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "max_id",
            last_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_cursor) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "since_id",
            first_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn status_history(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_scope_owner(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let history = match state.loader(viewer).status_history(status_id).await {
        Ok(Some(history)) => history,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    let values = history
        .iter()
        .map(|edit| state.serializer().status_edit(edit))
        .collect::<Result<Vec<_>, _>>();
    match values
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn status_source(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match required_scope_owner(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let status = match state.loader(viewer).authorized_status(status_id).await {
        Ok(Some(status)) => status,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    match serde_json::to_vec(&state.serializer().status_source(&status)) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

async fn status_interaction_policy_update(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_STATUSES).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let viewer = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Ok(Some(status_id)) = strict_optional_id_parameter(&rack, "id") else {
        return record_not_found();
    };
    let quote_approval_policy = match rack.get("quote_approval_policy") {
        None | Some(RackValue::Null) => None,
        Some(RackValue::Scalar(value)) if value.trim().is_empty() => None,
        Some(RackValue::Scalar(value)) => Some(value.as_str()),
        Some(_) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Validation failed: Quote approval policy is invalid",
            );
        }
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let outcome = match writer
        .update_status_interaction_policy(&authenticated, status_id, quote_approval_policy)
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return status_saved_write_error(&error),
    };
    let status = match state
        .loader(Some(viewer))
        .authorized_status(outcome.status_id)
        .await
    {
        Ok(Some(status)) => status,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state.serializer().status(&status, StatusShape::Full) {
        Ok(value) => json_response(
            StatusCode::OK,
            serde_json::to_vec(&value).expect("status response is serializable"),
        ),
        Err(_) => internal_error(),
    }
}

async fn revoke_quote(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_STATUSES).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let viewer = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Ok(Some(quoted_status_id)) = strict_optional_id_parameter(&rack, "quoted_status_id") else {
        return record_not_found();
    };
    let Ok(Some(quoting_status_id)) = strict_optional_id_parameter(&rack, "id") else {
        return record_not_found();
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let outcome = match writer
        .revoke_quote(
            &authenticated,
            quoted_status_id,
            quoting_status_id,
            state.origin.as_str(),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return status_saved_write_error(&error),
    };
    let status = match state
        .loader(Some(viewer))
        .authorized_status(outcome.status_id)
        .await
    {
        Ok(Some(status)) => status,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state.serializer().status(&status, StatusShape::Full) {
        Ok(value) => json_response(
            StatusCode::OK,
            serde_json::to_vec(&value).expect("status response is serializable"),
        ),
        Err(_) => internal_error(),
    }
}

async fn status_quotes(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match required_scope_owner(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((status_path, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let (max_id, since_id) = match cursor_pair(&rack, "max_id", "since_id") {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = FollowCollectionOptions {
        max_id,
        since_id,
        limit: match limit_parameter(&rack, 20, 40) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let page = match state
        .loader(viewer)
        .status_quotes(status_id, &options)
        .await
    {
        Ok(Some(page)) => page,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    let mut response = statuses_response(&state, &page.statuses);
    let parameters = parameters(query.as_deref());
    let route = format!("api/v1/statuses/{status_path}/quotes");
    let mut links = Vec::new();
    if page.records_continue
        && let Some(last_cursor) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "max_id",
            last_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if !page.statuses.is_empty()
        && let Some(first_cursor) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "since_id",
            first_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn favourited_by(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_account_association(state, rack, query, uri, headers, false).await
}

async fn reblogged_by(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_account_association(state, rack, query, uri, headers, true).await
}

async fn status_account_association(
    state: WebState,
    rack: RackParameters,
    query: Option<String>,
    uri: Uri,
    headers: HeaderMap,
    reblogs: bool,
) -> Response<Body> {
    let viewer = match optional_scope_owner(&state, &headers, READ_ACCOUNTS).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((status_path, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let parameters = parameters(query.as_deref());
    let (max_id, since_id) = match cursor_pair(&rack, "max_id", "since_id") {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = FollowCollectionOptions {
        max_id,
        since_id,
        limit: match limit_parameter(&rack, 40, 80) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let page = match if reblogs {
        state.loader(viewer).reblogged_by(status_id, &options).await
    } else {
        state
            .loader(viewer)
            .favourited_by(status_id, &options)
            .await
    } {
        Ok(Some(page)) => page,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    let serializer = state.serializer();
    let accounts = page
        .accounts
        .iter()
        .map(|account| serializer.account(account))
        .collect::<Result<Vec<_>, _>>();
    let mut response = match accounts
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    };
    let suffix = if reblogs {
        "reblogged_by"
    } else {
        "favourited_by"
    };
    let route = format!("api/v1/statuses/{status_path}/{suffix}");
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| page.accounts.len() == limit)
        && let Some(last_cursor) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "max_id",
            last_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_cursor) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            &route,
            &parameters,
            "since_id",
            first_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn reblog_status(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_reblog_write(state, rack, uri, headers, true).await
}

async fn unreblog_status(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_reblog_write(state, rack, uri, headers, false).await
}

#[allow(clippy::too_many_lines)]
async fn status_reblog_write(
    state: WebState,
    rack: RackParameters,
    uri: Uri,
    headers: HeaderMap,
    enabled: bool,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_STATUSES).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    if enabled {
        match state.loader(Some(owner)).authorized_status(status_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return record_not_found(),
            Err(_) => return internal_error(),
        }
    }
    let visibility = if enabled {
        match rack.get("visibility") {
            None | Some(RackValue::Null) => None,
            Some(RackValue::Scalar(value)) => Some(value.as_str()),
            Some(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid visibility"),
        }
    } else {
        None
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let outcome = match writer
        .set_reblog_with_origin(
            &authenticated,
            status_id,
            visibility,
            enabled,
            Some(state.origin.as_str()),
            state.instance_runtime.limited_federation,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return status_saved_write_error(&error),
    };
    if !enabled && !outcome.removed {
        match state.loader(Some(owner)).authorized_status(status_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return record_not_found(),
            Err(_) => return internal_error(),
        }
    }
    let response_status_id = if enabled {
        outcome.status_id
    } else {
        outcome.target_status_id
    };
    let loaded_status = if !enabled && outcome.removed {
        state
            .loader(Some(owner))
            .status_without_authorization(response_status_id)
            .await
    } else {
        state
            .loader(Some(owner))
            .authorized_status(response_status_id)
            .await
    };
    let status = match loaded_status {
        Ok(Some(status)) => status,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    let mut status = status;
    if !enabled
        && outcome.removed
        && status.account.id == owner
        && let Some(statuses_count) = outcome.account_statuses_count_before_removal
    {
        // Rails renders this response before its asynchronous removal worker
        // decrements the reblogger's account counter.
        status.account.statuses_count = statuses_count;
    }
    let status = if !enabled && !outcome.removed {
        status.without_status_relationships()
    } else {
        status
    };
    let body = state
        .serializer()
        .status(&status, StatusShape::Full)
        .ok()
        .and_then(|value| {
            let mut value = serde_json::to_value(value).ok()?;
            if enabled {
                value
                    .as_object_mut()?
                    .insert("reblogged".to_owned(), serde_json::Value::Bool(true));
            }
            serde_json::to_vec(&value).ok()
        });
    match body {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn bookmark_status(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_saved_write(state, uri, headers, false, true).await
}

async fn unbookmark_status(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_saved_write(state, uri, headers, false, false).await
}

async fn favourite_status(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_saved_write(state, uri, headers, true, true).await
}

async fn unfavourite_status(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_saved_write(state, uri, headers, true, false).await
}

async fn mute_status(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_mute_write(state, uri, headers, true).await
}

async fn unmute_status(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_mute_write(state, uri, headers, false).await
}

async fn pin_status(State(state): State<WebState>, uri: Uri, headers: HeaderMap) -> Response<Body> {
    status_pin_write(state, uri, headers, true).await
}

async fn unpin_status(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    status_pin_write(state, uri, headers, false).await
}

async fn status_mute_write(
    state: WebState,
    uri: Uri,
    headers: HeaderMap,
    muted: bool,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_MUTES).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    match state.loader(Some(owner)).authorized_status(status_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .set_status_mute(&authenticated, status_id, muted)
        .await
    {
        return status_saved_write_error(&error);
    }
    let status = match state.loader(Some(owner)).authorized_status(status_id).await {
        Ok(Some(status)) => status,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .status(&status, StatusShape::Full)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn status_pin_write(
    state: WebState,
    uri: Uri,
    headers: HeaderMap,
    pinned: bool,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_ACCOUNTS).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    match state.loader(Some(owner)).authorized_status(status_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .set_status_pin(&authenticated, status_id, pinned)
        .await
    {
        return status_saved_write_error(&error);
    }
    let status = match state.loader(Some(owner)).authorized_status(status_id).await {
        Ok(Some(status)) => status,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .status(&status, StatusShape::Full)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn status_saved_write(
    state: WebState,
    uri: Uri,
    headers: HeaderMap,
    favourite: bool,
    enabled: bool,
) -> Response<Body> {
    let scopes = if favourite {
        WRITE_FAVOURITES
    } else {
        WRITE_BOOKMARKS
    };
    let authenticated = match required_write_viewer(&state, &headers, scopes).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    if enabled {
        match state.loader(Some(owner)).authorized_status(status_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return record_not_found(),
            Err(_) => return internal_error(),
        }
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let (response_status_id, removed) = if favourite {
        let outcome = match writer
            .set_favourite_with_origin(
                &authenticated,
                status_id,
                enabled,
                Some(state.origin.as_str()),
                state.instance_runtime.limited_federation,
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => return status_saved_write_error(&error),
        };
        (
            if !enabled && outcome.activity_id.is_some() {
                outcome.status_id
            } else {
                status_id
            },
            !enabled && outcome.activity_id.is_some(),
        )
    } else {
        let outcome = match writer
            .set_bookmark(&authenticated, status_id, enabled)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => return status_saved_write_error(&error),
        };
        (
            if !enabled && outcome.removed {
                outcome.status_id
            } else {
                status_id
            },
            !enabled && outcome.removed,
        )
    };
    if !enabled && !removed {
        match state.loader(Some(owner)).authorized_status(status_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return record_not_found(),
            Err(_) => return internal_error(),
        }
    }
    let status = match if !enabled && removed {
        state
            .loader(Some(owner))
            .status_without_authorization(response_status_id)
            .await
    } else {
        state
            .loader(Some(owner))
            .authorized_status(response_status_id)
            .await
    } {
        Ok(Some(status)) => status,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .status(&status, StatusShape::Full)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

fn status_saved_write_error(error: &WriteError) -> Response<Body> {
    match error {
        WriteError::NotFound => record_not_found(),
        WriteError::Unauthorized => error_response(StatusCode::UNAUTHORIZED, "Unauthorized"),
        WriteError::Forbidden => {
            error_response(StatusCode::FORBIDDEN, "This action is not allowed")
        }
        WriteError::InvalidInput(message) => error_response(StatusCode::BAD_REQUEST, message),
        WriteError::Validation(_) => {
            error_response(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string())
        }
        WriteError::Conflict => error_response(
            StatusCode::CONFLICT,
            "Conflict during update, please try again",
        ),
        WriteError::RateLimited
        | WriteError::Sqlx(_)
        | WriteError::Job(_)
        | WriteError::Filesystem(_) => internal_error(),
    }
}

fn remote_poll_refresh_document_is_supported(object: &serde_json::Value, status_uri: &str) -> bool {
    supported_activitypub_context(object.get("@context"))
        && object.get("id").and_then(serde_json::Value::as_str) == Some(status_uri)
        && (equals_or_includes(object.get("type"), "Question")
            || equals_or_includes(object.get("type"), "Note"))
}

async fn refresh_remote_poll(state: &WebState, poll_id: i64, viewer_account_id: i64) -> bool {
    let Ok(Some((
        author_id,
        actor_uri,
        status_uri,
        last_fetched_at,
        expires_at,
        expected_lock_version,
    ))) = state.repository.remote_poll_refresh_target(poll_id).await
    else {
        return false;
    };
    let now = Utc::now().naive_utc();
    if last_fetched_at.is_some_and(|last| {
        last >= now - ChronoDuration::minutes(1)
            || expires_at.is_some_and(|expires_at| last >= expires_at)
    }) {
        return false;
    }
    let Ok(target) = Url::parse(&status_uri) else {
        return false;
    };
    let Ok(domain) = canonical_remote_domain_from_url(&target) else {
        return false;
    };
    if !state
        .repository
        .remote_domain_allowed(&domain, state.instance_runtime.limited_federation)
        .await
        .unwrap_or(false)
    {
        return false;
    }
    let Ok(Some(signer_account)) = state.repository.account(viewer_account_id).await else {
        return false;
    };
    if signer_account.domain.is_some() {
        return false;
    }
    let Some(private_key) = signer_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
    else {
        return false;
    };
    let key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&state.origin, &signer_account)
    );
    let signer = HttpSignatureSigner {
        key_id: &key_id,
        private_key_pem: private_key.as_str(),
    };
    let Ok(response) = state
        .remote_fetcher
        .get_signed(
            target,
            &[
                ACTIVITY_JSON,
                "application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\"",
            ],
            &signer,
        )
        .await
    else {
        return false;
    };
    let Ok(object) = serde_json::from_slice::<serde_json::Value>(&response.body) else {
        return false;
    };
    if !remote_poll_refresh_document_is_supported(&object, &status_uri) {
        return false;
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return false;
    };
    writer
        .apply_signed_remote_poll_refresh(
            author_id,
            &actor_uri,
            &object,
            state.origin.as_str(),
            poll_id,
            expected_lock_version,
        )
        .await
        .is_ok()
}

async fn poll_show(State(state): State<WebState>, uri: Uri, headers: HeaderMap) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, poll_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let mut poll = match state.loader(viewer).authorized_poll(poll_id).await {
        Ok(Some(poll)) => poll,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    if let Some(viewer_account_id) = viewer
        && refresh_remote_poll(&state, poll_id, viewer_account_id).await
    {
        poll = match state.loader(viewer).authorized_poll(poll_id).await {
            Ok(Some(poll)) => poll,
            Ok(None) => return not_found(),
            Err(_) => return internal_error(),
        };
    }
    serde_json::to_vec(&state.serializer().poll(&poll)).map_or_else(
        |_| internal_error(),
        |body| json_response(StatusCode::OK, body),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PollVoteChoicesError {
    Missing,
    Invalid,
}

#[allow(clippy::cast_possible_truncation)]
fn poll_vote_number(value: &serde_json::Number) -> Result<i32, PollVoteChoicesError> {
    if let Some(value) = value.as_i64() {
        return value.try_into().map_err(|_| PollVoteChoicesError::Invalid);
    }
    if let Some(value) = value.as_u64() {
        return value.try_into().map_err(|_| PollVoteChoicesError::Invalid);
    }
    let value = value.as_f64().ok_or(PollVoteChoicesError::Invalid)?;
    let truncated = value.trunc();
    if !value.is_finite() || truncated < f64::from(i32::MIN) || truncated > f64::from(i32::MAX) {
        return Err(PollVoteChoicesError::Invalid);
    }
    Ok(truncated as i32)
}

fn poll_vote_choices(parameters: &RackParameters) -> Result<Vec<i32>, PollVoteChoicesError> {
    let values = match parameters.get("choices") {
        None | Some(RackValue::Null) => return Err(PollVoteChoicesError::Missing),
        Some(RackValue::Array(values)) if values.is_empty() => {
            return Err(PollVoteChoicesError::Missing);
        }
        Some(RackValue::Array(values)) => values,
        Some(_) => return Err(PollVoteChoicesError::Invalid),
    };
    values
        .iter()
        .map(|value| match value {
            RackValue::Scalar(value) => value
                .trim()
                .parse::<i32>()
                .map_err(|_| PollVoteChoicesError::Invalid),
            RackValue::Number(value) => poll_vote_number(value),
            _ => Err(PollVoteChoicesError::Invalid),
        })
        .collect()
}

async fn poll_vote(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_STATUSES).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let Some((_, poll_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    match state.loader(Some(owner)).authorized_poll(poll_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    }
    let choices = match poll_vote_choices(&rack) {
        Ok(choices) => choices,
        Err(PollVoteChoicesError::Missing) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "param is missing or the value is empty or invalid: choices",
            );
        }
        Err(PollVoteChoicesError::Invalid) => {
            return error_response(StatusCode::BAD_REQUEST, "Invalid choices");
        }
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    if let Err(error) = writer
        .vote_poll(&authenticated, poll_id, &choices, state.origin.as_str())
        .await
    {
        return status_saved_write_error(&error);
    }
    let poll = match state.loader(Some(owner)).authorized_poll(poll_id).await {
        Ok(Some(poll)) => poll,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    serde_json::to_vec(&state.serializer().poll(&poll)).map_or_else(
        |_| internal_error(),
        |body| json_response(StatusCode::OK, body),
    )
}

async fn status_show(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let status = match state.loader(viewer).authorized_status(status_id).await {
        Ok(Some(status)) => status,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .status(&status, StatusShape::Full)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn status_delete(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match required_write_viewer(&state, &headers, WRITE_STATUSES).await {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return record_not_found();
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner.account_id(),
        Err(error) => return error.into_http_response().map(Body::from),
    };
    let status = match state.loader(Some(owner)).authorized_status(status_id).await {
        Ok(Some(status)) => status,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    let body = state
        .serializer()
        .status(&status, StatusShape::Source)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok());
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .delete_status_with_origin(
            &authenticated,
            status_id,
            boolean_parameter(&rack, "delete_media"),
            Some(state.origin.as_str()),
        )
        .await
    {
        Ok(_) => {}
        Err(error) => return status_saved_write_error(&error),
    }
    match body {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn status_context(
    State(state): State<WebState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let viewer = match optional_viewer(&state, &headers, READ_STATUSES).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };
    let Some((_, status_id)) = uri_path_id(&uri, 4) else {
        return not_found();
    };
    let context = match state.loader(viewer).status_context(status_id).await {
        Ok(Some(context)) => context,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    match state
        .serializer()
        .status_context(&context)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn public_timeline(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let parameters = parameters(query.as_deref());
    let requested_feed = requested_feed_options(&rack);
    let (viewer, access) = match timeline_viewer(
        &state,
        &headers,
        READ_STATUSES,
        &requested_feed,
        FeedKind::Public,
    )
    .await
    {
        Ok(result) => result,
        Err(response) => return response,
    };
    let mut options = match timeline_options(&rack) {
        Ok(options) => options,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    if !apply_feed_access(&mut options, access) {
        return timeline_response(
            &state,
            "api/v1/timelines/public",
            &parameters,
            &["local", "remote", "limit", "only_media"],
            &[],
        );
    }
    let Ok(statuses) = state.loader(viewer).public_timeline(&options).await else {
        return internal_error();
    };
    timeline_response(
        &state,
        "api/v1/timelines/public",
        &parameters,
        &["local", "remote", "limit", "only_media"],
        &statuses,
    )
}

async fn tag_timeline(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let parameters = parameters(query.as_deref());
    let requested_feed = requested_feed_options(&rack);
    let (viewer, access) = match timeline_viewer(
        &state,
        &headers,
        READ_STATUSES,
        &requested_feed,
        FeedKind::Topic,
    )
    .await
    {
        Ok(result) => result,
        Err(response) => return response,
    };
    let Some(hashtag) = uri_path_segment(&uri, 5) else {
        return not_found();
    };
    match state.loader(viewer).tag_exists(&hashtag).await {
        Ok(false) => {
            return timeline_response(
                &state,
                &format!("api/v1/timelines/tag/{hashtag}"),
                &parameters,
                &["local", "limit", "only_media"],
                &[],
            );
        }
        Ok(true) => {}
        Err(_) => return internal_error(),
    }
    let mut page = match timeline_options(&rack) {
        Ok(page) => page,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    if !apply_feed_access(&mut page, access) {
        return timeline_response(
            &state,
            &format!("api/v1/timelines/tag/{hashtag}"),
            &parameters,
            &["local", "limit", "only_media"],
            &[],
        );
    }
    let options = TagTimelineOptions {
        page,
        any: rack_array_values(&rack, "any"),
        all: rack_array_values(&rack, "all"),
        none: rack_array_values(&rack, "none"),
    };
    let Ok(statuses) = state.loader(viewer).tag_timeline(&hashtag, &options).await else {
        return internal_error();
    };
    timeline_response(
        &state,
        &format!("api/v1/timelines/tag/{hashtag}"),
        &parameters,
        &["local", "limit", "only_media"],
        &statuses,
    )
}

async fn home_timeline(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_STATUSES).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let parameters = parameters(query.as_deref());
    let options = match timeline_options(&rack) {
        Ok(options) => options,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let Ok(statuses) = state
        .loader(Some(owner))
        .home_timeline(owner, &options)
        .await
    else {
        return internal_error();
    };
    timeline_response(
        &state,
        "api/v1/timelines/home",
        &parameters,
        &["local", "limit"],
        &statuses,
    )
}

async fn list_timeline(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    uri: Uri,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer(&state, &headers, READ_LISTS).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some((list_path, list_id)) = uri_path_id(&uri, 5) else {
        return not_found();
    };
    match state
        .loader(Some(owner))
        .owned_list_exists(owner, list_id)
        .await
    {
        Ok(false) => return record_not_found(),
        Ok(true) => {}
        Err(_) => return internal_error(),
    }
    let parameters = parameters(query.as_deref());
    let options = match timeline_options(&rack) {
        Ok(options) => options,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let statuses = match state
        .loader(Some(owner))
        .list_timeline(owner, list_id, &options)
        .await
    {
        Ok(Some(statuses)) => statuses,
        Ok(None) => return record_not_found(),
        Err(_) => return internal_error(),
    };
    timeline_response(
        &state,
        &format!("api/v1/timelines/list/{list_path}"),
        &parameters,
        &["limit"],
        &statuses,
    )
}

async fn favourites(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    saved_statuses(state, rack, query, headers, SavedStatusKind::Favourites).await
}

async fn bookmarks(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    saved_statuses(state, rack, query, headers, SavedStatusKind::Bookmarks).await
}

async fn saved_statuses(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    kind: SavedStatusKind,
) -> Response<Body> {
    let (scopes, route) = match kind {
        SavedStatusKind::Favourites => (READ_FAVOURITES, "api/v1/favourites"),
        SavedStatusKind::Bookmarks => (READ_BOOKMARKS, "api/v1/bookmarks"),
    };
    let owner = match required_viewer(&state, &headers, scopes).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let parameters = parameters(query.as_deref());
    let (max_id, min_id, since_id) = match cursor_triplet(&rack) {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = SavedStatusesOptions {
        max_id,
        min_id,
        since_id,
        limit: match limit_parameter(&rack, 20, 40) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let Ok(page) = state
        .loader(Some(owner))
        .saved_statuses(owner, kind, &options)
        .await
    else {
        return internal_error();
    };
    let mut response = statuses_response(&state, &page.statuses);
    let mut links = Vec::new();
    if page.records_continue
        && let Some(last_cursor) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            route,
            &parameters,
            "max_id",
            last_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_cursor) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            route,
            &parameters,
            "min_id",
            first_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

async fn blocks(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    account_list(state, rack, query, headers, AccountListKind::Blocks).await
}

async fn mutes(
    state: State<WebState>,
    rack: Extension<RackParameters>,
    query: RawQuery,
    headers: HeaderMap,
) -> Response<Body> {
    account_list(state, rack, query, headers, AccountListKind::Mutes).await
}

async fn account_list(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    kind: AccountListKind,
) -> Response<Body> {
    let (scopes, route) = match kind {
        AccountListKind::Blocks => (READ_BLOCKS, "api/v1/blocks"),
        AccountListKind::Mutes => (READ_MUTES, "api/v1/mutes"),
    };
    let owner = match required_viewer(&state, &headers, scopes).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let parameters = parameters(query.as_deref());
    let (max_id, since_id) = match cursor_pair(&rack, "max_id", "since_id") {
        Ok(cursors) => cursors,
        Err(error) => return cursor_parameter_error(&headers, error),
    };
    let options = AccountListOptions {
        max_id,
        since_id,
        limit: match limit_parameter(&rack, 40, 80) {
            Ok(limit) => limit,
            Err(()) => return framework_internal_error(),
        },
    };
    let Ok(page) = state
        .loader(Some(owner))
        .account_list(owner, kind, &options)
        .await
    else {
        return internal_error();
    };
    let serializer = state.serializer();
    let values = page.entries.iter().map(|entry| match kind {
        AccountListKind::Blocks => serializer
            .account(&entry.account)
            .ok()
            .and_then(|value| serde_json::to_value(value).ok()),
        AccountListKind::Mutes => serializer
            .muted_account(&entry.account, entry.mute_expires_at)
            .ok()
            .and_then(|value| serde_json::to_value(value).ok()),
    });
    let mut response = match values
        .collect::<Option<Vec<_>>>()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => return internal_error(),
    };
    let mut links = Vec::new();
    if usize::try_from(options.limit).is_ok_and(|limit| page.entries.len() == limit)
        && let Some(last_cursor) = page.last_cursor
        && let Some(url) = pagination_url(
            &state,
            route,
            &parameters,
            "max_id",
            last_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_cursor) = page.first_cursor
        && let Some(url) = pagination_url(
            &state,
            route,
            &parameters,
            "since_id",
            first_cursor,
            &["limit"],
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

fn timeline_response(
    state: &WebState,
    route: &str,
    parameters: &[(String, String)],
    preserved_names: &[&str],
    statuses: &[crate::mastodon::rest::StatusProjection],
) -> Response<Body> {
    let mut response = statuses_response(state, statuses);
    let mut links = Vec::new();
    if let Some(last_id) = statuses.last().map(|status| status.id)
        && let Some(url) =
            pagination_url(state, route, parameters, "max_id", last_id, preserved_names)
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_id) = statuses.first().map(|status| status.id)
        && let Some(url) = pagination_url(
            state,
            route,
            parameters,
            "min_id",
            first_id,
            preserved_names,
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

fn notification_response(
    state: &WebState,
    route: &str,
    parameters: &[(String, String)],
    preserved_names: &[&str],
    ids: &[i64],
    body: Vec<u8>,
) -> Response<Body> {
    let mut response = json_response(StatusCode::OK, body);
    let mut links = Vec::new();
    if let Some(last_id) = ids.last()
        && let Some(url) = pagination_url_repeated(
            state,
            route,
            parameters,
            "max_id",
            *last_id,
            preserved_names,
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_id) = ids.first()
        && let Some(url) = pagination_url_repeated(
            state,
            route,
            parameters,
            "min_id",
            *first_id,
            preserved_names,
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

fn notification_request_response(
    state: &WebState,
    route: &str,
    parameters: &[(String, String)],
    preserved_names: &[&str],
    ids: &[i64],
    body: Vec<u8>,
    limit: i64,
) -> Response<Body> {
    let mut response = json_response(StatusCode::OK, body);
    let mut links = Vec::new();
    if usize::try_from(limit).is_ok_and(|limit| ids.len() == limit)
        && let Some(last_id) = ids.last()
        && let Some(url) = pagination_url_repeated(
            state,
            route,
            parameters,
            "max_id",
            *last_id,
            preserved_names,
        )
    {
        links.push(format!("<{url}>; rel=\"next\""));
    }
    if let Some(first_id) = ids.first()
        && let Some(url) = pagination_url_repeated(
            state,
            route,
            parameters,
            "min_id",
            *first_id,
            preserved_names,
        )
    {
        links.push(format!("<{url}>; rel=\"prev\""));
    }
    set_link_header(&mut response, &links);
    response
}

fn statuses_response(
    state: &WebState,
    statuses: &[crate::mastodon::rest::StatusProjection],
) -> Response<Body> {
    let serializer = state.serializer();
    let values = statuses
        .iter()
        .map(|status| serializer.status(status, StatusShape::Full))
        .collect::<Result<Vec<_>, _>>();
    match values
        .ok()
        .and_then(|values| serde_json::to_vec(&values).ok())
    {
        Some(body) => json_response(StatusCode::OK, body),
        None => internal_error(),
    }
}

async fn optional_viewer(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<Option<i64>, Response<Body>> {
    optional_viewer_owner(state, headers, scopes)
        .await
        .map(|owner| owner.map(OAuthResourceOwner::account_id))
}

async fn optional_viewer_owner(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<Option<OAuthResourceOwner>, Response<Body>> {
    if !headers.contains_key(AUTHORIZATION) {
        return Ok(None);
    }
    match state.authenticator.authenticate(headers, scopes).await {
        Ok(authenticated) => Ok(authenticated.resource_owner()),
        Err(OAuthAuthenticationError::OAuth(
            OAuthError::Unauthenticated
            | OAuthError::InvalidToken(crate::mastodon::InvalidTokenReason::Unknown),
        )) => Ok(None),
        Err(OAuthAuthenticationError::OAuth(error)) => {
            Err(error.into_http_response().map(Body::from))
        }
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

async fn required_viewer_owner(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<OAuthResourceOwner, Response<Body>> {
    match state.authenticator.authenticate(headers, scopes).await {
        Ok(authenticated) => authenticated
            .require_user()
            .map_err(|error| error.into_http_response().map(Body::from)),
        Err(OAuthAuthenticationError::OAuth(error)) => {
            Err(error.into_http_response().map(Body::from))
        }
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

async fn required_scope_owner(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<Option<i64>, Response<Body>> {
    match state.authenticator.authenticate(headers, scopes).await {
        Ok(authenticated) => Ok(authenticated
            .resource_owner()
            .map(OAuthResourceOwner::account_id)),
        Err(OAuthAuthenticationError::OAuth(error)) => {
            Err(error.into_http_response().map(Body::from))
        }
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

async fn optional_scope_owner(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<Option<i64>, Response<Body>> {
    if !headers.contains_key(AUTHORIZATION) {
        return Ok(None);
    }
    match state.authenticator.authenticate(headers, scopes).await {
        Ok(authenticated) => Ok(authenticated
            .resource_owner()
            .map(OAuthResourceOwner::account_id)),
        Err(OAuthAuthenticationError::OAuth(error)) => {
            Err(error.into_http_response().map(Body::from))
        }
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

async fn optional_authenticated_user_id(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<Option<i64>, Response<Body>> {
    if !headers.contains_key(AUTHORIZATION) {
        return Ok(None);
    }
    match state.authenticator.authenticate(headers, NO_SCOPE).await {
        Ok(authenticated) => Ok(authenticated
            .resource_owner()
            .map(OAuthResourceOwner::user_id)),
        Err(OAuthAuthenticationError::OAuth(_)) => Ok(None),
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

async fn required_viewer(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<i64, Response<Body>> {
    required_viewer_owner(state, headers, scopes)
        .await
        .map(OAuthResourceOwner::account_id)
}

async fn required_write_viewer(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<AuthenticatedBearer, Response<Body>> {
    match state.authenticator.authenticate(headers, scopes).await {
        Ok(authenticated) => authenticated
            .require_user()
            .map(|_| authenticated)
            .map_err(|error| error.into_http_response().map(Body::from)),
        Err(OAuthAuthenticationError::OAuth(error)) => {
            Err(error.into_http_response().map(Body::from))
        }
        Err(OAuthAuthenticationError::Repository(_)) => Err(internal_error()),
    }
}

fn parameters(query: Option<&str>) -> Vec<(String, String)> {
    query
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

fn path_id(value: &str) -> Option<i64> {
    parse_path_id(value, true)
}

fn activitypub_path_id(value: &str) -> Option<i64> {
    if value != trim_ascii_start(value) || value.starts_with('+') {
        return None;
    }
    path_id(value)
}

fn route_path_id(value: &str) -> Option<i64> {
    parse_path_id(value, false)
}

fn parse_path_id(value: &str, require_full: bool) -> Option<i64> {
    let value = trim_ascii_start(value);
    let (negative, digits) = value.strip_prefix('-').map_or_else(
        || (false, value.strip_prefix('+').unwrap_or(value)),
        |value| (true, value),
    );
    let digits = if require_full {
        if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
            return None;
        }
        digits
    } else {
        &digits[..digits.bytes().take_while(u8::is_ascii_digit).count()]
    };
    if digits.is_empty() {
        return None;
    }
    let mut value = 0_i64;
    for digit in digits.bytes() {
        let digit = i64::from(digit - b'0');
        value = if negative {
            value.checked_mul(10)?.checked_sub(digit)?
        } else {
            value.checked_mul(10)?.checked_add(digit)?
        };
    }
    Some(value)
}

fn uri_path_id(uri: &Uri, segment: usize) -> Option<(String, i64)> {
    let path = uri.path().split('/').nth(segment)?;
    let decoded = percent_decode_str(path).decode_utf8().ok()?;
    let id = route_path_id(&decoded)?;
    Some((canonical_path_segment(&decoded), id))
}

fn canonical_path_segment(value: &str) -> String {
    utf8_percent_encode(value, RAILS_PATH_SEGMENT).to_string()
}

fn uri_path_segment(uri: &Uri, segment: usize) -> Option<String> {
    percent_decode_str(uri.path().split('/').nth(segment)?)
        .decode_utf8()
        .ok()
        .filter(|value| !value.is_empty())
        .map(std::borrow::Cow::into_owned)
}

fn string_parameter<'a>(parameters: &'a [(String, String)], name: &str) -> Option<&'a str> {
    parameters
        .iter()
        .rfind(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str())
}

fn rack_array_values(parameters: &RackParameters, name: &str) -> Vec<String> {
    match parameters.get(name) {
        Some(RackValue::Scalar(value)) => vec![value.clone()],
        Some(RackValue::Number(value)) => vec![value.to_string()],
        Some(RackValue::Boolean(value)) => vec![value.to_string()],
        Some(RackValue::Array(values)) => values
            .iter()
            .filter_map(|value| match value {
                RackValue::Null => Some(String::new()),
                RackValue::Scalar(value) => Some(value.clone()),
                RackValue::Number(value) => Some(ruby_json_number(value)),
                RackValue::Boolean(value) => Some(value.to_string()),
                RackValue::Array(_) | RackValue::Object(_) | RackValue::Upload(_) => None,
            })
            .collect(),
        None | Some(RackValue::Null | RackValue::Object(_) | RackValue::Upload(_)) => Vec::new(),
    }
}

fn rack_array_parameter(parameters: &RackParameters, name: &str) -> Vec<String> {
    if matches!(parameters.get(name), Some(RackValue::Array(_))) {
        rack_array_values(parameters, name)
    } else {
        Vec::new()
    }
}

fn grouped_types_parameter_invalid(parameters: &RackParameters) -> bool {
    matches!(
        parameters.get("grouped_types"),
        Some(RackValue::Scalar(value)) if !value.trim().is_empty()
    ) || matches!(
        parameters.get("grouped_types"),
        Some(
            RackValue::Boolean(_)
                | RackValue::Number(_)
                | RackValue::Object(_)
                | RackValue::Upload(_),
        )
    )
}

fn notification_supported_types(parameters: &RackParameters) -> Option<Vec<String>> {
    match parameters.get("supported_types") {
        Some(RackValue::Scalar(value)) => Some(vec![value.clone()]),
        Some(RackValue::Array(_)) => Some(rack_array_values(parameters, "supported_types")),
        None
        | Some(
            RackValue::Null
            | RackValue::Boolean(_)
            | RackValue::Number(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => None,
    }
}

fn tagged_parameter(parameters: &RackParameters) -> Option<String> {
    match parameters.get("tagged") {
        None | Some(RackValue::Null) => None,
        Some(RackValue::Scalar(value)) => (!value.trim().is_empty()).then(|| value.clone()),
        Some(RackValue::Number(value)) => Some(ruby_json_number(value)),
        Some(RackValue::Boolean(value)) => Some(value.to_string()),
        Some(RackValue::Array(values)) => values.iter().find_map(|value| match value {
            RackValue::Scalar(value) if !value.trim().is_empty() => Some(value.clone()),
            RackValue::Number(value) => Some(ruby_json_number(value)),
            RackValue::Boolean(value) => Some(value.to_string()),
            RackValue::Null
            | RackValue::Scalar(_)
            | RackValue::Array(_)
            | RackValue::Object(_)
            | RackValue::Upload(_) => None,
        }),
        Some(RackValue::Object(_) | RackValue::Upload(_)) => Some(String::new()),
    }
}

fn batch_account_ids(parameters: &RackParameters) -> Result<Vec<i64>, ()> {
    let Some(RackValue::Array(values)) = parameters.get("id") else {
        return Ok(Vec::new());
    };
    // Strong parameters permit(id: []) discards the entire value unless it is
    // an array of permitted scalars; do not salvage IDs from mixed nested arrays.
    if values
        .iter()
        .any(|value| matches!(value, RackValue::Array(_) | RackValue::Object(_)))
    {
        return Ok(Vec::new());
    }
    let coerced = relationship_ids(parameters)?;
    let mut seen = std::collections::HashSet::new();
    // Count raw unique values, not unique integers. Keep JSON strings and numbers
    // distinct too: ["1", 1] occupies two slots in Ruby's uniq before map(&:to_i).
    Ok(values
        .iter()
        .zip(coerced)
        .filter_map(|(raw, id)| {
            seen.insert((std::mem::discriminant(raw), rack_value_display(raw)))
                .then_some(id)
        })
        .collect())
}

fn relationship_ids(parameters: &RackParameters) -> Result<Vec<i64>, ()> {
    match parameters.get("id") {
        None | Some(RackValue::Null) => Ok(Vec::new()),
        Some(RackValue::Scalar(value)) => Ok(vec![ruby_integer(value)]),
        Some(RackValue::Number(value)) => Ok(vec![json_number_integer(value)?]),
        Some(RackValue::Boolean(_) | RackValue::Object(_) | RackValue::Upload(_)) => Err(()),
        Some(RackValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                RackValue::Null => Ok(0),
                RackValue::Scalar(value) => Ok(ruby_integer(value)),
                RackValue::Number(value) => json_number_integer(value),
                RackValue::Boolean(_)
                | RackValue::Array(_)
                | RackValue::Object(_)
                | RackValue::Upload(_) => Err(()),
            })
            .collect(),
    }
}

fn notification_options(
    parameters: &RackParameters,
    grouped: bool,
) -> Result<NotificationOptions, CursorParameterError> {
    notification_options_with_limits(parameters, grouped, 40, 80, true)
}

fn notification_options_with_limits(
    parameters: &RackParameters,
    grouped: bool,
    default_limit: i64,
    maximum_limit: i64,
    parse_cursors: bool,
) -> Result<NotificationOptions, CursorParameterError> {
    let (max_id, min_id, since_id) = if parse_cursors {
        cursor_triplet(parameters)?
    } else {
        (None, None, None)
    };
    let requested_types = rack_array_parameter(parameters, "types");
    let excluded_types = rack_array_parameter(parameters, "exclude_types");
    Ok(NotificationOptions {
        max_id,
        min_id,
        since_id,
        limit: limit_parameter(parameters, default_limit, maximum_limit)
            .map_err(|()| CursorParameterError::InvalidScalar)?,
        account_id: if grouped {
            None
        } else {
            integer_parameter(parameters, "account_id")?
        },
        types: notification_type_filter_with_exclusions(&requested_types, &excluded_types),
        exclude_types: Vec::new(),
        grouped_types: if grouped {
            rack_array_parameter(parameters, "grouped_types")
        } else {
            Vec::new()
        },
        include_filtered: boolean_parameter(parameters, "include_filtered"),
    })
}

fn expand_accounts_parameter(parameters: &RackParameters) -> Option<bool> {
    match parameters.get("expand_accounts") {
        None | Some(RackValue::Null) => Some(false),
        Some(RackValue::Scalar(value)) => match value.as_str() {
            "full" => Some(false),
            "partial_avatars" => Some(true),
            _ => None,
        },
        Some(
            RackValue::Boolean(_)
            | RackValue::Number(_)
            | RackValue::Array(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => None,
    }
}

fn invalid_expand_accounts_response(parameters: &RackParameters) -> Response<Body> {
    let value = parameters
        .get("expand_accounts")
        .map_or_else(|| "nil".to_owned(), rack_value_display);
    let message = format!(
        "Invalid value for 'expand_accounts': '{value}', allowed values are 'full' and 'partial_avatars'"
    );
    error_response(StatusCode::BAD_REQUEST, &message)
}

fn rack_value_display(value: &RackValue) -> String {
    match value {
        RackValue::Null => "nil".to_owned(),
        RackValue::Scalar(value) => value.clone(),
        RackValue::Number(value) => ruby_json_number(value),
        RackValue::Boolean(value) => value.to_string(),
        RackValue::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(rack_value_inspect)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        RackValue::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| {
                    format!(
                        "\"{}\"=>{}",
                        key.replace('"', "\\\""),
                        rack_value_inspect(value)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        RackValue::Upload(upload) => format!("[uploaded file {}]", upload.file_name),
    }
}

fn rack_value_inspect(value: &RackValue) -> String {
    match value {
        RackValue::Scalar(value) => format!("\"{}\"", value.replace('"', "\\\"")),
        value => rack_value_display(value),
    }
}

fn timeline_options(parameters: &RackParameters) -> Result<TimelineOptions, CursorParameterError> {
    let (max_id, min_id, since_id) = cursor_triplet(parameters)?;
    Ok(TimelineOptions {
        max_id,
        min_id,
        since_id,
        limit: limit_parameter(parameters, 20, 40)
            .map_err(|()| CursorParameterError::InvalidScalar)?,
        local: boolean_parameter(parameters, "local"),
        remote: boolean_parameter(parameters, "remote"),
        only_media: boolean_parameter(parameters, "only_media"),
    })
}

fn requested_feed_options(parameters: &RackParameters) -> TimelineOptions {
    TimelineOptions {
        local: boolean_parameter(parameters, "local"),
        remote: boolean_parameter(parameters, "remote"),
        ..TimelineOptions::default()
    }
}

#[derive(Clone, Copy)]
enum FeedKind {
    Public,
    Topic,
}

#[derive(Clone, Copy)]
struct FeedAccess {
    local: bool,
    remote: bool,
}

async fn timeline_viewer(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
    options: &TimelineOptions,
    kind: FeedKind,
) -> Result<(Option<i64>, FeedAccess), Response<Body>> {
    let settings = state
        .repository
        .settings()
        .await
        .map_err(|_| internal_error())?;
    let setting = |name: &str| {
        settings
            .iter()
            .find(|setting| setting.var == name)
            .and_then(|setting| setting.value.as_ref())
            .and_then(|value| yaml_scalar(value.raw()))
            .unwrap_or_else(|| "public".to_owned())
    };
    let (local, remote) = match kind {
        FeedKind::Public => (
            setting("local_live_feed_access"),
            setting("remote_live_feed_access"),
        ),
        FeedKind::Topic => (
            setting("local_topic_feed_access"),
            setting("remote_topic_feed_access"),
        ),
    };
    let requires_user = if options.local {
        local != "public"
    } else if options.remote {
        remote != "public"
    } else {
        local != "public" || remote != "public"
    };
    let owner = if requires_user {
        Some(required_timeline_viewer(state, headers, scopes).await?)
    } else {
        optional_viewer_owner(state, headers, scopes).await?
    };
    let viewer = owner.map(OAuthResourceOwner::account_id);
    let can_view_disabled = match owner {
        Some(owner) => state
            .repository
            .user_can_view_feeds(owner.user_id(), owner.account_id())
            .await
            .map_err(|_| internal_error())?,
        None => false,
    };
    let allowed = |setting: &str| match setting {
        "public" => true,
        "authenticated" => viewer.is_some(),
        "disabled" => can_view_disabled,
        _ => false,
    };
    Ok((
        viewer,
        FeedAccess {
            local: allowed(&local),
            remote: allowed(&remote),
        },
    ))
}

fn apply_feed_access(options: &mut TimelineOptions, access: FeedAccess) -> bool {
    let requested_local = !options.remote || options.local;
    let requested_remote = !options.local || options.remote;
    let local = requested_local && access.local;
    let remote = requested_remote && access.remote;
    options.local = local && !remote;
    options.remote = remote && !local;
    local || remote
}

async fn required_timeline_viewer(
    state: &WebState,
    headers: &HeaderMap,
    scopes: RequiredScopes,
) -> Result<OAuthResourceOwner, Response<Body>> {
    if !headers.contains_key(AUTHORIZATION) {
        return Err(OAuthError::UserRequired
            .into_http_response()
            .map(Body::from));
    }
    required_viewer_owner(state, headers, scopes).await
}

fn yaml_scalar(raw: &str) -> Option<String> {
    let value = raw.trim();
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorParameterError {
    InvalidScalar,
    Overflow,
}

type CursorTriplet = (Option<i64>, Option<i64>, Option<i64>);

fn strict_optional_id_parameter(
    parameters: &RackParameters,
    name: &str,
) -> Result<Option<i64>, ()> {
    match parameters.get(name) {
        None | Some(RackValue::Null) => Ok(None),
        Some(RackValue::Scalar(value)) if value.trim().is_empty() => Ok(None),
        Some(RackValue::Scalar(value)) => value.trim().parse::<i64>().map(Some).map_err(|_| ()),
        Some(RackValue::Number(value)) => json_number_integer(value).map(Some),
        Some(
            RackValue::Boolean(_)
            | RackValue::Array(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => Err(()),
    }
}

fn integer_parameter(
    parameters: &RackParameters,
    name: &str,
) -> Result<Option<i64>, CursorParameterError> {
    let empty_bound = || {
        if name == "max_id" { i64::MIN } else { i64::MAX }
    };
    let value = match parameters.get(name) {
        None | Some(RackValue::Null) => return Ok(None),
        Some(RackValue::Scalar(value)) if value.trim().is_empty() => return Ok(None),
        Some(RackValue::Scalar(value)) => value,
        Some(RackValue::Number(value)) => {
            return json_number_integer(value)
                .map(Some)
                .map_err(|()| CursorParameterError::Overflow);
        }
        Some(RackValue::Boolean(_)) => return Err(CursorParameterError::InvalidScalar),
        Some(RackValue::Array(_) | RackValue::Object(_) | RackValue::Upload(_)) => {
            return Ok(Some(empty_bound()));
        }
    };
    checked_integer_prefix(value)
        .map_err(|()| CursorParameterError::Overflow)
        .map(|value| Some(value.unwrap_or_else(empty_bound)))
}

fn cursor_pair(
    parameters: &RackParameters,
    first: &str,
    second: &str,
) -> Result<(Option<i64>, Option<i64>), CursorParameterError> {
    Ok((
        integer_parameter(parameters, first)?,
        integer_parameter(parameters, second)?,
    ))
}

fn cursor_triplet(parameters: &RackParameters) -> Result<CursorTriplet, CursorParameterError> {
    Ok((
        integer_parameter(parameters, "max_id")?,
        integer_parameter(parameters, "min_id")?,
        integer_parameter(parameters, "since_id")?,
    ))
}

fn cursor_parameter_error(_headers: &HeaderMap, error: CursorParameterError) -> Response<Body> {
    match error {
        CursorParameterError::InvalidScalar | CursorParameterError::Overflow => {
            framework_internal_error()
        }
    }
}

fn boolean_parameter(parameters: &RackParameters, name: &str) -> bool {
    parameters.get(name).is_some_and(boolean_value)
}

fn optional_boolean_parameter(parameters: &RackParameters, name: &str) -> Option<bool> {
    match parameters.get(name) {
        None | Some(RackValue::Null) => None,
        Some(_) => Some(boolean_parameter(parameters, name)),
    }
}

fn limit_parameter(parameters: &RackParameters, default: i64, maximum: i64) -> Result<i64, ()> {
    match parameters.get("limit") {
        None | Some(RackValue::Null) => Ok(default),
        Some(RackValue::Scalar(value)) => Ok(ruby_integer(value).saturating_abs().min(maximum)),
        Some(RackValue::Number(value)) => json_number_limit(value, maximum),
        Some(
            RackValue::Boolean(_)
            | RackValue::Array(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => Err(()),
    }
}

fn nonnegative_search_parameter(
    parameters: &RackParameters,
    name: &str,
    default: i64,
    maximum: Option<i64>,
) -> Result<i64, ()> {
    let value = match parameters.get(name) {
        None | Some(RackValue::Null) => return Ok(default),
        Some(RackValue::Scalar(value)) => ruby_integer(value),
        Some(RackValue::Number(value)) => json_number_integer(value)?,
        Some(
            RackValue::Boolean(_)
            | RackValue::Array(_)
            | RackValue::Object(_)
            | RackValue::Upload(_),
        ) => return Err(()),
    };
    if value < 0 {
        return Err(());
    }
    Ok(maximum.map_or(value, |maximum| value.min(maximum)))
}

fn ruby_integer(value: &str) -> i64 {
    let value = trim_ascii_start(value);
    let bytes = value.as_bytes();
    let (negative, start) = match bytes.first() {
        Some(b'-') => (true, 1),
        Some(b'+') => (false, 1),
        _ => (false, 0),
    };
    let magnitude = bytes[start..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .fold(0_i64, |number, byte| {
            number
                .saturating_mul(10)
                .saturating_add(i64::from(*byte - b'0'))
        });
    if negative {
        magnitude.saturating_neg()
    } else {
        magnitude
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn json_number_integer(value: &serde_json::Number) -> Result<i64, ()> {
    value
        .as_i64()
        .map(Ok)
        .or_else(|| {
            value
                .as_u64()
                .map(|value| i64::try_from(value).map_err(|_| ()))
        })
        .unwrap_or_else(|| {
            let value = value.as_f64().ok_or(())?;
            if value < i64::MIN as f64 || value >= i64::MAX as f64 {
                Err(())
            } else {
                Ok(value as i64)
            }
        })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn json_number_limit(value: &serde_json::Number, maximum: i64) -> Result<i64, ()> {
    if value.as_i64().is_none() && value.as_u64().is_none() && value.as_f64().is_none() {
        return Err(());
    }
    if value
        .as_i64()
        .is_some_and(|value| value.unsigned_abs() >= maximum as u64)
        || value.as_u64().is_some_and(|value| value >= maximum as u64)
        || value
            .as_f64()
            .is_some_and(|value| value.abs() >= maximum as f64)
    {
        Ok(maximum)
    } else {
        Ok(value.as_f64().unwrap_or_default().abs() as i64)
    }
}

fn ruby_json_number(value: &serde_json::Number) -> String {
    if value.as_i64().is_some() || value.as_u64().is_some() {
        return value.to_string();
    }
    value.as_f64().map_or_else(
        || value.to_string(),
        |value| {
            let absolute = value.abs();
            if absolute >= 1e15 || (absolute != 0.0 && absolute < 1e-4) {
                let scientific = format!("{value:e}");
                let (mantissa, exponent) = scientific
                    .split_once('e')
                    .expect("scientific float contains exponent");
                let mantissa = if mantissa.contains('.') {
                    mantissa.to_owned()
                } else {
                    format!("{mantissa}.0")
                };
                let (sign, digits) = exponent
                    .strip_prefix('+')
                    .map_or_else(
                        || exponent.strip_prefix('-').map(|digits| ('-', digits)),
                        |digits| Some(('+', digits)),
                    )
                    .unwrap_or(('+', exponent));
                let exponent = if digits.len() < 2 {
                    format!("{sign}0{digits}")
                } else {
                    format!("{sign}{digits}")
                };
                format!("{mantissa}e{exponent}")
            } else if value.fract() == 0.0 {
                format!("{value:.1}")
            } else {
                value.to_string()
            }
        },
    )
}

fn checked_integer_prefix(value: &str) -> Result<Option<i64>, ()> {
    let value = trim_ascii_start(value);
    let (negative, digits) = value.strip_prefix('-').map_or_else(
        || (false, value.strip_prefix('+').unwrap_or(value)),
        |value| (true, value),
    );
    let mut number = 0_i64;
    let mut found = false;
    for digit in digits.bytes().take_while(u8::is_ascii_digit) {
        found = true;
        let digit = i64::from(digit - b'0');
        number = if negative {
            number
                .checked_mul(10)
                .and_then(|value| value.checked_sub(digit))
        } else {
            number
                .checked_mul(10)
                .and_then(|value| value.checked_add(digit))
        }
        .ok_or(())?;
    }
    Ok(found.then_some(number))
}

fn trim_ascii_start(value: &str) -> &str {
    value.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c'])
}

fn pagination_url(
    state: &WebState,
    route: &str,
    parameters: &[(String, String)],
    cursor_name: &str,
    cursor: i64,
    preserved_names: &[&str],
) -> Option<String> {
    let mut url = state.origin.join(route).ok()?;
    {
        let cursor = cursor.to_string();
        let mut parameters = preserved_names
            .iter()
            .filter_map(|name| string_parameter(parameters, name).map(|value| (*name, value)))
            .collect::<Vec<_>>();
        parameters.push((cursor_name, &cursor));
        parameters.sort_by_key(|(name, _)| *name);
        let mut pairs = url.query_pairs_mut();
        for (name, value) in parameters {
            pairs.append_pair(name, value);
        }
    }
    Some(url.to_string())
}

fn pagination_url_repeated(
    state: &WebState,
    route: &str,
    parameters: &[(String, String)],
    cursor_name: &str,
    cursor: i64,
    preserved_names: &[&str],
) -> Option<String> {
    let mut url = state.origin.join(route).ok()?;
    let cursor = cursor.to_string();
    let mut preserved = parameters
        .iter()
        .filter(|(name, _)| preserved_names.contains(&name.as_str()))
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    preserved.push((cursor_name, &cursor));
    preserved.sort_by_key(|(name, _)| *name);
    {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in preserved {
            pairs.append_pair(name, value);
        }
    }
    Some(url.to_string())
}

fn set_link_header(response: &mut Response<Body>, links: &[String]) {
    if !links.is_empty()
        && let Ok(value) = HeaderValue::from_str(&links.join(", "))
    {
        response.headers_mut().insert("link", value);
    }
}

fn json_response(status: StatusCode, body: Vec<u8>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(body))
        .expect("static response headers are valid")
}

fn empty_json_response() -> Response<Body> {
    json_response(StatusCode::OK, b"{}".to_vec())
}

fn error_response(status: StatusCode, message: &str) -> Response<Body> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": message }))
        .expect("an error envelope with one string is serializable");
    json_response(status, body)
}

fn rate_limited_response(limited: RateLimitExceeded) -> Response<Body> {
    let mut response = error_response(StatusCode::TOO_MANY_REQUESTS, "Too many requests");
    add_rate_limit_headers(&mut response, limited);
    response
}

async fn report_response_with_rate_limit(
    writer: &WriteRepository,
    owner: i64,
    mut response: Response<Body>,
) -> Response<Body> {
    if let Ok(count) = writer.report_rate_limit_count(owner).await {
        let limit = usize::try_from(REPORT_RATE_LIMIT).unwrap_or_default();
        let used = usize::try_from(count).unwrap_or(limit);
        add_rate_limit_status_headers(
            &mut response,
            RateLimitStatus {
                limit,
                remaining: limit.saturating_sub(used),
                period: REPORT_RATE_LIMIT_PERIOD,
            },
        );
    }
    response
}

fn add_rate_limit_headers(response: &mut Response<Body>, limited: RateLimitExceeded) {
    add_rate_limit_status_headers(
        response,
        RateLimitStatus {
            limit: limited.limit,
            remaining: 0,
            period: limited.period,
        },
    );
}

fn add_rate_limit_status_headers(response: &mut Response<Body>, status: RateLimitStatus) {
    let period = status.period.as_secs();
    if period == 0 {
        return;
    }
    let now = unix_timestamp_seconds();
    let seconds_until_reset = period - now % period;
    let reset = now.saturating_add(seconds_until_reset);
    let reset =
        chrono::DateTime::<Utc>::from_timestamp(i64::try_from(reset).unwrap_or(i64::MAX), 0)
            .map_or_else(
                || reset.to_string(),
                |timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Micros, true),
            );
    if let Ok(value) = HeaderValue::from_str(&status.limit.to_string()) {
        response.headers_mut().insert("x-ratelimit-limit", value);
    }
    if let Ok(value) = HeaderValue::from_str(&status.remaining.to_string()) {
        response
            .headers_mut()
            .insert("x-ratelimit-remaining", value);
    }
    if let Ok(value) = HeaderValue::from_str(&reset) {
        response.headers_mut().insert("x-ratelimit-reset", value);
    }
    if let Ok(value) = HeaderValue::from_str(&seconds_until_reset.to_string()) {
        response.headers_mut().insert("retry-after", value);
    }
}

fn record_not_found() -> Response<Body> {
    error_response(StatusCode::NOT_FOUND, "Record not found")
}

fn not_found() -> Response<Body> {
    error_response(StatusCode::NOT_FOUND, "Not Found")
}

fn api_not_found() -> Response<Body> {
    not_found()
}

fn framework_not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(CONTENT_TYPE, "application/json; charset=UTF-8")
        .body(Body::from(r#"{"status":404,"error":"Not Found"}"#))
        .expect("static framework 404 response is valid")
}

fn internal_error() -> Response<Body> {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
}

fn framework_internal_error() -> Response<Body> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(CONTENT_TYPE, "application/json; charset=UTF-8")
        .header(VARY, "Origin")
        .header(FRAMEWORK_ERROR_HEADER, "1")
        .body(Body::from(
            r#"{"status":500,"error":"Internal Server Error"}"#,
        ))
        .expect("static framework error response is valid")
}

#[cfg(all(test, feature = "test-support"))]
mod account_search_tests;

#[cfg(all(test, feature = "test-support"))]
mod batch_accounts_tests;

mod extended_description;

#[cfg(test)]
mod extended_description_tests;

mod web_settings;

#[cfg(test)]
mod profile_tests;

#[cfg(test)]
mod web_settings_tests;

#[cfg(test)]
mod api_empty_reads_tests;

#[cfg(test)]
mod cached_media_response_tests;

#[cfg(test)]
mod hashtag_controls_tests;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use axum::http::header::{
        ACCEPT, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
        ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_REQUEST_HEADERS,
        ACCESS_CONTROL_REQUEST_METHOD, ORIGIN,
    };
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn stale_login_session_denial_is_an_authentication_failure() {
        let headers = HeaderMap::new();
        let respond = |error: &WriteError| {
            browser_session_creation_error_response(
                true,
                b"test-signing-key",
                &headers,
                "alice@example.invalid",
                error,
                None,
            )
        };
        let denied = respond(&WriteError::Unauthorized);
        assert_eq!(denied.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(denied.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("invalid_credentials"));
        assert_eq!(
            respond(&WriteError::Sqlx(sqlx::Error::PoolTimedOut)).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn paperclip_media_requires_status_access_unless_moderating_discarded_media() {
        assert!(paperclip_media_access_allowed(true, false, false));
        assert!(!paperclip_media_access_allowed(false, false, true));
        assert!(!paperclip_media_access_allowed(false, true, false));
        assert!(paperclip_media_access_allowed(false, true, true));
    }

    #[test]
    fn frontend_shell_uses_the_pinned_manifest_and_escapes_state() {
        let frontend =
            FrontendAssets::load(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("public"))
                .expect("pinned frontend manifests load");
        let runtime = InstanceRuntimeConfig {
            domain: "fixture<example".to_owned(),
            version: "0.1.0".to_owned(),
            source_url: "https://example.invalid/rustodon".to_owned(),
            streaming_api: "wss://fixture.example".to_owned(),
            vapid_public_key: Some("fixture-vapid-key".to_owned()),
            thumbnail_url: "/packs/assets/preview.png".to_owned(),
            thumbnail_description: String::new(),
            thumbnail_blurhash: None,
            thumbnail_versions: None,
            icons: Vec::new(),
            languages: vec!["en".to_owned()],
            translation_enabled: false,
            limited_federation: false,
            single_user_mode: false,
            terms_of_service_url: None,
            sso_signup_url: None,
            wrapstodon: None,
        };
        let document = frontend_document(
            &frontend,
            &runtime,
            "/home",
            None,
            None,
            None,
            "csrf-value",
            "csp-nonce",
        )
        .expect("pinned frontend entries resolve");
        assert!(document.contains("id=\"mastodon\""));
        assert!(document.contains("content=\"/home\""));
        assert!(document.contains("content=\"csrf-value\""));
        assert!(document.contains("applicationServerKey"));
        assert!(document.contains("/packs/"));
        assert!(document.contains("\\u003c"));
        assert!(document.contains("nonce=\"csp-nonce\""));
        let policy = frontend_content_security_policy("csp-nonce");
        assert!(policy.contains("'nonce-csp-nonce'"));
        assert!(!policy.contains("unsafe-inline"));

        let manifest = frontend_manifest_value(&frontend, "Fixture <instance>")
            .expect("pinned frontend icons resolve");
        assert_eq!(manifest["instance"]["id"], "/home");
        assert_eq!(manifest["instance"]["name"], "Fixture <instance>");
        assert_eq!(manifest["instance"]["icons"].as_array().unwrap().len(), 9);
        assert!(safe_frontend_path("../secret").is_none());
        assert!(safe_frontend_path("packs\\secret").is_none());
        assert_eq!(
            json_script(&serde_json::json!({"value": "</script>"})),
            Some(r#"{"value":"\u003c/script\u003e"}"#.to_owned())
        );
    }

    #[test]
    fn html_responses_include_security_headers() {
        let response = html_response(StatusCode::OK, "<main>fixture</main>".to_owned());
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.headers()["x-xss-protection"], "0");
        assert_eq!(response.headers()["referrer-policy"], "same-origin");
        assert_eq!(
            response.headers()["content-security-policy"],
            "default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:"
        );
    }

    #[test]
    fn signature_sensitive_activity_responses_are_not_shared() {
        let response =
            signature_sensitive_activity_response(&HeaderMap::new(), serde_json::json!({}));
        assert_eq!(response.headers()[VARY], "Accept, Signature");
        assert!(!response.headers().contains_key(CACHE_CONTROL));

        let mut authorization_headers = HeaderMap::new();
        authorization_headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer fixture"));
        let response =
            signature_sensitive_activity_response(&authorization_headers, serde_json::json!({}));
        assert!(!response.headers().contains_key(CACHE_CONTROL));

        let mut signature_headers = HeaderMap::new();
        signature_headers.insert("signature", HeaderValue::from_static("fixture"));
        let response =
            signature_sensitive_activity_response(&signature_headers, serde_json::json!({}));
        assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
    }

    #[test]
    fn status_activitypub_responses_follow_rails_cache_policy() {
        let response = signature_sensitive_status_response(
            &HeaderMap::new(),
            serde_json::json!({}),
            "https://fixture.invalid/status/1",
            true,
            true,
            ActivityPubStatusDocument::Note {
                pending_quote: false,
            },
        );
        assert_eq!(response.headers()[VARY], ACTIVITYPUB_STATUS_PUBLIC_VARY);
        assert_eq!(
            response.headers()[CACHE_CONTROL],
            ACTIVITYPUB_STATUS_PUBLIC_CACHE
        );

        for (name, value) in [
            ("authorization", "Bearer fixture"),
            ("signature", "fixture"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(name, HeaderValue::from_static(value));
            let response = signature_sensitive_status_response(
                &headers,
                serde_json::json!({}),
                "https://fixture.invalid/status/1",
                true,
                true,
                ActivityPubStatusDocument::Note {
                    pending_quote: false,
                },
            );
            assert_eq!(response.headers()[VARY], ACTIVITYPUB_STATUS_PUBLIC_VARY);
            assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
        }

        let response = signature_sensitive_status_response(
            &HeaderMap::new(),
            serde_json::json!({}),
            "https://fixture.invalid/status/1",
            true,
            false,
            ActivityPubStatusDocument::Note {
                pending_quote: false,
            },
        );
        assert_eq!(response.headers()[VARY], ACTIVITYPUB_STATUS_PUBLIC_VARY);
        assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);

        let response = signature_sensitive_status_response(
            &HeaderMap::new(),
            serde_json::json!({}),
            "https://fixture.invalid/status/1",
            false,
            true,
            ActivityPubStatusDocument::Note {
                pending_quote: false,
            },
        );
        assert_eq!(response.headers()[VARY], ACTIVITYPUB_STATUS_AUTHORIZED_VARY);
        assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);

        let response = signature_sensitive_status_response(
            &HeaderMap::new(),
            serde_json::json!({}),
            "https://fixture.invalid/status/1",
            true,
            true,
            ActivityPubStatusDocument::Note {
                pending_quote: true,
            },
        );
        assert_eq!(
            response.headers()[CACHE_CONTROL],
            ACTIVITYPUB_STATUS_PENDING_QUOTE_CACHE
        );

        let response = signature_sensitive_status_response(
            &HeaderMap::new(),
            serde_json::json!({}),
            "https://fixture.invalid/status/1",
            true,
            true,
            ActivityPubStatusDocument::Activity,
        );
        assert_eq!(
            response.headers()[CACHE_CONTROL],
            ACTIVITYPUB_STATUS_PUBLIC_CACHE
        );

        let response = signature_sensitive_status_response(
            &HeaderMap::new(),
            serde_json::json!({}),
            "https://fixture.invalid/status/1",
            true,
            false,
            ActivityPubStatusDocument::Activity,
        );
        assert_eq!(
            response.headers()[CACHE_CONTROL],
            ACTIVITYPUB_STATUS_PRIVATE_ACTIVITY_CACHE
        );
    }

    #[test]
    fn status_paperclip_cache_variants_are_isolated_by_viewer_credentials() {
        let (cache_control, vary) =
            paperclip_response_policy(PaperclipAttachment::MediaFile, &HeaderMap::new());
        assert_eq!(cache_control, PAPERCLIP_CACHE);
        assert_eq!(vary, Some(PAPERCLIP_STATUS_VARY));

        for (name, value) in [
            ("authorization", "Bearer fixture"),
            ("authorization", ""),
            ("cookie", "_mastodon_session=fixture"),
            ("signature", "fixture"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(name, HeaderValue::from_static(value));
            let (cache_control, vary) =
                paperclip_response_policy(PaperclipAttachment::MediaThumbnail, &headers);
            assert_eq!(cache_control, PRIVATE_CACHE);
            assert_eq!(vary, Some(PAPERCLIP_STATUS_VARY));
        }

        let (cache_control, vary) =
            paperclip_response_policy(PaperclipAttachment::AccountAvatar, &HeaderMap::new());
        assert_eq!(cache_control, PAPERCLIP_CACHE);
        assert_eq!(vary, None);
    }

    #[test]
    fn activitypub_status_failures_are_not_shared() {
        for (limited_federation, expected_vary) in [
            (false, ACTIVITYPUB_STATUS_PUBLIC_VARY),
            (true, ACTIVITYPUB_STATUS_AUTHORIZED_VARY),
        ] {
            let response = finalize_activitypub_status_response(
                limited_federation,
                error_response(StatusCode::NOT_FOUND, "Not Found"),
            );
            assert_eq!(response.headers()[VARY], expected_vary);
            assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
        }

        let response = finalize_activitypub_status_response(
            false,
            activitypub_status_redirect("https://fixture.invalid/status/1"),
        );
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(response.headers()[VARY], ACTIVITYPUB_STATUS_PUBLIC_VARY);
        assert_eq!(response.headers()[CACHE_CONTROL], PRIVATE_CACHE);
    }

    #[test]
    fn browser_account_documents_escape_values_and_include_csrf_fields() {
        let sign_in = browser_sign_in_document(
            "csrf\"value",
            Some("alice\"@example.invalid"),
            Some("invalid <credentials>"),
            Some("/oauth/authorize?client_id=fixture&state=state"),
        );
        assert!(sign_in.contains("value=\"csrf&quot;value\""));
        assert!(sign_in.contains("alice&quot;@example.invalid"));
        assert!(sign_in.contains("invalid &lt;credentials&gt;"));
        assert!(sign_in.contains("for=\"otp_attempt\""));
        assert!(sign_in.contains("/auth/password/new"));
        assert!(sign_in.contains(
            "name=\"return_to\" value=\"/oauth/authorize?client_id=fixture&amp;state=state\""
        ));
        let reset = browser_password_reset_document(
            "csrf\"value",
            Some("reset<&token"),
            Some("expired <token>"),
        );
        for document in [&sign_in, &reset] {
            assert!(document.contains("<html lang=\"en\">"));
            assert!(document.contains("href=\"/rustodon-assets/rustodon-"));
            assert!(document.contains("referrerpolicy=\"no-referrer\""));
            assert!(document.contains("class=\"rustodon-brand\""));
            assert!(document.contains("<body class=\"rustodon rustodon--compact\">"));
        }
        assert!(sign_in.contains("<p class=\"alert\" role=\"alert\">"));
        assert!(reset.contains("action=\"/auth/password\""));
        assert!(reset.contains("name=\"reset_password_token\" value=\"reset&lt;&amp;token\""));
        assert!(reset.contains("name=\"user[password_confirmation]\""));

        assert_eq!(
            hidden_csrf("csrf\"value"),
            "<input type=\"hidden\" name=\"csrf_token\" value=\"csrf&quot;value\">"
        );
        let options = settings_options(
            &[("public", "Public"), ("private", "Followers only")],
            "private",
        );
        assert!(options.contains("value=\"private\" selected"));
        assert!(!options.contains("value=\"public\" selected"));
    }

    #[tokio::test]
    async fn rustodon_owned_oauth_oob_and_settings_documents_share_the_shell() {
        let consent = oauth_consent_response(
            "Fixture <App>",
            "client<&",
            "urn:ietf:wg:oauth:2.0:oob",
            OAuthResponseMode::Query,
            "read <write>",
            Some("state<&"),
            Some("challenge<&"),
            Some("S256"),
            Some("csrf<&"),
        );
        let consent = axum::body::to_bytes(consent.into_body(), 16 * 1024)
            .await
            .expect("OAuth consent body is readable");
        let consent = String::from_utf8(consent.to_vec()).expect("OAuth consent is UTF-8");
        assert!(consent.contains("href=\"/rustodon-assets/rustodon-"));
        assert!(consent.contains("<body class=\"rustodon rustodon--compact\">"));
        assert!(consent.contains("Authorize Fixture &lt;App&gt;"));
        assert!(consent.contains("method=\"post\" action=\"/oauth/authorize\""));
        assert!(consent.contains("name=\"client_id\" value=\"client&lt;&amp;\""));
        assert!(consent.contains("name=\"approve\" value=\"true\" type=\"submit\""));
        assert!(consent.contains("button button--secondary"));
        assert!(consent.contains("name=\"approve\" value=\"false\" type=\"submit\""));

        let oob = oauth_oob_document("code<&");
        assert!(oob.contains("href=\"/rustodon-assets/rustodon-"));
        assert!(oob.contains("class=\"authorization-code\">code&lt;&amp;</code>"));

        let settings = browser_settings_page(
            "Profile",
            "<form method=\"post\" action=\"/settings/profile\"><input name=\"display_name\" value=\"Alice &amp; Bob\"></form>",
            "csrf<&",
            None,
            Some(SettingsSection::Profile),
        );
        let settings = axum::body::to_bytes(settings.into_body(), 16 * 1024)
            .await
            .expect("settings body is readable");
        let settings = String::from_utf8(settings.to_vec()).expect("settings body is UTF-8");
        assert!(settings.contains("href=\"/rustodon-assets/rustodon-"));
        assert!(settings.contains("<body class=\"rustodon rustodon--settings\">"));
        assert!(settings.contains("class=\"settings-layout\""));
        assert!(settings.contains("href=\"/settings/profile\" aria-current=\"page\""));
        assert!(settings.contains("method=\"post\" action=\"/settings/profile\""));
        assert!(settings.contains("name=\"display_name\" value=\"Alice &amp; Bob\""));

        let delete = browser_delete_form(true, "csrf<&");
        assert!(
            delete.contains("class=\"danger-zone\" method=\"post\" action=\"/settings/delete\"")
        );
        assert!(delete.contains("name=\"csrf_token\" value=\"csrf&lt;&amp;\""));
        assert!(delete.contains("button button--danger"));

        let disable_two_factor = browser_disable_two_factor_form("csrf<&");
        assert!(disable_two_factor.contains(
            "method=\"post\" action=\"/settings/two_factor_authentication_methods/disable\""
        ));
        assert!(disable_two_factor.contains("class=\"danger-zone\""));
        assert!(disable_two_factor.contains("button button--danger"));
    }

    #[tokio::test]
    async fn rustodon_stylesheet_route_and_responsive_contract_are_stable() {
        for contract in [
            "#181820",
            "#21212c",
            "#3a3a50",
            "#5638cc",
            "--secondary-hover:",
            "--secondary-hover: #d8d8e5",
            "--secondary-hover-text: #27272f",
            ":focus-visible",
            "@media (prefers-color-scheme: light)",
            "@media (max-width: 700px)",
            "@media (prefers-reduced-motion: reduce)",
            ".button--secondary",
            ".button--danger",
            "button,\n.button",
            ".alert",
            ".authorization-code",
            ".settings-content code",
            ".recovery-codes",
            ".rustodon-brand",
            ".settings-layout",
            "label > input:not([type=\"checkbox\"])",
            ".settings-content fieldset > div",
            ".danger-zone > :first-child",
            "overflow-wrap: anywhere",
        ] {
            assert!(RUSTODON_STYLESHEET.contains(contract), "missing {contract}");
        }
        assert!(!RUSTODON_STYLESHEET.contains("overflow-x: hidden"));
        assert!(!RUSTODON_STYLESHEET.contains("http://"));
        assert!(!RUSTODON_STYLESHEET.contains("https://"));

        let document = browser_sign_in_document("csrf", None, None, None);
        let stylesheet = document
            .split_once("<link rel=\"stylesheet\" href=\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(href, _)| href)
            .expect("shared shell has a stylesheet link");
        assert!(stylesheet.starts_with("/rustodon-assets/rustodon-"));
        assert!(document.contains(&format!(
            "href=\"{stylesheet}\" referrerpolicy=\"no-referrer\""
        )));
        let hash = stylesheet
            .strip_prefix("/rustodon-assets/rustodon-")
            .and_then(|name| name.strip_suffix(".css"))
            .expect("stylesheet URL has a content hash");
        let expected_hash = Sha256::digest(RUSTODON_STYLESHEET.as_bytes())
            .iter()
            .take(6)
            .fold(String::new(), |mut encoded, byte| {
                let _ = write!(encoded, "{byte:02x}");
                encoded
            });
        assert_eq!(hash, expected_hash);
        assert_eq!(frontend_asset_cache_control(stylesheet), FRONTEND_CACHE);
        assert_eq!(
            frontend_asset_cache_control(&format!("{stylesheet}?source=oauth&state=secret")),
            FRONTEND_CACHE
        );
        assert_eq!(
            frontend_asset_cache_control("/packs/app.css"),
            FRONTEND_CACHE
        );

        let response = rustodon_stylesheet().await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/css; charset=utf-8");
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("stylesheet body is readable");
        assert_eq!(body.as_ref(), RUSTODON_STYLESHEET.as_bytes());
    }

    #[tokio::test]
    async fn oauth_form_post_document_keeps_its_specialized_transport_boundary() {
        let redirect = Url::parse("https://client.invalid/callback").expect("redirect is valid");
        let response =
            oauth_form_post_response(&redirect, &[("code", "code<&"), ("state", "state\"")]);
        let policy = response.headers()["content-security-policy"]
            .to_str()
            .expect("form_post CSP is text");
        assert!(policy.contains("form-action https://client.invalid"));
        assert!(policy.contains("script-src 'sha256-"));
        assert!(!policy.contains("style-src"));
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("form_post body is readable");
        let body = String::from_utf8(body.to_vec()).expect("form_post body is UTF-8");
        assert_eq!(
            body,
            "<!doctype html><meta charset=\"utf-8\"><title>Authorization response</title><form method=\"post\" action=\"https://client.invalid/callback\"><input type=\"hidden\" name=\"code\" value=\"code&lt;&amp;\"><input type=\"hidden\" name=\"state\" value=\"state&quot;\"><noscript><button type=\"submit\">Continue</button></noscript></form><script>document.forms[0].submit();</script>"
        );
        assert!(!body.contains("/rustodon-assets/rustodon-"));
        assert!(!body.contains("class=\"rustodon"));
    }

    #[test]
    fn browser_login_return_targets_are_local_only() {
        assert_eq!(
            valid_browser_return_to(Some("/oauth/authorize?state=fixture")),
            Some("/oauth/authorize?state=fixture")
        );
        for target in [
            "",
            "oauth/authorize",
            "//attacker.invalid/oauth/authorize",
            "/\\attacker.invalid/oauth/authorize",
            "https://attacker.invalid/oauth/authorize",
            "/oauth/authorize\r\nLocation: https://attacker.invalid",
        ] {
            assert_eq!(valid_browser_return_to(Some(target)), None, "{target:?}");
        }
    }

    #[test]
    fn oauth_login_redirect_preserves_authorization_query() {
        let parameters = RackParameters::parse(
            "client_id=fixture-client&redirect_uri=https%3A%2F%2Fclient.invalid%2Fcallback&state=fixture-state",
        )
        .expect("OAuth parameters are valid");
        let response = oauth_authorize_sign_in_redirect(&parameters);
        let location = response.headers()[LOCATION]
            .to_str()
            .expect("redirect location is valid");
        let query = location.split_once('?').map_or("", |(_, query)| query);
        let return_to = url::form_urlencoded::parse(query.as_bytes())
            .find_map(|(name, value)| (name == "return_to").then_some(value.into_owned()));
        assert_eq!(
            return_to.as_deref(),
            Some(
                "/oauth/authorize?client_id=fixture-client&redirect_uri=https%3A%2F%2Fclient.invalid%2Fcallback&state=fixture-state"
            )
        );
    }

    #[test]
    fn browser_html_negotiation_respects_accept_quality_values() {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/html;q=0.8"),
        );
        assert!(accepts_html(&headers));

        headers.insert(ACCEPT, HeaderValue::from_static("text/html;q=0"));
        assert!(!accepts_html(&headers));

        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        assert!(!accepts_html(&headers));
    }

    #[tokio::test]
    async fn browser_form_errors_render_html_and_keep_json_for_api_requests() {
        let signing_key = [7; 32];
        let mut html_headers = HeaderMap::new();
        html_headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
        let response = browser_sign_in_error_response(
            false,
            &signing_key,
            &html_headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_credentials",
            "Invalid email or password.",
            Some("alice@example.invalid"),
            None,
        );
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
        assert!(response.headers().contains_key(SET_COOKIE));
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("sign-in error body is readable");
        let body = String::from_utf8(body.to_vec()).expect("sign-in error is UTF-8");
        assert!(body.contains("Invalid email or password."));
        assert!(body.contains("alice@example.invalid"));
        assert!(body.contains("name=\"csrf_token\""));

        let response = browser_authentication_error_response(
            false,
            &signing_key,
            &html_headers,
            "alice@example.invalid",
            &BrowserAuthenticationError::InvalidCredentials,
            None,
        );
        assert_eq!(response.status(), StatusCode::OK);

        let mut json_headers = HeaderMap::new();
        json_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let response = browser_authentication_error_response(
            false,
            &signing_key,
            &json_headers,
            "alice@example.invalid",
            &BrowserAuthenticationError::InvalidCredentials,
            None,
        );
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let response = browser_sign_in_error_response(
            false,
            &signing_key,
            &json_headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_credentials",
            "Invalid email or password.",
            Some("alice@example.invalid"),
            None,
        );
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("JSON error body is readable");
        assert_eq!(body.as_ref(), br#"{"error":"invalid_credentials"}"#);

        let response = browser_password_reset_error_response(
            false,
            &signing_key,
            &html_headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_reset_token",
            "The reset link is invalid or has expired.",
            Some("reset-token"),
        );
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("password error body is readable");
        let body = String::from_utf8(body.to_vec()).expect("password error is UTF-8");
        assert!(body.contains("Choose a new password"));
        assert!(body.contains("The reset link is invalid or has expired."));
        assert!(body.contains("value=\"reset-token\""));

        let response = browser_confirmation_error_response(
            &html_headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_confirmation_token",
            "The confirmation link is invalid or has expired.",
        );
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("confirmation error body is readable");
        let body = String::from_utf8(body.to_vec()).expect("confirmation error is UTF-8");
        assert!(body.contains("The confirmation link is invalid or has expired."));
        assert!(body.contains("/auth/sign_in"));
        assert!(body.contains("href=\"/rustodon-assets/rustodon-"));
        assert!(body.contains("<body class=\"rustodon rustodon--compact\">"));
        assert!(body.contains("class=\"alert\" role=\"alert\""));
    }

    #[test]
    fn frontend_paths_only_accept_pinned_spa_routes() {
        for path in [
            "/",
            "/about",
            "/home",
            "/@alice/example-status",
            "/notifications_v2",
            "/notifications_v2/requests",
            "/overview/about",
            "/pinned",
            "/collections",
            "/deck",
            "/links",
            "/lists",
            "/start",
            "/statuses",
            "/lists/123/edit",
            "/statuses/123",
            "/terms-of-service/2026-01-01",
        ] {
            assert!(is_frontend_path(path), "{path} should be a frontend path");
        }
        for path in ["/api/v1/unknown", "/.well-known/unknown", "/random"] {
            assert!(
                !is_frontend_path(path),
                "{path} should not be a frontend path"
            );
        }
    }

    #[test]
    fn api_route_ids_preserve_rails_prefix_casting_without_relaxing_other_ids() {
        assert_eq!(
            route_path_id("116844606259201001%3Fjunk"),
            Some(116_844_606_259_201_001)
        );
        assert_eq!(
            route_path_id("116844606259201001?junk"),
            Some(116_844_606_259_201_001)
        );
        assert_eq!(
            route_path_id("+116844606259201001"),
            Some(116_844_606_259_201_001)
        );
        assert_eq!(route_path_id("not-an-id"), None);
        assert_eq!(path_id("116844606259201001?junk"), None);
        assert_eq!(
            activitypub_path_id("-116844606259201001"),
            Some(-116_844_606_259_201_001)
        );
        assert_eq!(activitypub_path_id("+116844606259201001"), None);
        assert_eq!(activitypub_path_id(" 116844606259201001"), None);
    }

    #[test]
    fn media_proxy_accepts_mastodon_path_shapes() {
        assert_eq!(
            media_proxy_path("116844606259201001"),
            Some((116_844_606_259_201_001, false))
        );
        assert_eq!(
            media_proxy_path("116844606259201001/small"),
            Some((116_844_606_259_201_001, true))
        );
        assert_eq!(
            media_proxy_path("116844606259201001/media/small"),
            Some((116_844_606_259_201_001, true))
        );
        assert_eq!(media_proxy_path("not-an-id/original"), None);
    }

    #[test]
    fn remote_media_proxy_accepts_only_supported_media_types() {
        assert!(crate::remote::content_type_allowed(
            Some("image/png"),
            SUPPORTED_MIME_TYPES
        ));
        assert!(!crate::remote::content_type_allowed(
            Some("text/html"),
            SUPPORTED_MIME_TYPES
        ));
        assert!(!crate::remote::content_type_allowed(
            Some("image/svg+xml"),
            SUPPORTED_MIME_TYPES
        ));
    }

    #[test]
    fn remote_account_search_only_resolves_exact_non_local_handles() {
        assert_eq!(
            remote_account_search_handle(Some("@Alice@remote.example"), "local.example", 0),
            Some(("Alice".to_owned(), "remote.example".to_owned()))
        );
        assert_eq!(
            remote_account_search_handle(Some("alice@local.example"), "local.example", 0),
            None
        );
        assert_eq!(
            remote_account_search_handle(Some("alice@remote.example"), "local.example", 1),
            None
        );
        assert_eq!(
            remote_account_search_handle(Some("alice@remote@example"), "local.example", 0),
            None
        );
        assert_eq!(
            remote_account_search_handle(Some("alice@BÜCHER.example."), "local.example", 0),
            Some(("alice".to_owned(), "xn--bcher-kva.example".to_owned()))
        );
        assert_eq!(
            remote_account_search_handle(Some("bad:name@remote.example"), "local.example", 0),
            None
        );
        assert_eq!(
            remote_account_search_handle(Some("alice..bob@remote.example"), "local.example", 0),
            Some(("alice..bob".to_owned(), "remote.example".to_owned()))
        );
    }

    #[test]
    fn remote_signature_domains_distinguish_local_and_remote_key_ids() {
        let origin = Url::parse("https://local.example/").unwrap();
        assert_eq!(
            remote_signature_domain("acct:alice@local.example", "local.example", &origin),
            Ok(None)
        );
        assert_eq!(
            remote_signature_domain(
                "https://local.example/users/alice#main-key",
                "local.example",
                &origin,
            ),
            Ok(None)
        );
        assert_eq!(
            remote_signature_domain("acct:alice@remote.example", "local.example", &origin),
            Ok(Some("remote.example".to_owned()))
        );
        assert_eq!(
            remote_signature_domain(
                "https://remote.example/users/alice",
                "local.example",
                &origin,
            ),
            Ok(Some("remote.example".to_owned()))
        );
        assert_eq!(
            remote_signature_domain(
                "http://remote.example:443/users/alice#main-key",
                "local.example",
                &origin,
            ),
            Ok(Some("remote.example:443".to_owned()))
        );
        assert!(remote_signature_domain("not-a-key", "local.example", &origin).is_err());
    }

    #[test]
    fn uri_only_create_inbox_keys_preserve_personal_delivery_targets() {
        let activity = serde_json::json!({
            "id": "https://remote.example/activities/42",
            "type": "Create",
            "actor": "https://remote.example/users/alice",
            "object": "https://remote.example/statuses/42"
        });
        let base = activitypub_inbox_logical_key(
            &activity,
            br#"{"id":"ignored"}"#,
            "https://remote.example/users/alice",
        );

        assert_eq!(
            activitypub_inbox_delivery_logical_key(&activity, &base, Some(7)),
            activitypub_inbox_delivery_logical_key(&activity, &base, Some(7)),
        );
        assert_ne!(
            activitypub_inbox_delivery_logical_key(&activity, &base, Some(7)),
            activitypub_inbox_delivery_logical_key(&activity, &base, Some(8)),
        );
        assert_ne!(
            activitypub_inbox_delivery_logical_key(&activity, &base, Some(7)),
            activitypub_inbox_delivery_logical_key(&activity, &base, None),
        );
    }

    #[test]
    fn question_inbox_keys_preserve_personal_delivery_targets_for_type_arrays() {
        let activity = serde_json::json!({
            "id": "https://remote.example/activities/question-42",
            "type": ["Create"],
            "actor": "https://remote.example/users/alice",
            "object": {
                "id": "https://remote.example/statuses/42",
                "type": ["Question"]
            }
        });
        let base = activitypub_inbox_logical_key(
            &activity,
            br#"{"id":"ignored"}"#,
            "https://remote.example/users/alice",
        );
        assert_ne!(
            activitypub_inbox_delivery_logical_key(&activity, &base, Some(7)),
            activitypub_inbox_delivery_logical_key(&activity, &base, Some(8)),
        );
    }

    #[test]
    fn activitypub_inbox_activity_ids_are_scoped_to_the_verified_actor() {
        let alice = serde_json::json!({
            "id": "https://remote.example/activities/42",
            "type": "Follow",
            "actor": "https://remote.example/users/alice"
        });
        assert_eq!(
            activitypub_inbox_logical_key(&alice, b"alice", "https://remote.example/users/alice",),
            activitypub_inbox_logical_key(&alice, b"alice", "https://remote.example/users/alice",)
        );
        assert_eq!(
            activitypub_inbox_ordering_key("https://remote.example/users/alice"),
            activitypub_inbox_ordering_key("https://remote.example/users/alice"),
        );
        assert_ne!(
            activitypub_inbox_ordering_key("https://remote.example/users/alice"),
            activitypub_inbox_ordering_key("https://remote.example/users/bob"),
        );
    }

    #[test]
    fn activitypub_inbox_without_an_id_deduplicates_exact_retries() {
        let activity = serde_json::json!({"type": "Delete"});
        let body = br#"{"type":"Delete"}"#;

        assert_eq!(
            activitypub_inbox_logical_key(&activity, body, "key"),
            activitypub_inbox_logical_key(&activity, body, "key")
        );
        assert_ne!(
            activitypub_inbox_logical_key(&activity, body, "key"),
            activitypub_inbox_logical_key(&activity, br#"{"type":"Update"}"#, "key")
        );
        assert_ne!(
            activitypub_inbox_logical_key(&activity, body, "key"),
            activitypub_inbox_logical_key(&activity, body, "actor-2")
        );
    }

    #[test]
    fn activitypub_inbox_body_limit_matches_mastodon() {
        assert_eq!(ACTIVITYPUB_INBOX_BODY_LIMIT_BYTES, 1024 * 1024);
    }

    #[test]
    fn poll_request_parser_uses_rails_expiry_coercion_for_form_and_json() {
        let form = RackParameters::parse(
            "poll[options][]=Tea&poll[options][]=Coffee&poll[expires_in]=%20%20%2B300junk&poll[multiple]=true",
        )
        .expect("valid form poll");
        let poll = status_poll(&form)
            .expect("form poll should parse")
            .expect("form poll should be present");
        assert_eq!(poll.options, ["Tea", "Coffee"]);
        assert_eq!(poll.expires_in, 300);
        assert!(poll.multiple);

        let json = RackParameters::from_json(&json!({
            "poll": {
                "options": ["Tea", "Coffee"],
                "expires_in": 300.9,
                "hide_totals": true
            }
        }));
        let poll = status_poll(&json)
            .expect("JSON poll should parse")
            .expect("JSON poll should be present");
        assert_eq!(poll.expires_in, 300);
        assert!(poll.hide_totals);

        for value in ["abc", "+", "299junk", "-300xyz"] {
            let parameters = RackParameters::from_json(&json!({
                "poll": { "options": ["Tea", "Coffee"], "expires_in": value }
            }));
            assert!(matches!(
                status_poll(&parameters),
                Err(WriteError::Validation("Expires at is too soon"))
            ));
        }
        for value in [json!(null), json!(""), json!("   ")] {
            let parameters = RackParameters::from_json(&json!({
                "poll": { "options": ["Tea", "Coffee"], "expires_in": value }
            }));
            assert!(matches!(
                status_poll(&parameters),
                Err(WriteError::Validation("Expires at can't be blank"))
            ));
        }
        let malformed = RackParameters::from_json(&json!({
            "poll": { "options": ["Tea", "Coffee"], "expires_in": true }
        }));
        assert!(matches!(
            status_poll(&malformed),
            Err(WriteError::InvalidInput("Invalid poll expires_in"))
        ));
    }

    #[test]
    fn signed_poll_refresh_requires_supported_json_ld_status_objects() {
        let status_uri = "https://remote.example/statuses/1";
        for status_type in [json!("Question"), json!("Note"), json!(["Object", "Note"])] {
            let valid = json!({
                "@context": [
                    "https://www.w3.org/ns/activitystreams",
                    {"votersCount": "http://joinmastodon.org/ns#votersCount"}
                ],
                "id": status_uri,
                "type": status_type
            });
            assert!(remote_poll_refresh_document_is_supported(
                &valid, status_uri
            ));
        }

        for invalid in [
            json!({"id": status_uri, "type": "Question"}),
            json!({"@context": "https://example.invalid/context", "id": status_uri, "type": "Question"}),
            json!({"@context": {"as": "https://www.w3.org/ns/activitystreams#"}, "id": status_uri, "type": "Note"}),
            json!({"@context": "https://www.w3.org/ns/activitystreams", "id": "https://remote.example/statuses/2", "type": "Question"}),
            json!({"@context": "https://www.w3.org/ns/activitystreams", "id": status_uri, "type": "Person"}),
        ] {
            assert!(!remote_poll_refresh_document_is_supported(
                &invalid, status_uri
            ));
        }
    }

    #[test]
    fn poll_vote_request_parser_matches_ruby_integer_coercion() {
        let form = RackParameters::parse(
            "choices[]=%20%2B1%20&choices[]=-2&choices[]=2147483647&choices[]=-2147483648",
        )
        .expect("valid choices form");
        assert_eq!(
            poll_vote_choices(&form),
            Ok(vec![1, -2, i32::MAX, i32::MIN])
        );
        let json = RackParameters::from_json(&json!({
            "choices": [0, 1.9, -2.9, 2_147_483_647.9, -2_147_483_648.9]
        }));
        assert_eq!(
            poll_vote_choices(&json),
            Ok(vec![0, 1, -2, i32::MAX, i32::MIN])
        );

        for value in [json!(null), json!([])] {
            let parameters = RackParameters::from_json(&json!({ "choices": value }));
            assert_eq!(
                poll_vote_choices(&parameters),
                Err(PollVoteChoicesError::Missing)
            );
        }
        assert_eq!(
            poll_vote_choices(&RackParameters::default()),
            Err(PollVoteChoicesError::Missing)
        );
        for value in [
            json!("0"),
            json!(["1.5"]),
            json!(["nope"]),
            json!([true]),
            json!([null]),
            json!([2_147_483_648_i64]),
            json!([-2_147_483_649_i64]),
            json!([2_147_483_648.0]),
            json!([-2_147_483_649.0]),
        ] {
            let parameters = RackParameters::from_json(&json!({ "choices": value }));
            assert_eq!(
                poll_vote_choices(&parameters),
                Err(PollVoteChoicesError::Invalid)
            );
        }
    }

    #[test]
    fn status_idempotency_binds_account_reply_and_quote_targets() {
        let fingerprint = |reply, quote, poll: Option<&PollCreate>| {
            status_idempotency_fingerprint(
                "fixture status",
                &[],
                None,
                Some("public"),
                Some("en"),
                None,
                Some(false),
                reply,
                quote,
                poll,
            )
        };
        let base = fingerprint(None, None, None);
        assert_ne!(base, fingerprint(Some(42), None, None));
        assert_ne!(base, fingerprint(None, Some(42), None));
        assert_ne!(
            fingerprint(None, Some(42), None),
            fingerprint(None, Some(43), None)
        );
        let trimmed = status_idempotency_fingerprint(
            " fixture status ",
            &[],
            Some(" "),
            Some("public"),
            Some("en"),
            None,
            Some(false),
            None,
            None,
            None,
        );
        assert_eq!(base, trimmed);
        let poll = PollCreate {
            options: vec!["Tea".to_owned(), "Coffee".to_owned()],
            expires_in: 300,
            multiple: false,
            hide_totals: false,
        };
        assert_ne!(base, fingerprint(None, None, Some(&poll)));
        for changed in [
            PollCreate {
                options: vec!["Tea".to_owned(), "Water".to_owned()],
                ..poll.clone()
            },
            PollCreate {
                expires_in: 301,
                ..poll.clone()
            },
            PollCreate {
                multiple: true,
                ..poll.clone()
            },
            PollCreate {
                hide_totals: true,
                ..poll.clone()
            },
        ] {
            assert_ne!(
                fingerprint(None, None, Some(&poll)),
                fingerprint(None, None, Some(&changed))
            );
        }
        assert_ne!(
            base,
            status_idempotency_fingerprint(
                "fixture status",
                &[],
                None,
                Some("public"),
                Some("en"),
                Some("nobody"),
                Some(false),
                None,
                None,
                None,
            )
        );
        assert_eq!(status_idempotency_scope(101), "status:create:101");
        assert_ne!(status_idempotency_scope(101), status_idempotency_scope(102));
    }

    #[test]
    fn status_media_ids_require_an_array_of_decimal_ids() {
        let parameters =
            RackParameters::parse("media_ids%5B%5D=12&media_ids%5B%5D=-4&media_ids%5B%5D=12")
                .unwrap();
        assert_eq!(status_media_ids(&parameters), Ok(vec![12, -4, 12]));
        assert_eq!(
            status_media_ids(&RackParameters::parse("media_ids=12").unwrap()),
            Err(())
        );
        assert_eq!(
            status_media_ids(&RackParameters::parse("media_ids%5B%5D=nope").unwrap()),
            Err(())
        );
    }

    #[test]
    fn status_media_attributes_parse_nested_description_and_focus() {
        let parameters = RackParameters::parse(
            "media_attributes%5B%5D%5Bid%5D=12&media_attributes%5B%5D%5Bdescription%5D=alt+text&media_attributes%5B%5D%5Bfocus%5D=0.25%2C-0.5",
        )
        .unwrap();
        assert_eq!(
            status_media_attributes(&parameters),
            Ok(Some(vec![StatusMediaAttributeUpdate {
                id: 12,
                description: AccountProfileValue::Value("alt text".to_owned()),
                focus: AccountProfileValue::Value(MediaFocus { x: 0.25, y: -0.5 }),
            }]))
        );
    }

    #[test]
    fn report_id_parameters_accept_scalar_or_array_decimal_ids() {
        let scalar = RackParameters::parse("status_ids=12").unwrap();
        let array = RackParameters::parse("status_ids%5B%5D=12&status_ids%5B%5D=-4").unwrap();
        let invalid = RackParameters::parse("status_ids%5B%5D=nope").unwrap();
        assert_eq!(report_id_parameter(&scalar, "status_ids"), Ok(vec![12]));
        assert_eq!(report_id_parameter(&array, "status_ids"), Ok(vec![12, -4]));
        assert_eq!(report_id_parameter(&invalid, "status_ids"), Err(()));
    }

    #[test]
    fn report_forward_domains_preserve_omission_and_normalize_arrays() {
        assert_eq!(
            report_forward_domains_parameter(&RackParameters::parse("").unwrap()),
            Ok(None)
        );
        assert_eq!(
            report_forward_domains_parameter(
                &RackParameters::parse(
                    "forward_to_domains%5B%5D=Remote.Example.&forward_to_domains%5B%5D=remote.example"
                )
                .unwrap()
            ),
            Ok(Some(vec!["remote.example".to_owned()]))
        );
        assert_eq!(
            report_forward_domains_parameter(
                &RackParameters::parse("forward_to_domains=remote.example").unwrap()
            ),
            Err(())
        );
    }

    #[test]
    fn boolean_parameters_cast_json_numbers_like_rails() {
        let false_parameters = RackParameters::from_json(&serde_json::json!({ "value": 0 }));
        let true_parameters = RackParameters::from_json(&serde_json::json!({ "value": 1 }));
        let float_parameters = RackParameters::from_json(&serde_json::json!({ "value": 0.0 }));
        assert!(!boolean_parameter(&false_parameters, "value"));
        assert!(boolean_parameter(&true_parameters, "value"));
        assert!(boolean_parameter(&float_parameters, "value"));
    }

    #[test]
    fn activitypub_negotiation_honors_media_parameters_and_quality() {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("text/html, application/activity+json; q=0"),
        );
        assert!(!accepts_activitypub(&headers));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json; charset=utf-8; q=0.5"),
        );
        assert!(accepts_activitypub(&headers));
        headers.insert(ACCEPT, HeaderValue::from_static("application/ld+json;Q=0"));
        assert!(!accepts_activitypub(&headers));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/activity+json;q=0.1, text/html;q=0.9"),
        );
        assert!(!accepts_activitypub(&headers));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("text/html;q=0.9, application/activity+json;q=1"),
        );
        assert!(accepts_activitypub(&headers));
    }

    #[test]
    fn federation_authority_matching_preserves_non_default_ports() {
        let origin = Url::parse("https://example.test:8443/").expect("valid origin");
        assert_eq!(
            federation_url_authority(&origin).as_deref(),
            Some("example.test:8443")
        );
        let default_port = Url::parse("https://example.test:443/").expect("valid origin");
        assert_eq!(
            federation_url_authority(&default_port).as_deref(),
            Some("example.test")
        );
    }

    #[test]
    fn forwarded_metadata_is_accepted_only_from_trusted_peers() {
        let trusted = ["10.0.0.0/8".parse().unwrap()];
        let headers = HeaderMap::from_iter([
            (
                "x-forwarded-for".parse().unwrap(),
                "198.51.100.8, 10.1.2.3".parse().unwrap(),
            ),
            (
                "x-forwarded-proto".parse().unwrap(),
                "https".parse().unwrap(),
            ),
            (
                "x-forwarded-host".parse().unwrap(),
                "social.example".parse().unwrap(),
            ),
        ]);
        let peer = "10.1.2.3:4321".parse().unwrap();
        let metadata = request_metadata(peer, &headers, &trusted).unwrap();
        assert_eq!(
            metadata.client_ip,
            "198.51.100.8".parse::<IpAddr>().unwrap()
        );
        assert_eq!(metadata.scheme.as_deref(), Some("https"));
        assert_eq!(metadata.host.as_deref(), Some("social.example"));

        let untrusted =
            request_metadata("203.0.113.7:1234".parse().unwrap(), &headers, &trusted).unwrap();
        assert_eq!(
            untrusted.client_ip,
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
        assert!(untrusted.scheme.is_none());
        assert!(untrusted.host.is_none());

        let unconfigured = request_metadata(peer, &headers, &[]).unwrap();
        assert_eq!(unconfigured.client_ip, peer.ip());
        assert!(unconfigured.scheme.is_none());
        assert!(unconfigured.host.is_none());
    }

    #[test]
    fn duplicate_forwarded_headers_fail_closed() {
        let mut headers = HeaderMap::new();
        headers.append(
            "x-forwarded-host",
            HeaderValue::from_static("social.example"),
        );
        headers.append(
            "x-forwarded-host",
            HeaderValue::from_static("attacker.example"),
        );
        assert!(header_text(&headers, "x-forwarded-host").is_err());
    }

    #[test]
    fn malformed_forwarding_from_a_trusted_peer_fails_closed() {
        let trusted = ["10.0.0.0/8".parse().unwrap()];
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        assert!(request_metadata("10.1.2.3:1".parse().unwrap(), &headers, &trusted).is_err());
    }

    #[test]
    fn request_hosts_preserve_domain_and_explicit_port_boundaries() {
        let allowed = ["social.example".to_owned(), "web.example:8443".to_owned()];
        assert!(request_host_matches_allowed(
            "social.example:18790",
            &allowed
        ));
        assert!(request_host_matches_allowed(
            "SOCIAL.EXAMPLE:18790",
            &allowed
        ));
        assert!(request_host_matches_allowed("web.example:8443", &allowed));
        assert!(!request_host_matches_allowed("web.example", &allowed));
        assert!(!request_host_matches_allowed("web.example:443", &allowed));
        assert!(!request_host_matches_allowed(
            "social.example.evil:18790",
            &allowed
        ));
        assert!(!request_host_matches_allowed(
            "social.example@evil:18790",
            &allowed
        ));
        assert!(!request_host_matches_allowed(
            "social.example/path",
            &allowed
        ));

        let default_port = ["default.example:443".to_owned()];
        assert!(request_host_matches_allowed(
            "default.example",
            &default_port
        ));
        assert!(request_host_matches_allowed(
            "default.example:443",
            &default_port
        ));
        assert!(!request_host_matches_allowed(
            "default.example:8443",
            &default_port
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn api_route_inventory_is_unique_and_declares_protocol_contracts() {
        assert_eq!(API_ROUTE_INVENTORY.len(), 131);
        assert_eq!(REST_BODY_LIMIT_BYTES, 103_809_024);
        assert_eq!(
            API_ROUTE_INVENTORY
                .iter()
                .map(|route| {
                    (
                        route.path,
                        match route.method {
                            ApiMethod::Get => "GET",
                            ApiMethod::Post => "POST",
                            ApiMethod::Delete => "DELETE",
                            ApiMethod::Patch => "PATCH",
                            ApiMethod::Put => "PUT",
                        },
                    )
                })
                .collect::<BTreeSet<_>>()
                .len(),
            API_ROUTE_INVENTORY.len()
        );
        assert_eq!(
            api_route("/api/v1/instance/translation_languages")
                .expect("disabled translation response is inventoried")
                .support,
            ApiRouteSupport::DisabledResponse
        );
        assert_eq!(
            api_route("/api/v1/timelines/list/9001")
                .expect("dynamic list route matches")
                .pagination,
            PaginationContract::StatusId
        );
        assert_eq!(
            api_route("/api/v1/accounts/relationships")
                .expect("relationships are inventoried")
                .authentication,
            ApiAuthentication::Required(READ_FOLLOWS.as_slice())
        );
        assert_eq!(
            api_route("/api/v1/accounts/update_credentials")
                .expect("profile updates are inventoried")
                .authentication,
            ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice())
        );
        for path in ["/api/v1/profile/avatar", "/api/v1/profile/header"] {
            assert_eq!(
                api_route(path)
                    .expect("profile media deletion is inventoried")
                    .authentication,
                ApiAuthentication::Required(WRITE_ACCOUNTS.as_slice())
            );
        }
        for path in ["/api/v1/media", "/api/v1/media/9001", "/api/v2/media"] {
            assert_eq!(
                api_route(path)
                    .expect("media API routes are inventoried")
                    .authentication,
                ApiAuthentication::Required(WRITE_MEDIA.as_slice())
            );
        }
        assert_eq!(
            api_route("/api/v1/media/9001")
                .expect("media update route is inventoried")
                .authentication,
            ApiAuthentication::Required(WRITE_MEDIA.as_slice())
        );
        assert_eq!(
            API_ROUTE_INVENTORY
                .iter()
                .find(|route| route.path == "/api/v1/media/{id}" && route.method == ApiMethod::Patch)
                .expect("media PATCH route is inventoried")
                .method,
            ApiMethod::Patch
        );
        assert_eq!(
            API_ROUTE_INVENTORY
                .iter()
                .filter(|route| route.method == ApiMethod::Put)
                .count(),
            6
        );
        assert_eq!(
            API_ROUTE_INVENTORY
                .iter()
                .find(|route| {
                    route.path == "/api/v1/statuses/{id}/interaction_policy"
                        && route.method == ApiMethod::Put
                })
                .expect("status interaction-policy PUT is inventoried")
                .authentication,
            ApiAuthentication::Required(WRITE_STATUSES.as_slice())
        );
        assert_eq!(
            API_ROUTE_INVENTORY
                .iter()
                .find(|route| {
                    route.path == "/api/v1/statuses/{quoted_status_id}/quotes/{id}/revoke"
                        && route.method == ApiMethod::Post
                })
                .expect("frontend quote revoke POST is inventoried")
                .authentication,
            ApiAuthentication::Required(WRITE_STATUSES.as_slice())
        );
        assert_eq!(
            API_ROUTE_INVENTORY
                .iter()
                .filter(|route| route.method == ApiMethod::Post)
                .count(),
            42
        );
        assert!(api_route("/api/v1/markers").is_some());
    }

    #[test]
    fn v1_required_api_routes_are_inventoried_with_explicit_support() {
        for (index, route) in V1_REQUIRED_API_ROUTES.iter().enumerate() {
            assert!(
                !V1_REQUIRED_API_ROUTES[..index].contains(route),
                "v1 route requirement is duplicated: {route:?}"
            );
        }
        for &(path, method, expected_support) in V1_REQUIRED_API_ROUTES {
            let route = API_ROUTE_INVENTORY
                .iter()
                .find(|route| route.path == path && route.method == method)
                .unwrap_or_else(|| {
                    panic!("v1 route is missing from the inventory: {method:?} {path}")
                });
            assert_eq!(
                route.support, expected_support,
                "v1 route has the wrong support declaration: {method:?} {path}"
            );
        }
    }

    #[test]
    fn notification_stream_targets_respect_scopes_and_merged_events() {
        let mut subscriptions = StreamingSubscriptions::default();
        subscriptions.insert(StreamName::User.into(), 0);
        subscriptions.insert(StreamName::UserNotification.into(), 0);
        let event = StreamEvent {
            id: 1,
            account_id: 101,
            event: "notification".to_owned(),
            object_id: 42,
            before: None,
            after: None,
        };
        assert_eq!(
            stream_account_targets(
                &event,
                &OAuthScopes::parse(Some("read:statuses")),
                &subscriptions,
            ),
            Vec::new()
        );
        assert_eq!(
            stream_account_targets(&event, &OAuthScopes::parse(Some("read")), &subscriptions,),
            vec![StreamName::User.into(), StreamName::UserNotification.into()]
        );

        let merged = StreamEvent {
            event: "notifications_merged".to_owned(),
            ..event.clone()
        };
        assert_eq!(
            stream_account_targets(&merged, &OAuthScopes::parse(Some("read")), &subscriptions,),
            vec![StreamName::User.into(), StreamName::UserNotification.into()]
        );

        let status_update = StreamEvent {
            event: STATUS_UPDATE_NOTIFICATION_EVENT.to_owned(),
            ..event
        };
        assert_eq!(
            stream_account_targets(
                &status_update,
                &OAuthScopes::parse(Some("read")),
                &subscriptions,
            ),
            vec![StreamName::User.into(), StreamName::UserNotification.into()]
        );
        assert_eq!(
            stream_protocol_event(STATUS_UPDATE_NOTIFICATION_EVENT),
            "status.update"
        );
    }

    #[test]
    fn conversation_stream_targets_use_the_direct_subscription() {
        let mut subscriptions = StreamingSubscriptions::default();
        subscriptions.insert(StreamName::Direct.into(), 0);
        let event = StreamEvent {
            id: 1,
            account_id: 101,
            event: "conversation".to_owned(),
            object_id: 42,
            before: None,
            after: None,
        };
        assert_eq!(
            stream_account_targets(
                &event,
                &OAuthScopes::parse(Some("read:statuses")),
                &subscriptions,
            ),
            vec![StreamName::Direct.into()]
        );

        let mut user_subscription = StreamingSubscriptions::default();
        user_subscription.insert(StreamName::User.into(), 0);
        assert_eq!(
            stream_account_targets(
                &event,
                &OAuthScopes::parse(Some("read")),
                &user_subscription,
            ),
            Vec::new()
        );
    }

    #[test]
    fn streaming_authentication_accepts_pinned_token_locations() {
        let mut subprotocol = HeaderMap::new();
        subprotocol.insert(
            SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static("protocol-token"),
        );
        let (protocol_headers, protocol) = streaming_credentials(&subprotocol, None);
        assert_eq!(protocol_headers[AUTHORIZATION], "Bearer protocol-token");
        assert_eq!(protocol.as_deref(), Some("protocol-token"));

        let (query_headers, query_protocol) =
            streaming_credentials(&subprotocol, Some("access_token=query%2Btoken"));
        assert_eq!(query_headers[AUTHORIZATION], "Bearer query+token");
        assert_eq!(query_protocol, None);

        let (empty_query_headers, empty_query_protocol) =
            streaming_credentials(&subprotocol, Some("access_token="));
        assert_eq!(empty_query_headers[AUTHORIZATION], "Bearer protocol-token");
        assert_eq!(empty_query_protocol.as_deref(), Some("protocol-token"));

        let mut explicit = subprotocol.clone();
        explicit.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer explicit-token"),
        );
        let (explicit_headers, explicit_protocol) =
            streaming_credentials(&explicit, Some("access_token=query-token"));
        assert_eq!(explicit_headers[AUTHORIZATION], "Bearer explicit-token");
        assert_eq!(explicit_protocol, None);
    }

    #[test]
    fn streaming_paths_select_supported_initial_streams() {
        for (path, stream) in [
            ("/api/v1/streaming/user", "user"),
            ("/api/v1/streaming/user/notification", "user:notification"),
            ("/api/v1/streaming/direct", "direct"),
            ("/api/v1/streaming/public", "public"),
            ("/api/v1/streaming/public/local", "public:local"),
            ("/api/v1/streaming/public/remote", "public:remote"),
            ("/api/v1/streaming/hashtag", "hashtag"),
            ("/api/v1/streaming/hashtag/local", "hashtag:local"),
            ("/api/v1/streaming/list", "list"),
        ] {
            assert_eq!(streaming_path_stream(path, false), Some(stream));
        }
        assert_eq!(
            streaming_path_stream("/api/v1/streaming/public", true),
            Some("public:media")
        );
        assert_eq!(
            streaming_path_stream("/api/v1/streaming/public/local", true),
            Some("public:local:media")
        );
        assert_eq!(
            streaming_path_stream("/api/v1/streaming/public/remote", true),
            Some("public:remote:media")
        );
        assert_eq!(
            streaming_path_stream("/api/v1/streaming/unknown", false),
            None
        );
    }

    #[test]
    fn delete_snapshots_route_all_nine_timeline_variants_without_status_rows() {
        let snapshot = TimelineRouteSnapshot {
            public: true,
            hashtag: true,
            local: true,
            had_media: true,
            language: Some("en".to_owned()),
            tags: vec!["rustlang".to_owned()],
            lists: vec![crate::streaming::TimelineListRoute {
                account_id: 101,
                list_id: 7,
            }],
        };
        for subscription in [
            Subscription::from(StreamName::Public),
            Subscription::from(StreamName::PublicMedia),
            Subscription::from(StreamName::PublicLocal),
            Subscription::from(StreamName::PublicLocalMedia),
            Subscription::new(StreamName::Hashtag, Some("RustLang".to_owned())),
            Subscription::new(StreamName::HashtagLocal, Some("RustLang".to_owned())),
            Subscription::new(StreamName::List, Some("7".to_owned())),
        ] {
            assert!(timeline_delete_snapshot_matches(
                &snapshot,
                &subscription,
                101,
                true,
            ));
        }
        for subscription in [
            Subscription::from(StreamName::PublicRemote),
            Subscription::from(StreamName::PublicRemoteMedia),
        ] {
            assert!(!timeline_delete_snapshot_matches(
                &snapshot,
                &subscription,
                101,
                true,
            ));
        }

        let remote = TimelineRouteSnapshot {
            local: false,
            ..snapshot
        };
        for subscription in [
            Subscription::from(StreamName::PublicRemote),
            Subscription::from(StreamName::PublicRemoteMedia),
        ] {
            assert!(timeline_delete_snapshot_matches(
                &remote,
                &subscription,
                101,
                true,
            ));
        }
        assert!(!timeline_delete_snapshot_matches(
            &remote,
            &Subscription::new(StreamName::List, Some("7".to_owned())),
            102,
            true,
        ));
        assert!(!timeline_delete_snapshot_matches(
            &remote,
            &Subscription::from(StreamName::Public),
            101,
            false,
        ));
    }

    #[test]
    fn timeline_membership_transitions_select_nondestructive_protocol_events() {
        assert_eq!(
            timeline_protocol_event("status.update", false, true),
            Some("update")
        );
        assert_eq!(
            timeline_protocol_event("status.update", true, true),
            Some("status.update")
        );
        assert_eq!(
            timeline_protocol_event("status.update", true, false),
            Some("status.update"),
            "a route-only exit must not globally delete the status from other open timelines"
        );
        assert_eq!(
            timeline_protocol_event("delete", true, false),
            Some("delete")
        );
        assert_eq!(timeline_protocol_event("delete", false, false), None);
    }

    #[test]
    fn timeline_replay_freshness_only_suppresses_wire_level_creates() {
        assert_eq!(
            timeline_replay_protocol_event("status.update", false, true, 10, 10),
            None,
            "a historical route entry must not prepend a REST-visible status"
        );
        assert_eq!(
            timeline_replay_protocol_event("status.update", false, true, 11, 10),
            Some("update"),
            "a route entry racing the subscribe boundary must be replayed"
        );
        assert_eq!(
            timeline_replay_protocol_event("status.update", true, true, 10, 10),
            Some("status.update"),
            "idempotent edits remain necessary for reconnect convergence"
        );
        assert_eq!(
            timeline_replay_protocol_event("delete", true, false, 10, 10),
            Some("delete"),
            "actual deletes remain necessary for reconnect convergence"
        );
    }

    #[test]
    fn dynamic_subscription_uses_its_own_create_freshness_boundary() {
        assert_eq!(subscription_create_after(10, 20, false), 10);
        assert_eq!(
            subscription_create_after(10, 20, true),
            20,
            "a later multiplexed subscription must not inherit the socket cursor"
        );
    }

    #[test]
    fn streaming_subscription_set_is_bounded() {
        let mut subscriptions = StreamingSubscriptions::default();
        for index in 0..STREAM_MAX_SUBSCRIPTIONS {
            let subscription = Subscription::new(StreamName::Hashtag, Some(format!("tag{index}")));
            assert!(subscriptions.has_capacity_for(&subscription));
            subscriptions.insert(subscription, 0);
        }
        assert!(subscriptions.has_capacity_for(&Subscription::new(
            StreamName::Hashtag,
            Some("tag0".to_owned())
        )));
        assert!(!subscriptions.has_capacity_for(&Subscription::new(
            StreamName::Hashtag,
            Some("one-too-many".to_owned())
        )));
    }

    #[tokio::test]
    async fn multipart_parameters_preserve_uploaded_profile_files() {
        let boundary = "rustodon-test-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"display_name\"\r\n\r\nAlice\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"avatar.gif\"\r\nContent-Type: image/gif\r\n\r\nGIF89a\r\n--{boundary}--\r\n"
        );
        let request = Request::builder()
            .uri("/api/v1/accounts/update_credentials")
            .header(
                CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body.clone()))
            .expect("multipart request is valid");
        assert_eq!(
            multipart_boundary(request.headers()[CONTENT_TYPE].to_str().unwrap()),
            Some(boundary.to_owned())
        );
        let raw_body = body.as_bytes();
        let marker = format!("--{boundary}");
        let header_start = marker.len() + 2;
        let header_end = find_bytes(&raw_body[header_start..], b"\r\n\r\n").unwrap();
        assert!(
            parse_multipart_headers(&raw_body[header_start..header_start + header_end]).is_ok()
        );
        let mut request = bounded_request(request, REST_BODY_LIMIT_BYTES)
            .await
            .expect("request is within the body limit");
        merge_request_parameters(&mut request).expect("multipart parameters parse");
        let parameters = request
            .extensions()
            .get::<RackParameters>()
            .expect("parameters are attached");
        assert!(matches!(
            parameters.get("display_name"),
            Some(RackValue::Scalar(value)) if value == "Alice"
        ));
        assert!(matches!(
            parameters.get("avatar"),
            Some(RackValue::Upload(upload))
                if upload.file_name == "avatar.gif"
                    && upload.content_type == "image/gif"
                    && upload.bytes == b"GIF89a"
        ));

        let removal = RackParameters::parse("avatar").expect("empty file fields parse");
        assert!(matches!(
            profile_media_change(&removal, "avatar", PaperclipAttachment::AccountAvatar, 101)
                .expect("empty file removes the avatar")
                .update,
            AccountMediaUpdate::Remove
        ));
    }

    #[test]
    fn cors_preflight_matches_supported_routes() {
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
        headers.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("GET"),
        );
        headers.insert(
            ACCESS_CONTROL_REQUEST_HEADERS,
            HeaderValue::from_static("authorization, x-client"),
        );
        let response = cors_preflight_response("/api/v1/timelines/home", &headers)
            .expect("implemented API route accepts preflight");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_METHODS],
            "POST, PUT, DELETE, GET, PATCH, OPTIONS"
        );
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_HEADERS],
            "authorization, x-client"
        );
        assert!(!response.headers().contains_key(VARY));
        assert!(cors_preflight_response("/api/v1/markers", &headers).is_some());
        assert!(cors_preflight_response("/api/v1/accounts/search", &headers).is_some());
        assert!(cors_preflight_response("/api/v1/accounts/familiar_followers", &headers).is_some());
        assert!(cors_preflight_response("/api/v1/accounts/search/statuses", &headers).is_some());
        headers.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("POST"),
        );
        assert!(cors_preflight_response("/api/v1/timelines/home", &headers).is_some());
        headers.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("GET"),
        );
        assert!(cors_preflight_response("/.well-known/nodeinfo", &headers).is_some());
        assert!(cors_preflight_response("/users/alice", &headers).is_some());
        assert!(cors_preflight_response("/@alice", &headers).is_some());
        assert!(cors_preflight_response("/oauth/authorize", &headers).is_none());
        headers.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("POST"),
        );
        assert!(cors_preflight_response("/oauth/token", &headers).is_some());
        assert!(cors_preflight_response("/oauth/revoke", &headers).is_some());
        assert!(cors_preflight_response("/oauth/userinfo", &headers).is_some());
        headers.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("GET"),
        );
        assert!(cors_preflight_response("/oauth/userinfo", &headers).is_some());
    }

    #[test]
    fn response_finalization_applies_cache_vary_and_cors_by_route() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
        let instance = finalize_api_response(
            "/api/v2/instance",
            &request_headers,
            json_response(StatusCode::OK, b"{}".to_vec()),
        );
        assert_eq!(
            instance.headers()[CACHE_CONTROL],
            "max-age=300, public, stale-while-revalidate=30, stale-if-error=86400"
        );
        assert_eq!(instance.headers()[VARY], "Accept, Origin");
        assert_eq!(instance.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert!(
            instance.headers()[ACCESS_CONTROL_EXPOSE_HEADERS]
                .to_str()
                .expect("static CORS headers are text")
                .contains("Link")
        );

        request_headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer fixture"));
        let account = finalize_api_response(
            "/api/v1/accounts/1",
            &request_headers,
            json_response(StatusCode::OK, b"{}".to_vec()),
        );
        assert_eq!(account.headers()[CACHE_CONTROL], "private, no-store");
        assert_eq!(account.headers()[VARY], "Authorization, Origin");

        let mut activity = Response::new(Body::empty());
        activity
            .headers_mut()
            .insert(VARY, HeaderValue::from_static("Accept, Signature"));
        let activity = finalize_external_cors_response(
            "/users/alice",
            &Method::GET,
            &request_headers,
            activity,
        );
        assert_eq!(activity.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(
            activity.headers()[ACCESS_CONTROL_EXPOSE_HEADERS],
            CORS_EXPOSE_HEADERS
        );
        assert_eq!(activity.headers()[VARY], "Accept, Signature");
        assert!(
            !finalize_external_cors_response(
                "/oauth/authorize",
                &Method::GET,
                &request_headers,
                Response::new(Body::empty())
            )
            .headers()
            .contains_key(ACCESS_CONTROL_ALLOW_ORIGIN)
        );
    }

    #[test]
    fn parameter_bearer_tokens_fill_missing_or_non_bearer_authorization_headers() {
        let mut request = Request::builder()
            .uri("/api/v1/apps/verify_credentials")
            .body(Body::empty())
            .expect("request is valid");
        request.extensions_mut().insert(
            RackParameters::parse("bearer_token=fixture-bearer-token-v4-6-5")
                .expect("bearer parameter is valid"),
        );
        inject_parameter_bearer(&mut request);
        assert_eq!(
            request.headers()[AUTHORIZATION],
            "Bearer fixture-bearer-token-v4-6-5"
        );

        let mut request = Request::builder()
            .uri("/api/v1/apps/verify_credentials")
            .header(AUTHORIZATION, "Bearer explicit")
            .body(Body::empty())
            .expect("request is valid");
        request.extensions_mut().insert(
            RackParameters::parse("access_token=fixture-bearer-token-v4-6-5")
                .expect("access parameter is valid"),
        );
        inject_parameter_bearer(&mut request);
        assert_eq!(request.headers()[AUTHORIZATION], "Bearer explicit");

        let mut request = Request::builder()
            .uri("/api/v1/apps/verify_credentials")
            .header(AUTHORIZATION, "Basic Y2xpZW50OnNlY3JldA==")
            .body(Body::empty())
            .expect("request is valid");
        request.extensions_mut().insert(
            RackParameters::parse("access_token=fixture-bearer-token-v4-6-5")
                .expect("access parameter is valid"),
        );
        inject_parameter_bearer(&mut request);
        assert_eq!(
            request.headers()[AUTHORIZATION],
            "Bearer fixture-bearer-token-v4-6-5"
        );

        let mut request = Request::builder()
            .uri("/api/v1/apps/verify_credentials")
            .header(AUTHORIZATION, "Bearer first")
            .body(Body::empty())
            .expect("request is valid");
        request
            .headers_mut()
            .append(AUTHORIZATION, HeaderValue::from_static("Bearer second"));
        request.extensions_mut().insert(
            RackParameters::parse("access_token=fixture-bearer-token-v4-6-5")
                .expect("access parameter is valid"),
        );
        inject_parameter_bearer(&mut request);
        assert_eq!(request.headers().get_all(AUTHORIZATION).iter().count(), 2);

        let parameters =
            RackParameters::parse("access_token=&bearer_token=fixture-bearer-token-v4-6-5")
                .expect("fallback parameters are valid");
        assert_eq!(
            parameter_bearer(&parameters),
            Some("fixture-bearer-token-v4-6-5")
        );
    }

    #[test]
    fn browser_auth_parameters_support_nested_and_flat_forms() {
        let nested = RackParameters::parse(
            "user%5Bemail%5D=alice%40fixture.invalid&user%5Bpassword%5D=secret",
        )
        .expect("nested browser parameters are valid");
        assert_eq!(
            browser_scalar(&nested, "email"),
            Some("alice@fixture.invalid")
        );
        assert_eq!(browser_scalar(&nested, "password"), Some("secret"));

        let flat = RackParameters::parse("email=alice%40fixture.invalid&password=secret")
            .expect("flat browser parameters are valid");
        assert_eq!(
            browser_scalar(&flat, "email"),
            Some("alice@fixture.invalid")
        );
        assert_eq!(browser_scalar(&flat, "missing"), None);
    }

    #[test]
    fn search_pagination_rejects_negative_and_structured_values() {
        let negative = RackParameters::parse("limit=-1&offset=-2").unwrap();
        assert!(nonnegative_search_parameter(&negative, "limit", 20, Some(40)).is_err());
        assert!(nonnegative_search_parameter(&negative, "offset", 0, None).is_err());

        let excessive = RackParameters::parse("limit=80").unwrap();
        assert_eq!(
            nonnegative_search_parameter(&excessive, "limit", 20, Some(40)),
            Ok(40)
        );
        let structured = RackParameters::parse("offset%5B%5D=1").unwrap();
        assert!(nonnegative_search_parameter(&structured, "offset", 0, None).is_err());
    }

    #[test]
    fn browser_sign_in_requires_a_matching_double_submit_csrf_token() {
        let signing_key = [7; 32];
        let csrf_token = new_browser_csrf_token(&signing_key);
        let parameters = RackParameters::parse(&format!("csrf_token={csrf_token}"))
            .expect("CSRF parameters are valid");
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("csrf_token={csrf_token}")).unwrap(),
        );
        assert!(browser_csrf_is_valid(
            &parameters,
            &headers,
            false,
            &signing_key
        ));

        headers.insert(COOKIE, HeaderValue::from_static("csrf_token=other-value"));
        assert!(!browser_csrf_is_valid(
            &parameters,
            &headers,
            false,
            &signing_key
        ));
        assert!(!browser_csrf_is_valid(
            &RackParameters::parse("").expect("empty parameters are valid"),
            &headers,
            false,
            &signing_key
        ));
    }

    #[test]
    fn browser_sign_out_accepts_form_or_header_csrf_tokens() {
        let signing_key = [7; 32];
        let csrf_token = new_browser_csrf_token(&signing_key);
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("csrf_token={csrf_token}")).unwrap(),
        );
        assert!(browser_sign_out_csrf_is_valid(
            &RackParameters::parse(&format!("csrf_token={csrf_token}"))
                .expect("form parameters are valid"),
            &headers,
            false,
            &signing_key
        ));

        headers.insert("x-csrf-token", HeaderValue::from_str(&csrf_token).unwrap());
        assert!(browser_sign_out_csrf_is_valid(
            &RackParameters::parse("").expect("empty parameters are valid"),
            &headers,
            false,
            &signing_key
        ));

        headers.insert("x-csrf-token", HeaderValue::from_static("wrong-value"));
        assert!(!browser_sign_out_csrf_is_valid(
            &RackParameters::parse("").expect("empty parameters are valid"),
            &headers,
            false,
            &signing_key
        ));
    }

    #[test]
    fn browser_csrf_rejects_unsigned_and_duplicate_cookies() {
        let signing_key = [7; 32];
        let parameters =
            RackParameters::parse("csrf_token=attacker").expect("CSRF parameters are valid");
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, HeaderValue::from_static("csrf_token=attacker"));
        assert!(!browser_csrf_is_valid(
            &parameters,
            &headers,
            false,
            &signing_key
        ));

        let csrf_token = new_browser_csrf_token(&signing_key);
        let parameters = RackParameters::parse(&format!("csrf_token={csrf_token}"))
            .expect("CSRF parameters are valid");
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("csrf_token=attacker; csrf_token={csrf_token}"))
                .unwrap(),
        );
        assert!(!browser_csrf_is_valid(
            &parameters,
            &headers,
            false,
            &signing_key
        ));
        assert!(request_cookie(&headers, BROWSER_CSRF_COOKIE).is_none());

        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("csrf_token={csrf_token}")).unwrap(),
        );
        assert!(!browser_csrf_is_valid(
            &parameters,
            &headers,
            true,
            &signing_key
        ));
    }

    #[test]
    fn browser_csrf_rejects_duplicates_across_cookie_headers() {
        let signing_key = [7; 32];
        let csrf_token = new_browser_csrf_token(&signing_key);
        let parameters = RackParameters::parse(&format!("csrf_token={csrf_token}"))
            .expect("CSRF parameters are valid");
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, HeaderValue::from_static("csrf_token=attacker"));
        headers.append(
            COOKIE,
            HeaderValue::from_str(&format!("theme=dark; csrf_token={csrf_token}")).unwrap(),
        );

        assert!(!browser_csrf_is_valid(
            &parameters,
            &headers,
            false,
            &signing_key
        ));
    }

    #[test]
    fn browser_csrf_ignores_malformed_unrelated_cookie_segments() {
        let signing_key = [7; 32];
        let csrf_token = new_browser_csrf_token(&signing_key);
        let parameters = RackParameters::parse(&format!("csrf_token={csrf_token}"))
            .expect("CSRF parameters are valid");
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!(
                "malformed; =missing-name; theme; csrf_token={csrf_token}; other=value"
            ))
            .unwrap(),
        );

        assert!(browser_csrf_is_valid(
            &parameters,
            &headers,
            false,
            &signing_key
        ));
    }

    #[test]
    fn browser_csrf_key_derivation_is_stable_and_secret_specific() {
        let first = derive_browser_csrf_signing_key("fixture-secret-key-base");
        let restarted = derive_browser_csrf_signing_key("fixture-secret-key-base");
        let other = derive_browser_csrf_signing_key("different-secret-key-base");
        let token = new_browser_csrf_token(&first);

        assert_eq!(first, restarted);
        assert_ne!(first, other);
        assert!(valid_browser_csrf_token(&token, &restarted));
        assert!(!valid_browser_csrf_token(&token, &other));
        assert_ne!(first, Sha256::digest(b"fixture-secret-key-base").as_slice());
    }

    #[test]
    fn secure_browser_csrf_rotates_legacy_and_invalid_cookies() {
        let signing_key = [7; 32];
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("csrf_token=transplanted; __Host-csrf_token=unsigned"),
        );

        let (token, cookie) = browser_page_csrf(&headers, true, &signing_key);
        assert!(valid_browser_csrf_token(&token, &signing_key));
        let cookie = cookie.expect("invalid secure cookie is rotated");
        assert!(cookie.starts_with("__Host-csrf_token="));
        assert!(cookie.contains("; Path=/;"));
        assert!(cookie.contains("; Secure"));
        assert!(!cookie.contains("Domain="));

        let legacy_only = HeaderMap::from_iter([(
            COOKIE,
            HeaderValue::from_str(&format!(
                "csrf_token={}",
                new_browser_csrf_token(&signing_key)
            ))
            .unwrap(),
        )]);
        assert!(
            browser_page_csrf(&legacy_only, true, &signing_key)
                .1
                .is_some()
        );

        let (_, cookie) = browser_page_csrf(&HeaderMap::new(), false, &signing_key);
        let cookie = cookie.expect("HTTP requests receive a CSRF cookie");
        assert!(cookie.starts_with("csrf_token="));
        assert!(!cookie.contains("; Secure"));
    }

    #[test]
    fn browser_cookies_are_secure_and_logout_clears_them() {
        let session = browser_cookie(BROWSER_SESSION_COOKIE, "session", 60, true, true);
        assert!(session.contains("HttpOnly"));
        assert!(session.contains("Secure"));
        assert!(session.contains("SameSite=Lax"));
        let cleared = browser_cookie(BROWSER_CSRF_COOKIE, "", 0, false, false);
        assert!(!cleared.contains("HttpOnly"));
        assert!(cleared.contains("Max-Age=0"));
        assert!(constant_time_equal(b"same", b"same"));
        assert!(!constant_time_equal(b"same", b"different"));
    }

    #[test]
    fn browser_settings_navigation_exposes_a_csrf_protected_logout_form() {
        let navigation =
            browser_settings_navigation("csrf<&", Some(SettingsSection::PostingDefaults));
        assert!(navigation.contains("method=\"post\" action=\"/auth/sign_out\""));
        assert!(navigation.contains("name=\"csrf_token\" value=\"csrf&lt;&amp;\""));
        assert!(navigation.contains("button button--secondary"));
        assert!(navigation.contains("type=\"submit\">Log out</button>"));
        assert!(
            navigation
                .contains("href=\"/settings/preferences/posting_defaults\" aria-current=\"page\"")
        );
        assert_eq!(navigation.matches("aria-current=\"page\"").count(), 1);
    }

    #[test]
    fn password_reset_limiter_matches_ip_and_email_boundaries() {
        let limiter = PasswordResetLimiter::default();
        let ip = "192.0.2.1".parse().expect("fixture IP is valid");
        for _ in 0..5 {
            assert!(limiter.check(ip, "person@example.invalid").is_ok());
        }
        assert!(limiter.check(ip, "person@example.invalid").is_err());

        for index in 0..20 {
            assert!(
                limiter
                    .check(ip, &format!("person-{index}@example.invalid"))
                    .is_ok()
            );
        }
        assert!(limiter.check(ip, "another@example.invalid").is_err());
        assert_eq!(
            attempt_ip_bucket("2001:db8:1::1".parse().unwrap()),
            attempt_ip_bucket("2001:db8:1::ffff".parse().unwrap())
        );
    }

    #[test]
    fn attempt_limiter_uses_fixed_epoch_buckets_and_bounds_state() {
        let limiter = AttemptLimiter::default();
        let key = "fixture".to_owned();
        let limits = [(key.clone(), 1, StdDuration::from_mins(1))];
        assert!(limiter.allow_at(limits.clone(), 59));
        assert!(!limiter.allow_at(limits.clone(), 59));
        assert!(limiter.allow_at(limits, 60));

        let limiter = AttemptLimiter::default();
        for index in 0..MAX_RATE_LIMIT_WINDOWS {
            assert!(limiter.allow_at([(format!("key-{index}"), 1, StdDuration::from_mins(1))], 0));
        }
        assert!(!limiter.allow_at(
            [("one-too-many".to_owned(), 1, StdDuration::from_mins(1))],
            0
        ));
    }

    #[test]
    fn browser_login_limiter_matches_mastodon_boundaries() {
        let limiter = BrowserLoginLimiter::default();
        let ip = "192.0.2.1".parse().expect("fixture IP is valid");
        for index in 0..25 {
            assert!(
                limiter
                    .check(ip, &format!("person-{index}@example.invalid"))
                    .is_ok()
            );
        }
        assert!(limiter.check(ip, "another@example.invalid").is_err());

        let limiter = BrowserLoginLimiter::default();
        for index in 0..25 {
            let ip = format!("198.51.100.{}", index + 1)
                .parse()
                .expect("fixture IP is valid");
            assert!(limiter.check(ip, "person@example.invalid").is_ok());
        }
        assert!(
            limiter
                .check(
                    "198.51.100.26".parse().expect("fixture IP is valid"),
                    "person@example.invalid"
                )
                .is_err()
        );
    }

    #[test]
    fn oauth_application_limiter_matches_mastodon_boundary() {
        let limiter = OAuthApplicationLimiter::default();
        let ip = "192.0.2.1".parse().expect("fixture IP is valid");
        for _ in 0..5 {
            assert!(limiter.check(ip).is_ok());
        }
        assert!(limiter.check(ip).is_err());
    }

    #[test]
    fn media_proxy_limiter_matches_mastodon_boundary() {
        let limiter = MediaProxyLimiter::default();
        let ip = "192.0.2.1".parse().expect("fixture IP is valid");
        for _ in 0..30 {
            assert!(limiter.check(ip).is_ok());
        }
        let rate_limited = limiter
            .check(ip)
            .expect_err("the 31st media proxy request must be limited");
        assert_eq!(rate_limited.limit, 30);
        assert_eq!(rate_limited.period, StdDuration::from_mins(10));
        let response = rate_limited_response(rate_limited);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["x-ratelimit-limit"], "30");
        assert_eq!(response.headers()["x-ratelimit-remaining"], "0");
        assert!(response.headers().contains_key("x-ratelimit-reset"));
        assert!(response.headers().contains_key("retry-after"));
    }

    #[test]
    fn media_upload_limiter_matches_mastodon_boundary() {
        let limiter = MediaUploadLimiter::default();
        for _ in 0..30 {
            assert!(limiter.check(101).is_ok());
        }
        let rate_limited = limiter
            .check(101)
            .expect_err("the 31st media upload request must be limited");
        assert_eq!(rate_limited.limit, 30);
        assert_eq!(rate_limited.period, StdDuration::from_mins(30));
    }

    #[test]
    fn browser_reauthentication_budgets_roll_over_at_epoch_boundaries() {
        let keys = BrowserReauthenticationLimiter::keys("192.0.2.91".parse().unwrap(), 91);
        assert_eq!((keys[0].1, keys[0].2), (25, StdDuration::from_mins(5)));
        assert_eq!((keys[1].1, keys[1].2), (10, StdDuration::from_hours(1)));
        for key in keys {
            let limiter = AttemptLimiter::default();
            // 2000-01-01 00:00:00 UTC is aligned to both production periods.
            let boundary = 946_684_800 + key.2.as_secs();
            for _ in 0..key.1 {
                assert!(limiter.allow_at([key.clone()], boundary - 1));
            }
            assert!(!limiter.allow_at([key.clone()], boundary - 1));
            assert!(
                limiter.allow_at([key], boundary),
                "crossing the epoch boundary replenishes a budget even after only one second"
            );
        }
    }

    #[cfg(feature = "test-support")]
    #[tokio::test]
    #[ignore = "requires a disposable RUSTODON_OPERATIONAL_DATABASE_URL"]
    async fn shared_rate_limiter_fixed_clock_preserves_epoch_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        let url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
        let first = SharedRateLimiter::new(PgPool::connect(&url).await?);
        let second = SharedRateLimiter::new(PgPool::connect(&url).await?);
        let keys = BrowserReauthenticationLimiter::keys("192.0.2.92".parse()?, 92);
        for (key, limit, period) in keys {
            let key = (format!("test:reauth-clock:{key}"), limit, period);
            let boundary = 946_684_800 + i64::try_from(period.as_secs())?;
            assert!(unix_timestamp_seconds() > u64::try_from(boundary)?);
            sqlx::query("DELETE FROM rustodon.rate_limit_windows WHERE window_key = $1")
                .bind(&key.0)
                .execute(&first.pool)
                .await?;
            let frozen_first = first.clone().with_fixed_time(boundary - 1);
            let frozen_second = second.clone().with_fixed_time(boundary - 1);
            for _ in 0..limit {
                assert!(frozen_first.try_allow([key.clone()]).await.is_ok());
            }
            assert!(
                frozen_second.try_allow([key.clone()]).await.is_err(),
                "both pools must retain a full historical bucket despite real wall time"
            );
            let advanced = second.clone().with_fixed_time(boundary);
            for _ in 0..limit {
                assert!(advanced.try_allow([key.clone()]).await.is_ok());
            }
            assert!(advanced.try_allow([key.clone()]).await.is_err());
            // Unconfigured instances still use real time and therefore see a
            // different bucket, even after configured clones exhaust theirs.
            assert!(first.try_allow([key.clone()]).await.is_ok());
            assert!(first.fixed_time.is_none());
            assert!(second.fixed_time.is_none());
            sqlx::query("DELETE FROM rustodon.rate_limit_windows WHERE window_key = $1")
                .bind(&key.0)
                .execute(&first.pool)
                .await?;
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "starts a disposable PostgreSQL fixture through Mise"]
    #[allow(clippy::too_many_lines)]
    async fn shared_rate_limiter_coordinates_independent_pools()
    -> Result<(), Box<dyn std::error::Error>> {
        let url = std::env::var("RUSTODON_OPERATIONAL_DATABASE_URL")?;
        let first_pool = PgPool::connect(&url).await?;
        let second_pool = PgPool::connect(&url).await?;
        let key = "test:shared-rate-limiter";
        sqlx::query("DELETE FROM rustodon.rate_limit_windows WHERE window_key = $1")
            .bind(key)
            .execute(&first_pool)
            .await?;
        let first = SharedRateLimiter::new(first_pool.clone());
        let second = SharedRateLimiter::new(second_pool);
        for _ in 0..30 {
            first
                .try_allow([(
                    key.to_owned(),
                    MEDIA_UPLOAD_RATE_LIMIT,
                    MEDIA_UPLOAD_RATE_LIMIT_PERIOD,
                )])
                .await
                .expect("the first pool can consume the shared window");
        }
        assert!(
            second
                .try_allow([(
                    key.to_owned(),
                    MEDIA_UPLOAD_RATE_LIMIT,
                    MEDIA_UPLOAD_RATE_LIMIT_PERIOD,
                )])
                .await
                .is_err(),
            "the second pool must observe the first pool's attempts"
        );
        sqlx::query("DELETE FROM rustodon.rate_limit_windows WHERE window_key = $1")
            .bind(key)
            .execute(&first_pool)
            .await?;

        let circuit_ip = "198.51.100.74".parse().expect("fixture IP is valid");
        let other_circuit_ip = "198.51.100.75".parse().expect("fixture IP is valid");
        let circuit_key = signature_fetch_circuit_key(circuit_ip);
        let other_circuit_key = signature_fetch_circuit_key(other_circuit_ip);
        for circuit_key in [&circuit_key, &other_circuit_key] {
            sqlx::query("DELETE FROM rustodon.rate_limit_windows WHERE window_key = $1")
                .bind(circuit_key)
                .execute(&first_pool)
                .await?;
        }
        assert!(!first.circuit_open(&circuit_key).await?);
        assert!(!remote_signature_fetch_should_trip(
            &RemoteFetchError::UnexpectedStatus(StatusCode::NOT_FOUND)
        ));
        assert!(!second.circuit_open(&circuit_key).await?);
        first
            .record_circuit_failure(&circuit_key, SIGNATURE_FETCH_COOL_OFF)
            .await?;
        assert!(second.circuit_open(&circuit_key).await?);
        assert!(!second.circuit_open(&other_circuit_key).await?);
        sqlx::query(
            "UPDATE rustodon.rate_limit_windows \
             SET expires_at = clock_timestamp() - INTERVAL '1 second' \
             WHERE window_key = $1",
        )
        .bind(&circuit_key)
        .execute(&first_pool)
        .await?;
        assert!(!first.circuit_open(&circuit_key).await?);
        for circuit_key in [&circuit_key, &other_circuit_key] {
            sqlx::query("DELETE FROM rustodon.rate_limit_windows WHERE window_key = $1")
                .bind(circuit_key)
                .execute(&first_pool)
                .await?;
        }

        let collision_ip = "198.51.100.73".parse().expect("fixture IP is valid");
        let password_limiter = PasswordResetLimiter::default();
        let browser_limiter = BrowserLoginLimiter::default();
        let mut collision_keys = Vec::new();
        for index in 0..25 {
            let email = format!("collision-{index}@example.invalid");
            collision_keys.extend(
                PasswordResetLimiter::keys(collision_ip, &email)
                    .into_iter()
                    .map(|(key, _, _)| key),
            );
            password_limiter
                .check_shared(Some(&first), collision_ip, &email)
                .await
                .expect("password-reset attempts should consume their own IP window");
        }
        let browser_email = "browser@example.invalid";
        collision_keys.extend(
            BrowserLoginLimiter::keys(collision_ip, browser_email)
                .into_iter()
                .map(|(key, _, _)| key),
        );
        browser_limiter
            .check_shared(Some(&second), collision_ip, browser_email)
            .await
            .expect("password-reset traffic must not consume the browser-login IP window");
        for collision_key in collision_keys {
            sqlx::query("DELETE FROM rustodon.rate_limit_windows WHERE window_key = $1")
                .bind(collision_key)
                .execute(&first_pool)
                .await?;
        }
        Ok(())
    }

    #[test]
    fn activitypub_inbox_limiter_bounds_each_client_ip() {
        let limiter = ActivityPubInboxLimiter::default();
        let ip = "192.0.2.1".parse().expect("fixture IP is valid");
        for _ in 0..ACTIVITYPUB_INBOX_RATE_LIMIT {
            assert!(limiter.check(ip).is_ok());
        }
        let rate_limited = limiter
            .check(ip)
            .expect_err("the request after the inbox limit must be limited");
        assert_eq!(rate_limited.limit, ACTIVITYPUB_INBOX_RATE_LIMIT);
        assert_eq!(rate_limited.period, ACTIVITYPUB_INBOX_RATE_LIMIT_PERIOD);

        let other_ip = "192.0.2.2".parse().expect("fixture IP is valid");
        assert!(limiter.check(other_ip).is_ok());

        let ipv6_limiter = ActivityPubInboxLimiter::default();
        let ipv6_first = "2001:db8::1".parse().expect("fixture IP is valid");
        for _ in 0..ACTIVITYPUB_INBOX_RATE_LIMIT {
            assert!(ipv6_limiter.check(ipv6_first).is_ok());
        }
        assert!(
            ipv6_limiter
                .check("2001:db8::2".parse().expect("fixture IP is valid"))
                .is_err()
        );
    }

    #[test]
    fn remote_account_resolution_limiter_bounds_each_handle() {
        let limiter = RemoteAccountResolutionLimiter::default();
        let ip = "192.0.2.1".parse().expect("fixture IP is valid");
        assert!(limiter.check(ip, "alice", "remote.example").is_ok());
        assert!(limiter.check(ip, "ALICE", "REMOTE.EXAMPLE").is_err());
        assert!(limiter.check(ip, "bob", "remote.example").is_ok());
    }

    #[test]
    fn signature_fetch_circuit_cools_off_per_client_ip() {
        let circuit = SignatureFetchCircuit::default();
        let ip = "192.0.2.1".parse().expect("fixture IP is valid");
        let other_ip = "192.0.2.2".parse().expect("fixture IP is valid");
        let now = 1_000;

        assert!(remote_signature_fetch_should_trip(
            &RemoteFetchError::Request
        ));
        assert!(!remote_signature_fetch_should_trip(
            &RemoteFetchError::UnexpectedStatus(StatusCode::NOT_FOUND)
        ));
        assert_eq!(
            remote_signature_fetch_failure(&RemoteFetchError::Request).status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(!remote_signature_fetch_should_trip(
            &RemoteFetchError::DomainBudgetExceeded
        ));
        assert_eq!(
            remote_signature_fetch_failure(&RemoteFetchError::DomainBudgetExceeded).status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            remote_signature_fetch_failure(&RemoteFetchError::UnexpectedStatus(
                StatusCode::NOT_FOUND
            ))
            .status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(circuit.allow_at(ip, now));
        circuit.record_failure_at(ip, now);
        assert!(!circuit.allow_at(ip, now));
        assert!(!circuit.allow_at(ip, now + SIGNATURE_FETCH_COOL_OFF.as_secs() - 1));
        assert!(circuit.allow_at(ip, now + SIGNATURE_FETCH_COOL_OFF.as_secs()));
        assert!(circuit.allow_at(other_ip, now));
    }

    #[test]
    fn rate_limited_responses_expose_mastodon_reset_headers() {
        let response = rate_limited_response(RateLimitExceeded {
            limit: 5,
            period: StdDuration::from_mins(10),
        });
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["x-ratelimit-limit"], "5");
        assert_eq!(response.headers()["x-ratelimit-remaining"], "0");
        assert!(response.headers().contains_key("x-ratelimit-reset"));
        assert!(response.headers().contains_key("retry-after"));
    }

    #[test]
    fn api_fallbacks_use_the_mastodon_json_envelope() {
        let response = api_not_found();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn paperclip_ranges_cover_rack_media_cases() {
        assert_eq!(
            paperclip_ranges(Some("bytes=2-4"), 10),
            RangeSelection::Ranges(vec![ByteRange { start: 2, end: 4 }])
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=7-"), 10),
            RangeSelection::Ranges(vec![ByteRange { start: 7, end: 9 }])
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=-3"), 10),
            RangeSelection::Ranges(vec![ByteRange { start: 7, end: 9 }])
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=0-1,8-9"), 10),
            RangeSelection::Ranges(vec![
                ByteRange { start: 0, end: 1 },
                ByteRange { start: 8, end: 9 },
            ])
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=10-"), 10),
            RangeSelection::Unsatisfiable
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=8-2"), 10),
            RangeSelection::Full
        );
        assert_eq!(
            paperclip_ranges(Some("Bytes=0-1"), 10),
            RangeSelection::Full
        );
        assert_eq!(
            paperclip_ranges(Some("bytes=18446744073709551616-"), 10),
            RangeSelection::Unsatisfiable
        );
        assert_eq!(paperclip_ranges(Some("bytes=0-1"), 0), RangeSelection::Full);
    }

    #[test]
    fn absolute_media_route_paths_are_normalized_once() {
        assert_eq!(
            media_route("https://media.example/assets"),
            ("/assets".to_owned(), Some("media.example".to_owned()))
        );
        assert_eq!(
            media_route("https://media.example/assets/"),
            ("/assets".to_owned(), Some("media.example".to_owned()))
        );
        assert_eq!(
            media_route("https://media.example/"),
            (String::new(), Some("media.example".to_owned()))
        );
    }

    #[tokio::test]
    async fn request_body_limit_rejects_oversized_bodies() {
        let request = Request::builder()
            .uri("/api/v2/instance")
            .body(Body::from(vec![0_u8; 5]))
            .expect("static request is valid");
        let response = bounded_request(request, 4)
            .await
            .expect_err("oversized request is rejected");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "application/json; charset=utf-8"
        );

        let request = Request::builder()
            .uri("/api/v2/instance")
            .body(Body::from("body"))
            .expect("static request is valid");
        let request = bounded_request(request, 4)
            .await
            .expect("accepted body is replayed");
        let body = axum::body::to_bytes(request.into_body(), 4)
            .await
            .expect("replayed body is readable");
        assert_eq!(body, "body");
    }

    #[tokio::test]
    async fn request_body_timeout_rejects_stalled_bodies() {
        let request = Request::builder()
            .uri("/api/v2/instance")
            .body(Body::from_stream(futures_util::stream::pending::<
                Result<Bytes, std::convert::Infallible>,
            >()))
            .expect("static request is valid");
        let response = bounded_request_with_timeout(request, 4, StdDuration::from_millis(1))
            .await
            .expect_err("stalled request is rejected");
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    }

    #[test]
    fn large_api_bodies_require_authenticated_admission() {
        let route = api_route_for_method(&Method::POST, "/api/v1/statuses")
            .expect("status creation route is inventoried");
        let unauthenticated = HeaderMap::new();
        let unauthenticated_request = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/statuses")
            .body(Body::empty())
            .expect("request is valid");
        assert_eq!(
            api_request_body_limit(
                "/api/v1/statuses",
                Some(route),
                &unauthenticated,
                &unauthenticated_request
            ),
            PUBLIC_REQUEST_BODY_LIMIT_BYTES
        );
        assert!(!api_request_requires_preauthentication(
            Some(route),
            &unauthenticated,
            &unauthenticated_request
        ));

        let mut authenticated = HeaderMap::new();
        authenticated.insert(AUTHORIZATION, HeaderValue::from_static("Bearer fixture"));
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/statuses")
            .header(
                CONTENT_LENGTH,
                (PUBLIC_REQUEST_BODY_LIMIT_BYTES + 1).to_string(),
            )
            .body(Body::empty())
            .expect("request is valid");
        assert!(api_request_requires_preauthentication(
            Some(route),
            &authenticated,
            &request
        ));
        assert!(content_length_exceeds_limit(
            request.headers(),
            PUBLIC_REQUEST_BODY_LIMIT_BYTES
        ));
        assert_eq!(
            api_request_body_limit("/api/v1/statuses", Some(route), &authenticated, &request),
            REST_BODY_LIMIT_BYTES
        );

        let mut query_authenticated = HeaderMap::new();
        inject_query_parameter_bearer(
            &mut query_authenticated,
            Some("access_token=fixture-bearer-token-v4-6-5"),
        );
        assert_eq!(
            query_authenticated[AUTHORIZATION],
            "Bearer fixture-bearer-token-v4-6-5"
        );
        assert_eq!(
            api_request_body_limit(
                "/api/v1/statuses",
                Some(route),
                &query_authenticated,
                &request
            ),
            REST_BODY_LIMIT_BYTES
        );

        let get_route = api_route_for_method(&Method::GET, "/api/v1/markers")
            .expect("marker reads are inventoried");
        let get_request = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/markers")
            .body(Body::empty())
            .expect("request is valid");
        assert!(!api_request_requires_preauthentication(
            Some(get_route),
            &authenticated,
            &get_request
        ));
        assert_eq!(
            api_request_body_limit(
                "/api/v1/markers",
                Some(get_route),
                &authenticated,
                &get_request
            ),
            PUBLIC_REQUEST_BODY_LIMIT_BYTES
        );

        let public_route = api_route_for_method(&Method::POST, "/api/v1/apps")
            .expect("app registration route is inventoried");
        assert!(required_api_scopes(Some(public_route)).is_none());
        assert_eq!(
            api_request_body_limit("/api/v1/apps", Some(public_route), &authenticated, &request),
            PUBLIC_REQUEST_BODY_LIMIT_BYTES
        );
    }

    #[test]
    fn query_validation_rejects_bad_escapes_and_utf8() {
        assert!(valid_query("limit=2&tagged=fixturetag"));
        assert!(!valid_query("limit=%"));
        assert!(!valid_query("limit=%GG"));
        assert!(!valid_query("limit=%FF"));
    }

    #[test]
    fn cursor_parameters_reject_signed_bigint_overflow() {
        let positive = RackParameters::parse("max_id=9223372036854775808").unwrap();
        assert_eq!(
            integer_parameter(&positive, "max_id"),
            Err(CursorParameterError::Overflow)
        );
        let negative = RackParameters::parse("min_id=-9223372036854775809").unwrap();
        assert_eq!(
            integer_parameter(&negative, "min_id"),
            Err(CursorParameterError::Overflow)
        );
    }

    #[test]
    fn nonnumeric_cursor_parameters_produce_empty_bounds() {
        let invalid = RackParameters::parse("max_id=invalid&min_id=%2B&since_id=-").unwrap();
        assert_eq!(integer_parameter(&invalid, "max_id"), Ok(Some(i64::MIN)));
        assert_eq!(integer_parameter(&invalid, "min_id"), Ok(Some(i64::MAX)));
        assert_eq!(integer_parameter(&invalid, "since_id"), Ok(Some(i64::MAX)));
    }

    #[test]
    fn cursor_parameters_preserve_rack_scalar_shapes() {
        let nested = RackParameters::parse("max_id%5B%5D=1").unwrap();
        assert_eq!(integer_parameter(&nested, "max_id"), Ok(Some(i64::MIN)));
        assert!(RackParameters::parse("max_id=1&max_id%5B%5D=2").is_err());
    }

    #[test]
    fn account_profile_updates_parse_nested_fields_and_source_settings() {
        let parameters = RackParameters::parse(
            "display_name=Profile+Name&note=Hello%21&bot=0&locked=false&discoverable=1&indexable=true&fields_attributes%5B%5D%5Bname%5D=Website&fields_attributes%5B%5D%5Bvalue%5D=https%3A%2F%2Fexample.com&source%5Bprivacy%5D=unlisted&source%5Bsensitive%5D=1&source%5Blanguage%5D=fr&source%5Bquote_policy%5D=followers",
        )
        .unwrap();
        let update = account_profile_update(&parameters).unwrap();
        assert_eq!(update.display_name.as_deref(), Some("Profile Name"));
        assert_eq!(update.note.as_deref(), Some("Hello!"));
        assert_eq!(update.bot, AccountProfileValue::Value(false));
        assert_eq!(update.locked, Some(false));
        assert_eq!(update.discoverable, AccountProfileValue::Value(true));
        assert_eq!(update.indexable, Some(true));
        assert_eq!(
            update.fields,
            Some(vec![AccountFieldUpdate {
                name: "Website".to_owned(),
                value: "https://example.com".to_owned(),
            }])
        );
        assert_eq!(
            update.source,
            Some(AccountSourceUpdate {
                privacy: AccountProfileValue::Value("unlisted".to_owned()),
                sensitive: AccountProfileValue::Value(true),
                language: AccountProfileValue::Value("fr".to_owned()),
                quote_policy: AccountProfileValue::Value("followers".to_owned()),
            })
        );
    }

    #[test]
    fn media_updates_parse_descriptions_and_focus_coordinates() {
        let parameters = RackParameters::parse("description=Alt+text&focus=0.25%2C-0.5").unwrap();
        let update = media_attachment_update(&parameters).unwrap();
        assert_eq!(
            update.description,
            AccountProfileValue::Value("Alt text".to_owned())
        );
        assert_eq!(
            update.focus,
            AccountProfileValue::Value(MediaFocus { x: 0.25, y: -0.5 })
        );

        let clear = RackParameters::parse("description=&focus%5B%5D=0&focus%5B%5D=1").unwrap();
        let update = media_attachment_update(&clear).unwrap();
        assert_eq!(
            update.description,
            AccountProfileValue::Value(String::new())
        );
        assert_eq!(
            update.focus,
            AccountProfileValue::Value(MediaFocus { x: 0.0, y: 1.0 })
        );

        for encoded in ["focus=", "focus"] {
            let unchanged = RackParameters::parse(encoded).unwrap();
            let update = media_attachment_update(&unchanged).unwrap();
            assert_eq!(update.focus, AccountProfileValue::Unchanged);
        }
    }

    #[test]
    fn consecutive_empty_brackets_match_rack_ordering() {
        let scalar_first = RackParameters::parse("a%5B%5D%5B%5D=1&a%5B%5D%5B%5D%5Bx%5D=2")
            .expect("mixed nested arrays parse");
        let Some(RackValue::Array(values)) = scalar_first.get("a") else {
            panic!("a is an array");
        };
        assert_eq!(values.len(), 2);
        assert!(matches!(
            values.as_slice(),
            [RackValue::Array(nested), RackValue::Object(object)]
                if matches!(nested.as_slice(), [RackValue::Scalar(value)] if value == "1")
                    && matches!(object.get("[]"), Some(RackValue::Object(child))
                        if matches!(child.get("x"), Some(RackValue::Scalar(value)) if value == "2"))
        ));

        let object_first = RackParameters::parse("a%5B%5D%5B%5D%5Bx%5D=1&a%5B%5D%5B%5D=2")
            .expect("terminal nested append after an object parses");
        let Some(RackValue::Array(values)) = object_first.get("a") else {
            panic!("a is an array");
        };
        assert!(matches!(
            values.as_slice(),
            [RackValue::Object(object)]
                if matches!(object.get("[]"), Some(RackValue::Object(child))
                    if matches!(child.get("x"), Some(RackValue::Scalar(value)) if value == "1"))
        ));
    }
}
