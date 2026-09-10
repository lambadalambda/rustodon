use std::error::Error;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use base64::{
    Engine as _, engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use hmac::{Hmac, Mac};
use http::header::{
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, HeaderValue, LOCATION,
    SET_COOKIE, VARY, WWW_AUTHENTICATE,
};
use http::{HeaderMap, StatusCode};
use ipnetwork::IpNetwork;
use pbkdf2::pbkdf2_hmac;
use reqwest::Method;
use rustodon::config::{ConfigWarning, SmtpConfig};
use rustodon::crypto::ActiveRecordEncryptionConfig;
use rustodon::mail::{CONFIRMATION_JOB_KIND, MailConfig};
use rustodon::mastodon::{
    BearerAuthenticator, BrowserAuthenticationMethod, NotificationActivity, NotificationCreate,
    NotificationCreateOutcome, Repository, WRITE_STATUSES, WriteRepository, verify_password,
};
use rustodon::paperclip::PaperclipRoot;
use rustodon::secret::SecretString;
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection};
use url::Url;

use super::artifacts::{MediaSnapshot, compare_media, compare_media_with_labels};
use super::comparison::{CapturedResponse, DEFAULT_MISMATCH_LIMIT, compare_responses};
use super::harness::{RequestSpec, send_identically, send_single};
use super::normalization::{MARKER_TIMESTAMP, MEDIA_BLURHASH};
use super::safety::{DifferentialConfig, HttpTargets};

const MARKER_USER_ID: i64 = 101;
const MARKER_TIMELINE: &str = "home";
const BROWSER_MEDIA_USER_ID: i64 = 104;
const BROWSER_MEDIA_ACCOUNT_ID: i64 = 116_844_606_259_201_004;
const NOTIFICATION_ACCOUNT_ID: i64 = 116_844_606_259_201_001;
const INTERACTION_ACCOUNT_ID: i64 = 116_844_606_259_201_001;
const RELATIONSHIP_REQUEST_SOURCE_ACCOUNT_ID: i64 = 116_844_606_259_202_002;
const RELATIONSHIP_FOLLOWER_ACCOUNT_ID: i64 = 116_844_606_259_202_001;
const RELATIONSHIP_TARGET_ACCOUNT_ID: i64 = -323;
const BOOKMARK_STATUS_ID: i64 = 116_844_842_188_805_001;
const REBLOG_STATUS_ID: i64 = 116_844_842_188_805_001;
const BLOCKED_REBLOG_TARGET_STATUS_ID: i64 = -415;
const BLOCKED_REBLOG_AUTHOR_ACCOUNT_ID: i64 = -323;
const FAVOURITE_STATUS_ID: i64 = 116_845_078_118_405_101;
const PIN_STATUS_ID: i64 = 116_845_314_048_005_201;
const FAVOURITE_TARGET_ACCOUNT_ID: i64 = 116_844_606_259_202_001;
const MEDIA_ACCOUNT_ID: i64 = 116_844_606_259_201_001;
const STATUS_MEDIA_ID: i64 = 116_844_842_188_806_002;
const STATUS_DELETE_MEDIA_ID: i64 = 116_844_842_188_806_003;
const REPORT_TARGET_ACCOUNT_ID: i64 = 116_844_606_259_202_001;
const REPORT_MENTION_TARGET_ACCOUNT_ID: i64 = 116_844_606_259_202_002;
const REPORT_STATUS_ID: i64 = 116_845_078_118_405_101;
const REPORT_COLLECTION_ID: i64 = 116_845_549_977_608_801;
const REPORT_SILENT_MENTION_STATUS_ID: i64 = -311;
const MEDIA_FIXTURE: &str = "target/mastodon-v4.6.5/spec/fixtures/files/attachment.jpg";
const PROFILE_MEDIA_BOUNDARY: &str = "rustodon-profile-media-boundary";
const ADMIN_RECOVERY_SECRET: &str = "fixture-differential-secret-key-base-0123456789abcdef";
const ADMIN_RECOVERY_PASSWORD: &str = "fixture-new-user-password";
const ADMIN_RESET_PASSWORD: &str = "fixture-admin-reset-password";
const ACTIVE_RECORD_PRIMARY_KEY: &str = "33333333333333333333333333333333";
const ACTIVE_RECORD_DETERMINISTIC_KEY: &str = "11111111111111111111111111111111";
const ACTIVE_RECORD_DERIVATION_SALT: &str = "22222222222222222222222222222222";

#[derive(Clone, Debug)]
struct BrowserFormState {
    session_cookie: Option<String>,
    csrf_cookie: Option<String>,
    csrf_token: Option<String>,
}

impl BrowserFormState {
    fn new(session_id: Option<&str>) -> Self {
        Self {
            session_cookie: session_id.map(|id| format!("_mastodon_session={id}")),
            csrf_cookie: None,
            csrf_token: None,
        }
    }

