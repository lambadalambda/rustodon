use http::HeaderMap;
use http::header::{AUTHORIZATION, HeaderValue};
use rustodon::mastodon::rest::{
    AccountListKind, AccountListOptions, AccountStatusesOptions, FollowCollectionKind,
    FollowCollectionOptions, RestProjectionLoader, RestSerializer, SavedStatusKind,
    SavedStatusesOptions, TagTimelineOptions, TimelineOptions,
};
use rustodon::mastodon::{
    AccountIdScheme, AccountKind, BearerAuthenticator, InvalidTokenReason, NotificationType,
    OAuthAuthenticationError, OAuthError, READ_ACCOUNTS, READ_STATUSES, Repository,
    StatusVisibility,
};
use rustodon::paperclip::PaperclipAttachment;
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use url::Url;

const ALICE: i64 = 116_844_606_259_201_001;
const NEWBIE: i64 = 116_844_606_259_201_003;
const BOB: i64 = 116_844_606_259_202_001;
const API_MODERATOR: i64 = 116_844_606_259_201_004;
const MATRIX_VIEWER: i64 = -323;
const PUBLIC_STATUS: i64 = 116_844_842_188_805_001;
const UNLISTED_STATUS: i64 = 116_844_846_120_965_002;
const PRIVATE_STATUS: i64 = 116_844_850_053_125_003;
const DIRECT_STATUS: i64 = 116_844_853_985_285_004;
const LIMITED_STATUS: i64 = 116_844_857_917_445_005;
const DELETED_UNKNOWN_STATUS: i64 = 116_846_257_766_400_501;

fn database_url() -> String {
    std::env::var("RUSTODON_MASTODON_DATABASE_URL")
        .expect("the Podman fixture task must provide RUSTODON_MASTODON_DATABASE_URL")
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
    assert!(repository.notification_policy(ALICE).await?.is_some());
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
