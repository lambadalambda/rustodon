use chrono::NaiveDateTime;
use rustodon::mastodon::rest::{
    AccountFieldProjection, AccountProjection, AnnouncementProjection,
    AnnouncementReactionProjection, CustomEmojiProjection, MediaAttachmentProjection,
    MentionProjection, NotificationProjection, PollOptionProjection, PollProjection,
    QuoteProjection, QuoteTargetAccess, QuoteTargetLinkProjection, RestSerializer,
    StatusApplicationProjection, StatusProjection, StatusShape, StatusViewerProjection,
    TagHistoryProjection, TagProjection,
};
use rustodon::mastodon::{AccountIdScheme, NotificationType};
use serde_json::{Value, json};
use url::Url;

const ALICE: i64 = 116_844_606_259_201_001;
const PUBLIC_STATUS: i64 = 116_844_842_188_805_001;

fn timestamp(value: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").expect("fixed timestamp")
}

fn serializer() -> RestSerializer<'static> {
    let origin = Box::leak(Box::new(
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/").unwrap(),
    ));
    RestSerializer::new(
        origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        timestamp("2026-08-11 12:00:00"),
    )
}

fn alice() -> AccountProjection {
    AccountProjection {
        id: ALICE,
        username: "alice".to_owned(),
        domain: None,
        actor_type: Some("Person".to_owned()),
        id_scheme: Some(AccountIdScheme::Username),
        display_name: "Alice Fixture".to_owned(),
        note: "Primary local fixture account".to_owned(),
        stored_uri: String::new(),
        stored_url: None,
        locked: false,
        discoverable: Some(true),
        indexable: true,
        memorial: false,
        moved: None,
        suspended: false,
        limited: false,
        sensitized: false,
        created_at: timestamp("2026-07-01 12:00:00"),
        avatar_file_name: Some("0112603425bb49c1.png".to_owned()),
        avatar_content_type: Some("image/png".to_owned()),
        avatar_storage_schema_version: None,
        avatar_description: "Deterministic Mastodon test avatar".to_owned(),
        header_file_name: None,
        header_content_type: None,
        header_storage_schema_version: None,
        header_description: String::new(),
        followers_count: 1,
        following_count: 1,
        statuses_count: 5,
        last_status_at: Some(timestamp("2026-07-01 13:00:00")),
        hide_collections: None,
        show_media: true,
        show_media_replies: true,
        show_featured: true,
        noindex: Some(false),
        feature_automatic: vec!["public".to_owned()],
        feature_manual: vec!["followers".to_owned()],
        feature_current_user: "denied".to_owned(),
        email_subscriptions: None,
        roles: Some(Vec::new()),
        emojis: Vec::new(),
        fields: Vec::new(),
        profile_mentions: Vec::new(),
    }
}

fn public_status() -> StatusProjection {
    StatusProjection {
        id: PUBLIC_STATUS,
        account: alice(),
        text: "Public fixture status with local media".to_owned(),
        spoiler_text: String::new(),
        visibility: 0,
        local: true,
        stored_uri: None,
        stored_url: None,
        language: Some("en".to_owned()),
        sensitive: false,
        in_reply_to_id: None,
        in_reply_to_account_id: None,
        replies_count: 1,
        reblogs_count: 2,
        favourites_count: 3,
        quotes_count: 4,
        edited_at: None,
        created_at: timestamp("2026-07-01 13:00:00"),
        viewer: None,
        reblog: None,
        show_application: true,
        application: Some(StatusApplicationProjection {
            name: "Rustodon fixture client".to_owned(),
            website: Some(String::new()),
        }),
        media_attachments: Vec::new(),
        mentions: Vec::new(),
        tags: vec![TagProjection {
            id: 9_201,
            name: "fixturetag".to_owned(),
            display_name: Some("FixtureTag".to_owned()),
            history: vec![TagHistoryProjection {
                day: "1786320000".to_owned(),
                accounts: "0".to_owned(),
                uses: "0".to_owned(),
            }],
            following: None,
            featuring: None,
        }],
        emojis: Vec::new(),
        tagged_collections: Vec::new(),
        quote: None,
        card: None,
        poll: None,
        quote_automatic: vec!["public".to_owned()],
        quote_manual: Vec::new(),
        quote_current_user: "denied".to_owned(),
    }
}

