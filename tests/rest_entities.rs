use chrono::NaiveDateTime;
use rustodon::mastodon::rest::{
    ApiDate, ApiDateTime, ApiSecondDateTime, DecimalId, RestApplication, RestMarker,
    RestMediaAttachment, RestRelationship,
};
use serde_json::json;

fn timestamp(value: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f").expect("fixed timestamp")
}

#[test]
fn ids_are_always_signed_decimal_json_strings() {
    assert_eq!(serde_json::to_value(DecimalId::new(-99)).unwrap(), "-99");
    assert_eq!(
        serde_json::to_value(DecimalId::new(116_844_842_188_805_001)).unwrap(),
        "116844842188805001"
    );
}

#[test]
fn api_dates_match_mastodons_utc_millisecond_contract() {
    assert_eq!(
        serde_json::to_value(ApiDateTime::new(timestamp("2026-07-01 13:00:00"))).unwrap(),
        "2026-07-01T13:00:00.000Z"
    );
    assert_eq!(
        serde_json::to_value(ApiDateTime::new(timestamp("2026-07-01 13:00:00.987654"))).unwrap(),
        "2026-07-01T13:00:00.987Z"
    );
    assert_eq!(
        serde_json::to_value(ApiDate::new(timestamp("2026-07-01 23:59:59"))).unwrap(),
        "2026-07-01"
    );
}

#[test]
fn relationship_preserves_nullable_languages_and_expiry() {
    let relationship = RestRelationship {
        id: DecimalId::new(116_844_606_259_202_001),
        following: true,
        showing_reblogs: true,
        notifying: true,
        languages: Some(Vec::new()),
        followed_by: true,
        blocking: false,
        blocked_by: false,
        muting: true,
        muting_notifications: false,
        muting_expires_at: Some(ApiSecondDateTime::new(timestamp("2026-08-01 00:00:00"))),
        requested: false,
        requested_by: false,
        domain_blocking: false,
        endorsed: false,
        note: String::new(),
    };

    let value = serde_json::to_value(relationship).unwrap();
    assert_eq!(value["languages"], json!([]));
    assert_eq!(value["muting_expires_at"], "2026-08-01T00:00:00Z");
    assert_eq!(value.as_object().unwrap().len(), 16);
}

#[test]
fn marker_and_status_application_keep_exact_scalar_shapes() {
    let marker = RestMarker {
        last_read_id: DecimalId::new(116_844_842_188_805_001),
        version: 7,
        updated_at: ApiDateTime::new(timestamp("2026-07-01 13:00:00")),
    };
    assert_eq!(
        serde_json::to_value(marker).unwrap(),
        json!({
            "last_read_id": "116844842188805001",
            "version": 7,
            "updated_at": "2026-07-01T13:00:00.000Z"
        })
    );

    assert_eq!(
        serde_json::to_value(RestApplication {
            name: "Rustodon fixture client".to_owned(),
            website: None,
        })
        .unwrap(),
        json!({"name": "Rustodon fixture client", "website": null})
    );
}

#[test]
fn media_attachment_keeps_required_nullable_keys() {
    let media = RestMediaAttachment {
        id: DecimalId::new(-101),
        media_type: "image".to_owned(),
        url: Some("https://fixture.invalid/media_proxy/-101/original".to_owned()),
        preview_url: None,
        remote_url: Some("https://media.fixture.invalid/first.jpg".to_owned()),
        preview_remote_url: None,
        text_url: None,
        meta: None,
        description: None,
        blurhash: None,
    };
    let value = serde_json::to_value(media).unwrap();
    assert_eq!(value["id"], "-101");
    assert_eq!(value["type"], "image");
    for key in [
        "url",
        "preview_url",
        "remote_url",
        "preview_remote_url",
        "text_url",
        "meta",
        "description",
        "blurhash",
    ] {
        assert!(value.get(key).is_some(), "missing key {key}");
    }
}
