use std::error::Error;
use std::sync::Arc;

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use http::HeaderMap;
use http::header::{AUTHORIZATION, HeaderValue};
use rustodon::jobs::{
    ACCOUNT_DELETION_DELAY_DAYS, ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND,
    ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND, ACTIVITYPUB_DELIVERY_JOB_KIND,
    ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND, JobSpec, Lane, MASTODON_ACCOUNT_PURGE_JOB_KIND,
    MASTODON_DOMAIN_BLOCK_JOB_KIND, NOTIFICATION_CREATE_JOB_KIND, Queue,
};
use rustodon::mastodon::rest::{
    AccountListKind, AccountListOptions, AccountStatusesOptions, FollowCollectionKind,
    FollowCollectionOptions, RestProjectionLoader, RestSerializer, SavedStatusKind,
    SavedStatusesOptions, TagTimelineOptions, TimelineOptions,
};
use rustodon::mastodon::{
    AccountFieldUpdate, AccountIdScheme, AccountKind, AccountMediaUpdate, AccountProfileUpdate,
    AccountProfileValue, AccountSourceUpdate, BearerAuthenticator, IdempotencyKey,
    InvalidTokenReason, MediaAttachmentCreate, MediaAttachmentUpdate, MediaFocus,
    NotificationActivity, NotificationCreate, NotificationCreateOutcome, NotificationType,
    OAuthAuthenticationError, OAuthError, READ_ACCOUNTS, READ_STATUSES, Repository,
    StatusMediaAttributeUpdate, StatusUpdate, StatusVisibility, WRITE_ACCOUNTS, WRITE_BOOKMARKS,
    WRITE_CONVERSATIONS, WRITE_FAVOURITES, WRITE_MEDIA, WRITE_REPORTS, WRITE_STATUSES, WriteError,
    WriteRepository,
};
use rustodon::operational_schema::migrate;
use rustodon::paperclip::PaperclipAttachment;
use rustodon::streaming::STREAM_EVENT_KIND;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use tokio::sync::Barrier;
use url::Url;

const ALICE: i64 = 116_844_606_259_201_001;
const MODERATOR: i64 = 116_844_606_259_201_002;
const NEWBIE: i64 = 116_844_606_259_201_003;
const BOB: i64 = 116_844_606_259_202_001;
const CAROL: i64 = 116_844_606_259_202_002;
const API_MODERATOR: i64 = 116_844_606_259_201_004;
const MATRIX_VIEWER: i64 = -323;
const REMOTE_AP_ACCOUNT: i64 = -331;
const PUBLIC_STATUS: i64 = 116_844_842_188_805_001;
const UNLISTED_STATUS: i64 = 116_844_846_120_965_002;
const PRIVATE_STATUS: i64 = 116_844_850_053_125_003;
const DIRECT_STATUS: i64 = 116_844_853_985_285_004;
const LIMITED_STATUS: i64 = 116_844_857_917_445_005;
const DELETED_UNKNOWN_STATUS: i64 = 116_846_257_766_400_501;

type AccountProfileSchemaState = (
    String,
    String,
    Option<String>,
    bool,
    Option<bool>,
    Option<bool>,
    bool,
    Option<Vec<String>>,
    Option<Value>,
    NaiveDateTime,
);

type AccountMediaSchemaState = (
    Option<String>,
    String,
    Option<String>,
    Option<i32>,
    Option<String>,
    Option<i32>,
    Option<NaiveDateTime>,
    Option<String>,
    String,
    Option<String>,
    Option<i32>,
    String,
    Option<i32>,
    Option<NaiveDateTime>,
);

fn database_url() -> String {
    std::env::var("RUSTODON_MASTODON_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_DATABASE_URL")
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn resolves_activitypub_signature_keys_with_ownership_metadata() -> Result<(), Box<dyn Error>>
{
    let repository = Repository::connect(&database_url()).await?;
    let origin = "https://fixture-v4-6-5.rustodon.invalid/";

    let local = repository
        .activitypub_signature_key(
            "https://fixture-v4-6-5.rustodon.invalid/users/alice#main-key",
            origin,
        )
        .await?
        .expect("the local legacy actor key should resolve");
    assert_eq!(local.account_id, ALICE);
    assert!(!local.revoked);
    assert!(local.expires_at.is_none());
    assert!(local.public_key.contains("BEGIN PUBLIC KEY"));

    let alias = repository
        .activitypub_signature_key("acct:alice@fixture-v4-6-5.rustodon.invalid", origin)
        .await?
        .expect("the local acct alias should resolve");
    assert_eq!(alias.account_id, ALICE);
    assert_eq!(alias.key_id, "acct:alice@fixture-v4-6-5.rustodon.invalid");

    let remote = repository
        .activitypub_signature_key(
            "https://remote.fixture.invalid/users/bob#secondary-key",
            origin,
        )
        .await?
        .expect("the persisted remote keypair should resolve");
    assert_eq!(remote.account_id, BOB);
    assert!(!remote.revoked);
    assert!(remote.expires_at.is_none());

    let revoked = repository
        .activitypub_signature_key(
            "https://fixture-v4-6-5.rustodon.invalid/users/alice#opaque-key",
            origin,
        )
        .await?
        .expect("revoked key metadata should remain visible to the verifier");
    assert_eq!(revoked.account_id, ALICE);
    assert!(revoked.revoked);
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn reads_every_mapped_mastodon_4_6_5_record_losslessly() -> sqlx::Result<()> {
    let repository = Repository::connect(&database_url()).await?;

    let instance = repository.account(-99).await?.unwrap();
    assert_eq!(instance.id, -99);
    assert_eq!(instance.id.to_string(), "-99");
    assert_eq!(instance.kind(), AccountKind::LocalService);
    assert!(!instance.has_user);
    assert!(!instance.login_capable_user);
    assert!(instance.private_key.as_ref().unwrap().is_present());
    assert!(!instance.public_key.is_empty());
    assert_eq!(instance.attribution_domains, None);
    assert_eq!(
        repository.account(ALICE).await?.unwrap().kind(),
        AccountKind::LocalLogin
    );
    assert_eq!(
        repository.account(ALICE).await?.unwrap().id_scheme,
        Some(AccountIdScheme::Username)
    );
    assert_eq!(
        repository
            .account(116_844_606_259_201_002)
            .await?
            .unwrap()
            .kind(),
        AccountKind::LocalUnavailable
    );
    assert_eq!(
        repository.account(NEWBIE).await?.unwrap().kind(),
        AccountKind::LocalUnavailable
    );
    let bob = repository.account(BOB).await?.unwrap();
    assert_eq!(bob.kind(), AccountKind::Remote);
    assert_eq!(bob.id_scheme, Some(AccountIdScheme::Numeric));
    assert_eq!(
        bob.also_known_as.as_deref(),
        Some(&["https://alias.remote.fixture.invalid/users/bob".to_owned()][..])
    );
    assert_eq!(
        bob.fields.as_ref().unwrap()[0]["value"],
        "Exact JSONB value"
    );
    assert_eq!(instance.also_known_as, None);
    assert_eq!(
        bob.attribution_domains.as_deref(),
        Some(&["media.remote.fixture.invalid".to_owned()][..])
    );
    let alice_account = repository.account(ALICE).await?.unwrap();
    assert_eq!(
        alice_account.avatar_file_name.as_deref(),
        Some("0112603425bb49c1.png")
    );
    assert_eq!(alice_account.inbox_url, "");
    assert!(alice_account.suspended_at.is_none());
    let avatar = repository
        .paperclip_metadata(PaperclipAttachment::AccountAvatar, ALICE)
        .await?
        .unwrap();
    assert!(!avatar.remote);
    assert_eq!(avatar.file_name, "0112603425bb49c1.png");
    let remote_avatar = repository
        .paperclip_metadata(PaperclipAttachment::AccountAvatar, BOB)
        .await?
        .unwrap();
    assert!(remote_avatar.remote);
    assert_eq!(remote_avatar.storage_schema_version, Some(1));
    let cached_media = repository
        .paperclip_metadata(PaperclipAttachment::MediaFile, 116_845_105_643_526_106)
        .await?
        .unwrap();
    assert!(cached_media.remote);
    assert_eq!(cached_media.file_name, "cached.jpg");
    let emoji = repository
        .paperclip_metadata(PaperclipAttachment::CustomEmojiImage, 12_001)
        .await?
        .unwrap();
    assert_eq!(emoji.file_name, "fixtureparty.png");
    let card = repository
        .paperclip_metadata(PaperclipAttachment::PreviewCardImage, 12_002)
        .await?
        .unwrap();
    assert!(card.remote);
    assert_eq!(card.file_name, "preview.png");
    assert!(repository.account_stat(-99).await?.is_some());

    let alice = repository.user(101).await?.unwrap();
    assert_eq!(alice.sign_up_ip.unwrap().to_string(), "192.0.2.0/24");
    assert_eq!(
        alice.settings.as_ref().unwrap().parse().unwrap()["default_privacy"],
        "private"
    );
    assert_eq!(
        alice.settings.as_ref().unwrap().parse().unwrap()["nested"]["number"].as_i64(),
        Some(9_007_199_254_740_993)
    );
    assert_eq!(
        alice.chosen_languages.as_deref(),
        Some(&["en".to_owned()][..])
    );
    assert_eq!(alice.otp_backup_codes.as_ref().unwrap().len(), 1);
    assert!(alice.otp_required_for_login);
    assert!(alice.has_webauthn_credentials);
    assert_eq!(
        alice.webauthn_id.as_deref(),
        Some("fixture-alice-webauthn-id")
    );
    let recovery_codes = format!("{:?}", alice.otp_backup_codes);
    assert!(!recovery_codes.contains("fixture-recovery-code"));
    assert!(recovery_codes.contains("REDACTED"));
    assert!(alice.confirmed_at.is_some());
    let moderator = repository.user(102).await?.unwrap();
    assert!(!moderator.otp_required_for_login);
    assert!(!moderator.has_webauthn_credentials);
    assert_eq!(moderator.chosen_languages, Some(Vec::new()));
    assert_eq!(moderator.otp_backup_codes, Some(Vec::new()));
    assert!(moderator.settings.is_none());
    assert_eq!(
        repository.user(103).await?.unwrap().settings.unwrap().raw(),
        ""
    );
    let everyone = repository.user_role(-99).await?.unwrap();
    assert_eq!(everyone.id, -99);
    assert_eq!(everyone.permissions.0, 1_152_921_504_606_912_512);

    assert!(
        repository
            .oauth_application(301)
            .await?
            .unwrap()
            .secret
            .is_present()
    );
    let token = repository
        .oauth_access_token("fixture-bearer-token-v4-6-5")
        .await?
        .unwrap();
    assert_eq!(token.id, 401);
    assert!(token.token.is_present());
    assert_eq!(token.created_at.to_string(), "2026-07-01 12:00:00");
    assert_eq!(token.last_used_ip.unwrap().to_string(), "192.0.2.10/32");

    for (id, visibility) in [
        (116_844_842_188_805_001, StatusVisibility::Public),
        (116_844_846_120_965_002, StatusVisibility::Unlisted),
        (116_844_850_053_125_003, StatusVisibility::Private),
        (116_844_853_985_285_004, StatusVisibility::Direct),
        (116_844_857_917_445_005, StatusVisibility::Limited),
    ] {
        let status = repository.status(id).await?.unwrap();
        assert_eq!(status.visibility, visibility);
        assert_eq!(status.id.to_string().parse::<i64>().unwrap(), id);
    }
    let public_status = repository.status(PUBLIC_STATUS).await?.unwrap();
    assert_eq!(public_status.application_id, Some(301));
    assert_eq!(public_status.quote_approval_policy.0, 2);
    let counters = repository.status_stat(PUBLIC_STATUS).await?.unwrap();
    assert_eq!(counters.id, 12001);
    assert_eq!(counters.reblogs_count, 1);
    let direct_status = repository.status(116_844_853_985_285_004).await?.unwrap();
    assert_eq!(direct_status.in_reply_to_account_id, Some(BOB));
    assert_eq!(direct_status.in_reply_to_id, Some(116_845_078_118_405_101));
    assert!(repository.status(DELETED_UNKNOWN_STATUS).await?.is_none());
    let deleted = repository
        .status_including_deleted(DELETED_UNKNOWN_STATUS)
        .await?
        .unwrap();
    assert_eq!(deleted.visibility, StatusVisibility::Unknown(99));
    assert!(deleted.deleted_at.is_some());
    assert_eq!(deleted.ordered_media_attachment_ids, Some(Vec::new()));
    assert!(
        repository
            .status_stat(DELETED_UNKNOWN_STATUS)
            .await?
            .is_none()
    );
    assert!(
        repository
            .status_edits(DELETED_UNKNOWN_STATUS)
            .await?
            .is_empty()
    );
    assert!(
        repository
            .media_attachments(DELETED_UNKNOWN_STATUS)
            .await?
            .is_empty()
    );
    assert!(
        repository
            .mentions(DELETED_UNKNOWN_STATUS)
            .await?
            .is_empty()
    );
    assert!(repository.tags(DELETED_UNKNOWN_STATUS).await?.is_empty());
    assert!(
        repository
            .status_tags(DELETED_UNKNOWN_STATUS)
            .await?
            .is_empty()
    );
    assert_eq!(
        repository
            .status_edits(116_845_105_643_525_105)
            .await?
            .len(),
        1
    );
    let public_edits = repository.status_edits(PUBLIC_STATUS).await?;
    assert_eq!(public_edits.len(), 1);
    assert_eq!(
        public_edits[0].media_descriptions,
        Some(vec![
            None,
            Some("Deterministic Mastodon test attachment".to_owned())
        ])
    );
    let media = repository.media_attachments(PUBLIC_STATUS).await?;
    assert_eq!(
        media
            .iter()
            .map(|attachment| attachment.id)
            .collect::<Vec<_>>(),
        vec![-101, 116_844_842_188_806_001, -102, -103]
    );
    let processed_media = media
        .iter()
        .find(|attachment| attachment.id == 116_844_842_188_806_001)
        .unwrap();
    assert_eq!(
        processed_media.file_file_name.as_deref(),
        Some("cd63911ad76f4d5d.jpg")
    );
    assert_eq!(
        processed_media.file_content_type.as_deref(),
        Some("image/jpeg")
    );
    assert_eq!(processed_media.file_file_size, Some(36_381));
    assert_eq!(
        processed_media.file_meta.as_ref().unwrap()["small"]["height"],
        392
    );
    assert_eq!(processed_media.thumbnail_file_size, None);
    assert_eq!(
        repository
            .media_attachments(116_844_846_120_965_002)
            .await?
            .iter()
            .map(|attachment| attachment.id)
            .collect::<Vec<_>>(),
        vec![-210, -209, -208, -207]
    );
    assert_eq!(repository.mentions(116_845_078_118_405_101).await?.len(), 1);
    assert_eq!(repository.tags(PUBLIC_STATUS).await?.len(), 1);
    assert_eq!(repository.status_tags(PUBLIC_STATUS).await?.len(), 1);

    assert!(repository.conversation(9301).await?.is_some());
    let account_conversations = repository.account_conversations(ALICE).await?;
    assert_eq!(account_conversations.len(), 1);
    assert_eq!(
        account_conversations[0].status_ids,
        vec![116_844_853_985_285_004]
    );
    assert_eq!(repository.conversation_mutes(ALICE).await?.len(), 1);
    assert_eq!(repository.follows(ALICE).await?.len(), 5);
    assert_eq!(repository.follow_requests(ALICE).await?.len(), 1);
    assert_eq!(repository.favourites(BOB).await?.len(), 1);
    assert_eq!(repository.bookmarks(ALICE).await?.len(), 3);
    assert_eq!(repository.blocks(ALICE).await?.len(), 1);
    assert_eq!(repository.mutes(ALICE).await?.len(), 1);
    assert_eq!(repository.account_domain_blocks(ALICE).await?.len(), 1);
    assert_eq!(repository.lists(ALICE).await?.len(), 3);
    assert_eq!(repository.list_accounts(9001).await?.len(), 5);
    assert_eq!(repository.status_pins(ALICE).await?.len(), 2);
    assert_eq!(repository.featured_tags(ALICE).await?.len(), 1);
    assert_eq!(repository.account_tags(ALICE).await?.len(), 1);

    assert_eq!(repository.custom_filters(ALICE).await?.len(), 1);
    assert_eq!(repository.custom_filter_keywords(9101).await?.len(), 1);
    assert_eq!(repository.custom_filter_statuses(9101).await?.len(), 3);

    let notifications = repository.notifications(ALICE).await?;
    let moderator_notifications = repository.notifications(116_844_606_259_201_002).await?;
    let api_moderator_notifications = repository.notifications(116_844_606_259_201_004).await?;
    assert_eq!(notifications.len() + moderator_notifications.len(), 19);
    assert_eq!(api_moderator_notifications.len(), 40);
    assert!(api_moderator_notifications.iter().all(|notification| {
        notification.notification_type == Some(NotificationType::Follow)
            && notification.group_key.as_deref() == Some("follow-api-moderator-stress")
    }));
    assert!(
        notifications
            .iter()
            .all(|notification| !notification.filtered)
    );
    let all_notifications = repository.notifications_including_filtered(ALICE).await?;
    assert_eq!(all_notifications.len(), 20);
    assert!(all_notifications.iter().any(|notification| {
        notification.id == 10025
            && notification.notification_type.is_none()
            && notification.activity_type.0 == "Status"
    }));
    assert!(all_notifications.iter().any(|notification| {
        notification.filtered
            && notification.notification_type
                == Some(NotificationType::Unknown("future_event".to_owned()))
    }));
    assert!(all_notifications.iter().any(|notification| {
        notification.id == 10019 && notification.notification_type.is_none()
    }));
    assert!(
        all_notifications
            .iter()
            .all(|notification| notification.id != 10021)
    );
    assert!(all_notifications.iter().any(|notification| {
        notification.id == 10020
            && notification.notification_type
                == Some(NotificationType::Unknown(
                    "future_deleted_status".to_owned(),
                ))
    }));
    let notification_policy = repository.notification_policy(ALICE).await?.unwrap();
    assert_eq!(notification_policy.for_bots.0, 0);
    assert_eq!(notification_policy.for_limited_accounts.0, 1);
    assert_eq!(notification_policy.for_new_accounts.0, 2);
    assert_eq!(notification_policy.for_not_followers.0, 0);
    assert_eq!(notification_policy.for_not_following.0, 99);
    assert_eq!(notification_policy.for_private_mentions.0, 1);
    assert_eq!(repository.notification_policy_summary(ALICE).await?, (2, 2));
    assert_eq!(repository.notification_permissions(ALICE).await?.len(), 1);
    let notification_requests = repository.notification_requests(ALICE).await?;
    assert_eq!(notification_requests.len(), 2);
    assert!(
        notification_requests
            .iter()
            .all(|request| request.id != -95)
    );
    assert!(
        notification_requests
            .iter()
            .find(|request| request.id == -96)
            .unwrap()
            .last_status_id
            .is_none()
    );

    assert_eq!(repository.domain_allows().await?.len(), 1);
    let domain_blocks = repository.domain_blocks().await?;
    assert_eq!(domain_blocks.len(), 1);
    assert_eq!(domain_blocks[0].severity.unwrap().0, 99);
    let settings = repository.settings().await?;
    assert_eq!(settings.len(), 6);
    let scalar = settings
        .iter()
        .find(|setting| setting.var == "fixture_scalar")
        .unwrap();
    assert!(
        scalar.value.as_ref().unwrap().parse().unwrap()[0]
            .as_bool()
            .unwrap()
    );
    assert_eq!(scalar.value.as_ref().unwrap().raw(), "--- true\n");
    let tagged = settings
        .iter()
        .find(|setting| setting.var == "fixture_tagged")
        .unwrap();
    let tagged_docs = tagged.value.as_ref().unwrap().parse().unwrap();
    assert_eq!(
        tagged_docs[0].get_tag().unwrap().to_string(),
        "!ruby/hash:ActiveSupport::HashWithIndifferentAccess"
    );
    assert_eq!(
        settings
            .iter()
            .find(|setting| setting.var == "remote_live_feed_access")
            .and_then(|setting| setting.value.as_ref())
            .unwrap()
            .raw(),
        "--- authenticated\n"
    );
    assert!(!repository.user_can_view_feeds(101, ALICE).await?);
    assert!(repository.user_can_view_feeds(104, API_MODERATOR).await?);
    assert!(!repository.user_can_view_feeds(104, ALICE).await?);
    assert!(!repository.user_can_view_feeds(101, API_MODERATOR).await?);
    let alice_loader = RestProjectionLoader::new(
        repository.clone(),
        Some(ALICE),
        "fixture-v4-6-5.rustodon.invalid",
    );
    assert!(alice_loader.credential_account(104, ALICE).await?.is_none());
    assert!(
        alice_loader
            .credential_account(101, API_MODERATOR)
            .await?
            .is_none()
    );

    let quotes = repository.quotes(ALICE).await?;
    assert_eq!(quotes.len(), 4);
    let quote = quotes
        .iter()
        .find(|quote| quote.id == 116_845_314_048_008_702)
        .unwrap();
    assert_eq!(quote.id, 116_845_314_048_008_702);
    assert_eq!(quote.quoted_status_id, Some(116_845_093_847_045_103));
    assert_eq!(
        quote.approval_uri.as_deref(),
        Some("https://remote.fixture.invalid/activities/accept-116845314048008702")
    );
    let deleted_target_quote = quotes.iter().find(|quote| quote.id == -94).unwrap();
    assert_eq!(deleted_target_quote.state.0, 4);
    assert_eq!(
        deleted_target_quote.quoted_status_id,
        Some(DELETED_UNKNOWN_STATUS)
    );
    assert!(quotes.iter().all(|quote| quote.id != -97));
    let collections = repository.collections(BOB).await?;
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].original_number_of_items, Some(1));
    let collection_items = repository.collection_items(116_845_549_977_608_801).await?;
    assert_eq!(collection_items.len(), 1);
    assert_eq!(collection_items[0].state.0, 1);
    assert_eq!(
        collection_items[0].object_uri.as_deref(),
        Some("https://fixture-v4-6-5.rustodon.invalid/users/alice")
    );
    assert!(repository.poll(8201).await?.is_some());
    assert_eq!(repository.poll_votes(8201).await?.len(), 1);
    assert!(repository.poll(8203).await?.is_none());
    assert!(repository.poll_votes(8203).await?.is_empty());
    let keypairs = repository.keypairs(ALICE).await?;
    assert_eq!(keypairs.len(), 1);
    assert!(keypairs[0].private_key.as_ref().unwrap().is_present());
    assert_eq!(keypairs[0].key_type.0, 0);
    assert!(keypairs[0].revoked);
    let remote_keypairs = repository.keypairs(BOB).await?;
    assert_eq!(remote_keypairs.len(), 1);
    assert!(remote_keypairs[0].private_key.is_none());
    assert_eq!(
        remote_keypairs[0].uri,
        "https://remote.fixture.invalid/users/bob#secondary-key"
    );
    assert_eq!(remote_keypairs[0].public_key, bob.public_key);
    assert_eq!(
        repository
            .relationship_severance_event(8301)
            .await?
            .unwrap()
            .event_type
            .0,
        0
    );
    assert_eq!(
        repository
            .account_relationship_severance_event(8302)
            .await?
            .unwrap()
            .following_count,
        1
    );
    assert_eq!(repository.account_warning(8401).await?.unwrap().action.0, 0);
    let annual_report = repository.generated_annual_report(8501).await?.unwrap();
    assert_eq!(annual_report.year, 2025);
    assert!(annual_report.share_key.as_ref().unwrap().is_present());
    let rendered_report = format!("{annual_report:?}");
    assert!(!rendered_report.contains("fixture-annual-report-share-key"));
    assert!(rendered_report.contains("REDACTED"));
    assert_eq!(repository.report(8601).await?.unwrap().category.0, 1000);
    assert!(repository.tombstone(9901).await?.is_some());

    Ok(())
}

fn bearer_headers(token: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static(token));
    headers
}

async fn oauth_failure(
    authenticator: &BearerAuthenticator,
    token: &'static str,
    required: rustodon::mastodon::RequiredScopes,
) -> OAuthError {
    match authenticator
        .authenticate(&bearer_headers(token), required)
        .await
        .expect_err("the fixture token should be rejected")
    {
        OAuthAuthenticationError::OAuth(error) => error,
        OAuthAuthenticationError::Repository(error) => {
            panic!("fixture OAuth lookup failed: {error}")
        }
    }
}