#[test]
fn announcement_serialization_matches_authenticated_client_contract() {
    let emoji = CustomEmojiProjection {
        id: 12_001,
        shortcode: "fixtureparty".to_owned(),
        domain: None,
        file_name: "fixtureparty.png".to_owned(),
        storage_schema_version: Some(1),
        visible_in_picker: true,
        category: None,
        featured: None,
    };
    let announcement = AnnouncementProjection {
        id: 8_701,
        text: "Hello @alice #FixtureTag :fixtureparty:".to_owned(),
        starts_at: Some(timestamp("2026-07-01 17:36:00")),
        ends_at: None,
        all_day: false,
        published_at: Some(timestamp("2026-07-01 17:36:00")),
        updated_at: timestamp("2026-07-01 17:37:00"),
        read: true,
        mentions: vec![MentionProjection { account: alice() }],
        statuses: vec![public_status()],
        tags: vec!["fixturetag".to_owned()],
        emojis: vec![emoji.clone()],
        reactions: vec![AnnouncementReactionProjection {
            name: "fixtureparty".to_owned(),
            count: 2,
            me: true,
            custom_emoji: Some(emoji),
        }],
    };

    let value = serde_json::to_value(serializer().announcement(&announcement).unwrap()).unwrap();
    assert_eq!(value["id"], "8701");
    assert_eq!(value["read"], true);
    assert_eq!(value["mentions"][0]["acct"], "alice");
    assert_eq!(value["statuses"][0]["id"], PUBLIC_STATUS.to_string());
    assert_eq!(
        value["tags"],
        json!([{
            "name": "fixturetag",
            "url": "https://fixture-v4-6-5.rustodon.invalid/tags/fixturetag"
        }])
    );
    assert_eq!(value["emojis"][0]["shortcode"], "fixtureparty");
    assert_eq!(value["reactions"][0]["count"], 2);
    assert_eq!(value["reactions"][0]["me"], true);
    assert!(value["reactions"][0]["url"].is_string());
}

#[test]
fn local_account_serialization_matches_canonical_urls_dates_and_nulls() {
    let value = serde_json::to_value(serializer().account(&alice()).unwrap()).unwrap();
    assert_eq!(value["id"], ALICE.to_string());
    assert_eq!(value["acct"], "alice");
    assert_eq!(value["created_at"], "2026-07-01T00:00:00.000Z");
    assert_eq!(value["last_status_at"], "2026-07-01");
    assert_eq!(value["note"], "<p>Primary local fixture account</p>");
    assert_eq!(
        value["url"],
        "https://fixture-v4-6-5.rustodon.invalid/@alice"
    );
    assert_eq!(
        value["uri"],
        "https://fixture-v4-6-5.rustodon.invalid/users/alice"
    );
    assert_eq!(
        value["avatar"],
        "https://fixture-v4-6-5.rustodon.invalid/system/accounts/avatars/116/844/606/259/201/001/original/0112603425bb49c1.png"
    );
    assert!(value["hide_collections"].is_null());
    assert_eq!(value["noindex"], false);
    assert_eq!(value["roles"], json!([]));
    assert!(value.get("suspended").is_none());
}

#[test]
fn custom_emoji_serialization_preserves_category_metadata_and_media_urls() {
    let value = serde_json::to_value(serializer().custom_emoji(&CustomEmojiProjection {
        id: 12_001,
        shortcode: "fixtureparty".to_owned(),
        domain: Some("remote.fixture.invalid".to_owned()),
        file_name: "fixtureparty.png".to_owned(),
        storage_schema_version: Some(1),
        visible_in_picker: true,
        category: Some("Fixture".to_owned()),
        featured: Some(true),
    }))
    .unwrap();
    assert_eq!(
        value["url"],
        "https://fixture-v4-6-5.rustodon.invalid/system/cache/custom_emojis/images/000/012/001/original/fixtureparty.png"
    );
    assert_eq!(
        value["static_url"],
        "https://fixture-v4-6-5.rustodon.invalid/system/cache/custom_emojis/images/000/012/001/static/fixtureparty.png"
    );
    assert_eq!(value["category"], "Fixture");
    assert_eq!(value["featured"], true);
    assert_eq!(value["visible_in_picker"], true);
}

