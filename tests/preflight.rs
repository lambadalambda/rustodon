use std::collections::{BTreeSet, HashMap};

use rustodon::config::Config;
use rustodon::preflight::{
    CatalogEntry, CatalogKind, Diagnostic, KeyFailure, PersistedIdentifier, PreflightReport,
    Severity, TimestampFunction, compare_catalog, compare_migration_versions, compare_sequences,
    configuration_diagnostics, expected_catalog, expected_migration_versions, expected_sequences,
    key_failure_diagnostic, media_root_diagnostics, validate_canonical_domains,
    validate_timestamp_function,
};
use serde_json::Value;

fn environment() -> HashMap<String, String> {
    HashMap::from([
        ("LOCAL_DOMAIN".to_owned(), "social.example".to_owned()),
        ("WEB_DOMAIN".to_owned(), "web.example".to_owned()),
        (
            "PAPERCLIP_ROOT_PATH".to_owned(),
            "/tmp/rustodon-media".to_owned(),
        ),
        ("SECRET_KEY_BASE".to_owned(), "secret-base-value".to_owned()),
        (
            "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY".to_owned(),
            "primary-secret-value".to_owned(),
        ),
        (
            "ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY".to_owned(),
            "deterministic-secret-value".to_owned(),
        ),
        (
            "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT".to_owned(),
            "derivation-salt-value".to_owned(),
        ),
    ])
}

fn diagnostic_codes(config: &Config) -> Vec<&'static str> {
    configuration_diagnostics(config)
        .iter()
        .map(Diagnostic::code)
        .collect()
}

#[test]
fn report_order_display_success_and_redaction_are_stable() {
    let report = PreflightReport::from_diagnostics([
        Diagnostic::warning("PF_Z_WARNING", "later warning", "warning hint"),
        Diagnostic::fatal("PF_B_FATAL", "second fatal", "second hint"),
        Diagnostic::fatal("PF_A_FATAL", "first fatal", "first hint"),
    ]);

    assert_eq!(
        report
            .diagnostics()
            .iter()
            .map(Diagnostic::code)
            .collect::<Vec<_>>(),
        ["PF_A_FATAL", "PF_B_FATAL", "PF_Z_WARNING"]
    );
    assert!(!report.is_success());
    let rendered = report.to_string();
    assert!(rendered.starts_with("preflight failed: 2 fatal, 1 warning"));
    assert!(rendered.contains("[FATAL PF_A_FATAL] first fatal"));
    assert!(rendered.contains("remediation: first hint"));
    for secret in [
        "secret-base-value",
        "primary-secret-value",
        "postgres://operator:password@database.example/mastodon",
        "-----BEGIN PRIVATE KEY-----",
    ] {
        assert!(!rendered.contains(secret));
    }

    let warnings =
        PreflightReport::from_diagnostics([Diagnostic::warning("PF_WARNING", "warning", "hint")]);
    assert!(warnings.is_success());
    assert_eq!(warnings.diagnostics()[0].severity(), Severity::Warning);
}

#[test]
fn every_unsupported_configuration_has_a_targeted_code() {
    for (variable, code) in [
        ("S3_ENABLED", "PF_CONFIG_OBJECT_STORAGE_S3"),
        ("SWIFT_ENABLED", "PF_CONFIG_OBJECT_STORAGE_SWIFT"),
        ("AZURE_ENABLED", "PF_CONFIG_OBJECT_STORAGE_AZURE"),
        ("LDAP_ENABLED", "PF_CONFIG_AUTH_LDAP"),
        ("PAM_ENABLED", "PF_CONFIG_AUTH_PAM"),
        ("CAS_ENABLED", "PF_CONFIG_AUTH_CAS"),
        ("SAML_ENABLED", "PF_CONFIG_AUTH_SAML"),
        ("OIDC_ENABLED", "PF_CONFIG_AUTH_OIDC"),
        ("OMNIAUTH_ONLY", "PF_CONFIG_OMNIAUTH_ONLY"),
        ("ONE_CLICK_SSO_LOGIN", "PF_CONFIG_ONE_CLICK_SSO"),
    ] {
        let mut values = environment();
        values.insert(variable.to_owned(), "true".to_owned());
        let config = Config::from_environment(&values).unwrap();
        assert!(diagnostic_codes(&config).contains(&code), "{variable}");
    }
}

#[test]
fn disabled_smtp_has_a_targeted_warning() {
    let config = Config::from_environment(&environment()).unwrap();
    let diagnostics = configuration_diagnostics(&config);
    assert!(diagnostics.iter().any(
        |item| item.code() == "PF_CONFIG_SMTP_DISABLED" && item.severity() == Severity::Warning
    ));
}