    fn cookie_header(&self) -> String {
        [self.session_cookie.as_deref(), self.csrf_cookie.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn update_from_response(
        &mut self,
        response: &CapturedResponse,
        csrf_field: &str,
    ) -> Result<(), Box<dyn Error>> {
        for value in response.headers.get_all(SET_COOKIE) {
            let Some(cookie) = value
                .to_str()
                .ok()
                .and_then(|value| value.split(';').next())
            else {
                continue;
            };
            if cookie.starts_with("_mastodon_session=") {
                self.session_cookie = Some(cookie.to_owned());
            } else if cookie.starts_with("__Host-csrf_token=") || cookie.starts_with("csrf_token=")
            {
                self.csrf_cookie = Some(cookie.to_owned());
            }
        }
        self.csrf_token = Some(
            hidden_form_value(&response.body, csrf_field)
                .ok_or_else(|| format!("rendered form did not contain {csrf_field}"))?,
        );
        Ok(())
    }

    fn csrf_token(&self) -> Result<&str, Box<dyn Error>> {
        self.csrf_token
            .as_deref()
            .ok_or_else(|| "browser flow has no rendered CSRF token".into())
    }
}

fn operation_and_cleanup(
    operation: Result<(), Box<dyn Error>>,
    cleanup: Result<(), Box<dyn Error>>,
) -> Result<(), Box<dyn Error>> {
    match (operation, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(operation), Err(cleanup)) => {
            Err(format!("{operation}; cleanup also failed: {cleanup}").into())
        }
    }
}

pub(crate) fn fixture_active_record_encryption() -> ActiveRecordEncryptionConfig {
    ActiveRecordEncryptionConfig::new(
        SecretString::new(ACTIVE_RECORD_PRIMARY_KEY.to_owned()),
        SecretString::new(ACTIVE_RECORD_DETERMINISTIC_KEY.to_owned()),
        SecretString::new(ACTIVE_RECORD_DERIVATION_SALT.to_owned()),
    )
    .expect("fixture Active Record encryption configuration should be valid")
}

#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
struct MarkerState {
    last_read_id: i64,
    lock_version: i32,
    updated_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
struct AccountProfileState {
    display_name: String,
    note: String,
    actor_type: Option<String>,
    locked: bool,
    discoverable: Option<bool>,
    hide_collections: Option<bool>,
    indexable: bool,
    attribution_domains: Option<Vec<String>>,
    fields: Option<Value>,
    updated_at: NaiveDateTime,
    user_settings: Option<String>,
    user_updated_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
struct AccountMediaState {
    avatar_content_type: Option<String>,
    avatar_description: String,
    avatar_file_name: Option<String>,
    avatar_file_size: Option<i32>,
    avatar_remote_url: Option<String>,
    avatar_storage_schema_version: Option<i32>,
    avatar_updated_at: Option<NaiveDateTime>,
    header_content_type: Option<String>,
    header_description: String,
    header_file_name: Option<String>,
    header_file_size: Option<i32>,
    header_remote_url: String,
    header_storage_schema_version: Option<i32>,
    header_updated_at: Option<NaiveDateTime>,
    updated_at: NaiveDateTime,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
struct MediaState {
    id: i64,
    account_id: Option<i64>,
    status_id: Option<i64>,
    media_type: i32,
    processing: Option<i32>,
    description: Option<String>,
    remote_url: String,
    file_content_type: Option<String>,
    file_file_name: Option<String>,
    file_file_size: Option<i32>,
    file_meta: Option<Value>,
    file_storage_schema_version: Option<i32>,
    blurhash: Option<String>,
}

type StableAccountProfileState<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    bool,
    Option<bool>,
    Option<bool>,
    bool,
    Option<&'a Vec<String>>,
    Option<&'a Value>,
    Option<Value>,
);

struct RelationshipConcurrencyBaseline {
    follows: Vec<Value>,
    requests: Vec<Value>,
    incoming_follows: Vec<Value>,
    incoming_requests: Vec<Value>,
    blocks: Vec<Value>,
    mutes: Vec<Value>,
    source_notifications: Vec<Value>,
    target_notifications: Vec<Value>,
    source_notification_requests: Vec<Value>,
    target_notification_requests: Vec<Value>,
    source_stats: Option<Value>,
    target_stats: Option<Value>,
}

struct StatusInteractionNotificationBaseline {
    owner_notifications: Vec<Value>,
    target_notifications: Vec<Value>,
    owner_notification_requests: Vec<Value>,
    target_notification_requests: Vec<Value>,
}

type DifferentialStatusFields = (
    String,
    String,
    i32,
    Option<String>,
    bool,
    bool,
    Option<i64>,
    Option<i64>,
    bool,
    Option<Vec<i64>>,
);

type DifferentialEditedStatusFields = (
    String,
    String,
    bool,
    Option<String>,
    Option<Vec<i64>>,
    bool,
    i64,
);

type StableInteractionAccountStat = (Option<i64>, Option<i64>, Option<i64>, Option<i64>);

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReportState {
    account_id: i64,
    target_account_id: i64,
    application_id: Option<i64>,
    category: i32,
    comment: String,
    forwarded: Option<bool>,
    rule_ids: Option<Vec<i64>>,
    status_ids: Vec<i64>,
    collection_ids: Vec<i64>,
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_report_writes_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("report writer differential configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("report writer differential configuration must include a Rust writer URL");
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .expect("report writer differential configuration must include a Rust owner URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_before = report_count(mastodon_owner.url()).await?;
    let rust_before = report_count(rust_writer.url()).await?;
    let mut mastodon_report_id = None;
    let mut rust_report_id = None;
    let mut mastodon_silent_report_id = None;
    let mut rust_silent_report_id = None;

    let result = async {
        let invalid_request = report_request(
            "account_id=116844606259202001&comment=invalid+collection&category=other&collection_ids%5B%5D=9223372036854775807",
        )?;
        let invalid = send_identically(&targets, &invalid_request).await?;
        if invalid.mastodon.status != 404 || invalid.rust.status != 404 {
            return Err(format!(
                "invalid report collection status differs: Mastodon={}, Rust={}",
                invalid.mastodon.status, invalid.rust.status
            )
            .into());
        }
        if report_count(mastodon_owner.url()).await? != mastodon_before
            || report_count(rust_writer.url()).await? != rust_before
        {
            return Err("invalid report attachment changed persisted report counts".into());
        }

        let invalid_rules_request = report_request(
            "account_id=116844606259202001&comment=invalid+rules&category=other&rule_ids%5B%5D=9223372036854775807",
        )?;
        let invalid_rules = send_identically(&targets, &invalid_rules_request).await?;
        if invalid_rules.mastodon.status != 422 || invalid_rules.rust.status != 422 {
            return Err(format!(
                "invalid report rule status differs: Mastodon={}, Rust={}",
                invalid_rules.mastodon.status, invalid_rules.rust.status
            )
            .into());
        }
        if report_count(mastodon_owner.url()).await? != mastodon_before
            || report_count(rust_writer.url()).await? != rust_before
        {
            return Err("invalid report rule changed persisted report counts".into());
        }

        let request = report_request(&format!(
            "account_id={REPORT_TARGET_ACCOUNT_ID}&comment=differential+report&category=spam&status_ids%5B%5D={REPORT_STATUS_ID}&collection_ids%5B%5D={REPORT_COLLECTION_ID}&forward=false"
        ))?;
        let mut responses = send_identically(&targets, &request).await?;
        mastodon_report_id = Some(normalize_generated_report_response(
            &mut responses.mastodon,
            "Mastodon",
        )?);
        rust_report_id = Some(normalize_generated_report_response(
            &mut responses.rust,
            "Rust",
        )?);
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("report create response: {error}"))?;

        let mastodon_state = report_state(mastodon_owner.url(), mastodon_report_id.unwrap()).await?;
        let rust_state = report_state(rust_writer.url(), rust_report_id.unwrap()).await?;
        if mastodon_state != rust_state {
            return Err(format!(
                "report database state differs: Mastodon={mastodon_state:?}, Rust={rust_state:?}"
            )
            .into());
        }

        let mention_request = report_request(&format!(
            "account_id={REPORT_MENTION_TARGET_ACCOUNT_ID}&comment=silent+mention&category=other&status_ids%5B%5D={REPORT_SILENT_MENTION_STATUS_ID}"
        ))?;
        let mut mention_responses = send_identically(&targets, &mention_request).await?;
        mastodon_silent_report_id = Some(normalize_generated_report_response(
            &mut mention_responses.mastodon,
            "Mastodon silent mention",
        )?);
        rust_silent_report_id = Some(normalize_generated_report_response(
            &mut mention_responses.rust,
            "Rust silent mention",
        )?);
        compare_responses(
            &mention_responses.mastodon,
            &mention_responses.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("silent mention report response: {error}"))?;
        let mastodon_mention_state =
            report_state(mastodon_owner.url(), mastodon_silent_report_id.unwrap()).await?;
        let rust_mention_state =
            report_state(rust_writer.url(), rust_silent_report_id.unwrap()).await?;
        if mastodon_mention_state != rust_mention_state
            || mastodon_mention_state.status_ids != vec![REPORT_SILENT_MENTION_STATUS_ID]
        {
            return Err(format!(
                "silent mention report state differs: Mastodon={mastodon_mention_state:?}, Rust={rust_mention_state:?}"
            )
            .into());
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    if let Some(report_id) = mastodon_report_id {
        delete_report(mastodon_owner.url(), report_id).await?;
    }
    if let Some(report_id) = rust_report_id {
        delete_report(rust_owner.url(), report_id).await?;
    }
    if let Some(report_id) = mastodon_silent_report_id {
        delete_report(mastodon_owner.url(), report_id).await?;
    }
    if let Some(report_id) = rust_silent_report_id {
        delete_report(rust_owner.url(), report_id).await?;
    }
    result
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_write_transactions_case(
    config: DifferentialConfig,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("writer differential configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("writer differential configuration must include a Rust writer URL");
    let mastodon_before = marker_state(config.mastodon_database.url()).await?;
    let rust_before = marker_state(config.rust_database.url()).await?;
    assert_eq!(
        (mastodon_before.last_read_id, mastodon_before.lock_version),
        (rust_before.last_read_id, rust_before.lock_version)
    );
    let mastodon_count_before = marker_count(config.mastodon_database.url()).await?;
    let rust_count_before = marker_count(config.rust_database.url()).await?;
    assert_eq!(mastodon_count_before, rust_count_before);
    let mastodon_media_before = MediaSnapshot::capture(&config.mastodon_media)?;
    let rust_media_before = MediaSnapshot::capture(&config.rust_media)?;
    compare_media(
        &mastodon_media_before,
        &rust_media_before,
        super::comparison::DEFAULT_MISMATCH_LIMIT,
    )?;

    let repository = Repository::connect(config.rust_database.url()).await?;
    let authenticator = BearerAuthenticator::new(repository);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let writer = WriteRepository::connect(rust_writer.url()).await?;

    let operation = async {
        let mastodon_after =
            update_marker_like_rails(mastodon_owner.url(), mastodon_before.last_read_id + 1)
                .await?;
        let rust_after = writer
            .update_marker(
                &authenticated,
                MARKER_TIMELINE,
                rust_before.last_read_id + 1,
                Some(rust_before.lock_version),
            )
            .await?;
        let mastodon_after_state = marker_state(config.mastodon_database.url()).await?;
        let rust_after_state = marker_state(config.rust_database.url()).await?;
        let mastodon_media_after = MediaSnapshot::capture(&config.mastodon_media)?;
        let rust_media_after = MediaSnapshot::capture(&config.rust_media)?;
        Ok::<_, Box<dyn Error>>((
            mastodon_after,
            rust_after,
            mastodon_after_state,
            rust_after_state,
            mastodon_media_after,
            rust_media_after,
        ))
    }
    .await;
    restore_marker(mastodon_owner.url(), &mastodon_before).await?;
    restore_marker(rust_writer.url(), &rust_before).await?;
    let (
        mastodon_after,
        rust_after,
        mastodon_after_state,
        rust_after_state,
        mastodon_media_after,
        rust_media_after,
    ) = operation?;
    assert_eq!(
        mastodon_after,
        (rust_after.last_read_id, rust_after.lock_version)
    );
    assert_eq!(
        (
            mastodon_after_state.last_read_id,
            mastodon_after_state.lock_version
        ),
        (rust_after_state.last_read_id, rust_after_state.lock_version)
    );
    assert_eq!(
        (
            mastodon_after_state.last_read_id,
            mastodon_after_state.lock_version
        ),
        (mastodon_after.0, mastodon_after.1)
    );
    assert_eq!(
        marker_count(config.mastodon_database.url()).await?,
        mastodon_count_before
    );
    assert_eq!(
        marker_count(config.rust_database.url()).await?,
        rust_count_before
    );
    compare_media_with_labels(
        &mastodon_media_before,
        &mastodon_media_after,
        "Mastodon before",
        "Mastodon after",
        super::comparison::DEFAULT_MISMATCH_LIMIT,
    )?;
    compare_media_with_labels(
        &rust_media_before,
        &rust_media_after,
        "Rust before",
        "Rust after",
        super::comparison::DEFAULT_MISMATCH_LIMIT,
    )?;
    compare_media(
        &mastodon_media_after,
        &rust_media_after,
        super::comparison::DEFAULT_MISMATCH_LIMIT,
    )?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_status_creation_writes_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("status writer differential configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("status writer differential configuration must include a Rust writer URL");
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .expect("status writer differential configuration must include a Rust owner URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_account_stats =
        interaction_account_stat(mastodon_owner.url(), INTERACTION_ACCOUNT_ID).await?;
    let rust_account_stats =
        interaction_account_stat(rust_writer.url(), INTERACTION_ACCOUNT_ID).await?;
    let mastodon_featured_before = featured_tag_state(mastodon_owner.url()).await?;
    let rust_featured_before = featured_tag_state(rust_writer.url()).await?;
    let mastodon_reply_stats = interaction_stat(mastodon_owner.url(), REBLOG_STATUS_ID).await?;
    let rust_reply_stats = interaction_stat(rust_writer.url(), REBLOG_STATUS_ID).await?;
    let mut mastodon_ids = Vec::new();
    let mut rust_ids = Vec::new();
    prepare_status_media(mastodon_owner.url()).await?;
    prepare_status_media(rust_writer.url()).await?;
    let operation = async {
        for (visibility, label) in [
            ("public", "public"),
            ("unlisted", "unlisted"),
            ("private", "private"),
            ("direct", "direct"),
            ("limited", "limited"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
            );
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            );
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            let body = format!(
                "status=fixture+status+write&spoiler_text=content+warning&visibility={visibility}&sensitive=true&language=fr"
            );
            let request = RequestSpec::new(
                Method::POST,
                "/api/v1/statuses",
                None,
                headers,
                body.into_bytes(),
            )?;
            let mut responses = send_identically(&targets, &request).await?;
            mastodon_ids.push(normalize_generated_status_response(
                &mut responses.mastodon,
                "Mastodon",
            )?);
            rust_ids.push(normalize_generated_status_response(&mut responses.rust, "Rust")?);
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    http::header::CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::VARY,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("status create {label}: {error}"))?;
        }
        let request = status_request(
            Method::POST,
            "/api/v1/statuses",
            &format!(
                "status=fixture+reply+write&visibility=public&in_reply_to_id={REBLOG_STATUS_ID}"
            ),
        )?;
        let mut responses = send_identically(&targets, &request).await?;
        mastodon_ids.push(normalize_generated_status_response(
            &mut responses.mastodon,
            "Mastodon reply",
        )?);
        rust_ids.push(normalize_generated_status_response(
            &mut responses.rust,
            "Rust reply",
        )?);
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("status reply create: {error}"))?;
        let media_request_body = format!(
            "status=fixture+media+write+%23fixturetag+%40moderator&visibility=public&media_ids%5B%5D={STATUS_MEDIA_ID}"
        );
        let media_request = status_request(
            Method::POST,
            "/api/v1/statuses",
            &media_request_body,
        )?;
        let mut media_responses = send_identically(&targets, &media_request).await?;
        mastodon_ids.push(normalize_generated_status_response(
            &mut media_responses.mastodon,
            "Mastodon media status",
        )?);
        rust_ids.push(normalize_generated_status_response(
            &mut media_responses.rust,
            "Rust media status",
        )?);
        compare_responses(
            &media_responses.mastodon,
            &media_responses.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("status media create: {error}"))?;
        for (mastodon_id, rust_id) in mastodon_ids.iter().zip(&rust_ids) {
            let mut mastodon_connection = PgConnection::connect(mastodon_owner.url()).await?;
            let mastodon_row: DifferentialStatusFields = sqlx::query_as(
                "SELECT text, spoiler_text, visibility, language, sensitive, local, \
                        in_reply_to_id, in_reply_to_account_id, reply, \
                        ordered_media_attachment_ids FROM statuses WHERE id = $1",
            )
            .bind(mastodon_id)
            .fetch_one(&mut mastodon_connection)
            .await?;
            let mut rust_connection = PgConnection::connect(rust_writer.url()).await?;
            let rust_row: DifferentialStatusFields = sqlx::query_as(
                "SELECT text, spoiler_text, visibility, language, sensitive, local, \
                        in_reply_to_id, in_reply_to_account_id, reply, \
                        ordered_media_attachment_ids FROM statuses WHERE id = $1",
            )
            .bind(rust_id)
            .fetch_one(&mut rust_connection)
            .await?;
            if mastodon_row != rust_row {
                return Err(format!(
                    "status {mastodon_id}/{rust_id} database fields differ: Mastodon={mastodon_row:?}, Rust={rust_row:?}"
                )
                .into());
            }
        }
        let edited_mastodon_id = *mastodon_ids
            .first()
            .ok_or("status creation did not return an editable status id")?;
        let edited_rust_id = *rust_ids
            .first()
            .ok_or("Rust status creation did not return an editable status id")?;
        let edit_body =
            "status=fixture+edited+status&spoiler_text=edited+warning&sensitive=true&language=fr-FR";
        let mastodon_edit_request = status_request(
            Method::PATCH,
            &format!("/api/v1/statuses/{edited_mastodon_id}"),
            edit_body,
        )?;
        let rust_edit_request = status_request(
            Method::PATCH,
            &format!("/api/v1/statuses/{edited_rust_id}"),
            edit_body,
        )?;
        let (mut mastodon_edit, mut rust_edit) = tokio::join!(
            send_single(targets.mastodon(), &mastodon_edit_request, "Mastodon"),
            send_single(targets.rust(), &rust_edit_request, "Rust")
        );
        let mastodon_edit = mastodon_edit.as_mut().map_err(|error| error.to_string())?;
        let rust_edit = rust_edit.as_mut().map_err(|error| error.to_string())?;
        normalize_generated_status_response(mastodon_edit, "Mastodon edited status")?;
        normalize_generated_status_response(rust_edit, "Rust edited status")?;
        compare_responses(
            mastodon_edit,
            rust_edit,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("status edit: {error}"))?;
        let mastodon_edited_state = edited_status_state(mastodon_owner.url(), edited_mastodon_id).await?;
        let rust_edited_state = edited_status_state(rust_writer.url(), edited_rust_id).await?;
        if mastodon_edited_state != rust_edited_state {
            return Err(format!(
                "edited status database fields differ: Mastodon={mastodon_edited_state:?}, Rust={rust_edited_state:?}"
            )
            .into());
        }
        let mastodon_replay_request = status_request(
            Method::PUT,
            &format!("/api/v1/statuses/{edited_mastodon_id}"),
            edit_body,
        )?;
        let rust_replay_request = status_request(
            Method::PUT,
            &format!("/api/v1/statuses/{edited_rust_id}"),
            edit_body,
        )?;
        let (mut mastodon_replay, mut rust_replay) = tokio::join!(
            send_single(targets.mastodon(), &mastodon_replay_request, "Mastodon"),
            send_single(targets.rust(), &rust_replay_request, "Rust")
        );
        let mastodon_replay = mastodon_replay.as_mut().map_err(|error| error.to_string())?;
        let rust_replay = rust_replay.as_mut().map_err(|error| error.to_string())?;
        normalize_generated_status_response(mastodon_replay, "Mastodon PUT replay")?;
        normalize_generated_status_response(rust_replay, "Rust PUT replay")?;
        compare_responses(
            mastodon_replay,
            rust_replay,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("status PUT replay: {error}"))?;
        if edited_status_state(mastodon_owner.url(), edited_mastodon_id).await?
            != mastodon_edited_state
            || edited_status_state(rust_writer.url(), edited_rust_id).await?
                != rust_edited_state
        {
            return Err("identical status PUT replay changed persisted state".into());
        }
        let edited_media_mastodon_id = *mastodon_ids
            .last()
            .ok_or("status creation did not return a media status id")?;
        let edited_media_rust_id = *rust_ids
            .last()
            .ok_or("Rust status creation did not return a media status id")?;
        let media_attribute_edit_body = format!(
            "status=fixture+media+attributes+edited&media_ids%5B%5D={STATUS_MEDIA_ID}&media_attributes%5B%5D%5Bid%5D={STATUS_MEDIA_ID}&media_attributes%5B%5D%5Bdescription%5D=updated+media+description&media_attributes%5B%5D%5Bfocus%5D=0.25%2C-0.5"
        );
        let mastodon_media_attribute_edit_request = status_request(
            Method::PATCH,
            &format!("/api/v1/statuses/{edited_media_mastodon_id}"),
            &media_attribute_edit_body,
        )?;
        let rust_media_attribute_edit_request = status_request(
            Method::PATCH,
            &format!("/api/v1/statuses/{edited_media_rust_id}"),
            &media_attribute_edit_body,
        )?;
        let (mut mastodon_media_attribute_edit, mut rust_media_attribute_edit) = tokio::join!(
            send_single(
                targets.mastodon(),
                &mastodon_media_attribute_edit_request,
                "Mastodon"
            ),
            send_single(
                targets.rust(),
                &rust_media_attribute_edit_request,
                "Rust"
            )
        );
        let mastodon_media_attribute_edit = mastodon_media_attribute_edit
            .as_mut()
            .map_err(|error| error.to_string())?;
        let rust_media_attribute_edit = rust_media_attribute_edit
            .as_mut()
            .map_err(|error| error.to_string())?;
        normalize_generated_status_response(
            mastodon_media_attribute_edit,
            "Mastodon media attribute edit",
        )?;
        normalize_generated_status_response(
            rust_media_attribute_edit,
            "Rust media attribute edit",
        )?;
        compare_responses(
            mastodon_media_attribute_edit,
            rust_media_attribute_edit,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("status media attribute edit: {error}"))?;
        let media_edit_body = "status=fixture+media+edited+%23fixturetag+%40moderator";
        let mastodon_media_edit_request = status_request(
            Method::PATCH,
            &format!("/api/v1/statuses/{edited_media_mastodon_id}"),
            media_edit_body,
        )?;
        let rust_media_edit_request = status_request(
            Method::PATCH,
            &format!("/api/v1/statuses/{edited_media_rust_id}"),
            media_edit_body,
        )?;
        let (mut mastodon_media_edit, mut rust_media_edit) = tokio::join!(
            send_single(
                targets.mastodon(),
                &mastodon_media_edit_request,
                "Mastodon"
            ),
            send_single(targets.rust(), &rust_media_edit_request, "Rust")
        );
        let mastodon_media_edit = mastodon_media_edit
            .as_mut()
            .map_err(|error| error.to_string())?;
        let rust_media_edit = rust_media_edit
            .as_mut()
            .map_err(|error| error.to_string())?;
        normalize_generated_status_response(mastodon_media_edit, "Mastodon edited media status")?;
        normalize_generated_status_response(rust_media_edit, "Rust edited media status")?;
        compare_responses(
            mastodon_media_edit,
            rust_media_edit,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("status media edit: {error}"))?;
        let mastodon_edited_media_state =
            edited_status_state(mastodon_owner.url(), edited_media_mastodon_id).await?;
        let rust_edited_media_state =
            edited_status_state(rust_writer.url(), edited_media_rust_id).await?;
        if mastodon_edited_media_state != rust_edited_media_state {
            return Err(format!(
                "edited media status database fields differ: Mastodon={mastodon_edited_media_state:?}, Rust={rust_edited_media_state:?}"
            )
            .into());
        }
        let delete_media_create_request = status_request(
            Method::POST,
            "/api/v1/statuses",
            &format!(
                "status=fixture+delete+media+write&visibility=public&media_ids%5B%5D={STATUS_DELETE_MEDIA_ID}"
            ),
        )?;
        let mut delete_media_created =
            send_identically(&targets, &delete_media_create_request).await?;
        mastodon_ids.push(normalize_generated_status_response(
            &mut delete_media_created.mastodon,
            "Mastodon delete-media status",
        )?);
        rust_ids.push(normalize_generated_status_response(
            &mut delete_media_created.rust,
            "Rust delete-media status",
        )?);
        compare_responses(
            &delete_media_created.mastodon,
            &delete_media_created.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("status delete-media create: {error}"))?;
        restore_interaction_account_stat(
            mastodon_owner.url(),
            INTERACTION_ACCOUNT_ID,
            mastodon_account_stats.as_ref(),
        )
        .await?;
        restore_interaction_account_stat(
            rust_owner.url(),
            INTERACTION_ACCOUNT_ID,
            rust_account_stats.as_ref(),
        )
        .await?;
        let mastodon_preserve_media_request = status_request(
            Method::DELETE,
            &format!("/api/v1/statuses/{edited_media_mastodon_id}"),
            "",
        )?;
        let rust_preserve_media_request = status_request(
            Method::DELETE,
            &format!("/api/v1/statuses/{edited_media_rust_id}"),
            "",
        )?;
        let (mut mastodon_preserve_media, mut rust_preserve_media) = tokio::join!(
            send_single(
                targets.mastodon(),
                &mastodon_preserve_media_request,
                "Mastodon"
            ),
            send_single(targets.rust(), &rust_preserve_media_request, "Rust")
        );
        let mastodon_preserve_media = mastodon_preserve_media
            .as_mut()
            .map_err(|error| error.to_string())?;
        let rust_preserve_media = rust_preserve_media
            .as_mut()
            .map_err(|error| error.to_string())?;
        normalize_generated_status_response(
            mastodon_preserve_media,
            "Mastodon preserved-media status",
        )?;
        normalize_generated_status_response(rust_preserve_media, "Rust preserved-media status")?;
        compare_responses(
            mastodon_preserve_media,
            rust_preserve_media,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("status preserve media: {error}"))?;
        restore_interaction_account_stat(
            mastodon_owner.url(),
            INTERACTION_ACCOUNT_ID,
            mastodon_account_stats.as_ref(),
        )
        .await?;
        restore_interaction_account_stat(
            rust_owner.url(),
            INTERACTION_ACCOUNT_ID,
            rust_account_stats.as_ref(),
        )
        .await?;
        let delete_media_mastodon_id = *mastodon_ids
            .last()
            .ok_or("status creation did not return a delete-media status id")?;
        let delete_media_rust_id = *rust_ids
            .last()
            .ok_or("Rust status creation did not return a delete-media status id")?;
        let mastodon_delete_media_request = status_request(
            Method::DELETE,
            &format!("/api/v1/statuses/{delete_media_mastodon_id}"),
            "",
        )?
        .with_query("delete_media=true");
        let rust_delete_media_request = status_request(
            Method::DELETE,
            &format!("/api/v1/statuses/{delete_media_rust_id}"),
            "",
        )?
        .with_query("delete_media=true");
        let (mut mastodon_delete_media, mut rust_delete_media) = tokio::join!(
            send_single(
                targets.mastodon(),
                &mastodon_delete_media_request,
                "Mastodon"
            ),
            send_single(targets.rust(), &rust_delete_media_request, "Rust")
        );
        let mastodon_delete_media = mastodon_delete_media
            .as_mut()
            .map_err(|error| error.to_string())?;
        let rust_delete_media = rust_delete_media
            .as_mut()
            .map_err(|error| error.to_string())?;
        normalize_generated_status_response(mastodon_delete_media, "Mastodon delete-media status")?;
        normalize_generated_status_response(rust_delete_media, "Rust delete-media status")?;
        compare_responses(
            mastodon_delete_media,
            rust_delete_media,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("status delete-media: {error}"))?;
        restore_interaction_account_stat(
            mastodon_owner.url(),
            INTERACTION_ACCOUNT_ID,
            mastodon_account_stats.as_ref(),
        )
        .await?;
        restore_interaction_account_stat(
            rust_owner.url(),
            INTERACTION_ACCOUNT_ID,
            rust_account_stats.as_ref(),
        )
        .await?;
        let deleted_mastodon_id = *mastodon_ids
            .first()
            .ok_or("status creation did not return a status id")?;
        let deleted_rust_id = *rust_ids
            .first()
            .ok_or("Rust status creation did not return a status id")?;
        let mastodon_delete_request = status_request(
            Method::DELETE,
            &format!("/api/v1/statuses/{deleted_mastodon_id}"),
            "",
        )?;
        let rust_delete_request = status_request(
            Method::DELETE,
            &format!("/api/v1/statuses/{deleted_rust_id}"),
            "",
        )?;
        let (mut mastodon_delete, mut rust_delete) = tokio::join!(
            send_single(targets.mastodon(), &mastodon_delete_request, "Mastodon"),
            send_single(targets.rust(), &rust_delete_request, "Rust")
        );
        let mastodon_delete = mastodon_delete.as_mut().map_err(|error| error.to_string())?;
        let rust_delete = rust_delete.as_mut().map_err(|error| error.to_string())?;
        normalize_generated_status_response(mastodon_delete, "Mastodon deleted status")?;
        normalize_generated_status_response(rust_delete, "Rust deleted status")?;
        compare_responses(
            mastodon_delete,
            rust_delete,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("status delete: {error}"))?;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    let mastodon_featured_after = featured_tag_state(mastodon_owner.url()).await?;
    let rust_featured_after = featured_tag_state(rust_writer.url()).await?;
    if operation.is_ok()
        && (mastodon_featured_after.0 != mastodon_featured_before.0 + 1
            || rust_featured_after.0 != rust_featured_before.0 + 1
            || mastodon_featured_after.1.is_none()
            || rust_featured_after.1.is_none())
    {
        return Err(format!(
            "featured tag state did not advance with the public hashtag: Mastodon={mastodon_featured_after:?}, Rust={rust_featured_after:?}"
        )
        .into());
    }
    restore_created_statuses(mastodon_owner.url(), &mastodon_ids).await?;
    restore_created_statuses(rust_writer.url(), &rust_ids).await?;
    cleanup_status_media(mastodon_owner.url()).await?;
    cleanup_status_media(rust_writer.url()).await?;
    restore_featured_tag(mastodon_owner.url(), &mastodon_featured_before).await?;
    restore_featured_tag(rust_writer.url(), &rust_featured_before).await?;
    restore_interaction_account_stat(
        mastodon_owner.url(),
        INTERACTION_ACCOUNT_ID,
        mastodon_account_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        rust_owner.url(),
        INTERACTION_ACCOUNT_ID,
        rust_account_stats.as_ref(),
    )
    .await?;
    restore_interaction_stat(
        mastodon_owner.url(),
        REBLOG_STATUS_ID,
        mastodon_reply_stats.as_ref(),
    )
    .await?;
    restore_interaction_stat(
        rust_writer.url(),
        REBLOG_STATUS_ID,
        rust_reply_stats.as_ref(),
    )
    .await?;
    operation
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_status_interaction_writes_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("interaction writer differential configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("interaction writer differential configuration must include a Rust writer URL");
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .expect("interaction writer differential configuration must include a Rust owner URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_bookmarks = interaction_rows(mastodon_owner.url(), "bookmarks").await?;
    let rust_bookmarks = interaction_rows(rust_writer.url(), "bookmarks").await?;
    let mastodon_reblogs = interaction_status_rows(mastodon_owner.url(), REBLOG_STATUS_ID).await?;
    let rust_reblogs = interaction_status_rows(rust_writer.url(), REBLOG_STATUS_ID).await?;
    let mastodon_conversations =
        interaction_conversation_rows(mastodon_owner.url(), REBLOG_STATUS_ID).await?;
    let rust_conversations =
        interaction_conversation_rows(rust_writer.url(), REBLOG_STATUS_ID).await?;
    let mastodon_conversation_mutes = interaction_conversation_mutes(mastodon_owner.url()).await?;
    let rust_conversation_mutes = interaction_conversation_mutes(rust_writer.url()).await?;
    let mastodon_status_pins = interaction_status_pins(mastodon_owner.url()).await?;
    let rust_status_pins = interaction_status_pins(rust_writer.url()).await?;
    let mastodon_favourites = interaction_rows(mastodon_owner.url(), "favourites").await?;
    let rust_favourites = interaction_rows(rust_writer.url(), "favourites").await?;
    let mastodon_reblog_stats = interaction_stat(mastodon_owner.url(), REBLOG_STATUS_ID).await?;
    let rust_reblog_stats = interaction_stat(rust_writer.url(), REBLOG_STATUS_ID).await?;
    let mastodon_account_stats =
        interaction_account_stat(mastodon_owner.url(), INTERACTION_ACCOUNT_ID).await?;
    let rust_account_stats =
        interaction_account_stat(rust_writer.url(), INTERACTION_ACCOUNT_ID).await?;
    let mastodon_stats = interaction_stat(mastodon_owner.url(), FAVOURITE_STATUS_ID).await?;
    let rust_stats = interaction_stat(rust_writer.url(), FAVOURITE_STATUS_ID).await?;
    let mastodon_target_notifications = notification_rows_for_account(
        mastodon_owner.url(),
        "notifications",
        FAVOURITE_TARGET_ACCOUNT_ID,
    )
    .await?;
    let rust_target_notifications = notification_rows_for_account(
        rust_writer.url(),
        "notifications",
        FAVOURITE_TARGET_ACCOUNT_ID,
    )
    .await?;
    let operation = async {
        for (label, method, path) in [
            (
                "bookmark create",
                Method::POST,
                format!("/api/v1/statuses/{BOOKMARK_STATUS_ID}/bookmark"),
            ),
            (
                "bookmark duplicate create",
                Method::POST,
                format!("/api/v1/statuses/{BOOKMARK_STATUS_ID}/bookmark"),
            ),
            (
                "bookmark delete",
                Method::POST,
                format!("/api/v1/statuses/{BOOKMARK_STATUS_ID}/unbookmark"),
            ),
            (
                "bookmark duplicate delete",
                Method::POST,
                format!("/api/v1/statuses/{BOOKMARK_STATUS_ID}/unbookmark"),
            ),
            (
                "status mute create",
                Method::POST,
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/mute"),
            ),
            (
                "status mute duplicate create",
                Method::POST,
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/mute"),
            ),
            (
                "status mute delete",
                Method::POST,
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/unmute"),
            ),
            (
                "status mute duplicate delete",
                Method::POST,
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/unmute"),
            ),
            (
                "status pin create",
                Method::POST,
                format!("/api/v1/statuses/{PIN_STATUS_ID}/pin"),
            ),
            (
                "status pin delete",
                Method::POST,
                format!("/api/v1/statuses/{PIN_STATUS_ID}/unpin"),
            ),
            (
                "reblog create",
                Method::POST,
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/reblog"),
            ),
            (
                "reblog duplicate create",
                Method::POST,
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/reblog"),
            ),
            (
                "reblog delete",
                Method::POST,
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/unreblog"),
            ),
            (
                "reblog duplicate delete",
                Method::POST,
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/unreblog"),
            ),
            (
                "favourite create",
                Method::POST,
                format!("/api/v1/statuses/{FAVOURITE_STATUS_ID}/favourite"),
            ),
            (
                "favourite duplicate create",
                Method::POST,
                format!("/api/v1/statuses/{FAVOURITE_STATUS_ID}/favourite"),
            ),
            (
                "favourite delete",
                Method::POST,
                format!("/api/v1/statuses/{FAVOURITE_STATUS_ID}/unfavourite"),
            ),
            (
                "favourite duplicate delete",
                Method::POST,
                format!("/api/v1/statuses/{FAVOURITE_STATUS_ID}/unfavourite"),
            ),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
            );
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            let request = RequestSpec::new(method, path, None, headers, Vec::new())?;
            let mut responses = send_identically(&targets, &request).await?;
            if label.starts_with("reblog ") && label.ends_with("create") {
                normalize_generated_reblog_response(&mut responses.mastodon, &mut responses.rust)
                    .map_err(|error| format!("{label}: {error}"))?;
            }
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    http::header::CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::VARY,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("{label}: {error}"))?;
            if interaction_conversation_mutes(mastodon_owner.url()).await?
                != interaction_conversation_mutes(rust_writer.url()).await?
            {
                return Err(format!("{label}: conversation mute rows differ").into());
            }
            if interaction_status_pin_state(mastodon_owner.url()).await?
                != interaction_status_pin_state(rust_writer.url()).await?
            {
                return Err(format!("{label}: status pin rows differ").into());
            }
            if label == "reblog delete" {
                drain_mastodon_reblog_removal(mastodon_owner.url(), REBLOG_STATUS_ID).await?;
            }
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    let notification_check: Result<(), Box<dyn Error>> = if operation.is_ok() {
        let mastodon_after = notification_rows_for_account(
            mastodon_owner.url(),
            "notifications",
            FAVOURITE_TARGET_ACCOUNT_ID,
        )
        .await?;
        let rust_after = notification_rows_for_account(
            rust_writer.url(),
            "notifications",
            FAVOURITE_TARGET_ACCOUNT_ID,
        )
        .await?;
        if mastodon_after != mastodon_target_notifications {
            Err("Mastodon status interaction changed target notifications".into())
        } else if rust_after != rust_target_notifications {
            Err("Rust status interaction changed target notifications".into())
        } else {
            Ok(())
        }
    } else {
        Ok(())
    };
    restore_interaction_rows(mastodon_owner.url(), "bookmarks", &mastodon_bookmarks).await?;
    restore_interaction_rows(rust_writer.url(), "bookmarks", &rust_bookmarks).await?;
    restore_interaction_conversation_rows(
        mastodon_owner.url(),
        REBLOG_STATUS_ID,
        &mastodon_conversations,
    )
    .await?;
    restore_interaction_conversation_rows(rust_writer.url(), REBLOG_STATUS_ID, &rust_conversations)
        .await?;
    restore_interaction_conversation_mutes(mastodon_owner.url(), &mastodon_conversation_mutes)
        .await?;
    restore_interaction_conversation_mutes(rust_writer.url(), &rust_conversation_mutes).await?;
    restore_interaction_status_pins(mastodon_owner.url(), &mastodon_status_pins).await?;
    restore_interaction_status_pins(rust_writer.url(), &rust_status_pins).await?;
    restore_interaction_status_rows(mastodon_owner.url(), REBLOG_STATUS_ID, &mastodon_reblogs)
        .await?;
    restore_interaction_status_rows(rust_writer.url(), REBLOG_STATUS_ID, &rust_reblogs).await?;
    restore_interaction_account_stat(
        mastodon_owner.url(),
        INTERACTION_ACCOUNT_ID,
        mastodon_account_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        rust_owner.url(),
        INTERACTION_ACCOUNT_ID,
        rust_account_stats.as_ref(),
    )
    .await?;
    restore_interaction_rows(mastodon_owner.url(), "favourites", &mastodon_favourites).await?;
    restore_interaction_rows(rust_writer.url(), "favourites", &rust_favourites).await?;
    restore_interaction_stat(
        mastodon_owner.url(),
        REBLOG_STATUS_ID,
        mastodon_reblog_stats.as_ref(),
    )
    .await?;
    restore_interaction_stat(
        rust_writer.url(),
        REBLOG_STATUS_ID,
        rust_reblog_stats.as_ref(),
    )
    .await?;
    restore_interaction_stat(
        mastodon_owner.url(),
        FAVOURITE_STATUS_ID,
        mastodon_stats.as_ref(),
    )
    .await?;
    restore_interaction_stat(rust_writer.url(), FAVOURITE_STATUS_ID, rust_stats.as_ref()).await?;
    restore_rows_for_account(
        mastodon_owner.url(),
        "notifications",
        FAVOURITE_TARGET_ACCOUNT_ID,
        &mastodon_target_notifications,
    )
    .await?;
    restore_rows_for_account(
        rust_writer.url(),
        "notifications",
        FAVOURITE_TARGET_ACCOUNT_ID,
        &rust_target_notifications,
    )
    .await?;
    operation?;
    notification_check?;
    run_concurrent_status_interactions_case(config.clone(), rust_url).await?;
    run_blocked_reblog_removal_case(config.clone(), rust_url).await?;
    run_blocked_saved_removal_case(config, rust_url).await
}

#[allow(clippy::too_many_lines)]
async fn run_blocked_reblog_removal_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("blocked reblog differential configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("blocked reblog differential configuration must include a Rust writer URL");
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .expect("blocked reblog differential configuration must include a Rust owner URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_reblogs =
        interaction_status_rows(mastodon_owner.url(), BLOCKED_REBLOG_TARGET_STATUS_ID).await?;
    let rust_reblogs =
        interaction_status_rows(rust_writer.url(), BLOCKED_REBLOG_TARGET_STATUS_ID).await?;
    let mastodon_conversations =
        interaction_conversation_rows(mastodon_owner.url(), BLOCKED_REBLOG_TARGET_STATUS_ID)
            .await?;
    let rust_conversations =
        interaction_conversation_rows(rust_writer.url(), BLOCKED_REBLOG_TARGET_STATUS_ID).await?;
    let mastodon_stats =
        interaction_stat(mastodon_owner.url(), BLOCKED_REBLOG_TARGET_STATUS_ID).await?;
    let rust_stats = interaction_stat(rust_writer.url(), BLOCKED_REBLOG_TARGET_STATUS_ID).await?;
    let mastodon_account_stats =
        interaction_account_stat(mastodon_owner.url(), INTERACTION_ACCOUNT_ID).await?;
    let rust_account_stats =
        interaction_account_stat(rust_writer.url(), INTERACTION_ACCOUNT_ID).await?;
    let mastodon_block = author_block_state(mastodon_owner.url()).await?;
    let rust_block = author_block_state(rust_writer.url()).await?;
    let operation = async {
        let create_request = status_interaction_request(
            &format!("/api/v1/statuses/{BLOCKED_REBLOG_TARGET_STATUS_ID}/reblog"),
            None,
        )?;
        let mut create_responses = send_identically(&targets, &create_request).await?;
        normalize_generated_reblog_response(
            &mut create_responses.mastodon,
            &mut create_responses.rust,
        )
        .map_err(|error| format!("blocked reblog create: {error}"))?;
        compare_responses(
            &create_responses.mastodon,
            &create_responses.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("blocked reblog create: {error}"))?;

        set_author_block(mastodon_owner.url(), true).await?;
        set_author_block(rust_writer.url(), true).await?;

        let remove_request = status_interaction_request(
            &format!("/api/v1/statuses/{BLOCKED_REBLOG_TARGET_STATUS_ID}/unreblog"),
            None,
        )?;
        let remove_responses = send_identically(&targets, &remove_request).await?;
        compare_responses(
            &remove_responses.mastodon,
            &remove_responses.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("blocked reblog removal: {error}"))?;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    restore_author_block(mastodon_owner.url(), mastodon_block.as_ref()).await?;
    restore_author_block(rust_writer.url(), rust_block.as_ref()).await?;
    restore_interaction_conversation_rows(
        mastodon_owner.url(),
        BLOCKED_REBLOG_TARGET_STATUS_ID,
        &mastodon_conversations,
    )
    .await?;
    restore_interaction_conversation_rows(
        rust_writer.url(),
        BLOCKED_REBLOG_TARGET_STATUS_ID,
        &rust_conversations,
    )
    .await?;
    restore_interaction_status_rows(
        mastodon_owner.url(),
        BLOCKED_REBLOG_TARGET_STATUS_ID,
        &mastodon_reblogs,
    )
    .await?;
    restore_interaction_status_rows(
        rust_writer.url(),
        BLOCKED_REBLOG_TARGET_STATUS_ID,
        &rust_reblogs,
    )
    .await?;
    restore_interaction_stat(
        mastodon_owner.url(),
        BLOCKED_REBLOG_TARGET_STATUS_ID,
        mastodon_stats.as_ref(),
    )
    .await?;
    restore_interaction_stat(
        rust_writer.url(),
        BLOCKED_REBLOG_TARGET_STATUS_ID,
        rust_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        mastodon_owner.url(),
        INTERACTION_ACCOUNT_ID,
        mastodon_account_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        rust_owner.url(),
        INTERACTION_ACCOUNT_ID,
        rust_account_stats.as_ref(),
    )
    .await?;
    operation
}

#[allow(clippy::too_many_lines)]
async fn run_blocked_saved_removal_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("blocked saved removal configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("blocked saved removal configuration must include a Rust writer URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_bookmarks = interaction_rows(mastodon_owner.url(), "bookmarks").await?;
    let rust_bookmarks = interaction_rows(rust_writer.url(), "bookmarks").await?;
    let mastodon_favourites = interaction_rows(mastodon_owner.url(), "favourites").await?;
    let rust_favourites = interaction_rows(rust_writer.url(), "favourites").await?;
    let mastodon_stats =
        interaction_stat(mastodon_owner.url(), BLOCKED_REBLOG_TARGET_STATUS_ID).await?;
    let rust_stats = interaction_stat(rust_writer.url(), BLOCKED_REBLOG_TARGET_STATUS_ID).await?;
    let mastodon_block = author_block_state(mastodon_owner.url()).await?;
    let rust_block = author_block_state(rust_writer.url()).await?;
    let operation = async {
        for (label, path) in [
            (
                "blocked bookmark create",
                format!("/api/v1/statuses/{BLOCKED_REBLOG_TARGET_STATUS_ID}/bookmark"),
            ),
            (
                "blocked favourite create",
                format!("/api/v1/statuses/{BLOCKED_REBLOG_TARGET_STATUS_ID}/favourite"),
            ),
        ] {
            let request = status_interaction_request(&path, None)?;
            let responses = send_identically(&targets, &request).await?;
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    http::header::CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::VARY,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("{label}: {error}"))?;
        }

        set_author_block(mastodon_owner.url(), true).await?;
        set_author_block(rust_writer.url(), true).await?;

        for (label, path) in [
            (
                "blocked bookmark removal",
                format!("/api/v1/statuses/{BLOCKED_REBLOG_TARGET_STATUS_ID}/unbookmark"),
            ),
            (
                "blocked favourite removal",
                format!("/api/v1/statuses/{BLOCKED_REBLOG_TARGET_STATUS_ID}/unfavourite"),
            ),
        ] {
            let request = status_interaction_request(&path, None)?;
            let responses = send_identically(&targets, &request).await?;
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    http::header::CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::VARY,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("{label}: {error}"))?;
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    restore_author_block(mastodon_owner.url(), mastodon_block.as_ref()).await?;
    restore_author_block(rust_writer.url(), rust_block.as_ref()).await?;
    restore_interaction_rows(mastodon_owner.url(), "bookmarks", &mastodon_bookmarks).await?;
    restore_interaction_rows(rust_writer.url(), "bookmarks", &rust_bookmarks).await?;
    restore_interaction_rows(mastodon_owner.url(), "favourites", &mastodon_favourites).await?;
    restore_interaction_rows(rust_writer.url(), "favourites", &rust_favourites).await?;
    restore_interaction_stat(
        mastodon_owner.url(),
        BLOCKED_REBLOG_TARGET_STATUS_ID,
        mastodon_stats.as_ref(),
    )
    .await?;
    restore_interaction_stat(
        rust_writer.url(),
        BLOCKED_REBLOG_TARGET_STATUS_ID,
        rust_stats.as_ref(),
    )
    .await?;
    operation
}

#[allow(clippy::too_many_lines)]
async fn run_concurrent_status_interactions_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config.mastodon_owner_database.as_ref().expect(
        "concurrent interaction differential configuration must include a Mastodon owner URL",
    );
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("concurrent interaction differential configuration must include a Rust writer URL");
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .expect("concurrent interaction differential configuration must include a Rust owner URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_bookmarks = interaction_rows(mastodon_owner.url(), "bookmarks").await?;
    let rust_bookmarks = interaction_rows(rust_writer.url(), "bookmarks").await?;
    let mastodon_favourites = interaction_rows(mastodon_owner.url(), "favourites").await?;
    let rust_favourites = interaction_rows(rust_writer.url(), "favourites").await?;
    let mastodon_reblogs = interaction_status_rows(mastodon_owner.url(), REBLOG_STATUS_ID).await?;
    let rust_reblogs = interaction_status_rows(rust_writer.url(), REBLOG_STATUS_ID).await?;
    let mastodon_conversations =
        interaction_conversation_rows(mastodon_owner.url(), REBLOG_STATUS_ID).await?;
    let rust_conversations =
        interaction_conversation_rows(rust_writer.url(), REBLOG_STATUS_ID).await?;
    let mastodon_stats = interaction_stat(mastodon_owner.url(), REBLOG_STATUS_ID).await?;
    let rust_stats = interaction_stat(rust_writer.url(), REBLOG_STATUS_ID).await?;
    let mastodon_favourite_stats =
        interaction_stat(mastodon_owner.url(), FAVOURITE_STATUS_ID).await?;
    let rust_favourite_stats = interaction_stat(rust_writer.url(), FAVOURITE_STATUS_ID).await?;
    let mastodon_account_stats =
        interaction_account_stat(mastodon_owner.url(), INTERACTION_ACCOUNT_ID).await?;
    let rust_account_stats =
        interaction_account_stat(rust_writer.url(), INTERACTION_ACCOUNT_ID).await?;
    let mastodon_notifications =
        status_interaction_notification_baseline(mastodon_owner.url()).await?;
    let rust_notifications = status_interaction_notification_baseline(rust_writer.url()).await?;
    let operation = async {
        for (label, seed_path, path, is_remove) in [
            (
                "concurrent bookmark create",
                String::new(),
                format!("/api/v1/statuses/{BOOKMARK_STATUS_ID}/bookmark"),
                false,
            ),
            (
                "concurrent bookmark remove",
                format!("/api/v1/statuses/{BOOKMARK_STATUS_ID}/bookmark"),
                format!("/api/v1/statuses/{BOOKMARK_STATUS_ID}/unbookmark"),
                true,
            ),
            (
                "concurrent favourite create",
                String::new(),
                format!("/api/v1/statuses/{FAVOURITE_STATUS_ID}/favourite"),
                false,
            ),
            (
                "concurrent favourite remove",
                format!("/api/v1/statuses/{FAVOURITE_STATUS_ID}/favourite"),
                format!("/api/v1/statuses/{FAVOURITE_STATUS_ID}/unfavourite"),
                true,
            ),
            (
                "concurrent reblog create",
                String::new(),
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/reblog"),
                false,
            ),
            (
                "concurrent reblog remove",
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/reblog"),
                format!("/api/v1/statuses/{REBLOG_STATUS_ID}/unreblog"),
                true,
            ),
        ] {
            restore_status_interaction_concurrency_pair(
                mastodon_owner.url(),
                mastodon_owner.url(),
                rust_writer.url(),
                rust_owner.url(),
                &mastodon_bookmarks,
                &rust_bookmarks,
                &mastodon_favourites,
                &rust_favourites,
                &mastodon_reblogs,
                &rust_reblogs,
                &mastodon_conversations,
                &rust_conversations,
                mastodon_stats.as_ref(),
                rust_stats.as_ref(),
                mastodon_favourite_stats.as_ref(),
                rust_favourite_stats.as_ref(),
                mastodon_account_stats.as_ref(),
                rust_account_stats.as_ref(),
                &mastodon_notifications,
                &rust_notifications,
            )
            .await?;
            if is_remove {
                let seed_request = status_interaction_request(&seed_path, None)?;
                let mut seeded = send_identically(&targets, &seed_request).await?;
                if label == "concurrent reblog remove"
                    && seeded.mastodon.status == 200
                    && seeded.rust.status == 200
                {
                    normalize_generated_reblog_response(&mut seeded.mastodon, &mut seeded.rust)
                        .map_err(|error| format!("{label} seed: {error}"))?;
                }
                compare_responses(
                    &seeded.mastodon,
                    &seeded.rust,
                    &[
                        http::header::CONTENT_TYPE,
                        http::header::CACHE_CONTROL,
                        http::header::VARY,
                    ],
                    &[],
                    DEFAULT_MISMATCH_LIMIT,
                )
                .map_err(|error| format!("{label} seed: {error}"))?;
            }
            let request = status_interaction_request(&path, None)?;
            let (mastodon_responses, rust_responses) = tokio::join!(
                concurrent_status_interaction_requests(targets.mastodon(), &request, "Mastodon"),
                concurrent_status_interaction_requests(targets.rust(), &request, "Rust")
            );
            let mastodon_responses = mastodon_responses?;
            let rust_responses = rust_responses?;
            for (index, (mut mastodon, mut rust)) in [
                (mastodon_responses.0, rust_responses.0),
                (mastodon_responses.1, rust_responses.1),
            ]
            .into_iter()
            .enumerate()
            {
                if matches!(
                    label,
                    "concurrent bookmark create"
                        | "concurrent favourite create"
                        | "concurrent reblog create"
                ) && mastodon.status == 422
                {
                    if !is_duplicate_record_response(&mastodon) {
                        return Err(format!(
                            "{label} response {index} was an unexpected Mastodon 422"
                        )
                        .into());
                    }
                    if rust.status != 200 {
                        return Err(format!(
                            "{label} response {index} had unexpected statuses: Mastodon={}, Rust={}"
                        , mastodon.status, rust.status)
                        .into());
                    }
                    continue;
                }
                if label == "concurrent reblog create" {
                    normalize_generated_reblog_response(&mut mastodon, &mut rust)
                        .map_err(|error| format!("{label} response {index}: {error}"))?;
                }
                if label == "concurrent reblog remove"
                    && mastodon.status == 200
                    && rust.status == 200
                {
                    normalize_concurrent_reblog_remove_response(&mut mastodon, &mut rust)
                        .map_err(|error| format!("{label} response {index}: {error}"))?;
                }
                compare_responses(
                    &mastodon,
                    &rust,
                    &[
                        http::header::CONTENT_TYPE,
                        http::header::CACHE_CONTROL,
                        http::header::VARY,
                    ],
                    &[],
                    DEFAULT_MISMATCH_LIMIT,
                )
                .map_err(|error| format!("{label} response {index}: {error}"))?;
            }
            if label == "concurrent favourite remove" {
                drain_mastodon_favourite_removal(mastodon_owner.url(), FAVOURITE_STATUS_ID).await?;
            } else if label == "concurrent reblog remove" {
                drain_mastodon_reblog_removal(mastodon_owner.url(), REBLOG_STATUS_ID).await?;
            }
            let (table, account_id, status_id) = match label {
                "concurrent bookmark create" | "concurrent bookmark remove" => {
                    ("bookmarks", INTERACTION_ACCOUNT_ID, BOOKMARK_STATUS_ID)
                }
                "concurrent favourite create" | "concurrent favourite remove" => {
                    ("favourites", INTERACTION_ACCOUNT_ID, FAVOURITE_STATUS_ID)
                }
                "concurrent reblog create" | "concurrent reblog remove" => {
                    ("statuses", INTERACTION_ACCOUNT_ID, REBLOG_STATUS_ID)
                }
                _ => unreachable!("concurrent interaction case is fixed"),
            };
            let mastodon_rows =
                interaction_concurrent_state(mastodon_owner.url(), table, account_id, status_id)
                    .await?;
            let rust_rows =
                interaction_concurrent_state(rust_writer.url(), table, account_id, status_id)
                    .await?;
            validate_concurrent_status_interaction_state(
                label,
                table,
                is_remove,
                "Mastodon",
                &mastodon_rows,
            )?;
            validate_concurrent_status_interaction_state(
                label,
                table,
                is_remove,
                "Rust",
                &rust_rows,
            )?;
            if mastodon_rows != rust_rows {
                return Err(format!(
                    "{label} persisted rows differ: Mastodon={mastodon_rows:?}, Rust={rust_rows:?}"
                )
                .into());
            }
            let counter_status_id = if label.contains("favourite") {
                FAVOURITE_STATUS_ID
            } else {
                REBLOG_STATUS_ID
            };
            let mastodon_stat = interaction_stat(mastodon_owner.url(), counter_status_id).await?;
            let rust_stat = interaction_stat(rust_writer.url(), counter_status_id).await?;
            if stable_interaction_stat(mastodon_stat.as_ref())
                != stable_interaction_stat(rust_stat.as_ref())
            {
                return Err(format!(
                    "{label} status counters differ: Mastodon={mastodon_stat:?}, Rust={rust_stat:?}"
                )
                    .into());
            }
            let mastodon_account_stat =
                interaction_account_stat(mastodon_owner.url(), INTERACTION_ACCOUNT_ID).await?;
            let rust_account_stat =
                interaction_account_stat(rust_writer.url(), INTERACTION_ACCOUNT_ID).await?;
            if stable_interaction_account_stat(mastodon_account_stat.as_ref())
                != stable_interaction_account_stat(rust_account_stat.as_ref())
            {
                return Err(format!(
                    "{label} account counters differ: Mastodon={mastodon_account_stat:?}, Rust={rust_account_stat:?}"
                )
                .into());
            }
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    restore_status_interaction_concurrency_pair(
        mastodon_owner.url(),
        mastodon_owner.url(),
        rust_writer.url(),
        rust_owner.url(),
        &mastodon_bookmarks,
        &rust_bookmarks,
        &mastodon_favourites,
        &rust_favourites,
        &mastodon_reblogs,
        &rust_reblogs,
        &mastodon_conversations,
        &rust_conversations,
        mastodon_stats.as_ref(),
        rust_stats.as_ref(),
        mastodon_favourite_stats.as_ref(),
        rust_favourite_stats.as_ref(),
        mastodon_account_stats.as_ref(),
        rust_account_stats.as_ref(),
        &mastodon_notifications,
        &rust_notifications,
    )
    .await?;
    operation
}

fn status_interaction_request(
    path: &str,
    body: Option<&str>,
) -> Result<RequestSpec, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let body = body.unwrap_or_default();
    if !body.is_empty() {
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
    }
    Ok(RequestSpec::new(
        Method::POST,
        path,
        None,
        headers,
        body.as_bytes().to_owned(),
    )?)
}

fn is_duplicate_record_response(response: &CapturedResponse) -> bool {
    serde_json::from_slice::<Value>(&response.body)
        .ok()
        .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_owned))
        .as_deref()
        == Some("Duplicate record")
}

async fn concurrent_status_interaction_requests(
    target: &Url,
    request: &RequestSpec,
    side: &'static str,
) -> Result<(CapturedResponse, CapturedResponse), Box<dyn Error>> {
    let first = send_single(target, request, side);
    let second = send_single(target, request, side);
    let (first, second) = tokio::join!(first, second);
    Ok((first?, second?))
}

async fn status_interaction_notification_baseline(
    url: &str,
) -> Result<StatusInteractionNotificationBaseline, sqlx::Error> {
    Ok(StatusInteractionNotificationBaseline {
        owner_notifications: notification_rows_for_account(
            url,
            "notifications",
            INTERACTION_ACCOUNT_ID,
        )
        .await?,
        target_notifications: notification_rows_for_account(
            url,
            "notifications",
            FAVOURITE_TARGET_ACCOUNT_ID,
        )
        .await?,
        owner_notification_requests: notification_rows_for_account(
            url,
            "notification_requests",
            INTERACTION_ACCOUNT_ID,
        )
        .await?,
        target_notification_requests: notification_rows_for_account(
            url,
            "notification_requests",
            FAVOURITE_TARGET_ACCOUNT_ID,
        )
        .await?,
    })
}

#[allow(clippy::too_many_arguments)]
async fn restore_status_interaction_concurrency_pair(
    mastodon_url: &str,
    mastodon_account_stats_url: &str,
    rust_url: &str,
    rust_account_stats_url: &str,
    mastodon_bookmarks: &[Value],
    rust_bookmarks: &[Value],
    mastodon_favourites: &[Value],
    rust_favourites: &[Value],
    mastodon_reblogs: &[Value],
    rust_reblogs: &[Value],
    mastodon_conversations: &[Value],
    rust_conversations: &[Value],
    mastodon_stats: Option<&Value>,
    rust_stats: Option<&Value>,
    mastodon_favourite_stats: Option<&Value>,
    rust_favourite_stats: Option<&Value>,
    mastodon_account_stats: Option<&Value>,
    rust_account_stats: Option<&Value>,
    mastodon_notifications: &StatusInteractionNotificationBaseline,
    rust_notifications: &StatusInteractionNotificationBaseline,
) -> Result<(), Box<dyn Error>> {
    let mastodon = restore_status_interaction_concurrency_baseline(
        mastodon_url,
        mastodon_account_stats_url,
        mastodon_bookmarks,
        mastodon_favourites,
        mastodon_reblogs,
        mastodon_conversations,
        mastodon_stats,
        mastodon_favourite_stats,
        mastodon_account_stats,
        mastodon_notifications,
    )
    .await;
    let rust = restore_status_interaction_concurrency_baseline(
        rust_url,
        rust_account_stats_url,
        rust_bookmarks,
        rust_favourites,
        rust_reblogs,
        rust_conversations,
        rust_stats,
        rust_favourite_stats,
        rust_account_stats,
        rust_notifications,
    )
    .await;
    match (mastodon, rust) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(mastodon), Err(rust)) => {
            Err(format!("Mastodon restore failed: {mastodon}; Rust restore failed: {rust}").into())
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(Box::new(error)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn restore_status_interaction_concurrency_baseline(
    url: &str,
    account_stats_url: &str,
    bookmarks: &[Value],
    favourites: &[Value],
    reblogs: &[Value],
    conversations: &[Value],
    status_stats: Option<&Value>,
    favourite_stats: Option<&Value>,
    account_stats: Option<&Value>,
    notifications: &StatusInteractionNotificationBaseline,
) -> Result<(), sqlx::Error> {
    restore_interaction_conversation_rows(url, REBLOG_STATUS_ID, conversations).await?;
    restore_interaction_status_rows(url, REBLOG_STATUS_ID, reblogs).await?;
    restore_interaction_rows(url, "bookmarks", bookmarks).await?;
    restore_interaction_rows(url, "favourites", favourites).await?;
    restore_interaction_stat(url, REBLOG_STATUS_ID, status_stats).await?;
    restore_interaction_stat(url, FAVOURITE_STATUS_ID, favourite_stats).await?;
    restore_rows_for_account(
        url,
        "notifications",
        INTERACTION_ACCOUNT_ID,
        &notifications.owner_notifications,
    )
    .await?;
    restore_rows_for_account(
        url,
        "notifications",
        FAVOURITE_TARGET_ACCOUNT_ID,
        &notifications.target_notifications,
    )
    .await?;
    restore_rows_for_account(
        url,
        "notification_requests",
        INTERACTION_ACCOUNT_ID,
        &notifications.owner_notification_requests,
    )
    .await?;
    restore_rows_for_account(
        url,
        "notification_requests",
        FAVOURITE_TARGET_ACCOUNT_ID,
        &notifications.target_notification_requests,
    )
    .await?;
    restore_interaction_account_stat(account_stats_url, INTERACTION_ACCOUNT_ID, account_stats).await
}

async fn drain_mastodon_favourite_removal(url: &str, status_id: i64) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    let activity_id = sqlx::query_scalar::<_, i64>(
        "DELETE FROM public.favourites WHERE account_id = $1 AND status_id = $2 RETURNING id",
    )
    .bind(INTERACTION_ACCOUNT_ID)
    .bind(status_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(activity_id) = activity_id {
        sqlx::query(
            "UPDATE public.status_stats SET \
               favourites_count = GREATEST(favourites_count - 1, 0), \
               untrusted_favourites_count = CASE \
                 WHEN untrusted_favourites_count IS NULL THEN NULL \
                 ELSE GREATEST(untrusted_favourites_count - 1, 0) END, \
               updated_at = clock_timestamp() \
             WHERE status_id = $1",
        )
        .bind(status_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "DELETE FROM public.notifications \
             WHERE account_id = $1 AND activity_type = 'Favourite' AND activity_id = $2",
        )
        .bind(FAVOURITE_TARGET_ACCOUNT_ID)
        .bind(activity_id)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}

async fn drain_mastodon_reblog_removal(url: &str, status_id: i64) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "UPDATE public.status_stats SET \
           reblogs_count = GREATEST(reblogs_count - 1, 0), \
           untrusted_reblogs_count = CASE \
             WHEN untrusted_reblogs_count IS NULL THEN NULL \
             ELSE GREATEST(untrusted_reblogs_count - 1, 0) END, \
           updated_at = clock_timestamp() \
         WHERE status_id = $1",
    )
    .bind(status_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE public.account_stats SET statuses_count = GREATEST(statuses_count - 1, 0), \
         updated_at = clock_timestamp() WHERE account_id = $1",
    )
    .bind(INTERACTION_ACCOUNT_ID)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await
}

async fn interaction_concurrent_state(
    url: &str,
    table: &str,
    account_id: i64,
    status_id: i64,
) -> Result<Vec<Value>, sqlx::Error> {
    let query = match table {
        "bookmarks" | "favourites" => format!(
            "SELECT jsonb_build_object('present', true) FROM public.{table} \
             WHERE account_id = $1 AND status_id = $2 ORDER BY id"
        ),
        "statuses" => {
            "SELECT jsonb_build_object('visibility', visibility, 'reblog_of_id', reblog_of_id, \
                     'deleted', deleted_at IS NOT NULL) FROM public.statuses \
             WHERE account_id = $1 AND reblog_of_id = $2 ORDER BY id"
                .to_owned()
        }
        _ => unreachable!("concurrent status interaction state uses fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(&query)
        .bind(account_id)
        .bind(status_id)
        .fetch_all(&mut connection)
        .await
}

fn validate_concurrent_status_interaction_state(
    label: &str,
    table: &str,
    is_remove: bool,
    side: &str,
    rows: &[Value],
) -> Result<(), Box<dyn Error>> {
    match table {
        "bookmarks" | "favourites" => {
            let expected = usize::from(!is_remove);
            if rows.len() != expected {
                return Err(format!(
                    "{label} {side} state had {} rows; expected {expected}: {rows:?}",
                    rows.len()
                )
                .into());
            }
        }
        "statuses" => {
            if rows.len() != 1 {
                return Err(format!(
                    "{label} {side} state had {} reblog rows; expected exactly one: {rows:?}",
                    rows.len()
                )
                .into());
            }
            let deleted = rows[0].get("deleted").and_then(Value::as_bool);
            if deleted != Some(is_remove) {
                return Err(format!(
                    "{label} {side} reblog deleted state was {deleted:?}; expected {is_remove}: {rows:?}"
                )
                .into());
            }
        }
        _ => unreachable!("concurrent status interaction state uses fixed tables"),
    }
    Ok(())
}

fn stable_interaction_stat(row: Option<&Value>) -> Option<(Option<i64>, Option<i64>, Option<i64>)> {
    row.map(|row| {
        (
            row.get("reblogs_count").and_then(Value::as_i64),
            row.get("favourites_count").and_then(Value::as_i64),
            row.get("quotes_count").and_then(Value::as_i64),
        )
    })
}

fn stable_interaction_account_stat(row: Option<&Value>) -> Option<StableInteractionAccountStat> {
    row.map(|row| {
        (
            row.get("statuses_count").and_then(Value::as_i64),
            row.get("followers_count").and_then(Value::as_i64),
            row.get("following_count").and_then(Value::as_i64),
            row.get("media_count").and_then(Value::as_i64),
        )
    })
}

#[allow(clippy::too_many_lines)]
async fn run_concurrent_relationship_writes_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config.mastodon_owner_database.as_ref().expect(
        "concurrent relationship differential configuration must include a Mastodon owner URL",
    );
    let rust_writer = config.rust_write_database.as_ref().expect(
        "concurrent relationship differential configuration must include a Rust writer URL",
    );
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .expect("concurrent relationship differential configuration must include a Rust owner URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_baseline = relationship_concurrency_baseline(mastodon_owner.url()).await?;
    let rust_baseline = relationship_concurrency_baseline(rust_writer.url()).await?;
    let operation = async {
        for (label, path, body) in [
            (
                "concurrent follow",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/follow"),
                Some("reblogs=false&notify=true&languages%5B%5D=fr"),
            ),
            (
                "concurrent block",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/block"),
                None,
            ),
            (
                "concurrent mute",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/mute"),
                Some("notifications=false"),
            ),
        ] {
            restore_relationship_concurrency_baseline(
                mastodon_owner.url(),
                mastodon_owner.url(),
                &mastodon_baseline,
            )
            .await?;
            restore_relationship_concurrency_baseline(
                rust_writer.url(),
                rust_owner.url(),
                &rust_baseline,
            )
            .await?;
            let request = relationship_request(&path, body)?;
            let (mastodon_responses, rust_responses) = tokio::try_join!(
                concurrent_relationship_requests(targets.mastodon(), &request, "Mastodon"),
                concurrent_relationship_requests(targets.rust(), &request, "Rust")
            )?;
            for (index, (mastodon, rust)) in [
                (mastodon_responses.0, rust_responses.0),
                (mastodon_responses.1, rust_responses.1),
            ]
            .into_iter()
            .enumerate()
            {
                if matches!(
                    label,
                    "concurrent bookmark"
                        | "concurrent favourite"
                        | "concurrent block"
                        | "concurrent mute"
                        | "concurrent reblog"
                )
                    && mastodon.status == 422
                {
                    if rust.status != 200 {
                        return Err(format!(
                            "{label} response {index} had unexpected statuses: Mastodon={}, Rust={}"
                        , mastodon.status, rust.status)
                        .into());
                    }
                    continue;
                }
                compare_responses(
                    &mastodon,
                    &rust,
                    &[
                        http::header::CONTENT_TYPE,
                        http::header::CACHE_CONTROL,
                        http::header::VARY,
                    ],
                    &[],
                    DEFAULT_MISMATCH_LIMIT,
                )
                .map_err(|error| format!("{label} response {index}: {error}"))?;
            }
            for table in ["follows", "follow_requests", "blocks", "mutes"] {
                let mastodon_state = relationship_pair_state(
                    mastodon_owner.url(),
                    table,
                    INTERACTION_ACCOUNT_ID,
                    RELATIONSHIP_TARGET_ACCOUNT_ID,
                )
                .await?;
                let rust_state = relationship_pair_state(
                    rust_writer.url(),
                    table,
                    INTERACTION_ACCOUNT_ID,
                    RELATIONSHIP_TARGET_ACCOUNT_ID,
                )
                .await?;
                if mastodon_state != rust_state {
                    return Err(format!(
                        "{label} {table} state differs: Mastodon={mastodon_state:?}, Rust={rust_state:?}"
                    )
                    .into());
                }
            }
            for account_id in [INTERACTION_ACCOUNT_ID, RELATIONSHIP_TARGET_ACCOUNT_ID] {
                let mastodon_counters =
                    relationship_counter_state(mastodon_owner.url(), account_id).await?;
                let rust_counters = relationship_counter_state(rust_writer.url(), account_id).await?;
                if mastodon_counters != rust_counters {
                    return Err(format!(
                        "{label} account {account_id} counters differ: Mastodon={mastodon_counters:?}, Rust={rust_counters:?}"
                    )
                    .into());
                }
            }
            if label != "concurrent follow" {
                for (table, account_id) in [
                    ("notifications", INTERACTION_ACCOUNT_ID),
                    ("notifications", RELATIONSHIP_TARGET_ACCOUNT_ID),
                    ("notification_requests", INTERACTION_ACCOUNT_ID),
                    ("notification_requests", RELATIONSHIP_TARGET_ACCOUNT_ID),
                ] {
                    let mastodon_rows = notification_rows_for_account(
                        mastodon_owner.url(),
                        table,
                        account_id,
                    )
                    .await?;
                    let rust_rows =
                        notification_rows_for_account(rust_writer.url(), table, account_id).await?;
                    if mastodon_rows != rust_rows {
                        return Err(format!(
                            "{label} {table} account {account_id} rows differ: Mastodon={mastodon_rows:?}, Rust={rust_rows:?}"
                        )
                        .into());
                    }
                }
            }
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    restore_relationship_concurrency_baseline(
        mastodon_owner.url(),
        mastodon_owner.url(),
        &mastodon_baseline,
    )
    .await?;
    restore_relationship_concurrency_baseline(rust_writer.url(), rust_owner.url(), &rust_baseline)
        .await?;
    operation
}

async fn relationship_concurrency_baseline(
    url: &str,
) -> Result<RelationshipConcurrencyBaseline, sqlx::Error> {
    Ok(RelationshipConcurrencyBaseline {
        follows: relationship_rows(url, "follows").await?,
        requests: relationship_rows(url, "follow_requests").await?,
        incoming_follows: relationship_target_rows(url, "follows", INTERACTION_ACCOUNT_ID).await?,
        incoming_requests: relationship_target_rows(url, "follow_requests", INTERACTION_ACCOUNT_ID)
            .await?,
        blocks: relationship_rows(url, "blocks").await?,
        mutes: relationship_rows(url, "mutes").await?,
        source_notifications: notification_rows_for_account(
            url,
            "notifications",
            INTERACTION_ACCOUNT_ID,
        )
        .await?,
        target_notifications: notification_rows_for_account(
            url,
            "notifications",
            RELATIONSHIP_TARGET_ACCOUNT_ID,
        )
        .await?,
        source_notification_requests: notification_rows_for_account(
            url,
            "notification_requests",
            INTERACTION_ACCOUNT_ID,
        )
        .await?,
        target_notification_requests: notification_rows_for_account(
            url,
            "notification_requests",
            RELATIONSHIP_TARGET_ACCOUNT_ID,
        )
        .await?,
        source_stats: interaction_account_stat(url, INTERACTION_ACCOUNT_ID).await?,
        target_stats: interaction_account_stat(url, RELATIONSHIP_TARGET_ACCOUNT_ID).await?,
    })
}

async fn restore_relationship_concurrency_baseline(
    url: &str,
    account_stats_url: &str,
    baseline: &RelationshipConcurrencyBaseline,
) -> Result<(), sqlx::Error> {
    restore_relationship_rows(url, "follows", &baseline.follows).await?;
    restore_relationship_rows(url, "follow_requests", &baseline.requests).await?;
    restore_relationship_target_rows(
        url,
        "follows",
        INTERACTION_ACCOUNT_ID,
        &baseline.incoming_follows,
    )
    .await?;
    restore_relationship_target_rows(
        url,
        "follow_requests",
        INTERACTION_ACCOUNT_ID,
        &baseline.incoming_requests,
    )
    .await?;
    restore_relationship_rows(url, "blocks", &baseline.blocks).await?;
    restore_relationship_rows(url, "mutes", &baseline.mutes).await?;
    restore_rows_for_account(
        url,
        "notifications",
        INTERACTION_ACCOUNT_ID,
        &baseline.source_notifications,
    )
    .await?;
    restore_rows_for_account(
        url,
        "notifications",
        RELATIONSHIP_TARGET_ACCOUNT_ID,
        &baseline.target_notifications,
    )
    .await?;
    restore_rows_for_account(
        url,
        "notification_requests",
        INTERACTION_ACCOUNT_ID,
        &baseline.source_notification_requests,
    )
    .await?;
    restore_rows_for_account(
        url,
        "notification_requests",
        RELATIONSHIP_TARGET_ACCOUNT_ID,
        &baseline.target_notification_requests,
    )
    .await?;
    restore_interaction_account_stat(
        account_stats_url,
        INTERACTION_ACCOUNT_ID,
        baseline.source_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        account_stats_url,
        RELATIONSHIP_TARGET_ACCOUNT_ID,
        baseline.target_stats.as_ref(),
    )
    .await
}

fn relationship_request(path: &str, body: Option<&str>) -> Result<RequestSpec, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-follow-v4-6-5"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let body = body.unwrap_or_default();
    if !body.is_empty() {
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
    }
    Ok(RequestSpec::new(
        Method::POST,
        path,
        None,
        headers,
        body.as_bytes().to_owned(),
    )?)
}

async fn concurrent_relationship_requests(
    target: &Url,
    request: &RequestSpec,
    side: &'static str,
) -> Result<(CapturedResponse, CapturedResponse), Box<dyn Error>> {
    let first = send_single(target, request, side);
    let second = send_single(target, request, side);
    let (first, second) = tokio::join!(first, second);
    Ok((first?, second?))
}

async fn relationship_pair_state(
    url: &str,
    table: &str,
    account_id: i64,
    target_account_id: i64,
) -> Result<Vec<Value>, sqlx::Error> {
    let query = match table {
        "follows" | "follow_requests" => format!(
            "SELECT jsonb_build_object(\
                       'show_reblogs', show_reblogs, 'notify', notify, 'languages', languages) \
             FROM public.{table} WHERE account_id = $1 AND target_account_id = $2 ORDER BY id"
        ),
        "blocks" => "SELECT jsonb_build_object('present', true) FROM public.blocks \
             WHERE account_id = $1 AND target_account_id = $2 ORDER BY id"
            .to_owned(),
        "mutes" => "SELECT jsonb_build_object('hide_notifications', hide_notifications, \
                     'expires_at', expires_at) FROM public.mutes \
             WHERE account_id = $1 AND target_account_id = $2 ORDER BY id"
            .to_owned(),
        _ => unreachable!("concurrent relationship state uses fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(&query)
        .bind(account_id)
        .bind(target_account_id)
        .fetch_all(&mut connection)
        .await
}

async fn relationship_counter_state(
    url: &str,
    account_id: i64,
) -> Result<Option<(i64, i64)>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_as(
        "SELECT following_count, followers_count FROM public.account_stats WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_optional(&mut connection)
    .await
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_relationship_writes_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("relationship writer differential configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("relationship writer differential configuration must include a Rust writer URL");
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .expect("relationship writer differential configuration must include a Rust owner URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_follows = relationship_rows(mastodon_owner.url(), "follows").await?;
    let rust_follows = relationship_rows(rust_writer.url(), "follows").await?;
    let mastodon_requests = relationship_rows(mastodon_owner.url(), "follow_requests").await?;
    let rust_requests = relationship_rows(rust_writer.url(), "follow_requests").await?;
    let mastodon_incoming_follows =
        relationship_target_rows(mastodon_owner.url(), "follows", INTERACTION_ACCOUNT_ID).await?;
    let rust_incoming_follows =
        relationship_target_rows(rust_writer.url(), "follows", INTERACTION_ACCOUNT_ID).await?;
    let mastodon_incoming_requests = relationship_target_rows(
        mastodon_owner.url(),
        "follow_requests",
        INTERACTION_ACCOUNT_ID,
    )
    .await?;
    let rust_incoming_requests =
        relationship_target_rows(rust_writer.url(), "follow_requests", INTERACTION_ACCOUNT_ID)
            .await?;
    let mastodon_blocks = relationship_rows(mastodon_owner.url(), "blocks").await?;
    let rust_blocks = relationship_rows(rust_writer.url(), "blocks").await?;
    let mastodon_mutes = relationship_rows(mastodon_owner.url(), "mutes").await?;
    let rust_mutes = relationship_rows(rust_writer.url(), "mutes").await?;
    let mastodon_notifications = notification_rows_for_account(
        mastodon_owner.url(),
        "notifications",
        RELATIONSHIP_TARGET_ACCOUNT_ID,
    )
    .await?;
    let rust_notifications = notification_rows_for_account(
        rust_writer.url(),
        "notifications",
        RELATIONSHIP_TARGET_ACCOUNT_ID,
    )
    .await?;
    let mastodon_owner_notifications = notification_rows_for_account(
        mastodon_owner.url(),
        "notifications",
        INTERACTION_ACCOUNT_ID,
    )
    .await?;
    let rust_owner_notifications =
        notification_rows_for_account(rust_writer.url(), "notifications", INTERACTION_ACCOUNT_ID)
            .await?;
    let mastodon_notification_requests = notification_rows_for_account(
        mastodon_owner.url(),
        "notification_requests",
        RELATIONSHIP_TARGET_ACCOUNT_ID,
    )
    .await?;
    let rust_notification_requests = notification_rows_for_account(
        rust_writer.url(),
        "notification_requests",
        RELATIONSHIP_TARGET_ACCOUNT_ID,
    )
    .await?;
    let mastodon_owner_notification_requests = notification_rows_for_account(
        mastodon_owner.url(),
        "notification_requests",
        INTERACTION_ACCOUNT_ID,
    )
    .await?;
    let rust_owner_notification_requests = notification_rows_for_account(
        rust_writer.url(),
        "notification_requests",
        INTERACTION_ACCOUNT_ID,
    )
    .await?;
    let mastodon_source_stats =
        interaction_account_stat(mastodon_owner.url(), INTERACTION_ACCOUNT_ID).await?;
    let rust_source_stats =
        interaction_account_stat(rust_writer.url(), INTERACTION_ACCOUNT_ID).await?;
    let mastodon_target_stats =
        interaction_account_stat(mastodon_owner.url(), RELATIONSHIP_TARGET_ACCOUNT_ID).await?;
    let rust_target_stats =
        interaction_account_stat(rust_writer.url(), RELATIONSHIP_TARGET_ACCOUNT_ID).await?;
    let mastodon_request_source_stats =
        interaction_account_stat(mastodon_owner.url(), RELATIONSHIP_REQUEST_SOURCE_ACCOUNT_ID)
            .await?;
    let rust_request_source_stats =
        interaction_account_stat(rust_writer.url(), RELATIONSHIP_REQUEST_SOURCE_ACCOUNT_ID).await?;
    let mastodon_follower_stats =
        interaction_account_stat(mastodon_owner.url(), RELATIONSHIP_FOLLOWER_ACCOUNT_ID).await?;
    let rust_follower_stats =
        interaction_account_stat(rust_writer.url(), RELATIONSHIP_FOLLOWER_ACCOUNT_ID).await?;
    let operation = async {
        for (label, path, body) in [
            (
                "follow create",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/follow"),
                Some("reblogs=false&notify=true&languages%5B%5D=fr"),
            ),
            (
                "follow duplicate",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/follow"),
                Some("reblogs=false&notify=true&languages%5B%5D=fr"),
            ),
            (
                "unfollow",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/unfollow"),
                None,
            ),
            (
                "unfollow duplicate",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/unfollow"),
                None,
            ),
            (
                "block create",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/block"),
                None,
            ),
            (
                "block duplicate",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/block"),
                None,
            ),
            (
                "unblock",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/unblock"),
                None,
            ),
            (
                "unblock duplicate",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/unblock"),
                None,
            ),
            (
                "mute create",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/mute"),
                Some("notifications=false"),
            ),
            (
                "mute duplicate",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/mute"),
                Some("notifications=false"),
            ),
            (
                "unmute",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/unmute"),
                None,
            ),
            (
                "unmute duplicate",
                format!("/api/v1/accounts/{RELATIONSHIP_TARGET_ACCOUNT_ID}/unmute"),
                None,
            ),
            (
                "follow request authorize",
                format!(
                    "/api/v1/follow_requests/{RELATIONSHIP_REQUEST_SOURCE_ACCOUNT_ID}/authorize"
                ),
                None,
            ),
            (
                "follow request authorize duplicate",
                format!(
                    "/api/v1/follow_requests/{RELATIONSHIP_REQUEST_SOURCE_ACCOUNT_ID}/authorize"
                ),
                None,
            ),
            (
                "remove follower",
                format!(
                    "/api/v1/accounts/{RELATIONSHIP_FOLLOWER_ACCOUNT_ID}/remove_from_followers"
                ),
                None,
            ),
            (
                "remove follower duplicate",
                format!(
                    "/api/v1/accounts/{RELATIONSHIP_FOLLOWER_ACCOUNT_ID}/remove_from_followers"
                ),
                None,
            ),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_static("Bearer fixture-bearer-follow-v4-6-5"),
            );
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            let body = body
                .map(str::as_bytes)
                .map(ToOwned::to_owned)
                .unwrap_or_default();
            if !body.is_empty() {
                headers.insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/x-www-form-urlencoded"),
                );
            }
            let request = RequestSpec::new(Method::POST, path, None, headers, body)?;
            let responses = send_identically(&targets, &request).await?;
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    http::header::CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::VARY,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("{label}: {error}"))?;
        }
        restore_relationship_target_rows(
            mastodon_owner.url(),
            "follows",
            INTERACTION_ACCOUNT_ID,
            &mastodon_incoming_follows,
        )
        .await?;
        restore_relationship_target_rows(
            rust_writer.url(),
            "follows",
            INTERACTION_ACCOUNT_ID,
            &rust_incoming_follows,
        )
        .await?;
        restore_relationship_target_rows(
            mastodon_owner.url(),
            "follow_requests",
            INTERACTION_ACCOUNT_ID,
            &mastodon_incoming_requests,
        )
        .await?;
        restore_relationship_target_rows(
            rust_writer.url(),
            "follow_requests",
            INTERACTION_ACCOUNT_ID,
            &rust_incoming_requests,
        )
        .await?;
        for (label, path) in [
            (
                "follow request reject",
                format!("/api/v1/follow_requests/{RELATIONSHIP_REQUEST_SOURCE_ACCOUNT_ID}/reject"),
            ),
            (
                "follow request reject duplicate",
                format!("/api/v1/follow_requests/{RELATIONSHIP_REQUEST_SOURCE_ACCOUNT_ID}/reject"),
            ),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_static("Bearer fixture-bearer-follow-v4-6-5"),
            );
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            let request = RequestSpec::new(Method::POST, path, None, headers, Vec::new())?;
            let responses = send_identically(&targets, &request).await?;
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    http::header::CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::VARY,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("{label}: {error}"))?;
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    let notification_check = async {
        if operation.is_ok() {
            let checks = [
                (
                    mastodon_owner.url(),
                    &mastodon_notifications,
                    RELATIONSHIP_TARGET_ACCOUNT_ID,
                    "Mastodon target notifications",
                ),
                (
                    rust_writer.url(),
                    &rust_notifications,
                    RELATIONSHIP_TARGET_ACCOUNT_ID,
                    "Rust target notifications",
                ),
            ];
            for (url, expected, account_id, label) in checks {
                if notification_rows_for_account(url, "notifications", account_id).await?
                    != *expected
                {
                    return Err(format!("{label} changed during relationship writes").into());
                }
            }
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    restore_relationship_rows(mastodon_owner.url(), "follows", &mastodon_follows).await?;
    restore_relationship_rows(rust_writer.url(), "follows", &rust_follows).await?;
    restore_relationship_rows(mastodon_owner.url(), "follow_requests", &mastodon_requests).await?;
    restore_relationship_rows(rust_writer.url(), "follow_requests", &rust_requests).await?;
    restore_relationship_target_rows(
        mastodon_owner.url(),
        "follows",
        INTERACTION_ACCOUNT_ID,
        &mastodon_incoming_follows,
    )
    .await?;
    restore_relationship_target_rows(
        rust_writer.url(),
        "follows",
        INTERACTION_ACCOUNT_ID,
        &rust_incoming_follows,
    )
    .await?;
    restore_relationship_target_rows(
        mastodon_owner.url(),
        "follow_requests",
        INTERACTION_ACCOUNT_ID,
        &mastodon_incoming_requests,
    )
    .await?;
    restore_relationship_target_rows(
        rust_writer.url(),
        "follow_requests",
        INTERACTION_ACCOUNT_ID,
        &rust_incoming_requests,
    )
    .await?;
    restore_relationship_rows(mastodon_owner.url(), "blocks", &mastodon_blocks).await?;
    restore_relationship_rows(rust_writer.url(), "blocks", &rust_blocks).await?;
    restore_relationship_rows(mastodon_owner.url(), "mutes", &mastodon_mutes).await?;
    restore_relationship_rows(rust_writer.url(), "mutes", &rust_mutes).await?;
    restore_rows_for_account(
        mastodon_owner.url(),
        "notifications",
        RELATIONSHIP_TARGET_ACCOUNT_ID,
        &mastodon_notifications,
    )
    .await?;
    restore_rows_for_account(
        rust_writer.url(),
        "notifications",
        RELATIONSHIP_TARGET_ACCOUNT_ID,
        &rust_notifications,
    )
    .await?;
    restore_rows_for_account(
        mastodon_owner.url(),
        "notifications",
        INTERACTION_ACCOUNT_ID,
        &mastodon_owner_notifications,
    )
    .await?;
    restore_rows_for_account(
        rust_writer.url(),
        "notifications",
        INTERACTION_ACCOUNT_ID,
        &rust_owner_notifications,
    )
    .await?;
    restore_rows_for_account(
        mastodon_owner.url(),
        "notification_requests",
        RELATIONSHIP_TARGET_ACCOUNT_ID,
        &mastodon_notification_requests,
    )
    .await?;
    restore_rows_for_account(
        rust_writer.url(),
        "notification_requests",
        RELATIONSHIP_TARGET_ACCOUNT_ID,
        &rust_notification_requests,
    )
    .await?;
    restore_rows_for_account(
        mastodon_owner.url(),
        "notification_requests",
        INTERACTION_ACCOUNT_ID,
        &mastodon_owner_notification_requests,
    )
    .await?;
    restore_rows_for_account(
        rust_writer.url(),
        "notification_requests",
        INTERACTION_ACCOUNT_ID,
        &rust_owner_notification_requests,
    )
    .await?;
    restore_interaction_account_stat(
        mastodon_owner.url(),
        INTERACTION_ACCOUNT_ID,
        mastodon_source_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        rust_owner.url(),
        INTERACTION_ACCOUNT_ID,
        rust_source_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        mastodon_owner.url(),
        RELATIONSHIP_TARGET_ACCOUNT_ID,
        mastodon_target_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        rust_owner.url(),
        RELATIONSHIP_TARGET_ACCOUNT_ID,
        rust_target_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        mastodon_owner.url(),
        RELATIONSHIP_REQUEST_SOURCE_ACCOUNT_ID,
        mastodon_request_source_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        rust_owner.url(),
        RELATIONSHIP_REQUEST_SOURCE_ACCOUNT_ID,
        rust_request_source_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        mastodon_owner.url(),
        RELATIONSHIP_FOLLOWER_ACCOUNT_ID,
        mastodon_follower_stats.as_ref(),
    )
    .await?;
    restore_interaction_account_stat(
        rust_owner.url(),
        RELATIONSHIP_FOLLOWER_ACCOUNT_ID,
        rust_follower_stats.as_ref(),
    )
    .await?;
    operation?;
    notification_check?;
    run_concurrent_relationship_writes_case(config, rust_url).await
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_notification_writes_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("notification writer differential configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("notification writer differential configuration must include a Rust writer URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_notifications = notification_rows(mastodon_owner.url(), "notifications").await?;
    let mastodon_requests =
        notification_rows(mastodon_owner.url(), "notification_requests").await?;
    let rust_notifications = notification_rows(rust_writer.url(), "notifications").await?;
    let rust_requests = notification_rows(rust_writer.url(), "notification_requests").await?;
    let mastodon_application_max_id = oauth_application_max_id(mastodon_owner.url()).await?;
    let rust_application_max_id = oauth_application_max_id(rust_writer.url()).await?;
    let mastodon_policies = notification_policy_rows(mastodon_owner.url())
        .await
        .map_err(|error| format!("Mastodon policy baseline: {error}"))?;
    let rust_policies = notification_policy_rows(rust_writer.url())
        .await
        .map_err(|error| format!("Rust policy baseline: {error}"))?;
    if mastodon_policies != rust_policies {
        return Err("notification policy baseline differs".into());
    }
    let mastodon_permissions = notification_permission_rows(mastodon_owner.url()).await?;
    let rust_permissions = notification_permission_rows(rust_writer.url()).await?;
    if notification_permission_pairs(&mastodon_permissions)
        != notification_permission_pairs(&rust_permissions)
    {
        return Err("notification permission baseline differs".into());
    }
    let mastodon_marker = marker_state(mastodon_owner.url()).await?;
    let rust_marker = marker_state(rust_writer.url()).await?;
    let rust_creation = async {
        for (label, body, basic_auth) in [
            (
                "oauth client credentials default scope",
                "grant_type=client_credentials&client_id=rustodon-fixture-client-v4-6-5&client_secret=fixture-only-client-secret-v4-6-5",
                None,
            ),
            (
                "oauth client credentials basic authentication",
                "grant_type=client_credentials",
                Some(STANDARD.encode(
                    "rustodon-fixture-client-v4-6-5:fixture-only-client-secret-v4-6-5",
                )),
            ),
            (
                "oauth client credentials invalid scope",
                "grant_type=client_credentials&client_id=rustodon-fixture-client-v4-6-5&client_secret=fixture-only-client-secret-v4-6-5&scope=read+read%3Astatuses",
                None,
            ),
            (
                "oauth client credentials invalid client",
                "grant_type=client_credentials&client_id=rustodon-fixture-client-v4-6-5&client_secret=wrong",
                None,
            ),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            );
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            if let Some(value) = basic_auth {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Basic {value}"))?,
                );
            }
            let request = RequestSpec::new(
                Method::POST,
                "/oauth/token",
                None,
                headers,
                body.as_bytes().to_vec(),
            )?;
            let responses = send_identically(&targets, &request).await?;
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::PRAGMA,
                    http::header::WWW_AUTHENTICATE,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("{label}: {error}"))?;
        }

        let mut app_headers = HeaderMap::new();
        app_headers.insert(
            HOST,
            HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
        );
        app_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        app_headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        app_headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        let app_request = RequestSpec::new(
            Method::POST,
            "/api/v1/apps/",
            None,
            app_headers,
            b"client_name=Rustodon+Fixture+App&redirect_uris=urn%3Aietf%3Awg%3Aoauth%3A2.0%3Aoob&scopes=read+write&website=https%3A%2F%2Fapp.example%2F".to_vec(),
        )?;
        let app_responses = send_identically(&targets, &app_request).await?;
        if app_responses.mastodon.status != 200 || app_responses.rust.status != 200 {
            return Err(format!(
                "oauth app registration status differs: Mastodon={}, Rust={}",
                app_responses.mastodon.status, app_responses.rust.status
            )
            .into());
        }
        let mut mastodon_app = serde_json::from_slice::<Value>(&app_responses.mastodon.body)?;
        let mut rust_app = serde_json::from_slice::<Value>(&app_responses.rust.body)?;
        let mastodon_client_id = mastodon_app["client_id"]
            .as_str()
            .ok_or("Mastodon app registration omitted client_id")?
            .to_owned();
        let mastodon_client_secret = mastodon_app["client_secret"]
            .as_str()
            .ok_or("Mastodon app registration omitted client_secret")?
            .to_owned();
        let rust_client_id = rust_app["client_id"]
            .as_str()
            .ok_or("Rust app registration omitted client_id")?
            .to_owned();
        let rust_client_secret = rust_app["client_secret"]
            .as_str()
            .ok_or("Rust app registration omitted client_secret")?
            .to_owned();
        let token_request = |client_id: &str, client_secret: &str| -> Result<_, Box<dyn Error>> {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            );
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            RequestSpec::new(
                Method::POST,
                "/oauth/token",
                None,
                headers,
                format!(
                    "grant_type=client_credentials&client_id={client_id}&client_secret={client_secret}&scope=read+write"
                )
                .into_bytes(),
            )
            .map_err(Into::into)
        };
        let mastodon_token_response = send_single(
            targets.mastodon(),
            &token_request(&mastodon_client_id, &mastodon_client_secret)?,
            "Mastodon",
        )
        .await?;
        let rust_token_response = send_single(
            targets.rust(),
            &token_request(&rust_client_id, &rust_client_secret)?,
            "Rust",
        )
        .await?;
        if mastodon_token_response.status != 200 || rust_token_response.status != 200 {
            return Err(format!(
                "new OAuth app client credentials status differs: Mastodon={}, Rust={}",
                mastodon_token_response.status, rust_token_response.status
            )
            .into());
        }
        let mut mastodon_token = serde_json::from_slice::<Value>(&mastodon_token_response.body)?;
        let mut rust_token = serde_json::from_slice::<Value>(&rust_token_response.body)?;
        let mastodon_access_token = mastodon_token["access_token"]
            .as_str()
            .ok_or("Mastodon token response omitted access_token")?
            .to_owned();
        let rust_access_token = rust_token["access_token"]
            .as_str()
            .ok_or("Rust token response omitted access_token")?
            .to_owned();
        let revoke_request =
            |token: &str, client_id: &str, client_secret: &str| -> Result<_, Box<dyn Error>> {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            );
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            RequestSpec::new(
                Method::POST,
                "/oauth/revoke",
                None,
                headers,
                format!(
                    "token={token}&client_id={client_id}&client_secret={client_secret}"
                )
                .into_bytes(),
            )
            .map_err(Into::into)
            };
        let mastodon_revoke_response = send_single(
            targets.mastodon(),
            &revoke_request(
                &mastodon_access_token,
                &mastodon_client_id,
                &mastodon_client_secret,
            )?,
            "Mastodon",
        )
        .await?;
        let rust_revoke_response = send_single(
            targets.rust(),
            &revoke_request(&rust_access_token, &rust_client_id, &rust_client_secret)?,
            "Rust",
        )
        .await?;
        compare_responses(
            &mastodon_revoke_response,
            &rust_revoke_response,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("oauth token revocation: {error}"))?;
        for key in ["access_token", "created_at"] {
            mastodon_token
                .as_object_mut()
                .ok_or("Mastodon token response was not an object")?
                .insert(key.to_owned(), Value::String("<generated>".to_owned()));
            rust_token
                .as_object_mut()
                .ok_or("Rust token response was not an object")?
                .insert(key.to_owned(), Value::String("<generated>".to_owned()));
        }
        if mastodon_token != rust_token {
            return Err(format!(
                "new OAuth app client credentials response fields differ: Mastodon={mastodon_token}, Rust={rust_token}"
            )
            .into());
        }
        if !oauth_access_token_is_revoked(mastodon_owner.url(), &mastodon_access_token).await?
            || !oauth_access_token_is_revoked(rust_writer.url(), &rust_access_token).await?
        {
            return Err("OAuth token revocation did not persist revoked_at".into());
        }
        for key in ["id", "client_id", "client_secret"] {
            mastodon_app
                .as_object_mut()
                .unwrap()
                .insert(key.to_owned(), Value::String("<generated>".to_owned()));
            rust_app
                .as_object_mut()
                .unwrap()
                .insert(key.to_owned(), Value::String("<generated>".to_owned()));
        }
        if mastodon_app != rust_app {
            return Err(format!(
                "oauth app registration response fields differ: Mastodon={mastodon_app}, Rust={rust_app}"
            )
            .into());
        }
        remove_oauth_applications_after(mastodon_owner.url(), mastodon_application_max_id).await?;
        remove_oauth_applications_after(rust_writer.url(), rust_application_max_id).await?;

        let policy_operations = [
            (
                "notification policy missing token",
                Method::PUT,
                "/api/v1/notifications/policy/",
                None,
                "filter_not_following=true",
            ),
            (
                "notification policy wrong scope",
                Method::PUT,
                "/api/v1/notifications/policy/",
                Some("fixture-bearer-matrix-viewer-v4-6-5"),
                "filter_not_following=true",
            ),
            (
                "notification policy v1 update",
                Method::PUT,
                "/api/v1/notifications/policy/",
                Some("fixture-bearer-token-v4-6-5"),
                "filter_not_following=true&filter_bots=true",
            ),
            (
                "notification policy v2 update",
                Method::PUT,
                "/api/v2/notifications/policy/",
                Some("fixture-bearer-token-v4-6-5"),
                "for_not_following=filter&for_limited_accounts=drop",
            ),
        ];
        for (label, method, path, token, body) in policy_operations {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            );
            if let Some(token) = token {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {token}"))?,
                );
            }
            let request =
                RequestSpec::new(method, path, None, headers, body.as_bytes().to_owned())?;
            let responses = send_identically(&targets, &request).await?;
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    http::header::CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::VARY,
                    http::header::WWW_AUTHENTICATE,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("{label}: {error}"))?;
            if notification_policy_state(mastodon_owner.url()).await?
                != notification_policy_state(rust_writer.url()).await?
            {
                return Err(format!("{label}: policy rows differ").into());
            }
            restore_notification_policies(mastodon_owner.url(), &mastodon_policies).await?;
            restore_notification_policies(rust_writer.url(), &rust_policies).await?;
        }

        let mut connection = PgConnection::connect(rust_writer.url()).await?;
        sqlx::query(
            "DELETE FROM notifications WHERE account_id = $1 AND activity_id = $2 \
             AND activity_type = 'Favourite' AND type = 'favourite'",
        )
        .bind(NOTIFICATION_ACCOUNT_ID)
        .bind(8101_i64)
        .execute(&mut connection)
        .await?;
        let writer = WriteRepository::connect(rust_writer.url()).await?;
        let outcome = writer
            .create_notification(NotificationCreate {
                recipient_account_id: NOTIFICATION_ACCOUNT_ID,
                activity: NotificationActivity::Favourite { id: 8101 },
                silenced: false,
            })
            .await?;
        if !matches!(
            outcome,
            NotificationCreateOutcome::Created {
                filtered: false,
                ..
            }
        ) {
            return Err(
                format!("least-privilege notification creation returned {outcome:?}").into(),
            );
        }

        let mastodon_request_notifications =
            notification_rows(mastodon_owner.url(), "notifications").await?;
        let rust_request_notifications =
            notification_rows(rust_writer.url(), "notifications").await?;
        let request_operations = [
            (
                "notification request missing token",
                "/api/v1/notifications/requests/-96/accept/",
                None,
                "",
            ),
            (
                "notification request wrong scope",
                "/api/v1/notifications/requests/-96/accept/",
                Some("fixture-bearer-matrix-viewer-v4-6-5"),
                "",
            ),
            (
                "notification request missing member",
                "/api/v1/notifications/requests/999999/accept/",
                Some("fixture-bearer-token-v4-6-5"),
                "",
            ),
            (
                "notification request accept",
                "/api/v1/notifications/requests/-96/accept/",
                Some("fixture-bearer-token-v4-6-5"),
                "",
            ),
            (
                "notification request dismiss",
                "/api/v1/notifications/requests/-95/dismiss/",
                Some("fixture-bearer-token-v4-6-5"),
                "",
            ),
            (
                "notification requests bulk accept",
                "/api/v1/notifications/requests/accept/",
                Some("fixture-bearer-token-v4-6-5"),
                "id%5B%5D=-96&id%5B%5D=-95",
            ),
            (
                "notification requests bulk dismiss",
                "/api/v1/notifications/requests/dismiss/",
                Some("fixture-bearer-token-v4-6-5"),
                "id%5B%5D=-96&id%5B%5D=-95",
            ),
            (
                "notification requests scalar accept",
                "/api/v1/notifications/requests/accept/",
                Some("fixture-bearer-token-v4-6-5"),
                "id=-96",
            ),
        ];
        for (label, path, token, body) in request_operations {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            if let Some(token) = token {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {token}"))?,
                );
            }
            if !body.is_empty() {
                headers.insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/x-www-form-urlencoded"),
                );
            }
            let request = RequestSpec::new(
                Method::POST,
                path,
                None,
                headers,
                body.as_bytes().to_owned(),
            )?;
            let responses = send_identically(&targets, &request).await?;
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    http::header::CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::VARY,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("{label}: {error}"))?;
            let mastodon_pairs = notification_permission_pairs(
                &notification_permission_rows(mastodon_owner.url()).await?,
            );
            let rust_pairs = notification_permission_pairs(
                &notification_permission_rows(rust_writer.url()).await?,
            );
            if mastodon_pairs != rust_pairs {
                return Err(format!("{label}: permission rows differ").into());
            }
            let mastodon_after_requests =
                notification_rows(mastodon_owner.url(), "notification_requests").await?;
            let rust_after_requests =
                notification_rows(rust_writer.url(), "notification_requests").await?;
            if mastodon_after_requests != rust_after_requests {
                return Err(format!("{label}: request rows differ").into());
            }
            if notification_rows(mastodon_owner.url(), "notifications").await?
                != mastodon_request_notifications
                || notification_rows(rust_writer.url(), "notifications").await?
                    != rust_request_notifications
            {
                return Err(format!("{label}: notification rows changed").into());
            }
            restore_rows(
                mastodon_owner.url(),
                "notification_requests",
                &mastodon_requests,
            )
            .await?;
            restore_rows(rust_writer.url(), "notification_requests", &rust_requests).await?;
            restore_notification_permissions(mastodon_owner.url(), &mastodon_permissions).await?;
            restore_notification_permissions(rust_writer.url(), &rust_permissions).await?;
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    restore_rows(rust_writer.url(), "notifications", &rust_notifications).await?;
    remove_oauth_applications_after(rust_writer.url(), rust_application_max_id).await?;
    restore_notification_policies(rust_writer.url(), &rust_policies).await?;
    rust_creation?;
    let operation = async {
        let mut marker_headers = HeaderMap::new();
        marker_headers.insert(
            HOST,
            HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
        );
        marker_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        marker_headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        marker_headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        marker_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let marker_request = RequestSpec::new(
            Method::POST,
            "/api/v1/markers/",
            None,
            marker_headers,
            format!("home%5Blast_read_id%5D={}", rust_marker.last_read_id + 1).into_bytes(),
        )?;
        let marker_responses = send_identically(&targets, &marker_request).await?;
        compare_responses(
            &marker_responses.mastodon,
            &marker_responses.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[MARKER_TIMESTAMP],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("v1 marker update: {error}"))?;
        for (label, path, token) in [
            (
                "v1 notification dismiss missing token",
                "/api/v1/notifications/10001/dismiss/",
                None,
            ),
            (
                "v1 notification clear wrong scope",
                "/api/v1/notifications/clear/",
                Some("fixture-bearer-matrix-viewer-v4-6-5"),
            ),
            (
                "v1 notification dismiss wrong owner",
                "/api/v1/notifications/10001/dismiss/",
                Some("fixture-bearer-api-moderator-v4-6-5"),
            ),
            (
                "v1 notification dismiss",
                "/api/v1/notifications/10001/dismiss/",
                Some("fixture-bearer-token-v4-6-5"),
            ),
            (
                "v2 notification dismiss group",
                "/api/v2/notifications/favourite-116844842188805001-495255/dismiss/",
                Some("fixture-bearer-token-v4-6-5"),
            ),
            (
                "v2 notification dismiss ungrouped",
                "/api/v2/notifications/ungrouped-10002/dismiss/",
                Some("fixture-bearer-token-v4-6-5"),
            ),
            (
                "v1 notification clear",
                "/api/v1/notifications/clear/",
                Some("fixture-bearer-token-v4-6-5"),
            ),
            (
                "v2 notification clear",
                "/api/v2/notifications/clear/",
                Some("fixture-bearer-token-v4-6-5"),
            ),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(
                http::header::ACCEPT,
                HeaderValue::from_static("application/json"),
            );
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            if let Some(token) = token {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {token}"))?,
                );
            }
            let request = RequestSpec::new(Method::POST, path, None, headers, Vec::new())?;
            let responses = send_identically(&targets, &request).await?;
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    http::header::CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::VARY,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("{label}: {error}"))?;
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    restore_marker(mastodon_owner.url(), &mastodon_marker).await?;
    restore_marker(rust_writer.url(), &rust_marker).await?;
    restore_rows(
        mastodon_owner.url(),
        "notifications",
        &mastodon_notifications,
    )
    .await?;
    restore_rows(
        mastodon_owner.url(),
        "notification_requests",
        &mastodon_requests,
    )
    .await?;
    restore_rows(rust_writer.url(), "notifications", &rust_notifications).await?;
    restore_rows(rust_writer.url(), "notification_requests", &rust_requests).await?;
    remove_oauth_applications_after(mastodon_owner.url(), mastodon_application_max_id).await?;
    remove_oauth_applications_after(rust_writer.url(), rust_application_max_id).await?;
    restore_notification_policies(mastodon_owner.url(), &mastodon_policies).await?;
    restore_notification_policies(rust_writer.url(), &rust_policies).await?;
    restore_notification_permissions(mastodon_owner.url(), &mastodon_permissions).await?;
    restore_notification_permissions(rust_writer.url(), &rust_permissions).await?;
    operation
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_browser_authentication_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("browser authentication requires the Mastodon owner database");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("browser authentication requires the Rust writer database");
    let snapshot_user = |database: &str| {
        let database = database.to_owned();
        async move {
            let mut connection = PgConnection::connect(&database).await?;
            sqlx::query_as::<
                _,
                (
                    Option<NaiveDateTime>,
                    Option<NaiveDateTime>,
                    i32,
                    NaiveDateTime,
                    Option<Vec<String>>,
                ),
            >(
                "SELECT current_sign_in_at, last_sign_in_at, sign_in_count, updated_at, \
                        otp_backup_codes::text[] FROM users WHERE id = 101",
            )
            .fetch_one(&mut connection)
            .await
        }
    };
    let mastodon_user_state = snapshot_user(mastodon_owner.url()).await?;
    let rust_user_state = snapshot_user(rust_writer.url()).await?;
    let snapshot_browser_lifecycle = |database: &str| {
        let database = database.to_owned();
        async move {
            let mut connection = PgConnection::connect(&database).await?;
            sqlx::query_as::<_, (bool, Option<NaiveDateTime>, bool, Option<i64>)>(
                "SELECT user_record.disabled, account.suspended_at, account.memorial, \
                        account.moved_to_account_id \
                   FROM users user_record \
                   JOIN accounts account ON account.id = user_record.account_id \
                  WHERE user_record.id = 101",
            )
            .fetch_one(&mut connection)
            .await
        }
    };
    let rust_browser_lifecycle = snapshot_browser_lifecycle(rust_writer.url()).await?;
    let mastodon_login_activities =
        browser_user_rows(mastodon_owner.url(), "login_activities").await?;
    let rust_login_activities = browser_user_rows(rust_writer.url(), "login_activities").await?;
    let mastodon_sessions = browser_user_rows(mastodon_owner.url(), "session_activations").await?;
    let rust_sessions = browser_user_rows(rust_writer.url(), "session_activations").await?;
    let set_browser_lifecycle = |database: &str,
                                 disabled: bool,
                                 suspended: bool,
                                 memorial: bool,
                                 moved: bool| {
        let database = database.to_owned();
        async move {
            let mut connection = PgConnection::connect(&database).await?;
            sqlx::query(
                "UPDATE users SET disabled = $1, otp_backup_codes = ARRAY['fixture-recovery-code'] \
                      WHERE id = 101",
            )
            .bind(disabled)
            .execute(&mut connection)
            .await?;
            sqlx::query(
                    "UPDATE accounts SET suspended_at = CASE WHEN $1 THEN clock_timestamp() ELSE NULL END, \
                        memorial = $2, moved_to_account_id = CASE WHEN $3 THEN 116844606259201002 ELSE NULL END \
                      WHERE id = 116844606259201001",
                )
                .bind(suspended)
                .bind(memorial)
                .bind(moved)
                .execute(&mut connection)
                .await?;
            Ok::<(), sqlx::Error>(())
        }
    };
    let restore_browser_lifecycle =
        |database: &str, state: (bool, Option<NaiveDateTime>, bool, Option<i64>)| {
            let database = database.to_owned();
            async move {
                let mut connection = PgConnection::connect(&database).await?;
                sqlx::query("UPDATE users SET disabled = $1 WHERE id = 101")
                    .bind(state.0)
                    .execute(&mut connection)
                    .await?;
                sqlx::query(
                "UPDATE accounts SET suspended_at = $1, memorial = $2, moved_to_account_id = $3 \
                      WHERE id = 116844606259201001",
            )
            .bind(state.1)
            .bind(state.2)
            .bind(state.3)
            .execute(&mut connection)
            .await?;
                Ok::<(), sqlx::Error>(())
            }
        };
    let operation = async {
    let lifecycle_writer = WriteRepository::connect(rust_writer.url()).await?;
    let lifecycle_reader = Repository::connect(rust_writer.url()).await?;
    for (label, disabled, suspended, memorial, moved) in [
        ("disabled browser user", true, false, false, false),
        ("suspended browser account", false, true, false, false),
        ("moved browser account", false, false, false, true),
        ("memorial browser account", false, false, true, false),
    ] {
        set_browser_lifecycle(rust_writer.url(), disabled, suspended, memorial, moved).await?;
        let authentication = lifecycle_writer
            .authenticate_browser_user(
                "alice@fixture.invalid",
                "fixture-password",
                Some("fixture-recovery-code"),
                1_000_000_000,
                IpNetwork::from("192.0.2.10".parse::<std::net::IpAddr>()?),
                "rustodon-browser-lifecycle-test",
            )
            .await;
        if memorial {
            if authentication.is_ok() {
                return Err(format!("{label}: memorial account authenticated").into());
            }
        } else {
            let authentication = authentication
                .map_err(|error| format!("{label}: non-memorial account was rejected: {error}"))?;
            let session_id = lifecycle_writer
                .create_browser_session(
                    authentication.user_id,
                    IpNetwork::from("192.0.2.10".parse::<std::net::IpAddr>()?),
                    "rustodon-browser-lifecycle-test",
                )
                .await?;
            if lifecycle_reader
                .browser_session(&session_id)
                .await?
                .is_none()
            {
                return Err(format!("{label}: browser session was rejected after login").into());
            }
            let mut consent_headers = HeaderMap::new();
            consent_headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            consent_headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
            consent_headers.insert(
                COOKIE,
                HeaderValue::from_str(&format!("_mastodon_session={session_id}"))?,
            );
            consent_headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            let consent = send_single(
                rust_url,
                &RequestSpec::new(
                    Method::GET,
                    "/oauth/authorize",
                    Some(
                        "client_id=rustodon-fixture-client-v4-6-5&redirect_uri=urn%3Aietf%3Awg%3Aoauth%3A2.0%3Aoob&response_type=code&scope=read"
                            .to_owned(),
                    ),
                    consent_headers,
                    Vec::new(),
                )?,
                "Rust OAuth lifecycle guard",
            )
            .await?;
            if consent.status != StatusCode::FOUND.as_u16()
                || consent
                    .headers
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    != Some("/settings/profile")
            {
                return Err(format!(
                    "{label}: non-functional browser session reached OAuth consent"
                )
                .into());
            }
            lifecycle_writer.delete_browser_session(&session_id).await?;
        }
    }
    restore_browser_lifecycle(rust_writer.url(), rust_browser_lifecycle).await?;
    let mut lifecycle_connection = PgConnection::connect(rust_writer.url()).await?;
    sqlx::query("UPDATE users SET otp_backup_codes = $1 WHERE id = 101")
        .bind(rust_user_state.4.clone())
        .execute(&mut lifecycle_connection)
        .await?;
    for (label, body) in [
        (
            "browser invalid password",
            "user%5Bemail%5D=alice%40fixture.invalid&user%5Bpassword%5D=wrong-password",
        ),
        (
            "browser missing two factor",
            "user%5Bemail%5D=alice%40fixture.invalid&user%5Bpassword%5D=fixture-password",
        ),
        (
            "browser invalid backup code",
            "user%5Bemail%5D=alice%40fixture.invalid&user%5Bpassword%5D=fixture-password&user%5Botp_attempt%5D=wrong-code",
        ),
    ] {
        let mut mastodon_form = BrowserFormState::new(None);
        load_browser_form(
            targets.mastodon(),
            "/auth/sign_in",
            &mut mastodon_form,
            "authenticity_token",
            "Mastodon sign-in",
        )
        .await?;
        let mut rust_form = BrowserFormState::new(None);
        load_browser_form(
            targets.rust(),
            "/auth/sign_in",
            &mut rust_form,
            "csrf_token",
            "Rust sign-in",
        )
        .await?;
        let mastodon_request = browser_form_request(
            Method::POST,
            "/auth/sign_in",
            &mastodon_form,
            Some("authenticity_token"),
            body,
        )?;
        let rust_request = browser_form_request(
            Method::POST,
            "/auth/sign_in",
            &rust_form,
            Some("csrf_token"),
            body,
        )?;
        let (mastodon_response, rust_response) = tokio::join!(
            send_single(targets.mastodon(), &mastodon_request, "Mastodon sign-in"),
            send_single(targets.rust(), &rust_request, "Rust sign-in")
        );
        let mastodon_response = mastodon_response?;
        let rust_response = rust_response?;
        if mastodon_response.status != rust_response.status {
            return Err(format!(
                "{label}: status differs: Mastodon={}, Rust={}",
                mastodon_response.status, rust_response.status,
            )
            .into());
        }
        for (side, response) in [("Mastodon", &mastodon_response), ("Rust", &rust_response)] {
            let content_type = response
                .headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if !content_type.starts_with("text/html") {
                return Err(format!(
                    "{label}: {side} returned a non-HTML error: content_type={content_type:?} body={:?}",
                    String::from_utf8_lossy(&response.body),
                )
                .into());
            }
        }
        if !rust_response
            .body
            .windows(b"<form".len())
            .any(|window| window == b"<form")
        {
            return Err(format!(
                "{label}: Rust did not render the sign-in form after failure: body={:?}",
                String::from_utf8_lossy(&rust_response.body),
            )
            .into());
        }
        if !(100..600).contains(&mastodon_response.status) {
            return Err(format!("{label}: invalid HTTP status").into());
        }
    }
    let backup_code_count = |database: &str| {
        let database = database.to_owned();
        async move {
            let mut connection = PgConnection::connect(&database).await?;
            sqlx::query_scalar::<_, i32>(
                "SELECT COALESCE(cardinality(otp_backup_codes), 0) FROM users WHERE id = 101",
            )
            .fetch_one(&mut connection)
            .await
        }
    };
    let mastodon_count = backup_code_count(
        config
            .mastodon_owner_database
            .as_ref()
            .expect("browser authentication requires the Mastodon owner database")
            .url(),
    )
    .await?;
    let rust_count = backup_code_count(
        config
            .rust_write_database
            .as_ref()
            .expect("browser authentication requires the Rust writer database")
            .url(),
    )
    .await?;
    if mastodon_count != 1 || rust_count != 1 || mastodon_count != rust_count {
        return Err(format!(
            "browser invalid backup code changed persisted codes: Mastodon={mastodon_count}, Rust={rust_count}"
        )
        .into());
    }
    let writer = WriteRepository::connect(rust_writer.url()).await?;
    let authentication = writer
        .authenticate_browser_user(
            "alice@fixture.invalid",
            "fixture-password",
            Some("fixture-recovery-code"),
            1_000_000_000,
            IpNetwork::from("192.0.2.10".parse::<std::net::IpAddr>()?),
            "rustodon-browser-auth-test",
        )
        .await?;
    if authentication.method != BrowserAuthenticationMethod::BackupCode {
        return Err("Rust browser authentication did not accept the backup code".into());
    }
    let rust_count = backup_code_count(rust_writer.url()).await?;
    if rust_count != 0 {
        return Err(format!("Rust backup code was not consumed: {rust_count}").into());
    }
    let session_id = writer
        .create_browser_session(
            authentication.user_id,
            IpNetwork::from("192.0.2.10".parse::<std::net::IpAddr>()?),
            "rustodon-browser-auth-test",
        )
        .await?;
    let reader = Repository::connect(rust_writer.url()).await?;
    if reader.browser_session(&session_id).await?.is_none() {
        return Err("Rust browser session was not persisted".into());
    }
    let mut session_connection = PgConnection::connect(rust_writer.url()).await?;
    let session_token_id = sqlx::query_scalar::<_, i64>(
        "SELECT access_token_id FROM session_activations WHERE session_id = $1",
    )
    .bind(&session_id)
    .fetch_one(&mut session_connection)
    .await?;
    sqlx::query("UPDATE oauth_access_tokens SET revoked_at = clock_timestamp() WHERE id = $1")
        .bind(session_token_id)
        .execute(&mut session_connection)
        .await?;
    if reader.browser_session(&session_id).await?.is_some() {
        return Err("revoked browser session token remained usable".into());
    }
    sqlx::query("UPDATE oauth_access_tokens SET revoked_at = NULL WHERE id = $1")
        .bind(session_token_id)
        .execute(&mut session_connection)
        .await?;
    sqlx::query("UPDATE oauth_access_tokens SET expires_in = 0 WHERE id = $1")
        .bind(session_token_id)
        .execute(&mut session_connection)
        .await?;
    if reader.browser_session(&session_id).await?.is_some() {
        return Err("expired browser session token remained usable".into());
    }
    sqlx::query("UPDATE oauth_access_tokens SET expires_in = NULL WHERE id = $1")
        .bind(session_token_id)
        .execute(&mut session_connection)
        .await?;
    let mut shell_headers = HeaderMap::new();
    shell_headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    shell_headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
    let mut authenticated_form = BrowserFormState::new(Some(&session_id));
    let profile_form = load_browser_form(
        rust_url,
        "/settings/profile",
        &mut authenticated_form,
        "csrf_token",
        "Rust authenticated profile",
    )
    .await?;
    shell_headers.insert(
        COOKIE,
        HeaderValue::from_str(&authenticated_form.cookie_header())?,
    );
    shell_headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let shell_request = RequestSpec::new(Method::GET, "/", None, shell_headers, Vec::new())?;
    let shell = send_single(rust_url, &shell_request, "authenticated web shell").await?;
    if shell.status != 200 {
        return Err(format!("authenticated web shell returned HTTP {}", shell.status).into());
    }
    let shell_html = String::from_utf8(shell.body)?;
    let initial_state = shell_html
        .find("<script id=\"initial-state\"")
        .and_then(|start| {
            let content_start = start + shell_html[start..].find('>')? + 1;
            let content_end = content_start + shell_html[content_start..].find("</script>")?;
            Some(&shell_html[content_start..content_end])
        })
        .ok_or("authenticated web shell did not contain initial state")?;
    let initial_state: Value = serde_json::from_str(initial_state)?;
    if initial_state["meta"]["me"] != "116844606259201001"
        || initial_state["compose"]["me"] != "116844606259201001"
        || initial_state["accounts"]["116844606259201001"].is_null()
        || initial_state["meta"]["access_token"]
            .as_str()
            .is_none_or(str::is_empty)
    {
        return Err("authenticated web shell did not hydrate the browser session".into());
    }
    let mut rust_connection = PgConnection::connect(rust_writer.url()).await?;
    let (session_access_token_id, token_id, resource_owner_id, scopes) =
        sqlx::query_as::<_, (Option<i64>, Option<i64>, Option<i64>, Option<String>)>(
            "SELECT session.access_token_id, access_token.id, access_token.resource_owner_id,
                access_token.scopes
           FROM session_activations session
           LEFT JOIN oauth_access_tokens access_token ON access_token.id = session.access_token_id
          WHERE session.session_id = $1",
        )
        .bind(&session_id)
        .fetch_one(&mut rust_connection)
        .await?;
    if session_access_token_id != token_id
        || resource_owner_id != Some(authentication.user_id)
        || scopes.as_deref() != Some("read write follow")
    {
        return Err("Rust browser session did not own its default OAuth token".into());
    }
    let mut status_headers = HeaderMap::new();
    status_headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    status_headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/activity+json"),
    );
    status_headers.insert(
        COOKIE,
        HeaderValue::from_str(&format!("_mastodon_session={session_id}"))?,
    );
    status_headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let browser_status = send_single(
        rust_url,
        &RequestSpec::new(
            Method::GET,
            "/users/alice/statuses/116844850053125003",
            None,
            status_headers,
            Vec::new(),
        )?,
        "Rust browser ActivityPub status",
    )
    .await?;
    if browser_status.status != StatusCode::OK.as_u16()
        || !browser_status
            .body
            .windows(b"116844850053125003".len())
            .any(|window| window == b"116844850053125003")
    {
        return Err("browser session did not authorize the ActivityPub status read".into());
    }
    let mut collection_headers = HeaderMap::new();
    collection_headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    collection_headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/activity+json"),
    );
    collection_headers.insert(
        COOKIE,
        HeaderValue::from_str(&format!("_mastodon_session={session_id}"))?,
    );
    collection_headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let browser_collection = send_single(
        rust_url,
        &RequestSpec::new(
            Method::GET,
            "/users/alice/statuses/116844850053125003/replies",
            None,
            collection_headers,
            Vec::new(),
        )?,
        "Rust browser ActivityPub collection",
    )
    .await?;
    if browser_collection.status != StatusCode::NOT_FOUND.as_u16() {
        return Err(format!(
            "browser session incorrectly authorized the ActivityPub collection: HTTP {}",
            browser_collection.status
        )
        .into());
    }
    if !writer.touch_browser_session(&session_id).await? {
        return Err("Rust browser session was not touched".into());
    }
    let mut logout_headers = HeaderMap::new();
    logout_headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    logout_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    authenticated_form.update_from_response(&profile_form, "csrf_token")?;
    logout_headers.insert(
        COOKIE,
        HeaderValue::from_str(&authenticated_form.cookie_header())?,
    );
    logout_headers.insert(
        "x-csrf-token",
        HeaderValue::from_str(authenticated_form.csrf_token()?)?,
    );
    logout_headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let logout = send_single(
        rust_url,
        &RequestSpec::new(
            Method::DELETE,
            "/auth/sign_out",
            None,
            logout_headers,
            Vec::new(),
        )?,
        "Rust browser logout",
    )
    .await?;
    if logout.status != StatusCode::OK.as_u16()
        || !logout
            .headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"))
        || !logout
            .body
            .windows(br#""redirect_to":"/auth/sign_in""#.len())
            .any(|window| window == br#""redirect_to":"/auth/sign_in""#)
    {
        return Err(format!(
            "Rust browser logout returned an unexpected response: status={}, content_type={:?}, body={:?}",
            logout.status,
            logout.headers.get(CONTENT_TYPE),
            String::from_utf8_lossy(&logout.body),
        )
        .into());
    }
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM oauth_access_tokens WHERE id = $1)",
    )
    .bind(token_id)
    .fetch_one(&mut rust_connection)
    .await?
    {
        return Err("Rust browser logout left the session OAuth token behind".into());
    }
    if reader.browser_session(&session_id).await?.is_some() {
        return Err("Rust browser session remained after deletion".into());
    }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let cleanup = async {
        let current_rust_session_tokens =
            browser_session_access_token_ids(rust_writer.url()).await?;
        let baseline_rust_session_tokens = rust_sessions
            .iter()
            .filter_map(|row| row.get("access_token_id").and_then(Value::as_i64))
            .collect::<Vec<_>>();
        restore_browser_lifecycle(rust_writer.url(), rust_browser_lifecycle).await?;
        for (database, user_state) in [
            (mastodon_owner.url(), mastodon_user_state),
            (rust_writer.url(), rust_user_state),
        ] {
            let mut connection = PgConnection::connect(database).await?;
            sqlx::query(
                "UPDATE users SET current_sign_in_at = $1, last_sign_in_at = $2, \
                    sign_in_count = $3, updated_at = $4, otp_backup_codes = $5 WHERE id = 101",
            )
            .bind(user_state.0)
            .bind(user_state.1)
            .bind(user_state.2)
            .bind(user_state.3)
            .bind(user_state.4)
            .execute(&mut connection)
            .await?;
        }
        restore_browser_user_rows(
            mastodon_owner.url(),
            "login_activities",
            &mastodon_login_activities,
        )
        .await?;
        restore_browser_user_rows(
            rust_writer.url(),
            "login_activities",
            &rust_login_activities,
        )
        .await?;
        restore_browser_user_rows(
            mastodon_owner.url(),
            "session_activations",
            &mastodon_sessions,
        )
        .await?;
        restore_browser_user_rows(rust_writer.url(), "session_activations", &rust_sessions).await?;
        remove_oauth_access_tokens(
            rust_writer.url(),
            &current_rust_session_tokens
                .into_iter()
                .filter(|id| !baseline_rust_session_tokens.contains(id))
                .collect::<Vec<_>>(),
        )
        .await?;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    operation_and_cleanup(operation, cleanup)
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_account_settings_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("account settings require a Rust writer URL");
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .expect("account settings require a Rust owner URL for cleanup");
    let before = account_profile_state(rust_writer.url()).await?;
    let tags_before = account_tag_rows(rust_writer.url()).await?;
    let account_stats_before =
        interaction_account_stat(rust_owner.url(), INTERACTION_ACCOUNT_ID).await?;
    let media_before = MediaSnapshot::capture(&config.rust_media)?;
    let browser_media_state_before =
        account_media_state(rust_writer.url(), BROWSER_MEDIA_ACCOUNT_ID).await?;
    let writer = WriteRepository::connect(rust_writer.url()).await?;
    let session_id = writer
        .create_browser_session(
            MARKER_USER_ID,
            IpNetwork::from("192.0.2.20".parse::<std::net::IpAddr>()?),
            "rustodon-account-settings-test",
        )
        .await?;
    let mut browser = BrowserFormState::new(Some(&session_id));
    let browser_media_session_id = writer
        .create_browser_session(
            BROWSER_MEDIA_USER_ID,
            IpNetwork::from("192.0.2.21".parse::<std::net::IpAddr>()?),
            "rustodon-account-media-test",
        )
        .await?;
    let mut browser_media = BrowserFormState::new(Some(&browser_media_session_id));
    let profile_image = std::fs::read(MEDIA_FIXTURE)?;
    let mut created_status_ids = Vec::new();
    let request = |method: Method,
                   path: &str,
                   cookie: Option<&str>,
                   body: &[u8]|
     -> Result<RequestSpec, Box<dyn Error>> {
        let mut headers = HeaderMap::new();
        headers.insert(
            HOST,
            HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
        );
        headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        if let Some(cookie) = cookie {
            headers.insert(COOKIE, HeaderValue::from_str(cookie)?);
        }
        if !body.is_empty() {
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            );
        }
        Ok(RequestSpec::new(
            method,
            path,
            None,
            headers,
            body.to_vec(),
        )?)
    };
    let operation = async {
        let unauthenticated = send_single(
            rust_url,
            &request(Method::GET, "/settings/profile", None, &[])? ,
            "Rust",
        )
        .await?;
        if unauthenticated.status != StatusCode::FOUND.as_u16()
            || unauthenticated
                .headers
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                != Some("/auth/sign_in")
        {
            return Err("unauthenticated account settings did not redirect to sign in".into());
        }

        for (path, expected_status, marker) in [
            ("/settings/profile", 200, "fields_attributes[0][name]"),
            ("/settings/preferences/appearance", 200, "Appearance"),
            (
                "/settings/preferences/posting_defaults",
                200,
                "source[privacy]",
            ),
            ("/settings/security", 200, "Change password"),
            ("/settings/delete", 200, "Delete account"),
            (
                "/settings/two_factor_authentication_methods",
                200,
                "Two-factor authentication",
            ),
        ] {
            let response = send_single(
                rust_url,
                &browser_form_request(Method::GET, path, &browser, None, "")?,
                "Rust",
            )
            .await?;
            if response.status != expected_status
                || !String::from_utf8_lossy(&response.body).contains(marker)
            {
                return Err(format!("account settings page failed: {path}").into());
            }
            browser.update_from_response(&response, "csrf_token")?;
            if path == "/settings/profile"
                && !String::from_utf8_lossy(&response.body)
                    .contains("method=\"post\" action=\"/auth/sign_out\"")
            {
                return Err("account settings did not render the logout form".into());
            }
        }
        let preferences_redirect = send_single(
            rust_url,
            &request(
                Method::GET,
                "/settings/preferences",
                Some(&browser.cookie_header()),
                &[],
            )? ,
            "Rust",
        )
        .await?;
        if preferences_redirect.status != StatusCode::FOUND.as_u16()
            || preferences_redirect
                .headers
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                != Some("/settings/preferences/appearance")
        {
            return Err("account preferences did not redirect to appearance".into());
        }

        load_browser_form(
            rust_url,
            "/settings/profile",
            &mut browser,
            "csrf_token",
            "Rust profile settings",
        )
        .await?;
        let profile = browser_form_request(
            Method::POST,
            "/settings/profile",
            &browser,
            Some("csrf_token"),
            "display_name=Browser+settings+profile&note=Browser+settings+note&bot=0&locked=0&discoverable=1&fields_attributes%5B0%5D%5Bname%5D=Website&fields_attributes%5B0%5D%5Bvalue%5D=https%3A%2F%2Fexample.com",
        )?;
        let profile_response = send_single(rust_url, &profile, "Rust").await?;
        if profile_response.status != StatusCode::FOUND.as_u16()
            || profile_response
                .headers
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                != Some("/settings/profile")
        {
            return Err("browser profile settings did not redirect after saving".into());
        }
        let after_profile = account_profile_state(rust_writer.url()).await?;
        if after_profile.display_name != "Browser settings profile"
            || after_profile.note != "Browser settings note"
            || after_profile.discoverable != Some(true)
        {
            return Err("browser profile settings did not persist account fields".into());
        }

        load_browser_form(
            rust_url,
            "/settings/profile",
            &mut browser_media,
            "csrf_token",
            "Rust media profile settings",
        )
        .await?;
        let upload = browser_profile_upload_request(
            &browser_media.cookie_header(),
            &browser_profile_multipart_body(browser_media.csrf_token()?, &profile_image),
        )?;
        let upload_response = send_single(rust_url, &upload, "Rust").await?;
        if upload_response.status != StatusCode::FOUND.as_u16()
            || upload_response
                .headers
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                != Some("/settings/profile")
        {
            return Err(format!(
                "browser profile media upload did not redirect after saving: status={}, location={:?}, body={}",
                upload_response.status,
                upload_response.headers.get(LOCATION),
                String::from_utf8_lossy(&upload_response.body),
            )
            .into());
        }
        let uploaded_media_state =
            account_media_state(rust_writer.url(), BROWSER_MEDIA_ACCOUNT_ID).await?;
        let media_after_upload = MediaSnapshot::capture(&config.rust_media)?;
        assert_browser_profile_media_upload(
            &media_before,
            &media_after_upload,
            &uploaded_media_state,
        )?;

        load_browser_form(
            rust_url,
            "/settings/preferences/posting_defaults",
            &mut browser,
            "csrf_token",
            "Rust posting defaults",
        )
        .await?;
        let posting_defaults = browser_form_request(
            Method::POST,
            "/settings/preferences/posting_defaults",
            &browser,
            Some("csrf_token"),
            "source%5Bprivacy%5D=unlisted&source%5Bsensitive%5D=1&source%5Blanguage%5D=fr&source%5Bquote_policy%5D=followers",
        )?;
        let posting_response = send_single(rust_url, &posting_defaults, "Rust").await?;
        if posting_response.status != StatusCode::FOUND.as_u16()
            || posting_response
                .headers
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                != Some("/settings/preferences/posting_defaults")
        {
            return Err("browser posting defaults did not redirect after saving".into());
        }
        let after_posting = account_profile_state(rust_writer.url()).await?;
        let settings: Value = serde_json::from_str(
            after_posting
                .user_settings
                .as_deref()
                .ok_or("browser posting defaults did not persist user settings")?,
        )?;
        if settings["default_privacy"] != "unlisted"
            || settings["default_sensitive"] != true
            || settings["default_language"] != "fr"
            || settings["default_quote_policy"] != "followers"
        {
            return Err("browser posting defaults did not persist all values".into());
        }

        load_browser_form(
            rust_url,
            "/settings/preferences/posting_defaults",
            &mut browser,
            "csrf_token",
            "Rust private posting defaults",
        )
        .await?;
        let private_defaults = browser_form_request(
            Method::POST,
            "/settings/preferences/posting_defaults",
            &browser,
            Some("csrf_token"),
            "source%5Bprivacy%5D=private&source%5Bquote_policy%5D=public",
        )?;
        let private_response = send_single(rust_url, &private_defaults, "Rust").await?;
        if private_response.status != StatusCode::FOUND.as_u16() {
            return Err("private posting defaults did not redirect after saving".into());
        }
        let private_state = account_profile_state(rust_writer.url()).await?;
        let private_settings: Value = serde_json::from_str(
            private_state
                .user_settings
                .as_deref()
                .ok_or("private posting defaults did not persist user settings")?,
        )?;
        if private_settings["default_privacy"] != "private"
            || private_settings["default_quote_policy"] != "nobody"
        {
            return Err("private posting defaults did not disable automatic quotes".into());
        }

        set_status_default_state(rust_writer.url(), true, "{}").await?;
        let locked_status = send_single(
            rust_url,
            &status_request(Method::POST, "/api/v1/statuses", "status=locked+default+status")?,
            "Rust",
        )
        .await?;
        let locked_status_id = created_status_id(&locked_status.body)?;
        created_status_ids.push(locked_status_id);
        if status_quote_state(rust_writer.url(), locked_status_id).await? != (2, 0) {
            return Err("locked account status defaults were not private and non-quotable".into());
        }

        set_status_default_state(
            rust_writer.url(),
            false,
            r#"{"default_privacy":"public","default_quote_policy":"followers"}"#,
        )
        .await?;
        let follower_quote_status = send_single(
            rust_url,
            &status_request(
                Method::POST,
                "/api/v1/statuses",
                "status=follower+quote+default",
            )?,
            "Rust",
        )
        .await?;
        let follower_quote_status_id = created_status_id(&follower_quote_status.body)?;
        created_status_ids.push(follower_quote_status_id);
        if status_quote_state(rust_writer.url(), follower_quote_status_id).await? != (0, 262_144) {
            return Err("follower quote default was not stored as an automatic policy".into());
        }

        let nobody_quote_status = send_single(
            rust_url,
            &status_request(
                Method::POST,
                "/api/v1/statuses",
                "status=nobody+quote&quote_approval_policy=nobody",
            )?,
            "Rust",
        )
        .await?;
        let nobody_quote_status_id = created_status_id(&nobody_quote_status.body)?;
        created_status_ids.push(nobody_quote_status_id);
        if status_quote_state(rust_writer.url(), nobody_quote_status_id).await? != (0, 0) {
            return Err("explicit nobody quote policy was not stored".into());
        }

        let missing_csrf = request(
            Method::POST,
            "/settings/profile",
            Some(&browser.cookie_header()),
            b"display_name=Rejected+profile+write",
        )?;
        let missing_csrf_response = send_single(rust_url, &missing_csrf, "Rust").await?;
        if missing_csrf_response.status != StatusCode::UNPROCESSABLE_ENTITY.as_u16()
            || !String::from_utf8_lossy(&missing_csrf_response.body).contains("could not be verified")
        {
            return Err("account settings accepted a missing CSRF token".into());
        }

        load_browser_form(
            rust_url,
            "/settings/security",
            &mut browser,
            "csrf_token",
            "Rust security settings",
        )
        .await?;
        let invalid_password = browser_form_request(
            Method::POST,
            "/settings/security",
            &browser,
            Some("csrf_token"),
            "current_password=wrong-password&password=fixture-new-password&password_confirmation=fixture-new-password",
        )?;
        let invalid_password_response = send_single(rust_url, &invalid_password, "Rust").await?;
        if invalid_password_response.status != StatusCode::UNPROCESSABLE_ENTITY.as_u16()
            || !String::from_utf8_lossy(&invalid_password_response.body)
                .contains("current password is incorrect")
        {
            return Err("account security page did not reject an invalid password".into());
        }
        browser.update_from_response(&invalid_password_response, "csrf_token")?;
        load_browser_form(
            rust_url,
            "/settings/delete",
            &mut browser,
            "csrf_token",
            "Rust delete settings",
        )
        .await?;
        let invalid_deletion = browser_form_request(
            Method::POST,
            "/settings/delete",
            &browser,
            Some("csrf_token"),
            "password=wrong-password",
        )?;
        let invalid_deletion_response = send_single(rust_url, &invalid_deletion, "Rust").await?;
        if invalid_deletion_response.status != StatusCode::UNPROCESSABLE_ENTITY.as_u16()
            || !String::from_utf8_lossy(&invalid_deletion_response.body)
                .contains("password or username confirmation is incorrect")
        {
            return Err("account deletion did not reject an invalid challenge".into());
        }

        browser.update_from_response(&invalid_deletion_response, "csrf_token")?;
        let logout = browser_form_request(
            Method::POST,
            "/auth/sign_out",
            &browser,
            Some("csrf_token"),
            "",
        )?;
        let logout_response = send_single(rust_url, &logout, "Rust browser HTML logout").await?;
        if logout_response.status != StatusCode::FOUND.as_u16()
            || logout_response
                .headers
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                != Some("/auth/sign_in")
        {
            return Err(format!(
                "browser HTML logout did not redirect to sign in: status={}, location={:?}",
                logout_response.status,
                logout_response.headers.get(LOCATION),
            )
            .into());
        }
        let after_logout = send_single(
            rust_url,
            &browser_form_request(Method::GET, "/settings/profile", &browser, None, "")?,
            "Rust browser session after logout",
        )
        .await?;
        if after_logout.status != StatusCode::FOUND.as_u16()
            || after_logout
                .headers
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                != Some("/auth/sign_in")
        {
            return Err("browser HTML logout left the settings session usable".into());
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    let status_cleanup_result =
        restore_created_statuses(rust_writer.url(), &created_status_ids).await;
    let delete_result = writer.delete_browser_session(&session_id).await;
    let browser_media_session_delete_result = writer
        .delete_browser_session(&browser_media_session_id)
        .await;
    let browser_media_after = MediaSnapshot::capture(&config.rust_media)?;
    let browser_media_state_restore_result = restore_account_media_state(
        rust_writer.url(),
        BROWSER_MEDIA_ACCOUNT_ID,
        &browser_media_state_before,
    )
    .await;
    let browser_media_file_restore_result =
        remove_added_media_files(&config.rust_media, &media_before, &browser_media_after);
    let restore_result = restore_account_profile_state(rust_writer.url(), &before).await;
    let tags_restore_result = restore_account_tag_rows(rust_writer.url(), &tags_before).await;
    let account_stats_restore_result = restore_interaction_account_stat(
        rust_owner.url(),
        INTERACTION_ACCOUNT_ID,
        account_stats_before.as_ref(),
    )
    .await;
    operation?;
    status_cleanup_result?;
    delete_result?;
    browser_media_session_delete_result?;
    browser_media_state_restore_result?;
    browser_media_file_restore_result?;
    restore_result?;
    tags_restore_result?;
    account_stats_restore_result?;
    let media_after = MediaSnapshot::capture(&config.rust_media)?;
    compare_media_with_labels(
        &media_before,
        &media_after,
        "Rust before",
        "Rust after",
        DEFAULT_MISMATCH_LIMIT,
    )?;
    let browser_media_state_after =
        account_media_state(rust_writer.url(), BROWSER_MEDIA_ACCOUNT_ID).await?;
    if browser_media_state_after != browser_media_state_before {
        return Err(
            "browser profile media database state did not roll back to its baseline".into(),
        );
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_browser_two_factor_management_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    const USER_ID: i64 = 101;
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("browser 2FA management requires a Rust writer URL");
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .expect("browser 2FA management requires a Rust owner URL");
    let snapshot = {
        let mut connection = PgConnection::connect(rust_writer.url()).await?;
        sqlx::query_as::<
            _,
            (
                bool,
                Option<String>,
                Option<Vec<String>>,
                Option<i32>,
                NaiveDateTime,
                Option<i64>,
            ),
        >(
            "SELECT otp_required_for_login, otp_secret, otp_backup_codes::text[], \
                    consumed_timestep, updated_at, role_id FROM users WHERE id = $1",
        )
        .bind(USER_ID)
        .fetch_one(&mut connection)
        .await?
    };
    let webauthn_before = {
        let mut connection = PgConnection::connect(rust_owner.url()).await?;
        sqlx::query_as::<
            _,
            (
                i64,
                NaiveDateTime,
                String,
                String,
                String,
                i64,
                NaiveDateTime,
                Option<i64>,
            ),
        >(
            "SELECT id, created_at, external_id, nickname, public_key, sign_count, \
                    updated_at, user_id FROM webauthn_credentials WHERE user_id = $1",
        )
        .bind(USER_ID)
        .fetch_optional(&mut connection)
        .await?
    };
    let encryption = fixture_active_record_encryption();
    let writer = rustodon::mastodon::WriteRepository::connect(rust_writer.url())
        .await?
        .with_active_record_encryption(encryption.clone());
    let session_id = writer
        .create_browser_session(
            USER_ID,
            IpNetwork::from("192.0.2.30".parse::<std::net::IpAddr>()?),
            "rustodon-browser-2fa-test",
        )
        .await?;
    let mut browser = BrowserFormState::new(Some(&session_id));
    let operation = async {
        let methods = load_browser_form(
            rust_url,
            "/settings/two_factor_authentication_methods",
            &mut browser,
            "csrf_token",
            "Rust 2FA methods",
        )
        .await?;
        let methods_html = String::from_utf8_lossy(&methods.body);
        if methods.status != StatusCode::OK.as_u16()
            || !methods_html.contains("Regenerate recovery codes")
            || !methods_html.contains("cannot be disabled here")
        {
            return Err("enabled 2FA methods page did not render management controls".into());
        }

        let invalid_disable = send_single(
            rust_url,
            &browser_form_request(
                Method::POST,
                "/settings/two_factor_authentication_methods/disable",
                &browser,
                Some("csrf_token"),
                "current_password=wrong-password",
            )?,
            "Rust invalid 2FA disable",
        )
        .await?;
        if invalid_disable.status != StatusCode::UNPROCESSABLE_ENTITY.as_u16()
            || !String::from_utf8_lossy(&invalid_disable.body)
                .contains("current password is incorrect")
        {
            return Err("2FA disable accepted an invalid password".into());
        }
        browser.update_from_response(&invalid_disable, "csrf_token")?;

        let protected_disable = send_single(
            rust_url,
            &browser_form_request(
                Method::POST,
                "/settings/two_factor_authentication_methods/disable",
                &browser,
                Some("csrf_token"),
                "current_password=fixture-password",
            )?,
            "Rust required-role 2FA disable",
        )
        .await?;
        if protected_disable.status != StatusCode::UNPROCESSABLE_ENTITY.as_u16()
            || !String::from_utf8_lossy(&protected_disable.body)
                .contains("Two-factor authentication could not be disabled")
        {
            return Err("required-role 2FA disable bypassed server-side protection".into());
        }
        browser.update_from_response(&protected_disable, "csrf_token")?;
        if !two_factor_state(rust_writer.url(), USER_ID).await?.0 {
            return Err("required-role 2FA disable changed persisted authentication state".into());
        }
        let mut owner_connection = PgConnection::connect(rust_owner.url()).await?;
        sqlx::query("UPDATE users SET role_id = NULL WHERE id = $1")
            .bind(USER_ID)
            .execute(&mut owner_connection)
            .await?;

        load_browser_form(
            rust_url,
            "/settings/two_factor_authentication_methods",
            &mut browser,
            "csrf_token",
            "Rust 2FA methods after role change",
        )
        .await?;
        let disabled = send_single(
            rust_url,
            &browser_form_request(
                Method::POST,
                "/settings/two_factor_authentication_methods/disable",
                &browser,
                Some("csrf_token"),
                "current_password=fixture-password",
            )?,
            "Rust disable 2FA",
        )
        .await?;
        if disabled.status != StatusCode::FOUND.as_u16()
            || disabled
                .headers
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                != Some("/settings/otp_authentication")
        {
            return Err("2FA disable did not redirect to setup".into());
        }
        let disabled_state = two_factor_state(rust_writer.url(), USER_ID).await?;
        if disabled_state.0 || disabled_state.1.is_some() || disabled_state.2 != Some(Vec::new()) {
            return Err("2FA disable did not clear persisted authentication state".into());
        }
        let mut connection = PgConnection::connect(rust_writer.url()).await?;
        let webauthn_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM webauthn_credentials WHERE user_id = $1",
        )
        .bind(USER_ID)
        .fetch_one(&mut connection)
        .await?;
        if webauthn_count != 0 {
            return Err("2FA disable did not clear security-key credentials".into());
        }

        let methods_after_disable = send_single(
            rust_url,
            &browser_form_request(
                Method::GET,
                "/settings/two_factor_authentication_methods",
                &browser,
                None,
                "",
            )?,
            "Rust disabled 2FA methods",
        )
        .await?;
        if methods_after_disable.status != StatusCode::FOUND.as_u16()
            || methods_after_disable
                .headers
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                != Some("/settings/otp_authentication")
        {
            return Err("disabled 2FA methods page did not redirect to setup".into());
        }

        let setup = load_browser_form(
            rust_url,
            "/settings/otp_authentication",
            &mut browser,
            "csrf_token",
            "Rust 2FA setup",
        )
        .await?;
        if setup.status != StatusCode::OK.as_u16()
            || !String::from_utf8_lossy(&setup.body).contains("Set up two-factor authentication")
        {
            return Err("2FA setup page did not render".into());
        }
        let setup_start = send_single(
            rust_url,
            &browser_form_request(
                Method::POST,
                "/settings/otp_authentication",
                &browser,
                Some("csrf_token"),
                "current_password=fixture-password",
            )?,
            "Rust 2FA setup start",
        )
        .await?;
        if setup_start.status != StatusCode::OK.as_u16() {
            return Err("2FA setup start did not render confirmation".into());
        }
        browser.update_from_response(&setup_start, "csrf_token")?;
        let secret = hidden_form_value(&setup_start.body, "otp_secret")
            .ok_or("2FA confirmation did not contain a secret")?;
        if secret.len() != 32
            || !secret
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte))
        {
            return Err("2FA setup generated an invalid Base32 secret".into());
        }
        let attempt = fixture_totp_code(&secret, Utc::now().timestamp())?;
        let confirmed = send_single(
            rust_url,
            &browser_form_request(
                Method::POST,
                "/settings/two_factor_authentication/confirmation",
                &browser,
                Some("csrf_token"),
                &format!(
                    "current_password=fixture-password&otp_secret={secret}&otp_attempt={attempt}"
                ),
            )?,
            "Rust 2FA confirmation",
        )
        .await?;
        if confirmed.status != StatusCode::OK.as_u16()
            || !String::from_utf8_lossy(&confirmed.body).contains("Recovery codes")
        {
            return Err("2FA confirmation did not render recovery codes".into());
        }
        browser.update_from_response(&confirmed, "csrf_token")?;
        let enabled_state = two_factor_state(rust_writer.url(), USER_ID).await?;
        let stored_secret = enabled_state
            .1
            .as_deref()
            .ok_or("2FA confirmation did not persist an encrypted secret")?;
        let decrypted_secret = encryption
            .decrypt_string(stored_secret, 256)
            .map_err(|_| "2FA confirmation stored an undecryptable secret")?;
        if !enabled_state.0
            || stored_secret == secret
            || decrypted_secret.expose_secret() != secret
            || enabled_state
                .2
                .as_ref()
                .is_none_or(|codes| codes.len() != 10)
            || enabled_state
                .2
                .as_ref()
                .is_some_and(|codes| codes.iter().any(|code| !code.starts_with("$2")))
        {
            return Err("2FA confirmation did not persist a hashed backup-code set".into());
        }

        load_browser_form(
            rust_url,
            "/settings/two_factor_authentication_methods",
            &mut browser,
            "csrf_token",
            "Rust recovery-code form",
        )
        .await?;
        let regenerated = send_single(
            rust_url,
            &browser_form_request(
                Method::POST,
                "/settings/two_factor_authentication/recovery_codes",
                &browser,
                Some("csrf_token"),
                "current_password=fixture-password",
            )?,
            "Rust backup-code regeneration",
        )
        .await?;
        if regenerated.status != StatusCode::OK.as_u16()
            || !String::from_utf8_lossy(&regenerated.body).contains("Recovery codes")
        {
            return Err("backup-code regeneration did not render recovery codes".into());
        }
        browser.update_from_response(&regenerated, "csrf_token")?;

        load_browser_form(
            rust_url,
            "/settings/two_factor_authentication_methods",
            &mut browser,
            "csrf_token",
            "Rust final 2FA disable form",
        )
        .await?;
        let disabled_again = send_single(
            rust_url,
            &browser_form_request(
                Method::POST,
                "/settings/two_factor_authentication_methods/disable",
                &browser,
                Some("csrf_token"),
                "current_password=fixture-password",
            )?,
            "Rust final 2FA disable",
        )
        .await?;
        if disabled_again.status != StatusCode::FOUND.as_u16() {
            return Err("final 2FA disable failed".into());
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    let cleanup = async {
    let mut connection = PgConnection::connect(rust_writer.url()).await?;
    sqlx::query(
        "UPDATE users SET otp_required_for_login = $1, otp_secret = $2, \
                otp_backup_codes = $3, consumed_timestep = $4, updated_at = $5 WHERE id = $6",
    )
    .bind(snapshot.0)
    .bind(snapshot.1)
    .bind(snapshot.2)
    .bind(snapshot.3)
    .bind(snapshot.4)
    .bind(USER_ID)
    .execute(&mut connection)
    .await?;
    let mut owner_connection = PgConnection::connect(rust_owner.url()).await?;
    sqlx::query("UPDATE users SET role_id = $1 WHERE id = $2")
        .bind(snapshot.5)
        .bind(USER_ID)
        .execute(&mut owner_connection)
        .await?;
    sqlx::query("DELETE FROM webauthn_credentials WHERE user_id = $1")
        .bind(USER_ID)
        .execute(&mut owner_connection)
        .await?;
    if let Some((
        id,
        created_at,
        external_id,
        nickname,
        public_key,
        sign_count,
        updated_at,
        user_id,
    )) = webauthn_before
    {
        sqlx::query(
            "INSERT INTO webauthn_credentials \
                (id, created_at, external_id, nickname, public_key, sign_count, updated_at, user_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(created_at)
        .bind(external_id)
        .bind(nickname)
        .bind(public_key)
        .bind(sign_count)
        .bind(updated_at)
        .bind(user_id)
        .execute(&mut owner_connection)
            .await?;
    }
        writer.delete_browser_session(&session_id).await?;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    operation_and_cleanup(operation, cleanup)
}

async fn two_factor_state(
    database: &str,
    user_id: i64,
) -> Result<(bool, Option<String>, Option<Vec<String>>), sqlx::Error> {
    let mut connection = PgConnection::connect(database).await?;
    sqlx::query_as(
        "SELECT otp_required_for_login, otp_secret, otp_backup_codes::text[] \
         FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&mut connection)
    .await
}

fn fixture_totp_code(secret: &str, timestamp: i64) -> Result<String, Box<dyn Error>> {
    let mut decoded = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u8;
    for byte in secret.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'2'..=b'7' => byte - b'2' + 26,
            _ => return Err("invalid test Base32 secret".into()),
        };
        buffer = (buffer << 5) | u32::from(value);
        bits += 5;
        while bits >= 8 {
            bits -= 8;
            decoded.push(u8::try_from(buffer >> bits)?);
            buffer = if bits == 0 {
                0
            } else {
                buffer & ((1_u32 << bits) - 1)
            };
        }
    }
    let counter = timestamp.div_euclid(30).to_be_bytes();
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(&decoded)?;
    mac.update(&counter);
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let value = (u32::from(digest[offset]) & 0x7f) << 24
        | u32::from(digest[offset + 1]) << 16
        | u32::from(digest[offset + 2]) << 8
        | u32::from(digest[offset + 3]);
    Ok(format!("{:06}", value % 1_000_000))
}

async fn password_reset_form_tokens(target: &Url) -> Result<(String, String), Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let request = RequestSpec::new(Method::GET, "/auth/password/new", None, headers, Vec::new())?;
    let response = send_single(target, &request, "password reset form").await?;
    if response.status != 200 {
        return Err(format!("password reset form returned HTTP {}", response.status).into());
    }
    let cookie = response
        .headers
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .collect::<Vec<_>>()
        .join("; ");
    if cookie.is_empty() {
        return Err("password reset form did not set a session/CSRF cookie".into());
    }
    let field = if String::from_utf8_lossy(&response.body).contains("name=\"authenticity_token\"") {
        "authenticity_token"
    } else {
        "csrf_token"
    };
    let token = hidden_form_value(&response.body, field)
        .ok_or_else(|| format!("password reset form did not contain {field}"))?;
    Ok((cookie, token))
}

fn hidden_form_value(body: &[u8], name: &str) -> Option<String> {
    let html = String::from_utf8_lossy(body);
    let name_marker = format!("name=\"{name}\"");
    let input = html.get(html.find(&name_marker)?..)?;
    let value = input.split("value=\"").nth(1)?.split('"').next()?;
    Some(value.to_owned())
}

fn browser_form_request(
    method: Method,
    path: &str,
    state: &BrowserFormState,
    csrf_field: Option<&str>,
    fields: &str,
) -> Result<RequestSpec, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let cookies = state.cookie_header();
    if !cookies.is_empty() {
        headers.insert(COOKIE, HeaderValue::from_str(&cookies)?);
    }
    let body = if let Some(csrf_field) = csrf_field {
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair(csrf_field, state.csrf_token()?);
        let mut body = serializer.finish();
        if !fields.is_empty() {
            body.push('&');
            body.push_str(fields);
        }
        body.into_bytes()
    } else {
        Vec::new()
    };
    Ok(RequestSpec::new(method, path, None, headers, body)?)
}

async fn load_browser_form(
    target: &Url,
    path: &str,
    state: &mut BrowserFormState,
    csrf_field: &str,
    side: &'static str,
) -> Result<CapturedResponse, Box<dyn Error>> {
    let response = send_single(
        target,
        &browser_form_request(Method::GET, path, state, None, "")?,
        side,
    )
    .await?;
    if response.status != StatusCode::OK.as_u16() {
        return Err(format!("{side} form {path} returned HTTP {}", response.status).into());
    }
    state.update_from_response(&response, csrf_field)?;
    Ok(response)
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_password_recovery_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    const RESET_TOKEN: &str = "fixture-password-reset-token";
    const RESET_GRANT_TOKEN: &str = "fixture-password-reset-grant";
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("password recovery requires a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("password recovery requires a Rust writer URL");
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .expect("password recovery requires a Rust owner URL for cleanup");
    let snapshot = |database: &str| {
        let database = database.to_owned();
        async move {
            let mut connection = PgConnection::connect(&database).await?;
            sqlx::query_as::<
                _,
                (
                    String,
                    Option<String>,
                    Option<NaiveDateTime>,
                    Option<String>,
                    Option<NaiveDateTime>,
                    NaiveDateTime,
                ),
            >(
                "SELECT encrypted_password, reset_password_token, reset_password_sent_at, \
                        sign_in_token, sign_in_token_sent_at, updated_at \
                 FROM users WHERE id = 105",
            )
            .fetch_one(&mut connection)
            .await
        }
    };
    let mastodon_before = snapshot(mastodon_owner.url()).await?;
    let rust_before = snapshot(rust_writer.url()).await?;
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_form = password_reset_form_tokens(targets.mastodon()).await?;
    let rust_form = password_reset_form_tokens(targets.rust()).await?;
    let mut cleanup_connection = PgConnection::connect(rust_owner.url()).await?;
    let oauth_grant_sequence: (i64, bool) =
        sqlx::query_as("SELECT last_value, is_called FROM public.oauth_access_grants_id_seq")
            .fetch_one(&mut cleanup_connection)
            .await?;
    let form_request = |method: Method,
                        form: &(String, String),
                        rails: bool,
                        fields: &str|
     -> Result<RequestSpec, Box<dyn Error>> {
        let mut headers = HeaderMap::new();
        headers.insert(
            HOST,
            HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
        );
        headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        headers.insert(COOKIE, HeaderValue::from_str(&form.0)?);
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        let csrf_name = if rails {
            "authenticity_token"
        } else {
            "csrf_token"
        };
        Ok(RequestSpec::new(
            method,
            "/auth/password",
            None,
            headers,
            format!("{csrf_name}={}&{fields}", form.1).into_bytes(),
        )?)
    };
    let digest = format!("{:x}", Sha256::digest(RESET_TOKEN.as_bytes()));
    let seed_token = |database: &str| {
        let database = database.to_owned();
        let digest = digest.clone();
        async move {
            let mut connection = PgConnection::connect(&database).await?;
            sqlx::query(
                "UPDATE users SET reset_password_token = $1, \
                        reset_password_sent_at = clock_timestamp() WHERE id = 105",
            )
            .bind(digest)
            .execute(&mut connection)
            .await?;
            Ok::<(), sqlx::Error>(())
        }
    };
    let operation = async {
        {
            let mut connection = PgConnection::connect(rust_writer.url()).await?;
            sqlx::query("DELETE FROM oauth_access_grants WHERE token = $1")
                .bind(RESET_GRANT_TOKEN)
                .execute(&mut connection)
                .await?;
            sqlx::query(
                "INSERT INTO oauth_access_grants ( \
                    application_id, code_challenge, code_challenge_method, created_at, expires_in, \
                    redirect_uri, resource_owner_id, revoked_at, scopes, token) \
                 VALUES (301, NULL, NULL, clock_timestamp(), 600, \
                         'urn:ietf:wg:oauth:2.0:oob', 105, NULL, 'read', $1)",
            )
            .bind(RESET_GRANT_TOKEN)
            .execute(&mut connection)
            .await?;
        }
        let mastodon_request = form_request(
            Method::POST,
            &mastodon_form,
            true,
            "user%5Bemail%5D=pending%40fixture.invalid",
        )?;
        let rust_request = form_request(
            Method::POST,
            &rust_form,
            false,
            "user%5Bemail%5D=pending%40fixture.invalid",
        )?;
        let mastodon_response =
            send_single(targets.mastodon(), &mastodon_request, "Mastodon password reset request")
                .await?;
        let rust_response =
            send_single(targets.rust(), &rust_request, "Rust password reset request").await?;
        if mastodon_response.status != 302 || rust_response.status != 302 {
            return Err(format!(
                "password reset request status differs: Mastodon={}, Rust={}",
                mastodon_response.status, rust_response.status,
            )
            .into());
        }
        seed_token(rust_writer.url()).await?;
        let rust_request = form_request(
            Method::PATCH,
            &rust_form,
            false,
            &format!(
                "reset_password_token={RESET_TOKEN}&user%5Bpassword%5D=fixture-new-password&user%5Bpassword_confirmation%5D=fixture-new-password"
            ),
        )?;
        let rust_response =
            send_single(targets.rust(), &rust_request, "Rust password reset update").await?;
        if rust_response.status != 302 {
            return Err(format!(
                "Rust password reset update returned HTTP {}",
                rust_response.status,
            )
            .into());
        }
        let mut connection = PgConnection::connect(rust_writer.url()).await?;
        let (password, token) = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT encrypted_password, reset_password_token FROM users WHERE id = 105",
        )
        .fetch_one(&mut connection)
        .await?;
        if !verify_password("fixture-new-password", &password) || token.is_some() {
            return Err("password reset did not consume the token and update the password".into());
        }
        let grant_revoked = sqlx::query_scalar::<_, bool>(
            "SELECT revoked_at IS NOT NULL FROM oauth_access_grants WHERE token = $1",
        )
        .bind(RESET_GRANT_TOKEN)
        .fetch_one(&mut connection)
        .await?;
        if !grant_revoked {
            return Err("password reset did not revoke the pending OAuth grant".into());
        }
        let replay = form_request(
            Method::PATCH,
            &rust_form,
            false,
            &format!(
                "reset_password_token={RESET_TOKEN}&user%5Bpassword%5D=fixture-new-password&user%5Bpassword_confirmation%5D=fixture-new-password"
            ),
        )?;
        let replay_response =
            send_single(targets.rust(), &replay, "Rust password reset replay").await?;
        if replay_response.status != 422 {
            return Err(format!(
                "Rust password reset replay returned HTTP {}",
                replay_response.status
            )
            .into());
        }
        sqlx::query(
            "UPDATE users SET reset_password_token = $1, \
                    reset_password_sent_at = clock_timestamp() - INTERVAL '7 hours' \
             WHERE id = 105",
        )
        .bind(&digest)
        .execute(&mut connection)
        .await?;
        let expired = form_request(
            Method::PATCH,
            &rust_form,
            false,
            &format!(
                "reset_password_token={RESET_TOKEN}&user%5Bpassword%5D=fixture-new-password&user%5Bpassword_confirmation%5D=fixture-new-password"
            ),
        )?;
        let expired_response =
            send_single(targets.rust(), &expired, "Rust expired password reset").await?;
        if expired_response.status != 422 {
            return Err(format!(
                "Rust expired password reset returned HTTP {}",
                expired_response.status
            )
            .into());
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    for (database, state) in [
        (mastodon_owner.url(), mastodon_before),
        (rust_writer.url(), rust_before),
    ] {
        let mut connection = PgConnection::connect(database).await?;
        sqlx::query(
            "UPDATE users SET encrypted_password = $1, reset_password_token = $2, \
                    reset_password_sent_at = $3, sign_in_token = $4, \
                    sign_in_token_sent_at = $5, updated_at = $6 WHERE id = 105",
        )
        .bind(state.0)
        .bind(state.1)
        .bind(state.2)
        .bind(state.3)
        .bind(state.4)
        .bind(state.5)
        .execute(&mut connection)
        .await?;
        sqlx::query("DELETE FROM oauth_access_grants WHERE token = $1")
            .bind(RESET_GRANT_TOKEN)
            .execute(&mut connection)
            .await?;
    }
    sqlx::query("SELECT setval('public.oauth_access_grants_id_seq'::regclass, $1, $2)")
        .bind(oauth_grant_sequence.0)
        .bind(oauth_grant_sequence.1)
        .execute(&mut cleanup_connection)
        .await?;
    operation
}

type DifferentialSequenceState = (i64, bool, i64, bool, i64, bool, i64, bool);

async fn differential_sequence_state(
    connection: &mut PgConnection,
) -> Result<DifferentialSequenceState, sqlx::Error> {
    sqlx::query_as(
        "SELECT \
            (SELECT last_value FROM public.accounts_id_seq), \
            (SELECT is_called FROM public.accounts_id_seq), \
            (SELECT last_value FROM public.account_stats_id_seq), \
            (SELECT is_called FROM public.account_stats_id_seq), \
            (SELECT last_value FROM public.users_id_seq), \
            (SELECT is_called FROM public.users_id_seq), \
            (SELECT last_value FROM rustodon.outbox_events_id_seq), \
            (SELECT is_called FROM rustodon.outbox_events_id_seq)",
    )
    .fetch_one(connection)
    .await
}

async fn cleanup_admin_create_user_case(
    connection: &mut PgConnection,
    emails: &[String],
    sequences: DifferentialSequenceState,
) -> Result<(), sqlx::Error> {
    let account_ids: Vec<i64> =
        sqlx::query_scalar("SELECT account_id FROM users WHERE email = ANY($1)")
            .bind(emails.to_vec())
            .fetch_all(&mut *connection)
            .await?;
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE payload #>> '{arguments,to}' = ANY($1)")
        .bind(emails.to_vec())
        .execute(&mut *connection)
        .await?;
    sqlx::query("DELETE FROM users WHERE email = ANY($1)")
        .bind(emails.to_vec())
        .execute(&mut *connection)
        .await?;
    if !account_ids.is_empty() {
        sqlx::query("DELETE FROM account_stats WHERE account_id = ANY($1)")
            .bind(&account_ids)
            .execute(&mut *connection)
            .await?;
        sqlx::query("DELETE FROM accounts WHERE id = ANY($1)")
            .bind(&account_ids)
            .execute(&mut *connection)
            .await?;
    }
    sqlx::query(
        "SELECT \
            setval('public.accounts_id_seq'::regclass, $1, $2), \
            setval('public.account_stats_id_seq'::regclass, $3, $4), \
            setval('public.users_id_seq'::regclass, $5, $6), \
            setval('rustodon.outbox_events_id_seq'::regclass, $7, $8)",
    )
    .bind(sequences.0)
    .bind(sequences.1)
    .bind(sequences.2)
    .bind(sequences.3)
    .bind(sequences.4)
    .bind(sequences.5)
    .bind(sequences.6)
    .bind(sequences.7)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

fn open_confirmation_envelope(sealed_token: &str, secret: &str) -> Result<String, Box<dyn Error>> {
    let envelope = URL_SAFE_NO_PAD.decode(sealed_token)?;
    if envelope.len() < 1 + 12 + 16 || envelope[0] != 1 {
        return Err("confirmation envelope has an invalid version or length".into());
    }
    let nonce_start = 1;
    let ciphertext_start = nonce_start + 12;
    let tag_start = envelope.len() - 16;
    let mut ciphertext = envelope[ciphertext_start..tag_start].to_vec();
    let key = Sha256::digest(secret.as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key)?;
    cipher
        .decrypt_in_place_detached(
            Nonce::from_slice(&envelope[nonce_start..ciphertext_start]),
            b"rustodon-mail-token-v1:rustodon.mail.confirmation",
            &mut ciphertext,
            Tag::from_slice(&envelope[tag_start..]),
        )
        .map_err(|_| "confirmation envelope authentication failed")?;
    Ok(String::from_utf8(ciphertext)?)
}

fn devise_confirmation_digest(token: &str, secret: &str) -> String {
    let mut derived_key = [0_u8; 64];
    pbkdf2_hmac::<Sha1>(
        secret.as_bytes(),
        b"Devise confirmation_token",
        65_536,
        &mut derived_key,
    );
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&derived_key).expect("HMAC key is valid");
    mac.update(token.as_bytes());
    format!("{:x}", mac.finalize().into_bytes())
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_admin_create_user_case(
    config: DifferentialConfig,
) -> Result<(), Box<dyn Error>> {
    const CONFIRMATION_TOKEN: &str = "fixture-confirmation-token";
    const EXPIRED_TOKEN: &str = "fixture-expired-confirmation-token";

    config.validate_database_comments().await?;
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .ok_or("admin create-user requires a Rust writer URL")?;
    let rust_owner = config
        .rust_owner_database
        .as_ref()
        .ok_or("admin create-user requires a Rust owner URL for cleanup")?;
    let email = format!("admin-create-{}@fixture.invalid", config.run_id);
    let writer = WriteRepository::connect(rust_writer.url()).await?;
    let mut connection = PgConnection::connect(rust_writer.url()).await?;
    let mut cleanup_connection = PgConnection::connect(rust_owner.url()).await?;
    let sequences = differential_sequence_state(&mut cleanup_connection).await?;
    let fixed_email = format!("confirmation-{}@fixture.invalid", config.run_id);
    let expired_email = format!("expired-{}@fixture.invalid", config.run_id);
    let cleanup_emails = vec![email.clone(), fixed_email.clone(), expired_email.clone()];
    let mail_config = MailConfig::new(
        SmtpConfig::Disabled {
            warning: ConfigWarning::SmtpDisabled,
        },
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        SecretString::new(ADMIN_RECOVERY_SECRET.to_owned()),
    );
    let operation = async {
        let username = format!("fixture_admin_{}", config.run_id);
        let output = run_admin_create_user_cli(
            config.rust_database.url(),
            rust_writer.url(),
            config
                .rust_media
                .to_str()
                .ok_or("Rust media path is not UTF-8")?,
            &email,
            &username,
            ADMIN_RECOVERY_PASSWORD,
        )?;
        if !output.status.success() {
            return Err(format!(
                "admin create-user failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )
            .into());
        }
        if !String::from_utf8_lossy(&output.stderr).contains("confirmation mail queued") {
            return Err("admin create-user did not report queued confirmation mail".into());
        }

        let (
            admin_user_id,
            admin_account_id,
            encrypted_password,
            confirmation_digest,
            sent_at,
            confirmed_at,
        ): (
            i64,
            i64,
            String,
            Option<String>,
            Option<NaiveDateTime>,
            Option<NaiveDateTime>,
        ) = sqlx::query_as(
            "SELECT users.id, users.account_id, users.encrypted_password, \
                users.confirmation_token, users.confirmation_sent_at, users.confirmed_at \
         FROM users WHERE lower(users.email) = lower($1)",
        )
        .bind(&email)
        .fetch_one(&mut connection)
        .await?;
        if !verify_password(ADMIN_RECOVERY_PASSWORD, &encrypted_password)
            || confirmation_digest.is_none()
            || sent_at.is_none()
            || confirmed_at.is_some()
        {
            return Err("admin create-user did not create the expected pending account".into());
        }
        let account_stats: i64 =
            sqlx::query_scalar("SELECT count(*) FROM account_stats WHERE account_id = $1")
                .bind(admin_account_id)
                .fetch_one(&mut connection)
                .await?;
        if account_stats != 1 {
            return Err("admin create-user did not create account_stats".into());
        }
        let outbox = sqlx::query_as::<_, (String, Option<String>, Value)>(
            "SELECT kind, logical_key, payload FROM rustodon.outbox_events \
         WHERE kind = $1 AND payload #>> '{arguments,to}' = $2",
        )
        .bind(CONFIRMATION_JOB_KIND)
        .bind(&email)
        .fetch_all(&mut connection)
        .await?;
        if outbox.len() != 1 {
            return Err(format!(
                "expected one confirmation outbox event, got {}",
                outbox.len()
            )
            .into());
        }
        let (kind, logical_key, payload) = &outbox[0];
        let sealed_token = payload["arguments"]["token"]
            .as_str()
            .ok_or("confirmation outbox token is missing")?;
        let opened_token = open_confirmation_envelope(sealed_token, ADMIN_RECOVERY_SECRET)?;
        let expected_digest = devise_confirmation_digest(&opened_token, ADMIN_RECOVERY_SECRET);
        if kind != CONFIRMATION_JOB_KIND
            || !logical_key
                .as_deref()
                .is_some_and(|key| key.starts_with("confirmation:"))
            || payload["lane"] != "mail"
            || payload["arguments"]["to"] != email
            || opened_token.len() != 43
            || opened_token.chars().any(char::is_whitespace)
            || confirmation_digest.as_deref() != Some(expected_digest.as_str())
        {
            return Err(
                "confirmation outbox event has unsafe, invalid, or mismatched metadata".into(),
            );
        }

        let reset_output = run_admin_reset_password_cli(
            config.rust_database.url(),
            rust_writer.url(),
            config
                .rust_media
                .to_str()
                .ok_or("Rust media path is not UTF-8")?,
            &email,
            ADMIN_RESET_PASSWORD,
        )?;
        if !reset_output.status.success()
            || !String::from_utf8_lossy(&reset_output.stderr).contains("password reset")
        {
            return Err(format!(
                "admin reset-password failed: stdout={} stderr={}",
                String::from_utf8_lossy(&reset_output.stdout),
                String::from_utf8_lossy(&reset_output.stderr),
            )
            .into());
        }
        let reset_encrypted_password: String =
            sqlx::query_scalar("SELECT encrypted_password FROM users WHERE id = $1")
                .bind(admin_user_id)
                .fetch_one(&mut connection)
                .await?;
        if !verify_password(ADMIN_RESET_PASSWORD, &reset_encrypted_password) {
            return Err("admin reset-password did not replace the password".into());
        }
        let cli_confirmed = writer
            .confirm_user_with_token_and_secret(&opened_token, ADMIN_RECOVERY_SECRET)
            .await
            .map_err(|error| format!("CLI confirmation failed: {error}"))?;
        let cli_replayed = writer
            .confirm_user_with_token_and_secret(&opened_token, ADMIN_RECOVERY_SECRET)
            .await
            .map_err(|error| format!("CLI confirmation replay failed: {error}"))?;
        if !cli_confirmed || cli_replayed {
            return Err("CLI confirmation token was not single-use".into());
        }

        let fixed_username = format!("fixture_confirm_{}", config.run_id);
        let fixed_job = mail_config.confirmation_job(&fixed_email, CONFIRMATION_TOKEN)?;
        let fixed_user = writer
            .create_local_user_with_confirmation(
                &fixed_email,
                &fixed_username,
                ADMIN_RECOVERY_PASSWORD,
                CONFIRMATION_TOKEN,
                Some(ADMIN_RECOVERY_SECRET),
                &fixed_job,
            )
            .await
            .map_err(|error| format!("fixed confirmation user creation failed: {error}"))?;
        if fixed_user.confirmed {
            return Err("confirmation user was created as confirmed".into());
        }
        let confirmed = writer
            .confirm_user_with_token_and_secret(CONFIRMATION_TOKEN, ADMIN_RECOVERY_SECRET)
            .await
            .map_err(|error| format!("fixed confirmation failed: {error}"))?;
        let replayed = writer
            .confirm_user_with_token_and_secret(CONFIRMATION_TOKEN, ADMIN_RECOVERY_SECRET)
            .await
            .map_err(|error| format!("fixed confirmation replay failed: {error}"))?;
        if !confirmed || replayed {
            return Err("confirmation token was not single-use".into());
        }
        let fixed_state: (bool, bool, bool) = sqlx::query_as(
            "SELECT confirmed_at IS NOT NULL, confirmation_token IS NULL, \
                confirmation_sent_at IS NULL FROM users WHERE id = $1",
        )
        .bind(fixed_user.user_id)
        .fetch_one(&mut connection)
        .await?;
        if fixed_state != (true, true, true) {
            return Err("confirmation did not consume the stored token".into());
        }

        let expired_username = format!("fixture_expired_{}", config.run_id);
        let expired_job = mail_config.confirmation_job(&expired_email, EXPIRED_TOKEN)?;
        let expired_user = writer
            .create_local_user_with_confirmation(
                &expired_email,
                &expired_username,
                ADMIN_RECOVERY_PASSWORD,
                EXPIRED_TOKEN,
                Some(ADMIN_RECOVERY_SECRET),
                &expired_job,
            )
            .await
            .map_err(|error| format!("expired confirmation user creation failed: {error}"))?;
        sqlx::query(
            "UPDATE users SET confirmation_sent_at = clock_timestamp() - INTERVAL '3 days' \
         WHERE id = $1",
        )
        .bind(expired_user.user_id)
        .execute(&mut connection)
        .await?;
        if writer
            .confirm_user_with_token_and_secret(EXPIRED_TOKEN, ADMIN_RECOVERY_SECRET)
            .await
            .map_err(|error| format!("expired confirmation failed: {error}"))?
        {
            return Err("expired confirmation token was accepted".into());
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    let cleanup =
        cleanup_admin_create_user_case(&mut cleanup_connection, &cleanup_emails, sequences).await;
    match (operation, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(operation), Ok(())) => Err(operation),
        (Ok(()), Err(cleanup)) => {
            Err(format!("admin create-user cleanup failed: {cleanup}").into())
        }
        (Err(operation), Err(cleanup)) => {
            Err(format!("admin create-user failed: {operation}; cleanup failed: {cleanup}").into())
        }
    }
}

fn run_admin_create_user_cli(
    database_url: &str,
    write_database_url: &str,
    media_root: &str,
    email: &str,
    username: &str,
    password: &str,
) -> Result<std::process::Output, Box<dyn Error>> {
    run_admin_cli(
        &[
            "admin",
            "create-user",
            "--email",
            email,
            "--username",
            username,
        ],
        database_url,
        write_database_url,
        media_root,
        password,
    )
}

fn run_admin_reset_password_cli(
    database_url: &str,
    write_database_url: &str,
    media_root: &str,
    email: &str,
    password: &str,
) -> Result<std::process::Output, Box<dyn Error>> {
    run_admin_cli(
        &["admin", "reset-password", "--email", email],
        database_url,
        write_database_url,
        media_root,
        password,
    )
}

fn run_admin_cli(
    args: &[&str],
    database_url: &str,
    write_database_url: &str,
    media_root: &str,
    password: &str,
) -> Result<std::process::Output, Box<dyn Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustodon"))
        .args(args)
        .env_clear()
        .env("DATABASE_URL", database_url)
        .env("WRITE_DATABASE_URL", write_database_url)
        .env("LOCAL_DOMAIN", "fixture-v4-6-5.rustodon.invalid")
        .env("WEB_DOMAIN", "fixture-v4-6-5.rustodon.invalid")
        .env("PAPERCLIP_ROOT_PATH", media_root)
        .env("PAPERCLIP_ROOT_URL", "/system")
        .env("DB_POOL", "1")
        .env("DB_SSLMODE", "disable")
        .env("SECRET_KEY_BASE", ADMIN_RECOVERY_SECRET)
        .env(
            "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY",
            "33333333333333333333333333333333",
        )
        .env(
            "ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY",
            "11111111111111111111111111111111",
        )
        .env(
            "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT",
            "22222222222222222222222222222222",
        )
        .env("SMTP_SERVER", "127.0.0.1")
        .env("SMTP_PORT", "9")
        .env(
            "SMTP_FROM_ADDRESS",
            "notifications@fixture-v4-6-5.rustodon.invalid",
        )
        .env("SMTP_DOMAIN", "fixture-v4-6-5.rustodon.invalid")
        .env("SMTP_AUTH_METHOD", "none")
        .env("SMTP_ENABLE_STARTTLS", "never")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("admin command stdin was not available")?
        .write_all(format!("{password}\n").as_bytes())?;
    Ok(child.wait_with_output()?)
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_oauth_authorization_code_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("OAuth authorization-code differential requires a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("OAuth authorization-code differential requires a Rust writer URL");
    let code = "fixture-authorization-code-v4-6-5";
    let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    let snapshot_oauth_ids = |database: &str| {
        let database = database.to_owned();
        async move {
            let mut connection = PgConnection::connect(&database).await?;
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT \
                    (SELECT COALESCE(MAX(id), 0) FROM oauth_access_grants), \
                    (SELECT COALESCE(MAX(id), 0) FROM oauth_access_tokens)",
            )
            .fetch_one(&mut connection)
            .await
        }
    };
    let mastodon_oauth_ids = snapshot_oauth_ids(mastodon_owner.url()).await?;
    let rust_oauth_ids = snapshot_oauth_ids(rust_writer.url()).await?;
    for database in [mastodon_owner.url(), rust_writer.url()] {
        let mut connection = PgConnection::connect(database).await?;
        sqlx::query(
            "INSERT INTO oauth_access_grants ( \
                application_id, code_challenge, code_challenge_method, created_at, expires_in, \
                redirect_uri, resource_owner_id, revoked_at, scopes, token) \
             VALUES (301, $1, 'S256', clock_timestamp(), 600, \
                     'urn:ietf:wg:oauth:2.0:oob', 101, NULL, 'profile', $2)",
        )
        .bind(challenge)
        .bind(code)
        .execute(&mut connection)
        .await?;
    }
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let request = |code: &str| -> Result<RequestSpec, Box<dyn Error>> {
        let mut headers = HeaderMap::new();
        headers.insert(
            HOST,
            HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
        );
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        Ok(RequestSpec::new(
            Method::POST,
            "/oauth/token",
            None,
            headers,
            format!(
                "grant_type=authorization_code&client_id=rustodon-fixture-client-v4-6-5&client_secret=fixture-only-client-secret-v4-6-5&code={code}&redirect_uri=urn%3Aietf%3Awg%3Aoauth%3A2.0%3Aoob&code_verifier=dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
            )
            .into_bytes(),
        )?)
    };
    let responses = send_identically(&targets, &request(code)?).await?;
    if responses.mastodon.status != 200 || responses.rust.status != 200 {
        return Err(format!(
            "OAuth authorization-code status differs: Mastodon={}, Rust={}",
            responses.mastodon.status, responses.rust.status,
        )
        .into());
    }
    let mut mastodon_body = serde_json::from_slice::<Value>(&responses.mastodon.body)?;
    let mut rust_body = serde_json::from_slice::<Value>(&responses.rust.body)?;
    let mastodon_access_token = mastodon_body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or("Mastodon OAuth response did not include an access token")?
        .to_owned();
    let rust_access_token = rust_body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or("Rust OAuth response did not include an access token")?
        .to_owned();
    for body in [&mut mastodon_body, &mut rust_body] {
        let object = body
            .as_object_mut()
            .ok_or("OAuth authorization-code response is not an object")?;
        object.insert(
            "access_token".to_owned(),
            Value::String("<generated>".to_owned()),
        );
        object.insert(
            "created_at".to_owned(),
            Value::String("<generated>".to_owned()),
        );
    }
    if mastodon_body != rust_body {
        return Err(format!(
            "OAuth authorization-code response differs: Mastodon={mastodon_body}, Rust={rust_body}"
        )
        .into());
    }
    let userinfo_request = |token: &str| -> Result<RequestSpec, Box<dyn Error>> {
        let mut headers = HeaderMap::new();
        headers.insert(
            HOST,
            HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
        );
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        Ok(RequestSpec::new(
            Method::GET,
            "/oauth/userinfo",
            None,
            headers,
            Vec::new(),
        )?)
    };
    let mastodon_userinfo = send_single(
        targets.mastodon(),
        &userinfo_request(&mastodon_access_token)?,
        "Mastodon OAuth userinfo",
    )
    .await?;
    let rust_userinfo = send_single(
        targets.rust(),
        &userinfo_request(&rust_access_token)?,
        "Rust OAuth userinfo",
    )
    .await?;
    compare_responses(
        &mastodon_userinfo,
        &rust_userinfo,
        &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("OAuth userinfo: {error}"))?;
    let replay = send_identically(&targets, &request(code)?).await?;
    compare_responses(
        &replay.mastodon,
        &replay.rust,
        &[
            CONTENT_TYPE,
            CACHE_CONTROL,
            http::header::PRAGMA,
            WWW_AUTHENTICATE,
        ],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("OAuth authorization-code replay: {error}"))?;
    let writer = WriteRepository::connect(rust_writer.url()).await?;
    let session_id = writer
        .create_browser_session(
            101,
            IpNetwork::from("192.0.2.10".parse::<std::net::IpAddr>()?),
            "rustodon-oauth-consent-test",
        )
        .await?;
    let consent_query = format!(
        "client_id=rustodon-fixture-client-v4-6-5&redirect_uri=urn%3Aietf%3Awg%3Aoauth%3A2.0%3Aoob&response_type=code&scope=read&state=consent-state&code_challenge={challenge}&code_challenge_method=S256"
    );
    let mut consent_headers = HeaderMap::new();
    consent_headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    consent_headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
    consent_headers.insert(
        COOKIE,
        HeaderValue::from_str(&format!(
            "_mastodon_session={session_id}; csrf_token=consent-csrf"
        ))?,
    );
    consent_headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let consent_request = RequestSpec::new(
        Method::GET,
        "/oauth/authorize",
        Some(consent_query.clone()),
        consent_headers.clone(),
        Vec::new(),
    )?;
    let consent_response =
        send_single(targets.rust(), &consent_request, "Rust OAuth consent").await?;
    if consent_response.status != 200
        || !String::from_utf8_lossy(&consent_response.body).contains("Authorize")
    {
        return Err("Rust OAuth consent page was not served".into());
    }
    let consent_csrf_cookie = consent_response
        .headers
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .split(';')
                .next()
                .filter(|cookie| cookie.starts_with("__Host-csrf_token="))
        })
        .ok_or("Rust OAuth consent page did not rotate the CSRF cookie")?;
    let consent_csrf_token = hidden_form_value(&consent_response.body, "csrf_token")
        .ok_or("Rust OAuth consent page did not render the CSRF token")?;
    consent_headers.insert(
        COOKIE,
        HeaderValue::from_str(&format!(
            "_mastodon_session={session_id}; {consent_csrf_cookie}"
        ))?,
    );
    consent_headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    let approve_request = RequestSpec::new(
        Method::POST,
        "/oauth/authorize",
        Some(consent_query),
        consent_headers,
        format!("csrf_token={consent_csrf_token}&approve=true").into_bytes(),
    )?;
    let approve_response =
        send_single(targets.rust(), &approve_request, "Rust OAuth approval").await?;
    if approve_response.status != 200
        || !String::from_utf8_lossy(&approve_response.body).contains("<p>")
    {
        return Err(format!(
            "Rust OAuth approval did not render the OOB code: status={}, body={}",
            approve_response.status,
            String::from_utf8_lossy(&approve_response.body),
        )
        .into());
    }
    writer.delete_browser_session(&session_id).await?;
    for (database, oauth_ids) in [
        (mastodon_owner.url(), mastodon_oauth_ids),
        (rust_writer.url(), rust_oauth_ids),
    ] {
        let mut connection = PgConnection::connect(database).await?;
        let revoked = sqlx::query_scalar::<_, bool>(
            "SELECT revoked_at IS NOT NULL FROM oauth_access_grants WHERE token = $1",
        )
        .bind(code)
        .fetch_one(&mut connection)
        .await?;
        if !revoked {
            return Err("OAuth authorization grant was not revoked after exchange".into());
        }
        sqlx::query("DELETE FROM oauth_access_grants WHERE id > $1")
            .bind(oauth_ids.0)
            .execute(&mut connection)
            .await?;
        sqlx::query("DELETE FROM oauth_access_tokens WHERE id > $1")
            .bind(oauth_ids.1)
            .execute(&mut connection)
            .await?;
    }
    Ok(())
}

pub(crate) async fn run_conversation_writes_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("conversation differential configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("conversation differential configuration must include a Rust writer URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_rows = notification_rows(mastodon_owner.url(), "account_conversations").await?;
    let rust_rows = notification_rows(rust_writer.url(), "account_conversations").await?;
    let operation = async {
        for (label, method, path, token) in [
            (
                "conversation index",
                Method::GET,
                "/api/v1/conversations/",
                "fixture-bearer-read-statuses-v4-6-5",
            ),
            (
                "conversation read",
                Method::POST,
                "/api/v1/conversations/9302/read/",
                "fixture-bearer-token-v4-6-5",
            ),
            (
                "conversation unread",
                Method::POST,
                "/api/v1/conversations/9302/unread/",
                "fixture-bearer-token-v4-6-5",
            ),
            (
                "conversation delete",
                Method::DELETE,
                "/api/v1/conversations/9302/",
                "fixture-bearer-token-v4-6-5",
            ),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
            );
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
            headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
            let request = RequestSpec::new(method, path, None, headers, Vec::new())?;
            let responses = send_identically(&targets, &request).await?;
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[
                    http::header::CONTENT_TYPE,
                    http::header::CACHE_CONTROL,
                    http::header::VARY,
                    http::header::LINK,
                ],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("{label}: {error}"))?;
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    restore_rows(
        mastodon_owner.url(),
        "account_conversations",
        &mastodon_rows,
    )
    .await?;
    restore_rows(rust_writer.url(), "account_conversations", &rust_rows).await?;
    operation
}

pub(crate) async fn run_account_profile_writes_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("profile differential configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("profile differential configuration must include a Rust writer URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_before = account_profile_state(mastodon_owner.url()).await?;
    let rust_before = account_profile_state(rust_writer.url()).await?;
    assert_eq!(
        account_profile_stable_state(&mastodon_before),
        account_profile_stable_state(&rust_before)
    );
    let mastodon_account_tags = account_tag_rows(mastodon_owner.url()).await?;
    let rust_account_tags = account_tag_rows(rust_writer.url()).await?;
    let operation = async {
        let mut denied_headers = profile_headers("fixture-bearer-read-statuses-v4-6-5")?;
        denied_headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let denied_request = RequestSpec::new(
            Method::PATCH,
            "/api/v1/accounts/update_credentials",
            None,
            denied_headers,
            b"display_name=Denied+profile+write".to_vec(),
        )?;
        let denied = send_identically(&targets, &denied_request).await?;
        compare_responses(
            &denied.mastodon,
            &denied.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("profile update wrong scope: {error}"))?;

        let headers = profile_headers("fixture-bearer-token-v4-6-5")?;
        let request = RequestSpec::new(
            Method::PATCH,
            "/api/v1/accounts/update_credentials",
            None,
            headers,
            b"display_name=Profile+write&note=Profile+update+note&bot=true&locked=false&discoverable=true&hide_collections=false&indexable=true&attribution_domains%5B%5D=https%3A%2F%2Fexample.com&attribution_domains%5B%5D=example.com&attribution_domains%5B%5D=*.example.org&fields_attributes%5B%5D%5Bname%5D=Website&fields_attributes%5B%5D%5Bvalue%5D=https%3A%2F%2Fexample.com&fields_attributes%5B%5D%5Bname%5D=Pronouns&fields_attributes%5B%5D%5Bvalue%5D=they%2Fthem&source%5Bprivacy%5D=unlisted&source%5Bsensitive%5D=true&source%5Blanguage%5D=fr&source%5Bquote_policy%5D=followers".to_vec(),
        )?;
        let responses = send_identically(&targets, &request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("profile update: {error}"))?;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    let mastodon_after = account_profile_state(mastodon_owner.url()).await?;
    let rust_after = account_profile_state(rust_writer.url()).await?;
    restore_account_profile_state(mastodon_owner.url(), &mastodon_before).await?;
    restore_account_profile_state(rust_writer.url(), &rust_before).await?;
    restore_account_tag_rows(mastodon_owner.url(), &mastodon_account_tags).await?;
    restore_account_tag_rows(rust_writer.url(), &rust_account_tags).await?;
    operation?;
    assert_eq!(
        account_profile_stable_state(&mastodon_after),
        account_profile_stable_state(&rust_after)
    );
    assert_eq!(mastodon_after.display_name, "Profile write");
    assert_eq!(mastodon_after.note, "Profile update note");
    assert_eq!(mastodon_after.actor_type.as_deref(), Some("Service"));
    assert_eq!(mastodon_after.discoverable, Some(true));
    assert_eq!(
        mastodon_after.attribution_domains,
        Some(vec!["example.com".to_owned(), "example.org".to_owned(),])
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_media_writes_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn Error>> {
    config.validate_database_comments().await?;
    let mastodon_owner = config
        .mastodon_owner_database
        .as_ref()
        .expect("media differential configuration must include a Mastodon owner URL");
    let rust_writer = config
        .rust_write_database
        .as_ref()
        .expect("media differential configuration must include a Rust writer URL");
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let mastodon_rows_before = media_rows(mastodon_owner.url(), MEDIA_ACCOUNT_ID).await?;
    let rust_rows_before = media_rows(rust_writer.url(), MEDIA_ACCOUNT_ID).await?;
    if mastodon_rows_before != rust_rows_before {
        return Err("media database baselines differ before the write case".into());
    }
    let mastodon_media_before = MediaSnapshot::capture(&config.mastodon_media)?;
    let rust_media_before = MediaSnapshot::capture(&config.rust_media)?;
    compare_media(
        &mastodon_media_before,
        &rust_media_before,
        DEFAULT_MISMATCH_LIMIT,
    )?;

    let image = std::fs::read(MEDIA_FIXTURE)?;
    let operation = async {
        let denied = media_request(
            Method::POST,
            "/api/v1/media/",
            "fixture-bearer-read-statuses-v4-6-5",
            &multipart_image_body(&image),
        )?;
        let denied_responses = send_identically(&targets, &denied).await?;
        compare_responses(
            &denied_responses.mastodon,
            &denied_responses.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("media create wrong scope: {error}"))?;

        let create = media_request(
            Method::POST,
            "/api/v1/media/",
            "fixture-bearer-token-v4-6-5",
            &multipart_image_body(&image),
        )?;
        let mut created = send_identically(&targets, &create).await?;
        let mastodon_id = normalize_media_response(&mut created.mastodon, "Mastodon")?;
        let rust_id = normalize_media_response(&mut created.rust, "Rust")?;
        compare_responses(
            &created.mastodon,
            &created.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[MEDIA_BLURHASH],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("media create: {error}"))?;

        let mastodon_created = media_state(mastodon_owner.url(), mastodon_id).await?;
        let rust_created = media_state(rust_writer.url(), rust_id).await?;
        compare_media_rows(&mastodon_created, &rust_created)?;

        let mastodon_after_create = MediaSnapshot::capture(&config.mastodon_media)?;
        let rust_after_create = MediaSnapshot::capture(&config.rust_media)?;
        assert_new_media_files(&mastodon_media_before, &mastodon_after_create, "Mastodon")?;
        assert_new_media_files(&rust_media_before, &rust_after_create, "Rust")?;

        let update_body = b"description=Updated+attachment&focus=0.25%2C-0.5".to_vec();
        let mastodon_update = media_request(
            Method::PATCH,
            &format!("/api/v1/media/{mastodon_id}/"),
            "fixture-bearer-token-v4-6-5",
            &update_body,
        )?;
        let rust_update = media_request(
            Method::PATCH,
            &format!("/api/v1/media/{rust_id}/"),
            "fixture-bearer-token-v4-6-5",
            &update_body,
        )?;
        let (mut mastodon_updated, mut rust_updated) = tokio::join!(
            send_single(targets.mastodon(), &mastodon_update, "Mastodon"),
            send_single(targets.rust(), &rust_update, "Rust")
        );
        let mastodon_updated = mastodon_updated
            .as_mut()
            .map_err(|error| error.to_string())?;
        let rust_updated = rust_updated.as_mut().map_err(|error| error.to_string())?;
        normalize_media_response(mastodon_updated, "Mastodon")?;
        normalize_media_response(rust_updated, "Rust")?;
        compare_responses(
            mastodon_updated,
            rust_updated,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[MEDIA_BLURHASH],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("media update: {error}"))?;

        let mastodon_blank_focus = media_request(
            Method::PATCH,
            &format!("/api/v1/media/{mastodon_id}/"),
            "fixture-bearer-token-v4-6-5",
            b"focus=",
        )?;
        let rust_blank_focus = media_request(
            Method::PATCH,
            &format!("/api/v1/media/{rust_id}/"),
            "fixture-bearer-token-v4-6-5",
            b"focus=",
        )?;
        let (mut mastodon_blank_focus, mut rust_blank_focus) = tokio::join!(
            send_single(targets.mastodon(), &mastodon_blank_focus, "Mastodon"),
            send_single(targets.rust(), &rust_blank_focus, "Rust")
        );
        let mastodon_blank_focus = mastodon_blank_focus
            .as_mut()
            .map_err(|error| error.to_string())?;
        let rust_blank_focus = rust_blank_focus
            .as_mut()
            .map_err(|error| error.to_string())?;
        normalize_media_response(mastodon_blank_focus, "Mastodon")?;
        normalize_media_response(rust_blank_focus, "Rust")?;
        compare_responses(
            mastodon_blank_focus,
            rust_blank_focus,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[MEDIA_BLURHASH],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("media blank focus update: {error}"))?;

        let mastodon_show = media_request(
            Method::GET,
            &format!("/api/v1/media/{mastodon_id}/"),
            "fixture-bearer-token-v4-6-5",
            &[],
        )?;
        let rust_show = media_request(
            Method::GET,
            &format!("/api/v1/media/{rust_id}/"),
            "fixture-bearer-token-v4-6-5",
            &[],
        )?;
        let (mastodon_show, rust_show) = tokio::join!(
            send_single(targets.mastodon(), &mastodon_show, "Mastodon"),
            send_single(targets.rust(), &rust_show, "Rust")
        );
        let mut mastodon_show = mastodon_show.map_err(|error| error.to_string())?;
        let mut rust_show = rust_show.map_err(|error| error.to_string())?;
        normalize_media_response(&mut mastodon_show, "Mastodon")?;
        normalize_media_response(&mut rust_show, "Rust")?;
        compare_responses(
            &mastodon_show,
            &rust_show,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[MEDIA_BLURHASH],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("media show: {error}"))?;

        let mastodon_delete = media_request(
            Method::DELETE,
            &format!("/api/v1/media/{mastodon_id}/"),
            "fixture-bearer-token-v4-6-5",
            &[],
        )?;
        let rust_delete = media_request(
            Method::DELETE,
            &format!("/api/v1/media/{rust_id}/"),
            "fixture-bearer-token-v4-6-5",
            &[],
        )?;
        let (mastodon_deleted, rust_deleted) = tokio::join!(
            send_single(targets.mastodon(), &mastodon_delete, "Mastodon"),
            send_single(targets.rust(), &rust_delete, "Rust")
        );
        let mastodon_deleted = mastodon_deleted.map_err(|error| error.to_string())?;
        let rust_deleted = rust_deleted.map_err(|error| error.to_string())?;
        compare_responses(
            &mastodon_deleted,
            &rust_deleted,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("media delete: {error}"))?;

        let v2_create = media_request(
            Method::POST,
            "/api/v2/media/",
            "fixture-bearer-token-v4-6-5",
            &multipart_image_body(&image),
        )?;
        let mut v2_created = send_identically(&targets, &v2_create).await?;
        let mastodon_v2_id = normalize_media_response(&mut v2_created.mastodon, "Mastodon")?;
        let rust_v2_id = normalize_media_response(&mut v2_created.rust, "Rust")?;
        compare_responses(
            &v2_created.mastodon,
            &v2_created.rust,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[MEDIA_BLURHASH],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("media v2 create: {error}"))?;
        let mastodon_v2 = media_state(mastodon_owner.url(), mastodon_v2_id).await?;
        let rust_v2 = media_state(rust_writer.url(), rust_v2_id).await?;
        compare_media_rows(&mastodon_v2, &rust_v2)?;
        let mastodon_media_after_v2 = MediaSnapshot::capture(&config.mastodon_media)?;
        let rust_media_after_v2 = MediaSnapshot::capture(&config.rust_media)?;
        assert_new_media_files(
            &mastodon_media_before,
            &mastodon_media_after_v2,
            "Mastodon v2",
        )?;
        assert_new_media_files(&rust_media_before, &rust_media_after_v2, "Rust v2")?;

        let mastodon_v2_delete = media_request(
            Method::DELETE,
            &format!("/api/v1/media/{mastodon_v2_id}/"),
            "fixture-bearer-token-v4-6-5",
            &[],
        )?;
        let rust_v2_delete = media_request(
            Method::DELETE,
            &format!("/api/v1/media/{rust_v2_id}/"),
            "fixture-bearer-token-v4-6-5",
            &[],
        )?;
        let (mastodon_v2_deleted, rust_v2_deleted) = tokio::join!(
            send_single(targets.mastodon(), &mastodon_v2_delete, "Mastodon"),
            send_single(targets.rust(), &rust_v2_delete, "Rust")
        );
        let mastodon_v2_deleted = mastodon_v2_deleted.map_err(|error| error.to_string())?;
        let rust_v2_deleted = rust_v2_deleted.map_err(|error| error.to_string())?;
        compare_responses(
            &mastodon_v2_deleted,
            &rust_v2_deleted,
            &[
                http::header::CONTENT_TYPE,
                http::header::CACHE_CONTROL,
                http::header::VARY,
            ],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("media v2 cleanup: {error}"))?;

        let invalid_upload = multipart_missing_file_body();
        let mut media_limiter_triggered = false;
        for _ in 0..=30 {
            let request = media_request(
                Method::POST,
                "/api/v1/media/",
                "fixture-bearer-token-v4-6-5",
                &invalid_upload,
            )?;
            let responses = send_identically(&targets, &request).await?;
            if responses.mastodon.status != responses.rust.status {
                return Err(format!(
                    "media upload rate-limit status mismatch: Mastodon={}, Rust={}",
                    responses.mastodon.status, responses.rust.status
                )
                .into());
            }
            if responses.mastodon.status == 429 {
                media_limiter_triggered = true;
                break;
            }
        }
        if !media_limiter_triggered {
            return Err("media upload rate limiter did not trigger within 31 requests".into());
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    let _ = cleanup_media_rows(mastodon_owner.url()).await;
    let _ = cleanup_media_rows(rust_writer.url()).await;
    let mastodon_rows_after = media_rows(mastodon_owner.url(), MEDIA_ACCOUNT_ID).await?;
    let rust_rows_after = media_rows(rust_writer.url(), MEDIA_ACCOUNT_ID).await?;
    let mastodon_media_after = MediaSnapshot::capture(&config.mastodon_media)?;
    let rust_media_after = MediaSnapshot::capture(&config.rust_media)?;
    operation?;
    if mastodon_rows_after != mastodon_rows_before || rust_rows_after != rust_rows_before {
        return Err("media database state did not roll back to its baseline".into());
    }
    compare_media_with_labels(
        &mastodon_media_before,
        &mastodon_media_after,
        "Mastodon before",
        "Mastodon after",
        DEFAULT_MISMATCH_LIMIT,
    )?;
    compare_media_with_labels(
        &rust_media_before,
        &rust_media_after,
        "Rust before",
        "Rust after",
        DEFAULT_MISMATCH_LIMIT,
    )?;
    compare_media(
        &mastodon_media_after,
        &rust_media_after,
        DEFAULT_MISMATCH_LIMIT,
    )?;
    Ok(())
}

fn media_request(
    method: Method,
    path: &str,
    token: &str,
    body: &[u8],
) -> Result<RequestSpec, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}"))?,
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    if body.is_empty() {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    } else if method == Method::POST {
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("multipart/form-data; boundary=rustodon-media-boundary"),
        );
    } else {
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
    }
    Ok(RequestSpec::new(
        method,
        path,
        None,
        headers,
        body.to_owned(),
    )?)
}

fn multipart_image_body(image: &[u8]) -> Vec<u8> {
    let mut body = b"--rustodon-media-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"attachment.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n".to_vec();
    body.extend_from_slice(image);
    body.extend_from_slice(b"\r\n--rustodon-media-boundary--\r\n");
    body
}

fn multipart_missing_file_body() -> Vec<u8> {
    b"--rustodon-media-boundary\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\nmissing file\r\n--rustodon-media-boundary--\r\n".to_vec()
}

fn normalize_media_response(
    response: &mut CapturedResponse,
    side: &str,
) -> Result<i64, Box<dyn Error>> {
    let mut document: Value = serde_json::from_slice(&response.body)?;
    let id = document
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "{side} media response has no string id (status {}, body {})",
                response.status,
                String::from_utf8_lossy(&response.body)
            )
        })?
        .parse::<i64>()?;
    if id <= 0 {
        return Err(format!("{side} media response id is not positive").into());
    }
    document["id"] = Value::String("<generated-media-id>".to_owned());
    for (field, style) in [("url", "original"), ("preview_url", "small")] {
        let value = document
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{side} media {field} is missing or empty"))?;
        document[field] = Value::String(normalize_media_url(value, field, style, side)?);
    }
    if let Some(value) = document.get("text_url").and_then(Value::as_str) {
        if value.is_empty() {
            return Err(format!("{side} media text_url is empty").into());
        }
        document["text_url"] = Value::String(normalize_media_url(value, "text_url", "", side)?);
    }
    response.body = serde_json::to_vec(&document)?;
    Ok(id)
}

fn normalize_media_url(
    value: &str,
    field: &str,
    style: &str,
    side: &str,
) -> Result<String, Box<dyn Error>> {
    let mut url = Url::parse(value)
        .map_err(|error| format!("{side} media {field} is not an absolute URL: {error}"))?;
    if url.host_str().is_none() || url.query().is_some() || url.fragment().is_some() {
        return Err(format!("{side} media {field} has an invalid URL shape").into());
    }
    let trailing_slash = url.path().ends_with('/');
    let mut segments = url
        .path_segments()
        .ok_or_else(|| format!("{side} media {field} has no URL path"))?
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if field == "text_url" {
        let media = segments
            .iter()
            .position(|segment| segment == "media")
            .filter(|position| segments.len() == position + 2)
            .ok_or_else(|| format!("{side} media text_url has an invalid path"))?;
        if segments[media + 1].parse::<i64>().is_err() {
            return Err(format!("{side} media text_url has a nonnumeric id").into());
        }
        "<generated-media-id>".clone_into(&mut segments[media + 1]);
    } else {
        let files = segments
            .windows(2)
            .position(|pair| pair == ["media_attachments", "files"])
            .ok_or_else(|| format!("{side} media {field} has an invalid Paperclip path"))?
            + 1;
        let style_position = files + 7;
        if segments.len() != files + 9
            || !segments[files + 1..style_position]
                .iter()
                .all(|segment| segment.parse::<u32>().is_ok())
            || segments[style_position] != style
            || segments[style_position + 1].is_empty()
        {
            return Err(format!("{side} media {field} has an invalid Paperclip path").into());
        }
        for segment in &mut segments[files + 1..style_position] {
            "<generated-media-partition>".clone_into(segment);
        }
        "<generated-media-file>".clone_into(&mut segments[style_position + 1]);
    }
    let mut path = format!("/{}", segments.join("/"));
    if trailing_slash {
        path.push('/');
    }
    url.set_path(&path);
    Ok(url.to_string())
}

fn compare_media_rows(mastodon: &MediaState, rust: &MediaState) -> Result<(), Box<dyn Error>> {
    // libvips and the Rust image encoder produce different bytes; compare the
    // stable metadata while still requiring both sides to persist an artifact.
    if mastodon.account_id != rust.account_id
        || mastodon.status_id != rust.status_id
        || mastodon.media_type != rust.media_type
        || mastodon.processing != rust.processing
        || mastodon.description != rust.description
        || mastodon.remote_url != rust.remote_url
        || mastodon.file_content_type != rust.file_content_type
        || mastodon.file_meta != rust.file_meta
        || mastodon.file_storage_schema_version != rust.file_storage_schema_version
    {
        return Err(
            format!("media database rows differ: Mastodon={mastodon:?}, Rust={rust:?}").into(),
        );
    }
    if mastodon.file_file_size.is_none_or(|size| size <= 0)
        || rust.file_file_size.is_none_or(|size| size <= 0)
        || !valid_media_blurhash(mastodon.blurhash.as_deref())
        || !valid_media_blurhash(rust.blurhash.as_deref())
    {
        return Err("media database rows contain an invalid blurhash".into());
    }
    Ok(())
}

fn valid_media_blurhash(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        value.len() == 36
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'#' | b'$'
                            | b'%'
                            | b'*'
                            | b'+'
                            | b','
                            | b'-'
                            | b'.'
                            | b':'
                            | b';'
                            | b'='
                            | b'?'
                            | b'@'
                            | b'['
                            | b']'
                            | b'^'
                            | b'_'
                            | b'{'
                            | b'|'
                            | b'}'
                            | b'~'
                    )
            })
    })
}

fn assert_new_media_files(
    before: &MediaSnapshot,
    after: &MediaSnapshot,
    side: &str,
) -> Result<(), Box<dyn Error>> {
    let added = after
        .files
        .keys()
        .filter(|path| !before.files.contains_key(*path))
        .collect::<Vec<_>>();
    if added.len() < 2
        || !added.iter().any(|path| path.contains("/original/"))
        || !added.iter().any(|path| path.contains("/small/"))
    {
        return Err(
            format!("{side} media create did not produce readable original/small files").into(),
        );
    }
    Ok(())
}

async fn media_rows(url: &str, account_id: i64) -> Result<Vec<MediaState>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_as(
        "SELECT id, account_id, status_id, type AS media_type, processing, description,
                remote_url, file_content_type, file_file_name, file_file_size, file_meta,
                file_storage_schema_version, blurhash
         FROM public.media_attachments WHERE account_id = $1 ORDER BY id",
    )
    .bind(account_id)
    .fetch_all(&mut connection)
    .await
}

async fn media_state(url: &str, id: i64) -> Result<MediaState, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_as(
        "SELECT id, account_id, status_id, type AS media_type, processing, description,
                remote_url, file_content_type, file_file_name, file_file_size, file_meta,
                file_storage_schema_version, blurhash
         FROM public.media_attachments WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut connection)
    .await
}

async fn cleanup_media_rows(url: &str) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query("DELETE FROM public.media_attachments WHERE account_id = $1 AND status_id IS NULL")
        .bind(MEDIA_ACCOUNT_ID)
        .execute(&mut connection)
        .await
        .map(|_| ())
}

fn profile_headers(token: &str) -> Result<HeaderMap, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}"))?,
    );
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    Ok(headers)
}