async fn oauth_user_failure(
    authenticator: &BearerAuthenticator,
    token: &'static str,
    required: rustodon::mastodon::RequiredScopes,
) -> OAuthError {
    authenticator
        .authenticate(&bearer_headers(token), required)
        .await
        .expect("owner state does not invalidate an optional-auth token")
        .require_user()
        .expect_err("the fixture owner should fail a user-required endpoint")
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn authenticates_fixture_oauth_tokens_without_mutating_rows() -> sqlx::Result<()> {
    let url = database_url();
    let mut connection = PgConnection::connect(&url).await?;
    let before = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(access_token) FROM oauth_access_tokens access_token ORDER BY id",
    )
    .fetch_all(&mut connection)
    .await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&url).await?);

    let broad = authenticator
        .authenticate(
            &bearer_headers("Bearer fixture-bearer-token-v4-6-5"),
            READ_STATUSES,
        )
        .await
        .expect("existing broad-scope token should authenticate");
    assert_eq!(broad.token_id(), 401);
    assert_eq!(broad.application_id(), Some(301));
    assert_eq!(broad.require_user().unwrap().user_id(), 101);
    assert!(broad.scopes().contains("read"));

    let granular_status = authenticator
        .authenticate(
            &bearer_headers("Bearer fixture-bearer-read-statuses-v4-6-5"),
            READ_STATUSES,
        )
        .await
        .expect("granular status token should authenticate");
    assert_eq!(granular_status.token_id(), 402);
    assert_eq!(granular_status.application_id(), Some(301));
    assert_eq!(granular_status.resource_owner().unwrap().user_id(), 101);
    let granular_account = authenticator
        .authenticate(
            &bearer_headers("Bearer fixture-bearer-read-accounts-v4-6-5"),
            READ_ACCOUNTS,
        )
        .await
        .expect("granular account token should authenticate");
    assert_eq!(granular_account.token_id(), 403);
    assert_eq!(granular_account.application_id(), Some(301));
    assert_eq!(granular_account.resource_owner().unwrap().user_id(), 101);

    assert_eq!(
        oauth_failure(
            &authenticator,
            "Bearer fixture-bearer-read-accounts-v4-6-5",
            READ_STATUSES,
        )
        .await,
        OAuthError::InsufficientScope(READ_STATUSES)
    );
    assert_eq!(
        oauth_failure(
            &authenticator,
            "Bearer fixture-bearer-unknown-v4-6-5",
            READ_ACCOUNTS,
        )
        .await,
        OAuthError::InvalidToken(InvalidTokenReason::Unknown)
    );
    assert_eq!(
        oauth_failure(
            &authenticator,
            "Bearer fixture-bearer-revoked-v4-6-5",
            READ_ACCOUNTS,
        )
        .await,
        OAuthError::InvalidToken(InvalidTokenReason::Revoked)
    );
    assert_eq!(
        oauth_failure(
            &authenticator,
            "Bearer fixture-bearer-expired-v4-6-5",
            READ_ACCOUNTS,
        )
        .await,
        OAuthError::InvalidToken(InvalidTokenReason::Expired)
    );
    assert_eq!(
        oauth_failure(
            &authenticator,
            "Bearer fixture-bearer-insufficient-v4-6-5",
            READ_ACCOUNTS,
        )
        .await,
        OAuthError::InsufficientScope(READ_ACCOUNTS)
    );
    assert_eq!(
        oauth_user_failure(
            &authenticator,
            "Bearer fixture-bearer-disabled-user-v4-6-5",
            READ_ACCOUNTS,
        )
        .await,
        OAuthError::UserDisabled
    );
    assert_eq!(
        oauth_user_failure(
            &authenticator,
            "Bearer fixture-bearer-missing-2fa-v4-6-5",
            READ_ACCOUNTS,
        )
        .await,
        OAuthError::UserDisabled
    );

    let application_only = authenticator
        .authenticate(
            &bearer_headers("Bearer fixture-bearer-application-only-v4-6-5"),
            READ_ACCOUNTS,
        )
        .await
        .expect("application-only token is valid until an endpoint requires a user");
    assert_eq!(application_only.application_id(), Some(301));
    assert_eq!(
        application_only.require_user(),
        Err(OAuthError::UserRequired)
    );

    let after = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(access_token) FROM oauth_access_tokens access_token ORDER BY id",
    )
    .fetch_all(&mut connection)
    .await?;
    assert_eq!(
        before, after,
        "OAuth authentication must not update token rows"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn loads_and_serializes_secret_free_rest_account_projections() -> sqlx::Result<()> {
    let url = database_url();
    let mut connection = PgConnection::connect(&url).await?;
    let before = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(account) FROM accounts account ORDER BY id",
    )
    .fetch_all(&mut connection)
    .await?;
    let repository = Repository::connect(&url).await?;
    let anonymous =
        RestProjectionLoader::new(repository.clone(), None, "fixture-v4-6-5.rustodon.invalid");
    let accounts = anonymous.accounts(&[ALICE, BOB]).await?;
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0].id, ALICE);
    assert_eq!(accounts[0].feature_current_user, "denied");
    assert_eq!(accounts[0].profile_mentions.len(), 3);
    assert!(
        accounts[0]
            .profile_mentions
            .iter()
            .any(|account| account.id == BOB)
    );
    assert!(
        accounts[0]
            .profile_mentions
            .iter()
            .any(|account| account.id == 116_844_606_259_201_002)
    );
    assert!(
        accounts[0]
            .profile_mentions
            .iter()
            .any(|account| account.id == 116_844_606_259_202_003)
    );
    assert_eq!(accounts[1].id, BOB);
    assert_eq!(accounts[1].fields.len(), 1);
    let local_quote = anonymous
        .authorized_status(116_845_314_048_005_201)
        .await?
        .expect("local quote fixture status should load");
    let quoted_status = local_quote
        .quote
        .as_ref()
        .and_then(|quote| quote.quoted_status.as_deref())
        .expect("accepted visible quote target should load");
    assert_eq!(quoted_status.id, 116_845_093_847_045_103);
    assert!(quoted_status.reblog.is_none());
    let private_quote = anonymous
        .authorized_status(116_845_078_118_405_101)
        .await?
        .expect("public quote fixture status should load");
    let private_quote = private_quote
        .quote
        .expect("public fixture status should retain its quote state");
    assert_eq!(
        private_quote.target_access,
        rustodon::mastodon::rest::QuoteTargetAccess::Unauthorized
    );
    assert!(private_quote.target_link.is_none());
    assert!(private_quote.quoted_status.is_none());
    let domain_quote_viewer = RestProjectionLoader::new(
        repository.clone(),
        Some(-320),
        "fixture-v4-6-5.rustodon.invalid",
    );
    let domain_blocked_quote = domain_quote_viewer
        .authorized_status(116_845_317_980_165_202)
        .await?
        .expect("public quote fixture status should load")
        .quote
        .expect("public fixture status should retain its quote state");
    assert_eq!(
        domain_blocked_quote.target_access,
        rustodon::mastodon::rest::QuoteTargetAccess::Unauthorized
    );
    assert!(domain_blocked_quote.target_link.is_none());
    assert!(domain_blocked_quote.quoted_status.is_none());

    for visible_id in [116_844_842_188_805_001, 116_844_846_120_965_002] {
        assert!(anonymous.authorized_status(visible_id).await?.is_some());
    }
    for hidden_id in [
        116_844_850_053_125_003,
        116_844_853_985_285_004,
        116_844_857_917_445_005,
        116_846_257_766_400_501,
    ] {
        assert!(anonymous.authorized_status(hidden_id).await?.is_none());
    }

    let owner = RestProjectionLoader::new(
        repository.clone(),
        Some(ALICE),
        "fixture-v4-6-5.rustodon.invalid",
    );
    let quote_page = owner
        .status_quotes(
            116_845_321_912_325_301,
            &FollowCollectionOptions {
                max_id: None,
                since_id: None,
                limit: 2,
            },
        )
        .await?
        .expect("quoted status should be visible to its owner");
    assert_eq!(
        quote_page
            .statuses
            .iter()
            .map(|status| status.id)
            .collect::<Vec<_>>(),
        vec![116_844_842_188_805_001]
    );
    for own_id in [
        116_844_850_053_125_003,
        116_844_853_985_285_004,
        116_844_857_917_445_005,
    ] {
        assert!(owner.authorized_status(own_id).await?.is_some());
    }

    let unrelated = RestProjectionLoader::new(
        repository.clone(),
        Some(116_844_606_259_201_004),
        "fixture-v4-6-5.rustodon.invalid",
    );
    assert!(
        unrelated
            .authorized_status(116_844_842_188_805_001)
            .await?
            .is_some()
    );
    assert!(
        unrelated
            .authorized_status(116_844_850_053_125_003)
            .await?
            .is_some()
    );

    let mentioned_follower = RestProjectionLoader::new(
        repository.clone(),
        Some(BOB),
        "fixture-v4-6-5.rustodon.invalid",
    );
    assert!(
        mentioned_follower
            .authorized_status(116_844_850_053_125_003)
            .await?
            .is_some()
    );
    assert!(
        mentioned_follower
            .authorized_status(116_844_853_985_285_004)
            .await?
            .is_some()
    );
    assert!(
        mentioned_follower
            .authorized_status(116_844_857_917_445_005)
            .await?
            .is_none()
    );
    assert!(
        unrelated
            .authorized_status(116_844_857_917_445_005)
            .await?
            .is_some(),
        "silent mentions authorize limited statuses"
    );
    assert!(
        owner.authorized_status(-311).await?.is_some(),
        "silent mentions survive loss of the follow relationship"
    );
    assert!(owner.authorized_status(-312).await?.is_none());
    assert!(
        unrelated.authorized_status(-313).await?.is_none(),
        "an author-side block hides public status show"
    );
    let domain_viewer = RestProjectionLoader::new(
        repository.clone(),
        Some(-320),
        "fixture-v4-6-5.rustodon.invalid",
    );
    assert!(
        domain_viewer
            .authorized_status(PUBLIC_STATUS)
            .await?
            .is_none(),
        "the author's user-domain block hides public status show"
    );
    assert!(anonymous.authorized_status(-310).await?.is_none());

    let ids = anonymous
        .account_statuses(ALICE, &AccountStatusesOptions::default())
        .await?
        .into_iter()
        .map(|status| status.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            116_845_314_048_005_201,
            116_844_846_120_965_002,
            116_844_842_188_805_001,
            -416,
        ]
    );
    let ids = owner
        .account_statuses(ALICE, &AccountStatusesOptions::default())
        .await?
        .into_iter()
        .map(|status| status.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            116_845_314_048_005_201,
            116_844_857_917_445_005,
            116_844_853_985_285_004,
            116_844_850_053_125_003,
            116_844_846_120_965_002,
            116_844_842_188_805_001,
            -416,
            -421,
        ]
    );
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let mut owner_connection = PgConnection::connect(&owner_url).await?;
    sqlx::query("UPDATE statuses SET deleted_at = clock_timestamp() WHERE id = $1")
        .bind(-417_i64)
        .execute(&mut owner_connection)
        .await?;
    let deleted_source_ids = owner
        .account_statuses(ALICE, &AccountStatusesOptions::default())
        .await?
        .into_iter()
        .map(|status| status.id)
        .collect::<Vec<_>>();
    sqlx::query("UPDATE statuses SET deleted_at = NULL WHERE id = $1")
        .bind(-417_i64)
        .execute(&mut owner_connection)
        .await?;
    assert!(!deleted_source_ids.contains(&-416));
    let ids = mentioned_follower
        .account_statuses(ALICE, &AccountStatusesOptions::default())
        .await?
        .into_iter()
        .map(|status| status.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            116_845_314_048_005_201,
            116_844_853_985_285_004,
            116_844_850_053_125_003,
            116_844_846_120_965_002,
            116_844_842_188_805_001,
            -416,
        ]
    );
    let ids = unrelated
        .account_statuses(ALICE, &AccountStatusesOptions::default())
        .await?
        .into_iter()
        .map(|status| status.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            116_845_314_048_005_201,
            116_844_857_917_445_005,
            116_844_853_985_285_004,
            116_844_850_053_125_003,
            116_844_846_120_965_002,
            116_844_842_188_805_001,
            -416,
        ]
    );
    let options = AccountStatusesOptions {
        exclude_direct: true,
        ..AccountStatusesOptions::default()
    };
    let ids = owner
        .account_statuses(ALICE, &options)
        .await?
        .into_iter()
        .map(|status| status.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            116_845_314_048_005_201,
            116_844_850_053_125_003,
            116_844_846_120_965_002,
            116_844_842_188_805_001,
            -416,
        ]
    );
    let options = AccountStatusesOptions {
        only_media: true,
        ..AccountStatusesOptions::default()
    };
    assert_eq!(
        anonymous
            .account_statuses(ALICE, &options)
            .await?
            .into_iter()
            .map(|status| status.id)
            .collect::<Vec<_>>(),
        vec![116_844_846_120_965_002, PUBLIC_STATUS]
    );
    let options = AccountStatusesOptions {
        exclude_replies: true,
        ..AccountStatusesOptions::default()
    };
    assert_eq!(
        owner
            .account_statuses(ALICE, &options)
            .await?
            .into_iter()
            .map(|status| status.id)
            .collect::<Vec<_>>(),
        vec![
            116_845_314_048_005_201,
            LIMITED_STATUS,
            PRIVATE_STATUS,
            UNLISTED_STATUS,
            PUBLIC_STATUS,
            -416,
            -421,
        ]
    );
    let options = AccountStatusesOptions {
        pinned: true,
        ..AccountStatusesOptions::default()
    };
    assert_eq!(
        anonymous
            .account_statuses(ALICE, &options)
            .await?
            .into_iter()
            .map(|status| status.id)
            .collect::<Vec<_>>(),
        vec![PUBLIC_STATUS, UNLISTED_STATUS]
    );
    let options = AccountStatusesOptions {
        min_id: Some(PUBLIC_STATUS),
        limit: 2,
        ..AccountStatusesOptions::default()
    };
    assert_eq!(
        owner
            .account_statuses(ALICE, &options)
            .await?
            .into_iter()
            .map(|status| status.id)
            .collect::<Vec<_>>(),
        vec![116_844_850_053_125_003, 116_844_846_120_965_002]
    );
    let followers = anonymous
        .follow_collection(
            ALICE,
            FollowCollectionKind::Followers,
            &FollowCollectionOptions::default(),
        )
        .await?;
    assert_eq!(followers.first_cursor, Some(8007));
    assert_eq!(followers.last_cursor, Some(8002));
    assert_eq!(
        followers
            .accounts
            .iter()
            .map(|account| account.id)
            .collect::<Vec<_>>(),
        vec![API_MODERATOR, 116_844_606_259_201_002, BOB]
    );
    let following = anonymous
        .follow_collection(
            ALICE,
            FollowCollectionKind::Following,
            &FollowCollectionOptions::default(),
        )
        .await?;
    assert_eq!(following.first_cursor, Some(8012));
    assert_eq!(following.last_cursor, Some(8001));
    assert_eq!(
        following
            .accounts
            .iter()
            .map(|account| account.id)
            .collect::<Vec<_>>(),
        vec![-332, -331, -330, 116_844_606_259_201_002, BOB]
    );
    let follower_page = anonymous
        .follow_collection(
            ALICE,
            FollowCollectionKind::Followers,
            &FollowCollectionOptions {
                max_id: Some(8006),
                since_id: None,
                limit: 1,
            },
        )
        .await?;
    assert_eq!(follower_page.first_cursor, Some(8002));
    assert_eq!(follower_page.last_cursor, Some(8002));
    assert_eq!(follower_page.accounts[0].id, BOB);
    let following_page = anonymous
        .follow_collection(
            ALICE,
            FollowCollectionKind::Following,
            &FollowCollectionOptions {
                max_id: None,
                since_id: Some(8001),
                limit: 1,
            },
        )
        .await?;
    assert_eq!(following_page.first_cursor, Some(8012));
    assert_eq!(following_page.last_cursor, Some(8012));
    assert_eq!(following_page.accounts[0].id, -332);
    let hidden = anonymous
        .follow_collection(
            116_844_606_259_202_003,
            FollowCollectionKind::Followers,
            &FollowCollectionOptions::default(),
        )
        .await?;
    assert!(hidden.accounts.is_empty());
    let context = anonymous
        .status_context(116_844_846_120_965_002)
        .await?
        .expect("public root context should be authorized");
    assert_eq!(
        context
            .ancestors
            .iter()
            .map(|status| status.id)
            .collect::<Vec<_>>(),
        vec![PUBLIC_STATUS]
    );
    assert!(context.descendants.is_empty());
    let context = owner
        .status_context(116_844_846_120_965_002)
        .await?
        .expect("owner context should be authorized");
    assert_eq!(
        context
            .descendants
            .iter()
            .map(|status| status.id)
            .collect::<Vec<_>>(),
        vec![116_844_850_053_125_003]
    );
    assert!(
        !context.descendants.iter().any(|status| status.id == -314),
        "viewer-domain-blocked members are filtered from context"
    );
    let private_context = owner
        .status_context(PRIVATE_STATUS)
        .await?
        .expect("owner should be authorized to load private context");
    assert_eq!(
        private_context
            .ancestors
            .iter()
            .map(|status| status.id)
            .collect::<Vec<_>>(),
        vec![PUBLIC_STATUS, UNLISTED_STATUS]
    );
    let blocked_context = unrelated
        .status_context(PUBLIC_STATUS)
        .await?
        .expect("public context should be authorized");
    assert!(
        !blocked_context
            .descendants
            .iter()
            .any(|status| status.id == -313),
        "author-side blocks filter context members"
    );
    assert!(
        blocked_context
            .descendants
            .iter()
            .any(|status| status.id == -315),
        "a silenced viewer retains their own context members"
    );
    let anonymous_context = anonymous
        .status_context(PUBLIC_STATUS)
        .await?
        .expect("public context should be authorized");
    assert!(
        !anonymous_context
            .descendants
            .iter()
            .any(|status| status.id == -315),
        "silenced context members stay hidden from non-followers"
    );
    let domain_context = owner
        .status_context(PUBLIC_STATUS)
        .await?
        .expect("public context should be authorized");
    assert!(
        !domain_context
            .descendants
            .iter()
            .any(|status| status.id == -314),
        "viewer-domain-blocked members are filtered from context"
    );
    assert!(
        anonymous
            .status_context(116_844_850_053_125_003)
            .await?
            .is_none()
    );

    let authenticated =
        RestProjectionLoader::new(repository, Some(ALICE), "fixture-v4-6-5.rustodon.invalid");
    let bob = authenticated.account(BOB).await?.unwrap();
    assert_eq!(bob.feature_current_user, "missing");
    let origin = Url::parse("https://fixture-v4-6-5.rustodon.invalid/").unwrap();
    let serializer = RestSerializer::new(
        &origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    let alice_json = serde_json::to_value(serializer.account(&accounts[0]).unwrap()).unwrap();
    let bob_json = serde_json::to_value(serializer.account(&bob).unwrap()).unwrap();
    assert_eq!(alice_json["id"], ALICE.to_string());
    assert_eq!(
        alice_json["note"],
        "<p>Primary local fixture account <span class=\"h-card\" translate=\"no\"><a href=\"https://remote.fixture.invalid/@bob\" class=\"u-url mention\">@<span>bob</span></a></span></p>"
    );
    assert_eq!(
        alice_json["fields"][0]["value"],
        "<span class=\"h-card\" translate=\"no\"><a href=\"https://fixture-v4-6-5.rustodon.invalid/@moderator\" class=\"u-url mention\">@<span>moderator</span></a></span>"
    );
    assert_eq!(
        alice_json["fields"][1]["value"],
        "<span class=\"h-card\" translate=\"no\"><a href=\"https://remote.fixture.invalid/@bob\" class=\"u-url mention\">@<span>bob@remote.fixture.invalid</span></a></span>"
    );
    assert_eq!(
        alice_json["fields"][2]["value"],
        "<span class=\"h-card\" translate=\"no\"><a href=\"https://remote.fixture.invalid/@suspended\" class=\"u-url mention\">@<span>suspended@remote.fixture.invalid</span></a></span>"
    );
    assert_eq!(bob_json["acct"], "bob@remote.fixture.invalid");
    assert_eq!(bob_json["fields"][0]["value"], "Exact JSONB value");

    let after = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(account) FROM accounts account ORDER BY id",
    )
    .fetch_all(&mut connection)
    .await?;
    assert_eq!(before, after, "REST projection loading must be read-only");
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn status_authorization_matrix_matches_mastodon_policy() -> sqlx::Result<()> {
    let repository = Repository::connect(&database_url()).await?;
    let cases = [
        (None, [true, true, false, false, false]),
        (Some(ALICE), [true, true, true, true, true]),
        (Some(BOB), [true, true, true, true, false]),
        (Some(MATRIX_VIEWER), [true, true, false, false, false]),
        (Some(API_MODERATOR), [true, true, true, true, true]),
    ];
    let status_ids = [
        PUBLIC_STATUS,
        UNLISTED_STATUS,
        PRIVATE_STATUS,
        DIRECT_STATUS,
        LIMITED_STATUS,
    ];

    for (viewer, expected) in cases {
        let loader = RestProjectionLoader::new(
            repository.clone(),
            viewer,
            "fixture-v4-6-5.rustodon.invalid",
        );
        for (status_id, visible) in status_ids.into_iter().zip(expected) {
            assert_eq!(
                loader.authorized_status(status_id).await?.is_some(),
                visible,
                "viewer {viewer:?}, status {status_id}"
            );
        }
    }

    let owner = RestProjectionLoader::new(
        repository.clone(),
        Some(ALICE),
        "fixture-v4-6-5.rustodon.invalid",
    );
    assert!(owner.authorized_status(-311).await?.is_some());
    assert!(owner.authorized_status(-312).await?.is_none());
    assert!(
        owner
            .authorized_status(DELETED_UNKNOWN_STATUS)
            .await?
            .is_none()
    );

    let blocked = RestProjectionLoader::new(
        repository.clone(),
        Some(API_MODERATOR),
        "fixture-v4-6-5.rustodon.invalid",
    );
    assert!(blocked.authorized_status(-313).await?.is_none());
    let domain_blocked = RestProjectionLoader::new(
        repository.clone(),
        Some(-320),
        "fixture-v4-6-5.rustodon.invalid",
    );
    assert!(
        domain_blocked
            .authorized_status(PUBLIC_STATUS)
            .await?
            .is_none()
    );
    let anonymous = RestProjectionLoader::new(repository, None, "fixture-v4-6-5.rustodon.invalid");
    assert!(anonymous.authorized_status(-310).await?.is_none());
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn account_statuses_owner_preserves_reblogs_from_blocked_sources() -> sqlx::Result<()> {
    const BLOCK_ID: i64 = -9510;
    const SOURCE_ACCOUNT: i64 = MATRIX_VIEWER;
    const OWNER_REBLOG: i64 = -416;

    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let mut owner = PgConnection::connect(&owner_url).await?;
    sqlx::query("DELETE FROM blocks WHERE id = $1")
        .bind(BLOCK_ID)
        .execute(&mut owner)
        .await?;
    sqlx::query(
        "INSERT INTO blocks (id, account_id, target_account_id, uri, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, '2026-07-01 20:00:00', '2026-07-01 20:00:00')",
    )
    .bind(BLOCK_ID)
    .bind(ALICE)
    .bind(SOURCE_ACCOUNT)
    .bind("https://fixture-v4-6-5.rustodon.invalid/users/alice#blocks/9510")
    .execute(&mut owner)
    .await?;

    let result = async {
        let repository = Repository::connect(&database_url()).await?;
        let loader =
            RestProjectionLoader::new(repository, Some(ALICE), "fixture-v4-6-5.rustodon.invalid");
        let statuses = loader
            .account_statuses(ALICE, &AccountStatusesOptions::default())
            .await?;
        Ok::<_, sqlx::Error>(status_ids(statuses).contains(&OWNER_REBLOG))
    }
    .await;

    sqlx::query("DELETE FROM blocks WHERE id = $1")
        .bind(BLOCK_ID)
        .execute(&mut owner)
        .await?;
    assert!(result?, "an account owner should retain their own reblog");
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn tagged_account_statuses_preserve_reblogs_from_blocked_sources() -> sqlx::Result<()> {
    const BLOCK_ID: i64 = -9512;
    const ACCOUNT: i64 = -330;
    const SOURCE_ACCOUNT: i64 = MATRIX_VIEWER;
    const TAGGED_REBLOG: i64 = -409;

    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let mut owner = PgConnection::connect(&owner_url).await?;
    sqlx::query("DELETE FROM blocks WHERE id = $1")
        .bind(BLOCK_ID)
        .execute(&mut owner)
        .await?;
    sqlx::query(
        "INSERT INTO blocks (id, account_id, target_account_id, uri, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, '2026-07-01 20:00:00', '2026-07-01 20:00:00')",
    )
    .bind(BLOCK_ID)
    .bind(ALICE)
    .bind(SOURCE_ACCOUNT)
    .bind("https://fixture-v4-6-5.rustodon.invalid/users/alice#blocks/9512")
    .execute(&mut owner)
    .await?;

    let result = async {
        let repository = Repository::connect(&database_url()).await?;
        let loader =
            RestProjectionLoader::new(repository, Some(ALICE), "fixture-v4-6-5.rustodon.invalid");
        let statuses = loader
            .account_statuses(
                ACCOUNT,
                &AccountStatusesOptions {
                    tagged: Some("FixtureTag".to_owned()),
                    ..AccountStatusesOptions::default()
                },
            )
            .await?;
        Ok::<_, sqlx::Error>(status_ids(statuses).contains(&TAGGED_REBLOG))
    }
    .await;

    sqlx::query("DELETE FROM blocks WHERE id = $1")
        .bind(BLOCK_ID)
        .execute(&mut owner)
        .await?;
    assert!(
        result?,
        "tagged account statuses must retain reblogs when the source is blocked"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn followed_tag_home_statuses_hide_authors_who_block_the_viewer() -> sqlx::Result<()> {
    const BLOCK_ID: i64 = -9511;
    const SOURCE_ACCOUNT: i64 = MATRIX_VIEWER;
    const TAG_STATUS: i64 = -414;

    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let mut owner = PgConnection::connect(&owner_url).await?;
    sqlx::query("DELETE FROM blocks WHERE id = $1")
        .bind(BLOCK_ID)
        .execute(&mut owner)
        .await?;
    sqlx::query(
        "INSERT INTO blocks (id, account_id, target_account_id, uri, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, '2026-07-01 20:00:00', '2026-07-01 20:00:00')",
    )
    .bind(BLOCK_ID)
    .bind(SOURCE_ACCOUNT)
    .bind(ALICE)
    .bind("https://fixture-v4-6-5.rustodon.invalid/users/matrix_viewer#blocks/9511")
    .execute(&mut owner)
    .await?;

    let result = async {
        let repository = Repository::connect(&database_url()).await?;
        let loader =
            RestProjectionLoader::new(repository, Some(ALICE), "fixture-v4-6-5.rustodon.invalid");
        let statuses = loader
            .home_timeline(
                ALICE,
                &TimelineOptions {
                    max_id: Some(0),
                    limit: 40,
                    ..TimelineOptions::default()
                },
            )
            .await?;
        Ok::<_, sqlx::Error>(status_ids(statuses).contains(&TAG_STATUS))
    }
    .await;

    sqlx::query("DELETE FROM blocks WHERE id = $1")
        .bind(BLOCK_ID)
        .execute(&mut owner)
        .await?;
    assert!(
        !result?,
        "followed-tag statuses must hide authors blocking the viewer"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn status_context_returns_the_requested_anonymous_descendant_limit() -> sqlx::Result<()> {
    const ROOT_STATUS: i64 = -9700;
    const LAST_STATUS: i64 = -9761;

    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let mut owner = PgConnection::connect(&owner_url).await?;
    sqlx::query(
        "WITH chain(n) AS (SELECT generate_series(0, 61)) \
         INSERT INTO statuses (id, account_id, text, spoiler_text, visibility, local, uri, url, \
           language, sensitive, reply, ordered_media_attachment_ids, in_reply_to_id, \
           in_reply_to_account_id, reblog_of_id, created_at, updated_at) \
         SELECT $1 - n, $2, format('context limit %s', n), '', 0, true, \
           format('https://fixture-v4-6-5.rustodon.invalid/users/alice/statuses/%s', $1 - n), \
           format('https://fixture-v4-6-5.rustodon.invalid/users/alice/statuses/%s', $1 - n), \
           'en', false, n > 0, NULL, \
           CASE WHEN n = 0 THEN NULL ELSE $1 END, \
           CASE WHEN n = 0 THEN NULL ELSE $2 END, NULL, \
           TIMESTAMP '2026-08-01 00:00:00' + n * INTERVAL '1 second', \
           TIMESTAMP '2026-08-01 00:00:00' + n * INTERVAL '1 second' \
         FROM chain",
    )
    .bind(ROOT_STATUS)
    .bind(ALICE)
    .execute(&mut owner)
    .await?;

    let result = async {
        let repository = Repository::connect(&database_url()).await?;
        let loader = RestProjectionLoader::new(repository, None, "fixture-v4-6-5.rustodon.invalid");
        let context = loader
            .status_context(ROOT_STATUS)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?;
        Ok::<_, sqlx::Error>(context.descendants.len())
    }
    .await;

    sqlx::query("DELETE FROM statuses WHERE id BETWEEN $1 AND $2")
        .bind(LAST_STATUS)
        .bind(ROOT_STATUS)
        .execute(&mut owner)
        .await?;
    assert_eq!(
        result?, 60,
        "anonymous context descendants should honor the limit"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn status_quotes_filter_blocked_authors_before_pagination() -> sqlx::Result<()> {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let mut owner = PgConnection::connect(&owner_url).await?;
    sqlx::query("DELETE FROM blocks WHERE id = $1")
        .bind(-9509_i64)
        .execute(&mut owner)
        .await?;
    sqlx::query(
        "INSERT INTO blocks (id, account_id, target_account_id, uri, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, '2026-07-01 20:00:00', '2026-07-01 20:00:00')",
    )
    .bind(-9509_i64)
    .bind(BOB)
    .bind(API_MODERATOR)
    .bind("https://remote.fixture.invalid/users/bob#blocks/9509-test")
    .execute(&mut owner)
    .await?;

    let repository = Repository::connect(&database_url()).await?;
    let loader = RestProjectionLoader::new(
        repository,
        Some(API_MODERATOR),
        "fixture-v4-6-5.rustodon.invalid",
    );
    let page = loader
        .status_quotes(PUBLIC_STATUS, &FollowCollectionOptions::default())
        .await?
        .expect("the public status should be visible to the API moderator");

    assert!(page.statuses.is_empty());
    assert_eq!(page.first_cursor, None);
    assert_eq!(page.last_cursor, None);
    assert!(!page.records_continue);
    sqlx::query("DELETE FROM blocks WHERE id = $1")
        .bind(-9509_i64)
        .execute(&mut owner)
        .await
        .map(|_| ())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn unknown_visibility_fails_closed_across_status_context_and_quote() -> sqlx::Result<()> {
    let url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let mut owner = PgConnection::connect(&owner_url).await?;
    sqlx::query("UPDATE statuses SET deleted_at = NULL, in_reply_to_id = $1 WHERE id = $2")
        .bind(PUBLIC_STATUS)
        .bind(DELETED_UNKNOWN_STATUS)
        .execute(&mut owner)
        .await?;
    sqlx::query("UPDATE statuses SET visibility = 99 WHERE id = $1")
        .bind(116_845_093_847_045_103_i64)
        .execute(&mut owner)
        .await?;

    let result = async {
        let repository = Repository::connect(&url).await?;
        let alice = RestProjectionLoader::new(
            repository.clone(),
            Some(ALICE),
            "fixture-v4-6-5.rustodon.invalid",
        );
        let bob =
            RestProjectionLoader::new(repository, Some(BOB), "fixture-v4-6-5.rustodon.invalid");
        let root_hidden = alice
            .authorized_status(DELETED_UNKNOWN_STATUS)
            .await?
            .is_none();
        let context = alice
            .status_context(PUBLIC_STATUS)
            .await?
            .expect("public root should remain visible");
        let context_hidden = context
            .ancestors
            .iter()
            .chain(&context.descendants)
            .all(|status| status.id != DELETED_UNKNOWN_STATUS);
        let quote = bob
            .authorized_status(116_845_314_048_005_201)
            .await?
            .expect("public quote source should remain visible")
            .quote
            .expect("quote state should remain present");
        Ok::<_, sqlx::Error>((
            root_hidden,
            context_hidden,
            quote.target_access,
            quote.target_link.is_none(),
            quote.quoted_status.is_none(),
        ))
    }
    .await;

    sqlx::query(
        "UPDATE statuses SET deleted_at = TIMESTAMP '2026-07-01 19:30:00', \
         in_reply_to_id = NULL WHERE id = $1",
    )
    .bind(DELETED_UNKNOWN_STATUS)
    .execute(&mut owner)
    .await?;
    sqlx::query("UPDATE statuses SET visibility = 0 WHERE id = $1")
        .bind(116_845_093_847_045_103_i64)
        .execute(&mut owner)
        .await?;

    let (root_hidden, context_hidden, quote_access, link_hidden, target_hidden) = result?;
    assert!(root_hidden);
    assert!(context_hidden);
    assert_eq!(
        quote_access,
        rustodon::mastodon::rest::QuoteTargetAccess::Unauthorized
    );
    assert!(link_hidden);
    assert!(target_hidden);
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn credential_projection_uses_the_exact_oauth_user() -> sqlx::Result<()> {
    let url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let mut owner = PgConnection::connect(&owner_url).await?;
    sqlx::query("UPDATE users SET account_id = $1, settings = '{\"noindex\":true}' WHERE id = 104")
        .bind(ALICE)
        .execute(&mut owner)
        .await?;

    let result = async {
        let loader = RestProjectionLoader::new(
            Repository::connect(&url).await?,
            Some(ALICE),
            "fixture-v4-6-5.rustodon.invalid",
        );
        loader
            .credential_account(104, ALICE)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }
    .await;

    sqlx::query("UPDATE users SET account_id = $1, settings = NULL WHERE id = 104")
        .bind(API_MODERATOR)
        .execute(&mut owner)
        .await?;

    let credential = result?;
    assert_eq!(credential.account.noindex, Some(true));
    assert_eq!(credential.account.roles.as_deref().unwrap().len(), 1);
    assert_eq!(credential.account.roles.as_ref().unwrap()[0].id, 92);
    assert_eq!(credential.role.id, 92);
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn postgresql_timelines_and_collections_match_mastodon_selection() -> sqlx::Result<()> {
    let repository = Repository::connect(&database_url()).await?;
    let anonymous =
        RestProjectionLoader::new(repository.clone(), None, "fixture-v4-6-5.rustodon.invalid");
    let owner = RestProjectionLoader::new(
        repository.clone(),
        Some(ALICE),
        "fixture-v4-6-5.rustodon.invalid",
    );
    let negative_page = TimelineOptions {
        max_id: Some(0),
        limit: 40,
        ..TimelineOptions::default()
    };

    let public = status_ids(anonymous.public_timeline(&negative_page).await?);
    assert!(public.contains(&-400), "ordinary public posts are included");
    assert!(public.contains(&-404), "public self-replies are included");
    assert!(
        public.contains(&-423),
        "anonymous public reads do not filter languages"
    );
    for excluded in [-402, -403, -410, -411, -413] {
        assert!(!public.contains(&excluded), "public status {excluded}");
    }
    let authenticated_public = status_ids(owner.public_timeline(&negative_page).await?);
    assert!(authenticated_public.contains(&-400));
    for excluded in [-401, -412, -423] {
        assert!(
            !authenticated_public.contains(&excluded),
            "authenticated public status {excluded}"
        );
    }
    let public_media = status_ids(
        anonymous
            .public_timeline(&TimelineOptions {
                only_media: true,
                local: true,
                ..TimelineOptions::default()
            })
            .await?,
    );
    assert!(public_media.contains(&116_845_314_048_005_201));
    let first_page = owner
        .public_timeline(&TimelineOptions {
            max_id: Some(0),
            limit: 2,
            ..TimelineOptions::default()
        })
        .await?;
    assert_eq!(first_page.len(), 2);
    assert!(first_page[0].id > first_page[1].id);
    let newer = status_ids(
        owner
            .public_timeline(&TimelineOptions {
                max_id: Some(0),
                min_id: Some(first_page[1].id),
                limit: 2,
                ..TimelineOptions::default()
            })
            .await?,
    );
    assert_eq!(newer, vec![first_page[0].id]);

    let tagged = status_ids(
        anonymous
            .tag_timeline(
                "FixtureTag",
                &TagTimelineOptions {
                    page: negative_page.clone(),
                    ..TagTimelineOptions::default()
                },
            )
            .await?,
    );
    for included in [-408, -409, -423, -431] {
        assert!(tagged.contains(&included), "tagged status {included}");
    }
    let authenticated_tagged = status_ids(
        owner
            .tag_timeline(
                "fixturetag",
                &TagTimelineOptions {
                    page: negative_page.clone(),
                    ..TagTimelineOptions::default()
                },
            )
            .await?,
    );
    assert!(
        authenticated_tagged.contains(&-423),
        "authenticated tag timelines retain all tagged public languages"
    );
    assert!(!tagged.contains(&-410));
    assert!(!tagged.contains(&-411));
    let tag_media = status_ids(
        anonymous
            .tag_timeline(
                "fixturetag",
                &TagTimelineOptions {
                    page: TimelineOptions {
                        only_media: true,
                        local: true,
                        ..TimelineOptions::default()
                    },
                    ..TagTimelineOptions::default()
                },
            )
            .await?,
    );
    assert!(tag_media.contains(&116_845_314_048_005_201));
    let combined = status_ids(
        anonymous
            .tag_timeline(
                "fixturetag",
                &TagTimelineOptions {
                    page: negative_page.clone(),
                    any: vec!["anytag".to_owned()],
                    all: vec!["alltag".to_owned()],
                    none: vec!["nonetag".to_owned()],
                },
            )
            .await?,
    );
    assert!(combined.contains(&-409));
    assert!(combined.contains(&-430));
    assert!(!combined.contains(&-431));
    assert!(
        anonymous
            .tag_timeline(
                "missingtag",
                &TagTimelineOptions {
                    page: negative_page.clone(),
                    any: vec!["fixturetag".to_owned()],
                    ..TagTimelineOptions::default()
                },
            )
            .await?
            .is_empty(),
        "a missing base hashtag cannot fall through to any[] tags"
    );
    assert_eq!(
        status_ids(
            anonymous
                .tag_timeline(
                    "ＦｉｘｔｕｒｅＴａｇ",
                    &TagTimelineOptions {
                        page: negative_page.clone(),
                        ..TagTimelineOptions::default()
                    },
                )
                .await?,
        ),
        tagged,
        "hashtag lookup uses Mastodon's NFKC normalization"
    );
    let raw_limited = status_ids(
        anonymous
            .tag_timeline(
                "fixturetag",
                &TagTimelineOptions {
                    page: negative_page.clone(),
                    any: vec![
                        "ＦｉｘｔｕｒｅＴａｇ".to_owned(),
                        "missingone".to_owned(),
                        "missingtwo".to_owned(),
                        "anytag".to_owned(),
                    ],
                    ..TagTimelineOptions::default()
                },
            )
            .await?,
    );
    assert!(
        !raw_limited.contains(&-430),
        "the four-tag any limit is applied before normalization"
    );

    let home = status_ids(owner.home_timeline(ALICE, &negative_page).await?);
    for included in [
        -400, -402, -403, -404, -405, -407, -408, -409, -414, -419, -420, -421, -423, -426,
    ] {
        assert!(home.contains(&included), "home status {included}");
    }
    for excluded in [-401, -406, -410, -411, -412, -418, -424, -425, -427] {
        assert!(!home.contains(&excluded), "home status {excluded}");
    }
    let filtered = owner
        .home_timeline(ALICE, &negative_page)
        .await?
        .into_iter()
        .find(|status| status.id == -420)
        .and_then(|status| status.viewer)
        .expect("authenticated home status has viewer relationships");
    assert_eq!(filtered.filtered[0].filter.id, 9101);

    let normal_list = owner
        .list_timeline(ALICE, 9001, &negative_page)
        .await?
        .expect("Alice owns the normal fixture list");
    let normal_list = status_ids(normal_list);
    for included in [
        -400, -402, -403, -404, -405, -406, -407, -409, -419, -420, -424, -426, -430, -431,
    ] {
        assert!(normal_list.contains(&included), "list status {included}");
    }
    for excluded in [-401, -408, -412, -418, -421, -423, -425, -427] {
        assert!(!normal_list.contains(&excluded), "list status {excluded}");
    }
    assert!(
        owner
            .list_timeline(ALICE, 9001, &TimelineOptions::default())
            .await?
            .expect("Alice owns the normal fixture list")
            .iter()
            .any(|status| status.id == PUBLIC_STATUS),
        "an owner can be an active list member without a follow row"
    );
    let owner_boost = owner
        .list_timeline(ALICE, 9001, &TimelineOptions::default())
        .await?
        .expect("Alice owns the normal fixture list")
        .into_iter()
        .find(|status| status.id == -416)
        .expect("an owner self-member boost bypasses follow boost preferences");
    let wrapper_filters = &owner_boost
        .viewer
        .as_ref()
        .expect("authenticated status has viewer data")
        .filtered;
    let nested_filters = &owner_boost
        .reblog
        .as_ref()
        .expect("boost has a nested status")
        .viewer
        .as_ref()
        .expect("authenticated nested status has viewer data")
        .filtered;
    assert_eq!(wrapper_filters, nested_filters);
    assert_eq!(wrapper_filters[0].status_matches, Some(vec![-416]));
    let followed_list = status_ids(
        owner
            .list_timeline(ALICE, 9002, &negative_page)
            .await?
            .expect("Alice owns the exclusive followed-replies list"),
    );
    assert!(followed_list.contains(&-424));
    let no_replies_list = status_ids(
        owner
            .list_timeline(ALICE, 9005, &negative_page)
            .await?
            .expect("Alice owns the no-replies list"),
    );
    assert!(no_replies_list.contains(&-404));
    assert!(no_replies_list.contains(&-405));
    assert!(!no_replies_list.contains(&-407));
    assert!(
        owner
            .list_timeline(API_MODERATOR, 9001, &negative_page)
            .await?
            .is_none(),
        "another account cannot read Alice's list"
    );
    assert!(
        owner
            .account_statuses(-331, &AccountStatusesOptions::default())
            .await?
            .iter()
            .any(|status| status.id == -406),
        "exclusive-list suppression is not status authorization"
    );
    let normalized_account_tag = anonymous
        .account_statuses(
            ALICE,
            &AccountStatusesOptions {
                tagged: Some("ＦｉｘｔｕｒｅＴａｇ".to_owned()),
                ..AccountStatusesOptions::default()
            },
        )
        .await?;
    assert_eq!(
        status_ids(normalized_account_tag),
        vec![116_845_314_048_005_201, PUBLIC_STATUS]
    );

    let favourites = owner
        .saved_statuses(
            ALICE,
            SavedStatusKind::Favourites,
            &SavedStatusesOptions::default(),
        )
        .await?;
    assert_eq!(favourites.first_cursor, Some(8111));
    assert_eq!(favourites.last_cursor, Some(8110));
    assert_eq!(status_ids(favourites.statuses), vec![-412, -400]);
    let favourite_page = owner
        .saved_statuses(
            ALICE,
            SavedStatusKind::Favourites,
            &SavedStatusesOptions {
                max_id: Some(8111),
                limit: 1,
                ..SavedStatusesOptions::default()
            },
        )
        .await?;
    assert_eq!(favourite_page.first_cursor, Some(8110));
    assert_eq!(status_ids(favourite_page.statuses), vec![-400]);

    let bookmarks = owner
        .saved_statuses(
            ALICE,
            SavedStatusKind::Bookmarks,
            &SavedStatusesOptions::default(),
        )
        .await?;
    assert_eq!(bookmarks.first_cursor, Some(9511));
    assert_eq!(bookmarks.last_cursor, Some(9501));
    assert_eq!(
        status_ids(bookmarks.statuses),
        vec![-412, -400, 116_845_078_118_405_101]
    );

    let blocks = owner
        .account_list(
            ALICE,
            AccountListKind::Blocks,
            &AccountListOptions::default(),
        )
        .await?;
    assert_eq!(blocks.first_cursor, Some(9502));
    assert_eq!(blocks.last_cursor, Some(9502));
    assert_eq!(blocks.entries[0].account.id, 116_844_606_259_202_002);
    let mutes = owner
        .account_list(
            ALICE,
            AccountListKind::Mutes,
            &AccountListOptions::default(),
        )
        .await?;
    assert_eq!(mutes.first_cursor, Some(9503));
    assert_eq!(mutes.last_cursor, Some(9503));
    assert_eq!(mutes.entries[0].account.id, BOB);
    assert_eq!(
        mutes.entries[0].mute_expires_at.unwrap().to_string(),
        "2026-08-01 00:00:00"
    );
    Ok(())
}

fn status_ids(statuses: Vec<rustodon::mastodon::rest::StatusProjection>) -> Vec<i64> {
    statuses.into_iter().map(|status| status.id).collect()
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn fixture_role_cannot_mutate_even_after_disabling_session_read_only() -> sqlx::Result<()> {
    let url = database_url();
    let repository = Repository::connect(&url).await?;
    let before = repository
        .status_including_deleted(DELETED_UNKNOWN_STATUS)
        .await?;
    let mut connection = PgConnection::connect(&url).await?;
    let read_only: String = sqlx::query_scalar("SHOW default_transaction_read_only")
        .fetch_one(&mut connection)
        .await?;
    assert_eq!(read_only, "on");

    sqlx::query("SET default_transaction_read_only = off")
        .execute(&mut connection)
        .await?;
    for statement in [
        "INSERT INTO bookmarks (account_id, status_id, created_at, updated_at) VALUES (1, 1, now(), now())",
        "UPDATE accounts SET display_name = 'mutated' WHERE id = -99",
        "DELETE FROM tombstones WHERE id = 9901",
        "TRUNCATE TABLE statuses",
        "CREATE TABLE rustodon_forbidden_fixture_table (id bigint)",
        "CREATE SCHEMA rustodon_forbidden_fixture_schema",
        "CREATE TEMPORARY TABLE rustodon_forbidden_fixture_temp (id bigint)",
        "SELECT timestamp_id('statuses')",
    ] {
        let error = sqlx::query(statement)
            .execute(&mut connection)
            .await
            .expect_err("the fixture reader must not have write privileges");
        assert!(
            error.as_database_error().is_some(),
            "unexpected error: {error}"
        );
    }

    assert!(
        repository
            .status_including_deleted(DELETED_UNKNOWN_STATUS)
            .await?
            .is_some()
    );
    assert_eq!(
        before.unwrap().text,
        "Soft-deleted unknown visibility fixture status"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn write_repository_updates_markers_with_optimistic_lock()
-> Result<(), Box<dyn std::error::Error>> {
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let (last_read_id, lock_version, updated_at): (i64, i32, NaiveDateTime) = sqlx::query_as(
        "SELECT last_read_id, lock_version, updated_at FROM markers \
         WHERE user_id = $1 AND timeline = $2",
    )
    .bind(101_i64)
    .bind("home")
    .fetch_one(&pool)
    .await?;
    let notification_markers: i64 =
        sqlx::query_scalar("SELECT count(*) FROM markers WHERE user_id = $1 AND timeline = $2")
            .bind(101_i64)
            .bind("notifications")
            .fetch_one(&pool)
            .await?;
    assert_eq!(notification_markers, 0);

    let updated = writer
        .update_marker(&authenticated, "home", last_read_id + 1, Some(lock_version))
        .await;
    let conflict = writer
        .update_marker(&authenticated, "home", last_read_id + 2, Some(lock_version))
        .await;
    let inserted = writer
        .update_marker(&authenticated, "notifications", 123, None)
        .await;
    let both = writer
        .update_markers(
            &authenticated,
            &[
                ("home".to_owned(), Some(last_read_id + 2)),
                ("notifications".to_owned(), Some(456)),
            ],
        )
        .await?;
    assert_eq!(both.len(), 2);
    assert_eq!(both[0].last_read_id, last_read_id + 2);
    assert_eq!(both[1].last_read_id, 456);
    sqlx::query(
        "UPDATE markers SET last_read_id = $1, lock_version = $2, updated_at = $3 \
         WHERE user_id = $4 AND timeline = $5",
    )
    .bind(last_read_id)
    .bind(lock_version)
    .bind(updated_at)
    .bind(101_i64)
    .bind("home")
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM markers WHERE user_id = $1 AND timeline = $2")
        .bind(101_i64)
        .bind("notifications")
        .execute(&pool)
        .await?;
    let updated = updated?;
    assert_eq!(updated.last_read_id, last_read_id + 1);
    assert_eq!(updated.lock_version, lock_version + 1);
    assert!(matches!(
        conflict.expect_err("a stale marker update must conflict"),
        WriteError::Conflict
    ));
    let inserted = inserted?;
    assert_eq!(inserted.last_read_id, 123);
    assert_eq!(inserted.lock_version, 1);
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
#[allow(clippy::type_complexity)]
async fn write_repository_creates_reports_transactionally() -> Result<(), Box<dyn Error>> {
    const REPORT_STATUS: i64 = 116_845_078_118_405_101;
    const REPORT_COLLECTION: i64 = 116_845_549_977_608_801;

    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer_url = std::env::var("RUSTODON_MASTODON_WRITER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_WRITER_DATABASE_URL");
    let writer = WriteRepository::connect(&writer_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_REPORTS).await?;
    let mut report_ids = Vec::new();
    let mut forwarded_report_uris = Vec::new();
    let mut admin_deleted_status_ids = Vec::new();
    let result =
        async {
            let before_count: i64 = sqlx::query_scalar("SELECT count(*) FROM reports")
                .fetch_one(&pool)
                .await?;
            let invalid = writer
                .create_report(
                    &authenticated,
                    BOB,
                    "invalid collection",
                    Some("other"),
                    &[],
                    &[i64::MAX],
                    &[],
                    Some(false),
                    None,
                    "https://fixture-v4-6-5.rustodon.invalid/",
                    false,
                )
                .await;
            assert!(matches!(invalid, Err(WriteError::NotFound)));
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM reports")
                    .fetch_one(&pool)
                    .await?,
                before_count
            );

            let previous_block: Option<Value> = sqlx::query_scalar(
                "SELECT to_jsonb(block) FROM blocks block \
                 WHERE account_id = $1 AND target_account_id = $2",
            )
            .bind(BOB)
            .bind(ALICE)
            .fetch_optional(&pool)
            .await?;
            sqlx::query(
                "DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2",
            )
            .bind(BOB)
            .bind(ALICE)
            .execute(&pool)
            .await?;
            sqlx::query(
                "INSERT INTO blocks (account_id, target_account_id, created_at, updated_at) \
                 VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
            )
            .bind(BOB)
            .bind(ALICE)
            .execute(&pool)
            .await?;
            let blocked_status_report = writer
                .create_report(
                    &authenticated,
                    BOB,
                    "blocked status",
                    Some("other"),
                    &[REPORT_STATUS],
                    &[],
                    &[],
                    Some(false),
                    None,
                    "https://fixture-v4-6-5.rustodon.invalid/",
                    false,
                )
                .await;
            sqlx::query(
                "DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2",
            )
            .bind(BOB)
            .bind(ALICE)
            .execute(&pool)
            .await?;
            if let Some(previous_block) = previous_block {
                sqlx::query(
                    "INSERT INTO blocks \
                     SELECT * FROM jsonb_populate_record(NULL::public.blocks, $1)",
                )
                .bind(previous_block)
                .execute(&pool)
                .await?;
            }
            assert!(matches!(blocked_status_report, Err(WriteError::NotFound)));

            let invalid_violation = writer
                .create_report(
                    &authenticated,
                    BOB,
                    "missing report rules",
                    Some("violation"),
                    &[],
                    &[],
                    &[],
                    Some(false),
                    None,
                    "https://fixture-v4-6-5.rustodon.invalid/",
                    false,
                )
                .await;
            assert!(matches!(
                invalid_violation,
                Err(WriteError::Validation("violation reports require rules"))
            ));

            let attached_report = writer
                .create_report(
                    &authenticated,
                    BOB,
                    "attached report",
                    Some("spam"),
                    &[REPORT_STATUS],
                    &[REPORT_COLLECTION],
                    &[],
                    Some(false),
                    None,
                    "https://fixture-v4-6-5.rustodon.invalid/",
                    false,
                )
                .await?;
            report_ids.push(attached_report);
            let row: (
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
                "SELECT account_id, target_account_id, application_id, category, comment, \
                    forwarded, rule_ids, status_ids, uri \
             FROM reports WHERE id = $1",
            )
            .bind(attached_report)
            .fetch_one(&pool)
            .await?;
            assert_eq!(row.0, ALICE);
            assert_eq!(row.1, BOB);
            assert_eq!(row.2, Some(301));
            assert_eq!(row.3, 1_000);
            assert_eq!(row.4, "attached report");
            assert_eq!(row.5, Some(false));
            assert_eq!(row.6, None);
            assert_eq!(row.7, vec![REPORT_STATUS]);
            assert!(row.8.as_deref().is_some_and(|uri| {
                uri.starts_with("https://fixture-v4-6-5.rustodon.invalid/")
            }));
            assert_eq!(
                sqlx::query_scalar::<_, Vec<i64>>(
                    "SELECT array_agg(collection_id ORDER BY id) \
                 FROM collection_reports WHERE report_id = $1",
                )
                .bind(attached_report)
                .fetch_one(&pool)
                .await?,
                vec![REPORT_COLLECTION]
            );

            let loader = RestProjectionLoader::new(
                Repository::connect(&database_url).await?,
                Some(ALICE),
                "fixture-v4-6-5.rustodon.invalid",
            );
            let projection = loader
                .report(attached_report)
                .await?
                .expect("the newly-created report should be readable");
            assert_eq!(projection.status_ids, vec![REPORT_STATUS]);
            assert_eq!(projection.collection_ids, vec![REPORT_COLLECTION]);
            assert_eq!(projection.category, 1_000);
            let unauthorized = writer
                .set_report_resolution(NEWBIE, attached_report, true)
                .await;
            assert!(
                matches!(&unauthorized, Err(WriteError::Unauthorized)),
                "unexpected unauthorized result: {unauthorized:?}"
            );
            writer
                .set_report_resolution(MODERATOR, attached_report, true)
                .await?;
            let resolved: (Option<NaiveDateTime>, Option<i64>) = sqlx::query_as(
                "SELECT action_taken_at, action_taken_by_account_id \
                 FROM reports WHERE id = $1",
            )
            .bind(attached_report)
            .fetch_one(&pool)
            .await?;
            assert!(resolved.0.is_some());
            assert_eq!(resolved.1, Some(MODERATOR));
            assert_eq!(
                sqlx::query_scalar::<_, String>(
                    "SELECT action FROM admin_action_logs \
                     WHERE target_type = 'Report' AND target_id = $1 ORDER BY id DESC LIMIT 1",
                )
                .bind(attached_report)
                .fetch_one(&pool)
                .await?,
                "resolve"
            );
            writer
                .set_report_resolution(MODERATOR, attached_report, false)
                .await?;
            let reopened: (Option<NaiveDateTime>, Option<i64>) = sqlx::query_as(
                "SELECT action_taken_at, action_taken_by_account_id \
                 FROM reports WHERE id = $1",
            )
            .bind(attached_report)
            .fetch_one(&pool)
            .await?;
            assert_eq!(reopened, (None, None));

            let admin_deleted_status = writer
                .create_status(
                    &authenticated,
                    "admin deleted status",
                    &[],
                    None,
                    Some(false),
                    Some("public"),
                    None,
                    None,
                    None,
                )
                .await?
                .status_id;
            admin_deleted_status_ids.push(admin_deleted_status);
            let unauthorized_delete = writer
                .delete_status_as_moderator(NEWBIE, admin_deleted_status, false)
                .await;
            assert!(matches!(
                &unauthorized_delete,
                Err(WriteError::Unauthorized)
            ));
            assert!(
                writer
                    .delete_status_as_moderator(MODERATOR, admin_deleted_status, false)
                    .await?
                    .is_empty()
            );
            assert!(
                sqlx::query_scalar::<_, Option<NaiveDateTime>>(
                    "SELECT deleted_at FROM statuses WHERE id = $1"
                )
                .bind(admin_deleted_status)
                .fetch_one(&pool)
                .await?
                .is_some()
            );
            assert_eq!(
                sqlx::query_scalar::<_, String>(
                    "SELECT action FROM admin_action_logs \
                     WHERE target_type = 'Status' AND target_id = $1 ORDER BY id DESC LIMIT 1",
                )
                .bind(admin_deleted_status)
                .fetch_one(&pool)
                .await?,
                "destroy"
            );

            let statuses_count_before: i64 = sqlx::query_scalar(
                "SELECT statuses_count FROM account_stats WHERE account_id = $1",
            )
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
            let direct_status = writer
                .create_status(
                    &authenticated,
                    "direct counter status",
                    &[],
                    None,
                    Some(false),
                    Some("direct"),
                    None,
                    None,
                    None,
                )
                .await?
                .status_id;
            admin_deleted_status_ids.push(direct_status);
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT statuses_count FROM account_stats WHERE account_id = $1",
                )
                .bind(ALICE)
                .fetch_one(&pool)
                .await?,
                statuses_count_before
            );
            sqlx::query("UPDATE statuses SET account_id = $2 WHERE id = $1")
                .bind(direct_status)
                .bind(CAROL)
                .execute(&pool)
                .await?;
            writer.reconcile_account_stats(MODERATOR, CAROL).await?;
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT statuses_count FROM account_stats WHERE account_id = $1",
                )
                .bind(CAROL)
                .fetch_one(&pool)
                .await?,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM statuses \
                     WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3",
                )
                .bind(CAROL)
                .fetch_one(&pool)
                .await?
            );
            writer
                .delete_status_as_moderator(MODERATOR, direct_status, false)
                .await?;
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT statuses_count FROM account_stats WHERE account_id = $1",
                )
                .bind(ALICE)
                .fetch_one(&pool)
                .await?,
                statuses_count_before
            );

            let reply_parent = writer
                .create_status(
                    &authenticated,
                    "reply counter parent",
                    &[],
                    None,
                    Some(false),
                    Some("public"),
                    None,
                    None,
                    None,
                )
                .await?
                .status_id;
            let replies_before: i64 = sqlx::query_scalar(
                "SELECT replies_count FROM status_stats WHERE status_id = $1",
            )
            .bind(reply_parent)
            .fetch_one(&pool)
            .await?;
            let reply_status = writer
                .create_status(
                    &authenticated,
                    "reply counter child",
                    &[],
                    None,
                    Some(false),
                    Some("public"),
                    None,
                    Some(reply_parent),
                    None,
                )
                .await?
                .status_id;
            admin_deleted_status_ids.extend([reply_status, reply_parent]);
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT replies_count FROM status_stats WHERE status_id = $1",
                )
                .bind(reply_parent)
                .fetch_one(&pool)
                .await?,
                replies_before + 1
            );
            writer
                .delete_status_as_moderator(MODERATOR, reply_status, false)
                .await?;
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT replies_count FROM status_stats WHERE status_id = $1",
                )
                .bind(reply_parent)
                .fetch_one(&pool)
                .await?,
                replies_before
            );
            writer
                .delete_status_as_moderator(MODERATOR, reply_parent, false)
                .await?;
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT statuses_count FROM account_stats WHERE account_id = $1",
                )
                .bind(ALICE)
                .fetch_one(&pool)
                .await?,
                statuses_count_before
            );

            let stats_before: (i64, i64, Option<NaiveDateTime>, i64) = sqlx::query_as(
                "SELECT followers_count, following_count, last_status_at, statuses_count \
                 FROM account_stats WHERE account_id = $1",
            )
            .bind(CAROL)
            .fetch_one(&pool)
            .await?;
            sqlx::query(
                "UPDATE account_stats SET followers_count = 999, following_count = 998, \
                 statuses_count = 997, last_status_at = NULL WHERE account_id = $1",
            )
            .bind(CAROL)
            .execute(&pool)
            .await?;
            sqlx::query("UPDATE status_stats SET replies_count = 999 WHERE status_id = $1")
                .bind(-313_i64)
                .execute(&pool)
                .await?;
            let unauthorized_reconcile = writer.reconcile_account_stats(NEWBIE, CAROL).await;
            assert!(matches!(
                &unauthorized_reconcile,
                Err(WriteError::Unauthorized)
            ));
            writer.reconcile_account_stats(MODERATOR, CAROL).await?;
            let stats_after: (i64, i64, Option<NaiveDateTime>, i64) = sqlx::query_as(
                "SELECT followers_count, following_count, last_status_at, statuses_count \
                 FROM account_stats WHERE account_id = $1",
            )
            .bind(CAROL)
            .fetch_one(&pool)
            .await?;
            let expected_stats: (i64, i64, Option<NaiveDateTime>, i64) = sqlx::query_as(
                "SELECT \
                 (SELECT count(*) FROM follows WHERE target_account_id = $1), \
                 (SELECT count(*) FROM follows WHERE account_id = $1), \
                  (SELECT max(created_at) FROM statuses WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3), \
                  (SELECT count(*) FROM statuses WHERE account_id = $1 AND deleted_at IS NULL AND visibility <> 3)",
            )
            .bind(CAROL)
            .fetch_one(&pool)
            .await?;
            assert_eq!(stats_after, expected_stats);
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT replies_count FROM status_stats WHERE status_id = $1",
                )
                .bind(-313_i64)
                .fetch_one(&pool)
                .await?,
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM statuses \
                     WHERE in_reply_to_id = $1 AND deleted_at IS NULL",
                )
                .bind(-313_i64)
                .fetch_one(&pool)
                .await?
            );
            assert_eq!(
                sqlx::query_scalar::<_, String>(
                    "SELECT action FROM admin_action_logs \
                     WHERE target_type = 'Account' AND target_id = $1 ORDER BY id DESC LIMIT 1",
                )
                .bind(CAROL)
                .fetch_one(&pool)
                .await?,
                "reconcile_account_stats"
            );
            sqlx::query(
                "UPDATE account_stats SET followers_count = $2, following_count = $3, \
                 last_status_at = $4, statuses_count = $5 WHERE account_id = $1",
            )
            .bind(CAROL)
            .bind(stats_before.0)
            .bind(stats_before.1)
            .bind(stats_before.2)
            .bind(stats_before.3)
            .execute(&pool)
            .await?;

            let notification_report = writer
                .create_report(
                    &authenticated,
                    CAROL,
                    "staff notification report",
                    None,
                    &[],
                    &[],
                    &[],
                    None,
                    None,
                    "https://fixture-v4-6-5.rustodon.invalid/",
                    false,
                )
                .await?;
            report_ids.push(notification_report);
            assert_eq!(
                sqlx::query_scalar::<_, Option<bool>>("SELECT forwarded FROM reports WHERE id = $1")
                    .bind(notification_report)
                    .fetch_one(&pool)
                    .await?,
                None
            );
            for recipient_account_id in [MODERATOR, API_MODERATOR] {
                assert_eq!(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM rustodon.outbox_events \
                     WHERE kind = $1 \
                       AND payload -> 'arguments' ->> 'recipient_account_id' = $2 \
                       AND payload -> 'arguments' ->> 'activity_type' = 'admin.report' \
                       AND payload -> 'arguments' ->> 'activity_id' = $3",
                    )
                    .bind(NOTIFICATION_CREATE_JOB_KIND)
                    .bind(recipient_account_id.to_string())
                    .bind(notification_report.to_string())
                    .fetch_one(&pool)
                    .await?,
                    1
                );
            }
            sqlx::query(
                "UPDATE statuses SET in_reply_to_id = $2, in_reply_to_account_id = $3 \
                 WHERE id = $1",
            )
                .bind(-424_i64)
                .bind(-400_i64)
                .bind(-330_i64)
                .execute(&pool)
                .await?;
            let forwarded_report = writer
                .create_report(
                    &authenticated,
                    REMOTE_AP_ACCOUNT,
                    "forwarded report",
                    None,
                    &[-424_i64],
                    &[],
                    &[],
                    Some(true),
                    None,
                    "https://fixture-v4-6-5.rustodon.invalid/",
                    false,
                )
                .await?;
            report_ids.push(forwarded_report);
            let (forwarded, forwarded_uri): (bool, Option<String>) = sqlx::query_as(
                "SELECT forwarded, uri FROM reports WHERE id = $1",
            )
            .bind(forwarded_report)
            .fetch_one(&pool)
            .await?;
            assert!(forwarded);
            let forwarded_uri = forwarded_uri.expect("local forwarded reports have a URI");
            assert!(forwarded_uri.contains("/payloads/"));
            forwarded_report_uris.push(forwarded_uri.clone());
            let target_uri: String =
                sqlx::query_scalar("SELECT uri FROM accounts WHERE id = $1")
                    .bind(REMOTE_AP_ACCOUNT)
                    .fetch_one(&pool)
                    .await?;
            let (remote_domain, body): (String, Value) = sqlx::query_as(
                "SELECT payload -> 'arguments' ->> 'remote_domain', \
                        payload -> 'arguments' -> 'body' \
                   FROM rustodon.outbox_events \
                  WHERE kind = $1 AND payload -> 'arguments' -> 'body' ->> 'id' = $2",
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .bind(&forwarded_uri)
            .fetch_one(&pool)
            .await?;
            assert_eq!(remote_domain, "remote.fixture.invalid");
            assert_eq!(body["type"], "Flag");
            assert_eq!(
                body["actor"],
                "https://fixture-v4-6-5.rustodon.invalid/actor"
            );
            assert_eq!(body["id"], forwarded_uri);
            assert_eq!(body["content"], "forwarded report");
            assert_eq!(body["object"][0], target_uri);
            let reply_status_uri: String =
                sqlx::query_scalar("SELECT uri FROM statuses WHERE id = $1")
                    .bind(-424_i64)
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(body["object"][1], reply_status_uri);
            let inboxes: Vec<String> = sqlx::query_scalar(
                "SELECT payload -> 'arguments' ->> 'inbox_url' \
                   FROM rustodon.outbox_events \
                  WHERE kind = $1 AND payload -> 'arguments' -> 'body' ->> 'id' = $2 \
                  ORDER BY 1",
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .bind(&forwarded_uri)
            .fetch_all(&pool)
            .await?;
            assert_eq!(
                inboxes,
                vec![
                    "https://remote.fixture.invalid/users/exclusive_author/inbox",
                    "https://remote.fixture.invalid/users/timeline_author/inbox",
                ]
            );
            Ok::<(), Box<dyn Error>>(())
        }
        .await;

    sqlx::query(
        "UPDATE statuses SET in_reply_to_id = $2, in_reply_to_account_id = $3 WHERE id = $1",
    )
    .bind(-424_i64)
    .bind(116_845_078_118_405_101_i64)
    .bind(ALICE)
    .execute(&pool)
    .await?;

    if !report_ids.is_empty() {
        if !forwarded_report_uris.is_empty() {
            sqlx::query(
                "DELETE FROM rustodon.outbox_events \
                 WHERE kind = $1 AND payload -> 'arguments' -> 'body' ->> 'id' = ANY($2)",
            )
            .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
            .bind(&forwarded_report_uris)
            .execute(&pool)
            .await?;
        }
        sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND payload -> 'arguments' ->> 'activity_id' = ANY($2)")
            .bind(NOTIFICATION_CREATE_JOB_KIND)
            .bind(report_ids.iter().map(ToString::to_string).collect::<Vec<_>>())
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM collection_reports WHERE report_id = ANY($1)")
            .bind(&report_ids)
            .execute(&pool)
            .await?;
        sqlx::query(
            "DELETE FROM admin_action_logs WHERE target_type = 'Report' AND target_id = ANY($1)",
        )
        .bind(&report_ids)
        .execute(&pool)
        .await?;
        sqlx::query("DELETE FROM reports WHERE id = ANY($1)")
            .bind(&report_ids)
            .execute(&pool)
            .await?;
    }
    if !admin_deleted_status_ids.is_empty() {
        sqlx::query(
            "DELETE FROM rustodon.outbox_events \
             WHERE payload -> 'arguments' ->> 'status_id' = ANY($1)",
        )
        .bind(
            admin_deleted_status_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "DELETE FROM admin_action_logs WHERE target_type = 'Status' AND target_id = ANY($1)",
        )
        .bind(&admin_deleted_status_ids)
        .execute(&pool)
        .await?;
        sqlx::query("DELETE FROM status_pins WHERE status_id = ANY($1)")
            .bind(&admin_deleted_status_ids)
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM statuses_tags WHERE status_id = ANY($1)")
            .bind(&admin_deleted_status_ids)
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM mentions WHERE status_id = ANY($1)")
            .bind(&admin_deleted_status_ids)
            .execute(&pool)
            .await?;
        sqlx::query(
            "DELETE FROM notifications WHERE activity_type = 'Status' AND activity_id = ANY($1)",
        )
        .bind(&admin_deleted_status_ids)
        .execute(&pool)
        .await?;
        sqlx::query("DELETE FROM status_stats WHERE status_id = ANY($1)")
            .bind(&admin_deleted_status_ids)
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM conversations WHERE parent_status_id = ANY($1)")
            .bind(&admin_deleted_status_ids)
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
            .bind(&admin_deleted_status_ids)
            .execute(&pool)
            .await?;
    }
    sqlx::query(
        "DELETE FROM admin_action_logs \
         WHERE action = 'reconcile_account_stats' AND target_type = 'Account' AND target_id = $1",
    )
    .bind(CAROL)
    .execute(&pool)
    .await?;
    result
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_applies_account_and_domain_moderation_transactionally()
-> Result<(), Box<dyn Error>> {
    const MODERATOR_ROLE: i64 = 92;
    const MODERATION_DOMAIN: &str = "moderation-block.fixture.invalid";
    const ORIGIN: &str = "https://fixture-v4-6-5.rustodon.invalid/";
    const DOMAIN_REMOTE_ACCOUNT: i64 = -334;
    const DOMAIN_REMOTE_STATS: i64 = -334;
    let moderation_account_domain = format!("{MODERATION_DOMAIN}:8443");

    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer_url = std::env::var("RUSTODON_MASTODON_WRITER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_WRITER_DATABASE_URL");
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let mut operational_connection = PgConnection::connect(&owner_url).await?;
    migrate(&mut operational_connection).await?;
    for statement in [
        "GRANT USAGE ON SCHEMA rustodon TO rustodon_differential_writer",
        "GRANT SELECT, DELETE ON TABLE rustodon.durable_jobs TO rustodon_differential_writer",
        "GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE rustodon.idempotency_keys, rustodon.outbox_events, rustodon.ordering_markers TO rustodon_differential_writer",
        "GRANT USAGE, SELECT ON SEQUENCE rustodon.outbox_events_id_seq TO rustodon_differential_writer",
    ] {
        sqlx::query(statement).execute(&pool).await?;
    }
    let writer = WriteRepository::connect(&writer_url).await?;
    let domain_remote_uri = format!("https://{moderation_account_domain}/users/domain-block");
    sqlx::query(
        "INSERT INTO accounts (id, actor_type, domain, username, uri, created_at, updated_at)
         VALUES ($1, 'Person', $2, 'domain_block', $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(DOMAIN_REMOTE_ACCOUNT)
    .bind(&moderation_account_domain)
    .bind(&domain_remote_uri)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO account_stats (id, account_id, created_at, updated_at)
         VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
    )
    .bind(DOMAIN_REMOTE_STATS)
    .bind(DOMAIN_REMOTE_ACCOUNT)
    .execute(&pool)
    .await?;
    sqlx::query(
        "UPDATE accounts SET
             avatar_content_type = 'image/png', avatar_file_name = 'domain-avatar.png',
             avatar_file_size = 12, avatar_storage_schema_version = 1,
             avatar_updated_at = clock_timestamp(), header_content_type = 'image/png',
             header_file_name = 'domain-header.png', header_file_size = 24,
             header_storage_schema_version = 1, header_updated_at = clock_timestamp()
           WHERE id = $1",
    )
    .bind(DOMAIN_REMOTE_ACCOUNT)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO media_attachments (
             id, account_id, type, processing, file_content_type, file_file_name,
             file_file_size, file_storage_schema_version, file_updated_at,
             thumbnail_content_type, thumbnail_file_name, thumbnail_file_size,
             thumbnail_storage_schema_version, thumbnail_updated_at, remote_url,
             created_at, updated_at)
         VALUES ($1, $2, 0, 2, 'image/png', 'domain-media.png', 36, 1,
                 clock_timestamp(), 'image/png', 'domain-media-thumb.png', 18, 1,
                 clock_timestamp(), 'https://media.example.invalid/domain-media.png',
                 clock_timestamp(), clock_timestamp())",
    )
    .bind(DOMAIN_REMOTE_STATS)
    .bind(DOMAIN_REMOTE_ACCOUNT)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO custom_emojis (
             id, domain, shortcode, image_content_type, image_file_name, image_file_size,
             image_storage_schema_version, image_remote_url, image_updated_at,
             created_at, updated_at)
         VALUES ($1, $2, 'domain_block', 'image/png', 'domain-emoji.png', 8, 1,
                 'https://media.example.invalid/domain-emoji.png', clock_timestamp(),
                 clock_timestamp(), clock_timestamp())",
    )
    .bind(DOMAIN_REMOTE_STATS)
    .bind(MODERATION_DOMAIN)
    .execute(&pool)
    .await?;
    let original_account: (Option<NaiveDateTime>, Option<i32>, NaiveDateTime) = sqlx::query_as(
        "SELECT suspended_at, suspension_origin, updated_at FROM accounts WHERE id = $1",
    )
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;
    let original_permissions: i64 =
        sqlx::query_scalar("SELECT permissions FROM user_roles WHERE id = $1")
            .bind(MODERATOR_ROLE)
            .fetch_one(&pool)
            .await?;
    let original_deletion_requests: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_deletion_requests WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    let original_email_blocks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM canonical_email_blocks WHERE reference_account_id = $1",
    )
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;
    let original_warning_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM account_warnings WHERE target_account_id = $1 ORDER BY id",
    )
    .bind(ALICE)
    .fetch_all(&pool)
    .await?;
    let original_account_update_event_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM rustodon.outbox_events \
         WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2 ORDER BY id",
    )
    .bind(ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND)
    .bind(ALICE.to_string())
    .fetch_all(&pool)
    .await?;
    let original_account_purge_event_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM rustodon.outbox_events \
         WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2 ORDER BY id",
    )
    .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
    .bind(ALICE.to_string())
    .fetch_all(&pool)
    .await?;
    let original_account_delete_event_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM rustodon.outbox_events \
         WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2 ORDER BY id",
    )
    .bind(ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND)
    .bind(ALICE.to_string())
    .fetch_all(&pool)
    .await?;
    let original_report_states: Vec<(i64, Option<NaiveDateTime>, Option<i64>, NaiveDateTime)> =
        sqlx::query_as(
            "SELECT id, action_taken_at, action_taken_by_account_id, updated_at \
             FROM reports WHERE target_account_id = $1 ORDER BY id",
        )
        .bind(ALICE)
        .fetch_all(&pool)
        .await?;
    let original_account_audit_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM admin_action_logs \
         WHERE target_type = 'Account' AND target_id = $1 ORDER BY id",
    )
    .bind(ALICE)
    .fetch_all(&pool)
    .await?;
    let moderation_report_id: i64 = sqlx::query_scalar(
        "INSERT INTO reports (account_id, category, comment, created_at, forwarded, status_ids, \
                 target_account_id, updated_at) \
         VALUES ($1, 0, 'moderation fixture report', clock_timestamp(), false, '{}', $2, clock_timestamp()) \
         RETURNING id",
    )
    .bind(MODERATOR)
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;
    let mut domain_block_ids = Vec::new();
    let mut moderation_warning_ids: Vec<i64> = Vec::new();
    let mut moderation_report_ids = Vec::new();

    let result = async {
        let self_suspend = writer
            .set_account_suspension(
                MODERATOR,
                MODERATOR,
                true,
                "https://fixture-v4-6-5.rustodon.invalid",
            )
            .await;
        assert!(
            matches!(self_suspend, Err(WriteError::Unauthorized)),
            "self suspension result: {self_suspend:?}"
        );
        let unauthorized_suspend = writer
            .set_account_suspension(
                NEWBIE,
                ALICE,
                true,
                "https://fixture-v4-6-5.rustodon.invalid",
            )
            .await;
        assert!(matches!(
            unauthorized_suspend,
            Err(WriteError::Unauthorized)
        ));
        writer
            .set_account_suspension(
                MODERATOR,
                ALICE,
                true,
                "https://fixture-v4-6-5.rustodon.invalid",
            )
            .await?;
        let suspended: (Option<NaiveDateTime>, Option<i32>) =
            sqlx::query_as("SELECT suspended_at, suspension_origin FROM accounts WHERE id = $1")
                .bind(ALICE)
                .fetch_one(&pool)
                .await?;
        assert!(suspended.0.is_some());
        assert_eq!(suspended.1, Some(0));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_deletion_requests WHERE account_id = $1",
            )
            .bind(ALICE)
            .fetch_one(&pool)
            .await?,
            original_deletion_requests + 1
        );
        let (deletion_created_at, purge_run_at): (NaiveDateTime, DateTime<Utc>) = sqlx::query_as(
            "SELECT request.created_at, (event.payload ->> 'run_at')::timestamptz
               FROM account_deletion_requests request
               JOIN rustodon.outbox_events event
                 ON event.kind = $2
                AND event.payload -> 'arguments' ->> 'account_id' = $3
              WHERE request.account_id = $1
              ORDER BY event.id DESC
              LIMIT 1",
        )
        .bind(ALICE)
        .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
        .bind(ALICE.to_string())
        .fetch_one(&pool)
        .await?;
        let expected_purge_at =
            deletion_created_at.and_utc() + Duration::days(ACCOUNT_DELETION_DELAY_DAYS);
        assert!(purge_run_at >= expected_purge_at);
        assert!(purge_run_at <= expected_purge_at + Duration::seconds(1));
        let (delete_actor_uri, delete_run_at): (String, DateTime<Utc>) = sqlx::query_as(
            "SELECT event.payload -> 'arguments' ->> 'actor_uri',
                    (event.payload ->> 'run_at')::timestamptz
               FROM rustodon.outbox_events event
              WHERE event.kind = $1
                AND event.payload -> 'arguments' ->> 'account_id' = $2
              ORDER BY event.id DESC
              LIMIT 1",
        )
        .bind(ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND)
        .bind(ALICE.to_string())
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            delete_actor_uri,
            "https://fixture-v4-6-5.rustodon.invalid/users/alice"
        );
        assert!(delete_run_at >= expected_purge_at);
        assert!(delete_run_at <= expected_purge_at + Duration::seconds(1));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM canonical_email_blocks WHERE reference_account_id = $1",
            )
            .bind(ALICE)
            .fetch_one(&pool)
            .await?,
            original_email_blocks + 1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_warnings WHERE target_account_id = $1",
            )
            .bind(ALICE)
            .fetch_one(&pool)
            .await?,
            i64::try_from(original_warning_ids.len()).unwrap() + 1
        );
        let warning: (i64, i64, i32, i64, String) = sqlx::query_as(
            "SELECT id, account_id, action, target_account_id, text \
             FROM account_warnings WHERE target_account_id = $1 ORDER BY id DESC LIMIT 1",
        )
        .bind(ALICE)
        .fetch_one(&pool)
        .await?;
        assert_eq!(warning.1, MODERATOR);
        assert_eq!(warning.2, 4000);
        assert_eq!(warning.3, ALICE);
        assert!(warning.4.is_empty());
        moderation_warning_ids.push(warning.0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events \
                 WHERE kind = $1 \
                   AND payload -> 'arguments' ->> 'recipient_account_id' = $2 \
                   AND payload -> 'arguments' ->> 'activity_type' = 'AccountWarning' \
                   AND payload -> 'arguments' ->> 'activity_id' = $3",
            )
            .bind(NOTIFICATION_CREATE_JOB_KIND)
            .bind(ALICE.to_string())
            .bind(warning.0.to_string())
            .fetch_one(&pool)
            .await?,
            1
        );
        let report_state: (Option<NaiveDateTime>, Option<i64>) = sqlx::query_as(
            "SELECT action_taken_at, action_taken_by_account_id \
             FROM reports WHERE id = $1",
        )
        .bind(moderation_report_id)
        .fetch_one(&pool)
        .await?;
        assert!(report_state.0.is_some());
        assert_eq!(report_state.1, Some(MODERATOR));
        let suspend_log: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT action, human_identifier, route_param FROM admin_action_logs \
             WHERE target_type = 'Account' AND target_id = $1 ORDER BY id DESC LIMIT 1",
        )
        .bind(ALICE)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            suspend_log,
            ("suspend".to_owned(), Some("alice".to_owned()), None)
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events \
                 WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2",
            )
            .bind(ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND)
            .bind(ALICE.to_string())
            .fetch_one(&pool)
            .await?,
            1
        );

        let unauthorized_unsuspend = writer
            .set_account_suspension(
                NEWBIE,
                ALICE,
                false,
                "https://fixture-v4-6-5.rustodon.invalid",
            )
            .await;
        assert!(matches!(
            unauthorized_unsuspend,
            Err(WriteError::Unauthorized)
        ));
        writer
            .set_account_suspension(
                MODERATOR,
                ALICE,
                false,
                "https://fixture-v4-6-5.rustodon.invalid",
            )
            .await?;
        let unsuspended: (Option<NaiveDateTime>, Option<i32>) =
            sqlx::query_as("SELECT suspended_at, suspension_origin FROM accounts WHERE id = $1")
                .bind(ALICE)
                .fetch_one(&pool)
                .await?;
        assert_eq!(unsuspended, (None, None));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_deletion_requests WHERE account_id = $1",
            )
            .bind(ALICE)
            .fetch_one(&pool)
            .await?,
            original_deletion_requests
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events \
                 WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2",
            )
            .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
            .bind(ALICE.to_string())
            .fetch_one(&pool)
            .await?,
            i64::try_from(original_account_purge_event_ids.len()).unwrap()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events \
                 WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2",
            )
            .bind(ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND)
            .bind(ALICE.to_string())
            .fetch_one(&pool)
            .await?,
            i64::try_from(original_account_delete_event_ids.len()).unwrap()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM canonical_email_blocks WHERE reference_account_id = $1",
            )
            .bind(ALICE)
            .fetch_one(&pool)
            .await?,
            original_email_blocks
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM account_warnings WHERE target_account_id = $1",
            )
            .bind(ALICE)
            .fetch_one(&pool)
            .await?,
            i64::try_from(original_warning_ids.len()).unwrap() + 1
        );
        let report_state: (Option<NaiveDateTime>, Option<i64>) = sqlx::query_as(
            "SELECT action_taken_at, action_taken_by_account_id \
             FROM reports WHERE id = $1",
        )
        .bind(moderation_report_id)
        .fetch_one(&pool)
        .await?;
        assert!(report_state.0.is_some());
        assert_eq!(report_state.1, Some(MODERATOR));
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT action FROM admin_action_logs \
                 WHERE target_type = 'Account' AND target_id = $1 ORDER BY id DESC LIMIT 1",
            )
            .bind(ALICE)
            .fetch_one(&pool)
            .await?,
            "unsuspend"
        );

        sqlx::query("UPDATE user_roles SET permissions = permissions | $1 WHERE id = $2")
            .bind(1_i64 << 5)
            .bind(MODERATOR_ROLE)
            .execute(&pool)
            .await?;
        sqlx::query("UPDATE accounts SET domain = $2 WHERE id = $1")
            .bind(DOMAIN_REMOTE_ACCOUNT)
            .bind(&moderation_account_domain)
            .execute(&pool)
            .await?;
        let unauthorized_block = writer
            .set_domain_block(NEWBIE, MODERATION_DOMAIN, 1, true, false, ORIGIN)
            .await;
        assert!(matches!(unauthorized_block, Err(WriteError::Unauthorized)));
        let domain_block_id = writer
            .set_domain_block(
                MODERATOR,
                "Moderation-Block.Fixture.Invalid.",
                0,
                false,
                false,
                ORIGIN,
            )
            .await?;
        domain_block_ids.push(domain_block_id);
        let domain_row: (String, i32, bool, bool, NaiveDateTime) = sqlx::query_as(
            "SELECT domain, severity, reject_media, reject_reports, created_at \
             FROM domain_blocks WHERE id = $1",
        )
        .bind(domain_block_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            (
                domain_row.0.clone(),
                domain_row.1,
                domain_row.2,
                domain_row.3
            ),
            (MODERATION_DOMAIN.to_owned(), 0, false, false)
        );
        let domain_matches: i64 =
            sqlx::query_scalar("SELECT count(*) FROM accounts WHERE id = $1 AND domain = $2")
                .bind(DOMAIN_REMOTE_ACCOUNT)
                .bind(&moderation_account_domain)
                .fetch_one(&pool)
                .await?;
        assert_eq!(domain_matches, 1);
        assert_eq!(
            sqlx::query_as::<_, (Option<NaiveDateTime>, Option<NaiveDateTime>)>(
                "SELECT silenced_at, suspended_at FROM accounts WHERE id = $1",
            )
            .bind(DOMAIN_REMOTE_ACCOUNT)
            .fetch_one(&pool)
            .await?,
            (Some(domain_row.4), None)
        );
        let create_log: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT action, human_identifier, route_param FROM admin_action_logs \
             WHERE target_type = 'DomainBlock' AND target_id = $1 ORDER BY id DESC LIMIT 1",
        )
        .bind(domain_block_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            create_log,
            (
                "create".to_owned(),
                Some(MODERATION_DOMAIN.to_owned()),
                None
            )
        );

        let report_authenticator = BearerAuthenticator::new(Repository::connect(&owner_url).await?);
        let report_authenticated = report_authenticator
            .authenticate(
                &bearer_headers("Bearer fixture-bearer-token-v4-6-5"),
                WRITE_REPORTS,
            )
            .await?;
        writer
            .set_domain_block(MODERATOR, MODERATION_DOMAIN, 0, true, true, ORIGIN)
            .await?;
        assert_eq!(
            sqlx::query_as::<
                _,
                (
                    Option<String>,
                    Option<String>,
                    Option<i32>,
                    Option<String>,
                    Option<String>,
                    Option<i32>
                ),
            >(
                "SELECT avatar_file_name, avatar_content_type, avatar_file_size,
                        header_file_name, header_content_type, header_file_size
                   FROM accounts WHERE id = $1",
            )
            .bind(DOMAIN_REMOTE_ACCOUNT)
            .fetch_one(&pool)
            .await?,
            (
                Some("domain-avatar.png".to_owned()),
                Some("image/png".to_owned()),
                Some(12),
                Some("domain-header.png".to_owned()),
                Some("image/png".to_owned()),
                Some(24)
            )
        );
        assert_eq!(
            sqlx::query_as::<
                _,
                (
                    Option<String>,
                    Option<String>,
                    Option<i32>,
                    Option<String>,
                    Option<String>,
                    Option<i32>
                ),
            >(
                "SELECT file_file_name, file_content_type, file_file_size,
                        thumbnail_file_name, thumbnail_content_type, thumbnail_file_size
                   FROM media_attachments WHERE id = $1",
            )
            .bind(DOMAIN_REMOTE_STATS)
            .fetch_one(&pool)
            .await?,
            (
                Some("domain-media.png".to_owned()),
                Some("image/png".to_owned()),
                Some(36),
                Some("domain-media-thumb.png".to_owned()),
                Some("image/png".to_owned()),
                Some(18)
            )
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM custom_emojis WHERE id = $1",)
                .bind(DOMAIN_REMOTE_STATS)
                .fetch_one(&pool)
                .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE kind = $1 AND payload -> 'arguments' ->> 'domain_block_id' = $2",
            )
            .bind(MASTODON_DOMAIN_BLOCK_JOB_KIND)
            .bind(domain_block_id.to_string())
            .fetch_one(&pool)
            .await?,
            1
        );
        let local_report = writer
            .create_report(
                &report_authenticated,
                DOMAIN_REMOTE_ACCOUNT,
                "local report remains accepted",
                None,
                &[],
                &[],
                &[],
                None,
                None,
                "https://fixture-v4-6-5.rustodon.invalid/",
                false,
            )
            .await?;
        moderation_report_ids.push(local_report);

        assert_eq!(
            writer
                .set_domain_block(MODERATOR, MODERATION_DOMAIN, 1, false, true, ORIGIN)
                .await?,
            domain_block_id
        );
        assert_eq!(
            sqlx::query_as::<_, (i32, bool, bool)>(
                "SELECT severity, reject_media, reject_reports \
                 FROM domain_blocks WHERE id = $1",
            )
            .bind(domain_block_id)
            .fetch_one(&pool)
            .await?,
            (1, false, true)
        );
        assert_eq!(
            sqlx::query_as::<_, (Option<NaiveDateTime>, Option<NaiveDateTime>)>(
                "SELECT silenced_at, suspended_at FROM accounts WHERE id = $1",
            )
            .bind(DOMAIN_REMOTE_ACCOUNT)
            .fetch_one(&pool)
            .await?,
            (None, Some(domain_row.4))
        );

        sqlx::query(
            "UPDATE domain_blocks SET severity = NULL, reject_reports = false WHERE id = $1",
        )
        .bind(domain_block_id)
        .execute(&pool)
        .await?;
        assert_eq!(
            writer
                .set_domain_block(MODERATOR, MODERATION_DOMAIN, 1, false, true, ORIGIN)
                .await?,
            domain_block_id
        );
        assert_eq!(
            sqlx::query_as::<_, (i32, bool, bool)>(
                "SELECT severity, reject_media, reject_reports \
                 FROM domain_blocks WHERE id = $1",
            )
            .bind(domain_block_id)
            .fetch_one(&pool)
            .await?,
            (1, false, true)
        );

        let unauthorized_unblock = writer.unblock_domain(NEWBIE, MODERATION_DOMAIN).await;
        assert!(matches!(
            unauthorized_unblock,
            Err(WriteError::Unauthorized)
        ));
        writer.unblock_domain(MODERATOR, MODERATION_DOMAIN).await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM domain_blocks WHERE id = $1")
                .bind(domain_block_id)
                .fetch_one(&pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT action FROM admin_action_logs \
                 WHERE target_type = 'DomainBlock' AND target_id = $1 ORDER BY id DESC LIMIT 1",
            )
            .bind(domain_block_id)
            .fetch_one(&pool)
            .await?,
            "destroy"
        );
        assert_eq!(
            sqlx::query_as::<_, (Option<NaiveDateTime>, Option<NaiveDateTime>)>(
                "SELECT silenced_at, suspended_at FROM accounts WHERE id = $1",
            )
            .bind(DOMAIN_REMOTE_ACCOUNT)
            .fetch_one(&pool)
            .await?,
            (None, None)
        );
        sqlx::query(
            "UPDATE accounts SET suspended_at = clock_timestamp(), suspension_origin = 0 \
             WHERE id = $1",
        )
        .bind(DOMAIN_REMOTE_ACCOUNT)
        .execute(&pool)
        .await?;
        let remote_unsuspend = writer
            .set_account_suspension(
                MODERATOR,
                DOMAIN_REMOTE_ACCOUNT,
                false,
                "https://fixture-v4-6-5.rustodon.invalid",
            )
            .await;
        assert!(matches!(remote_unsuspend, Err(WriteError::InvalidInput(_))));
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    sqlx::query(
        "UPDATE accounts SET suspended_at = $2, suspension_origin = $3, updated_at = $4 \
         WHERE id = $1",
    )
    .bind(ALICE)
    .bind(original_account.0)
    .bind(original_account.1)
    .bind(original_account.2)
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE user_roles SET permissions = $1 WHERE id = $2")
        .bind(original_permissions)
        .bind(MODERATOR_ROLE)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM media_attachments WHERE id = $1")
        .bind(DOMAIN_REMOTE_STATS)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM custom_emojis WHERE id = $1")
        .bind(DOMAIN_REMOTE_STATS)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(DOMAIN_REMOTE_ACCOUNT)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(DOMAIN_REMOTE_ACCOUNT)
        .execute(&pool)
        .await?;
    for (report_id, action_taken_at, action_taken_by_account_id, updated_at) in
        original_report_states
    {
        sqlx::query(
            "UPDATE reports SET action_taken_at = $2, action_taken_by_account_id = $3, \
                    updated_at = $4 WHERE id = $1",
        )
        .bind(report_id)
        .bind(action_taken_at)
        .bind(action_taken_by_account_id)
        .bind(updated_at)
        .execute(&pool)
        .await?;
    }
    sqlx::query("DELETE FROM reports WHERE id = $1")
        .bind(moderation_report_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM admin_action_logs WHERE target_type = 'Report' AND target_id = $1")
        .bind(moderation_report_id)
        .execute(&pool)
        .await?;
    if !moderation_report_ids.is_empty() {
        sqlx::query(
            "DELETE FROM rustodon.outbox_events \
             WHERE kind = $1 AND payload -> 'arguments' ->> 'activity_id' = ANY($2)",
        )
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(
            moderation_report_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
        .execute(&pool)
        .await?;
        sqlx::query("DELETE FROM collection_reports WHERE report_id = ANY($1)")
            .bind(&moderation_report_ids)
            .execute(&pool)
            .await?;
        sqlx::query(
            "DELETE FROM admin_action_logs WHERE target_type = 'Report' AND target_id = ANY($1)",
        )
        .bind(&moderation_report_ids)
        .execute(&pool)
        .await?;
        sqlx::query("DELETE FROM reports WHERE id = ANY($1)")
            .bind(&moderation_report_ids)
            .execute(&pool)
            .await?;
    }
    sqlx::query("DELETE FROM account_warnings WHERE target_account_id = $1 AND NOT (id = ANY($2))")
        .bind(ALICE)
        .bind(&original_warning_ids)
        .execute(&pool)
        .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
         WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2 \
           AND NOT (id = ANY($3))",
    )
    .bind(ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND)
    .bind(ALICE.to_string())
    .bind(&original_account_update_event_ids)
    .execute(&pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
         WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2 \
           AND NOT (id = ANY($3))",
    )
    .bind(ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND)
    .bind(ALICE.to_string())
    .bind(&original_account_delete_event_ids)
    .execute(&pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
         WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2 \
           AND NOT (id = ANY($3))",
    )
    .bind(MASTODON_ACCOUNT_PURGE_JOB_KIND)
    .bind(ALICE.to_string())
    .bind(&original_account_purge_event_ids)
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM domain_blocks WHERE domain = $1 AND id = ANY($2)")
        .bind(MODERATION_DOMAIN)
        .bind(&domain_block_ids)
        .execute(&pool)
        .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE kind = $1 AND payload -> 'arguments' ->> 'domain_block_id' = ANY($2)",
    )
    .bind(MASTODON_DOMAIN_BLOCK_JOB_KIND)
    .bind(
        domain_block_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    )
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM relationship_severance_events WHERE target_name = $1")
        .bind(MODERATION_DOMAIN)
        .execute(&pool)
        .await?;
    sqlx::query(
        "DELETE FROM admin_action_logs \
         WHERE (target_type = 'Account' AND target_id = $1 AND NOT (id = ANY($3))) \
            OR (target_type = 'DomainBlock' AND target_id = ANY($2))",
    )
    .bind(ALICE)
    .bind(&domain_block_ids)
    .bind(&original_account_audit_ids)
    .execute(&pool)
    .await?;
    if !moderation_warning_ids.is_empty() {
        sqlx::query(
            "DELETE FROM rustodon.outbox_events \
             WHERE kind = $1 AND payload -> 'arguments' ->> 'activity_type' = 'AccountWarning' \
               AND payload -> 'arguments' ->> 'activity_id' = ANY($2)",
        )
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(
            moderation_warning_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
        .execute(&pool)
        .await?;
    }
    result
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_updates_account_profile_and_user_settings_transactionally()
-> Result<(), Box<dyn std::error::Error>> {
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events WHERE kind = $1 \
         AND payload -> 'arguments' ->> 'account_id' = $2",
    )
    .bind(ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND)
    .bind(ALICE.to_string())
    .execute(&pool)
    .await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_ACCOUNTS).await?;
    let before: AccountProfileSchemaState = sqlx::query_as(
        "SELECT display_name, note, actor_type, locked, discoverable, hide_collections, \
                indexable, attribution_domains, fields, updated_at \
         FROM accounts WHERE id = $1",
    )
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;
    let user_before: (Option<String>, NaiveDateTime) =
        sqlx::query_as("SELECT settings, updated_at FROM users WHERE id = $1")
            .bind(101_i64)
            .fetch_one(&pool)
            .await?;
    let account_tags_before: Vec<(i64, i64)> =
        sqlx::query_as("SELECT account_id, tag_id FROM accounts_tags WHERE account_id = $1")
            .bind(ALICE)
            .fetch_all(&pool)
            .await?;
    let media_before: AccountMediaSchemaState = sqlx::query_as(
        "SELECT avatar_content_type, avatar_description, avatar_file_name, avatar_file_size, \
                avatar_remote_url, avatar_storage_schema_version, avatar_updated_at, \
                header_content_type, header_description, header_file_name, header_file_size, \
                header_remote_url, header_storage_schema_version, header_updated_at \
         FROM accounts WHERE id = $1",
    )
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;

    writer
        .update_account_profile(
            &authenticated,
            &AccountProfileUpdate {
                display_name: Some("Profile write fixture".to_owned()),
                note: Some("Profile update note".to_owned()),
                avatar_description: None,
                header_description: None,
                avatar: AccountMediaUpdate::Unchanged,
                header: AccountMediaUpdate::Unchanged,
                bot: AccountProfileValue::Value(true),
                locked: Some(false),
                discoverable: AccountProfileValue::Value(true),
                hide_collections: AccountProfileValue::Value(false),
                indexable: Some(true),
                attribution_domains: Some(vec![
                    "https://example.com".to_owned(),
                    "example.com".to_owned(),
                    "*.example.org".to_owned(),
                ]),
                fields: Some(vec![
                    AccountFieldUpdate {
                        name: "Website".to_owned(),
                        value: "https://example.com".to_owned(),
                    },
                    AccountFieldUpdate {
                        name: "Pronouns".to_owned(),
                        value: "they/them".to_owned(),
                    },
                ]),
                source: Some(AccountSourceUpdate {
                    privacy: AccountProfileValue::Value("unlisted".to_owned()),
                    sensitive: AccountProfileValue::Value(true),
                    language: AccountProfileValue::Value("fr".to_owned()),
                    quote_policy: AccountProfileValue::Value("followers".to_owned()),
                }),
            },
        )
        .await?;
    writer
        .update_account_profile(
            &authenticated,
            &AccountProfileUpdate {
                avatar_description: Some("Fixture avatar description".to_owned()),
                avatar: AccountMediaUpdate::Replace {
                    file_name: "fixture-avatar.png".to_owned(),
                    content_type: "image/png".to_owned(),
                    file_size: 3,
                    storage_schema_version: 1,
                },
                ..AccountProfileUpdate::default()
            },
        )
        .await?;
    let after: AccountProfileSchemaState = sqlx::query_as(
        "SELECT display_name, note, actor_type, locked, discoverable, hide_collections, \
                indexable, attribution_domains, fields, updated_at \
         FROM accounts WHERE id = $1",
    )
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;
    let user_after: (Option<String>, NaiveDateTime) =
        sqlx::query_as("SELECT settings, updated_at FROM users WHERE id = $1")
            .bind(101_i64)
            .fetch_one(&pool)
            .await?;
    let account_tags_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM accounts_tags WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    let media_after: AccountMediaSchemaState = sqlx::query_as(
        "SELECT avatar_content_type, avatar_description, avatar_file_name, avatar_file_size, \
                avatar_remote_url, avatar_storage_schema_version, avatar_updated_at, \
                header_content_type, header_description, header_file_name, header_file_size, \
                header_remote_url, header_storage_schema_version, header_updated_at \
         FROM accounts WHERE id = $1",
    )
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;

    sqlx::query(
        "UPDATE accounts SET display_name = $1, note = $2, actor_type = $3, locked = $4, \
            discoverable = $5, hide_collections = $6, indexable = $7, \
            attribution_domains = $8, fields = $9, updated_at = $10 WHERE id = $11",
    )
    .bind(&before.0)
    .bind(&before.1)
    .bind(&before.2)
    .bind(before.3)
    .bind(before.4)
    .bind(before.5)
    .bind(before.6)
    .bind(&before.7)
    .bind(&before.8)
    .bind(before.9)
    .bind(ALICE)
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE users SET settings = $1, updated_at = $2 WHERE id = $3")
        .bind(&user_before.0)
        .bind(user_before.1)
        .bind(101_i64)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM accounts_tags WHERE account_id = $1")
        .bind(ALICE)
        .execute(&pool)
        .await?;
    for (account_id, tag_id) in &account_tags_before {
        sqlx::query("INSERT INTO accounts_tags (account_id, tag_id) VALUES ($1, $2)")
            .bind(account_id)
            .bind(tag_id)
            .execute(&pool)
            .await?;
    }
    sqlx::query(
        "UPDATE accounts SET avatar_content_type = $1, avatar_description = $2, \
            avatar_file_name = $3, avatar_file_size = $4, avatar_remote_url = $5, \
            avatar_storage_schema_version = $6, avatar_updated_at = $7, \
            header_content_type = $8, header_description = $9, header_file_name = $10, \
            header_file_size = $11, header_remote_url = $12, \
            header_storage_schema_version = $13, header_updated_at = $14 WHERE id = $15",
    )
    .bind(&media_before.0)
    .bind(&media_before.1)
    .bind(&media_before.2)
    .bind(media_before.3)
    .bind(&media_before.4)
    .bind(media_before.5)
    .bind(media_before.6)
    .bind(&media_before.7)
    .bind(&media_before.8)
    .bind(&media_before.9)
    .bind(media_before.10)
    .bind(&media_before.11)
    .bind(media_before.12)
    .bind(media_before.13)
    .bind(ALICE)
    .execute(&pool)
    .await?;

    assert_eq!(after.0, "Profile write fixture");
    assert_eq!(after.1, "Profile update note");
    assert_eq!(after.2.as_deref(), Some("Service"));
    assert!(!after.3);
    assert_eq!(after.4, Some(true));
    assert_eq!(after.5, Some(false));
    assert!(after.6);
    assert_eq!(
        after.7,
        Some(vec!["example.com".to_owned(), "example.org".to_owned()])
    );
    assert_eq!(after.8.as_ref().unwrap()[0]["name"], "Website");
    assert_eq!(after.8.as_ref().unwrap()[1]["value"], "they/them");
    assert_eq!(account_tags_after, 0);
    let settings: Value = serde_json::from_str(user_after.0.as_deref().unwrap()).unwrap();
    assert_eq!(settings["default_privacy"], "unlisted");
    assert_eq!(settings["default_sensitive"], true);
    assert_eq!(settings["default_language"], "fr");
    assert_eq!(settings["default_quote_policy"], "followers");
    assert!(after.9 > before.9);
    assert!(user_after.1 > user_before.1);
    assert_eq!(media_after.0.as_deref(), Some("image/png"));
    assert_eq!(media_after.1, "Fixture avatar description");
    assert_eq!(media_after.2.as_deref(), Some("fixture-avatar.png"));
    assert_eq!(media_after.3, Some(3));
    assert_eq!(media_after.4, None);
    assert_eq!(media_after.5, Some(1));
    assert!(media_after.6 > media_before.6);
    let account_update_jobs: Vec<Value> = sqlx::query_scalar(
        "SELECT payload -> 'arguments' FROM rustodon.outbox_events
          WHERE kind = $1 AND payload -> 'arguments' ->> 'account_id' = $2
          ORDER BY id",
    )
    .bind(ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND)
    .bind(ALICE.to_string())
    .fetch_all(&pool)
    .await?;
    assert_eq!(account_update_jobs.len(), 2);
    assert!(
        account_update_jobs
            .iter()
            .all(|job| job["updated_at_micros"].as_i64().is_some())
    );
    sqlx::query(
        "DELETE FROM rustodon.outbox_events WHERE kind = $1 \
         AND payload -> 'arguments' ->> 'account_id' = $2",
    )
    .bind(ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND)
    .bind(ALICE.to_string())
    .execute(&pool)
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn write_repository_creates_updates_and_deletes_unattached_image_media()
-> Result<(), Box<dyn std::error::Error>> {
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let repository = Repository::connect(&database_url).await?;
    let authenticator = BearerAuthenticator::new(repository.clone());
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_MEDIA).await?;
    let id = writer
        .create_media_attachment(
            &authenticated,
            &MediaAttachmentCreate {
                file_name: "fixture-media.jpg".to_owned(),
                content_type: "image/jpeg".to_owned(),
                file_size: 123,
                file_meta: serde_json::json!({
                    "original": {"width": 600, "height": 400, "size": "600x400"},
                    "small": {"width": 588, "height": 392, "size": "588x392"},
                }),
                blurhash: Some("L00000000000000000000000000000000".to_owned()),
                description: Some("Created media".to_owned()),
                focus: AccountProfileValue::Unchanged,
            },
        )
        .await?;
    let created = repository
        .media_attachment(ALICE, id)
        .await?
        .expect("created media is visible to the read role");
    assert_eq!(created.account_id, Some(ALICE));
    assert_eq!(created.media_type.0, 0);
    assert_eq!(created.processing.map(|value| value.0), Some(2));
    assert_eq!(created.file_file_name.as_deref(), Some("fixture-media.jpg"));
    assert_eq!(
        created.blurhash.as_deref(),
        Some("L00000000000000000000000000000000")
    );

    writer
        .update_media_attachment(
            &authenticated,
            id,
            &MediaAttachmentUpdate {
                description: AccountProfileValue::Value("Updated media".to_owned()),
                focus: AccountProfileValue::Value(MediaFocus { x: 0.25, y: -0.5 }),
            },
        )
        .await?;
    let updated = repository
        .media_attachment(ALICE, id)
        .await?
        .expect("updated media is visible to the read role");
    assert_eq!(updated.description.as_deref(), Some("Updated media"));
    assert_eq!(updated.file_meta.as_ref().unwrap()["focus"]["x"], 0.25);
    assert_eq!(updated.file_meta.as_ref().unwrap()["focus"]["y"], -0.5);

    let attached = writer
        .delete_media_attachment(&authenticated, 116_844_842_188_806_001)
        .await
        .expect_err("status-attached media cannot be deleted");
    assert!(matches!(
        attached,
        WriteError::Validation("Media attachment is currently used by a status")
    ));

    let deleted = writer.delete_media_attachment(&authenticated, id).await?;
    assert_eq!(deleted.id, id);
    assert!(repository.media_attachment(ALICE, id).await?.is_none());
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_saves_status_interactions_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator
        .authenticate(&headers, WRITE_BOOKMARKS)
        .await?;
    let bookmark_status = PUBLIC_STATUS;
    let boost_status = 116_845_321_912_325_301_i64;
    let favourite_status = 116_845_078_118_405_101_i64;

    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM bookmarks WHERE account_id = $1 AND status_id = $2",
        )
        .bind(ALICE)
        .bind(bookmark_status)
        .fetch_one(&pool)
        .await?,
        0
    );
    writer
        .set_bookmark(&authenticated, bookmark_status, true)
        .await?;
    writer
        .set_bookmark(&authenticated, bookmark_status, true)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM bookmarks WHERE account_id = $1 AND status_id = $2",
        )
        .bind(ALICE)
        .bind(bookmark_status)
        .fetch_one(&pool)
        .await?,
        1
    );
    writer
        .set_bookmark(&authenticated, bookmark_status, false)
        .await?;
    writer
        .set_bookmark(&authenticated, boost_status, true)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM bookmarks WHERE account_id = $1 AND status_id = $2",
        )
        .bind(ALICE)
        .bind(PUBLIC_STATUS)
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM bookmarks WHERE account_id = $1 AND status_id = $2",
        )
        .bind(ALICE)
        .bind(boost_status)
        .fetch_one(&pool)
        .await?,
        0
    );
    let projected_boost = RestProjectionLoader::new(
        Repository::connect(&database_url).await?,
        Some(ALICE),
        "fixture-v4-6-5.rustodon.invalid",
    )
    .authorized_status(boost_status)
    .await?
    .expect("the boost should be projected for its viewer");
    let boost_viewer = projected_boost
        .viewer
        .as_ref()
        .expect("the boost should include viewer relationships");
    assert!(!boost_viewer.bookmarked);
    assert!(
        projected_boost
            .reblog
            .as_ref()
            .and_then(|status| status.viewer.as_ref())
            .is_some_and(|viewer| viewer.bookmarked)
    );
    writer
        .set_bookmark(&authenticated, boost_status, false)
        .await?;

    let authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let inaccessible_reply = writer
        .create_status(
            &authenticated,
            "inaccessible reply",
            &[],
            None,
            None,
            None,
            None,
            Some(-312),
            None,
        )
        .await
        .expect_err("an inaccessible private status cannot receive a reply");
    assert!(matches!(inaccessible_reply, WriteError::NotFound));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM statuses WHERE account_id = $1 AND text = $2",
        )
        .bind(ALICE)
        .bind("inaccessible reply")
        .fetch_one(&pool)
        .await?,
        0
    );
    let blocked_reblog = writer
        .set_reblog(&authenticated, -313, None, true)
        .await
        .expect_err("a blocked author cannot be reblogged");
    assert!(matches!(blocked_reblog, WriteError::NotFound));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM statuses \
             WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL",
        )
        .bind(ALICE)
        .bind(-313_i64)
        .fetch_one(&pool)
        .await?,
        0
    );
    sqlx::query(
        "INSERT INTO blocks (account_id, target_account_id, uri, created_at, updated_at) \
         VALUES ($1, $2, NULL, clock_timestamp(), clock_timestamp())",
    )
    .bind(MATRIX_VIEWER)
    .bind(ALICE)
    .execute(&pool)
    .await?;
    let reverse_blocked_reblog = writer
        .set_reblog(&authenticated, -415, None, true)
        .await
        .expect_err("an author who blocks the viewer cannot be reblogged");
    assert!(matches!(reverse_blocked_reblog, WriteError::NotFound));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM statuses \
             WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL",
        )
        .bind(ALICE)
        .bind(-415_i64)
        .fetch_one(&pool)
        .await?,
        0
    );
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2")
        .bind(MATRIX_VIEWER)
        .bind(ALICE)
        .execute(&pool)
        .await?;
    let before_reblogs: i64 =
        sqlx::query_scalar("SELECT reblogs_count FROM status_stats WHERE status_id = $1")
            .bind(PUBLIC_STATUS)
            .fetch_one(&pool)
            .await?;
    let before_statuses_count: i64 =
        sqlx::query_scalar("SELECT statuses_count FROM account_stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    let first_reblog = writer
        .set_reblog(&authenticated, PUBLIC_STATUS, None, true)
        .await?;
    let second_reblog = writer
        .set_reblog(&authenticated, PUBLIC_STATUS, None, true)
        .await?;
    assert!(first_reblog.created);
    assert!(!second_reblog.created);
    assert_eq!(first_reblog.status_id, second_reblog.status_id);
    assert_eq!(first_reblog.target_status_id, PUBLIC_STATUS);
    assert_eq!(first_reblog.recipient_account_id, ALICE);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events \
             WHERE kind = $1 AND logical_key = $2",
        )
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(format!(
            "notification:reblog:{ALICE}:{}",
            first_reblog.status_id
        ))
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM statuses \
             WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL",
        )
        .bind(ALICE)
        .bind(PUBLIC_STATUS)
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT visibility FROM statuses WHERE id = $1")
            .bind(first_reblog.status_id)
            .fetch_one(&pool)
            .await?,
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT reblogs_count FROM status_stats WHERE status_id = $1")
            .bind(PUBLIC_STATUS)
            .fetch_one(&pool)
            .await?,
        before_reblogs + 1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT statuses_count FROM account_stats WHERE account_id = $1"
        )
        .bind(ALICE)
        .fetch_one(&pool)
        .await?,
        before_statuses_count + 1
    );
    let first_unreblog = writer
        .set_reblog(&authenticated, PUBLIC_STATUS, None, false)
        .await?;
    let second_unreblog = writer
        .set_reblog(&authenticated, PUBLIC_STATUS, None, false)
        .await?;
    assert_eq!(first_unreblog.target_status_id, PUBLIC_STATUS);
    assert_eq!(second_unreblog.target_status_id, PUBLIC_STATUS);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM statuses \
             WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL",
        )
        .bind(ALICE)
        .bind(PUBLIC_STATUS)
        .fetch_one(&pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT reblogs_count FROM status_stats WHERE status_id = $1")
            .bind(PUBLIC_STATUS)
            .fetch_one(&pool)
            .await?,
        before_reblogs
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT statuses_count FROM account_stats WHERE account_id = $1"
        )
        .bind(ALICE)
        .fetch_one(&pool)
        .await?,
        before_statuses_count
    );
    let concurrent_writer = writer.clone();
    let (concurrent_first, concurrent_second) = tokio::join!(
        writer.set_reblog(&authenticated, PUBLIC_STATUS, None, true),
        concurrent_writer.set_reblog(&authenticated, PUBLIC_STATUS, None, true),
    );
    let concurrent_first = concurrent_first?;
    let concurrent_second = concurrent_second?;
    assert_ne!(concurrent_first.created, concurrent_second.created);
    assert_eq!(concurrent_first.status_id, concurrent_second.status_id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM statuses \
             WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL",
        )
        .bind(ALICE)
        .bind(PUBLIC_STATUS)
        .fetch_one(&pool)
        .await?,
        1
    );
    let generated_id_unreblog = writer
        .set_reblog(&authenticated, concurrent_first.status_id, None, false)
        .await?;
    assert!(!generated_id_unreblog.removed);
    assert_eq!(generated_id_unreblog.status_id, concurrent_first.status_id);
    assert_eq!(
        generated_id_unreblog.target_status_id,
        concurrent_first.status_id
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM statuses \
             WHERE account_id = $1 AND reblog_of_id = $2 AND deleted_at IS NULL",
        )
        .bind(ALICE)
        .bind(PUBLIC_STATUS)
        .fetch_one(&pool)
        .await?,
        1
    );
    writer
        .set_reblog(&authenticated, PUBLIC_STATUS, None, false)
        .await?;

    let authenticated = authenticator
        .authenticate(&headers, WRITE_FAVOURITES)
        .await?;
    let blocked_status = -313_i64;
    let before_blocked_favourite_count: i64 =
        sqlx::query_scalar("SELECT favourites_count FROM status_stats WHERE status_id = $1")
            .bind(blocked_status)
            .fetch_one(&pool)
            .await?;
    let blocked_favourite = writer
        .set_favourite(&authenticated, blocked_status, true)
        .await
        .expect_err("a blocked author cannot be favourited");
    assert!(matches!(blocked_favourite, WriteError::NotFound));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM favourites WHERE account_id = $1 AND status_id = $2",
        )
        .bind(ALICE)
        .bind(blocked_status)
        .fetch_one(&pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT favourites_count FROM status_stats WHERE status_id = $1",
        )
        .bind(blocked_status)
        .fetch_one(&pool)
        .await?,
        before_blocked_favourite_count
    );
    let boost_favourite = writer
        .set_favourite(&authenticated, boost_status, true)
        .await?;
    assert_eq!(boost_favourite.recipient_account_id, ALICE);
    assert!(boost_favourite.activity_id.is_some());
    let projected_boost = RestProjectionLoader::new(
        Repository::connect(&database_url).await?,
        Some(ALICE),
        "fixture-v4-6-5.rustodon.invalid",
    )
    .authorized_status(boost_status)
    .await?
    .expect("the boost should be projected for its viewer");
    assert!(
        !projected_boost
            .viewer
            .as_ref()
            .is_some_and(|viewer| viewer.favourited)
    );
    assert!(
        projected_boost
            .reblog
            .as_ref()
            .and_then(|status| status.viewer.as_ref())
            .is_some_and(|viewer| viewer.favourited)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM favourites WHERE account_id = $1 AND status_id = $2",
        )
        .bind(ALICE)
        .bind(PUBLIC_STATUS)
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM favourites WHERE account_id = $1 AND status_id = $2",
        )
        .bind(ALICE)
        .bind(boost_status)
        .fetch_one(&pool)
        .await?,
        0
    );
    writer
        .set_favourite(&authenticated, boost_status, false)
        .await?;
    let before_count: i64 =
        sqlx::query_scalar("SELECT favourites_count FROM status_stats WHERE status_id = $1")
            .bind(favourite_status)
            .fetch_one(&pool)
            .await?;
    let first = writer
        .set_favourite(&authenticated, favourite_status, true)
        .await?;
    let second = writer
        .set_favourite(&authenticated, favourite_status, true)
        .await?;
    assert!(first.activity_id.is_some());
    assert!(second.activity_id.is_none());
    let favourite_activity_id = first.activity_id.expect("favourite activity is present");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events \
             WHERE kind = $1 AND logical_key = $2",
        )
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(format!(
            "notification:favourite:{}:{favourite_activity_id}",
            first.recipient_account_id
        ))
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM favourites WHERE account_id = $1 AND status_id = $2",
        )
        .bind(ALICE)
        .bind(favourite_status)
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT favourites_count FROM status_stats WHERE status_id = $1",
        )
        .bind(favourite_status)
        .fetch_one(&pool)
        .await?,
        before_count + 1
    );
    writer
        .set_favourite(&authenticated, favourite_status, false)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT favourites_count FROM status_stats WHERE status_id = $1",
        )
        .bind(favourite_status)
        .fetch_one(&pool)
        .await?,
        before_count
    );

    let domain_block_status_stats: Value =
        sqlx::query_scalar("SELECT to_jsonb(row) FROM status_stats row WHERE status_id = $1")
            .bind(favourite_status)
            .fetch_one(&pool)
            .await?;
    sqlx::query(
        "INSERT INTO favourites (account_id, status_id, created_at, updated_at) \
         VALUES ($1, $2, clock_timestamp(), clock_timestamp()) \
         ON CONFLICT (account_id, status_id) DO NOTHING",
    )
    .bind(ALICE)
    .bind(favourite_status)
    .execute(&pool)
    .await?;
    let domain_block_id: i64 = sqlx::query_scalar(
        "INSERT INTO domain_blocks (domain, severity, reject_media, reject_reports, \
         obfuscate, private_comment, public_comment, created_at, updated_at) \
         VALUES ($1, 1, false, false, false, NULL, NULL, clock_timestamp(), clock_timestamp()) \
         RETURNING id",
    )
    .bind("remote.fixture.invalid")
    .fetch_one(&pool)
    .await?;
    let domain_block_removal = writer
        .set_favourite_with_origin(
            &authenticated,
            favourite_status,
            false,
            Some("https://fixture-v4-6-5.rustodon.invalid/"),
            false,
        )
        .await;
    sqlx::query("DELETE FROM domain_blocks WHERE id = $1")
        .bind(domain_block_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM favourites WHERE account_id = $1 AND status_id = $2")
        .bind(ALICE)
        .bind(favourite_status)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id = $1")
        .bind(favourite_status)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO status_stats SELECT * FROM jsonb_populate_record(NULL::status_stats, $1)",
    )
    .bind(domain_block_status_stats)
    .execute(&pool)
    .await?;
    let domain_block_removal = domain_block_removal?;
    assert!(domain_block_removal.activity_id.is_some());
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_streams_reblog_lifecycle_to_visible_followers()
-> Result<(), Box<dyn Error>> {
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let before_stats: Value =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    let before_show_reblogs: bool = sqlx::query_scalar(
        "SELECT show_reblogs FROM follows
          WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(MODERATOR)
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;
    sqlx::query(
        "UPDATE follows SET show_reblogs = false
          WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(MODERATOR)
    .bind(ALICE)
    .execute(&pool)
    .await?;

    let boost = writer
        .set_reblog(&authenticated, PUBLIC_STATUS, None, true)
        .await?;
    assert!(boost.created);
    let boost_id = boost.status_id;
    let distribution_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events
          WHERE kind = $1
            AND payload -> 'arguments' ->> 'status_id' = $2
            AND payload -> 'arguments' ->> 'activity_type' = 'Create'",
    )
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(boost_id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(distribution_jobs, 1);
    let update_recipients = sqlx::query_scalar::<_, i64>(
        "SELECT (payload ->> 'account_id')::bigint
           FROM rustodon.outbox_events
          WHERE kind = $1
            AND payload ->> 'event' = 'update'
            AND payload ->> 'object_id' = $2
          ORDER BY (payload ->> 'account_id')::bigint",
    )
    .bind(STREAM_EVENT_KIND)
    .bind(boost_id.to_string())
    .fetch_all(&pool)
    .await?;
    assert_eq!(update_recipients, vec![ALICE, API_MODERATOR]);

    let removed = writer
        .set_reblog(&authenticated, PUBLIC_STATUS, None, false)
        .await?;
    assert!(removed.removed);
    let pending_boost_distribution: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events
          WHERE kind = $1
            AND payload -> 'arguments' ->> 'status_id' = $2
            AND payload -> 'arguments' ->> 'activity_type' = 'Create'",
    )
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(boost_id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(pending_boost_distribution, 0);
    let boost_delete_distribution: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events
          WHERE kind = $1
            AND payload -> 'arguments' ->> 'status_id' = $2
            AND payload -> 'arguments' ->> 'activity_type' = 'Delete'",
    )
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(boost_id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(boost_delete_distribution, 1);
    let delete_recipients = sqlx::query_scalar::<_, i64>(
        "SELECT (payload ->> 'account_id')::bigint
           FROM rustodon.outbox_events
          WHERE kind = $1
            AND payload ->> 'event' = 'delete'
            AND payload ->> 'object_id' = $2
          ORDER BY (payload ->> 'account_id')::bigint",
    )
    .bind(STREAM_EVENT_KIND)
    .bind(boost_id.to_string())
    .fetch_all(&pool)
    .await?;
    assert_eq!(delete_recipients, vec![ALICE, MODERATOR, API_MODERATOR]);

    let conversation_id: i64 = sqlx::query_scalar(
        "SELECT conversation_id FROM statuses WHERE id = $1 AND conversation_id IS NOT NULL",
    )
    .bind(boost_id)
    .fetch_one(&pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE kind = $1 AND payload ->> 'object_id' = $2",
    )
    .bind(STREAM_EVENT_KIND)
    .bind(boost_id.to_string())
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(format!("notification:reblog:{ALICE}:{boost_id}"))
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(boost_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id = $1")
        .bind(boost_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(conversation_id)
        .execute(&pool)
        .await?;
    sqlx::query(
        "UPDATE follows SET show_reblogs = $1
          WHERE account_id = $2 AND target_account_id = $3",
    )
    .bind(before_show_reblogs)
    .bind(MODERATOR)
    .bind(ALICE)
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(ALICE)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO account_stats SELECT * FROM jsonb_populate_record(NULL::account_stats, $1)",
    )
    .bind(&before_stats)
    .execute(&pool)
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_deletes_reblog_wrappers_from_user_stream() -> Result<(), Box<dyn Error>> {
    const REMOTE_REBLOGGER: i64 = -330;
    const REMOTE_REBLOG_URI: &str =
        "https://remote.fixture.invalid/users/timeline_author/statuses/schema-delete-reblog";
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let before_stats: Value =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    sqlx::query("UPDATE users SET disabled = false WHERE account_id = $1")
        .bind(NEWBIE)
        .execute(&pool)
        .await?;
    let original = writer
        .create_status(
            &authenticated,
            "stream deletion original @newbie",
            &[],
            None,
            Some(false),
            Some("public"),
            None,
            None,
            None,
        )
        .await?;
    let boost = writer
        .set_reblog(&authenticated, original.status_id, Some("public"), true)
        .await?;
    assert!(boost.created);
    let remote_reblog_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO statuses (
             account_id, text, spoiler_text, visibility, local, uri, url, language,
             sensitive, reply, reblog_of_id, created_at, updated_at)
         VALUES ($1, '', '', 3, false, $2, $2, NULL, false, false, $3,
                 clock_timestamp(), clock_timestamp())
         RETURNING id",
    )
    .bind(REMOTE_REBLOGGER)
    .bind(REMOTE_REBLOG_URI)
    .bind(original.status_id)
    .fetch_one(&pool)
    .await?;
    let previous_remote_reblogger_suspension: Option<NaiveDateTime> =
        sqlx::query_scalar("SELECT suspended_at FROM accounts WHERE id = $1")
            .bind(REMOTE_REBLOGGER)
            .fetch_one(&pool)
            .await?;
    sqlx::query("UPDATE accounts SET suspended_at = clock_timestamp() WHERE id = $1")
        .bind(REMOTE_REBLOGGER)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at)
         VALUES ($1, clock_timestamp(), clock_timestamp())",
    )
    .bind(remote_reblog_id)
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE status_stats SET reblogs_count = reblogs_count + 1 WHERE status_id = $1")
        .bind(original.status_id)
        .execute(&pool)
        .await?;
    let mention_id: i64 =
        sqlx::query_scalar("SELECT id FROM mentions WHERE status_id = $1 AND account_id = $2")
            .bind(original.status_id)
            .bind(NEWBIE)
            .fetch_one(&pool)
            .await?;
    let status_ids = vec![original.status_id, boost.status_id];
    let created_status_ids = vec![original.status_id, boost.status_id, remote_reblog_id];
    writer
        .delete_status(&authenticated, original.status_id, false)
        .await?;
    sqlx::query("UPDATE accounts SET suspended_at = $1 WHERE id = $2")
        .bind(previous_remote_reblogger_suspension)
        .bind(REMOTE_REBLOGGER)
        .execute(&pool)
        .await?;
    let delete_recipient_ids: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' -> 'recipient_account_ids'
           FROM rustodon.outbox_events
          WHERE kind = $1
            AND payload -> 'arguments' ->> 'status_id' = $2
            AND payload -> 'arguments' ->> 'activity_type' = 'Delete'",
    )
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(original.status_id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(delete_recipient_ids, json!([REMOTE_REBLOGGER]));
    let boost_delete_distribution: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rustodon.outbox_events
          WHERE kind = $1
            AND payload -> 'arguments' ->> 'status_id' = $2
            AND payload -> 'arguments' ->> 'activity_type' = 'Delete'",
    )
    .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
    .bind(boost.status_id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(boost_delete_distribution, 1);
    let status_id_strings = created_status_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    for status_id in &status_ids {
        let recipients = sqlx::query_scalar::<_, i64>(
            "SELECT (payload ->> 'account_id')::bigint
               FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload ->> 'event' = 'delete'
                AND payload ->> 'object_id' = $2
              ORDER BY (payload ->> 'account_id')::bigint",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(status_id.to_string())
        .fetch_all(&pool)
        .await?;
        let expected = if *status_id == original.status_id {
            vec![ALICE, MODERATOR, NEWBIE, API_MODERATOR]
        } else {
            vec![ALICE, MODERATOR, API_MODERATOR]
        };
        assert_eq!(recipients, expected);
    }
    let conversation_ids = sqlx::query_scalar::<_, i64>(
        "SELECT conversation_id FROM statuses
           WHERE id = ANY($1) AND conversation_id IS NOT NULL",
    )
    .bind(&created_status_ids)
    .fetch_all(&pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE kind = $1 AND payload ->> 'object_id' = ANY($2)",
    )
    .bind(STREAM_EVENT_KIND)
    .bind(&status_id_strings)
    .execute(&pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE payload -> 'arguments' ->> 'status_id' = ANY($1)",
    )
    .bind(&status_id_strings)
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(format!("notification:mention:{NEWBIE}:{mention_id}"))
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(format!("notification:reblog:{ALICE}:{}", boost.status_id))
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
        .bind(&created_status_ids)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id = ANY($1)")
        .bind(&created_status_ids)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE id = ANY($1)")
        .bind(&conversation_ids)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(ALICE)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO account_stats SELECT * FROM jsonb_populate_record(NULL::account_stats, $1)",
    )
    .bind(&before_stats)
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE users SET disabled = true WHERE account_id = $1")
        .bind(NEWBIE)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_saves_local_follows_idempotently() -> Result<(), Box<dyn Error>> {
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-follow-v4-6-5"),
    );
    let authenticated = authenticator
        .authenticate(&headers, rustodon::mastodon::WRITE_FOLLOWS)
        .await?;
    let before_following: i64 =
        sqlx::query_scalar("SELECT following_count FROM account_stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    let before_followers: i64 =
        sqlx::query_scalar("SELECT followers_count FROM account_stats WHERE account_id = $1")
            .bind(MATRIX_VIEWER)
            .fetch_one(&pool)
            .await?;
    writer
        .set_follow(
            &authenticated,
            MATRIX_VIEWER,
            true,
            Some(false),
            Some(true),
            Some(vec!["fr".to_owned()]),
        )
        .await?;
    writer
        .set_follow(&authenticated, MATRIX_VIEWER, true, None, None, None)
        .await?;
    let options = sqlx::query_as::<_, (bool, bool, Option<Vec<String>>)>(
        "SELECT show_reblogs, notify, languages FROM follows \
         WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(ALICE)
    .bind(MATRIX_VIEWER)
    .fetch_one(&pool)
    .await?;
    assert_eq!(options, (false, true, Some(vec!["fr".to_owned()])));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(ALICE)
        .bind(MATRIX_VIEWER)
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT following_count FROM account_stats WHERE account_id = $1",
        )
        .bind(ALICE)
        .fetch_one(&pool)
        .await?,
        before_following + 1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT followers_count FROM account_stats WHERE account_id = $1",
        )
        .bind(MATRIX_VIEWER)
        .fetch_one(&pool)
        .await?,
        before_followers + 1
    );
    writer
        .set_follow(&authenticated, MATRIX_VIEWER, false, None, None, None)
        .await?;
    writer
        .set_follow(&authenticated, MATRIX_VIEWER, false, None, None, None)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(ALICE)
        .bind(MATRIX_VIEWER)
        .fetch_one(&pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT following_count FROM account_stats WHERE account_id = $1",
        )
        .bind(ALICE)
        .fetch_one(&pool)
        .await?,
        before_following
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT followers_count FROM account_stats WHERE account_id = $1",
        )
        .bind(MATRIX_VIEWER)
        .fetch_one(&pool)
        .await?,
        before_followers
    );
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_records_remote_follow_and_undo_delivery() -> Result<(), Box<dyn Error>> {
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-follow-v4-6-5"),
    );
    let authenticated = authenticator
        .authenticate(&headers, rustodon::mastodon::WRITE_FOLLOWS)
        .await?;
    let origin = "https://fixture-v4-6-5.rustodon.invalid/";

    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_AP_ACCOUNT)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_AP_ACCOUNT)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_AP_ACCOUNT)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(REMOTE_AP_ACCOUNT)
        .bind(ALICE)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
        .bind(REMOTE_AP_ACCOUNT)
        .bind(ALICE)
        .execute(&pool)
        .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE logical_key LIKE 'activitypub:follow:%'
             OR logical_key LIKE 'activitypub:undo-follow:%'
             OR logical_key LIKE 'activitypub:block:%'
             OR logical_key LIKE 'activitypub:undo-block:%'
             OR logical_key LIKE 'activitypub:reject:%'",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.durable_jobs
          WHERE logical_key = 'activitypub:test-cancel-follow'",
    )
    .execute(&pool)
    .await?;

    let result = async {
        let first = writer
            .set_follow_with_origin(
                &authenticated,
                REMOTE_AP_ACCOUNT,
                true,
                None,
                None,
                None,
                Some(origin),
                false,
            )
            .await?;
        assert!(first.activity_id.is_some());
        let follow_uri: String = sqlx::query_scalar(
            "SELECT uri FROM follow_requests WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(ALICE)
        .bind(REMOTE_AP_ACCOUNT)
        .fetch_one(&pool)
        .await?;
        assert!(follow_uri.starts_with("https://fixture-v4-6-5.rustodon.invalid/payloads/follow-"));
        assert_eq!(first.activity_uri.as_deref(), Some(follow_uri.as_str()));
        let follow_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND logical_key LIKE 'activitypub:follow:%'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(follow_body["type"], "Follow");
        assert_eq!(follow_body["id"], follow_uri);
        assert_eq!(
            follow_body["actor"],
            "https://fixture-v4-6-5.rustodon.invalid/users/alice"
        );
        assert_eq!(
            follow_body["object"],
            "https://remote.fixture.invalid/users/exclusive_author"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT payload -> 'arguments' ->> 'remote_domain'
                   FROM rustodon.outbox_events
                  WHERE logical_key LIKE 'activitypub:follow:%'",
            )
            .fetch_one(&pool)
            .await?,
            "remote.fixture.invalid"
        );
        Queue::new(pool.clone())
            .enqueue(
                &JobSpec::new(
                    Lane::Push,
                    ACTIVITYPUB_DELIVERY_JOB_KIND,
                    json!({"body": {"id": follow_uri}}),
                )
                .logical_key("activitypub:test-cancel-follow"),
            )
            .await?;

        let duplicate = writer
            .set_follow_with_origin(
                &authenticated,
                REMOTE_AP_ACCOUNT,
                true,
                None,
                None,
                None,
                Some(origin),
                false,
            )
            .await?;
        assert!(duplicate.activity_id.is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE logical_key LIKE 'activitypub:follow:%'",
            )
            .fetch_one(&pool)
            .await?,
            1
        );

        let undone = writer
            .set_follow_with_origin(
                &authenticated,
                REMOTE_AP_ACCOUNT,
                false,
                None,
                None,
                None,
                Some(origin),
                false,
            )
            .await?;
        assert_eq!(undone.activity_uri.as_deref(), Some(follow_uri.as_str()));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events
                  WHERE logical_key LIKE 'activitypub:follow:%' AND dispatched_at IS NULL",
            )
            .fetch_one(&pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.durable_jobs
                  WHERE arguments -> 'body' ->> 'id' = $1",
            )
            .bind(&follow_uri)
            .fetch_one(&pool)
            .await?,
            0
        );
        let undo_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND logical_key LIKE 'activitypub:undo-follow:%'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(undo_body["type"], "Undo");
        assert_eq!(undo_body["object"]["id"], follow_uri);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
            )
            .bind(ALICE)
            .bind(REMOTE_AP_ACCOUNT)
            .fetch_one(&pool)
            .await?,
            0
        );

        let blocked_outgoing_follow_uri =
            "https://fixture-v4-6-5.rustodon.invalid/activities/schema-block-follow";
        sqlx::query(
            "INSERT INTO follows
               (account_id, target_account_id, show_reblogs, notify, languages, uri,
                created_at, updated_at)
             VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())",
        )
        .bind(ALICE)
        .bind(REMOTE_AP_ACCOUNT)
        .bind(blocked_outgoing_follow_uri)
        .execute(&pool)
        .await?;
        let blocked_incoming_follow_uri =
            "https://remote.fixture.invalid/activities/schema-block-incoming";
        let blocked_incoming_follow_id: i64 = sqlx::query_scalar(
            "INSERT INTO follows
               (account_id, target_account_id, show_reblogs, notify, languages, uri,
                 created_at, updated_at)
              VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())
              RETURNING id",
        )
        .bind(REMOTE_AP_ACCOUNT)
        .bind(ALICE)
        .bind(blocked_incoming_follow_uri)
        .fetch_one(&pool)
        .await?;
        writer
            .set_block_with_origin(&authenticated, REMOTE_AP_ACCOUNT, true, Some(origin))
            .await?;
        let block_uri: String = sqlx::query_scalar(
            "SELECT uri FROM blocks WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(ALICE)
        .bind(REMOTE_AP_ACCOUNT)
        .fetch_one(&pool)
        .await?;
        assert!(block_uri.starts_with("https://fixture-v4-6-5.rustodon.invalid/payloads/block-"));
        let block_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND logical_key LIKE 'activitypub:block:%'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(block_body["type"], "Block");
        assert_eq!(block_body["id"], block_uri);
        assert_eq!(
            block_body["object"],
            "https://remote.fixture.invalid/users/exclusive_author"
        );
        let blocked_undo_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'id' = $1",
        )
        .bind(blocked_outgoing_follow_uri)
        .fetch_one(&pool)
        .await?;
        assert_eq!(blocked_undo_body["type"], "Undo");
        let blocked_reject_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'id' = $1",
        )
        .bind(blocked_incoming_follow_uri)
        .fetch_one(&pool)
        .await?;
        assert_eq!(blocked_reject_body["type"], "Reject");
        assert_eq!(
            blocked_reject_body["id"],
            format!("{origin}users/alice#rejects/follows/{blocked_incoming_follow_id}")
        );

        writer
            .set_block_with_origin(&authenticated, REMOTE_AP_ACCOUNT, false, Some(origin))
            .await?;
        let undo_block_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND logical_key LIKE 'activitypub:undo-block:%'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(undo_block_body["type"], "Undo");
        assert_eq!(undo_block_body["object"]["id"], block_uri);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM blocks WHERE account_id = $1 AND target_account_id = $2",
            )
            .bind(ALICE)
            .bind(REMOTE_AP_ACCOUNT)
            .fetch_one(&pool)
            .await?,
            0
        );

        let rejected_follow_uri =
            "https://remote.fixture.invalid/activities/schema-rejected-follow";
        let rejected_follow_id: i64 = sqlx::query_scalar(
            "INSERT INTO follow_requests
               (account_id, target_account_id, show_reblogs, notify, languages, uri,
                 created_at, updated_at)
              VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())
              RETURNING id",
        )
        .bind(REMOTE_AP_ACCOUNT)
        .bind(ALICE)
        .bind(rejected_follow_uri)
        .fetch_one(&pool)
        .await?;
        writer
            .reject_follow_request_with_origin(&authenticated, REMOTE_AP_ACCOUNT, Some(origin))
            .await?;
        let reject_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'id' = $1",
        )
        .bind(rejected_follow_uri)
        .fetch_one(&pool)
        .await?;
        assert_eq!(reject_body["type"], "Reject");
        assert_eq!(
            reject_body["actor"],
            "https://fixture-v4-6-5.rustodon.invalid/users/alice"
        );
        assert_eq!(
            reject_body["id"],
            format!("{origin}users/alice#rejects/follows/{rejected_follow_id}")
        );

        let removed_follow_uri =
            "https://remote.fixture.invalid/activities/schema-removed-follower";
        let removed_follow_id: i64 = sqlx::query_scalar(
            "INSERT INTO follows
               (account_id, target_account_id, show_reblogs, notify, languages, uri,
                 created_at, updated_at)
              VALUES ($1, $2, true, false, NULL, $3, clock_timestamp(), clock_timestamp())
              RETURNING id",
        )
        .bind(REMOTE_AP_ACCOUNT)
        .bind(ALICE)
        .bind(removed_follow_uri)
        .fetch_one(&pool)
        .await?;
        writer
            .remove_follower_with_origin(&authenticated, REMOTE_AP_ACCOUNT, Some(origin))
            .await?;
        let remove_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'id' = $1",
        )
        .bind(removed_follow_uri)
        .fetch_one(&pool)
        .await?;
        assert_eq!(remove_body["type"], "Reject");
        assert_eq!(
            remove_body["id"],
            format!("{origin}users/alice#rejects/follows/{removed_follow_id}")
        );
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_AP_ACCOUNT)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_AP_ACCOUNT)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 AND target_account_id = $2")
        .bind(ALICE)
        .bind(REMOTE_AP_ACCOUNT)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(REMOTE_AP_ACCOUNT)
        .bind(ALICE)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 AND target_account_id = $2")
        .bind(REMOTE_AP_ACCOUNT)
        .bind(ALICE)
        .execute(&pool)
        .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE logical_key LIKE 'activitypub:follow:%'
             OR logical_key LIKE 'activitypub:undo-follow:%'
             OR logical_key LIKE 'activitypub:block:%'
             OR logical_key LIKE 'activitypub:undo-block:%'
             OR logical_key LIKE 'activitypub:reject:%'",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.durable_jobs
          WHERE logical_key = 'activitypub:test-cancel-follow'",
    )
    .execute(&pool)
    .await?;
    result
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_records_remote_like_and_announce_delivery() -> Result<(), Box<dyn Error>>
{
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let favourite_authenticated = authenticator
        .authenticate(&headers, WRITE_FAVOURITES)
        .await?;
    let status_authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let origin = "https://fixture-v4-6-5.rustodon.invalid/";
    let remote_status_uri =
        "https://remote.fixture.invalid/users/exclusive_author/statuses/schema-outbound";
    let remote_status_id: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (
            account_id, text, spoiler_text, visibility, local, uri, url, sensitive, reply,
            created_at, updated_at)
         VALUES ($1, 'remote outbound target', '', 0, false, $2, $2, false, false,
                 clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(REMOTE_AP_ACCOUNT)
    .bind(remote_status_uri)
    .fetch_one(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO status_stats (status_id, created_at, updated_at)
         VALUES ($1, clock_timestamp(), clock_timestamp())",
    )
    .bind(remote_status_id)
    .execute(&pool)
    .await?;
    let result = async {
        let favourite = writer
            .set_favourite_with_origin(
                &favourite_authenticated,
                remote_status_id,
                true,
                Some(origin),
                false,
            )
            .await?;
        let favourite_id = favourite.activity_id.expect("outbound favourite has an ID");
        let like_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Like'
                AND payload -> 'arguments' -> 'body' ->> 'object' = $1",
        )
        .bind(remote_status_uri)
        .fetch_one(&pool)
        .await?;
        assert_eq!(like_body["type"], "Like");
        assert_eq!(like_body["object"], remote_status_uri);
        assert_eq!(
            like_body["actor"],
            "https://fixture-v4-6-5.rustodon.invalid/users/alice"
        );
        assert_eq!(
            like_body["id"],
            format!("https://fixture-v4-6-5.rustodon.invalid/users/alice#likes/{favourite_id}")
        );
        let like_inbox: String = sqlx::query_scalar(
            "SELECT payload -> 'arguments' ->> 'inbox_url' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Like'
                AND payload -> 'arguments' -> 'body' ->> 'object' = $1",
        )
        .bind(remote_status_uri)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            like_inbox,
            "https://remote.fixture.invalid/users/exclusive_author/inbox"
        );
        writer
            .set_favourite_with_origin(
                &favourite_authenticated,
                remote_status_id,
                false,
                Some(origin),
                false,
            )
            .await?;
        let undo_like_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Undo'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'type' = 'Like'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'id' = $1",
        )
        .bind(like_body["id"].as_str().expect("Like has an ID"))
        .fetch_one(&pool)
        .await?;
        assert_eq!(undo_like_body["object"]["id"], like_body["id"]);

        let reblog = writer
            .set_reblog_with_origin(
                &status_authenticated,
                remote_status_id,
                Some("public"),
                true,
                Some(origin),
                false,
            )
            .await?;
        assert!(reblog.created);
        let announce_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Announce'
                AND payload -> 'arguments' -> 'body' ->> 'object' = $1",
        )
        .bind(remote_status_uri)
        .fetch_one(&pool)
        .await?;
        assert_eq!(announce_body["type"], "Announce");
        assert_eq!(announce_body["object"], remote_status_uri);
        let announce_inbox: String = sqlx::query_scalar(
            "SELECT payload -> 'arguments' ->> 'inbox_url' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Announce'
                AND payload -> 'arguments' -> 'body' ->> 'object' = $1",
        )
        .bind(remote_status_uri)
        .fetch_one(&pool)
        .await?;
        assert_eq!(announce_inbox, "https://remote.fixture.invalid/inbox");
        assert_eq!(
            announce_body["to"][0],
            "https://www.w3.org/ns/activitystreams#Public"
        );
        assert!(announce_body["cc"].as_array().is_some_and(|values| {
            values
                .iter()
                .any(|value| value == "https://remote.fixture.invalid/users/exclusive_author")
        }));
        writer
            .set_reblog_with_origin(
                &status_authenticated,
                remote_status_id,
                None,
                false,
                Some(origin),
                false,
            )
            .await?;
        let undo_announce_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events
              WHERE kind = 'rustodon.activitypub.deliver'
                AND payload -> 'arguments' -> 'body' ->> 'type' = 'Undo'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'type' = 'Announce'
                AND payload -> 'arguments' -> 'body' -> 'object' ->> 'object' = $1",
        )
        .bind(remote_status_uri)
        .fetch_one(&pool)
        .await?;
        assert_eq!(undo_announce_body["object"]["object"], remote_status_uri);
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    sqlx::query("DELETE FROM notifications WHERE activity_id = $1 AND activity_type = 'Status'")
        .bind(remote_status_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM favourites WHERE status_id = $1")
        .bind(remote_status_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE reblog_of_id = $1")
        .bind(remote_status_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id = $1")
        .bind(remote_status_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(remote_status_id)
        .execute(&pool)
        .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
           WHERE logical_key LIKE 'activitypub:like:%'
              OR logical_key LIKE 'activitypub:undo-like:%'
              OR logical_key LIKE 'activitypub:announce:%'
              OR logical_key LIKE 'activitypub:undo-announce:%'",
    )
    .execute(&pool)
    .await?;
    result
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_saves_local_blocks_and_mutes_idempotently() -> Result<(), Box<dyn Error>>
{
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-follow-v4-6-5"),
    );
    let authenticated = authenticator
        .authenticate(&headers, rustodon::mastodon::WRITE_BLOCKS)
        .await?;
    let conversation_id = 9901_i64;
    let account_conversation_id = 9902_i64;
    let notification_id = 116_846_900_000_000_001_i64;
    sqlx::query(
        "INSERT INTO conversations \
         (id, uri, parent_account_id, parent_status_id, created_at, updated_at) \
         VALUES ($1, NULL, $2, $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(conversation_id)
    .bind(ALICE)
    .bind(PUBLIC_STATUS)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO account_conversations \
         (id, account_id, conversation_id, last_status_id, participant_account_ids, \
          status_ids, unread) \
         VALUES ($1, $2, $3, $4, $5, $6, true)",
    )
    .bind(account_conversation_id)
    .bind(ALICE)
    .bind(conversation_id)
    .bind(PUBLIC_STATUS)
    .bind(vec![MATRIX_VIEWER])
    .bind(vec![PUBLIC_STATUS])
    .execute(&pool)
    .await?;
    let notification_request_id: i64 = sqlx::query_scalar(
        "INSERT INTO notification_requests \
         (account_id, created_at, from_account_id, last_status_id, notifications_count, updated_at) \
         VALUES ($1, clock_timestamp(), $2, $3, 1, clock_timestamp()) RETURNING id",
    )
    .bind(ALICE)
    .bind(MATRIX_VIEWER)
    .bind(PUBLIC_STATUS)
    .fetch_one(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO notifications \
         (id, account_id, activity_id, activity_type, created_at, filtered, \
          from_account_id, group_key, type, updated_at) \
         VALUES ($1, $2, $3, 'Status', clock_timestamp(), false, $4, NULL, 'status', clock_timestamp())",
    )
    .bind(notification_id)
    .bind(ALICE)
    .bind(PUBLIC_STATUS)
    .bind(MATRIX_VIEWER)
    .execute(&pool)
    .await?;
    let outgoing_request_id: i64 = sqlx::query_scalar(
        "INSERT INTO follow_requests (account_id, target_account_id, show_reblogs, notify, \
                                      languages, uri, created_at, updated_at) \
         VALUES ($1, $2, true, false, NULL, NULL, clock_timestamp(), clock_timestamp()) \
         RETURNING id",
    )
    .bind(ALICE)
    .bind(MATRIX_VIEWER)
    .fetch_one(&pool)
    .await?;
    let incoming_request_id: i64 = sqlx::query_scalar(
        "INSERT INTO follow_requests (account_id, target_account_id, show_reblogs, notify, \
                                      languages, uri, created_at, updated_at) \
         VALUES ($1, $2, true, false, NULL, NULL, clock_timestamp(), clock_timestamp()) \
         RETURNING id",
    )
    .bind(MATRIX_VIEWER)
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;

    sqlx::query(
        "INSERT INTO notification_permissions (account_id, from_account_id, created_at, updated_at) \
         VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
    )
    .bind(ALICE)
    .bind(MATRIX_VIEWER)
    .execute(&pool)
    .await?;
    writer
        .set_block(&authenticated, MATRIX_VIEWER, true)
        .await?;
    writer
        .set_block(&authenticated, MATRIX_VIEWER, true)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM blocks WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(ALICE)
        .bind(MATRIX_VIEWER)
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notification_permissions \
             WHERE account_id = $1 AND from_account_id = $2",
        )
        .bind(ALICE)
        .bind(MATRIX_VIEWER)
        .fetch_one(&pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM account_conversations WHERE id = $1",)
            .bind(account_conversation_id)
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notification_requests WHERE id = $1")
            .bind(notification_request_id)
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notifications WHERE id = $1")
            .bind(notification_id)
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM follow_requests WHERE id = $1")
            .bind(outgoing_request_id)
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM follow_requests WHERE id = $1")
            .bind(incoming_request_id)
            .fetch_one(&pool)
            .await?,
        0
    );
    writer.set_block(&authenticated, ALICE, true).await?;
    assert!(matches!(
        writer
            .set_follow(&authenticated, MATRIX_VIEWER, true, None, None, None)
            .await,
        Err(rustodon::mastodon::WriteError::NotFound)
    ));
    writer
        .set_block(&authenticated, MATRIX_VIEWER, false)
        .await?;
    writer
        .set_block(&authenticated, MATRIX_VIEWER, false)
        .await?;

    sqlx::query(
        "INSERT INTO account_conversations
         (id, account_id, conversation_id, last_status_id, participant_account_ids,
          status_ids, unread)
         VALUES ($1, $2, $3, $4, $5, $6, true)",
    )
    .bind(account_conversation_id)
    .bind(ALICE)
    .bind(conversation_id)
    .bind(PUBLIC_STATUS)
    .bind(vec![MATRIX_VIEWER])
    .bind(vec![PUBLIC_STATUS])
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO notification_requests
         (id, account_id, created_at, from_account_id, last_status_id,
          notifications_count, updated_at)
         VALUES ($1, $2, clock_timestamp(), $3, $4, 1, clock_timestamp())",
    )
    .bind(notification_request_id)
    .bind(ALICE)
    .bind(MATRIX_VIEWER)
    .bind(PUBLIC_STATUS)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO notifications
         (id, account_id, activity_id, activity_type, created_at, filtered,
          from_account_id, group_key, type, updated_at)
         VALUES ($1, $2, $3, 'Status', clock_timestamp(), false, $4, NULL,
                 'status', clock_timestamp())",
    )
    .bind(notification_id)
    .bind(ALICE)
    .bind(PUBLIC_STATUS)
    .bind(MATRIX_VIEWER)
    .execute(&pool)
    .await?;
    writer
        .set_mute(&authenticated, MATRIX_VIEWER, true, Some(true), None)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM account_conversations WHERE id = $1")
            .bind(account_conversation_id)
            .fetch_one(&pool)
            .await?,
        0,
        "hiding notifications on mute must remove conversations from the muted account",
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notification_requests WHERE id = $1")
            .bind(notification_request_id)
            .fetch_one(&pool)
            .await?,
        0,
        "hiding notifications on mute must remove notification requests",
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notifications WHERE id = $1")
            .bind(notification_id)
            .fetch_one(&pool)
            .await?,
        0,
        "hiding notifications on mute must remove notifications",
    );
    writer
        .set_mute(&authenticated, MATRIX_VIEWER, true, Some(false), None)
        .await?;
    writer
        .set_mute(&authenticated, MATRIX_VIEWER, true, Some(false), None)
        .await?;
    let mute = sqlx::query_as::<_, (bool, Option<chrono::NaiveDateTime>)>(
        "SELECT hide_notifications, expires_at FROM mutes \
         WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(ALICE)
    .bind(MATRIX_VIEWER)
    .fetch_one(&pool)
    .await?;
    assert_eq!(mute, (false, None));
    writer
        .set_mute(&authenticated, MATRIX_VIEWER, true, Some(false), Some(60))
        .await?;
    let scheduled_mute: (Option<chrono::NaiveDateTime>, i64) = sqlx::query_as(
        "SELECT mute.expires_at, \
                (SELECT count(*) FROM rustodon.outbox_events event \
                 WHERE event.kind = 'rustodon.mastodon.delete_mute' \
                   AND event.dispatched_at IS NULL \
                   AND event.payload -> 'arguments' ->> 'mute_id' = mute.id::text) \
         FROM mutes mute WHERE mute.account_id = $1 AND mute.target_account_id = $2",
    )
    .bind(ALICE)
    .bind(MATRIX_VIEWER)
    .fetch_one(&pool)
    .await?;
    assert!(scheduled_mute.0.is_some());
    assert_eq!(scheduled_mute.1, 1);
    writer
        .set_mute(&authenticated, MATRIX_VIEWER, false, None, None)
        .await?;
    writer
        .set_mute(&authenticated, MATRIX_VIEWER, false, None, None)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM mutes WHERE account_id = $1 AND target_account_id = $2",
        )
        .bind(ALICE)
        .bind(MATRIX_VIEWER)
        .fetch_one(&pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events \
             WHERE kind = 'rustodon.mastodon.delete_mute' AND dispatched_at IS NULL",
        )
        .fetch_one(&pool)
        .await?,
        0
    );
    sqlx::query("DELETE FROM notifications WHERE id = $1")
        .bind(notification_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM notification_requests WHERE id = $1")
        .bind(notification_request_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM account_conversations WHERE id = $1")
        .bind(account_conversation_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(conversation_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE id = $1")
        .bind(outgoing_request_id)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_saves_follow_request_decisions_idempotently() -> Result<(), Box<dyn Error>>
{
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-follow-v4-6-5"),
    );
    let authenticated = authenticator
        .authenticate(&headers, rustodon::mastodon::WRITE_FOLLOWS)
        .await?;
    let request: Value =
        sqlx::query_scalar("SELECT to_jsonb(row) FROM follow_requests row WHERE id = $1")
            .bind(8003_i64)
            .fetch_one(&pool)
            .await?;
    let notification: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(row) FROM notifications row WHERE id = $1")
            .bind(10005_i64)
            .fetch_optional(&pool)
            .await?;
    let before_following: i64 =
        sqlx::query_scalar("SELECT following_count FROM account_stats WHERE account_id = $1")
            .bind(CAROL)
            .fetch_one(&pool)
            .await?;
    let before_followers: i64 =
        sqlx::query_scalar("SELECT followers_count FROM account_stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    let carol_protocol: i32 = sqlx::query_scalar("SELECT protocol FROM accounts WHERE id = $1")
        .bind(CAROL)
        .fetch_one(&pool)
        .await?;
    sqlx::query("UPDATE accounts SET protocol = 1 WHERE id = $1")
        .bind(CAROL)
        .execute(&pool)
        .await?;

    let result = async {
        let authorized = writer
            .authorize_follow_request_with_origin(
                &authenticated,
                CAROL,
                Some("https://fixture-v4-6-5.rustodon.invalid/"),
            )
            .await?;
        assert!(authorized.activity_id.is_some());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM follows WHERE account_id = $1 AND target_account_id = $2",
            )
            .bind(CAROL)
            .bind(ALICE)
            .fetch_one(&pool)
            .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM follow_requests WHERE id = $1",
            )
            .bind(8003_i64)
            .fetch_one(&pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications WHERE id = $1",
            )
            .bind(10005_i64)
            .fetch_one(&pool)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT following_count FROM account_stats WHERE account_id = $1",
            )
            .bind(CAROL)
            .fetch_one(&pool)
            .await?,
            before_following + 1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT followers_count FROM account_stats WHERE account_id = $1",
            )
            .bind(ALICE)
            .fetch_one(&pool)
            .await?,
            before_followers + 1
        );
        let accept_body: Value = sqlx::query_scalar(
            "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events \
              WHERE kind = 'rustodon.activitypub.deliver' \
                AND logical_key LIKE 'activitypub:accept:%'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(accept_body["type"], "Accept");
        assert_eq!(
            accept_body["object"]["id"],
            "https://remote.fixture.invalid/users/carol#follows/8003"
        );
        assert_eq!(
            accept_body["actor"],
            "https://fixture-v4-6-5.rustodon.invalid/users/alice"
        );
        assert!(matches!(
            writer
                .authorize_follow_request(&authenticated, CAROL)
                .await,
            Err(WriteError::NotFound)
        ));
        writer.remove_follower(&authenticated, CAROL).await?;
        writer.remove_follower(&authenticated, CAROL).await?;
        sqlx::query(
            "INSERT INTO follow_requests SELECT * FROM jsonb_populate_record(NULL::follow_requests, $1)",
        )
        .bind(&request)
        .execute(&pool)
        .await?;
        if let Some(notification) = &notification {
            sqlx::query(
                "INSERT INTO notifications SELECT * FROM jsonb_populate_record(NULL::notifications, $1)",
            )
            .bind(notification)
            .execute(&pool)
            .await?;
        }
        writer.reject_follow_request(&authenticated, CAROL).await?;
        assert!(matches!(
            writer.reject_follow_request(&authenticated, CAROL).await,
            Err(WriteError::NotFound)
        ));
        sqlx::query(
            "DELETE FROM rustodon.outbox_events \
              WHERE kind = 'rustodon.activitypub.deliver' \
                AND logical_key LIKE 'activitypub:accept:%'",
        )
        .execute(&pool)
        .await?;
        sqlx::query("UPDATE accounts SET protocol = 0 WHERE id = $1")
            .bind(CAROL)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO follow_requests SELECT * FROM jsonb_populate_record(NULL::follow_requests, $1)",
        )
        .bind(&request)
        .execute(&pool)
        .await?;
        writer
            .authorize_follow_request_with_origin(
                &authenticated,
                CAROL,
                Some("https://fixture-v4-6-5.rustodon.invalid/"),
            )
            .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events \
                  WHERE kind = 'rustodon.activitypub.deliver' \
                    AND logical_key LIKE 'activitypub:accept:%'",
            )
            .fetch_one(&pool)
            .await?,
            0,
            "non-ActivityPub remote accounts must not receive Accept activities",
        );
        writer.remove_follower(&authenticated, CAROL).await?;
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    sqlx::query("DELETE FROM follows WHERE account_id = $1 AND target_account_id = $2")
        .bind(CAROL)
        .bind(ALICE)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE id = $1")
        .bind(8003_i64)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM notifications WHERE id = $1")
        .bind(10005_i64)
        .execute(&pool)
        .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
          WHERE kind = 'rustodon.activitypub.deliver' \
            AND logical_key LIKE 'activitypub:accept:%'",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO follow_requests SELECT * FROM jsonb_populate_record(NULL::follow_requests, $1)",
    )
    .bind(&request)
    .execute(&pool)
    .await?;
    if let Some(notification) = &notification {
        sqlx::query(
            "INSERT INTO notifications SELECT * FROM jsonb_populate_record(NULL::notifications, $1)",
        )
        .bind(notification)
        .execute(&pool)
        .await?;
    }
    sqlx::query("UPDATE account_stats SET following_count = $1 WHERE account_id = $2")
        .bind(before_following)
        .bind(CAROL)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE account_stats SET followers_count = $1 WHERE account_id = $2")
        .bind(before_followers)
        .bind(ALICE)
        .execute(&pool)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT following_count FROM account_stats WHERE account_id = $1",
        )
        .bind(CAROL)
        .fetch_one(&pool)
        .await?,
        before_following
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT followers_count FROM account_stats WHERE account_id = $1",
        )
        .bind(ALICE)
        .fetch_one(&pool)
        .await?,
        before_followers
    );
    sqlx::query("UPDATE accounts SET protocol = $1 WHERE id = $2")
        .bind(carol_protocol)
        .bind(CAROL)
        .execute(&pool)
        .await?;
    result
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn notification_policy_ignores_expired_mutes() -> Result<(), Box<dyn Error>> {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide a Mastodon owner URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let status_id: i64 = sqlx::query_scalar(
        "INSERT INTO statuses (account_id, text, spoiler_text, visibility, local, language, \
         sensitive, reply, created_at, updated_at) \
         VALUES ($1, 'expired mute probe', '', 0, true, 'en', false, false, \
                 clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;
    let favourite_id: i64 = sqlx::query_scalar(
        "INSERT INTO favourites (account_id, status_id, created_at, updated_at) \
         VALUES ($1, $2, clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(NEWBIE)
    .bind(status_id)
    .fetch_one(&pool)
    .await?;
    let mute_id: i64 = sqlx::query_scalar(
        "INSERT INTO mutes (account_id, target_account_id, hide_notifications, expires_at, \
         created_at, updated_at) \
         VALUES ($1, $2, true, clock_timestamp() - interval '1 second', \
                 clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(ALICE)
    .bind(NEWBIE)
    .fetch_one(&pool)
    .await?;
    let permission_id: i64 = sqlx::query_scalar(
        "INSERT INTO notification_permissions (account_id, from_account_id, created_at, updated_at) \
         VALUES ($1, $2, clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(ALICE)
    .bind(NEWBIE)
    .fetch_one(&pool)
    .await?;
    let result = async {
        let outcome = writer
            .create_notification(NotificationCreate {
                recipient_account_id: ALICE,
                activity: NotificationActivity::Favourite { id: favourite_id },
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
            return Err(format!("expired mute notification returned {outcome:?}").into());
        }
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    sqlx::query("DELETE FROM notifications WHERE activity_id = $1 AND activity_type = 'Favourite'")
        .bind(favourite_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM favourites WHERE id = $1")
        .bind(favourite_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM mutes WHERE id = $1")
        .bind(mute_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM notification_permissions WHERE id = $1")
        .bind(permission_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(status_id)
        .execute(&pool)
        .await?;
    result
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_creates_text_statuses_for_all_visibilities() -> Result<(), Box<dyn Error>>
{
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator
        .authenticate(&headers, rustodon::mastodon::WRITE_STATUSES)
        .await?;
    let before_stats: Value =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    let mut status_ids = Vec::new();
    for (visibility, expected) in [
        ("public", 0_i32),
        ("unlisted", 1),
        ("private", 2),
        ("direct", 3),
        ("limited", 4),
    ] {
        let outcome = writer
            .create_status(
                &authenticated,
                "fixture status write",
                &[],
                Some("content warning"),
                Some(false),
                Some(visibility),
                Some("fr"),
                None,
                None,
            )
            .await?;
        status_ids.push(outcome.status_id);
        let row: (i32, String, String, bool, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT visibility, text, spoiler_text, sensitive, language, conversation_id \
             FROM statuses WHERE id = $1",
        )
        .bind(outcome.status_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            (row.0, row.1, row.2, row.3, row.4),
            (
                expected,
                "fixture status write".to_owned(),
                "content warning".to_owned(),
                true,
                Some("fr".to_owned())
            )
        );
        assert!(row.5.is_some());
    }
    let mention_status = writer
        .create_status(
            &authenticated,
            "@moderator durable mention",
            &[],
            None,
            Some(false),
            Some("public"),
            None,
            None,
            None,
        )
        .await?;
    status_ids.push(mention_status.status_id);
    let (mention_id, mention_recipient): (i64, i64) = sqlx::query_as(
        "SELECT id, account_id FROM mentions WHERE status_id = $1 ORDER BY id LIMIT 1",
    )
    .bind(mention_status.status_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(mention_recipient, MODERATOR);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events \
             WHERE kind = $1 AND logical_key = $2",
        )
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(format!("notification:mention:{MODERATOR}:{mention_id}"))
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1 AND payload ->> 'event' = 'status.update:notification'
                AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(MODERATOR.to_string())
        .bind(mention_status.status_id.to_string())
        .fetch_one(&pool)
        .await?,
        0,
        "status creation must not emit an edit notification stream event"
    );
    let bob_protocol: i32 = sqlx::query_scalar("SELECT protocol FROM accounts WHERE id = $1")
        .bind(BOB)
        .fetch_one(&pool)
        .await?;
    sqlx::query("UPDATE accounts SET protocol = 0 WHERE id = $1")
        .bind(BOB)
        .execute(&pool)
        .await?;
    let legacy_protocol_status = writer
        .create_status(
            &authenticated,
            "@bob@remote.fixture.invalid legacy protocol mention",
            &[],
            None,
            Some(false),
            Some("public"),
            None,
            None,
            None,
        )
        .await?;
    status_ids.push(legacy_protocol_status.status_id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mentions WHERE status_id = $1")
            .bind(legacy_protocol_status.status_id)
            .fetch_one(&pool)
            .await?,
        0,
        "non-ActivityPub remote accounts must not become local status mentions"
    );
    sqlx::query("UPDATE accounts SET protocol = $1 WHERE id = $2")
        .bind(bob_protocol)
        .bind(BOB)
        .execute(&pool)
        .await?;
    match writer
        .create_notification(NotificationCreate {
            recipient_account_id: MODERATOR,
            activity: NotificationActivity::Mention { id: mention_id },
            silenced: false,
        })
        .await?
    {
        NotificationCreateOutcome::Created { .. } | NotificationCreateOutcome::Existing { .. } => {}
        NotificationCreateOutcome::Dropped => {
            return Err("fixture mention notification was unexpectedly dropped".into());
        }
    }
    writer
        .update_status(
            &authenticated,
            mention_status.status_id,
            &StatusUpdate {
                text: Some("mention withdrawn".to_owned()),
                ..StatusUpdate::default()
            },
        )
        .await?;
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT silent FROM mentions WHERE id = $1")
            .bind(mention_id)
            .fetch_one(&pool)
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications \
             WHERE account_id = $1 AND activity_id = $2 AND activity_type = 'Mention'",
        )
        .bind(MODERATOR)
        .bind(mention_id)
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events \
             WHERE kind = $1 AND logical_key = $2",
        )
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(format!("notification:mention:{MODERATOR}:{mention_id}"))
        .fetch_one(&pool)
        .await?,
        0
    );
    writer
        .update_status(
            &authenticated,
            mention_status.status_id,
            &StatusUpdate {
                text: Some("@moderator mention restored".to_owned()),
                ..StatusUpdate::default()
            },
        )
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM rustodon.outbox_events
              WHERE kind = $1 AND payload ->> 'event' = 'status.update:notification'
                AND payload ->> 'account_id' = $2 AND payload ->> 'object_id' = $3",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(MODERATOR.to_string())
        .bind(mention_status.status_id.to_string())
        .fetch_one(&pool)
        .await?,
        1,
        "a status update mentioning a local account must enter its notification stream"
    );
    let first = writer
        .create_status(
            &authenticated,
            "idempotent fixture status",
            &[],
            None,
            None,
            Some("public"),
            None,
            None,
            Some(IdempotencyKey {
                scope: "status:create",
                key: "fixture-status-idempotency",
                fingerprint: [7; 32],
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            }),
        )
        .await?;
    let replay = writer
        .create_status(
            &authenticated,
            "idempotent fixture status",
            &[],
            None,
            None,
            Some("public"),
            None,
            None,
            Some(IdempotencyKey {
                scope: "status:create",
                key: "fixture-status-idempotency",
                fingerprint: [7; 32],
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            }),
        )
        .await?;
    assert_eq!(first, replay);
    status_ids.push(first.status_id);
    sqlx::query(
        "UPDATE rustodon.idempotency_keys \
            SET created_at = clock_timestamp() - interval '2 seconds', \
                expires_at = clock_timestamp() - interval '1 second' \
         WHERE scope = $1 AND key = $2",
    )
    .bind("status:create")
    .bind("fixture-status-idempotency")
    .execute(&pool)
    .await?;
    let expired_replay = writer
        .create_status(
            &authenticated,
            "idempotent fixture status",
            &[],
            None,
            None,
            Some("public"),
            None,
            None,
            Some(IdempotencyKey {
                scope: "status:create",
                key: "fixture-status-idempotency",
                fingerprint: [7; 32],
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            }),
        )
        .await?;
    assert_ne!(first, expired_replay);
    status_ids.push(expired_replay.status_id);
    let media_id = 116_844_842_188_806_002_i64;
    sqlx::query(
        "INSERT INTO media_attachments \
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
    .bind(ALICE)
    .execute(&pool)
    .await?;
    let media_status = writer
        .create_status(
            &authenticated,
            "media fixture status",
            &[media_id],
            None,
            Some(false),
            Some("public"),
            None,
            None,
            None,
        )
        .await?;
    status_ids.push(media_status.status_id);
    let update_reblog_status_id = -600_i64;
    let quoted_update_status_id = -601_i64;
    let quoted_update_id = -602_i64;
    sqlx::query(
        "INSERT INTO statuses \
         (id, account_id, text, spoiler_text, visibility, local, language, sensitive, reply, \
          ordered_media_attachment_ids, reblog_of_id, created_at, updated_at) \
         VALUES ($1, $2, '', '', 0, true, 'en', false, false, NULL, $3, \
                 clock_timestamp(), clock_timestamp()), \
                ($4, $2, 'quoted update fixture', '', 0, true, 'en', false, false, NULL, NULL, \
                 clock_timestamp(), clock_timestamp())",
    )
    .bind(update_reblog_status_id)
    .bind(MODERATOR)
    .bind(media_status.status_id)
    .bind(quoted_update_status_id)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO quotes \
         (id, account_id, status_id, quoted_account_id, quoted_status_id, state, legacy, \
          created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, 1, false, clock_timestamp(), clock_timestamp())",
    )
    .bind(quoted_update_id)
    .bind(MODERATOR)
    .bind(quoted_update_status_id)
    .bind(ALICE)
    .bind(media_status.status_id)
    .execute(&pool)
    .await?;
    status_ids.extend([update_reblog_status_id, quoted_update_status_id]);
    let attached_media: (Option<i64>, Option<Vec<i64>>) = sqlx::query_as(
        "SELECT status_id, (SELECT ordered_media_attachment_ids FROM statuses WHERE id = $1) \
         FROM media_attachments WHERE id = $2",
    )
    .bind(media_status.status_id)
    .bind(media_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        attached_media,
        (Some(media_status.status_id), Some(vec![media_id]))
    );
    let media_update = StatusUpdate {
        text: Some("edited media fixture status".to_owned()),
        spoiler_text: Some(String::new()),
        sensitive: Some(false),
        language: Some("en".to_owned()),
        media_ids: Some(vec![media_id]),
        media_attributes: None,
    };
    writer
        .update_status(&authenticated, media_status.status_id, &media_update)
        .await?;
    assert_eq!(
        sqlx::query_as::<_, (String, Option<NaiveDateTime>, i64)>(
            "SELECT text, edited_at, (SELECT count(*) FROM status_edits WHERE status_id = statuses.id) \
             FROM statuses WHERE id = $1",
        )
        .bind(media_status.status_id)
        .fetch_one(&pool)
        .await?,
        (
            "edited media fixture status".to_owned(),
            sqlx::query_scalar::<_, Option<NaiveDateTime>>(
                "SELECT edited_at FROM statuses WHERE id = $1",
            )
            .bind(media_status.status_id)
            .fetch_one(&pool)
            .await?,
            2,
        )
    );
    let second_media_update = StatusUpdate {
        text: Some("edited media fixture status again".to_owned()),
        media_attributes: Some(vec![StatusMediaAttributeUpdate {
            id: media_id,
            description: AccountProfileValue::Value("updated media description".to_owned()),
            focus: AccountProfileValue::Value(MediaFocus { x: 0.25, y: -0.5 }),
        }]),
        ..media_update.clone()
    };
    writer
        .update_status(&authenticated, media_status.status_id, &second_media_update)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM status_edits WHERE status_id = $1",)
            .bind(media_status.status_id)
            .fetch_one(&pool)
            .await?,
        3
    );
    assert_eq!(
        sqlx::query_as::<_, (Option<String>, Option<Value>)>(
            "SELECT description, file_meta FROM media_attachments WHERE id = $1",
        )
        .bind(media_id)
        .fetch_one(&pool)
        .await?,
        (
            Some("updated media description".to_owned()),
            Some(json!({
                "original": {"width": 600, "height": 400, "size": "600x400"},
                "small": {"width": 588, "height": 392, "size": "588x392"},
                "focus": {"x": 0.25, "y": -0.5},
            })),
        )
    );
    writer
        .update_status(&authenticated, media_status.status_id, &second_media_update)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM status_edits WHERE status_id = $1")
            .bind(media_status.status_id)
            .fetch_one(&pool)
            .await?,
        3
    );
    let update_distribution: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' FROM rustodon.outbox_events
          WHERE kind = 'rustodon.activitypub.distribute_status'
             AND payload -> 'arguments' ->> 'status_id' = $1
             AND payload -> 'arguments' ->> 'activity_type' = 'Update'
           ORDER BY id DESC LIMIT 1",
    )
    .bind(media_status.status_id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(update_distribution["status_id"], media_status.status_id);
    assert_eq!(update_distribution["activity_type"], "Update");
    let edited_at_micros =
        sqlx::query_scalar::<_, NaiveDateTime>("SELECT edited_at FROM statuses WHERE id = $1")
            .bind(media_status.status_id)
            .fetch_one(&pool)
            .await?
            .and_utc()
            .timestamp_micros();
    assert_eq!(update_distribution["edited_at_micros"], edited_at_micros);
    for (activity_type, activity_id) in [
        ("update", media_status.status_id),
        ("quoted_update", quoted_update_status_id),
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM rustodon.outbox_events \
                  WHERE kind = $1 \
                    AND logical_key = $2",
            )
            .bind(NOTIFICATION_CREATE_JOB_KIND)
            .bind(format!(
                "notification:{activity_type}:{MODERATOR}:{activity_id}:{edited_at_micros}"
            ))
            .fetch_one(&pool)
            .await?,
            1
        );
    }
    let preserved_media_id = 116_844_842_188_806_003_i64;
    sqlx::query(
        "INSERT INTO media_attachments \
         (id, account_id, type, processing, remote_url, file_content_type, \
          file_file_name, file_file_size, file_meta, file_storage_schema_version, \
          file_updated_at, blurhash, created_at, updated_at) \
         VALUES ($1, $2, 0, 2, '', 'image/jpeg', 'preserved-media.jpg', 36381, \
                 '{\"original\":{\"width\":600,\"height\":400,\"size\":\"600x400\"}}', \
                 1, clock_timestamp(), 'preserved-blurhash', clock_timestamp(), clock_timestamp())",
    )
    .bind(preserved_media_id)
    .bind(ALICE)
    .execute(&pool)
    .await?;
    let preserved_status = writer
        .create_status(
            &authenticated,
            "preserved media status",
            &[preserved_media_id],
            None,
            Some(false),
            Some("public"),
            None,
            None,
            None,
        )
        .await?;
    status_ids.push(preserved_status.status_id);
    writer
        .delete_status(&authenticated, preserved_status.status_id, false)
        .await?;
    let delete_distribution: Value = sqlx::query_scalar(
        "SELECT payload -> 'arguments' FROM rustodon.outbox_events
          WHERE kind = 'rustodon.activitypub.distribute_status'
            AND payload -> 'arguments' ->> 'status_id' = $1
            AND payload -> 'arguments' ->> 'activity_type' = 'Delete'",
    )
    .bind(preserved_status.status_id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(delete_distribution["status_id"], preserved_status.status_id);
    assert_eq!(delete_distribution["activity_type"], "Delete");
    assert!(
        sqlx::query_scalar::<_, Option<NaiveDateTime>>(
            "SELECT deleted_at FROM statuses WHERE id = $1",
        )
        .bind(preserved_status.status_id)
        .fetch_one(&pool)
        .await?
        .is_some()
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT status_id FROM media_attachments WHERE id = $1",
        )
        .bind(preserved_media_id)
        .fetch_one(&pool)
        .await?,
        None
    );
    let reported_status = writer
        .create_status(
            &authenticated,
            "reported media status",
            &[preserved_media_id],
            None,
            Some(false),
            Some("public"),
            None,
            None,
            None,
        )
        .await?;
    status_ids.push(reported_status.status_id);
    let report_id: i64 = sqlx::query_scalar(
        "INSERT INTO reports (account_id, status_ids, target_account_id, created_at, updated_at) \
         VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp()) RETURNING id",
    )
    .bind(NEWBIE)
    .bind(vec![reported_status.status_id])
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;
    writer
        .delete_status(&authenticated, media_status.status_id, true)
        .await?;
    assert!(
        sqlx::query_scalar::<_, Option<NaiveDateTime>>(
            "SELECT deleted_at FROM statuses WHERE id = $1",
        )
        .bind(media_status.status_id)
        .fetch_one(&pool)
        .await?
        .is_some()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_attachments WHERE id = $1",)
            .bind(media_id)
            .fetch_one(&pool)
            .await?,
        0
    );
    writer
        .delete_status(&authenticated, reported_status.status_id, true)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT status_id FROM media_attachments WHERE id = $1",
        )
        .bind(preserved_media_id)
        .fetch_one(&pool)
        .await?,
        Some(reported_status.status_id)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM status_stats WHERE status_id = ANY($1)",
        )
        .bind(&status_ids)
        .fetch_one(&pool)
        .await?,
         12
    );
    let reblog = writer
        .set_reblog(&authenticated, status_ids[0], Some("public"), true)
        .await?;
    assert!(reblog.created);
    status_ids.push(reblog.status_id);
    sqlx::query(
        "INSERT INTO status_pins (account_id, status_id, created_at, updated_at) \
         VALUES ($1, $2, clock_timestamp(), clock_timestamp())",
    )
    .bind(ALICE)
    .bind(status_ids[0])
    .execute(&pool)
    .await?;
    let count_before_delete: i64 =
        sqlx::query_scalar("SELECT statuses_count FROM account_stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    writer
        .delete_status(&authenticated, status_ids[0], false)
        .await?;
    assert!(
        sqlx::query_scalar::<_, Option<NaiveDateTime>>(
            "SELECT deleted_at FROM statuses WHERE id = $1",
        )
        .bind(status_ids[0])
        .fetch_one(&pool)
        .await?
        .is_some()
    );
    assert!(
        sqlx::query_scalar::<_, Option<NaiveDateTime>>(
            "SELECT deleted_at FROM statuses WHERE id = $1",
        )
        .bind(reblog.status_id)
        .fetch_one(&pool)
        .await?
        .is_some()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM status_pins WHERE status_id = $1")
            .bind(status_ids[0])
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT statuses_count FROM account_stats WHERE account_id = $1",
        )
        .bind(ALICE)
        .fetch_one(&pool)
        .await?,
        count_before_delete - 2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT reblogs_count FROM status_stats WHERE status_id = $1",
        )
        .bind(status_ids[0])
        .fetch_one(&pool)
        .await?,
        0
    );
    sqlx::query("DELETE FROM mentions WHERE status_id = ANY($1)")
        .bind(&status_ids)
        .execute(&pool)
        .await?;
    sqlx::query(
        "DELETE FROM notifications WHERE account_id = $1 AND activity_id = $2 \
         AND activity_type = 'Mention'",
    )
    .bind(MODERATOR)
    .bind(mention_id)
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2")
        .bind(NOTIFICATION_CREATE_JOB_KIND)
        .bind(format!("notification:mention:{MODERATOR}:{mention_id}"))
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM quotes WHERE id = $1")
        .bind(quoted_update_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
        .bind(&status_ids)
        .execute(&pool)
        .await?;
    let status_id_strings = status_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE payload -> 'arguments' ->> 'status_id' = ANY($1)",
    )
    .bind(status_id_strings)
    .execute(&pool)
    .await?;
    let notification_activity_ids = vec![media_status.status_id, quoted_update_status_id]
        .into_iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    sqlx::query(
        "DELETE FROM rustodon.outbox_events \
          WHERE kind = $1 \
            AND payload -> 'arguments' ->> 'activity_id' = ANY($2)",
    )
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(notification_activity_ids)
    .execute(&pool)
    .await?;
    sqlx::query(
        "DELETE FROM notifications WHERE activity_type = 'Status' AND activity_id = ANY($1)",
    )
    .bind(vec![media_status.status_id, quoted_update_status_id])
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id = ANY($1)")
        .bind(&status_ids)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE parent_status_id = ANY($1)")
        .bind(&status_ids)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM media_attachments WHERE id = $1")
        .bind(media_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM media_attachments WHERE id = $1")
        .bind(preserved_media_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM reports WHERE id = $1")
        .bind(report_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(ALICE)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM rustodon.idempotency_keys WHERE scope = $1 AND key = $2")
        .bind("status:create")
        .bind("fixture-status-idempotency")
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO account_stats SELECT * FROM jsonb_populate_record(NULL::account_stats, $1)",
    )
    .bind(&before_stats)
    .execute(&pool)
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn write_repository_unlinks_deleted_direct_statuses_from_conversations()
-> Result<(), Box<dyn Error>> {
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator.authenticate(&headers, WRITE_STATUSES).await?;
    let status_id = writer
        .create_status(
            &authenticated,
            "direct deletion conversation fixture",
            &[],
            None,
            Some(false),
            Some("direct"),
            None,
            None,
            None,
        )
        .await?
        .status_id;
    let conversation_id: i64 = sqlx::query_scalar(
        "SELECT conversation_id FROM statuses WHERE id = $1 AND conversation_id IS NOT NULL",
    )
    .bind(status_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM account_conversations
              WHERE account_id = $1 AND $2 = ANY(status_ids)",
        )
        .bind(ALICE)
        .bind(status_id)
        .fetch_one(&pool)
        .await?,
        1
    );

    writer
        .delete_status(&authenticated, status_id, false)
        .await?;

    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM account_conversations
              WHERE account_id = $1 AND conversation_id = $2",
        )
        .bind(ALICE)
        .bind(conversation_id)
        .fetch_one(&pool)
        .await?,
        0,
        "deleting the only direct status must remove its account conversation",
    );
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE payload -> 'arguments' ->> 'status_id' = $1
             OR payload ->> 'object_id' = $1",
    )
    .bind(status_id.to_string())
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id = $1")
        .bind(status_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE id = $1")
        .bind(conversation_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = $1")
        .bind(status_id)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_streams_limited_and_direct_mentions_to_followers()
-> Result<(), Box<dyn Error>> {
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let authenticated = authenticator
        .authenticate(&headers, rustodon::mastodon::WRITE_STATUSES)
        .await?;
    let before_stats: Value =
        sqlx::query_scalar("SELECT to_jsonb(stats) FROM account_stats stats WHERE account_id = $1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    let follow_before: Value =
        sqlx::query_scalar("SELECT to_jsonb(follow) FROM follows follow WHERE id = 8006")
            .fetch_one(&pool)
            .await?;
    let mute_before: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(mute) FROM mutes mute
          WHERE account_id = $1 AND target_account_id = $2",
    )
    .bind(MODERATOR)
    .bind(ALICE)
    .fetch_optional(&pool)
    .await?;
    let mut status_ids = Vec::new();
    let result: Result<(), Box<dyn Error>> = async {
        for (visibility, label) in [("limited", "limited"), ("direct", "direct")] {
            let outcome = writer
                .create_status(
                    &authenticated,
                    &format!("@moderator {label} stream status"),
                    &[],
                    None,
                    Some(false),
                    Some(visibility),
                    None,
                    None,
                    None,
                )
                .await?;
            status_ids.push(outcome.status_id);
            let recipients = sqlx::query_scalar::<_, i64>(
                "SELECT (payload ->> 'account_id')::bigint
                   FROM rustodon.outbox_events
                  WHERE kind = $1
                    AND payload ->> 'event' = 'update'
                    AND payload ->> 'object_id' = $2
                  ORDER BY (payload ->> 'account_id')::bigint",
            )
            .bind(STREAM_EVENT_KIND)
            .bind(outcome.status_id.to_string())
            .fetch_all(&pool)
            .await?;
            assert_eq!(recipients, vec![ALICE, MODERATOR]);
        }

        let silent_status = writer
            .create_status(
                &authenticated,
                "silent audience stream status",
                &[],
                None,
                Some(false),
                Some("limited"),
                None,
                None,
                None,
            )
            .await?;
        status_ids.push(silent_status.status_id);
        sqlx::query(
            "INSERT INTO mentions (account_id, status_id, silent, created_at, updated_at)
             VALUES ($1, $2, true, clock_timestamp(), clock_timestamp())",
        )
        .bind(MODERATOR)
        .bind(silent_status.status_id)
        .execute(&pool)
        .await?;
        writer
            .update_status(
                &authenticated,
                silent_status.status_id,
                &StatusUpdate {
                    text: Some("silent audience stream status edited".to_owned()),
                    ..StatusUpdate::default()
                },
            )
            .await?;
        let silent_recipients = sqlx::query_scalar::<_, i64>(
            "SELECT (payload ->> 'account_id')::bigint
               FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload ->> 'event' = 'status.update'
                AND payload ->> 'object_id' = $2
              ORDER BY (payload ->> 'account_id')::bigint",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(silent_status.status_id.to_string())
        .fetch_all(&pool)
        .await?;
        assert_eq!(silent_recipients, vec![ALICE, MODERATOR]);

        sqlx::query("UPDATE follows SET languages = ARRAY['en'] WHERE id = 8006")
            .execute(&pool)
            .await?;
        let french = writer
            .create_status(
                &authenticated,
                "French stream filter status",
                &[],
                None,
                Some(false),
                Some("public"),
                Some("fr"),
                None,
                None,
            )
            .await?;
        status_ids.push(french.status_id);
        let french_recipients = sqlx::query_scalar::<_, i64>(
            "SELECT (payload ->> 'account_id')::bigint
               FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload ->> 'event' = 'update'
                AND payload ->> 'object_id' = $2
              ORDER BY (payload ->> 'account_id')::bigint",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(french.status_id.to_string())
        .fetch_all(&pool)
        .await?;
        assert_eq!(french_recipients, vec![ALICE, API_MODERATOR]);
        writer
            .delete_status(&authenticated, french.status_id, false)
            .await?;
        let french_delete_recipients = sqlx::query_scalar::<_, i64>(
            "SELECT (payload ->> 'account_id')::bigint
               FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload ->> 'event' = 'delete'
                AND payload ->> 'object_id' = $2
              ORDER BY (payload ->> 'account_id')::bigint",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(french.status_id.to_string())
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            french_delete_recipients,
            vec![ALICE, MODERATOR, API_MODERATOR]
        );

        sqlx::query("DELETE FROM mutes WHERE account_id = $1 AND target_account_id = $2")
            .bind(MODERATOR)
            .bind(ALICE)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO mutes
                (account_id, target_account_id, hide_notifications, expires_at,
                 created_at, updated_at)
             VALUES ($1, $2, false, NULL, clock_timestamp(), clock_timestamp())",
        )
        .bind(MODERATOR)
        .bind(ALICE)
        .execute(&pool)
        .await?;
        let muted = writer
            .create_status(
                &authenticated,
                "Muted stream filter status",
                &[],
                None,
                Some(false),
                Some("public"),
                None,
                None,
                None,
            )
            .await?;
        status_ids.push(muted.status_id);
        let muted_recipients = sqlx::query_scalar::<_, i64>(
            "SELECT (payload ->> 'account_id')::bigint
               FROM rustodon.outbox_events
              WHERE kind = $1
                AND payload ->> 'event' = 'update'
                AND payload ->> 'object_id' = $2
              ORDER BY (payload ->> 'account_id')::bigint",
        )
        .bind(STREAM_EVENT_KIND)
        .bind(muted.status_id.to_string())
        .fetch_all(&pool)
        .await?;
        assert_eq!(muted_recipients, vec![ALICE, API_MODERATOR]);
        Ok(())
    }
    .await;

    let status_id_strings = status_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let conversation_ids = sqlx::query_scalar::<_, i64>(
        "SELECT conversation_id FROM statuses
          WHERE id = ANY($1) AND conversation_id IS NOT NULL",
    )
    .bind(&status_ids)
    .fetch_all(&pool)
    .await?;
    let conversation_id_strings = conversation_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    sqlx::query("DELETE FROM follows WHERE id = 8006")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO follows SELECT * FROM jsonb_populate_record(NULL::follows, $1)")
        .bind(follow_before)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM mutes WHERE account_id = $1 AND target_account_id = $2")
        .bind(MODERATOR)
        .bind(ALICE)
        .execute(&pool)
        .await?;
    if let Some(mute_before) = mute_before {
        sqlx::query("INSERT INTO mutes SELECT * FROM jsonb_populate_record(NULL::mutes, $1)")
            .bind(mute_before)
            .execute(&pool)
            .await?;
    }
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE kind = $1
            AND payload -> 'arguments' ->> 'activity_id' IN (
              SELECT id::text FROM mentions WHERE status_id = ANY($2)
            )",
    )
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(&status_ids)
    .execute(&pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE kind = $1
            AND (payload ->> 'object_id' = ANY($2)
              OR payload ->> 'object_id' = ANY($3))",
    )
    .bind(STREAM_EVENT_KIND)
    .bind(&status_id_strings)
    .bind(&conversation_id_strings)
    .execute(&pool)
    .await?;
    sqlx::query(
        "DELETE FROM rustodon.outbox_events
          WHERE payload -> 'arguments' ->> 'status_id' = ANY($1)",
    )
    .bind(&status_id_strings)
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM mentions WHERE status_id = ANY($1)")
        .bind(&status_ids)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM statuses WHERE id = ANY($1)")
        .bind(&status_ids)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM status_stats WHERE status_id = ANY($1)")
        .bind(&status_ids)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM conversations WHERE id = ANY($1)")
        .bind(&conversation_ids)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(ALICE)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO account_stats SELECT * FROM jsonb_populate_record(NULL::account_stats, $1)",
    )
    .bind(&before_stats)
    .execute(&pool)
    .await?;
    result
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
async fn conversation_updates_conflict_concurrently() -> Result<(), Box<dyn Error>> {
    let database_url = database_url();
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let owner = sqlx::PgPool::connect(&owner_url).await?;
    let mut operational_connection = PgConnection::connect(&owner_url).await?;
    migrate(&mut operational_connection).await?;
    let before: Value = sqlx::query_scalar(
        "SELECT to_jsonb(conversation) FROM account_conversations conversation WHERE id = $1",
    )
    .bind(9302_i64)
    .fetch_one(&owner)
    .await?;
    sqlx::query(
        "UPDATE account_conversations SET status_ids = $1, last_status_id = $2, lock_version = 0 \
         WHERE id = $3",
    )
    .bind(vec![DIRECT_STATUS])
    .bind(DIRECT_STATUS)
    .bind(9302_i64)
    .execute(&owner)
    .await?;
    let expected_lock_version: i32 =
        sqlx::query_scalar("SELECT lock_version FROM account_conversations WHERE id = $1")
            .bind(9302_i64)
            .fetch_one(&owner)
            .await?;
    let result = async {
        let writer = WriteRepository::connect(&owner_url).await?;
        let authenticator = BearerAuthenticator::new(Repository::connect(&database_url).await?);
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let authenticated = authenticator
            .authenticate(&headers, WRITE_CONVERSATIONS)
            .await?;
        let barrier = Arc::new(Barrier::new(2));
        let first_barrier = Arc::clone(&barrier);
        let second_barrier = Arc::clone(&barrier);
        let first_writer = writer.clone();
        let second_writer = writer.clone();
        let first_authenticated = authenticated.clone();
        let second_authenticated = authenticated;
        let first = async move {
            first_barrier.wait().await;
            first_writer
                .update_conversation_unread_with_lock_version(
                    &first_authenticated,
                    9302,
                    false,
                    expected_lock_version,
                )
                .await
        };
        let second = async move {
            second_barrier.wait().await;
            second_writer
                .update_conversation_unread_with_lock_version(
                    &second_authenticated,
                    9302,
                    true,
                    expected_lock_version,
                )
                .await
        };
        let (first, second) = tokio::join!(first, second);
        assert_eq!(
            [first.is_ok(), second.is_ok()]
                .into_iter()
                .filter(|ok| *ok)
                .count(),
            1,
            "first={first:?} second={second:?}"
        );
        assert_eq!(
            [
                matches!(first, Err(WriteError::Conflict)),
                matches!(second, Err(WriteError::Conflict))
            ]
            .into_iter()
            .filter(|conflict| *conflict)
            .count(),
            1
        );
        Ok::<(), Box<dyn Error>>(())
    }
    .await;
    sqlx::query("DELETE FROM account_conversations WHERE id = $1")
        .bind(9302_i64)
        .execute(&owner)
        .await?;
    sqlx::query(
        "INSERT INTO account_conversations SELECT * FROM jsonb_populate_record(NULL::account_conversations, $1)",
    )
    .bind(before)
    .execute(&owner)
    .await?;
    result
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_creates_core_notifications_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let request = NotificationCreate {
        recipient_account_id: ALICE,
        activity: NotificationActivity::Status {
            id: 116_845_078_118_405_101,
        },
        silenced: false,
    };
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE account_id = $1 \
         AND activity_id = $2 AND activity_type = 'Status' AND type = 'status'",
    )
    .bind(ALICE)
    .bind(116_845_078_118_405_101_i64)
    .fetch_one(&pool)
    .await?;
    assert_eq!(before, 0);
    let first = writer.create_notification(request).await?;
    let second = writer.create_notification(request).await?;
    let (id, filtered) = match first {
        NotificationCreateOutcome::Created { id, filtered } => (id, filtered),
        other => panic!("expected a new notification, got {other:?}"),
    };
    assert!(!filtered);
    assert_eq!(
        second,
        NotificationCreateOutcome::Existing {
            id,
            filtered: false
        }
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications WHERE account_id = $1 \
             AND activity_id = $2 AND activity_type = 'Status' AND type = 'status'",
        )
        .bind(ALICE)
        .bind(116_845_078_118_405_101_i64)
        .fetch_one(&pool)
        .await?,
        1
    );
    sqlx::query("DELETE FROM notifications WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;
    let mention_id: i64 =
        sqlx::query_scalar("SELECT id FROM mentions WHERE account_id = $1 ORDER BY id LIMIT 1")
            .bind(ALICE)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        writer
            .create_notification(NotificationCreate {
                recipient_account_id: BOB,
                activity: NotificationActivity::Mention { id: mention_id },
                silenced: false,
            })
            .await?,
        NotificationCreateOutcome::Dropped
    );
    let follow_request_id: i64 = sqlx::query_scalar(
        "SELECT id FROM follow_requests WHERE target_account_id = $1 ORDER BY id LIMIT 1",
    )
    .bind(ALICE)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        writer
            .create_notification(NotificationCreate {
                recipient_account_id: BOB,
                activity: NotificationActivity::FollowRequest {
                    id: follow_request_id,
                },
                silenced: false,
            })
            .await?,
        NotificationCreateOutcome::Dropped
    );
    let cases = [
        (
            ALICE,
            NotificationActivity::Mention { id: 7001 },
            "Mention",
            "mention",
            BOB,
            true,
            None,
        ),
        (
            ALICE,
            NotificationActivity::Status {
                id: 116_845_101_711_365_104,
            },
            "Status",
            "status",
            BOB,
            true,
            None,
        ),
        (
            ALICE,
            NotificationActivity::Reblog {
                id: 116_845_321_912_325_301,
            },
            "Status",
            "reblog",
            BOB,
            true,
            Some("reblog-116844842188805001-495255"),
        ),
        (
            ALICE,
            NotificationActivity::Follow { id: 8002 },
            "Follow",
            "follow",
            BOB,
            true,
            Some("follow-495252"),
        ),
        (
            ALICE,
            NotificationActivity::FollowRequest { id: 8003 },
            "FollowRequest",
            "follow_request",
            116_844_606_259_202_002,
            false,
            None,
        ),
        (
            ALICE,
            NotificationActivity::Favourite { id: 8101 },
            "Favourite",
            "favourite",
            BOB,
            true,
            Some("favourite-116844842188805001-495255"),
        ),
        (
            ALICE,
            NotificationActivity::Poll { id: 8201 },
            "Poll",
            "poll",
            BOB,
            true,
            None,
        ),
        (
            ALICE,
            NotificationActivity::Update {
                id: 116_845_105_643_525_105,
            },
            "Status",
            "update",
            BOB,
            true,
            None,
        ),
        (
            ALICE,
            NotificationActivity::SeveredRelationships { id: 8302 },
            "AccountRelationshipSeveranceEvent",
            "severed_relationships",
            ALICE,
            true,
            None,
        ),
        (
            ALICE,
            NotificationActivity::ModerationWarning { id: 8401 },
            "AccountWarning",
            "moderation_warning",
            ALICE,
            true,
            None,
        ),
        (
            ALICE,
            NotificationActivity::AnnualReport { id: 8501 },
            "GeneratedAnnualReport",
            "annual_report",
            ALICE,
            true,
            None,
        ),
        (
            API_MODERATOR,
            NotificationActivity::AdminSignUp { id: NEWBIE },
            "Account",
            "admin.sign_up",
            NEWBIE,
            true,
            Some("admin.sign_up-495252"),
        ),
        (
            API_MODERATOR,
            NotificationActivity::AdminReport { id: 8601 },
            "Report",
            "admin.report",
            ALICE,
            true,
            None,
        ),
        (
            ALICE,
            NotificationActivity::Quote {
                id: 116_845_317_980_168_701,
            },
            "Quote",
            "quote",
            BOB,
            true,
            None,
        ),
        (
            ALICE,
            NotificationActivity::QuotedUpdate {
                id: 116_845_314_048_005_201,
            },
            "Status",
            "quoted_update",
            BOB,
            true,
            None,
        ),
        (
            ALICE,
            NotificationActivity::AddedToCollection {
                id: 116_845_549_977_608_802,
            },
            "CollectionItem",
            "added_to_collection",
            BOB,
            true,
            None,
        ),
        (
            ALICE,
            NotificationActivity::CollectionUpdate {
                id: 116_845_549_977_608_801,
            },
            "Collection",
            "collection_update",
            BOB,
            true,
            None,
        ),
    ];
    for (
        recipient_account_id,
        activity,
        activity_type,
        notification_type,
        expected_from_account_id,
        should_create,
        expected_group_key,
    ) in cases
    {
        sqlx::query(
            "DELETE FROM notifications WHERE account_id = $1 AND activity_id = $2 \
             AND activity_type = $3 AND type = $4",
        )
        .bind(recipient_account_id)
        .bind(match activity {
            NotificationActivity::Mention { id }
            | NotificationActivity::Status { id }
            | NotificationActivity::Reblog { id }
            | NotificationActivity::Follow { id }
            | NotificationActivity::FollowRequest { id }
            | NotificationActivity::Favourite { id }
            | NotificationActivity::Poll { id }
            | NotificationActivity::Update { id }
            | NotificationActivity::SeveredRelationships { id }
            | NotificationActivity::ModerationWarning { id }
            | NotificationActivity::AnnualReport { id }
            | NotificationActivity::AdminSignUp { id }
            | NotificationActivity::AdminReport { id }
            | NotificationActivity::Quote { id }
            | NotificationActivity::QuotedUpdate { id }
            | NotificationActivity::AddedToCollection { id }
            | NotificationActivity::CollectionUpdate { id } => id,
        })
        .bind(activity_type)
        .bind(notification_type)
        .execute(&pool)
        .await?;
        let outcome = writer
            .create_notification(NotificationCreate {
                recipient_account_id,
                activity,
                silenced: false,
            })
            .await?;
        if !should_create {
            assert_eq!(outcome, NotificationCreateOutcome::Dropped);
            continue;
        }
        let id = match outcome {
            NotificationCreateOutcome::Created { id, filtered } => {
                assert!(!filtered);
                id
            }
            other => panic!("expected {notification_type} to be created, got {other:?}"),
        };
        let row: (String, i64, Option<String>) = sqlx::query_as(
            "SELECT activity_type, from_account_id, group_key FROM notifications WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(row.0, activity_type);
        assert_eq!(row.1, expected_from_account_id);
        assert_eq!(row.2.as_deref(), expected_group_key);
        let retry = writer
            .create_notification(NotificationCreate {
                recipient_account_id,
                activity,
                silenced: false,
            })
            .await?;
        if matches!(
            notification_type,
            "update" | "quoted_update" | "collection_update"
        ) {
            let replacement_id = match retry {
                NotificationCreateOutcome::Created { id, filtered } => {
                    assert!(!filtered);
                    id
                }
                other => panic!("expected {notification_type} replacement, got {other:?}"),
            };
            assert_ne!(replacement_id, id);
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notifications WHERE id = $1",)
                    .bind(id)
                    .fetch_one(&pool)
                    .await?,
                0
            );
            sqlx::query("DELETE FROM notifications WHERE id = $1")
                .bind(replacement_id)
                .execute(&pool)
                .await?;
        } else {
            assert_eq!(
                retry,
                NotificationCreateOutcome::Existing {
                    id,
                    filtered: false,
                }
            );
        }
        sqlx::query("DELETE FROM notifications WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn write_repository_notifies_active_followers_for_statuses()
-> Result<(), Box<dyn std::error::Error>> {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let writer = WriteRepository::connect(&owner_url).await?;
    let pool = sqlx::PgPool::connect(&owner_url).await?;
    let follow_before: Value =
        sqlx::query_scalar("SELECT to_jsonb(follow) FROM follows follow WHERE id = 8006")
            .fetch_one(&pool)
            .await?;
    let sign_in_before: Option<NaiveDateTime> =
        sqlx::query_scalar("SELECT current_sign_in_at FROM users WHERE account_id = $1")
            .bind(MODERATOR)
            .fetch_one(&pool)
            .await?;
    let notifications_before: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(notification) FROM notifications notification \
         WHERE account_id = $1 AND activity_id = $2 \
           AND activity_type = 'Status' AND type = 'status' ORDER BY id",
    )
    .bind(MODERATOR)
    .bind(PUBLIC_STATUS)
    .fetch_all(&pool)
    .await?;
    let configured_active_days = std::env::var("USER_ACTIVE_DAYS")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(7);
    let operation = async {
        sqlx::query(
            "DELETE FROM notifications WHERE account_id = $1 AND activity_id = $2 \
             AND activity_type = 'Status' AND type = 'status'",
        )
        .bind(MODERATOR)
        .bind(PUBLIC_STATUS)
        .execute(&pool)
        .await?;
        sqlx::query("UPDATE follows SET notify = true, languages = ARRAY['fr'] WHERE id = 8006")
            .execute(&pool)
            .await?;
        sqlx::query(
            "UPDATE users SET current_sign_in_at = clock_timestamp() WHERE account_id = $1",
        )
        .bind(MODERATOR)
        .execute(&pool)
        .await?;
        writer.notify_status_followers(PUBLIC_STATUS).await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications WHERE account_id = $1 \
                 AND activity_id = $2 AND activity_type = 'Status' AND type = 'status'",
            )
            .bind(MODERATOR)
            .bind(PUBLIC_STATUS)
            .fetch_one(&pool)
            .await?,
            0
        );
        sqlx::query("UPDATE follows SET languages = NULL WHERE id = 8006")
            .execute(&pool)
            .await?;
        writer.notify_status_followers(PUBLIC_STATUS).await?;
        writer.notify_status_followers(PUBLIC_STATUS).await?;
        let created: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notifications WHERE account_id = $1 \
             AND activity_id = $2 AND activity_type = 'Status' AND type = 'status'",
        )
        .bind(MODERATOR)
        .bind(PUBLIC_STATUS)
        .fetch_one(&pool)
        .await?;
        assert_eq!(created, 1);
        sqlx::query(
            "DELETE FROM notifications WHERE account_id = $1 AND activity_id = $2 \
             AND activity_type = 'Status' AND type = 'status'",
        )
        .bind(MODERATOR)
        .bind(PUBLIC_STATUS)
        .execute(&pool)
        .await?;
        sqlx::query(
            "UPDATE users SET current_sign_in_at = clock_timestamp() - make_interval(days => 8) \
             WHERE account_id = $1",
        )
        .bind(MODERATOR)
        .execute(&pool)
        .await?;
        writer.notify_status_followers(PUBLIC_STATUS).await?;
        let old_follower_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notifications WHERE account_id = $1 \
             AND activity_id = $2 AND activity_type = 'Status' AND type = 'status'",
        )
        .bind(MODERATOR)
        .bind(PUBLIC_STATUS)
        .fetch_one(&pool)
        .await?;
        assert_eq!(old_follower_count, i64::from(configured_active_days > 8));
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    sqlx::query(
        "DELETE FROM notifications WHERE account_id = $1 AND activity_id = $2 \
         AND activity_type = 'Status' AND type = 'status'",
    )
    .bind(MODERATOR)
    .bind(PUBLIC_STATUS)
    .execute(&pool)
    .await?;
    for notification in notifications_before {
        sqlx::query(
            "INSERT INTO notifications \
             SELECT * FROM jsonb_populate_record(NULL::notifications, $1)",
        )
        .bind(notification)
        .execute(&pool)
        .await?;
    }
    sqlx::query("DELETE FROM follows WHERE id = 8006")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO follows SELECT * FROM jsonb_populate_record(NULL::follows, $1)")
        .bind(follow_before)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE users SET current_sign_in_at = $1 WHERE account_id = $2")
        .bind(sign_in_before)
        .bind(MODERATOR)
        .execute(&pool)
        .await?;
    operation
}