#[test]
fn migration_inventory_is_exact_and_detects_both_set_directions() {
    let expected = expected_migration_versions();
    assert_eq!(expected.len(), 588);
    assert!(compare_migration_versions(expected).is_empty());

    let mut actual = expected.clone();
    let removed = actual.pop_first().unwrap();
    actual.insert("99999999999999".to_owned());
    let diagnostics = compare_migration_versions(&actual);
    assert!(diagnostics.iter().any(|item| {
        item.code() == "PF_DB_MIGRATION_MISSING" && item.message().contains(&removed)
    }));
    assert!(diagnostics.iter().any(|item| {
        item.code() == "PF_DB_MIGRATION_UNEXPECTED" && item.message().contains("99999999999999")
    }));
}

#[test]
fn every_expected_physical_catalog_entry_is_required_and_compared() {
    let expected = expected_catalog();
    assert!(!expected.is_empty());
    assert_eq!(
        expected
            .iter()
            .filter(|entry| entry.kind == CatalogKind::Relation)
            .count(),
        78
    );
    assert!(
        expected
            .iter()
            .any(|entry| entry.kind == CatalogKind::Relation && entry.name == "web_settings")
    );
    assert_eq!(
        expected
            .iter()
            .filter(|entry| entry.kind == CatalogKind::View)
            .count(),
        1
    );
    assert!(compare_catalog(expected).is_empty());

    for index in 0..expected.len() {
        let mut missing = expected.to_vec();
        let removed = missing.remove(index);
        let diagnostics = compare_catalog(&missing);
        assert!(
            diagnostics
                .iter()
                .any(|item| item.code().ends_with("_MISSING")),
            "missing mutation was not detected for {:?} {}",
            removed.kind,
            removed.name
        );

        let mut changed = expected.to_vec();
        mutate_definition(&mut changed[index].definition);
        let diagnostics = compare_catalog(&changed);
        assert!(
            !diagnostics.is_empty(),
            "definition mutation was not detected for {:?} {}",
            removed.kind,
            removed.name
        );
    }

    let mut unexpected = expected.to_vec();
    unexpected.push(CatalogEntry {
        kind: CatalogKind::Column,
        name: "accounts.rustodon_unexpected".to_owned(),
        definition: r#"{"schema":"public","relation":"accounts","position":999,"name":"rustodon_unexpected","type":"text","not_null":false,"default":null,"identity":"","generated":"","collation":"default"}"#.to_owned(),
    });
    assert!(
        compare_catalog(&unexpected)
            .iter()
            .any(|item| item.code() == "PF_DB_COLUMN_UNEXPECTED")
    );

    let mut unrelated = expected.to_vec();
    unrelated.push(CatalogEntry {
        kind: CatalogKind::Relation,
        name: "unrelated_extension_table".to_owned(),
        definition: r#"{"schema":"public","name":"unrelated_extension_table","kind":"r","persistence":"p","partitioned":false,"row_security":false}"#.to_owned(),
    });
    assert!(compare_catalog(&unrelated).is_empty());
}

fn mutate_definition(definition: &mut String) {
    let mut value: Value = serde_json::from_str(definition).unwrap();
    let field = value.as_object_mut().unwrap().values_mut().next().unwrap();
    *field = match field {
        Value::Null => Value::String("changed".to_owned()),
        Value::Bool(value) => Value::Bool(!*value),
        Value::Number(_) => Value::Number(999_999.into()),
        Value::String(value) => Value::String(format!("{value}_changed")),
        Value::Array(_) | Value::Object(_) => Value::Null,
    };
    *definition = serde_json::to_string(&value).unwrap();
}

fn mutate_catalog_field(kind: CatalogKind, name: &str, field: &str, value: Value) -> Diagnostic {
    let mut actual = expected_catalog().to_vec();
    let entry = actual
        .iter_mut()
        .find(|entry| entry.kind == kind && entry.name == name)
        .unwrap();
    let mut definition: Value = serde_json::from_str(&entry.definition).unwrap();
    definition[field] = value;
    entry.definition = serde_json::to_string(&definition).unwrap();
    compare_catalog(&actual).into_iter().next().unwrap()
}