fn account_profile_stable_state(state: &AccountProfileState) -> StableAccountProfileState<'_> {
    (
        &state.display_name,
        &state.note,
        state.actor_type.as_deref(),
        state.locked,
        state.discoverable,
        state.hide_collections,
        state.indexable,
        state.attribution_domains.as_ref(),
        state.fields.as_ref(),
        state
            .user_settings
            .as_deref()
            .and_then(|settings| serde_json::from_str(settings).ok()),
    )
}

async fn account_profile_state(url: &str) -> Result<AccountProfileState, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_as(
        "SELECT account.display_name, account.note, account.actor_type, account.locked, \
                account.discoverable, account.hide_collections, account.indexable, \
                account.attribution_domains, account.fields, account.updated_at, \
                account_user.settings AS user_settings, account_user.updated_at AS user_updated_at \
         FROM public.accounts account \
         JOIN public.users account_user ON account_user.account_id = account.id \
         WHERE account.id = $1 ORDER BY account_user.id LIMIT 1",
    )
    .bind(INTERACTION_ACCOUNT_ID)
    .fetch_one(&mut connection)
    .await
}

async fn account_media_state(url: &str, account_id: i64) -> Result<AccountMediaState, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_as(
        "SELECT avatar_content_type, avatar_description, avatar_file_name, avatar_file_size, \
                avatar_remote_url, avatar_storage_schema_version, avatar_updated_at, \
                header_content_type, header_description, header_file_name, header_file_size, \
                header_remote_url, header_storage_schema_version, header_updated_at, updated_at \
         FROM public.accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_one(&mut connection)
    .await
}

