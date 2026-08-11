use rustodon::mastodon::{
    AccountIdScheme, AccountKind, NotificationType, RawJsonText, RawYamlText, SecretText,
    StatusVisibility,
};

#[test]
fn open_values_preserve_known_and_unknown_database_values() {
    assert_eq!(AccountIdScheme::from(0), AccountIdScheme::Username);
    assert_eq!(AccountIdScheme::from(1), AccountIdScheme::Numeric);
    assert_eq!(AccountIdScheme::from(99), AccountIdScheme::Unknown(99));
    assert_eq!(AccountIdScheme::Unknown(99).raw(), 99);

    assert_eq!(StatusVisibility::from(0).raw(), 0);
    assert_eq!(StatusVisibility::from(4), StatusVisibility::Limited);
    assert_eq!(StatusVisibility::from(99), StatusVisibility::Unknown(99));
    assert_eq!(StatusVisibility::Unknown(99).raw(), 99);

    assert_eq!(NotificationType::from("mention"), NotificationType::Mention);
    assert_eq!(
        NotificationType::from("future_event"),
        NotificationType::Unknown("future_event".to_owned())
    );
    assert_eq!(
        NotificationType::Unknown("future_event".into()).raw(),
        "future_event"
    );
}

#[test]
fn account_kind_distinguishes_remote_service_and_login_accounts() {
    assert_eq!(
        AccountKind::classify(Some("remote.invalid"), false, false),
        AccountKind::Remote
    );
    assert_eq!(
        AccountKind::classify(None, false, false),
        AccountKind::LocalService
    );
    assert_eq!(
        AccountKind::classify(None, true, false),
        AccountKind::LocalUnavailable
    );
    assert_eq!(
        AccountKind::classify(None, true, true),
        AccountKind::LocalLogin
    );
}

#[test]
fn signed_ids_and_decimal_strings_are_lossless() {
    assert_eq!(
        116_846_257_766_400_501_i64.to_string(),
        "116846257766400501"
    );
    for id in [-99_i64, 0, 116_846_257_766_400_501] {
        assert_eq!(id.to_string().parse::<i64>(), Ok(id));
    }
}

#[test]
fn raw_settings_parse_without_rewriting_source_bytes() {
    let json_source = r#"{"default_privacy":"private","nested":{"number":9007199254740993}}"#;
    let json = RawJsonText::new(json_source.to_owned());
    assert_eq!(json.raw(), json_source);
    assert_eq!(json.parse().unwrap()["default_privacy"], "private");
    assert_eq!(
        RawJsonText::new(r#"{"number":123456789012345678901234567890}"#.to_owned())
            .parse()
            .unwrap()["number"]
            .to_string(),
        "123456789012345678901234567890"
    );

    let scalar_source = "--- true\n";
    let scalar = RawYamlText::new(scalar_source.to_owned());
    assert!(scalar.parse().unwrap()[0].as_bool().unwrap());
    assert_eq!(scalar.raw(), scalar_source);

    let tagged_source = "--- !ruby/hash:ActiveSupport::HashWithIndifferentAccess\nfixture: value\n";
    let tagged = RawYamlText::new(tagged_source.to_owned());
    let parsed = tagged.parse().unwrap();
    assert_eq!(
        parsed[0].get_tag().unwrap().to_string(),
        "!ruby/hash:ActiveSupport::HashWithIndifferentAccess"
    );
    assert_eq!(
        parsed[0].get_tagged_node().unwrap()["fixture"].as_str(),
        Some("value")
    );
    assert_eq!(tagged.raw().as_bytes(), tagged_source.as_bytes());
}

#[test]
fn secret_debug_output_never_contains_the_value() {
    let secret = SecretText::new("fixture-private-value".to_owned());
    assert!(secret.is_present());
    let rendered = format!("{secret:?}");
    assert!(!rendered.contains("fixture-private-value"));
    assert!(rendered.contains("REDACTED"));
}