#[test]
fn catalog_mismatches_have_representative_targeted_diagnostics() {
    assert_eq!(
        mutate_catalog_field(
            CatalogKind::Column,
            "accounts.id",
            "type",
            Value::String("integer".to_owned()),
        )
        .code(),
        "PF_DB_COLUMN_TYPE_CHANGED"
    );
    assert_eq!(
        mutate_catalog_field(
            CatalogKind::Column,
            "accounts.id",
            "not_null",
            Value::Bool(false),
        )
        .code(),
        "PF_DB_COLUMN_NULLABILITY_CHANGED"
    );
    assert_eq!(
        mutate_catalog_field(CatalogKind::Column, "accounts.id", "default", Value::Null,).code(),
        "PF_DB_COLUMN_DEFAULT_CHANGED"
    );
    assert_eq!(
        mutate_catalog_field(
            CatalogKind::Constraint,
            "accounts.accounts_pkey",
            "validated",
            Value::Bool(false),
        )
        .code(),
        "PF_DB_CONSTRAINT_CHANGED"
    );
    assert_eq!(
        mutate_catalog_field(
            CatalogKind::Index,
            "accounts.accounts_pkey",
            "valid",
            Value::Bool(false),
        )
        .code(),
        "PF_DB_INDEX_CHANGED"
    );
}

#[test]
fn all_seven_sequences_are_exact_and_each_mutation_is_detected() {
    let expected = expected_sequences();
    assert_eq!(expected.len(), 7);
    assert!(compare_sequences(expected).is_empty());

    for index in 0..expected.len() {
        let mut missing = expected.to_vec();
        missing.remove(index);
        assert!(
            compare_sequences(&missing)
                .iter()
                .any(|item| item.code() == "PF_DB_SEQUENCE_MISSING")
        );

        let mut changed = expected.to_vec();
        mutate_definition(&mut changed[index].definition);
        assert!(
            compare_sequences(&changed)
                .iter()
                .any(|item| item.code() == "PF_DB_SEQUENCE_CHANGED")
        );
    }
}

fn canonical_timestamp_function() -> TimestampFunction {
    TimestampFunction {
        identity_arguments: "table_name text".to_owned(),
        result: "bigint".to_owned(),
        language: "plpgsql".to_owned(),
        kind: "f".to_owned(),
        volatility: "v".to_owned(),
        parallel: "u".to_owned(),
        security_definer: false,
        leakproof: false,
        strict: false,
        config: None,
        body: r"
          DECLARE
            time_part bigint;
            sequence_base bigint;
            tail bigint;
          BEGIN
            time_part := (((date_part('epoch', now()) * 1000))::bigint << 16);
            sequence_base := (
              'x' || substr(
                md5(table_name || 'different-production-salt' || time_part::text), 1, 4
              )
            )::bit(16)::bigint;
            tail := ((sequence_base + nextval(table_name || '_id_seq')) & 65535);
            RETURN time_part | tail;
          END
        "
        .to_owned(),
    }
}

#[test]
fn timestamp_function_allows_only_salt_and_formatting_variation() {
    let mut function = canonical_timestamp_function();
    assert!(validate_timestamp_function(&function).is_empty());

    function.body = function.body.replace(
        "time_part := (",
        "-- production schema comments are not behavior\n time_part := (",
    );
    assert!(validate_timestamp_function(&function).is_empty());

    function.result = "integer".to_owned();
    assert_eq!(
        validate_timestamp_function(&function)[0].code(),
        "PF_DB_TIMESTAMP_ID_SHAPE"
    );
    function = canonical_timestamp_function();
    function.config = Some(vec!["search_path=public".to_owned()]);
    assert_eq!(
        validate_timestamp_function(&function)[0].code(),
        "PF_DB_TIMESTAMP_ID_SHAPE"
    );
    function = canonical_timestamp_function();
    function.body = function.body.replace("'epoch'", "'epo ch'");
    assert_eq!(
        validate_timestamp_function(&function)[0].code(),
        "PF_DB_TIMESTAMP_ID_BODY"
    );
    function = canonical_timestamp_function();
    function.body = function.body.replace("65535", "65534");
    assert_eq!(
        validate_timestamp_function(&function)[0].code(),
        "PF_DB_TIMESTAMP_ID_BODY"
    );
}

fn identifier(table: &str, row_id: i64, column: &str, value: &str) -> PersistedIdentifier {
    PersistedIdentifier {
        table: table.to_owned(),
        row_id,
        column: column.to_owned(),
        value: value.to_owned(),
    }
}