async fn restore_account_media_state(
    url: &str,
    account_id: i64,
    state: &AccountMediaState,
) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query(
        "UPDATE public.accounts SET \
            avatar_content_type = $1, avatar_description = $2, avatar_file_name = $3, \
            avatar_file_size = $4, avatar_remote_url = $5, \
            avatar_storage_schema_version = $6, avatar_updated_at = $7, \
            header_content_type = $8, header_description = $9, header_file_name = $10, \
            header_file_size = $11, header_remote_url = $12, \
            header_storage_schema_version = $13, header_updated_at = $14, \
            updated_at = $15 WHERE id = $16",
    )
    .bind(&state.avatar_content_type)
    .bind(&state.avatar_description)
    .bind(&state.avatar_file_name)
    .bind(state.avatar_file_size)
    .bind(&state.avatar_remote_url)
    .bind(state.avatar_storage_schema_version)
    .bind(state.avatar_updated_at)
    .bind(&state.header_content_type)
    .bind(&state.header_description)
    .bind(&state.header_file_name)
    .bind(state.header_file_size)
    .bind(&state.header_remote_url)
    .bind(state.header_storage_schema_version)
    .bind(state.header_updated_at)
    .bind(state.updated_at)
    .bind(account_id)
    .execute(&mut connection)
    .await
    .map(|_| ())
}

