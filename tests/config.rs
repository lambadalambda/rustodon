use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use rustodon::config::{
    Config, ConfigWarning, ExternalAuthProvider, ObjectStorageProvider, PaperclipRootUrl,
    PostgresConnection, PostgresSslMode, PostgresUrlSource, RedisEndpoint, SmtpAuthentication,
    SmtpConfig, SmtpDeliveryMethod, SmtpTransport, SmtpVerifyMode, StartTlsMode,
};
use rustodon::jobs::Lane;

fn required_environment() -> HashMap<String, String> {
    HashMap::from([
        ("LOCAL_DOMAIN".into(), "Example.COM".into()),
        ("PAPERCLIP_ROOT_PATH".into(), "/srv/mastodon/system".into()),
        ("SECRET_KEY_BASE".into(), "session-secret".into()),
        (
            "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY".into(),
            "primary-secret".into(),
        ),
        (
            "ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY".into(),
            "deterministic-secret".into(),
        ),
        (
            "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT".into(),
            "derivation-secret".into(),
        ),
    ])
}

fn error_for(name: &str, value: &str) -> rustodon::config::ConfigError {
    let mut environment = required_environment();
    environment.insert(name.into(), value.into());
    Config::from_environment(&environment).expect_err("configuration should be rejected")
}