#[test]
fn canonical_domains_distinguish_web_local_and_alternate_authorities() {
    let identifiers = [
        identifier("accounts", 1, "url", "https://web.example/@alice"),
        identifier(
            "statuses",
            2,
            "uri",
            "http://web.example/users/alice/statuses/2",
        ),
        identifier(
            "conversations",
            3,
            "uri",
            "tag:social.example,2026-08-11:objectId=3",
        ),
    ];
    assert!(validate_canonical_domains("web.example", "social.example", &identifiers).is_empty());

    let alternate = [identifier(
        "accounts",
        1,
        "url",
        "https://alternate.example/@alice",
    )];
    let diagnostics = validate_canonical_domains("web.example", "social.example", &alternate);
    assert!(
        diagnostics
            .iter()
            .any(|item| item.code() == "PF_DB_WEB_DOMAIN_MISMATCH")
    );
    assert!(
        diagnostics
            .iter()
            .any(|item| item.code() == "PF_DB_LOCAL_DOMAIN_UNVERIFIED")
    );
}

#[test]
fn canonical_domain_validator_classifies_unsupported_persisted_data_safely() {
    let diagnostics = validate_canonical_domains(
        "web.example",
        "social.example",
        &[
            identifier("accounts", 41, "url", "acct:alice@web.example"),
            identifier("statuses", 42, "uri", "https://%zz.invalid/status/42"),
            identifier("keypairs", 43, "uri", "tag:other.example,2026-08-11:key=43"),
        ],
    );
    let codes = diagnostics
        .iter()
        .map(Diagnostic::code)
        .collect::<BTreeSet<_>>();
    assert!(codes.contains("PF_DB_CANONICAL_IDENTIFIER_INVALID"));
    assert!(codes.contains("PF_DB_LOCAL_DOMAIN_MISMATCH"));
    let rendered = PreflightReport::from_diagnostics(diagnostics).to_string();
    assert!(!rendered.contains("acct:alice"));
    assert!(!rendered.contains("%zz"));
}

#[test]
fn key_failures_map_to_stable_secret_free_diagnostics() {
    for (failure, code) in [
        (KeyFailure::MissingPrivate, "PF_DB_KEY_MISSING_PRIVATE"),
        (KeyFailure::MissingPublic, "PF_DB_KEY_MISSING_PUBLIC"),
        (KeyFailure::CorruptPrivate, "PF_DB_KEY_CORRUPT_PRIVATE"),
        (KeyFailure::CorruptPublic, "PF_DB_KEY_CORRUPT_PUBLIC"),
        (KeyFailure::Mismatch, "PF_DB_KEY_MISMATCH"),
        (KeyFailure::Undecryptable, "PF_DB_KEY_UNDECRYPTABLE"),
        (KeyFailure::Signing, "PF_DB_KEY_SIGNING_FAILED"),
        (KeyFailure::Verification, "PF_DB_KEY_VERIFICATION_FAILED"),
    ] {
        let diagnostic = key_failure_diagnostic("keypairs", -99, failure);
        assert_eq!(diagnostic.code(), code);
        assert!(diagnostic.message().contains("keypairs row -99"));
        assert!(diagnostic.hint().contains("backup"));
        assert!(!diagnostic.to_string().contains("PRIVATE KEY"));
    }
}

#[cfg(unix)]
#[test]
fn media_root_requires_canonical_searchable_directory() {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = repository
        .join("target")
        .join(format!("preflight-media-{}", std::process::id()));
    let link = repository
        .join("target")
        .join(format!("preflight-media-link-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();

    let mut values = environment();
    values.insert(
        "PAPERCLIP_ROOT_PATH".to_owned(),
        root.to_string_lossy().into_owned(),
    );
    let config = Config::from_environment(&values).unwrap();
    assert!(media_root_diagnostics(&config).is_empty());

    fs::set_permissions(&root, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        media_root_diagnostics(&config)
            .iter()
            .any(|diagnostic| diagnostic.code() == "PF_MEDIA_ROOT_UNWRITABLE")
    );
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

    symlink(&root, &link).unwrap();
    values.insert(
        "PAPERCLIP_ROOT_PATH".to_owned(),
        link.to_string_lossy().into_owned(),
    );
    let config = Config::from_environment(&values).unwrap();
    assert!(
        media_root_diagnostics(&config)
            .iter()
            .any(|diagnostic| diagnostic.code() == "PF_MEDIA_ROOT_NOT_CANONICAL")
    );

    fs::remove_file(link).unwrap();
    fs::remove_dir(root).unwrap();
}

#[tokio::test]
#[ignore = "requires the native process environment and restored canonical fixture"]
async fn canonical_fixture_preflight() {
    let config = Config::from_process_environment().unwrap();
    let report = rustodon::preflight::run(&config).await;
    assert!(report.is_success(), "{report}");
}