fn assert_browser_profile_media_upload(
    before: &MediaSnapshot,
    after: &MediaSnapshot,
    state: &AccountMediaState,
) -> Result<(), Box<dyn Error>> {
    if state.avatar_content_type.as_deref() != Some("image/jpeg")
        || state.avatar_file_name.is_none()
        || state.avatar_file_size.is_none_or(|size| size <= 0)
        || state.avatar_storage_schema_version != Some(1)
        || state.avatar_description != "Browser avatar description"
        || state.header_content_type.as_deref() != Some("image/jpeg")
        || state.header_file_name.is_none()
        || state.header_file_size.is_none_or(|size| size <= 0)
        || state.header_storage_schema_version != Some(1)
        || state.header_description != "Browser header description"
    {
        return Err("browser profile media metadata did not persist both uploads".into());
    }
    let added = after
        .files
        .keys()
        .filter(|path| !before.files.contains_key(*path))
        .collect::<Vec<_>>();
    if added.len() != 2
        || !added
            .iter()
            .any(|path| path.starts_with("accounts/avatars/") && path.contains("/original/"))
        || !added
            .iter()
            .any(|path| path.starts_with("accounts/headers/") && path.contains("/original/"))
    {
        return Err(format!(
            "browser profile media upload produced unexpected Paperclip files: {added:?}"
        )
        .into());
    }
    Ok(())
}