#[test]
fn custom_emoji_without_category_omits_optional_category_fields() {
    let value = serde_json::to_value(serializer().custom_emoji(&CustomEmojiProjection {
        id: 12_001,
        shortcode: "fixtureparty".to_owned(),
        domain: Some("remote.fixture.invalid".to_owned()),
        file_name: "fixtureparty.png".to_owned(),
        storage_schema_version: Some(1),
        visible_in_picker: true,
        category: None,
        featured: None,
    }))
    .unwrap();
    assert!(value.get("category").is_none());
    assert!(value.get("featured").is_none());
}

#[test]
fn tag_serialization_preserves_display_name_history_and_relationships() {
    let value = serde_json::to_value(serializer().tag(&TagProjection {
        id: 9_201,
        name: "fixturetag".to_owned(),
        display_name: Some("FixtureTag".to_owned()),
        history: vec![TagHistoryProjection {
            day: "1782864000".to_owned(),
            accounts: "2".to_owned(),
            uses: "3".to_owned(),
        }],
        following: Some(true),
        featuring: Some(false),
    }))
    .unwrap();
    assert_eq!(value["id"], "9201");
    assert_eq!(value["name"], "FixtureTag");
    assert_eq!(
        value["url"],
        "https://fixture-v4-6-5.rustodon.invalid/tags/fixturetag"
    );
    assert_eq!(value["history"][0]["uses"], "3");
    assert_eq!(value["following"], true);
    assert_eq!(value["featuring"], false);
}

#[test]
fn remote_and_suspended_account_rules_are_explicit() {
    let mut remote = alice();
    remote.id = 116_844_606_259_202_001;
    remote.username = "bob".to_owned();
    remote.domain = Some("xn--bcher-kva.example".to_owned());
    remote.stored_uri = "https://remote.fixture.invalid/users/bob".to_owned();
    remote.stored_url = Some("javascript:unsafe()".to_owned());
    remote.note = "<p class=\"evil\">Remote <strong>bio</strong></p>".to_owned();
    remote.avatar_file_name = None;
    remote.roles = None;
    remote.fields = vec![AccountFieldProjection {
        name: "Fixture field".to_owned(),
        value: "<script>bad</script><b>Exact value</b>".to_owned(),
        verified_at: None,
    }];
    let value = serde_json::to_value(serializer().account(&remote).unwrap()).unwrap();
    assert_eq!(value["acct"], "bob@bücher.example");
    assert_eq!(value["url"], remote.stored_uri);
    assert_eq!(value["note"], "<p>Remote <strong>bio</strong></p>");
    assert_eq!(value["fields"][0]["value"], "<b>Exact value</b>");
    assert!(value.get("roles").is_none());
    assert!(value.get("noindex").is_none());

    remote.suspended = true;
    let suspended = serde_json::to_value(serializer().account(&remote).unwrap()).unwrap();
    assert_eq!(suspended["display_name"], "");
    assert_eq!(suspended["note"], "");
    assert_eq!(suspended["discoverable"], false);
    assert_eq!(suspended["fields"], json!([]));
    assert_eq!(suspended["suspended"], true);
}

#[test]
fn anonymous_status_has_required_nulls_and_masks_limited_visibility() {
    let mut status = public_status();
    status.visibility = 4;
    let value =
        serde_json::to_value(serializer().status(&status, StatusShape::Full).unwrap()).unwrap();
    assert_eq!(value["id"], PUBLIC_STATUS.to_string());
    assert_eq!(value["visibility"], "private");
    assert_eq!(
        value["content"],
        "<p>Public fixture status with local media</p>"
    );
    assert_eq!(
        value["application"],
        json!({"name": "Rustodon fixture client", "website": null})
    );
    assert_eq!(
        value["tags"],
        json!([{
            "name": "fixturetag",
            "url": "https://fixture-v4-6-5.rustodon.invalid/tags/fixturetag"
        }])
    );
    for key in [
        "reblog",
        "quote",
        "card",
        "poll",
        "edited_at",
        "in_reply_to_id",
    ] {
        assert!(value.get(key).is_some(), "missing {key}");
        assert!(value[key].is_null(), "{key} should be null");
    }
    for key in [
        "favourited",
        "reblogged",
        "muted",
        "bookmarked",
        "pinned",
        "filtered",
        "text",
    ] {
        assert!(value.get(key).is_none(), "unexpected {key}");
    }
}