#[test]
fn mastodon_compatible_defaults_are_typed() {
    let config = Config::from_environment(&required_environment()).unwrap();

    assert_eq!(config.domains.local_domain, "example.com");
    assert_eq!(config.domains.web_domain, "example.com");
    assert!(config.domains.alternate_domains.is_empty());
    assert_eq!(
        config.domains.canonical_origin.as_str(),
        "https://example.com/"
    );

    assert_eq!(config.database.pool_size, 5);
    assert_eq!(config.database.ssl_mode, PostgresSslMode::Prefer);
    match &config.database.connection {
        PostgresConnection::Tcp {
            host,
            port,
            database,
            username,
            password,
        } => {
            assert_eq!(host, "localhost");
            assert_eq!(*port, 5432);
            assert_eq!(database, "mastodon_production");
            assert_eq!(username, "mastodon");
            assert!(password.is_empty());
        }
        other => panic!("expected default TCP database, got {other:?}"),
    }

    assert_eq!(
        config.paperclip.root_path,
        PathBuf::from("/srv/mastodon/system")
    );
    assert_eq!(
        config.paperclip.root_url,
        PaperclipRootUrl::RootRelative("/system".into())
    );
    assert!(config.trusted_proxies.is_empty());
    assert_eq!(
        config.smtp,
        SmtpConfig::Disabled {
            warning: ConfigWarning::SmtpDisabled,
        }
    );
    assert!(config.unsupported.object_storage.is_empty());
    assert!(config.unsupported.external_auth.is_empty());
    assert!(!config.unsupported.omniauth_only);
    assert!(!config.unsupported.one_click_sso_login);
    assert!(matches!(
        config.sidekiq_redis,
        Some(RedisEndpoint::Tcp {
            ref host,
            port: 6379,
            database: 0,
            username: None,
            password: None,
        }) if host == "localhost"
    ));
    assert_eq!(
        config.worker.lanes,
        [Lane::Maintenance].into_iter().collect::<BTreeSet<_>>()
    );
    assert_eq!(config.worker.concurrency, 5);
    assert_eq!(config.worker.remote_http_concurrency, 4);
    assert_eq!(config.worker.media_concurrency, 2);
    assert_eq!(config.worker.lease_seconds, 60);
    assert_eq!(config.worker.poll_milliseconds, 250);
    assert_eq!(config.worker.heartbeat_seconds, 10);
    assert_eq!(config.worker.shutdown_seconds, 15);
    assert_eq!(config.web.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_eq!(config.web.port, 3000);
}

#[test]
fn web_listener_is_typed_loopback_by_default_and_rejects_ambiguous_values() {
    let mut environment = required_environment();
    environment.insert("BIND".into(), "::1".into());
    environment.insert("PORT".into(), "8443".into());
    let config = Config::from_environment(&environment).unwrap();
    assert_eq!(config.web.bind, IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
    assert_eq!(config.web.port, 8443);

    for (name, value) in [
        ("BIND", ""),
        ("BIND", "localhost"),
        ("BIND", "0.0.0.0/0"),
        ("PORT", ""),
        ("PORT", "0"),
        ("PORT", "65536"),
        ("PORT", "not-a-port"),
    ] {
        assert!(
            error_for(name, value).to_string().contains(name),
            "{name} error did not name its variable"
        );
    }
}

#[test]
fn worker_settings_are_typed_bounded_and_use_sidekiq_concurrency_as_a_fallback() {
    let mut environment = required_environment();
    environment.extend([
        ("SIDEKIQ_CONCURRENCY".into(), "7".into()),
        ("WORKER_LANES".into(), "push,ingress,push".into()),
        ("WORKER_REMOTE_HTTP_CONCURRENCY".into(), "3".into()),
        ("WORKER_MEDIA_CONCURRENCY".into(), "1".into()),
        ("WORKER_LEASE_SECONDS".into(), "90".into()),
        ("WORKER_POLL_MILLISECONDS".into(), "100".into()),
        ("WORKER_HEARTBEAT_SECONDS".into(), "15".into()),
        ("WORKER_SHUTDOWN_SECONDS".into(), "20".into()),
    ]);
    let config = Config::from_environment(&environment).unwrap();
    assert_eq!(config.worker.concurrency, 7);
    assert_eq!(
        config.worker.lanes,
        [Lane::Ingress, Lane::Push].into_iter().collect()
    );
    assert_eq!(config.worker.remote_http_concurrency, 3);
    assert_eq!(config.worker.media_concurrency, 1);
    assert_eq!(config.worker.lease_seconds, 90);
    assert_eq!(config.worker.poll_milliseconds, 100);
    assert_eq!(config.worker.heartbeat_seconds, 15);
    assert_eq!(config.worker.shutdown_seconds, 20);

    environment.insert("WORKER_CONCURRENCY".into(), "9".into());
    assert_eq!(
        Config::from_environment(&environment)
            .unwrap()
            .worker
            .concurrency,
        9
    );

    for (name, value) in [
        ("WORKER_LANES", "push,unknown"),
        ("WORKER_LANES", ""),
        ("WORKER_CONCURRENCY", "0"),
        ("WORKER_REMOTE_HTTP_CONCURRENCY", "0"),
        ("WORKER_MEDIA_CONCURRENCY", "0"),
        ("WORKER_LEASE_SECONDS", "4"),
        ("WORKER_POLL_MILLISECONDS", "9"),
        ("WORKER_HEARTBEAT_SECONDS", "0"),
        ("WORKER_SHUTDOWN_SECONDS", "0"),
    ] {
        assert!(
            error_for(name, value).to_string().contains(name),
            "{name} error did not name its variable"
        );
    }
}

#[test]
fn primary_database_url_wins_and_pool_uses_its_own_precedence() {
    let mut environment = required_environment();
    environment.extend([
        ("DB_HOST".into(), "ignored.internal".into()),
        (
            "DATABASE_URL".into(),
            "postgres://database.example/ignored".into(),
        ),
        (
            "PRIMARY_DATABASE_URL".into(),
            "postgresql://primary.example/mastodon?sslmode=verify-full".into(),
        ),
        ("MAX_THREADS".into(), "17".into()),
        ("DB_POOL".into(), "23".into()),
    ]);

    let config = Config::from_environment(&environment).unwrap();
    assert_eq!(config.database.pool_size, 23);
    assert_eq!(config.database.ssl_mode, PostgresSslMode::VerifyFull);
    match &config.database.connection {
        PostgresConnection::Url { source, url } => {
            assert_eq!(*source, PostgresUrlSource::PrimaryDatabaseUrl);
            assert_eq!(
                url.expose_secret(),
                "postgresql://primary.example/mastodon?sslmode=verify-full"
            );
        }
        other => panic!("expected URL database, got {other:?}"),
    }

    environment.remove("PRIMARY_DATABASE_URL");
    environment.remove("DB_POOL");
    let config = Config::from_environment(&environment).unwrap();
    assert_eq!(config.database.pool_size, 17);
    assert!(matches!(
        config.database.connection,
        PostgresConnection::Url {
            source: PostgresUrlSource::DatabaseUrl,
            ..
        }
    ));
}

#[test]
fn discrete_database_supports_unix_sockets_and_ssl_modes() {
    let mut environment = required_environment();
    environment.extend([
        ("DB_HOST".into(), "/run/postgresql".into()),
        ("DB_PORT".into(), "6432".into()),
        ("DB_NAME".into(), "production".into()),
        ("DB_USER".into(), "rustodon".into()),
        ("DB_PASS".into(), "database-secret".into()),
        ("DB_SSLMODE".into(), "verify-ca".into()),
    ]);

    let config = Config::from_environment(&environment).unwrap();
    assert_eq!(config.database.ssl_mode, PostgresSslMode::VerifyCa);
    match config.database.connection {
        PostgresConnection::UnixSocket {
            directory,
            port,
            database,
            username,
            password,
        } => {
            assert_eq!(directory, PathBuf::from("/run/postgresql"));
            assert_eq!(port, 6432);
            assert_eq!(database, "production");
            assert_eq!(username, "rustodon");
            assert_eq!(password.expose_secret(), "database-secret");
        }
        other => panic!("expected Unix-socket database, got {other:?}"),
    }
}

#[test]
fn domains_are_idna_normalized_and_alternates_are_deduplicated() {
    let mut environment = required_environment();
    environment.insert("LOCAL_DOMAIN".into(), "BÜCHER.Example".into());
    environment.insert("WEB_DOMAIN".into(), "WWW.BÜCHER.Example:8443".into());
    environment.insert(
        "ALTERNATE_DOMAINS".into(),
        " Alias.Example, bücher.example ,ALIAS.example ".into(),
    );

    let config = Config::from_environment(&environment).unwrap();
    assert_eq!(config.domains.local_domain, "xn--bcher-kva.example");
    assert_eq!(config.domains.web_domain, "www.xn--bcher-kva.example:8443");
    assert_eq!(
        config.domains.alternate_domains,
        ["alias.example", "xn--bcher-kva.example"]
    );
    assert_eq!(
        config.domains.canonical_origin.as_str(),
        "https://www.xn--bcher-kva.example:8443/"
    );
}

#[test]
fn malformed_domains_and_alternate_entries_are_rejected() {
    for (name, value) in [
        ("LOCAL_DOMAIN", ""),
        ("LOCAL_DOMAIN", "https://example.com"),
        ("LOCAL_DOMAIN", "person@example.com"),
        ("LOCAL_DOMAIN", "example.com/path"),
        ("LOCAL_DOMAIN", "example .com"),
        ("LOCAL_DOMAIN", "example.com:0"),
        ("WEB_DOMAIN", "http://web.example"),
        ("ALTERNATE_DOMAINS", "one.example,,two.example"),
        ("ALTERNATE_DOMAINS", "one.example,   "),
    ] {
        let error = error_for(name, value);
        assert!(error.variables().contains(&name), "got {error}");
    }

    let mut environment = required_environment();
    environment.insert("ALTERNATE_DOMAINS".into(), String::new());
    assert!(
        Config::from_environment(&environment)
            .unwrap()
            .domains
            .alternate_domains
            .is_empty()
    );
}

#[test]
fn paperclip_paths_are_explicit_and_clean() {
    for (name, value) in [
        ("PAPERCLIP_ROOT_PATH", "relative/system"),
        ("PAPERCLIP_ROOT_URL", "system"),
        ("PAPERCLIP_ROOT_URL", "/"),
        ("PAPERCLIP_ROOT_URL", "/system/../private"),
        ("PAPERCLIP_ROOT_URL", "/system//accounts"),
        ("PAPERCLIP_ROOT_URL", "http://media.example/system"),
    ] {
        let error = error_for(name, value);
        assert!(error.variables().contains(&name), "got {error}");
    }

    let mut environment = required_environment();
    environment.insert(
        "PAPERCLIP_ROOT_URL".into(),
        "https://Media.Example/assets".into(),
    );
    let config = Config::from_environment(&environment).unwrap();
    match config.paperclip.root_url {
        PaperclipRootUrl::Absolute(url) => {
            assert_eq!(url.as_str(), "https://media.example/assets");
        }
        other @ PaperclipRootUrl::RootRelative(_) => {
            panic!("expected absolute media URL, got {other:?}");
        }
    }
}

#[test]
fn trusted_proxies_are_normalized_deduplicated_and_bounded() {
    let mut environment = required_environment();
    environment.insert(
        "TRUSTED_PROXY_IP".into(),
        "10.2.3.4/8, 192.0.2.10\n2001:db8::1 192.0.2.10/32".into(),
    );
    let config = Config::from_environment(&environment).unwrap();
    assert_eq!(
        config
            .trusted_proxies
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["10.0.0.0/8", "192.0.2.10/32", "2001:db8::1/128"]
    );

    for value in ["", "not-an-address", "0.0.0.0/0", "::/0"] {
        let error = error_for("TRUSTED_PROXY_IP", value);
        assert!(error.variables().contains(&"TRUSTED_PROXY_IP"));
    }
}

#[test]
fn smtp_defaults_and_mailboxes_are_typed_when_enabled() {
    let mut environment = required_environment();
    environment.extend([
        ("SMTP_SERVER".into(), "Mail.Example".into()),
        ("SMTP_PORT".into(), "587".into()),
        ("SMTP_LOGIN".into(), "smtp-user".into()),
        ("SMTP_PASSWORD".into(), "smtp-secret".into()),
        (
            "SMTP_FROM_ADDRESS".into(),
            "Rustodon <notifications@example.com>".into(),
        ),
        ("SMTP_REPLY_TO".into(), "help@example.com".into()),
        ("SMTP_RETURN_PATH".into(), "bounces@example.com".into()),
        ("SMTP_DOMAIN".into(), "Example.COM".into()),
        ("SMTP_AUTH_METHOD".into(), "login".into()),
        ("SMTP_ENABLE_STARTTLS".into(), "always".into()),
        ("SMTP_OPENSSL_VERIFY_MODE".into(), "peer".into()),
        ("SMTP_CA_FILE".into(), "/custom/ca.pem".into()),
    ]);

    let config = Config::from_environment(&environment).unwrap();
    let SmtpConfig::Enabled(smtp) = config.smtp else {
        panic!("SMTP should be enabled");
    };
    assert_eq!(smtp.delivery_method, SmtpDeliveryMethod::Smtp);
    assert_eq!(smtp.server, "mail.example");
    assert_eq!(smtp.port, 587);
    assert_eq!(smtp.login.unwrap().expose_secret(), "smtp-user");
    assert_eq!(smtp.password.unwrap().expose_secret(), "smtp-secret");
    assert_eq!(smtp.from.display_name.as_deref(), Some("Rustodon"));
    assert_eq!(smtp.from.address, "notifications@example.com");
    assert_eq!(smtp.reply_to.unwrap().address, "help@example.com");
    assert_eq!(smtp.return_path.unwrap(), "bounces@example.com");
    assert_eq!(smtp.domain, "example.com");
    assert_eq!(smtp.authentication, SmtpAuthentication::Login);
    assert_eq!(
        smtp.transport,
        SmtpTransport::StartTls(StartTlsMode::Required)
    );
    assert_eq!(smtp.verify_mode, Some(SmtpVerifyMode::Peer));
    assert_eq!(smtp.ca_file, PathBuf::from("/custom/ca.pem"));
}

#[test]
fn smtp_transport_modes_and_contradictions_are_strict() {
    for (variables, expected) in [
        (
            [("SMTP_TLS", "true"), ("SMTP_ENABLE_STARTTLS_AUTO", "false")],
            SmtpTransport::Tls,
        ),
        (
            [("SMTP_SSL", "true"), ("SMTP_ENABLE_STARTTLS_AUTO", "false")],
            SmtpTransport::Ssl,
        ),
        (
            [("SMTP_ENABLE_STARTTLS", "never"), ("SMTP_TLS", "false")],
            SmtpTransport::Plain,
        ),
        (
            [("SMTP_ENABLE_STARTTLS", "auto"), ("SMTP_SSL", "false")],
            SmtpTransport::StartTls(StartTlsMode::Opportunistic),
        ),
    ] {
        let mut environment = required_environment();
        environment.insert("SMTP_SERVER".into(), "mail.example".into());
        for (name, value) in variables {
            environment.insert(name.into(), value.into());
        }
        let config = Config::from_environment(&environment).unwrap();
        let SmtpConfig::Enabled(settings) = config.smtp else {
            panic!("SMTP should be enabled");
        };
        assert_eq!(settings.transport, expected);
    }

    for variables in [
        vec![("SMTP_TLS", "true"), ("SMTP_SSL", "true")],
        vec![("SMTP_TLS", "true"), ("SMTP_ENABLE_STARTTLS", "always")],
        vec![("SMTP_SSL", "true"), ("SMTP_ENABLE_STARTTLS_AUTO", "true")],
        vec![("SMTP_AUTH_METHOD", "none"), ("SMTP_LOGIN", "user")],
    ] {
        let mut environment = required_environment();
        environment.insert("SMTP_SERVER".into(), "mail.example".into());
        for (name, value) in variables {
            environment.insert(name.into(), value.into());
        }
        assert!(Config::from_environment(&environment).is_err());
    }

    for (name, value) in [
        ("SMTP_PORT", "0"),
        ("SMTP_FROM_ADDRESS", "not-a-mailbox"),
        ("SMTP_DELIVERY_METHOD", "sendmail"),
        ("SMTP_AUTH_METHOD", "oauth"),
        ("SMTP_ENABLE_STARTTLS", "yes"),
        ("SMTP_TLS", "TRUE"),
        ("SMTP_OPENSSL_VERIFY_MODE", "sometimes"),
        ("SMTP_CA_FILE", "relative.pem"),
    ] {
        let mut environment = required_environment();
        environment.insert("SMTP_SERVER".into(), "mail.example".into());
        environment.insert(name.into(), value.into());
        let error = Config::from_environment(&environment).unwrap_err();
        assert!(error.variables().contains(&name), "got {error}");
    }
}

#[test]
fn unsupported_feature_flags_are_exact_and_exposed_for_preflight() {
    let mut environment = required_environment();
    for name in [
        "S3_ENABLED",
        "AZURE_ENABLED",
        "LDAP_ENABLED",
        "OIDC_ENABLED",
        "OMNIAUTH_ONLY",
        "ONE_CLICK_SSO_LOGIN",
    ] {
        environment.insert(name.into(), "true".into());
    }
    environment.insert("SWIFT_ENABLED".into(), "false".into());
    environment.insert("PAM_ENABLED".into(), "false".into());
    environment.insert("CAS_ENABLED".into(), "false".into());
    environment.insert("SAML_ENABLED".into(), "false".into());

    let config = Config::from_environment(&environment).unwrap();
    assert_eq!(
        config.unsupported.object_storage,
        BTreeSet::from([ObjectStorageProvider::S3, ObjectStorageProvider::Azure])
    );
    assert_eq!(
        config.unsupported.external_auth,
        BTreeSet::from([ExternalAuthProvider::Ldap, ExternalAuthProvider::Oidc])
    );
    assert!(config.unsupported.omniauth_only);
    assert!(config.unsupported.one_click_sso_login);

    for name in [
        "S3_ENABLED",
        "SWIFT_ENABLED",
        "AZURE_ENABLED",
        "LDAP_ENABLED",
        "PAM_ENABLED",
        "CAS_ENABLED",
        "SAML_ENABLED",
        "OIDC_ENABLED",
        "OMNIAUTH_ONLY",
        "ONE_CLICK_SSO_LOGIN",
    ] {
        let error = error_for(name, "TRUE");
        assert!(error.variables().contains(&name), "got {error}");
    }
}

#[test]
fn explicit_replica_configuration_is_rejected() {
    for name in [
        "REPLICA_DATABASE_URL",
        "REPLICA_DB_HOST",
        "REPLICA_DB_PORT",
        "REPLICA_DB_NAME",
        "REPLICA_DB_USER",
        "REPLICA_DB_PASS",
        "REPLICA_DB_SSLMODE",
        "REPLICA_DB_POOL",
    ] {
        let error = error_for(name, "configured");
        assert!(error.variables().contains(&name), "got {error}");
        assert!(error.to_string().contains("read replica"));
    }
}

#[test]
fn required_secrets_dummy_key_and_vapid_pair_are_validated() {
    for name in [
        "SECRET_KEY_BASE",
        "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY",
        "ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY",
        "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT",
    ] {
        let mut environment = required_environment();
        environment.remove(name);
        let error = Config::from_environment(&environment).unwrap_err();
        assert!(error.variables().contains(&name), "got {error}");
    }

    let error = error_for("SECRET_KEY_BASE_DUMMY", "1");
    assert!(error.variables().contains(&"SECRET_KEY_BASE_DUMMY"));

    for name in ["VAPID_PRIVATE_KEY", "VAPID_PUBLIC_KEY"] {
        let error = error_for(name, "one-sided-key");
        assert!(error.variables().contains(&"VAPID_PRIVATE_KEY"));
        assert!(error.variables().contains(&"VAPID_PUBLIC_KEY"));
    }

    for name in [
        "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY",
        "ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY",
        "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT",
    ] {
        let error = error_for(name, "public-test-value-DO_NOT_USE_IN_PRODUCTION");
        assert!(error.variables().contains(&name));
        assert!(!error.to_string().contains("public-test-value"));
    }
}

#[test]
fn sidekiq_redis_uses_mastodon_precedence_and_defaults() {
    let mut environment = required_environment();
    environment.extend([
        ("SIDEKIQ_REDIS_HOST".into(), "Redis.Example".into()),
        ("SIDEKIQ_REDIS_PORT".into(), "6380".into()),
        ("SIDEKIQ_REDIS_DB".into(), "4".into()),
        ("SIDEKIQ_REDIS_USER".into(), "redis-user".into()),
        ("SIDEKIQ_REDIS_PASSWORD".into(), "redis-secret".into()),
    ]);

    let config = Config::from_environment(&environment).unwrap();
    match config.sidekiq_redis.unwrap() {
        RedisEndpoint::Tcp {
            host,
            port,
            database,
            username,
            password,
        } => {
            assert_eq!(host, "redis.example");
            assert_eq!(port, 6380);
            assert_eq!(database, 4);
            assert_eq!(username.unwrap().expose_secret(), "redis-user");
            assert_eq!(password.unwrap().expose_secret(), "redis-secret");
        }
        other @ RedisEndpoint::Url(_) => {
            panic!("expected discrete Redis endpoint, got {other:?}");
        }
    }

    let mut environment = required_environment();
    environment.insert(
        "REDIS_URL".into(),
        "rediss://redis-user:redis-secret@redis.example/2".into(),
    );
    let config = Config::from_environment(&environment).unwrap();
    assert!(matches!(config.sidekiq_redis, Some(RedisEndpoint::Url(_))));

    let mut environment = required_environment();
    environment.extend([
        ("REDIS_HOST".into(), "base.example".into()),
        ("REDIS_DB".into(), "2".into()),
        ("SIDEKIQ_REDIS_DB".into(), "9".into()),
    ]);
    let config = Config::from_environment(&environment).unwrap();
    assert!(matches!(
        config.sidekiq_redis,
        Some(RedisEndpoint::Tcp {
            ref host,
            database: 2,
            ..
        }) if host == "base.example"
    ));

    let error = error_for("SIDEKIQ_REDIS_SENTINELS", "sentinel.example:26379");
    assert!(error.to_string().contains("Sentinel"));
}

#[test]
fn debug_display_and_errors_redact_every_secret() {
    let sentinels = [
        "db-url-secret-z91",
        "smtp-login-secret-z92",
        "smtp-password-secret-z93",
        "session-secret-z94",
        "primary-key-secret-z95",
        "deterministic-key-secret-z96",
        "salt-secret-z97",
        "vapid-private-secret-z98",
        "vapid-public-secret-z99",
        "redis-url-secret-z100",
    ];
    let mut environment = required_environment();
    environment.extend([
        (
            "PRIMARY_DATABASE_URL".into(),
            format!("postgres://user:{}@database.example/mastodon", sentinels[0]),
        ),
        ("SMTP_SERVER".into(), "mail.example".into()),
        ("SMTP_LOGIN".into(), sentinels[1].into()),
        ("SMTP_PASSWORD".into(), sentinels[2].into()),
        ("SECRET_KEY_BASE".into(), sentinels[3].into()),
        (
            "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY".into(),
            sentinels[4].into(),
        ),
        (
            "ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY".into(),
            sentinels[5].into(),
        ),
        (
            "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT".into(),
            sentinels[6].into(),
        ),
        ("VAPID_PRIVATE_KEY".into(), sentinels[7].into()),
        ("VAPID_PUBLIC_KEY".into(), sentinels[8].into()),
        (
            "SIDEKIQ_REDIS_URL".into(),
            format!("redis://user:{}@redis.example/0", sentinels[9]),
        ),
    ]);

    let config = Config::from_environment(&environment).unwrap();
    let rendered = format!("{config:?}\n{config}");
    for sentinel in sentinels {
        assert!(
            !rendered.contains(sentinel),
            "leaked {sentinel} in {rendered}"
        );
    }
    assert!(rendered.contains("REDACTED"));

    let secret_url = "postgres://user:error-secret-z101@database.example/%";
    let error = error_for("PRIMARY_DATABASE_URL", secret_url);
    let rendered = format!("{error:?}\n{error}");
    assert!(!rendered.contains("error-secret-z101"));
    assert!(rendered.contains("PRIMARY_DATABASE_URL"));

    let secret_value = "dummy-error-secret-z102";
    let error = error_for("SECRET_KEY_BASE_DUMMY", secret_value);
    let rendered = format!("{error:?}\n{error}");
    assert!(!rendered.contains(secret_value));
}

#[test]
fn representative_invalid_database_values_name_the_variable() {
    for (name, value) in [
        ("PRIMARY_DATABASE_URL", "mysql://database.example/mastodon"),
        ("DB_PORT", "70000"),
        ("DB_POOL", "0"),
        ("DB_SSLMODE", "trust-me"),
    ] {
        let error = error_for(name, value);
        assert!(error.variables().contains(&name), "got {error}");
    }
}

#[test]
fn public_paths_are_not_treated_as_filesystem_preflight() {
    let mut environment = required_environment();
    environment.insert(
        "PAPERCLIP_ROOT_PATH".into(),
        "/path/that/need/not/exist/while/loading".into(),
    );
    let config = Config::from_environment(&environment).unwrap();
    assert_eq!(
        config.paperclip.root_path,
        Path::new("/path/that/need/not/exist/while/loading")
    );
}