fn remove_added_media_files(
    root: &Path,
    before: &MediaSnapshot,
    after: &MediaSnapshot,
) -> Result<(), Box<dyn Error>> {
    let root = PaperclipRoot::open(root)?;
    for path in after
        .files
        .keys()
        .filter(|path| !before.files.contains_key(*path))
    {
        root.remove_file(Path::new(path))?;
    }
    Ok(())
}

async fn restore_account_profile_state(
    url: &str,
    state: &AccountProfileState,
) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "UPDATE public.accounts SET display_name = $1, note = $2, actor_type = $3, \
            locked = $4, discoverable = $5, hide_collections = $6, indexable = $7, \
            attribution_domains = $8, fields = $9, updated_at = $10 WHERE id = $11",
    )
    .bind(&state.display_name)
    .bind(&state.note)
    .bind(&state.actor_type)
    .bind(state.locked)
    .bind(state.discoverable)
    .bind(state.hide_collections)
    .bind(state.indexable)
    .bind(&state.attribution_domains)
    .bind(&state.fields)
    .bind(state.updated_at)
    .bind(INTERACTION_ACCOUNT_ID)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE public.users SET settings = $1, updated_at = $2 \
         WHERE account_id = $3",
    )
    .bind(&state.user_settings)
    .bind(state.user_updated_at)
    .bind(INTERACTION_ACCOUNT_ID)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await
}