#[test]
fn local_boost_uses_the_activity_url() {
    let mut boost = public_status();
    boost.id = -416;
    boost.reblog = Some(Box::new(public_status()));
    let value =
        serde_json::to_value(serializer().status(&boost, StatusShape::Full).unwrap()).unwrap();
    let activity = "https://fixture-v4-6-5.rustodon.invalid/users/alice/statuses/-416/activity";
    assert_eq!(value["uri"], activity);
    assert_eq!(value["url"], activity);
}

#[test]
fn unknown_notification_type_is_represented_without_guessing_a_target() {
    let notification = NotificationProjection {
        id: 10_018,
        notification_type: NotificationType::Unknown("future_event".to_owned()),
        created_at: timestamp("2026-07-01 18:00:18"),
        group_key: None,
        filtered: true,
        account: alice(),
        status: None,
        collection: None,
        report: None,
        event: None,
        moderation_warning: None,
        annual_report_year: None,
    };
    let value =
        serde_json::to_value(serializer().notification(&notification, None).unwrap()).unwrap();
    assert_eq!(value["type"], "future_event");
    assert_eq!(value["group_key"], "ungrouped-10018");
    assert_eq!(value["filtered"], true);
    for key in [
        "status",
        "collection",
        "report",
        "event",
        "moderation_warning",
    ] {
        assert!(value.get(key).is_none(), "unexpected guessed {key}");
    }
}

#[test]
fn authenticated_and_source_status_shapes_are_distinct() {
    let mut status = public_status();
    status.viewer = Some(StatusViewerProjection {
        viewer_account_id: ALICE,
        favourited: true,
        reblogged: false,
        muted: true,
        bookmarked: true,
        pinned: Some(true),
        filtered: Vec::new(),
    });
    let authenticated =
        serde_json::to_value(serializer().status(&status, StatusShape::Full).unwrap()).unwrap();
    assert_eq!(authenticated["favourited"], true);
    assert_eq!(authenticated["muted"], true);
    assert_eq!(authenticated["pinned"], true);
    assert_eq!(authenticated["filtered"], json!([]));

    let source =
        serde_json::to_value(serializer().status(&status, StatusShape::Source).unwrap()).unwrap();
    assert!(source.get("content").is_none());
    assert_eq!(source["text"], status.text);

    let source = serde_json::to_value(serializer().status_source(&status)).unwrap();
    assert_eq!(source.as_object().unwrap().len(), 3);
    assert_eq!(source["id"], status.id.to_string());
    assert_eq!(source["text"], status.text);
    assert_eq!(source["spoiler_text"], status.spoiler_text);
}

#[test]
fn shallow_quotes_do_not_disclose_unauthorized_status_ids() {
    let mut status = public_status();
    status.quote = Some(QuoteProjection {
        state: "unauthorized".to_owned(),
        accepted: true,
        quoted_status_id: Some(116_844_850_053_125_003),
        target_access: QuoteTargetAccess::Unauthorized,
        target_serializable: true,
        target_link: None,
        quoted_status: None,
    });

    let value =
        serde_json::to_value(serializer().status(&status, StatusShape::Shallow).unwrap()).unwrap();
    assert_eq!(value["quote"]["state"], "unauthorized");
    assert!(value["quote"]["quoted_status_id"].is_null());
}

