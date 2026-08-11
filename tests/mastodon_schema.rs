use rustodon::mastodon::{
    AccountIdScheme, AccountKind, NotificationType, Repository, StatusVisibility,
};
use sqlx::{Connection, PgConnection};

const ALICE: i64 = 116_844_606_259_201_001;
const NEWBIE: i64 = 116_844_606_259_201_003;
const BOB: i64 = 116_844_606_259_202_001;
const PUBLIC_STATUS: i64 = 116_844_842_188_805_001;
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
    assert_eq!(repository.follows(ALICE).await?.len(), 1);
    assert_eq!(repository.follow_requests(ALICE).await?.len(), 1);
    assert_eq!(repository.favourites(BOB).await?.len(), 1);
    assert_eq!(repository.bookmarks(ALICE).await?.len(), 1);
    assert_eq!(repository.blocks(ALICE).await?.len(), 1);
    assert_eq!(repository.mutes(ALICE).await?.len(), 1);
    assert_eq!(repository.account_domain_blocks(ALICE).await?.len(), 1);
    assert_eq!(repository.lists(ALICE).await?.len(), 2);
    assert_eq!(repository.list_accounts(9001).await?.len(), 1);
    assert_eq!(repository.status_pins(ALICE).await?.len(), 1);
    assert_eq!(repository.featured_tags(ALICE).await?.len(), 1);
    assert_eq!(repository.account_tags(ALICE).await?.len(), 1);

    assert_eq!(repository.custom_filters(ALICE).await?.len(), 1);
    assert_eq!(repository.custom_filter_keywords(9101).await?.len(), 1);
    assert_eq!(repository.custom_filter_statuses(9101).await?.len(), 1);

    let notifications = repository.notifications(ALICE).await?;
    let moderator_notifications = repository.notifications(116_844_606_259_201_002).await?;
    assert_eq!(notifications.len() + moderator_notifications.len(), 17);
    assert!(
        notifications
            .iter()
            .all(|notification| !notification.filtered)
    );
    let all_notifications = repository.notifications_including_filtered(ALICE).await?;
    assert_eq!(all_notifications.len(), 18);
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
    assert_eq!(settings.len(), 2);
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

    let quotes = repository.quotes(ALICE).await?;
    assert_eq!(quotes.len(), 2);
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