async fn account_tag_rows(url: &str) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_as(
        "SELECT account_id, tag_id FROM public.accounts_tags WHERE account_id = $1 ORDER BY tag_id",
    )
    .bind(INTERACTION_ACCOUNT_ID)
    .fetch_all(&mut connection)
    .await
}

async fn restore_account_tag_rows(url: &str, rows: &[(i64, i64)]) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM public.accounts_tags WHERE account_id = $1")
        .bind(INTERACTION_ACCOUNT_ID)
        .execute(&mut *transaction)
        .await?;
    for (account_id, tag_id) in rows {
        sqlx::query("INSERT INTO public.accounts_tags (account_id, tag_id) VALUES ($1, $2)")
            .bind(account_id)
            .bind(tag_id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await
}

async fn report_count(url: &str) -> Result<i64, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar("SELECT count(*) FROM public.reports")
        .fetch_one(&mut connection)
        .await
}

#[allow(clippy::type_complexity)]
async fn report_state(url: &str, report_id: i64) -> Result<ReportState, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let (
        account_id,
        target_account_id,
        application_id,
        category,
        comment,
        forwarded,
        rule_ids,
        status_ids,
        uri,
    ): (
        i64,
        i64,
        Option<i64>,
        i32,
        String,
        Option<bool>,
        Option<Vec<i64>>,
        Vec<i64>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT account_id, target_account_id, application_id, category, comment, forwarded, \
                rule_ids, status_ids, uri \
         FROM public.reports WHERE id = $1",
    )
    .bind(report_id)
    .fetch_one(&mut connection)
    .await?;
    let collection_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT COALESCE(array_agg(collection_id ORDER BY id), ARRAY[]::bigint[]) \
         FROM public.collection_reports WHERE report_id = $1",
    )
    .bind(report_id)
    .fetch_one(&mut connection)
    .await?;
    if !uri
        .as_deref()
        .is_some_and(|value| value.starts_with("https://fixture-v4-6-5.rustodon.invalid/"))
    {
        return Err(sqlx::Error::Protocol(format!(
            "report URI does not use the fixture payload format: {uri:?}"
        )));
    }
    Ok(ReportState {
        account_id,
        target_account_id,
        application_id,
        category,
        comment,
        forwarded,
        rule_ids,
        status_ids,
        collection_ids,
    })
}