#[tokio::test]
#[ignore = "starts a restored Mastodon PostgreSQL fixture through the Mise task"]
#[allow(clippy::too_many_lines)]
async fn notification_group_bucket_survives_last_row_deletion()
-> Result<(), Box<dyn std::error::Error>> {
    let owner_url = std::env::var("RUSTODON_MASTODON_OWNER_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_OWNER_DATABASE_URL");
    let mut connection = PgConnection::connect(&owner_url).await?;
    migrate(&mut connection).await?;
    sqlx::query("DELETE FROM rustodon.ordering_markers WHERE kind = 'notification_group'")
        .execute(&mut connection)
        .await?;
    let follow_rows: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(follow) FROM follows follow WHERE id = ANY($1) ORDER BY id",
    )
    .bind([8002_i64, 8007_i64])
    .fetch_all(&mut connection)
    .await?;
    let notification_rows: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(notification) FROM notifications notification \
         WHERE account_id = $1 AND activity_id = ANY($2) ORDER BY id",
    )
    .bind(ALICE)
    .bind([8002_i64, 8007_i64])
    .fetch_all(&mut connection)
    .await?;
    let writer = WriteRepository::connect(&owner_url).await?;

    let result = async {
        sqlx::query(
            "DELETE FROM notifications WHERE account_id = $1 AND activity_id = $2 \
             AND activity_type = 'Follow' AND type = 'follow'",
        )
        .bind(ALICE)
        .bind(8002_i64)
        .execute(&mut connection)
        .await?;
        let first = writer
            .create_notification(NotificationCreate {
                recipient_account_id: ALICE,
                activity: NotificationActivity::Follow { id: 8002 },
                silenced: false,
            })
            .await?;
        let first_id = match first {
            NotificationCreateOutcome::Created { id, filtered: false } => id,
            other => return Err(format!("expected the first grouped notification, got {other:?}").into()),
        };
        let first_group_key: String =
            sqlx::query_scalar("SELECT group_key FROM notifications WHERE id = $1")
                .bind(first_id)
                .fetch_one(&mut connection)
                .await?;
        sqlx::query("DELETE FROM notifications WHERE id = $1")
            .bind(first_id)
            .execute(&mut connection)
            .await?;
        sqlx::query("DELETE FROM follows WHERE id = $1")
            .bind(8002_i64)
            .execute(&mut connection)
            .await?;
        sqlx::query(
            "UPDATE follows SET account_id = $1, created_at = $2 WHERE id = $3",
        )
        .bind(BOB)
        .bind(NaiveDateTime::parse_from_str(
            "2026-07-01 18:00:00",
            "%Y-%m-%d %H:%M:%S",
        )?)
        .bind(8007_i64)
        .execute(&mut connection)
        .await?;
        let second = writer
            .create_notification(NotificationCreate {
                recipient_account_id: ALICE,
                activity: NotificationActivity::Follow { id: 8007 },
                silenced: false,
            })
            .await?;
        let second_id = match second {
            NotificationCreateOutcome::Created { id, filtered: false } => id,
            other => return Err(format!("expected the second grouped notification, got {other:?}").into()),
        };
        let second_group_key: String =
            sqlx::query_scalar("SELECT group_key FROM notifications WHERE id = $1")
                .bind(second_id)
                .fetch_one(&mut connection)
                .await?;
        if second_group_key != first_group_key {
            return Err(format!(
                "group bucket changed after deleting the last row: first={first_group_key}, second={second_group_key}"
            )
            .into());
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    sqlx::query("DELETE FROM notifications WHERE account_id = $1 AND activity_id = ANY($2)")
        .bind(ALICE)
        .bind([8002_i64, 8007_i64])
        .execute(&mut connection)
        .await?;
    sqlx::query("DELETE FROM follows WHERE id = ANY($1)")
        .bind([8002_i64, 8007_i64])
        .execute(&mut connection)
        .await?;
    for row in follow_rows {
        sqlx::query("INSERT INTO follows SELECT * FROM jsonb_populate_record(NULL::follows, $1)")
            .bind(row)
            .execute(&mut connection)
            .await?;
    }
    for row in notification_rows {
        sqlx::query(
            "INSERT INTO notifications SELECT * FROM jsonb_populate_record(NULL::notifications, $1)",
        )
        .bind(row)
        .execute(&mut connection)
        .await?;
    }
    sqlx::query("DELETE FROM rustodon.ordering_markers WHERE kind = 'notification_group'")
        .execute(&mut connection)
        .await?;
    result
}

#[tokio::test]
#[ignore = "requires the restored Mastodon fixture database"]
async fn hashtag_search_matches_normalized_prefixes_and_relationships() -> Result<(), Box<dyn Error>>
{
    let loader = RestProjectionLoader::new(
        Repository::connect(&database_url()).await?,
        Some(ALICE),
        "fixture-v4-6-5.rustodon.invalid",
    );

    let tags = loader.tag_search("#Fixt", 20, 0).await?;
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].id, 9201);
    assert_eq!(tags[0].name, "fixturetag");
    assert_eq!(tags[0].display_name.as_deref(), Some("FixtureTag"));
    assert_eq!(tags[0].following, Some(true));
    assert_eq!(tags[0].featuring, Some(true));
    assert!(loader.tag_search("#missing", 20, 0).await?.is_empty());
    Ok(())
}