#[test]
fn quote_targets_follow_source_and_reblog_rules() {
    let target = public_status();
    let mut pending = public_status();
    pending.quote = Some(QuoteProjection {
        state: "pending".to_owned(),
        accepted: false,
        quoted_status_id: Some(target.id),
        target_access: QuoteTargetAccess::Visible,
        target_serializable: true,
        target_link: Some(QuoteTargetLinkProjection {
            id: target.id,
            account: target.account.clone(),
            local: target.local,
            stored_url: target.stored_url.clone(),
        }),
        quoted_status: Some(Box::new(target.clone())),
    });
    let full =
        serde_json::to_value(serializer().status(&pending, StatusShape::Full).unwrap()).unwrap();
    let source =
        serde_json::to_value(serializer().status(&pending, StatusShape::Source).unwrap()).unwrap();
    assert!(full["quote"]["quoted_status"].is_null());
    assert_eq!(
        source["quote"]["quoted_status"]["id"],
        target.id.to_string()
    );

    pending.quote.as_mut().unwrap().target_access = QuoteTargetAccess::Unauthorized;
    let source =
        serde_json::to_value(serializer().status(&pending, StatusShape::Source).unwrap()).unwrap();
    assert_eq!(source["quote"]["state"], "pending");
    assert!(source["quote"]["quoted_status"].is_null());

    pending.quote.as_mut().unwrap().target_access = QuoteTargetAccess::Deleted;
    pending.quote.as_mut().unwrap().quoted_status = None;
    let source =
        serde_json::to_value(serializer().status(&pending, StatusShape::Source).unwrap()).unwrap();
    assert_eq!(source["quote"]["state"], "pending");
    assert!(source["quote"]["quoted_status"].is_null());

    let mut boost = target.clone();
    boost.id += 1;
    boost.reblog = Some(Box::new(target));
    pending.quote = Some(QuoteProjection {
        state: "accepted".to_owned(),
        accepted: true,
        quoted_status_id: Some(boost.id),
        target_access: QuoteTargetAccess::Visible,
        target_serializable: false,
        target_link: Some(QuoteTargetLinkProjection {
            id: boost.id,
            account: boost.account.clone(),
            local: boost.local,
            stored_url: boost.stored_url.clone(),
        }),
        quoted_status: Some(Box::new(boost)),
    });
    for shape in [StatusShape::Full, StatusShape::Shallow, StatusShape::Source] {
        let value = serde_json::to_value(serializer().status(&pending, shape).unwrap()).unwrap();
        let target_key = if shape == StatusShape::Shallow {
            "quoted_status_id"
        } else {
            "quoted_status"
        };
        assert!(value["quote"][target_key].is_null());
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn media_paths_proxy_fallbacks_and_poll_votes_match_wire_shapes() {
    let local = MediaAttachmentProjection {
        id: PUBLIC_STATUS + 1_000,
        media_type: 0,
        processing: Some(2),
        remote_url: String::new(),
        file_content_type: Some("image/jpeg".to_owned()),
        file_name: Some("image.jpg".to_owned()),
        file_storage_schema_version: None,
        thumbnail_file_name: None,
        thumbnail_storage_schema_version: None,
        thumbnail_remote_url: None,
        shortcode: None,
        meta: Some(json!({"original": {"width": 600}})),
        description: Some("Image".to_owned()),
        blurhash: Some("hash".to_owned()),
        discarded: false,
    };
    let local = serde_json::to_value(serializer().media_attachment(&local)).unwrap();
    assert_eq!(local["type"], "image");
    assert!(
        local["url"]
            .as_str()
            .unwrap()
            .contains("/system/media_attachments/files/")
    );
    assert!(
        local["preview_url"]
            .as_str()
            .unwrap()
            .contains("/small/image.jpg")
    );

    let remote = MediaAttachmentProjection {
        id: -101,
        media_type: 99,
        processing: None,
        remote_url: "https://media.fixture.invalid/first.jpg".to_owned(),
        file_content_type: None,
        file_name: None,
        file_storage_schema_version: None,
        thumbnail_file_name: None,
        thumbnail_storage_schema_version: None,
        thumbnail_remote_url: None,
        shortcode: None,
        meta: None,
        description: None,
        blurhash: None,
        discarded: false,
    };
    let remote = serde_json::to_value(serializer().media_attachment(&remote)).unwrap();
    assert_eq!(remote["type"], "unknown");
    assert_eq!(
        remote["url"],
        "https://fixture-v4-6-5.rustodon.invalid/media_proxy/-101/original"
    );

    let poll = PollProjection {
        id: 8_201,
        expires_at: Some(timestamp("2024-01-02 12:00:00")),
        multiple: false,
        votes_count: 1,
        voters_count: Some(1),
        options: vec![
            PollOptionProjection {
                title: "Tea".to_owned(),
                votes_count: Some(1),
            },
            PollOptionProjection {
                title: "Coffee".to_owned(),
                votes_count: Some(0),
            },
        ],
        emojis: Vec::new(),
        voted: Some(true),
        own_votes: Some(vec![0]),
    };
    let poll = serde_json::to_value(serializer().poll(&poll)).unwrap();
    assert_eq!(
        poll,
        json!({
            "id": "8201",
            "expires_at": "2024-01-02T12:00:00.000Z",
            "expired": true,
            "multiple": false,
            "votes_count": 1,
            "voters_count": 1,
            "options": [
                {"title": "Tea", "votes_count": 1},
                {"title": "Coffee", "votes_count": 0}
            ],
            "emojis": [],
            "voted": true,
            "own_votes": [0]
        })
    );

    let anonymous_hidden = PollProjection {
        id: 8_202,
        expires_at: Some(timestamp("2027-01-02 12:00:00")),
        multiple: true,
        votes_count: 3,
        voters_count: Some(2),
        options: vec![PollOptionProjection {
            title: "Hidden".to_owned(),
            votes_count: None,
        }],
        emojis: Vec::new(),
        voted: None,
        own_votes: None,
    };
    let anonymous_hidden = serde_json::to_value(serializer().poll(&anonymous_hidden)).unwrap();
    assert_eq!(anonymous_hidden["expired"], false);
    assert_eq!(anonymous_hidden["options"][0]["votes_count"], Value::Null);
    assert!(anonymous_hidden.get("voted").is_none());
    assert!(anonymous_hidden.get("own_votes").is_none());
}

#[test]
fn raw_gifs_use_image_rendering_without_reclassifying_transcoded_gifv() {
    let mut media = MediaAttachmentProjection {
        id: PUBLIC_STATUS + 1_001,
        media_type: 1,
        processing: Some(2),
        remote_url: "https://media.fixture.invalid/animation.gif".to_owned(),
        file_content_type: Some("image/gif".to_owned()),
        file_name: Some("animation.gif".to_owned()),
        file_storage_schema_version: Some(1),
        thumbnail_file_name: None,
        thumbnail_storage_schema_version: None,
        thumbnail_remote_url: None,
        shortcode: None,
        meta: Some(json!({"original": {"width": 320, "height": 240}})),
        description: Some("Animated image".to_owned()),
        blurhash: None,
        discarded: false,
    };
    let value = serde_json::to_value(serializer().media_attachment(&media)).unwrap();
    assert_eq!(value["type"], "image");
    assert!(
        value["url"]
            .as_str()
            .unwrap()
            .ends_with("/original/animation.gif")
    );
    assert!(
        value["preview_url"]
            .as_str()
            .unwrap()
            .ends_with("/small/animation.png")
    );
    assert_eq!(value["remote_url"], media.remote_url);
    assert_eq!(value["meta"], media.meta.clone().unwrap());
    assert_eq!(value["description"], "Animated image");

    for (kind, mime, name, expected) in [
        (1, Some("IMAGE/GIF"), "animation.gif", "image"),
        (1, Some("video/mp4"), "animation.mp4", "gifv"),
        (1, None, "animation.mp4", "gifv"),
        (2, Some("video/mp4"), "movie.mp4", "video"),
    ] {
        media.media_type = kind;
        media.file_content_type = mime.map(str::to_owned);
        media.file_name = Some(name.to_owned());
        let value = serde_json::to_value(serializer().media_attachment(&media)).unwrap();
        assert_eq!(value["type"], expected, "{kind}: {mime:?}");
        // The origin's .gif extension must not override the cached file format.
        assert_eq!(value["remote_url"], media.remote_url);
    }
}

#[test]
fn unknown_status_visibility_fails_closed() {
    let mut status = public_status();
    status.visibility = 99;
    let error = serializer()
        .status(&status, StatusShape::Full)
        .expect_err("unknown visibility must never be guessed");
    assert!(error.to_string().contains("unsupported visibility 99"));
}

#[test]
fn serializer_output_preserves_arbitrary_json_numbers() {
    let media = MediaAttachmentProjection {
        id: -1,
        media_type: 0,
        processing: Some(2),
        remote_url: String::new(),
        file_content_type: None,
        file_name: None,
        file_storage_schema_version: None,
        thumbnail_file_name: None,
        thumbnail_storage_schema_version: None,
        thumbnail_remote_url: None,
        shortcode: None,
        meta: Some(serde_json::from_str::<Value>("{\"n\":9007199254740993}").unwrap()),
        description: None,
        blurhash: None,
        discarded: false,
    };
    let value = serde_json::to_value(serializer().media_attachment(&media)).unwrap();
    assert_eq!(value["meta"]["n"].to_string(), "9007199254740993");
}

#[test]
fn media_root_urls_preserve_absolute_origins_and_derivative_extensions() {
    let origin = Url::parse("https://fixture-v4-6-5.rustodon.invalid/").unwrap();
    let serializer = RestSerializer::new(
        &origin,
        "fixture-v4-6-5.rustodon.invalid",
        "https://media.example/assets",
        timestamp("2026-07-01 12:00:00"),
    );
    let video = MediaAttachmentProjection {
        id: PUBLIC_STATUS + 1_000,
        media_type: 2,
        processing: Some(2),
        remote_url: String::new(),
        file_content_type: Some("video/mp4".to_owned()),
        file_name: Some("movie.mp4".to_owned()),
        file_storage_schema_version: None,
        thumbnail_file_name: None,
        thumbnail_storage_schema_version: None,
        thumbnail_remote_url: None,
        shortcode: None,
        meta: None,
        description: None,
        blurhash: None,
        discarded: false,
    };
    let value = serde_json::to_value(serializer.media_attachment(&video)).unwrap();
    assert!(
        value["url"]
            .as_str()
            .unwrap()
            .starts_with("https://media.example/assets/media_attachments/files/")
    );
    assert!(
        value["preview_url"]
            .as_str()
            .unwrap()
            .ends_with("/small/movie.png")
    );
}

#[test]
fn remote_rich_representations_never_use_original_as_preview() {
    let mut media = MediaAttachmentProjection {
        id: 9,
        media_type: 2,
        processing: Some(0),
        remote_url: "https://remote.invalid/source.mov".into(),
        file_content_type: Some("video/quicktime".into()),
        file_name: None,
        file_storage_schema_version: Some(1),
        thumbnail_file_name: None,
        thumbnail_storage_schema_version: None,
        thumbnail_remote_url: None,
        shortcode: None,
        meta: None,
        description: None,
        blurhash: None,
        discarded: false,
    };
    for (kind, mime) in [(2, "video/quicktime"), (4, "audio/wav")] {
        media.media_type = kind;
        media.file_content_type = Some(mime.into());
        for processing in [Some(0), Some(1), Some(3), None] {
            media.processing = processing;
            assert!(serializer().media_attachment(&media).preview_url.is_none());
        }
    }
    media.processing = Some(2);
    for (kind, mime, name, preview) in [
        (2, "video/mp4", "normalized.mp4", Some("normalized.png")),
        (4, "audio/mpeg", "normalized.mp3", None),
        (0, "image/jpeg", "normalized.jpeg", Some("normalized.jpeg")),
    ] {
        media.media_type = kind;
        media.file_content_type = Some(mime.into());
        media.file_name = Some(name.into());
        let value = serializer().media_attachment(&media);
        let root = "https://fixture-v4-6-5.rustodon.invalid/system/cache/media_attachments/files/000/000/009";
        assert_eq!(value.url, Some(format!("{root}/original/{name}")));
        assert_eq!(
            value.preview_url,
            preview.map(|name| format!("{root}/small/{name}"))
        );
    }
    media.media_type = 4;
    media.file_content_type = Some("audio/mpeg".into());
    media.file_name = Some("normalized.mp3".into());
    media.thumbnail_file_name = Some("real.png".into());
    media.thumbnail_storage_schema_version = Some(1);
    assert_eq!(
        serializer().media_attachment(&media).preview_url.as_deref(),
        Some(
            "https://fixture-v4-6-5.rustodon.invalid/system/cache/media_attachments/thumbnails/000/000/009/original/real.png"
        )
    );
    media.thumbnail_file_name = Some("not-an-image.mp4".into());
    assert!(serializer().media_attachment(&media).preview_url.is_none());
}