async fn delete_report(url: &str, report_id: i64) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM public.collection_reports WHERE report_id = $1")
        .bind(report_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM public.reports WHERE id = $1")
        .bind(report_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await
}

fn normalize_generated_reblog_response(
    mastodon: &mut CapturedResponse,
    rust: &mut CapturedResponse,
) -> Result<(), String> {
    let mut mastodon_json: Value = serde_json::from_slice(&mastodon.body)
        .map_err(|error| format!("Mastodon response is not JSON: {error}"))?;
    let mut rust_json: Value = serde_json::from_slice(&rust.body)
        .map_err(|error| format!("Rust response is not JSON: {error}"))?;
    normalize_generated_reblog_document(&mut mastodon_json, "Mastodon")?;
    normalize_generated_reblog_document(&mut rust_json, "Rust")?;
    mastodon.body = serde_json::to_vec(&mastodon_json).map_err(|error| error.to_string())?;
    rust.body = serde_json::to_vec(&rust_json).map_err(|error| error.to_string())?;
    Ok(())
}

fn normalize_concurrent_reblog_remove_response(
    mastodon: &mut CapturedResponse,
    rust: &mut CapturedResponse,
) -> Result<(), String> {
    for (side, response) in [("Mastodon", mastodon), ("Rust", rust)] {
        let mut document: Value = serde_json::from_slice(&response.body)
            .map_err(|error| format!("{side} response is not JSON: {error}"))?;
        let object = document
            .as_object_mut()
            .ok_or_else(|| format!("{side} response is not a JSON object"))?;
        // Rails can serialize either the discarded-status or no-op branch while
        // two unreblog requests race; the persisted state below is authoritative.
        object.remove("pinned");
        object.remove("reblogs_count");
        if let Some(account) = object.get_mut("account").and_then(Value::as_object_mut) {
            account.remove("statuses_count");
        }
        if let Some(quote) = object.get_mut("quote").and_then(Value::as_object_mut) {
            quote.remove("state");
        }
        response.body = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn normalize_generated_reblog_document(document: &mut Value, side: &str) -> Result<(), String> {
    let id = document
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{side} generated reblog id is missing or not a string"))?
        .to_owned();
    if !matches!(id.parse::<i64>(), Ok(value) if value > 0) {
        return Err(format!(
            "{side} generated reblog id is not a positive decimal"
        ));
    }
    let created_at = document
        .get("created_at")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{side} generated reblog timestamp is missing or not a string"))?;
    DateTime::parse_from_rfc3339(created_at)
        .map_err(|error| format!("{side} generated reblog timestamp is invalid: {error}"))?;
    for field in ["uri", "url"] {
        let value = document
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{side} generated reblog {field} is missing or not a string"))?;
        let suffix = format!("/{id}/activity");
        if !value.ends_with(&suffix) {
            return Err(format!(
                "{side} generated reblog {field} has an unexpected shape"
            ));
        }
        let prefix = value
            .strip_suffix(&suffix)
            .expect("the suffix was validated immediately above");
        document[field] = Value::String(format!("{prefix}/<generated-status-id>/activity"));
    }
    document["id"] = Value::String("<generated-status-id>".to_owned());
    document["created_at"] = Value::String("<generated-status-timestamp>".to_owned());
    Ok(())
}

fn normalize_generated_status_response(
    response: &mut CapturedResponse,
    side: &str,
) -> Result<i64, String> {
    let mut document: Value = serde_json::from_slice(&response.body)
        .map_err(|error| format!("{side} status response is not JSON: {error}"))?;
    let id = document
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "{side} generated status id is missing or not a string: {}",
                String::from_utf8_lossy(&response.body)
            )
        })?
        .parse::<i64>()
        .map_err(|error| format!("{side} generated status id is not decimal: {error}"))?;
    if id <= 0 {
        return Err(format!("{side} generated status id is not positive"));
    }
    let created_at = document
        .get("created_at")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{side} generated status timestamp is missing"))?;
    DateTime::parse_from_rfc3339(created_at)
        .map_err(|error| format!("{side} generated status timestamp is invalid: {error}"))?;
    for field in ["uri", "url"] {
        let value = document
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{side} generated status {field} is missing"))?;
        let suffix = format!("/{id}");
        if !value.ends_with(&suffix) {
            return Err(format!(
                "{side} generated status {field} has an unexpected shape"
            ));
        }
        let prefix = value
            .strip_suffix(&suffix)
            .expect("the suffix was validated immediately above");
        document[field] = Value::String(format!("{prefix}/<generated-status-id>"));
    }
    document["id"] = Value::String("<generated-status-id>".to_owned());
    document["created_at"] = Value::String("<generated-status-timestamp>".to_owned());
    if let Some(edited_at) = document.get("edited_at").and_then(Value::as_str) {
        DateTime::parse_from_rfc3339(edited_at).map_err(|error| {
            format!("{side} generated status edit timestamp is invalid: {error}")
        })?;
        document["edited_at"] = Value::String("<generated-edited-timestamp>".to_owned());
    }
    response.body = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
    Ok(id)
}

fn normalize_generated_report_response(
    response: &mut CapturedResponse,
    side: &str,
) -> Result<i64, String> {
    let mut document: Value = serde_json::from_slice(&response.body)
        .map_err(|error| format!("{side} report response is not JSON: {error}"))?;
    let id = document
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "{side} generated report id is missing or not a string (status={}, body={})",
                response.status,
                String::from_utf8_lossy(&response.body)
            )
        })?
        .parse::<i64>()
        .map_err(|error| format!("{side} generated report id is not decimal: {error}"))?;
    if id <= 0 {
        return Err(format!("{side} generated report id is not positive"));
    }
    let created_at = document
        .get("created_at")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{side} generated report timestamp is missing"))?;
    DateTime::parse_from_rfc3339(created_at)
        .map_err(|error| format!("{side} generated report timestamp is invalid: {error}"))?;
    document["id"] = Value::String("<generated-report-id>".to_owned());
    document["created_at"] = Value::String("<generated-report-timestamp>".to_owned());
    response.body = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
    Ok(id)
}

async fn set_status_default_state(
    url: &str,
    locked: bool,
    settings: &str,
) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("UPDATE public.accounts SET locked = $1 WHERE id = $2")
        .bind(locked)
        .bind(INTERACTION_ACCOUNT_ID)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE public.users SET settings = $1 WHERE account_id = $2")
        .bind(settings)
        .bind(INTERACTION_ACCOUNT_ID)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await
}

async fn status_quote_state(url: &str, status_id: i64) -> Result<(i32, i32), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_as("SELECT visibility, quote_approval_policy FROM public.statuses WHERE id = $1")
        .bind(status_id)
        .fetch_one(&mut connection)
        .await
}

fn created_status_id(body: &[u8]) -> Result<i64, Box<dyn Error>> {
    let value: Value = serde_json::from_slice(body)?;
    value
        .get("id")
        .and_then(Value::as_str)
        .ok_or("status response did not contain an id")?
        .parse::<i64>()
        .map_err(|error| format!("status response id was not decimal: {error}").into())
}

fn browser_profile_upload_request(
    cookie: &str,
    body: &[u8],
) -> Result<RequestSpec, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("text/html"));
    headers.insert(COOKIE, HeaderValue::from_str(cookie)?);
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("multipart/form-data; boundary=rustodon-profile-media-boundary"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    Ok(RequestSpec::new(
        Method::POST,
        "/settings/profile",
        None,
        headers,
        body.to_owned(),
    )?)
}

fn browser_profile_multipart_body(csrf_token: &str, image: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in [
        ("csrf_token", csrf_token),
        ("avatar_description", "Browser avatar description"),
        ("header_description", "Browser header description"),
    ] {
        body.extend_from_slice(
            format!(
                "--{PROFILE_MEDIA_BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    for (name, file_name) in [
        ("avatar", "browser-avatar.jpg"),
        ("header", "browser-header.jpg"),
    ] {
        body.extend_from_slice(
            format!(
                "--{PROFILE_MEDIA_BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: image/jpeg\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(image);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{PROFILE_MEDIA_BOUNDARY}--\r\n").as_bytes());
    body
}

fn status_request(method: Method, path: &str, body: &str) -> Result<RequestSpec, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    if !body.is_empty() {
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
    }
    Ok(RequestSpec::new(
        method,
        path,
        None,
        headers,
        body.as_bytes().to_owned(),
    )?)
}

fn report_request(body: &str) -> Result<RequestSpec, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    Ok(RequestSpec::new(
        Method::POST,
        "/api/v1/reports",
        None,
        headers,
        body.as_bytes().to_owned(),
    )?)
}

async fn notification_rows(url: &str, table: &str) -> Result<Vec<Value>, sqlx::Error> {
    notification_rows_for_account(url, table, NOTIFICATION_ACCOUNT_ID).await
}

async fn notification_rows_for_account(
    url: &str,
    table: &str,
    account_id: i64,
) -> Result<Vec<Value>, sqlx::Error> {
    let query = match table {
        "notifications" => {
            "SELECT to_jsonb(notification) FROM public.notifications notification \
             WHERE account_id = $1 ORDER BY id"
        }
        "notification_requests" => {
            "SELECT to_jsonb(request) FROM public.notification_requests request \
             WHERE account_id = $1 ORDER BY id"
        }
        "account_conversations" => {
            "SELECT to_jsonb(conversation) FROM public.account_conversations conversation \
             WHERE account_id = $1 ORDER BY id"
        }
        _ => unreachable!("notification write snapshots use fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(query)
        .bind(account_id)
        .fetch_all(&mut connection)
        .await
}

async fn notification_permission_rows(url: &str) -> Result<Vec<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT to_jsonb(permission) FROM public.notification_permissions permission \
         WHERE account_id = $1 ORDER BY id",
    )
    .bind(NOTIFICATION_ACCOUNT_ID)
    .fetch_all(&mut connection)
    .await
}

async fn notification_policy_rows(url: &str) -> Result<Vec<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT to_jsonb(policy) FROM public.notification_policies policy \
         WHERE account_id = $1 ORDER BY id",
    )
    .bind(NOTIFICATION_ACCOUNT_ID)
    .fetch_all(&mut connection)
    .await
}

async fn oauth_access_token_is_revoked(url: &str, token: &str) -> Result<bool, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT revoked_at IS NOT NULL FROM public.oauth_access_tokens WHERE token = $1",
    )
    .bind(token)
    .fetch_one(&mut connection)
    .await
}

async fn oauth_application_max_id(url: &str) -> Result<i64, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM public.oauth_applications")
        .fetch_one(&mut connection)
        .await
}

async fn browser_session_access_token_ids(url: &str) -> Result<Vec<i64>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT access_token_id FROM public.session_activations \
         WHERE user_id = 101 AND access_token_id IS NOT NULL",
    )
    .fetch_all(&mut connection)
    .await
}

async fn remove_oauth_access_tokens(url: &str, ids: &[i64]) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query("DELETE FROM public.oauth_access_tokens WHERE id = ANY($1)")
        .bind(ids)
        .execute(&mut connection)
        .await?;
    Ok(())
}

async fn browser_user_rows(url: &str, table: &str) -> Result<Vec<Value>, sqlx::Error> {
    let query = match table {
        "login_activities" => {
            "SELECT to_jsonb(row) FROM (SELECT * FROM public.login_activities WHERE user_id = 101 ORDER BY id) row"
        }
        "session_activations" => {
            "SELECT to_jsonb(row) FROM (SELECT * FROM public.session_activations WHERE user_id = 101 ORDER BY session_id) row"
        }
        _ => unreachable!("browser snapshots use fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(query).fetch_all(&mut connection).await
}

async fn restore_browser_user_rows(
    url: &str,
    table: &str,
    rows: &[Value],
) -> Result<(), sqlx::Error> {
    let (delete, insert) = match table {
        "login_activities" => (
            "DELETE FROM public.login_activities WHERE user_id = 101",
            "INSERT INTO public.login_activities SELECT * FROM jsonb_populate_record(NULL::public.login_activities, $1)",
        ),
        "session_activations" => (
            "DELETE FROM public.session_activations WHERE user_id = 101",
            "INSERT INTO public.session_activations SELECT * FROM jsonb_populate_record(NULL::public.session_activations, $1)",
        ),
        _ => unreachable!("browser snapshots use fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query(delete).execute(&mut *transaction).await?;
    for row in rows {
        sqlx::query(insert)
            .bind(row)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await
}

fn notification_permission_pairs(rows: &[Value]) -> Vec<(i64, i64)> {
    let mut pairs = rows
        .iter()
        .filter_map(|row| {
            Some((
                row.get("account_id")?.as_i64()?,
                row.get("from_account_id")?.as_i64()?,
            ))
        })
        .collect::<Vec<_>>();
    pairs.sort_unstable();
    pairs
}

async fn restore_rows(url: &str, table: &str, rows: &[Value]) -> Result<(), sqlx::Error> {
    restore_rows_for_account(url, table, NOTIFICATION_ACCOUNT_ID, rows).await
}

async fn restore_rows_for_account(
    url: &str,
    table: &str,
    account_id: i64,
    rows: &[Value],
) -> Result<(), sqlx::Error> {
    let (delete, insert) = match table {
        "notifications" => (
            "DELETE FROM public.notifications WHERE account_id = $1",
            "INSERT INTO public.notifications SELECT * FROM jsonb_populate_record(NULL::public.notifications, $1)",
        ),
        "notification_requests" => (
            "DELETE FROM public.notification_requests WHERE account_id = $1",
            "INSERT INTO public.notification_requests SELECT * FROM jsonb_populate_record(NULL::public.notification_requests, $1)",
        ),
        "account_conversations" => (
            "DELETE FROM public.account_conversations WHERE account_id = $1",
            "INSERT INTO public.account_conversations SELECT * FROM jsonb_populate_record(NULL::public.account_conversations, $1)",
        ),
        _ => unreachable!("notification write snapshots use fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query(delete)
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
    for row in rows {
        sqlx::query(insert)
            .bind(row)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await
}

async fn restore_notification_permissions(url: &str, rows: &[Value]) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM public.notification_permissions WHERE account_id = $1")
        .bind(NOTIFICATION_ACCOUNT_ID)
        .execute(&mut *transaction)
        .await?;
    for row in rows {
        sqlx::query(
            "INSERT INTO public.notification_permissions \
             SELECT * FROM jsonb_populate_record(NULL::public.notification_permissions, $1)",
        )
        .bind(row)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}

async fn restore_notification_policies(url: &str, rows: &[Value]) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM public.notification_policies WHERE account_id = $1")
        .bind(NOTIFICATION_ACCOUNT_ID)
        .execute(&mut *transaction)
        .await?;
    for row in rows {
        sqlx::query(
            "INSERT INTO public.notification_policies \
             SELECT * FROM jsonb_populate_record(NULL::public.notification_policies, $1)",
        )
        .bind(row)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}

async fn remove_oauth_applications_after(url: &str, max_id: i64) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query("DELETE FROM public.oauth_applications WHERE id > $1")
        .bind(max_id)
        .execute(&mut connection)
        .await
        .map(|_| ())
}

async fn relationship_rows(url: &str, table: &str) -> Result<Vec<Value>, sqlx::Error> {
    let query = match table {
        "follows" => {
            "SELECT to_jsonb(row) FROM public.follows row WHERE account_id = $1 ORDER BY id"
        }
        "follow_requests" => {
            "SELECT to_jsonb(row) FROM public.follow_requests row WHERE account_id = $1 ORDER BY id"
        }
        "blocks" => "SELECT to_jsonb(row) FROM public.blocks row WHERE account_id = $1 ORDER BY id",
        "mutes" => "SELECT to_jsonb(row) FROM public.mutes row WHERE account_id = $1 ORDER BY id",
        _ => unreachable!("relationship write snapshots use fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(query)
        .bind(INTERACTION_ACCOUNT_ID)
        .fetch_all(&mut connection)
        .await
}

async fn notification_policy_state(url: &str) -> Result<Vec<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT to_jsonb(policy) - 'created_at' - 'updated_at' \
         FROM public.notification_policies policy \
         WHERE account_id = $1 ORDER BY id",
    )
    .bind(NOTIFICATION_ACCOUNT_ID)
    .fetch_all(&mut connection)
    .await
}

async fn relationship_target_rows(
    url: &str,
    table: &str,
    target_account_id: i64,
) -> Result<Vec<Value>, sqlx::Error> {
    let query = match table {
        "follows" => {
            "SELECT to_jsonb(row) FROM public.follows row \
             WHERE target_account_id = $1 ORDER BY id"
        }
        "follow_requests" => {
            "SELECT to_jsonb(row) FROM public.follow_requests row \
             WHERE target_account_id = $1 ORDER BY id"
        }
        _ => unreachable!("incoming relationship snapshots use fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(query)
        .bind(target_account_id)
        .fetch_all(&mut connection)
        .await
}

async fn restore_relationship_rows(
    url: &str,
    table: &str,
    rows: &[Value],
) -> Result<(), sqlx::Error> {
    let (delete, insert) = match table {
        "follows" => (
            "DELETE FROM public.follows WHERE account_id = $1",
            "INSERT INTO public.follows SELECT * FROM jsonb_populate_record(NULL::public.follows, $1)",
        ),
        "follow_requests" => (
            "DELETE FROM public.follow_requests WHERE account_id = $1",
            "INSERT INTO public.follow_requests SELECT * FROM jsonb_populate_record(NULL::public.follow_requests, $1)",
        ),
        "blocks" => (
            "DELETE FROM public.blocks WHERE account_id = $1",
            "INSERT INTO public.blocks SELECT * FROM jsonb_populate_record(NULL::public.blocks, $1)",
        ),
        "mutes" => (
            "DELETE FROM public.mutes WHERE account_id = $1",
            "INSERT INTO public.mutes SELECT * FROM jsonb_populate_record(NULL::public.mutes, $1)",
        ),
        _ => unreachable!("relationship write snapshots use fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query(delete)
        .bind(INTERACTION_ACCOUNT_ID)
        .execute(&mut *transaction)
        .await?;
    for row in rows {
        sqlx::query(insert)
            .bind(row)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await
}

async fn restore_relationship_target_rows(
    url: &str,
    table: &str,
    target_account_id: i64,
    rows: &[Value],
) -> Result<(), sqlx::Error> {
    let (delete, insert) = match table {
        "follows" => (
            "DELETE FROM public.follows WHERE target_account_id = $1",
            "INSERT INTO public.follows SELECT * FROM jsonb_populate_record(NULL::public.follows, $1)",
        ),
        "follow_requests" => (
            "DELETE FROM public.follow_requests WHERE target_account_id = $1",
            "INSERT INTO public.follow_requests SELECT * FROM jsonb_populate_record(NULL::public.follow_requests, $1)",
        ),
        _ => unreachable!("incoming relationship snapshots use fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query(delete)
        .bind(target_account_id)
        .execute(&mut *transaction)
        .await?;
    for row in rows {
        sqlx::query(insert)
            .bind(row)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await
}

async fn restore_created_statuses(url: &str, status_ids: &[i64]) -> Result<(), sqlx::Error> {
    if status_ids.is_empty() {
        return Ok(());
    }
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "DELETE FROM public.notifications WHERE activity_type = 'Mention' \
         AND activity_id IN (SELECT id FROM public.mentions WHERE status_id = ANY($1))",
    )
    .bind(status_ids)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM public.notification_requests WHERE last_status_id = ANY($1)")
        .bind(status_ids)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM public.mentions WHERE status_id = ANY($1)")
        .bind(status_ids)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM public.status_stats WHERE status_id = ANY($1)")
        .bind(status_ids)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM public.statuses WHERE id = ANY($1)")
        .bind(status_ids)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM public.conversations WHERE parent_status_id = ANY($1)")
        .bind(status_ids)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await
}

async fn prepare_status_media(url: &str) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    for media_id in [STATUS_MEDIA_ID, STATUS_DELETE_MEDIA_ID] {
        sqlx::query(
            "INSERT INTO public.media_attachments \
             (id, account_id, type, processing, remote_url, file_content_type, \
              file_file_name, file_file_size, file_meta, file_storage_schema_version, \
              file_updated_at, blurhash, created_at, updated_at) \
             VALUES ($1, $2, 0, 2, '', 'image/jpeg', 'cd63911ad76f4d5d.jpg', 36381, \
                     '{\"original\":{\"width\":600,\"height\":400,\"size\":\"600x400\"}, \
                       \"small\":{\"width\":588,\"height\":392,\"size\":\"588x392\"}}', 1, \
                     clock_timestamp(), \
                     'UDKw:zyZ.9xs?KKQocn#0;-;%1i^Rk-.IVIU', clock_timestamp(), clock_timestamp())",
        )
        .bind(media_id)
        .bind(MEDIA_ACCOUNT_ID)
        .execute(&mut connection)
        .await?;
    }
    Ok(())
}

async fn cleanup_status_media(url: &str) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    for media_id in [STATUS_MEDIA_ID, STATUS_DELETE_MEDIA_ID] {
        sqlx::query("DELETE FROM public.media_attachments WHERE id = $1")
            .bind(media_id)
            .execute(&mut connection)
            .await?;
    }
    Ok(())
}

async fn edited_status_state(
    url: &str,
    status_id: i64,
) -> Result<DifferentialEditedStatusFields, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_as(
        "SELECT text, spoiler_text, sensitive, language, ordered_media_attachment_ids, \
                edited_at IS NOT NULL, \
                (SELECT count(*) FROM public.status_edits edit WHERE edit.status_id = status.id) \
         FROM public.statuses status WHERE status.id = $1",
    )
    .bind(status_id)
    .fetch_one(&mut connection)
    .await
}

async fn featured_tag_state(
    url: &str,
) -> Result<(i64, Option<NaiveDateTime>, NaiveDateTime), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_as(
        "SELECT statuses_count, last_status_at, updated_at FROM public.featured_tags \
         WHERE id = 9202",
    )
    .fetch_one(&mut connection)
    .await
}

async fn restore_featured_tag(
    url: &str,
    state: &(i64, Option<NaiveDateTime>, NaiveDateTime),
) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query(
        "UPDATE public.featured_tags SET statuses_count = $1, last_status_at = $2, \
         updated_at = $3 WHERE id = 9202",
    )
    .bind(state.0)
    .bind(state.1)
    .bind(state.2)
    .execute(&mut connection)
    .await
    .map(|_| ())
}

async fn marker_state(url: &str) -> Result<MarkerState, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_as(
        "SELECT last_read_id, lock_version, updated_at FROM markers \
         WHERE user_id = $1 AND timeline = $2",
    )
    .bind(MARKER_USER_ID)
    .bind(MARKER_TIMELINE)
    .fetch_one(&mut connection)
    .await
}

async fn marker_count(url: &str) -> Result<i64, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar("SELECT count(*) FROM markers")
        .fetch_one(&mut connection)
        .await
}

async fn update_marker_like_rails(url: &str, last_read_id: i64) -> Result<(i64, i32), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    let marker = sqlx::query_as(
        "UPDATE markers SET last_read_id = $1, lock_version = lock_version + 1, \
         updated_at = clock_timestamp() \
         WHERE user_id = $2 AND timeline = $3 \
         RETURNING last_read_id, lock_version",
    )
    .bind(last_read_id)
    .bind(MARKER_USER_ID)
    .bind(MARKER_TIMELINE)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(marker)
}

async fn restore_marker(url: &str, state: &MarkerState) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query(
        "UPDATE markers SET last_read_id = $1, lock_version = $2, updated_at = $3 \
         WHERE user_id = $4 AND timeline = $5",
    )
    .bind(state.last_read_id)
    .bind(state.lock_version)
    .bind(state.updated_at)
    .bind(MARKER_USER_ID)
    .bind(MARKER_TIMELINE)
    .execute(&mut connection)
    .await?;
    Ok(())
}

async fn interaction_rows(url: &str, table: &str) -> Result<Vec<Value>, sqlx::Error> {
    let query = match table {
        "bookmarks" => {
            "SELECT to_jsonb(row) FROM public.bookmarks row WHERE account_id = $1 ORDER BY id"
        }
        "favourites" => {
            "SELECT to_jsonb(row) FROM public.favourites row WHERE account_id = $1 ORDER BY id"
        }
        _ => unreachable!("status interaction snapshots use fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(query)
        .bind(INTERACTION_ACCOUNT_ID)
        .fetch_all(&mut connection)
        .await
}

async fn interaction_status_rows(
    url: &str,
    target_status_id: i64,
) -> Result<Vec<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT to_jsonb(status) FROM public.statuses status \
         WHERE account_id = $1 AND reblog_of_id = $2 ORDER BY id",
    )
    .bind(INTERACTION_ACCOUNT_ID)
    .bind(target_status_id)
    .fetch_all(&mut connection)
    .await
}

async fn author_block_state(url: &str) -> Result<Option<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT to_jsonb(block) FROM public.blocks block \
         WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(BLOCKED_REBLOG_AUTHOR_ACCOUNT_ID)
    .bind(INTERACTION_ACCOUNT_ID)
    .fetch_optional(&mut connection)
    .await
}

async fn set_author_block(url: &str, blocked: bool) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    if blocked {
        sqlx::query(
            "INSERT INTO public.blocks (account_id, target_account_id, uri, created_at, updated_at) \
             VALUES ($1, $2, NULL, clock_timestamp(), clock_timestamp()) \
             ON CONFLICT (account_id, target_account_id) DO NOTHING",
        )
        .bind(BLOCKED_REBLOG_AUTHOR_ACCOUNT_ID)
        .bind(INTERACTION_ACCOUNT_ID)
        .execute(&mut connection)
        .await?;
    } else {
        sqlx::query("DELETE FROM public.blocks WHERE account_id = $1 AND target_account_id = $2")
            .bind(BLOCKED_REBLOG_AUTHOR_ACCOUNT_ID)
            .bind(INTERACTION_ACCOUNT_ID)
            .execute(&mut connection)
            .await?;
    }
    Ok(())
}

async fn restore_author_block(url: &str, row: Option<&Value>) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM public.blocks WHERE account_id = $1 AND target_account_id = $2")
        .bind(BLOCKED_REBLOG_AUTHOR_ACCOUNT_ID)
        .bind(INTERACTION_ACCOUNT_ID)
        .execute(&mut *transaction)
        .await?;
    if let Some(row) = row {
        sqlx::query(
            "INSERT INTO public.blocks \
             SELECT * FROM jsonb_populate_record(NULL::public.blocks, $1)",
        )
        .bind(row)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}

async fn interaction_conversation_rows(
    url: &str,
    target_status_id: i64,
) -> Result<Vec<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT to_jsonb(conversation) FROM public.conversations conversation \
         WHERE parent_status_id IN ( \
           SELECT id FROM public.statuses \
           WHERE account_id = $1 AND reblog_of_id = $2) \
         ORDER BY id",
    )
    .bind(INTERACTION_ACCOUNT_ID)
    .bind(target_status_id)
    .fetch_all(&mut connection)
    .await
}

async fn interaction_conversation_mutes(url: &str) -> Result<Vec<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT to_jsonb(mute) FROM public.conversation_mutes mute \
         WHERE account_id = $1 ORDER BY conversation_id",
    )
    .bind(INTERACTION_ACCOUNT_ID)
    .fetch_all(&mut connection)
    .await
}

async fn interaction_status_pins(url: &str) -> Result<Vec<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT to_jsonb(pin) FROM public.status_pins pin \
         WHERE account_id = $1 ORDER BY id",
    )
    .bind(INTERACTION_ACCOUNT_ID)
    .fetch_all(&mut connection)
    .await
}

async fn restore_interaction_rows(
    url: &str,
    table: &str,
    rows: &[Value],
) -> Result<(), sqlx::Error> {
    let (delete, insert) = match table {
        "bookmarks" => (
            "DELETE FROM public.bookmarks WHERE account_id = $1",
            "INSERT INTO public.bookmarks SELECT * FROM jsonb_populate_record(NULL::public.bookmarks, $1)",
        ),
        "favourites" => (
            "DELETE FROM public.favourites WHERE account_id = $1",
            "INSERT INTO public.favourites SELECT * FROM jsonb_populate_record(NULL::public.favourites, $1)",
        ),
        _ => unreachable!("status interaction snapshots use fixed tables"),
    };
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query(delete)
        .bind(INTERACTION_ACCOUNT_ID)
        .execute(&mut *transaction)
        .await?;
    for row in rows {
        sqlx::query(insert)
            .bind(row)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await
}

async fn restore_interaction_status_rows(
    url: &str,
    target_status_id: i64,
    rows: &[Value],
) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM public.statuses WHERE account_id = $1 AND reblog_of_id = $2")
        .bind(INTERACTION_ACCOUNT_ID)
        .bind(target_status_id)
        .execute(&mut *transaction)
        .await?;
    for row in rows {
        sqlx::query(
            "INSERT INTO public.statuses \
             SELECT * FROM jsonb_populate_record(NULL::public.statuses, $1)",
        )
        .bind(row)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}

async fn restore_interaction_conversation_rows(
    url: &str,
    target_status_id: i64,
    rows: &[Value],
) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "DELETE FROM public.conversations \
         WHERE parent_status_id IN ( \
           SELECT id FROM public.statuses \
           WHERE account_id = $1 AND reblog_of_id = $2)",
    )
    .bind(INTERACTION_ACCOUNT_ID)
    .bind(target_status_id)
    .execute(&mut *transaction)
    .await?;
    for row in rows {
        sqlx::query(
            "INSERT INTO public.conversations \
             SELECT * FROM jsonb_populate_record(NULL::public.conversations, $1)",
        )
        .bind(row)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}

async fn restore_interaction_conversation_mutes(
    url: &str,
    rows: &[Value],
) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM public.conversation_mutes WHERE account_id = $1")
        .bind(INTERACTION_ACCOUNT_ID)
        .execute(&mut *transaction)
        .await?;
    for row in rows {
        sqlx::query(
            "INSERT INTO public.conversation_mutes \
             SELECT * FROM jsonb_populate_record(NULL::public.conversation_mutes, $1)",
        )
        .bind(row)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}

async fn restore_interaction_status_pins(url: &str, rows: &[Value]) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM public.status_pins WHERE account_id = $1")
        .bind(INTERACTION_ACCOUNT_ID)
        .execute(&mut *transaction)
        .await?;
    for row in rows {
        sqlx::query(
            "INSERT INTO public.status_pins \
             SELECT * FROM jsonb_populate_record(NULL::public.status_pins, $1)",
        )
        .bind(row)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}

async fn interaction_stat(url: &str, status_id: i64) -> Result<Option<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar("SELECT to_jsonb(row) FROM public.status_stats row WHERE status_id = $1")
        .bind(status_id)
        .fetch_optional(&mut connection)
        .await
}

async fn interaction_status_pin_state(url: &str) -> Result<Vec<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT jsonb_build_object('account_id', pin.account_id, 'status_id', pin.status_id) \
         FROM public.status_pins pin WHERE account_id = $1 ORDER BY status_id",
    )
    .bind(INTERACTION_ACCOUNT_ID)
    .fetch_all(&mut connection)
    .await
}

async fn interaction_account_stat(
    url: &str,
    account_id: i64,
) -> Result<Option<Value>, sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    sqlx::query_scalar(
        "SELECT to_jsonb(stats) FROM public.account_stats stats WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_optional(&mut connection)
    .await
}

async fn restore_interaction_stat(
    url: &str,
    status_id: i64,
    row: Option<&Value>,
) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM public.status_stats WHERE status_id = $1")
        .bind(status_id)
        .execute(&mut *transaction)
        .await?;
    if let Some(row) = row {
        sqlx::query(
            "INSERT INTO public.status_stats \
             SELECT * FROM jsonb_populate_record(NULL::public.status_stats, $1)",
        )
        .bind(row)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}

async fn restore_interaction_account_stat(
    url: &str,
    account_id: i64,
    row: Option<&Value>,
) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(url).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM public.account_stats WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut *transaction)
        .await?;
    if let Some(row) = row {
        sqlx::query(
            "INSERT INTO public.account_stats \
             SELECT * FROM jsonb_populate_record(NULL::public.account_stats, $1)",
        )
        .bind(row)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await
}
