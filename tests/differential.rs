mod differential {
    pub mod artifacts;
    pub mod comparison;
    pub mod database;
    pub mod extended_description;
    pub mod federation;
    pub mod harness;
    pub mod mixed_profile_media;
    pub mod normalization;
    pub mod read_only;
    pub mod reauth;
    #[cfg(feature = "test-support")]
    pub mod reauth_limits;
    pub mod safety;
    pub mod writes;
}

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap as AxumHeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use differential::artifacts::{MediaSnapshot, compare_media, compare_media_with_labels};
use differential::comparison::{CapturedResponse, DEFAULT_MISMATCH_LIMIT, compare_responses};
use differential::database::{
    TableSelection, compare_database_snapshots, compare_database_snapshots_with_labels,
    snapshot_database,
};
use differential::federation::{run_actor_media_case, run_federation_discovery_case};
use differential::harness::{RequestSpec, send_identically, send_single};
use differential::read_only::ReadOnlyGuard;
use differential::safety::{DifferentialConfig, HttpTargets};
use differential::writes::{
    fixture_active_record_encryption, run_account_profile_writes_case, run_account_settings_case,
    run_admin_create_user_case, run_browser_authentication_case,
    run_browser_two_factor_management_case, run_conversation_writes_case, run_media_writes_case,
    run_notification_writes_case, run_oauth_authorization_code_case, run_password_recovery_case,
    run_poll_lifecycle_case, run_relationship_writes_case, run_report_writes_case,
    run_status_creation_writes_case, run_status_interaction_writes_case,
    run_write_transactions_case,
};
use reqwest::Method;
use reqwest::header::{
    ACCEPT, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE,
    ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD, AUTHORIZATION, CACHE_CONTROL,
    CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, HOST, HeaderMap, HeaderName, HeaderValue,
    LAST_MODIFIED, LINK, ORIGIN, RANGE, VARY, WWW_AUTHENTICATE,
};
use rustodon::mastodon::rest::{
    AccountStatusesOptions, FollowCollectionKind, FollowCollectionOptions, InstanceRuntimeConfig,
    RestProjectionLoader, RestSerializer,
};
use rustodon::mastodon::{
    BearerAuthenticator, OAuthAuthenticationError, READ_ACCOUNTS, READ_FOLLOWS, READ_STATUSES,
    Repository,
};
use rustodon::web::{WebState, router as web_router};
use serde_json::Value;
use tokio::sync::oneshot;
use url::Url;

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn instance_v2() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = checked_instance_fixture()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = Router::new()
        .route("/api/v2/instance", get(fixture_instance))
        .with_state(Arc::new(fixture));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let result = run_instance_v2_case(&rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn oauth_bearer_authentication() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let production_state = WebState::new(
        repository,
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        config.rust_media.clone(),
        fixture_instance_runtime(),
        Vec::new(),
        vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
    )?;
    let app = web_router(production_state);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let result = run_oauth_bearer_authentication_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[derive(Clone)]
struct CoreRestFixture {
    repository: Repository,
    authenticator: BearerAuthenticator,
    origin: Url,
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn core_rest_serializers() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let state = CoreRestFixture {
        authenticator: BearerAuthenticator::new(repository.clone()),
        repository,
        origin: Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let production_state = WebState::new(
        state.repository.clone(),
        state.origin.clone(),
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        config.rust_media.clone(),
        fixture_instance_runtime(),
        Vec::new(),
        vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
    )?;
    let app = web_router(production_state);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let result = run_core_rest_serializers_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn write_transactions() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    run_write_transactions_case(config).await
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn poll_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    let writer_url = config
        .rust_write_database
        .as_ref()
        .expect("poll lifecycle requires a Rust writer URL")
        .url()
        .to_owned();
    let repository = Repository::connect(config.rust_database.url()).await?;
    let writer = rustodon::mastodon::WriteRepository::connect(&writer_url).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(
        WebState::new(
            repository,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            fixture_instance_runtime(),
            Vec::new(),
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result = run_poll_lifecycle_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn notification_writes() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    let writer_url = config
        .rust_write_database
        .as_ref()
        .expect("notification writer differential configuration must include a Rust writer URL")
        .url()
        .to_owned();
    let repository = Repository::connect(config.rust_database.url()).await?;
    let writer = rustodon::mastodon::WriteRepository::connect(&writer_url).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(
        WebState::new(
            repository,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            fixture_instance_runtime(),
            Vec::new(),
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let relationship_result = run_relationship_writes_case(config.clone(), &rust_url).await;
    let status_creation_result = run_status_creation_writes_case(config.clone(), &rust_url).await;
    let interaction_result = run_status_interaction_writes_case(config.clone(), &rust_url).await;
    let notification_result = run_notification_writes_case(config.clone(), &rust_url).await;
    let profile_result = run_account_profile_writes_case(config.clone(), &rust_url).await;
    let conversation_result = run_conversation_writes_case(config, &rust_url).await;
    let result = match (
        relationship_result,
        status_creation_result,
        interaction_result,
        notification_result,
        profile_result,
        conversation_result,
    ) {
        (Ok(()), Ok(()), Ok(()), Ok(()), Ok(()), Ok(())) => Ok(()),
        (Err(error), _, _, _, _, _)
        | (_, Err(error), _, _, _, _)
        | (_, _, Err(error), _, _, _)
        | (_, _, _, Err(error), _, _)
        | (_, _, _, _, Err(error), _)
        | (_, _, _, _, _, Err(error)) => Err(error),
    };
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn report_writes() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    let writer_url = config
        .rust_write_database
        .as_ref()
        .expect("report writes require a Rust writer URL")
        .url()
        .to_owned();
    let repository = Repository::connect(config.rust_database.url()).await?;
    let writer = rustodon::mastodon::WriteRepository::connect(&writer_url).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(
        WebState::new(
            repository,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            fixture_instance_runtime(),
            Vec::new(),
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result = run_report_writes_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn browser_authentication() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    let writer_url = config
        .rust_write_database
        .as_ref()
        .expect("browser authentication requires a Rust writer URL")
        .url()
        .to_owned();
    let repository = Repository::connect(config.rust_database.url()).await?;
    let writer = rustodon::mastodon::WriteRepository::connect(&writer_url).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(
        WebState::new(
            repository,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            fixture_instance_runtime(),
            Vec::new(),
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result = run_browser_authentication_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn account_settings() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    let writer_url = config
        .rust_write_database
        .as_ref()
        .expect("account settings require a Rust writer URL")
        .url()
        .to_owned();
    let repository = Repository::connect(config.rust_database.url()).await?;
    let writer = rustodon::mastodon::WriteRepository::connect(&writer_url).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(
        WebState::new(
            repository,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            fixture_instance_runtime(),
            Vec::new(),
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result = run_account_settings_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn browser_two_factor_management() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    let writer_url = config
        .rust_write_database
        .as_ref()
        .expect("browser 2FA management requires a Rust writer URL")
        .url()
        .to_owned();
    let encryption = fixture_active_record_encryption();
    let repository = Repository::connect(config.rust_database.url())
        .await?
        .with_active_record_encryption(encryption.clone());
    let writer = rustodon::mastodon::WriteRepository::connect(&writer_url)
        .await?
        .with_active_record_encryption(encryption);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(
        WebState::new(
            repository,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            fixture_instance_runtime(),
            Vec::new(),
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result = run_browser_two_factor_management_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn password_recovery() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    let writer_url = config
        .rust_write_database
        .as_ref()
        .expect("password recovery requires a Rust writer URL")
        .url()
        .to_owned();
    let repository = Repository::connect(config.rust_database.url()).await?;
    let writer = rustodon::mastodon::WriteRepository::connect(&writer_url).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(
        WebState::new(
            repository,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            fixture_instance_runtime(),
            Vec::new(),
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result = run_password_recovery_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn admin_create_user() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    run_admin_create_user_case(config).await
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn oauth_authorization_code() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    let writer_url = config
        .rust_write_database
        .as_ref()
        .expect("OAuth authorization code requires a Rust writer URL")
        .url()
        .to_owned();
    let repository = Repository::connect(config.rust_database.url()).await?;
    let writer = rustodon::mastodon::WriteRepository::connect(&writer_url).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(
        WebState::new(
            repository,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            fixture_instance_runtime(),
            Vec::new(),
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result = run_oauth_authorization_code_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn media_writes() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let writer_url = config
        .rust_write_database
        .as_ref()
        .expect("media writer differential configuration must include a Rust writer URL")
        .url()
        .to_owned();
    let repository = Repository::connect(config.rust_database.url()).await?;
    let writer = rustodon::mastodon::WriteRepository::connect(&writer_url).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(
        WebState::new(
            repository,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            fixture_instance_runtime(),
            Vec::new(),
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result = run_media_writes_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn mixed_profile_media() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let writer_url = config
        .rust_write_database
        .as_ref()
        .expect("media writer differential configuration must include a Rust writer URL")
        .url()
        .to_owned();
    let repository = Repository::connect(config.rust_database.url()).await?;
    let writer = rustodon::mastodon::WriteRepository::connect(&writer_url).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(
        WebState::new(
            repository,
            Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
            "fixture-v4-6-5.rustodon.invalid",
            "/system",
            config.rust_media.clone(),
            fixture_instance_runtime(),
            Vec::new(),
            vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
        )?
        .with_write_repository(writer),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result =
        differential::mixed_profile_media::run_mixed_profile_media_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn rest_protocol_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(WebState::new(
        repository,
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        config.rust_media.clone(),
        fixture_instance_runtime(),
        Vec::new(),
        vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
    )?);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let result = Box::pin(run_rest_protocol_contracts_case(config, &rust_url)).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn federation_discovery() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(WebState::new(
        repository,
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        config.rust_media.clone(),
        fixture_instance_runtime(),
        Vec::new(),
        vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
    )?);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let result = Box::pin(run_federation_discovery_case(config, &rust_url)).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn actor_media_root_url() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let media_root_url = std::env::var("RUSTODON_DIFFERENTIAL_PAPERCLIP_ROOT_URL")
        .unwrap_or_else(|_| "/system".to_owned());
    set_actor_header_fixture(&config, true).await?;
    set_actor_profile_emoji_fixture(&config, true).await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(WebState::new(
        repository,
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        "fixture-v4-6-5.rustodon.invalid",
        &media_root_url,
        config.rust_media.clone(),
        fixture_instance_runtime(),
        Vec::new(),
        vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
    )?);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let result = run_actor_media_case(config.clone(), &rust_url, &media_root_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    let profile_restore = set_actor_profile_emoji_fixture(&config, false).await;
    let restore = set_actor_header_fixture(&config, false).await;
    result.and(profile_restore).and(restore)
}

async fn set_actor_profile_emoji_fixture(
    config: &DifferentialConfig,
    present: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for owner in [
        config
            .mastodon_owner_database
            .as_ref()
            .expect("actor emoji differential requires the Mastodon owner target"),
        config
            .rust_owner_database
            .as_ref()
            .expect("actor emoji differential requires the Rust owner target"),
    ] {
        let pool = sqlx::PgPool::connect(owner.url()).await?;
        let display_name = if present {
            "display_name || ' :actorprofileblob:'"
        } else {
            "replace(display_name, ' :actorprofileblob:', '')"
        };
        sqlx::query(&format!(
            "UPDATE accounts SET display_name = {display_name} WHERE id = 116844606259201001"
        ))
        .execute(&pool)
        .await?;
        sqlx::query("DELETE FROM custom_emojis WHERE id = 12991")
            .execute(&pool)
            .await?;
        if present {
            sqlx::query(
                "INSERT INTO custom_emojis
                     (id, shortcode, domain, image_content_type, image_file_name, image_file_size,
                      image_storage_schema_version, disabled, visible_in_picker, created_at, updated_at)
                 VALUES (12991, 'actorprofileblob', NULL, 'image/png', 'actor-profile.png', 68,
                         1, false, false, TIMESTAMP '2026-07-01 14:08:00',
                         TIMESTAMP '2026-07-01 14:08:00')",
            )
            .execute(&pool)
            .await?;
        }
    }
    Ok(())
}

async fn set_actor_header_fixture(
    config: &DifferentialConfig,
    present: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let values = present.then_some((
        "rustodon-actor-header.png",
        "image/png",
        68_i64,
        chrono::NaiveDate::from_ymd_opt(2026, 7, 1)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap(),
        1_i32,
    ));
    for owner in [
        config
            .mastodon_owner_database
            .as_ref()
            .expect("actor media differential requires the Mastodon owner target"),
        config
            .rust_owner_database
            .as_ref()
            .expect("actor media differential requires the Rust owner target"),
    ] {
        let pool = sqlx::PgPool::connect(owner.url()).await?;
        sqlx::query(
            "UPDATE accounts SET header_file_name = $1, header_content_type = $2, \
             header_file_size = $3, header_updated_at = $4, \
             header_storage_schema_version = $5 WHERE id = 116844606259201001",
        )
        .bind(values.map(|value| value.0))
        .bind(values.map(|value| value.1))
        .bind(values.map(|value| value.2))
        .bind(values.map(|value| value.3))
        .bind(values.map(|value| value.4))
        .execute(&pool)
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn authorized_fetch_read_routes_require_signatures() -> Result<(), Box<dyn std::error::Error>>
{
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let mut runtime = fixture_instance_runtime();
    runtime.limited_federation = true;
    let app = web_router(WebState::new(
        repository,
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        config.rust_media.clone(),
        runtime,
        Vec::new(),
        vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
    )?);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let result = async {
        let client = reqwest::Client::builder().no_proxy().build()?;
        for path in [
            "/users/alice",
            "/users/alice/statuses/116844842188805001",
            "/users/alice/statuses/116844842188805001/activity",
            "/users/alice/outbox",
            "/users/alice/followers",
            "/users/alice/following",
        ] {
            let response = client
                .get(rust_url.join(path)?)
                .header(ACCEPT, "application/activity+json")
                .send()
                .await?;
            assert_eq!(response.status().as_u16(), 401, "unsigned route {path}");
        }
        let instance_actor = client
            .get(rust_url.join("/actor")?)
            .header(ACCEPT, "application/activity+json")
            .send()
            .await?;
        assert_eq!(instance_actor.status().as_u16(), 200);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn local_paperclip_media() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(WebState::new(
        repository,
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        config.rust_media.clone(),
        fixture_instance_runtime(),
        Vec::new(),
        vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
    )?);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let result = run_local_paperclip_media_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn local_paperclip_deleted_media() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(WebState::new(
        repository,
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        config.rust_media.clone(),
        fixture_instance_runtime(),
        Vec::new(),
        vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
    )?);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let result = run_local_paperclip_deleted_media_case(&config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
#[allow(clippy::too_many_lines)]
async fn local_web_client_shell() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(WebState::new(
        repository,
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        config.rust_media.clone(),
        fixture_instance_runtime(),
        Vec::new(),
        vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
    )?);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let client = reqwest::Client::new();
    let request = |path: &str| {
        client
            .get(
                rust_url
                    .join(path.trim_start_matches('/'))
                    .expect("valid test path"),
            )
            .header(HOST, "fixture-v4-6-5.rustodon.invalid")
    };

    let response = request("/").send().await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let has_cookie = response.headers().contains_key("set-cookie");
    let csp = response
        .headers()
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .map_or_else(String::new, ToOwned::to_owned);
    assert_eq!(response.headers()["x-frame-options"], "DENY");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.headers()["x-xss-protection"], "0");
    assert_eq!(response.headers()["referrer-policy"], "same-origin");
    let body = response.text().await?;
    assert_eq!(status, reqwest::StatusCode::OK, "{body}");
    assert_eq!(content_type.as_deref(), Some("text/html; charset=utf-8"));
    assert!(has_cookie);
    assert!(csp.contains("default-src 'none'"));
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("'nonce-"));
    assert!(body.contains("id=\"mastodon\""));
    assert!(body.contains("name=\"csrf-token\""));
    assert!(body.contains("/packs/application-"));

    let response = request("/statuses/116844606259201001").send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(response.text().await?.contains("id=\"mastodon\""));

    for path in [
        "/notifications_v2",
        "/notifications_v2/requests",
        "/overview/about",
        "/pinned",
    ] {
        let response = request(path).send().await?;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "{path}");
        assert!(response.text().await?.contains("id=\"mastodon\""));
    }

    let response = request("/manifest").send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.headers()[CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    let manifest: Value = serde_json::from_slice(&response.bytes().await?)?;
    assert_eq!(manifest["instance"]["id"], "/home");
    assert_eq!(manifest["instance"]["icons"].as_array().unwrap().len(), 9);

    let response = request("/sw.js").send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.headers()["service-worker-allowed"], "/");
    assert!(response.text().await?.contains("self"));

    let response = request("/favicon.ico").send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "image/png");

    for path in [
        "/badge.png",
        "/android-chrome-192x192.png",
        "/web-push-icon_expand.png",
        "/web-push-icon_favourite.png",
        "/web-push-icon_reblog.png",
        "/sounds/boop.mp3",
    ] {
        assert_eq!(
            request(path).send().await?.status(),
            reqwest::StatusCode::OK
        );
    }

    let response = request("/random").send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

    let _ = shutdown_tx.send(());
    server.await??;
    Ok(())
}

#[tokio::test]
#[ignore = "requires guarded Mastodon/PostgreSQL/media clones from tools/mastodon-fixture"]
async fn status_authorization_matrix() -> Result<(), Box<dyn std::error::Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository_root)?;
    config.validate_database_comments().await?;
    let repository = Repository::connect(config.rust_database.url()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let rust_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = web_router(WebState::new(
        repository,
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/")?,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        config.rust_media.clone(),
        fixture_instance_runtime(),
        Vec::new(),
        vec!["fixture-v4-6-5.rustodon.invalid".to_owned()],
    )?);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let result = run_status_authorization_matrix_case(config, &rust_url).await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}

async fn fixture_instance(State(body): State<Arc<Vec<u8>>>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(body.as_ref().clone()))
        .expect("the checked fixture response is valid")
}

#[allow(dead_code)]
async fn fixture_account(
    State(state): State<CoreRestFixture>,
    Path(account_id): Path<i64>,
    headers: AxumHeaderMap,
) -> Response {
    let viewer_account_id = if headers.contains_key(AUTHORIZATION) {
        match state
            .authenticator
            .authenticate(&headers, READ_ACCOUNTS)
            .await
        {
            Ok(authenticated) => authenticated
                .resource_owner()
                .map(rustodon::mastodon::OAuthResourceOwner::account_id),
            Err(OAuthAuthenticationError::OAuth(error)) => {
                return fixture_cors_response(error.into_http_response().map(Body::from));
            }
            Err(OAuthAuthenticationError::Repository(_)) => {
                return fixture_internal_error();
            }
        }
    } else {
        None
    };
    let loader = RestProjectionLoader::new(
        state.repository,
        viewer_account_id,
        "fixture-v4-6-5.rustodon.invalid",
    );
    let account = match loader.account(account_id).await {
        Ok(Some(account)) => account,
        Ok(None) => {
            return fixture_json_response(
                StatusCode::NOT_FOUND,
                br#"{"error":"Record not found"}"#.to_vec(),
            );
        }
        Err(_) => return fixture_internal_error(),
    };
    let serializer = RestSerializer::new(
        &state.origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    match serializer.account(&account).and_then(|account| {
        serde_json::to_vec(&account)
            .map_err(|_| rustodon::mastodon::rest::RestError::InvalidRemoteUrl(account_id))
    }) {
        Ok(body) => fixture_json_response(StatusCode::OK, body),
        Err(_) => fixture_internal_error(),
    }
}

#[allow(dead_code)]
async fn fixture_account_lookup(
    State(state): State<CoreRestFixture>,
    RawQuery(query): RawQuery,
    headers: AxumHeaderMap,
) -> Response {
    let viewer_account_id = if headers.contains_key(AUTHORIZATION) {
        match state
            .authenticator
            .authenticate(&headers, READ_ACCOUNTS)
            .await
        {
            Ok(authenticated) => authenticated
                .resource_owner()
                .map(rustodon::mastodon::OAuthResourceOwner::account_id),
            Err(OAuthAuthenticationError::OAuth(error)) => {
                return fixture_cors_response(error.into_http_response().map(Body::from));
            }
            Err(OAuthAuthenticationError::Repository(_)) => return fixture_internal_error(),
        }
    } else {
        None
    };
    let handle = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .find(|(name, _)| name == "acct")
        .map(|(_, value)| value.into_owned());
    let loader = RestProjectionLoader::new(
        state.repository,
        viewer_account_id,
        "fixture-v4-6-5.rustodon.invalid",
    );
    let account = match handle {
        Some(handle) => match loader.lookup_account(&handle).await {
            Ok(Some(account)) => account,
            Ok(None) => {
                return fixture_json_response(
                    StatusCode::NOT_FOUND,
                    br#"{"error":"Record not found"}"#.to_vec(),
                );
            }
            Err(_) => return fixture_internal_error(),
        },
        None => {
            return fixture_json_response(
                StatusCode::NOT_FOUND,
                br#"{"error":"Record not found"}"#.to_vec(),
            );
        }
    };
    let serializer = RestSerializer::new(
        &state.origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    match serializer.account(&account).and_then(|account| {
        serde_json::to_vec(&account)
            .map_err(|_| rustodon::mastodon::rest::RestError::InvalidRemoteUrl(account.id.value()))
    }) {
        Ok(body) => fixture_json_response(StatusCode::OK, body),
        Err(_) => fixture_internal_error(),
    }
}

#[allow(dead_code)]
async fn fixture_account_statuses(
    State(state): State<CoreRestFixture>,
    Path(account_id): Path<i64>,
    RawQuery(query): RawQuery,
    headers: AxumHeaderMap,
) -> Response {
    let viewer_account_id = if headers.contains_key(AUTHORIZATION) {
        match state
            .authenticator
            .authenticate(&headers, READ_STATUSES)
            .await
        {
            Ok(authenticated) => authenticated
                .resource_owner()
                .map(rustodon::mastodon::OAuthResourceOwner::account_id),
            Err(OAuthAuthenticationError::OAuth(error)) => {
                return fixture_cors_response(error.into_http_response().map(Body::from));
            }
            Err(OAuthAuthenticationError::Repository(_)) => return fixture_internal_error(),
        }
    } else {
        None
    };
    let parameters = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    let value = |name: &str| {
        parameters
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    };
    let boolean = |name: &str| matches!(value(name), Some("true" | "1"));
    let cursor = |name: &str| value(name).and_then(|value| value.parse::<i64>().ok());
    let limit = value("limit").map_or(20, |value| {
        value
            .parse::<i64>()
            .unwrap_or_default()
            .saturating_abs()
            .min(40)
    });
    let options = AccountStatusesOptions {
        max_id: cursor("max_id"),
        min_id: cursor("min_id"),
        since_id: cursor("since_id"),
        limit,
        pinned: boolean("pinned"),
        tagged: value("tagged").map(str::to_lowercase),
        only_media: boolean("only_media"),
        exclude_replies: boolean("exclude_replies"),
        exclude_reblogs: boolean("exclude_reblogs"),
        exclude_direct: boolean("exclude_direct"),
    };
    let loader = RestProjectionLoader::new(
        state.repository,
        viewer_account_id,
        "fixture-v4-6-5.rustodon.invalid",
    );
    match loader.account(account_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return fixture_json_response(
                StatusCode::NOT_FOUND,
                br#"{"error":"Record not found"}"#.to_vec(),
            );
        }
        Err(_) => return fixture_internal_error(),
    }
    let Ok(statuses) = loader.account_statuses(account_id, &options).await else {
        return fixture_internal_error();
    };
    let serializer = RestSerializer::new(
        &state.origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    let statuses = statuses
        .iter()
        .map(|status| serializer.status(status, rustodon::mastodon::rest::StatusShape::Full))
        .collect::<Result<Vec<_>, _>>();
    match statuses.and_then(|statuses| {
        serde_json::to_vec(&statuses)
            .map_err(|_| rustodon::mastodon::rest::RestError::InvalidRemoteUrl(account_id))
    }) {
        Ok(body) => fixture_json_response(StatusCode::OK, body),
        Err(_) => fixture_internal_error(),
    }
}

#[allow(dead_code)]
async fn fixture_account_followers(
    state: State<CoreRestFixture>,
    path: Path<i64>,
    query: RawQuery,
    headers: AxumHeaderMap,
) -> Response {
    fixture_account_follows(state, path, query, headers, FollowCollectionKind::Followers).await
}

#[allow(dead_code)]
async fn fixture_account_following(
    state: State<CoreRestFixture>,
    path: Path<i64>,
    query: RawQuery,
    headers: AxumHeaderMap,
) -> Response {
    fixture_account_follows(state, path, query, headers, FollowCollectionKind::Following).await
}

#[allow(dead_code)]
async fn fixture_account_follows(
    State(state): State<CoreRestFixture>,
    Path(account_id): Path<i64>,
    RawQuery(query): RawQuery,
    headers: AxumHeaderMap,
    kind: FollowCollectionKind,
) -> Response {
    let viewer_account_id = if headers.contains_key(AUTHORIZATION) {
        match state
            .authenticator
            .authenticate(&headers, READ_ACCOUNTS)
            .await
        {
            Ok(authenticated) => authenticated
                .resource_owner()
                .map(rustodon::mastodon::OAuthResourceOwner::account_id),
            Err(OAuthAuthenticationError::OAuth(error)) => {
                return fixture_cors_response(error.into_http_response().map(Body::from));
            }
            Err(OAuthAuthenticationError::Repository(_)) => return fixture_internal_error(),
        }
    } else {
        None
    };
    let parameters = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    let value = |name: &str| {
        parameters
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    };
    let options = FollowCollectionOptions {
        max_id: value("max_id").and_then(|value| value.parse().ok()),
        since_id: value("since_id").and_then(|value| value.parse().ok()),
        limit: value("limit").map_or(40, |value| {
            value
                .parse::<i64>()
                .unwrap_or_default()
                .saturating_abs()
                .min(80)
        }),
    };
    let loader = RestProjectionLoader::new(
        state.repository,
        viewer_account_id,
        "fixture-v4-6-5.rustodon.invalid",
    );
    match loader.account(account_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return fixture_json_response(
                StatusCode::NOT_FOUND,
                br#"{"error":"Record not found"}"#.to_vec(),
            );
        }
        Err(_) => return fixture_internal_error(),
    }
    let Ok(page) = loader.follow_collection(account_id, kind, &options).await else {
        return fixture_internal_error();
    };
    let serializer = RestSerializer::new(
        &state.origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    let accounts = page
        .accounts
        .iter()
        .map(|account| serializer.account(account))
        .collect::<Result<Vec<_>, _>>();
    match accounts.and_then(|accounts| {
        serde_json::to_vec(&accounts)
            .map_err(|_| rustodon::mastodon::rest::RestError::InvalidRemoteUrl(account_id))
    }) {
        Ok(body) => fixture_json_response(StatusCode::OK, body),
        Err(_) => fixture_internal_error(),
    }
}

#[allow(dead_code)]
async fn fixture_core_instance_v1(State(state): State<CoreRestFixture>) -> Response {
    fixture_core_instance(state, true).await
}

#[allow(dead_code)]
async fn fixture_core_instance_v2(State(state): State<CoreRestFixture>) -> Response {
    fixture_core_instance(state, false).await
}

#[allow(dead_code)]
async fn fixture_core_instance(state: CoreRestFixture, v1: bool) -> Response {
    let loader =
        RestProjectionLoader::new(state.repository, None, "fixture-v4-6-5.rustodon.invalid");
    let Ok(instance) = loader.instance(fixture_instance_runtime()).await else {
        return fixture_internal_error();
    };
    let serializer = RestSerializer::new(
        &state.origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    let body = if v1 {
        serializer
            .instance_v1(&instance)
            .ok()
            .and_then(|instance| serde_json::to_vec(&instance).ok())
    } else {
        serializer
            .instance_v2(&instance)
            .ok()
            .and_then(|instance| serde_json::to_vec(&instance).ok())
    };
    match body {
        Some(body) => fixture_json_response(StatusCode::OK, body),
        None => fixture_internal_error(),
    }
}

fn fixture_instance_runtime() -> InstanceRuntimeConfig {
    let icons = [
        (36, "DLiBQg3N"),
        (48, "C7lKWFwX"),
        (72, "9LRpA3QN"),
        (96, "BKKwkkY-"),
        (144, "D-ewI-KZ"),
        (192, "jYKJbpas"),
        (256, "DXt2vsq7"),
        (384, "CbK7cG33"),
        (512, "Dz2ThkhV"),
    ]
    .into_iter()
    .map(|(size, digest)| {
        (
            format!(
                "https://fixture-v4-6-5.rustodon.invalid/packs/assets/android-chrome-{size}x{size}-{digest}.png"
            ),
            format!("{size}x{size}"),
        )
    })
    .collect();
    InstanceRuntimeConfig {
        domain: "fixture-v4-6-5.rustodon.invalid".to_owned(),
        version: "4.6.5".to_owned(),
        source_url: "https://github.com/mastodon/mastodon".to_owned(),
        streaming_api: "wss://fixture-v4-6-5.rustodon.invalid".to_owned(),
        vapid_public_key: Some(
            "BB37UCyc8LLX4PNQSe-04vSFvpUWGrENubUaslVFM_l5TxcGVMY0C3RXPeUJAQHKYlcOM2P4vTYmkoo0VZGZTM4="
                .to_owned(),
        ),
        thumbnail_url:
            "https://fixture-v4-6-5.rustodon.invalid/packs/assets/preview-vSUsFXid.png"
                .to_owned(),
        thumbnail_description: "Two smiling cartoon mastodons (who look like elephants) toss a paper plane between them. They're surrounded by a bright blue sky and floating planets with trees and more mastodons on them."
            .to_owned(),
        thumbnail_blurhash: None,
        thumbnail_versions: None,
        icons,
        languages: vec!["en".to_owned()],
        active_month: 0,
        active_halfyear: 0,
        translation_enabled: false,
        limited_federation: false,
        single_user_mode: false,
        terms_of_service_url: None,
        sso_signup_url: None,
        wrapstodon: None,
    }
}

#[allow(dead_code)]
async fn fixture_relationships(
    State(state): State<CoreRestFixture>,
    RawQuery(query): RawQuery,
    headers: AxumHeaderMap,
) -> Response {
    let authenticated = match state
        .authenticator
        .authenticate(&headers, READ_FOLLOWS)
        .await
    {
        Ok(authenticated) => authenticated,
        Err(OAuthAuthenticationError::OAuth(error)) => {
            return fixture_cors_response(error.into_http_response().map(Body::from));
        }
        Err(OAuthAuthenticationError::Repository(_)) => return fixture_internal_error(),
    };
    let owner = match authenticated.require_user() {
        Ok(owner) => owner,
        Err(error) => return fixture_cors_response(error.into_http_response().map(Body::from)),
    };
    let ids = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .filter(|(name, _)| name == "id[]" || name == "id")
        .filter_map(|(_, value)| value.parse::<i64>().ok())
        .collect::<Vec<_>>();
    let loader = RestProjectionLoader::new(
        state.repository,
        Some(owner.account_id()),
        "fixture-v4-6-5.rustodon.invalid",
    );
    let with_suspended = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .any(|(name, value)| name == "with_suspended" && matches!(value.as_ref(), "true" | "1"));
    let Ok(relationships) = loader.relationships(&ids, with_suspended).await else {
        return fixture_internal_error();
    };
    let serializer = RestSerializer::new(
        &state.origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    let relationships = relationships
        .iter()
        .map(|relationship| serializer.relationship(relationship))
        .collect::<Vec<_>>();
    match serde_json::to_vec(&relationships) {
        Ok(body) => fixture_json_response(StatusCode::OK, body),
        Err(_) => fixture_internal_error(),
    }
}

#[allow(dead_code)]
async fn fixture_status(
    State(state): State<CoreRestFixture>,
    Path(status_id): Path<i64>,
    headers: AxumHeaderMap,
) -> Response {
    let viewer_account_id = if headers.contains_key(AUTHORIZATION) {
        match state
            .authenticator
            .authenticate(&headers, READ_STATUSES)
            .await
        {
            Ok(authenticated) => authenticated
                .resource_owner()
                .map(rustodon::mastodon::OAuthResourceOwner::account_id),
            Err(OAuthAuthenticationError::OAuth(error)) => {
                return fixture_cors_response(error.into_http_response().map(Body::from));
            }
            Err(OAuthAuthenticationError::Repository(_)) => return fixture_internal_error(),
        }
    } else {
        None
    };
    let loader = RestProjectionLoader::new(
        state.repository,
        viewer_account_id,
        "fixture-v4-6-5.rustodon.invalid",
    );
    let status = match loader.authorized_status(status_id).await {
        Ok(Some(status)) => status,
        Ok(None) => {
            return fixture_json_response(
                StatusCode::NOT_FOUND,
                br#"{"error":"Not Found"}"#.to_vec(),
            );
        }
        Err(_) => return fixture_internal_error(),
    };
    let serializer = RestSerializer::new(
        &state.origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    match serializer.status(&status, rustodon::mastodon::rest::StatusShape::Full) {
        Ok(status) => match serde_json::to_vec(&status) {
            Ok(body) => fixture_json_response(StatusCode::OK, body),
            Err(_) => fixture_internal_error(),
        },
        Err(_) => fixture_internal_error(),
    }
}

#[allow(dead_code)]
async fn fixture_status_context(
    State(state): State<CoreRestFixture>,
    Path(status_id): Path<i64>,
    headers: AxumHeaderMap,
) -> Response {
    let viewer_account_id = if headers.contains_key(AUTHORIZATION) {
        match state
            .authenticator
            .authenticate(&headers, READ_STATUSES)
            .await
        {
            Ok(authenticated) => authenticated
                .resource_owner()
                .map(rustodon::mastodon::OAuthResourceOwner::account_id),
            Err(OAuthAuthenticationError::OAuth(error)) => {
                return fixture_cors_response(error.into_http_response().map(Body::from));
            }
            Err(OAuthAuthenticationError::Repository(_)) => return fixture_internal_error(),
        }
    } else {
        None
    };
    let loader = RestProjectionLoader::new(
        state.repository,
        viewer_account_id,
        "fixture-v4-6-5.rustodon.invalid",
    );
    let context = match loader.status_context(status_id).await {
        Ok(Some(context)) => context,
        Ok(None) => {
            return fixture_json_response(
                StatusCode::NOT_FOUND,
                br#"{"error":"Not Found"}"#.to_vec(),
            );
        }
        Err(_) => return fixture_internal_error(),
    };
    let serializer = RestSerializer::new(
        &state.origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    match serializer.status_context(&context).and_then(|context| {
        serde_json::to_vec(&context)
            .map_err(|_| rustodon::mastodon::rest::RestError::InvalidRemoteUrl(status_id))
    }) {
        Ok(body) => fixture_json_response(StatusCode::OK, body),
        Err(_) => fixture_internal_error(),
    }
}

fn fixture_json_response(status: StatusCode, body: Vec<u8>) -> Response {
    fixture_cors_response(
        Response::builder()
            .status(status)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
            .header(CACHE_CONTROL, "private, no-store")
            .body(Body::from(body))
            .expect("static fixture response headers are valid"),
    )
}

fn fixture_internal_error() -> Response {
    fixture_json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        br#"{"error":"REST projection failed"}"#.to_vec(),
    )
}

fn fixture_cors_response(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Authorization, Origin"));
    response
}

async fn run_instance_v2_case(rust_url: &Url) -> Result<(), Box<dyn std::error::Error>> {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = DifferentialConfig::from_process_environment(&repository)?;
    config.validate_database_comments().await?;
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;

    let (mastodon_database_before, rust_database_before) = tokio::try_join!(
        snapshot_database(config.mastodon_database.url(), TableSelection::AllPublic),
        snapshot_database(config.rust_database.url(), TableSelection::AllPublic),
    )?;
    for snapshot in [&mastodon_database_before, &rust_database_before] {
        for materialized_view in [
            "account_summaries",
            "global_follow_recommendations",
            "instances",
        ] {
            if !snapshot.tables.contains_key(materialized_view) {
                return Err(format!(
                    "database snapshot omitted materialized view {materialized_view}"
                )
                .into());
            }
        }
    }
    compare_database_snapshots(
        &mastodon_database_before,
        &rust_database_before,
        DEFAULT_MISMATCH_LIMIT,
    )?;
    let mastodon_media_before = MediaSnapshot::capture(&config.mastodon_media)?;
    let rust_media_before = MediaSnapshot::capture(&config.rust_media)?;
    compare_media(
        &mastodon_media_before,
        &rust_media_before,
        DEFAULT_MISMATCH_LIMIT,
    )?;

    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    let request = RequestSpec::new(Method::GET, "/api/v2/instance", None, headers, Vec::new())?;
    let responses = send_identically(&targets, &request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )?;

    let (mastodon_database_after, rust_database_after) = tokio::try_join!(
        snapshot_database(config.mastodon_database.url(), TableSelection::AllPublic),
        snapshot_database(config.rust_database.url(), TableSelection::AllPublic),
    )?;
    compare_database_snapshots_with_labels(
        &mastodon_database_before,
        &mastodon_database_after,
        "Mastodon before",
        "Mastodon after",
        DEFAULT_MISMATCH_LIMIT,
    )?;
    compare_database_snapshots_with_labels(
        &rust_database_before,
        &rust_database_after,
        "Rust before",
        "Rust after",
        DEFAULT_MISMATCH_LIMIT,
    )?;
    compare_database_snapshots(
        &mastodon_database_after,
        &rust_database_after,
        DEFAULT_MISMATCH_LIMIT,
    )?;

    let mastodon_media_after = MediaSnapshot::capture(&config.mastodon_media)?;
    let rust_media_after = MediaSnapshot::capture(&config.rust_media)?;
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

#[allow(clippy::too_many_lines)]
async fn run_oauth_bearer_authentication_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn std::error::Error>> {
    let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
    let (mastodon_database_before, rust_database_before) = tokio::try_join!(
        snapshot_database(config.mastodon_database.url(), TableSelection::AllPublic),
        snapshot_database(config.rust_database.url(), TableSelection::AllPublic),
    )?;
    compare_database_snapshots(
        &mastodon_database_before,
        &rust_database_before,
        DEFAULT_MISMATCH_LIMIT,
    )?;
    let mastodon_media_before = MediaSnapshot::capture(&config.mastodon_media)?;
    let rust_media_before = MediaSnapshot::capture(&config.rust_media)?;
    compare_media(
        &mastodon_media_before,
        &rust_media_before,
        DEFAULT_MISMATCH_LIMIT,
    )?;

    let metadata_request = RequestSpec::new(
        Method::GET,
        "/.well-known/oauth-authorization-server",
        None,
        stable_request_headers(),
        Vec::new(),
    )?;
    let metadata = send_identically(&targets, &metadata_request).await?;
    compare_responses(
        &metadata.mastodon,
        &metadata.rust,
        &[CONTENT_TYPE, CACHE_CONTROL, VARY],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("OAuth metadata: {error}"))?;

    for (label, path, query, token) in [
        (
            "broad status read",
            "/api/v1/markers",
            Some("timeline[]=home&timeline[]=notifications"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "granular status read",
            "/api/v1/markers",
            Some("timeline[]=home&timeline[]=notifications"),
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "granular account read",
            "/api/v1/featured_tags/suggestions",
            None,
            Some("fixture-bearer-read-accounts-v4-6-5"),
        ),
        (
            "missing token",
            "/api/v1/markers",
            Some("timeline[]=home"),
            None,
        ),
        (
            "unknown token",
            "/api/v1/markers",
            Some("timeline[]=home"),
            Some("fixture-bearer-unknown-v4-6-5"),
        ),
        (
            "revoked token",
            "/api/v1/markers",
            Some("timeline[]=home"),
            Some("fixture-bearer-revoked-v4-6-5"),
        ),
        (
            "expired token",
            "/api/v1/markers",
            Some("timeline[]=home"),
            Some("fixture-bearer-expired-v4-6-5"),
        ),
        (
            "insufficient scope",
            "/api/v1/markers",
            Some("timeline[]=home"),
            Some("fixture-bearer-insufficient-v4-6-5"),
        ),
        (
            "application-only token",
            "/api/v1/markers",
            Some("timeline[]=home"),
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        (
            "disabled owner",
            "/api/v1/markers",
            Some("timeline[]=home"),
            Some("fixture-bearer-disabled-user-v4-6-5"),
        ),
        (
            "owner missing required 2FA",
            "/api/v1/markers",
            Some("timeline[]=home"),
            Some("fixture-bearer-missing-2fa-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            path,
            query.map(str::to_owned),
            headers,
            Vec::new(),
        )?;
        let responses = send_identically(&targets, &request).await?;
        if response_contains_fixture_credential(&responses.mastodon)
            || response_contains_fixture_credential(&responses.rust)
        {
            return Err(format!("{label} response exposed an OAuth fixture credential").into());
        }
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }

    for (label, path, token) in [
        (
            "application credentials",
            "/api/v1/apps/verify_credentials",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "application credentials trailing slash",
            "/api/v1/apps/verify_credentials/",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "application-only credentials",
            "/api/v1/apps/verify_credentials",
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        (
            "application credentials without read scope",
            "/api/v1/apps/verify_credentials",
            Some("fixture-bearer-insufficient-v4-6-5"),
        ),
        (
            "application credentials missing token",
            "/api/v1/apps/verify_credentials",
            None,
        ),
        (
            "application credentials revoked token",
            "/api/v1/apps/verify_credentials",
            Some("fixture-bearer-revoked-v4-6-5"),
        ),
        (
            "application credentials expired token",
            "/api/v1/apps/verify_credentials",
            Some("fixture-bearer-expired-v4-6-5"),
        ),
        (
            "application credentials unknown token",
            "/api/v1/apps/verify_credentials",
            Some("fixture-bearer-unknown-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = send_identically(&targets, &request).await?;
        if response_contains_fixture_credential(&responses.mastodon)
            || response_contains_fixture_credential(&responses.rust)
        {
            return Err(format!("{label} response exposed an OAuth fixture credential").into());
        }
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }

    for (label, query) in [
        (
            "application credentials access token parameter",
            "access_token=fixture-bearer-token-v4-6-5",
        ),
        (
            "application credentials bearer token parameter",
            "bearer_token=fixture-bearer-token-v4-6-5",
        ),
    ] {
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/apps/verify_credentials",
            Some(query.to_owned()),
            stable_request_headers(),
            Vec::new(),
        )?;
        let responses = send_identically(&targets, &request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }

    let (mastodon_database_after, rust_database_after) = tokio::try_join!(
        snapshot_database(config.mastodon_database.url(), TableSelection::AllPublic),
        snapshot_database(config.rust_database.url(), TableSelection::AllPublic),
    )?;
    compare_database_snapshots_with_labels(
        &mastodon_database_before,
        &mastodon_database_after,
        "Mastodon before",
        "Mastodon after",
        DEFAULT_MISMATCH_LIMIT,
    )?;
    compare_database_snapshots_with_labels(
        &rust_database_before,
        &rust_database_after,
        "Rust before",
        "Rust after",
        DEFAULT_MISMATCH_LIMIT,
    )?;
    compare_database_snapshots(
        &mastodon_database_after,
        &rust_database_after,
        DEFAULT_MISMATCH_LIMIT,
    )?;

    let mastodon_media_after = MediaSnapshot::capture(&config.mastodon_media)?;
    let rust_media_after = MediaSnapshot::capture(&config.rust_media)?;
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

#[allow(clippy::too_many_lines)]
async fn run_core_rest_serializers_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn std::error::Error>> {
    set_announcement_fixture(&config, true).await?;
    if let Err(error) = set_tag_search_fixture(&config, true).await {
        let _ = set_tag_search_fixture(&config, false).await;
        set_announcement_fixture(&config, false).await?;
        return Err(error);
    }
    let result = run_core_rest_serializers_read_only_case(config.clone(), rust_url).await;
    let tag_cleanup = set_tag_search_fixture(&config, false).await;
    let announcement_cleanup = set_announcement_fixture(&config, false).await;
    result.and(tag_cleanup).and(announcement_cleanup)
}

async fn set_tag_search_fixture(
    config: &DifferentialConfig,
    present: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for owner in [
        config
            .mastodon_owner_database
            .as_ref()
            .expect("tag search differential requires the Mastodon owner target"),
        config
            .rust_owner_database
            .as_ref()
            .expect("tag search differential requires the Rust owner target"),
    ] {
        let pool = sqlx::PgPool::connect(owner.url()).await?;
        let mut transaction = pool.begin().await?;
        sqlx::query("DELETE FROM tags WHERE id BETWEEN -9214 AND -9211")
            .execute(&mut *transaction)
            .await?;
        if present {
            sqlx::query(
                "INSERT INTO tags \
                 (id, name, display_name, usable, trendable, listable, reviewed_at, created_at, updated_at) \
                 VALUES \
                 (-9211, 'reviewprobe', 'ReviewProbe', true, false, true, NULL, \
                  TIMESTAMP '2026-07-01 17:41:00', TIMESTAMP '2026-07-01 17:41:00'), \
                 (-9212, 'reviewprobehidden', 'ReviewProbeHidden', true, false, true, NULL, \
                  TIMESTAMP '2026-07-01 17:41:00', TIMESTAMP '2026-07-01 17:41:00'), \
                 (-9213, 'reviewprobereviewed', 'ReviewProbeReviewed', true, false, true, \
                  TIMESTAMP '2026-07-01 17:41:00', TIMESTAMP '2026-07-01 17:41:00', \
                  TIMESTAMP '2026-07-01 17:41:00'), \
                 (-9214, 'reviewprobeunlisted', 'ReviewProbeUnlisted', true, false, false, \
                  TIMESTAMP '2026-07-01 17:41:00', TIMESTAMP '2026-07-01 17:41:00', \
                  TIMESTAMP '2026-07-01 17:41:00')",
            )
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
    }
    Ok(())
}

async fn set_announcement_fixture(
    config: &DifferentialConfig,
    present: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for owner in [
        config
            .mastodon_owner_database
            .as_ref()
            .expect("announcement differential requires the Mastodon owner target"),
        config
            .rust_owner_database
            .as_ref()
            .expect("announcement differential requires the Rust owner target"),
    ] {
        let pool = sqlx::PgPool::connect(owner.url()).await?;
        let mut transaction = pool.begin().await?;
        sqlx::query("DELETE FROM announcements WHERE id = -8701")
            .execute(&mut *transaction)
            .await?;
        if present {
            sqlx::query(
                "INSERT INTO announcements \
                 (id, all_day, created_at, ends_at, notification_sent_at, published, \
                  published_at, scheduled_at, starts_at, status_ids, text, updated_at) \
                 VALUES (-8701, false, TIMESTAMP '2026-07-01 17:36:00', NULL, NULL, true, \
                  TIMESTAMP '2026-07-01 17:36:00', NULL, TIMESTAMP '2026-07-01 17:36:00', \
                  ARRAY[116844842188805001]::bigint[], \
                  'Differential announcement word#ignored for @moderator #fixturetag', \
                  TIMESTAMP '2026-07-01 17:37:00')",
            )
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "INSERT INTO announcement_mutes \
                 (id, account_id, announcement_id, created_at, updated_at) \
                 VALUES (-8701, 116844606259201001, -8701, \
                  TIMESTAMP '2026-07-01 17:38:00', TIMESTAMP '2026-07-01 17:38:00')",
            )
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "INSERT INTO announcement_reactions \
                 (id, account_id, announcement_id, created_at, custom_emoji_id, name, updated_at) \
                 VALUES \
                  (-8701, 116844606259201001, -8701, TIMESTAMP '2026-07-01 17:39:00', \
                   NULL, 'wave', TIMESTAMP '2026-07-01 17:39:00'), \
                  (-8702, 116844606259201002, -8701, TIMESTAMP '2026-07-01 17:40:00', \
                   NULL, 'wave', TIMESTAMP '2026-07-01 17:40:00')",
            )
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn run_core_rest_serializers_read_only_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn std::error::Error>> {
    let guard = ReadOnlyGuard::begin(config, rust_url).await?;
    for path in [
        "/api/v1/instance",
        "/api/v2/instance",
        "/api/v1/instance/rules",
        "/api/v1/instance/translation_languages",
    ] {
        let request = RequestSpec::new(
            Method::GET,
            path,
            None,
            stable_request_headers(),
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("instance serializer {path}: {error}"))?;
    }
    let mut startup_headers = stable_request_headers();
    startup_headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let announcements = RequestSpec::new(
        Method::GET,
        "/api/v1/announcements",
        None,
        startup_headers,
        Vec::new(),
    )?;
    let responses = guard.send(&announcements).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("announcement serializer: {error}"))?;
    let announcements = serde_json::from_slice::<Value>(&responses.rust.body)?;
    let active = announcements
        .as_array()
        .and_then(|announcements| announcements.iter().find(|item| item["id"] == "-8701"))
        .ok_or("announcement serializer omitted the active persisted announcement")?;
    for key in ["mentions", "statuses", "tags", "emojis", "reactions"] {
        if !active[key].is_array() {
            return Err(format!("announcement serializer omitted the {key} array").into());
        }
    }
    if active["read"] != true || active["reactions"][0]["me"] != true {
        return Err("announcement serializer omitted authenticated mute/reaction state".into());
    }

    let mut search_headers = stable_request_headers();
    search_headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let hashtag_search = RequestSpec::new(
        Method::GET,
        "/api/v2/search",
        Some("q=fixturetag&type=hashtags".to_owned()),
        search_headers.clone(),
        Vec::new(),
    )?;
    let responses = guard.send(&hashtag_search).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("frontend hashtag search probe: {error}"))?;
    for (side, response) in [("Mastodon", &responses.mastodon), ("Rust", &responses.rust)] {
        let body: Value = serde_json::from_slice(&response.body)?;
        for key in ["accounts", "statuses", "hashtags"] {
            if !body[key].is_array() {
                return Err(format!("{side} hashtag search omitted the {key} array").into());
            }
        }
    }
    let reviewed_hashtag_search = RequestSpec::new(
        Method::GET,
        "/api/v2/search",
        Some("q=%23ReviewProbe&type=hashtags&exclude_unreviewed=true&limit=20&offset=0".to_owned()),
        search_headers,
        Vec::new(),
    )?;
    let responses = guard.send(&reviewed_hashtag_search).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("exclude-unreviewed hashtag search probe: {error}"))?;
    for (side, response) in [("Mastodon", &responses.mastodon), ("Rust", &responses.rust)] {
        let body: Value = serde_json::from_slice(&response.body)?;
        let names = body["hashtags"]
            .as_array()
            .ok_or_else(|| format!("{side} hashtag search omitted the hashtags array"))?
            .iter()
            .filter_map(|tag| tag["name"].as_str())
            .collect::<Vec<_>>();
        if names != ["ReviewProbe", "ReviewProbeReviewed"] {
            return Err(
                format!("{side} returned unexpected reviewed hashtag search: {names:?}").into(),
            );
        }
    }
    for (label, account_id, token) in [
        ("local account anonymous", 116_844_606_259_201_001_i64, None),
        (
            "local account authenticated",
            116_844_606_259_201_001,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        ("remote account anonymous", 116_844_606_259_202_001, None),
        (
            "remote account authenticated",
            116_844_606_259_202_001,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        ("disabled local account", 116_844_606_259_201_003, None),
        ("missing-2fa local account", 116_844_606_259_201_002, None),
        ("pending local account denial", -321, None),
        ("unconfirmed local account denial", -322, None),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            format!("/api/v1/accounts/{account_id}"),
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, account) in [
        ("lookup local", "alice"),
        (
            "lookup qualified local",
            "Alice@fixture-v4-6-5.rustodon.invalid",
        ),
        ("lookup remote", "BOB@REMOTE.FIXTURE.INVALID"),
        ("lookup missing", "missing@remote.fixture.invalid"),
    ] {
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/accounts/lookup",
            Some(format!(
                "acct={}",
                url::form_urlencoded::byte_serialize(account.as_bytes()).collect::<String>()
            )),
            stable_request_headers(),
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, query, following, token) in [
        (
            "account search missing query",
            None,
            false,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "account search blank query",
            Some(""),
            false,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "account search local partial",
            Some("alice"),
            false,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "account search remote domain",
            Some("remote"),
            false,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "account search exact remote",
            Some("BOB@REMOTE.FIXTURE.INVALID"),
            false,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "account search following included",
            Some("bob"),
            true,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "account search following excluded",
            Some("carol"),
            true,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        ("account search anonymous", Some("alice"), false, None),
        (
            "account search application-only",
            Some("alice"),
            false,
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        (
            "account search wrong scope",
            Some("alice"),
            false,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let query = query.map(|query| {
            format!(
                "q={}",
                url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
            )
        });
        let query = match (query, following) {
            (Some(query), true) => Some(format!("{query}&following=true")),
            (Some(query), false) => Some(query),
            (None, true) => Some("following=true".to_owned()),
            (None, false) => None,
        };
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/accounts/search",
            query,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    let mut headers = stable_request_headers();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/accounts/search/",
        Some("q=alice&limit=1&offset=1".to_owned()),
        headers,
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, CACHE_CONTROL, VARY],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("account search pagination: {error}"))?;
    for (label, query) in [
        (
            "account search invalid offset",
            "q=alice&limit=1&offset=garbage",
        ),
        (
            "account search negative offset",
            "q=alice&limit=1&offset=-1",
        ),
        (
            "account search malformed remote resolve",
            "q=alice%40remote.fixture.invalid%2F&resolve=true",
        ),
        (
            "account search exact self following",
            "q=alice%40fixture-v4-6-5.rustodon.invalid&following=true&limit=1",
        ),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/accounts/search",
            Some(query.to_owned()),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, account_id, query, token) in [
        (
            "local account statuses anonymous",
            116_844_606_259_201_001_i64,
            None,
            None,
        ),
        (
            "local account statuses owner",
            116_844_606_259_201_001,
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "local account statuses unrelated",
            116_844_606_259_201_001,
            None,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
        ),
        (
            "local account statuses exclude direct",
            116_844_606_259_201_001,
            Some("exclude_direct=true"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "local account statuses media",
            116_844_606_259_201_001,
            Some("only_media=true"),
            None,
        ),
        (
            "local account statuses tagged",
            116_844_606_259_201_001,
            Some("tagged=FixtureTag"),
            None,
        ),
        (
            "local account statuses normalized tag",
            116_844_606_259_201_001,
            Some(
                "tagged=%EF%BC%A6%EF%BD%89%EF%BD%98%EF%BD%94%EF%BD%95%EF%BD%92%EF%BD%85%EF%BC%B4%EF%BD%81%EF%BD%87",
            ),
            None,
        ),
        (
            "local account statuses blank tag",
            116_844_606_259_201_001,
            Some("tagged="),
            None,
        ),
        (
            "local account statuses whitespace tag",
            116_844_606_259_201_001,
            Some("tagged=+++"),
            None,
        ),
        (
            "local account statuses array tag",
            116_844_606_259_201_001,
            Some("tagged%5B%5D=fixturetag"),
            None,
        ),
        (
            "local account statuses hash tag",
            116_844_606_259_201_001,
            Some("tagged%5Bx%5D=fixturetag"),
            None,
        ),
        (
            "local account statuses min cursor",
            116_844_606_259_201_001,
            Some("min_id=116844842188805001&limit=2"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "local account statuses first page",
            116_844_606_259_201_001,
            Some("limit=1"),
            None,
        ),
        (
            "local account statuses max cursor",
            116_844_606_259_201_001,
            Some("max_id=116844846120965002&limit=2"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "local account statuses since cursor",
            116_844_606_259_201_001,
            Some("since_id=116844842188805001&limit=2"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "local account statuses exclude replies",
            116_844_606_259_201_001,
            Some("exclude_replies=true"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "local account pinned statuses",
            116_844_606_259_201_001,
            Some("pinned=true"),
            None,
        ),
        (
            "local account pinned first page",
            116_844_606_259_201_001,
            Some("pinned=true&limit=1"),
            None,
        ),
        (
            "local account pinned second page",
            116_844_606_259_201_001,
            Some("pinned=true&limit=1&max_id=116844842188805001"),
            None,
        ),
        (
            "suspended account statuses",
            116_844_606_259_202_003,
            None,
            None,
        ),
        (
            "suspended account statuses overflow cursor",
            116_844_606_259_202_003,
            Some("max_id=9223372036854775808"),
            None,
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            format!("/api/v1/accounts/{account_id}/statuses"),
            query.map(str::to_owned),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, LINK],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, query, token) in [
        (
            "follow requests authenticated",
            "/api/v1/follow_requests",
            None,
            Some("fixture-bearer-follow-v4-6-5"),
        ),
        (
            "follow requests trailing paginated",
            "/api/v1/follow_requests/",
            Some("limit=1"),
            Some("fixture-bearer-follow-v4-6-5"),
        ),
        (
            "follow requests max cursor",
            "/api/v1/follow_requests",
            Some("max_id=8004&limit=1"),
            Some("fixture-bearer-follow-v4-6-5"),
        ),
        (
            "follow requests since cursor",
            "/api/v1/follow_requests",
            Some("since_id=8002"),
            Some("fixture-bearer-follow-v4-6-5"),
        ),
        (
            "follow requests missing token",
            "/api/v1/follow_requests",
            None,
            None,
        ),
        (
            "follow requests wrong scope",
            "/api/v1/follow_requests",
            None,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "follow requests other owner",
            "/api/v1/follow_requests",
            None,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            path,
            query.map(str::to_owned),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "preferences authenticated",
            "/api/v1/preferences",
            Some("fixture-bearer-read-accounts-v4-6-5"),
        ),
        (
            "preferences trailing slash",
            "/api/v1/preferences/",
            Some("fixture-bearer-read-accounts-v4-6-5"),
        ),
        ("preferences missing token", "/api/v1/preferences", None),
        (
            "preferences wrong scope",
            "/api/v1/preferences",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "account collections anonymous",
            "/api/v1/accounts/116844606259202001/collections",
            None,
        ),
        (
            "account collections authenticated",
            "/api/v1/accounts/116844606259202001/collections/?limit=1",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "account in collections anonymous",
            "/api/v1/accounts/116844606259201001/in_collections",
            None,
        ),
        (
            "account in collections authenticated",
            "/api/v1/accounts/116844606259201001/in_collections?offset=1",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "account in collections other owner",
            "/api/v1/accounts/116844606259202001/in_collections",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "account collections missing account",
            "/api/v1/accounts/999999/collections",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "account collections wrong scope",
            "/api/v1/accounts/116844606259202001/collections",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let mut path_parts = path.splitn(2, '?');
        let request = RequestSpec::new(
            Method::GET,
            path_parts
                .next()
                .expect("account collection path is non-empty"),
            path_parts.next().map(str::to_owned),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE, LINK],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "list show broad scope",
            "/api/v1/lists/9001",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "list show granular scope",
            "/api/v1/lists/9001/",
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "list show other owner",
            "/api/v1/lists/9001",
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
        ),
        (
            "list show missing",
            "/api/v1/lists/999999",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        ("list show missing token", "/api/v1/lists/9001", None),
        (
            "list show wrong scope",
            "/api/v1/lists/9001",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "list accounts broad scope",
            "/api/v1/lists/9001/accounts",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "list accounts granular scope",
            "/api/v1/lists/9001/accounts/",
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "list accounts first page",
            "/api/v1/lists/9001/accounts?limit=1",
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "list accounts max cursor",
            "/api/v1/lists/9001/accounts?limit=1&max_id=116844606259202001",
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "list accounts since cursor",
            "/api/v1/lists/9001/accounts?limit=1&since_id=-332",
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "list accounts unlimited",
            "/api/v1/lists/9001/accounts?limit=0",
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "list accounts other owner",
            "/api/v1/lists/9001/accounts",
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
        ),
        (
            "list accounts missing token",
            "/api/v1/lists/9001/accounts",
            None,
        ),
        (
            "list accounts wrong scope",
            "/api/v1/lists/9001/accounts",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let mut path_parts = path.splitn(2, '?');
        let request = RequestSpec::new(
            Method::GET,
            path_parts.next().expect("list accounts path is non-empty"),
            path_parts.next().map(str::to_owned),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE, LINK],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "account lists owner",
            "/api/v1/accounts/116844606259201001/lists",
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "account lists member",
            "/api/v1/accounts/116844606259202001/lists/",
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "account lists other owner",
            "/api/v1/accounts/116844606259202001/lists",
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
        ),
        (
            "account lists missing account",
            "/api/v1/accounts/999999/lists",
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "account lists missing token",
            "/api/v1/accounts/116844606259202001/lists",
            None,
        ),
        (
            "account lists wrong scope",
            "/api/v1/accounts/116844606259202001/lists",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "followers anonymous",
            "/api/v1/accounts/116844606259201001/followers",
            None,
        ),
        (
            "followers authenticated",
            "/api/v1/accounts/116844606259201001/followers",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "following anonymous",
            "/api/v1/accounts/116844606259201001/following",
            None,
        ),
        (
            "following authenticated",
            "/api/v1/accounts/116844606259201001/following",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "suspended followers",
            "/api/v1/accounts/116844606259202003/followers",
            None,
        ),
        (
            "suspended followers overflow cursor",
            "/api/v1/accounts/116844606259202003/followers?max_id=9223372036854775808",
            None,
        ),
        (
            "followers first page",
            "/api/v1/accounts/116844606259201001/followers?limit=1",
            None,
        ),
        (
            "followers max cursor",
            "/api/v1/accounts/116844606259201001/followers?max_id=8006&limit=1",
            None,
        ),
        (
            "followers since cursor",
            "/api/v1/accounts/116844606259201001/followers?since_id=8002&limit=1",
            None,
        ),
        (
            "following first page",
            "/api/v1/accounts/116844606259201001/following?limit=1",
            None,
        ),
        (
            "following max cursor",
            "/api/v1/accounts/116844606259201001/following?max_id=8005&limit=1",
            None,
        ),
        (
            "following since cursor",
            "/api/v1/accounts/116844606259201001/following?since_id=8001&limit=1",
            None,
        ),
        (
            "author-blocked followers overflow cursor",
            "/api/v1/accounts/116844606259202002/followers?max_id=9223372036854775808",
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
        (
            "author-blocked following overflow cursor",
            "/api/v1/accounts/116844606259202002/following?max_id=9223372036854775808",
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let mut url = path.splitn(2, '?');
        let request = RequestSpec::new(
            Method::GET,
            url.next().expect("follow path is non-empty"),
            url.next().map(str::to_owned),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, LINK],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "lists broad scope",
            "/api/v1/lists",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "lists granular scope",
            "/api/v1/lists",
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "lists other owner",
            "/api/v1/lists/",
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
        ),
        ("lists missing token", "/api/v1/lists", None),
        (
            "lists wrong scope",
            "/api/v1/lists",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "lists application-only",
            "/api/v1/lists",
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "featured tags broad scope",
            "/api/v1/featured_tags",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "featured tags granular scope",
            "/api/v1/featured_tags",
            Some("fixture-bearer-read-accounts-v4-6-5"),
        ),
        (
            "featured tags other owner",
            "/api/v1/featured_tags/",
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
        ),
        ("featured tags missing token", "/api/v1/featured_tags", None),
        (
            "featured tags wrong scope",
            "/api/v1/featured_tags",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "featured tags application-only",
            "/api/v1/featured_tags",
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, query, token) in [
        (
            "followed tags broad scope",
            "/api/v1/followed_tags/",
            Some("limit=1"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "followed tags legacy follow scope",
            "/api/v1/followed_tags",
            None,
            Some("fixture-bearer-follow-v4-6-5"),
        ),
        (
            "followed tags missing token",
            "/api/v1/followed_tags",
            None,
            None,
        ),
        (
            "followed tags wrong scope",
            "/api/v1/followed_tags",
            None,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            path,
            query.map(str::to_owned),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "featured tag suggestions broad scope",
            "/api/v1/featured_tags/suggestions",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "featured tag suggestions granular trailing",
            "/api/v1/featured_tags/suggestions/",
            Some("fixture-bearer-read-accounts-v4-6-5"),
        ),
        (
            "featured tag suggestions missing token",
            "/api/v1/featured_tags/suggestions",
            None,
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        ("custom emojis anonymous", "/api/v1/custom_emojis", None),
        (
            "custom emojis authenticated trailing",
            "/api/v1/custom_emojis/",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "account featured tags anonymous",
            "/api/v1/accounts/116844606259201001/featured_tags",
            None,
        ),
        (
            "account featured tags trailing wrong scope",
            "/api/v1/accounts/116844606259201001/featured_tags/",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "account featured tags suspended",
            "/api/v1/accounts/116844606259202003/featured_tags",
            None,
        ),
        (
            "account featured tags unavailable",
            "/api/v1/accounts/-321/featured_tags",
            None,
        ),
        (
            "account featured tags missing",
            "/api/v1/accounts/not-an-id/featured_tags",
            None,
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, token) in [
        (
            "v1 notification unread missing token",
            "/api/v1/notifications/unread_count",
            None,
        ),
        (
            "v1 notification unread wrong scope",
            "/api/v1/notifications/unread_count",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "v1 notification show missing token",
            "/api/v1/notifications/10003",
            None,
        ),
        (
            "v1 notification show wrong scope",
            "/api/v1/notifications/10003",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "v2 notification unread missing token",
            "/api/v2/notifications/unread_count",
            None,
        ),
        (
            "v2 notification unread wrong scope",
            "/api/v2/notifications/unread_count",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "v2 notification show missing token",
            "/api/v2/notifications/reblog-116844842188805001-495255",
            None,
        ),
        (
            "v2 notification show wrong scope",
            "/api/v2/notifications/reblog-116844842188805001-495255",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, query, token) in [
        (
            "notification requests broad scope",
            "/api/v1/notifications/requests",
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "notification requests granular trailing",
            "/api/v1/notifications/requests/",
            Some("limit=1"),
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "notification requests max cursor",
            "/api/v1/notifications/requests",
            Some("limit=1&max_id=116846261698560601"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "notification requests min cursor",
            "/api/v1/notifications/requests",
            Some("limit=1&min_id=-96"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "notification requests missing token",
            "/api/v1/notifications/requests",
            None,
            None,
        ),
        (
            "notification requests wrong scope",
            "/api/v1/notifications/requests",
            None,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "notification request show",
            "/api/v1/notifications/requests/116846261698560601",
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "notification request show trailing",
            "/api/v1/notifications/requests/-96/",
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "notification request show missing",
            "/api/v1/notifications/requests/999999",
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "notification requests merged",
            "/api/v1/notifications/requests/merged",
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "notification requests merged trailing",
            "/api/v1/notifications/requests/merged/",
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "notification requests merged missing token",
            "/api/v1/notifications/requests/merged",
            None,
            None,
        ),
        (
            "notification requests merged wrong scope",
            "/api/v1/notifications/requests/merged",
            None,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "v1 notification policy",
            "/api/v1/notifications/policy",
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "v1 notification policy trailing",
            "/api/v1/notifications/policy/",
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "v1 notification policy missing token",
            "/api/v1/notifications/policy",
            None,
            None,
        ),
        (
            "v1 notification policy wrong scope",
            "/api/v1/notifications/policy",
            None,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "v2 notification policy",
            "/api/v2/notifications/policy",
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "v2 notification policy trailing",
            "/api/v2/notifications/policy/",
            None,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "v2 notification policy missing token",
            "/api/v2/notifications/policy",
            None,
            None,
        ),
        (
            "v2 notification policy wrong scope",
            "/api/v2/notifications/policy",
            None,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "status quotes",
            "/api/v1/statuses/116845321912325301/quotes",
            Some("limit=2"),
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
        (
            "status quotes trailing cursor",
            "/api/v1/statuses/116845321912325301/quotes/",
            Some("limit=1&since_id=-94"),
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
        (
            "status quotes missing token",
            "/api/v1/statuses/116845321912325301/quotes",
            Some("limit=2"),
            None,
        ),
        (
            "status quotes wrong scope",
            "/api/v1/statuses/116845321912325301/quotes",
            None,
            Some("fixture-bearer-read-accounts-v4-6-5"),
        ),
        (
            "status quotes application-only",
            "/api/v1/statuses/116845321912325301/quotes",
            Some("limit=1"),
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        (
            "public timeline",
            "/api/v1/timelines/public",
            Some("limit=2"),
            None,
        ),
        (
            "public application-only denial",
            "/api/v1/timelines/public",
            Some("limit=2"),
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        (
            "public local timeline",
            "/api/v1/timelines/public",
            Some("local=true&limit=2"),
            None,
        ),
        (
            "public edited-out media timeline",
            "/api/v1/timelines/public",
            Some("local=on&only_media=TRUE&limit=2"),
            None,
        ),
        (
            "public timeline authenticated",
            "/api/v1/timelines/public",
            Some("max_id=0&limit=2"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "public timeline blank cursors",
            "/api/v1/timelines/public",
            Some("max_id=&min_id=&since_id=&limit=2"),
            None,
        ),
        (
            "public timeline whitespace cursors",
            "/api/v1/timelines/public",
            Some("max_id=+++&min_id=+++&since_id=+++&limit=2"),
            None,
        ),
        (
            "tag timeline",
            "/api/v1/timelines/tag/FixtureTag",
            Some("local=true&max_id=0&limit=2"),
            None,
        ),
        (
            "tag timeline authenticated",
            "/api/v1/timelines/tag/FixtureTag",
            Some("max_id=0&limit=40"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "tag edited-out media timeline",
            "/api/v1/timelines/tag/FixtureTag",
            Some("local=true&only_media=true&limit=2"),
            None,
        ),
        (
            "tag combination timeline",
            "/api/v1/timelines/tag/fixturetag",
            Some("max_id=0&any%5B%5D=anytag&all%5B%5D=alltag&none%5B%5D=nonetag"),
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
        (
            "missing base tag timeline",
            "/api/v1/timelines/tag/missingtag",
            Some("local=true&any%5B%5D=fixturetag"),
            None,
        ),
        (
            "normalized tag timeline",
            "/api/v1/timelines/tag/%EF%BC%A6%EF%BD%89%EF%BD%98%EF%BD%94%EF%BD%95%EF%BD%92%EF%BD%85%EF%BC%B4%EF%BD%81%EF%BD%87",
            Some("local=true&max_id=0&limit=2"),
            None,
        ),
        (
            "raw-limited normalized tag timeline",
            "/api/v1/timelines/tag/fixturetag",
            Some(
                "max_id=0&any%5B%5D=%EF%BC%A6%EF%BD%89%EF%BD%98%EF%BD%94%EF%BD%95%EF%BD%92%EF%BD%85%EF%BC%B4%EF%BD%81%EF%BD%87&any%5B%5D=missingone&any%5B%5D=missingtwo&any%5B%5D=anytag",
            ),
            None,
        ),
        (
            "remote topic ordinary-user denial",
            "/api/v1/timelines/tag/FixtureTag",
            Some("remote=true&max_id=0&limit=2"),
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "remote topic moderator access",
            "/api/v1/timelines/tag/FixtureTag",
            Some("remote=true&max_id=0&limit=2"),
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
        (
            "home timeline",
            "/api/v1/timelines/home",
            Some("max_id=0"),
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "list timeline",
            "/api/v1/timelines/list/9001",
            Some("max_id=0"),
            Some("fixture-bearer-read-lists-v4-6-5"),
        ),
        (
            "list ownership denial",
            "/api/v1/timelines/list/9001",
            None,
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
        (
            "favourites",
            "/api/v1/favourites",
            Some("limit=1"),
            Some("fixture-bearer-read-favourites-v4-6-5"),
        ),
        (
            "bookmarks",
            "/api/v1/bookmarks",
            Some("limit=1"),
            Some("fixture-bearer-read-bookmarks-v4-6-5"),
        ),
        (
            "blocks",
            "/api/v1/blocks",
            Some("limit=1"),
            Some("fixture-bearer-read-blocks-v4-6-5"),
        ),
        (
            "mutes",
            "/api/v1/mutes",
            Some("limit=1"),
            Some("fixture-bearer-read-mutes-v4-6-5"),
        ),
        (
            "blocks legacy follow scope",
            "/api/v1/blocks",
            Some("limit=1"),
            Some("fixture-bearer-follow-v4-6-5"),
        ),
        (
            "mutes legacy follow scope",
            "/api/v1/mutes",
            Some("limit=1"),
            Some("fixture-bearer-follow-v4-6-5"),
        ),
        (
            "favourites wrong scope",
            "/api/v1/favourites",
            None,
            Some("fixture-bearer-read-bookmarks-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            path,
            query.map(str::to_owned),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, LINK],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    let mut headers = stable_request_headers();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/accounts/verify_credentials",
        None,
        headers,
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, CACHE_CONTROL, VARY],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("credential account: {error}"))?;
    for (label, account_id) in [
        ("relationship following", 116_844_606_259_202_001_i64),
        ("relationship blocked", 116_844_606_259_202_002_i64),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/accounts/relationships",
            Some(format!("id%5B%5D={account_id}")),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, query) in [
        (
            "relationship suspended omitted",
            "id%5B%5D=116844606259202003",
        ),
        (
            "relationship suspended included",
            "id%5B%5D=116844606259202003&with_suspended=true",
        ),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/accounts/relationships",
            Some(query.to_owned()),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, status_id, token) in [
        ("public status anonymous", 116_844_842_188_805_001_i64, None),
        (
            "public status authenticated",
            116_844_842_188_805_001,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        ("historical poll anonymous", 111_680_579_174_405_102, None),
        (
            "historical poll authenticated",
            111_680_579_174_405_102,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "explicit filter match authenticated",
            116_845_105_643_525_105,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "pending private quote anonymous",
            116_845_105_643_525_105,
            None,
        ),
        (
            "reblog quote target anonymous",
            116_845_101_711_365_104,
            None,
        ),
        ("boost anonymous", 116_845_321_912_325_301, None),
        (
            "boost authenticated",
            116_845_321_912_325_301,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        ("remote mention anonymous", 116_845_078_118_405_101, None),
        (
            "remote mention authenticated",
            116_845_078_118_405_101,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "private status authenticated",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "private status anonymous denial",
            116_844_850_053_125_003,
            None,
        ),
        (
            "private status unrelated denial",
            116_844_850_053_125_003,
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
        (
            "direct status authenticated",
            116_844_853_985_285_004,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "direct status anonymous denial",
            116_844_853_985_285_004,
            None,
        ),
        (
            "direct status unrelated denial",
            116_844_853_985_285_004,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
        ),
        (
            "limited status authenticated",
            116_844_857_917_445_005,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "limited status anonymous denial",
            116_844_857_917_445_005,
            None,
        ),
        (
            "limited status unrelated denial",
            116_844_857_917_445_005,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
        ),
        (
            "limited status silent mention",
            116_844_857_917_445_005,
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
        (
            "former follower silent mention",
            -311,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "former follower private denial",
            -312,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "author blocked public denial",
            -313,
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
        ("suspended author status denial", -310, None),
        (
            "soft-deleted status anonymous denial",
            116_846_257_766_400_501,
            None,
        ),
        (
            "soft-deleted status owner denial",
            116_846_257_766_400_501,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "unlisted deleted quote anonymous",
            116_844_846_120_965_002,
            None,
        ),
        ("local quote anonymous", 116_845_314_048_005_201, None),
        (
            "local quote authenticated",
            116_845_314_048_005_201,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        ("remote quote anonymous", 116_845_317_980_165_202, None),
        (
            "remote quote authenticated",
            116_845_317_980_165_202,
            Some("fixture-bearer-token-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            format!("/api/v1/statuses/{status_id}"),
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, status_id, token) in [
        (
            "status context anonymous",
            116_844_846_120_965_002_i64,
            None,
        ),
        (
            "status context owner",
            116_844_846_120_965_002,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "private status context anonymous denial",
            116_844_850_053_125_003,
            None,
        ),
        (
            "private status context owner ordered ancestors",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "public context owner domain filtering",
            116_844_842_188_805_001,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "public context author-block filtering",
            116_844_842_188_805_001,
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            format!("/api/v1/statuses/{status_id}/context"),
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, token) in [
        ("collection anonymous", None),
        (
            "collection authenticated",
            Some("fixture-bearer-token-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/collections/116845549977608801",
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, status_id, token) in [
        ("source missing token", 116_844_842_188_805_001_i64, None),
        (
            "source broad public",
            116_844_842_188_805_001,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "source granular public",
            116_844_842_188_805_001,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "source application public",
            116_844_842_188_805_001,
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        (
            "source wrong scope",
            116_844_842_188_805_001,
            Some("fixture-bearer-read-accounts-v4-6-5"),
        ),
        (
            "source private owner",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "source private application",
            116_844_850_053_125_003,
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        (
            "source deleted",
            116_846_257_766_400_501,
            Some("fixture-bearer-token-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            format!("/api/v1/statuses/{status_id}/source"),
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, status_id, token) in [
        (
            "history anonymous public",
            116_844_842_188_805_001_i64,
            None,
        ),
        (
            "history broad public",
            116_844_842_188_805_001,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "history granular public",
            116_844_842_188_805_001,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "history application public",
            116_844_842_188_805_001,
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        (
            "history wrong scope",
            116_844_842_188_805_001,
            Some("fixture-bearer-read-accounts-v4-6-5"),
        ),
        (
            "history private owner",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "history private application",
            116_844_850_053_125_003,
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        ("history unedited", 116_844_846_120_965_002, None),
        ("history poll fallback", 111_680_579_174_405_102, None),
        (
            "history deleted",
            116_846_257_766_400_501,
            Some("fixture-bearer-token-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            format!("/api/v1/statuses/{status_id}/history"),
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, status_id, token) in [
        ("favourited by anonymous", 116_844_842_188_805_001_i64, None),
        (
            "favourited by application",
            116_844_842_188_805_001,
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        (
            "favourited by owner exclusions",
            116_844_842_188_805_001,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "favourited by granular account",
            116_844_842_188_805_001,
            Some("fixture-bearer-read-accounts-v4-6-5"),
        ),
        (
            "favourited by moderator block filtering",
            116_844_842_188_805_001,
            Some("fixture-bearer-api-moderator-v4-6-5"),
        ),
        (
            "favourited by wrong scope",
            116_844_842_188_805_001,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "favourited by private anonymous",
            116_844_850_053_125_003,
            None,
        ),
        (
            "favourited by private owner",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "favourited by deleted",
            116_846_257_766_400_501,
            Some("fixture-bearer-token-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            format!("/api/v1/statuses/{status_id}/favourited_by"),
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    let mut headers = stable_request_headers();
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/statuses/116844842188805001/favourited_by/",
        Some("limit=1".to_owned()),
        headers.clone(),
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, LINK],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("favourited by pagination: {error}"))?;
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-api-moderator-v4-6-5"),
    );
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/statuses/116844842188805001/favourited_by",
        Some("limit=1&max_id=8103".to_owned()),
        headers,
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, LINK],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("favourited by cursor: {error}"))?;
    for (label, status_id, token) in [
        ("reblogged by anonymous", 116_844_842_188_805_001_i64, None),
        (
            "reblogged by application",
            116_844_842_188_805_001,
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
        (
            "reblogged by wrong scope",
            116_844_842_188_805_001,
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "reblogged by private anonymous",
            116_844_850_053_125_003,
            None,
        ),
        (
            "reblogged by private owner",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "reblogged by deleted",
            116_846_257_766_400_501,
            Some("fixture-bearer-token-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            format!("/api/v1/statuses/{status_id}/reblogged_by"),
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/statuses/116844842188805001/reblogged_by/",
        Some("limit=1&max_id=116845321912325301".to_owned()),
        stable_request_headers(),
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, LINK],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("reblogged by pagination: {error}"))?;
    for (label, path, token) in [
        (
            "filter definitions",
            "/api/v2/filters",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "filter definitions trailing slash",
            "/api/v2/filters/",
            Some("fixture-bearer-token-v4-6-5"),
        ),
        (
            "filter definitions other owner",
            "/api/v2/filters",
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
        ),
        ("filter definitions missing token", "/api/v2/filters", None),
        (
            "filter definitions wrong scope",
            "/api/v2/filters",
            Some("fixture-bearer-read-statuses-v4-6-5"),
        ),
        (
            "filter definitions application-only",
            "/api/v2/filters",
            Some("fixture-bearer-application-only-v4-6-5"),
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(Method::GET, path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, query, token) in [
        (
            "markers array",
            "/api/v1/markers",
            Some("timeline%5B%5D=home&timeline%5B%5D=notifications"),
            "fixture-bearer-token-v4-6-5",
        ),
        (
            "markers scalar",
            "/api/v1/markers",
            Some("timeline=home"),
            "fixture-bearer-token-v4-6-5",
        ),
        (
            "markers unknown timeline",
            "/api/v1/markers",
            Some("timeline=unknown"),
            "fixture-bearer-token-v4-6-5",
        ),
        (
            "markers missing timeline",
            "/api/v1/markers/",
            None,
            "fixture-bearer-token-v4-6-5",
        ),
        (
            "markers other owner",
            "/api/v1/markers",
            Some("timeline=home"),
            "fixture-bearer-matrix-viewer-v4-6-5",
        ),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
        let request = RequestSpec::new(
            Method::GET,
            path,
            query.map(str::to_owned),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    let mut headers = stable_request_headers();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/notifications",
        None,
        headers,
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("notification envelope: {error}"))?;
    for notification_type in [
        "mention",
        "status",
        "reblog",
        "follow",
        "follow_request",
        "favourite",
        "poll",
        "update",
        "severed_relationships",
        "moderation_warning",
        "annual_report",
        "quote",
        "quoted_update",
        "added_to_collection",
        "collection_update",
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let query = format!(
            "types%5B%5D={}",
            url::form_urlencoded::byte_serialize(notification_type.as_bytes()).collect::<String>()
        );
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/notifications",
            Some(query),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{notification_type} notification: {error}"))?;
    }
    for notification_type in ["admin.sign_up", "admin.report"] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-api-moderator-v4-6-5"),
        );
        let query = format!(
            "types%5B%5D={}",
            url::form_urlencoded::byte_serialize(notification_type.as_bytes()).collect::<String>()
        );
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/notifications",
            Some(query),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{notification_type} notification: {error}"))?;
    }
    for (label, path, query) in [
        (
            "notification type filter before limit",
            "/api/v1/notifications",
            Some("types%5B%5D=reblog".to_owned()),
        ),
        (
            "notification distinct-group pagination",
            "/api/v2/notifications",
            None,
        ),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-api-moderator-v4-6-5"),
        );
        let request = RequestSpec::new(Method::GET, path, query, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    let all_known_types = [
        "mention",
        "status",
        "reblog",
        "follow",
        "follow_request",
        "favourite",
        "poll",
        "update",
        "severed_relationships",
        "moderation_warning",
        "annual_report",
        "admin.sign_up",
        "admin.report",
        "quote",
        "quoted_update",
        "added_to_collection",
        "collection_update",
    ]
    .into_iter()
    .map(|kind| format!("types%5B%5D={kind}"))
    .collect::<Vec<_>>()
    .join("&");
    for (label, query) in [
        (
            "unknown notification type intersection",
            "types%5B%5D=future_event&include_filtered=true".to_owned(),
        ),
        ("all known notification type intersection", all_known_types),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/notifications",
            Some(query),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (notification_type, token) in [
        ("mention", "fixture-bearer-token-v4-6-5"),
        ("status", "fixture-bearer-token-v4-6-5"),
        ("reblog", "fixture-bearer-token-v4-6-5"),
        ("follow", "fixture-bearer-token-v4-6-5"),
        ("follow_request", "fixture-bearer-token-v4-6-5"),
        ("favourite", "fixture-bearer-token-v4-6-5"),
        ("poll", "fixture-bearer-token-v4-6-5"),
        ("update", "fixture-bearer-token-v4-6-5"),
        ("severed_relationships", "fixture-bearer-token-v4-6-5"),
        ("moderation_warning", "fixture-bearer-token-v4-6-5"),
        ("annual_report", "fixture-bearer-token-v4-6-5"),
        ("quote", "fixture-bearer-token-v4-6-5"),
        ("quoted_update", "fixture-bearer-token-v4-6-5"),
        ("added_to_collection", "fixture-bearer-token-v4-6-5"),
        ("collection_update", "fixture-bearer-token-v4-6-5"),
        ("admin.sign_up", "fixture-bearer-api-moderator-v4-6-5"),
        ("admin.report", "fixture-bearer-api-moderator-v4-6-5"),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
        let query = format!(
            "types%5B%5D={}",
            url::form_urlencoded::byte_serialize(notification_type.as_bytes()).collect::<String>()
        );
        let request = RequestSpec::new(
            Method::GET,
            "/api/v2/notifications",
            Some(query),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("grouped {notification_type} notification: {error}"))?;
    }
    for (label, query) in [
        ("grouped notification envelope", None),
        (
            "grouped partial-account envelope",
            Some("expand_accounts=partial_avatars".to_owned()),
        ),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let request = RequestSpec::new(
            Method::GET,
            "/api/v2/notifications",
            query,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (notification_type, token) in [
        ("severed_relationships", "fixture-bearer-token-v4-6-5"),
        ("moderation_warning", "fixture-bearer-token-v4-6-5"),
        ("added_to_collection", "fixture-bearer-token-v4-6-5"),
        ("admin.sign_up", "fixture-bearer-api-moderator-v4-6-5"),
        ("admin.report", "fixture-bearer-api-moderator-v4-6-5"),
    ] {
        for path in ["/api/v1/notifications", "/api/v2/notifications"] {
            let mut headers = stable_request_headers();
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
            let encoded_type = url::form_urlencoded::byte_serialize(notification_type.as_bytes())
                .collect::<String>();
            let request = RequestSpec::new(
                Method::GET,
                path,
                Some(format!(
                    "types%5B%5D={encoded_type}&supported_types%5B%5D=mention"
                )),
                headers,
                Vec::new(),
            )?;
            let responses = guard.send(&request).await?;
            compare_responses(
                &responses.mastodon,
                &responses.rust,
                &[CONTENT_TYPE],
                &[],
                DEFAULT_MISMATCH_LIMIT,
            )
            .map_err(|error| format!("fallback {path} {notification_type}: {error}"))?;
        }
    }
    for (label, path, query) in [
        (
            "v1 notification limit pagination",
            "/api/v1/notifications",
            "limit=1",
        ),
        (
            "v1 notification max cursor",
            "/api/v1/notifications",
            "max_id=10010&limit=3",
        ),
        (
            "v1 notification min cursor",
            "/api/v1/notifications",
            "min_id=10003&max_id=10010&limit=3",
        ),
        (
            "v1 notification since cursor",
            "/api/v1/notifications",
            "since_id=10003&limit=3",
        ),
        (
            "v1 notification exclusion filter",
            "/api/v1/notifications",
            "exclude_types%5B%5D=favourite&exclude_types%5B%5D=reblog",
        ),
        (
            "v1 notification scalar type parameter",
            "/api/v1/notifications",
            "types=mention",
        ),
        (
            "v2 notification limit pagination",
            "/api/v2/notifications",
            "limit=1",
        ),
        (
            "v2 notification max cursor",
            "/api/v2/notifications",
            "max_id=10010&limit=3",
        ),
        (
            "v2 notification min cursor",
            "/api/v2/notifications",
            "min_id=10003&max_id=10010&limit=3",
        ),
        (
            "v2 notification grouped types",
            "/api/v2/notifications",
            "grouped_types%5B%5D=follow&limit=5",
        ),
        (
            "v2 notification scalar grouped type parameter",
            "/api/v2/notifications",
            "grouped_types=follow&limit=5",
        ),
        (
            "v2 notification blank scalar grouped type parameter",
            "/api/v2/notifications",
            "grouped_types=&limit=5",
        ),
        (
            "v2 notification unknown grouped type",
            "/api/v2/notifications",
            "grouped_types%5B%5D=future_event&limit=5",
        ),
        (
            "v2 notification exclusion filter",
            "/api/v2/notifications",
            "exclude_types%5B%5D=favourite&grouped_types%5B%5D=follow",
        ),
        (
            "v2 notification invalid expansion",
            "/api/v2/notifications",
            "expand_accounts=invalid",
        ),
        (
            "v1 notification scalar supported type",
            "/api/v1/notifications",
            "types%5B%5D=severed_relationships&supported_types=mention",
        ),
        (
            "v2 notification scalar supported type",
            "/api/v2/notifications",
            "types%5B%5D=severed_relationships&supported_types=mention",
        ),
        (
            "v1 notification trailing slash",
            "/api/v1/notifications/",
            "limit=1",
        ),
        (
            "v2 notification trailing slash",
            "/api/v2/notifications/",
            "limit=1",
        ),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let request = RequestSpec::new(
            Method::GET,
            path,
            Some(query.to_owned()),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, LINK],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    for (label, path, query) in [
        (
            "v1 notification unread count",
            "/api/v1/notifications/unread_count",
            None,
        ),
        (
            "v1 notification unread count filtered",
            "/api/v1/notifications/unread_count",
            Some("limit=2&exclude_types%5B%5D=mention"),
        ),
        (
            "v2 notification unread count",
            "/api/v2/notifications/unread_count",
            None,
        ),
        (
            "v2 notification unread count grouped types",
            "/api/v2/notifications/unread_count",
            Some("limit=2&grouped_types%5B%5D=follow"),
        ),
        (
            "v2 notification unread blank scalar grouped types",
            "/api/v2/notifications/unread_count",
            Some("grouped_types="),
        ),
        (
            "v1 notification unread ignores cursors",
            "/api/v1/notifications/unread_count",
            Some("max_id=999999999999999999999999"),
        ),
        ("v1 notification show", "/api/v1/notifications/10003", None),
        (
            "v2 notification show grouped",
            "/api/v2/notifications/reblog-116844842188805001-495255",
            None,
        ),
        (
            "v2 notification show ungrouped",
            "/api/v2/notifications/ungrouped-10025",
            None,
        ),
        (
            "v2 notification show legacy grouped lookup",
            "/api/v2/notifications/ungrouped-10003",
            None,
        ),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
        );
        let request = RequestSpec::new(
            Method::GET,
            path,
            query.map(str::to_owned),
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }
    guard.finish().await
}

#[allow(clippy::too_many_lines)]
async fn run_rest_protocol_contracts_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn std::error::Error>> {
    let guard = ReadOnlyGuard::begin(config, rust_url).await?;
    let mut cors_headers = stable_request_headers();
    cors_headers.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
    for (label, method, path, query, headers, relevant_headers) in [
        (
            "cross-origin instance",
            Method::GET,
            "/api/v2/instance",
            None,
            cors_headers.clone(),
            &[
                CONTENT_TYPE,
                CACHE_CONTROL,
                VARY,
                ACCESS_CONTROL_ALLOW_ORIGIN,
                ACCESS_CONTROL_ALLOW_METHODS,
                ACCESS_CONTROL_MAX_AGE,
                ACCESS_CONTROL_EXPOSE_HEADERS,
            ][..],
        ),
        (
            "trailing slash instance",
            Method::GET,
            "/api/v2/instance/",
            None,
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "trailing slash dynamic account",
            Method::GET,
            "/api/v1/accounts/116844606259201001/",
            None,
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "cross-origin account",
            Method::GET,
            "/api/v1/accounts/116844606259201001",
            None,
            cors_headers.clone(),
            &[
                CONTENT_TYPE,
                CACHE_CONTROL,
                VARY,
                ACCESS_CONTROL_ALLOW_ORIGIN,
                ACCESS_CONTROL_ALLOW_METHODS,
                ACCESS_CONTROL_MAX_AGE,
                ACCESS_CONTROL_EXPOSE_HEADERS,
            ][..],
        ),
        (
            "cross-origin account bearer",
            Method::GET,
            "/api/v1/accounts/116844606259201001",
            None,
            {
                let mut headers = cors_headers.clone();
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
                );
                headers
            },
            &[
                CONTENT_TYPE,
                CACHE_CONTROL,
                VARY,
                ACCESS_CONTROL_ALLOW_ORIGIN,
                ACCESS_CONTROL_ALLOW_METHODS,
                ACCESS_CONTROL_MAX_AGE,
                ACCESS_CONTROL_EXPOSE_HEADERS,
            ][..],
        ),
        (
            "cross-origin wrong scope",
            Method::GET,
            "/api/v1/favourites",
            None,
            {
                let mut headers = cors_headers.clone();
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_static("Bearer fixture-bearer-read-lists-v4-6-5"),
                );
                headers
            },
            &[
                CONTENT_TYPE,
                CACHE_CONTROL,
                VARY,
                WWW_AUTHENTICATE,
                ACCESS_CONTROL_ALLOW_ORIGIN,
                ACCESS_CONTROL_ALLOW_METHODS,
                ACCESS_CONTROL_MAX_AGE,
                ACCESS_CONTROL_EXPOSE_HEADERS,
            ][..],
        ),
        (
            "cross-origin paginated timeline",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&max_id=0&limit=1".to_owned()),
            cors_headers.clone(),
            &[
                CONTENT_TYPE,
                CACHE_CONTROL,
                VARY,
                LINK,
                ACCESS_CONTROL_ALLOW_ORIGIN,
                ACCESS_CONTROL_ALLOW_METHODS,
                ACCESS_CONTROL_MAX_AGE,
                ACCESS_CONTROL_EXPOSE_HEADERS,
            ][..],
        ),
        (
            "wrong method",
            Method::POST,
            "/api/v2/instance",
            None,
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "malformed account id",
            Method::GET,
            "/api/v1/accounts/not-an-id",
            None,
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "plus account id",
            Method::GET,
            "/api/v1/accounts/+116844606259201001",
            None,
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "plus account id pagination",
            Method::GET,
            "/api/v1/accounts/+116844606259201001/statuses",
            Some("limit=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "encoded account id pagination",
            Method::GET,
            "/api/v1/accounts/116844606259201001%3Fjunk/statuses",
            Some("limit=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "unreserved encoded account pagination",
            Method::GET,
            "/api/v1/accounts/%3116844606259201001/statuses",
            Some("limit=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "lowercase escaped account pagination",
            Method::GET,
            "/api/v1/accounts/116844606259201001%3fjunk/statuses",
            Some("limit=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "encoded plus account pagination",
            Method::GET,
            "/api/v1/accounts/%2b116844606259201001/statuses",
            Some("limit=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "encoded bracket account pagination",
            Method::GET,
            "/api/v1/accounts/116844606259201001%5Bx/statuses",
            Some("limit=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "status id trailing junk",
            Method::GET,
            "/api/v1/statuses/-311junk",
            None,
            {
                let mut headers = stable_request_headers();
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
                );
                headers
            },
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "overflow account id",
            Method::GET,
            "/api/v1/accounts/9223372036854775808",
            None,
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "non-ascii account id whitespace",
            Method::GET,
            "/api/v1/accounts/%C2%A0116844606259201001",
            None,
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "minimum signed status id",
            Method::GET,
            "/api/v1/statuses/-9223372036854775808",
            None,
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "malformed account id wrong scope",
            Method::GET,
            "/api/v1/accounts/not-an-id",
            None,
            {
                let mut headers = stable_request_headers();
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_static("Bearer fixture-bearer-insufficient-v4-6-5"),
                );
                headers
            },
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE][..],
        ),
        (
            "malformed list id unauthenticated",
            Method::GET,
            "/api/v1/timelines/list/not-an-id",
            None,
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, WWW_AUTHENTICATE][..],
        ),
        (
            "relationships invalid id coercion",
            Method::GET,
            "/api/v1/accounts/relationships",
            Some("id%5B%5D=invalid".to_owned()),
            {
                let mut headers = stable_request_headers();
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
                );
                headers
            },
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "relationships trailing array junk",
            Method::GET,
            "/api/v1/accounts/relationships",
            Some("id%5B%5Djunk=116844606259202001".to_owned()),
            {
                let mut headers = stable_request_headers();
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
                );
                headers
            },
            &[CONTENT_TYPE, VARY][..],
        ),
        (
            "relationships scalar replaces array",
            Method::GET,
            "/api/v1/accounts/relationships",
            Some("id%5B%5D=116844606259202001&id=116844606259201002".to_owned()),
            {
                let mut headers = stable_request_headers();
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
                );
                headers
            },
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "duplicate scalar pagination",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&max_id=0&limit=1&limit=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "positive cursor overflow",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&max_id=9223372036854775808".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "negative cursor overflow",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&min_id=-9223372036854775809".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "nonnumeric max cursor",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&max_id=invalid".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "sign-only min cursor",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&min_id=%2B".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "array cursor shape",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&max_id%5B%5D=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "scalar to array cursor collision",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&max_id=1&max_id%5B%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "cursor array to hash collision",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&max_id%5B%5D=1&max_id%5Bx%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, VARY][..],
        ),
        (
            "unmatched bracket cursor",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&max_id%5Bx=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "unmatched bracket limit",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&limit%5Bx=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, VARY][..],
        ),
        (
            "nested limit shape",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&limit%5B%5D=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, VARY][..],
        ),
        (
            "boolean array shape",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local%5B%5D=true&limit=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "root array collision",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&%5B%5D=1&%5B%5D%5Bx%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, VARY][..],
        ),
        (
            "double array mixed append",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5B%5D%5B%5D=1&a%5B%5D%5B%5D%5Bx%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "triple array collision",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5B%5D%5B%5D%5B%5D=1&a%5B%5D%5B%5D%5Bx%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, VARY][..],
        ),
        (
            "array then trailing junk collision",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5Bb%5D%5B%5D=1&a%5Bb%5Djunk=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, VARY][..],
        ),
        (
            "array trailing key preservation",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5B%5Dfoo=1&a%5B%5D%5Bjunk%5D%5B%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "named trailing key preservation",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5Bb%5Dfoo=1&a%5Bb%5D%5Bjunk%5D%5B%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "repeated trailing push merge",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5B%5Dfoo=1&a%5B%5Dbar=2&a%5B%5D%5Bfoo%5D%5B%5D=3".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, VARY][..],
        ),
        (
            "nested array replaced by scalar",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5Bb%5D%5B%5D=1&a%5Bb%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "nested hash replaced by scalar",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5Bb%5D%5Bc%5D=1&a%5Bb%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "empty root parameter",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&%5B%5D=ignored&limit=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "trailing key junk",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5Bb%5Djunk=x&limit=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "blank authorization caches anonymously",
            Method::GET,
            "/api/v1/accounts/116844606259201001",
            None,
            {
                let mut headers = stable_request_headers();
                headers.insert(AUTHORIZATION, HeaderValue::from_static("   "));
                headers
            },
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "tag scalar replaces array",
            Method::GET,
            "/api/v1/timelines/tag/fixturetag",
            Some("max_id=0&any%5B%5D=missingtag&any=anytag".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "tag hash filter is neutral",
            Method::GET,
            "/api/v1/timelines/tag/fixturetag",
            Some("local=true&max_id=0&any%5Bx%5D=anytag".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "tag nested array filter is neutral",
            Method::GET,
            "/api/v1/timelines/tag/fixturetag",
            Some("local=true&max_id=0&all%5B%5D%5Bx%5D=alltag".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "array object type collision",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5B%5D%5Bx%5D=1&a%5B%5D%5Bx%5D%5B%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, VARY][..],
        ),
        (
            "special array child collision",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&a%5B%5D%5B%5D%5Bx%5D=1&a%5B%5D%5B%5D%5Bx%5D%5B%5D=2".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, VARY][..],
        ),
        (
            "malformed query utf8",
            Method::GET,
            "/api/v1/timelines/public",
            Some("limit=%FF".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "nested scalar query shape",
            Method::GET,
            "/api/v1/timelines/public",
            Some("limit%5B%5D=1".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
        (
            "bare limit value",
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&limit".to_owned()),
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK][..],
        ),
        (
            "malformed tag path utf8",
            Method::GET,
            "/api/v1/timelines/tag/%FF",
            None,
            stable_request_headers(),
            &[CONTENT_TYPE, CACHE_CONTROL, VARY][..],
        ),
    ] {
        let request = RequestSpec::new(method, path, query, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            relevant_headers,
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }

    for (label, content_type, body) in [
        (
            "JSON body parameters",
            "application/json",
            br#"{"local":true,"limit":1}"#.as_slice(),
        ),
        (
            "form body parameters",
            "application/x-www-form-urlencoded",
            b"local=true&limit=1".as_slice(),
        ),
        ("malformed JSON body", "application/json", b"{".as_slice()),
        (
            "JSON null body parameter",
            "application/json",
            br#"{"local":true,"limit":null}"#.as_slice(),
        ),
        (
            "JSON empty-array limit",
            "application/json",
            br#"{"local":true,"limit":[]}"#.as_slice(),
        ),
        (
            "ActivityStreams JSON body",
            "application/activity+json",
            br#"{"local":true,"limit":1}"#.as_slice(),
        ),
        (
            "problem JSON body",
            "application/problem+json",
            br#"{"local":true,"limit":1}"#.as_slice(),
        ),
        (
            "unregistered JSON suffix",
            "application/hal+json",
            br#"{"local":true,"limit":1}"#.as_slice(),
        ),
        (
            "JSON exponential limit",
            "application/json",
            br#"{"local":true,"limit":1e3}"#.as_slice(),
        ),
        (
            "JSON float zero boolean",
            "application/json",
            br#"{"local":0.0,"limit":1}"#.as_slice(),
        ),
        (
            "JSON fractional boolean",
            "application/json",
            br#"{"local":1e-3,"limit":1}"#.as_slice(),
        ),
        (
            "JSON small scientific link",
            "application/json",
            br#"{"local":1e-5,"limit":1}"#.as_slice(),
        ),
        (
            "JSON scientific link",
            "application/json",
            br#"{"local":true,"limit":1e15}"#.as_slice(),
        ),
        (
            "JSON decimal link boundary",
            "application/json",
            br#"{"local":true,"limit":1e14}"#.as_slice(),
        ),
        (
            "JSON overflowing cursor",
            "application/json",
            br#"{"local":true,"max_id":9223372036854775808}"#.as_slice(),
        ),
        (
            "JSON nonfinite exponent",
            "application/json",
            br#"{"local":true,"limit":1e400}"#.as_slice(),
        ),
        (
            "ignored JSON nonfinite exponent",
            "application/json",
            br#"{"local":true,"limit":1,"ignored":1e400}"#.as_slice(),
        ),
        (
            "JSON boolean limit",
            "application/json",
            br#"{"local":true,"limit":true}"#.as_slice(),
        ),
    ] {
        let mut headers = stable_request_headers();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/timelines/public",
            None,
            headers,
            body.to_vec(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }

    let mut merge_headers = stable_request_headers();
    merge_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/timelines/public",
        Some("local=true&max_id%5B%5D=1".to_owned()),
        merge_headers,
        br#"{"max_id":"2"}"#.to_vec(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("body/query parameter merge: {error}"))?;

    let mut relation_headers = stable_request_headers();
    relation_headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/accounts/relationships",
        Some("id%5Bx%5D=116844606259202001".to_owned()),
        relation_headers,
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, VARY],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("relationship hash shape: {error}"))?;

    let mut nested_relation_headers = stable_request_headers();
    nested_relation_headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-bearer-token-v4-6-5"),
    );
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/accounts/relationships",
        Some("id%5B%5D%5Bx%5D=1".to_owned()),
        nested_relation_headers,
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, VARY],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("nested relationship object: {error}"))?;

    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/timelines/public",
        Some("local=true&a%5B%5D%5Bx%5D%5Bz%5D=1&a%5B%5D%5Bx%5D%5By%5D=1&a%5B%5D%5Bx%5D%5By%5D=2&a%5B%5D%5Bx%5D%5Bz%5D%5B%5D=3".to_owned()),
        stable_request_headers(),
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, CACHE_CONTROL, VARY],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("nested array object reuse: {error}"))?;

    for (label, depth) in [("JSON depth 99", 99), ("JSON depth 100", 100)] {
        let mut value = "null".to_owned();
        for _ in 0..depth {
            value = format!(r#"{{"nested":{value}}}"#);
        }
        let body = format!(r#"{{"local":true,"limit":1,"irrelevant":{value}}}"#);
        let mut headers = stable_request_headers();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/timelines/public",
            None,
            headers,
            body.into_bytes(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }

    for (label, depth) in [
        ("JSON root array depth 100", 100),
        ("JSON root array depth 101", 101),
    ] {
        let mut body = "null".to_owned();
        for _ in 0..depth {
            body = format!("[{body}]");
        }
        let mut headers = stable_request_headers();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/timelines/public",
            Some("local=true&limit=1".to_owned()),
            headers,
            body.into_bytes(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CACHE_CONTROL, VARY, LINK],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }

    for (label, query) in [
        ("lookup array account", "acct%5B%5D=alice"),
        ("lookup hash account", "acct%5Bx%5D=alice"),
    ] {
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/accounts/lookup",
            Some(query.to_owned()),
            stable_request_headers(),
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, VARY],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
    }

    let deep_key = format!("local=true&deep{}=1", "%5Bx%5D".repeat(32));
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/timelines/public",
        Some(deep_key),
        stable_request_headers(),
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, VARY],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("Rack nesting limit: {error}"))?;

    let trailing_depth_key = format!("local=true&deep{}foo=1", "%5Bx%5D".repeat(31));
    let request = RequestSpec::new(
        Method::GET,
        "/api/v1/timelines/public",
        Some(trailing_depth_key),
        stable_request_headers(),
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, VARY],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("Rack trailing-suffix depth limit: {error}"))?;

    let mut preflight_headers = stable_request_headers();
    preflight_headers.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
    preflight_headers.insert(
        ACCESS_CONTROL_REQUEST_METHOD,
        HeaderValue::from_static("GET"),
    );
    preflight_headers.insert(
        ACCESS_CONTROL_REQUEST_HEADERS,
        HeaderValue::from_static("authorization, x-client"),
    );
    let request = RequestSpec::new(
        Method::OPTIONS,
        "/api/v1/timelines/home",
        None,
        preflight_headers,
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_status_headers_and_body(
        &responses.mastodon,
        &responses.rust,
        &[
            VARY,
            ACCESS_CONTROL_ALLOW_ORIGIN,
            ACCESS_CONTROL_ALLOW_METHODS,
            ACCESS_CONTROL_ALLOW_HEADERS,
            ACCESS_CONTROL_EXPOSE_HEADERS,
            ACCESS_CONTROL_MAX_AGE,
        ],
    )
    .map_err(|error| format!("CORS preflight: {error}"))?;
    for path in [
        "/api/v1/accounts/search",
        "/api/v1/accounts/familiar_followers",
        "/api/v1/accounts/familiar_followers/",
    ] {
        let mut account_preflight_headers = stable_request_headers();
        account_preflight_headers
            .insert(ORIGIN, HeaderValue::from_static("https://client.example"));
        account_preflight_headers.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("GET"),
        );
        account_preflight_headers.insert(
            ACCESS_CONTROL_REQUEST_HEADERS,
            HeaderValue::from_static("authorization"),
        );
        let request = RequestSpec::new(
            Method::OPTIONS,
            path,
            None,
            account_preflight_headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_status_headers_and_body(
            &responses.mastodon,
            &responses.rust,
            &[
                VARY,
                ACCESS_CONTROL_ALLOW_ORIGIN,
                ACCESS_CONTROL_ALLOW_METHODS,
                ACCESS_CONTROL_ALLOW_HEADERS,
                ACCESS_CONTROL_EXPOSE_HEADERS,
                ACCESS_CONTROL_MAX_AGE,
            ],
        )
        .map_err(|error| format!("account CORS preflight {path}: {error}"))?;
    }
    let mut oversized_preflight_headers = stable_request_headers();
    oversized_preflight_headers.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
    oversized_preflight_headers.insert(
        ACCESS_CONTROL_REQUEST_METHOD,
        HeaderValue::from_static("GET"),
    );
    let request = RequestSpec::new(
        Method::OPTIONS,
        "/api/v1/timelines/home",
        None,
        oversized_preflight_headers,
        vec![0; 99 * 1024 * 1024 + 1],
    )?;
    let responses = guard.send(&request).await?;
    compare_status_headers_and_body(
        &responses.mastodon,
        &responses.rust,
        &[
            ACCESS_CONTROL_ALLOW_ORIGIN,
            ACCESS_CONTROL_ALLOW_METHODS,
            ACCESS_CONTROL_EXPOSE_HEADERS,
            ACCESS_CONTROL_MAX_AGE,
        ],
    )
    .map_err(|error| format!("oversized CORS preflight: {error}"))?;
    let mut malformed_preflight = stable_request_headers();
    malformed_preflight.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
    malformed_preflight.insert(
        ACCESS_CONTROL_REQUEST_METHOD,
        HeaderValue::from_static("GET"),
    );
    let request = RequestSpec::new(
        Method::OPTIONS,
        "/api/v1/timelines/home",
        Some("limit=%FF".to_owned()),
        malformed_preflight,
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_status_headers_and_body(
        &responses.mastodon,
        &responses.rust,
        &[
            ACCESS_CONTROL_ALLOW_ORIGIN,
            ACCESS_CONTROL_ALLOW_METHODS,
            ACCESS_CONTROL_EXPOSE_HEADERS,
            ACCESS_CONTROL_MAX_AGE,
        ],
    )
    .map_err(|error| format!("malformed-query CORS preflight: {error}"))?;
    for path in [
        "/api/v1/accounts/not-an-id",
        "/api/v1/accounts/search/statuses",
        "/api/v1/accounts/familiar_followers/followers",
        "/api/v1/statuses/not-an-id/context",
        "/api/v1/statuses/not-an-id/context/",
        "/api/v1/timelines/list/not-an-id",
        "/api/v1/timelines/home/",
    ] {
        let mut malformed_id_preflight = stable_request_headers();
        malformed_id_preflight.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
        malformed_id_preflight.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("GET"),
        );
        let request = RequestSpec::new(
            Method::OPTIONS,
            path,
            None,
            malformed_id_preflight,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_status_headers_and_body(
            &responses.mastodon,
            &responses.rust,
            &[
                ACCESS_CONTROL_ALLOW_ORIGIN,
                ACCESS_CONTROL_ALLOW_METHODS,
                ACCESS_CONTROL_EXPOSE_HEADERS,
                ACCESS_CONTROL_MAX_AGE,
            ],
        )
        .map_err(|error| format!("malformed-id CORS preflight {path}: {error}"))?;
    }
    let mut malformed_path_preflight = stable_request_headers();
    malformed_path_preflight.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
    malformed_path_preflight.insert(
        ACCESS_CONTROL_REQUEST_METHOD,
        HeaderValue::from_static("GET"),
    );
    let request = RequestSpec::new(
        Method::OPTIONS,
        "/api/v1/timelines/tag/%FF",
        None,
        malformed_path_preflight,
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_status_headers_and_body(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, VARY, ACCESS_CONTROL_ALLOW_ORIGIN],
    )
    .map_err(|error| format!("malformed-path CORS preflight: {error}"))?;
    for path in ["/api/v1/unknown"] {
        let mut unsupported_preflight = stable_request_headers();
        unsupported_preflight.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
        unsupported_preflight.insert(
            ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("GET"),
        );
        let request = RequestSpec::new(
            Method::OPTIONS,
            path,
            None,
            unsupported_preflight,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        if responses.rust.status != 404
            || responses
                .rust
                .headers
                .contains_key(ACCESS_CONTROL_ALLOW_METHODS)
        {
            return Err(format!("unsupported route preflight was advertised: {path}").into());
        }
    }
    guard.finish().await
}

fn compare_status_headers_and_body(
    mastodon: &CapturedResponse,
    rust: &CapturedResponse,
    headers: &[reqwest::header::HeaderName],
) -> Result<(), String> {
    if mastodon.status != rust.status {
        return Err(format!(
            "status differs: Mastodon={}, Rust={}",
            mastodon.status, rust.status
        ));
    }
    for header in headers {
        if mastodon.headers.get_all(header).iter().collect::<Vec<_>>()
            != rust.headers.get_all(header).iter().collect::<Vec<_>>()
        {
            return Err(format!(
                "header {header} differs: Mastodon={:?}, Rust={:?}",
                mastodon.headers.get_all(header).iter().collect::<Vec<_>>(),
                rust.headers.get_all(header).iter().collect::<Vec<_>>()
            ));
        }
    }
    (mastodon.body == rust.body)
        .then_some(())
        .ok_or_else(|| "response bodies differ".to_owned())
}

#[allow(clippy::too_many_lines)]
async fn run_local_paperclip_media_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn std::error::Error>> {
    const PATHS: &[&str] = &[
        "/system/accounts/avatars/116/844/606/259/201/001/original/0112603425bb49c1.png",
        "/system/media_attachments/files/116/844/842/188/806/001/original/cd63911ad76f4d5d.jpg",
        "/system/media_attachments/files/116/844/842/188/806/001/small/cd63911ad76f4d5d.jpg",
        "/system/cache/accounts/avatars/116/844/606/259/202/001/original/bob.png",
        "/system/cache/media_attachments/files/116/845/105/643/526/106/original/cached.jpg",
        "/system/cache/media_attachments/files/116/845/105/643/526/106/small/cached.jpg",
        "/system/cache/custom_emojis/images/000/012/001/original/fixtureparty.png",
        "/system/cache/custom_emojis/images/000/012/001/static/fixtureparty.png",
        "/system/cache/preview_cards/images/000/012/002/original/preview.png",
    ];
    let guard = ReadOnlyGuard::begin(config, rust_url).await?;
    let comparisons = async {
        for path in PATHS {
            for method in [Method::GET, Method::HEAD] {
                let request = RequestSpec::new(
                    method.clone(),
                    *path,
                    None,
                    stable_request_headers(),
                    Vec::new(),
                )?;
                let responses = guard.send(&request).await?;
                compare_status_headers_and_body(
                    &responses.mastodon,
                    &responses.rust,
                    &[
                        CONTENT_TYPE,
                        CONTENT_LENGTH,
                        LAST_MODIFIED,
                        CACHE_CONTROL,
                        HeaderName::from_static("content-security-policy"),
                        HeaderName::from_static("x-content-type-options"),
                    ],
                )
                .map_err(|error| format!("{method} {path}: {error}"))?;
            }
        }

        let range_path = PATHS[1];
        for (label, range) in [
            ("bounded", "bytes=0-15"),
            ("open", "bytes=32-"),
            ("suffix", "bytes=-16"),
            ("multiple", "bytes=0-3,8-11"),
            ("unsatisfiable", "bytes=999999-"),
        ] {
            let mut headers = stable_request_headers();
            headers.insert(RANGE, HeaderValue::from_static(range));
            let request = RequestSpec::new(Method::GET, range_path, None, headers, Vec::new())?;
            let responses = guard.send(&request).await?;
            compare_status_headers_and_body(
                &responses.mastodon,
                &responses.rust,
                &[
                    CONTENT_TYPE,
                    CONTENT_LENGTH,
                    CONTENT_RANGE,
                    LAST_MODIFIED,
                    CACHE_CONTROL,
                    HeaderName::from_static("content-security-policy"),
                    HeaderName::from_static("x-content-type-options"),
                ],
            )
            .map_err(|error| format!("{label} range: {error}"))?;
        }

        let baseline = RequestSpec::new(
            Method::GET,
            range_path,
            None,
            stable_request_headers(),
            Vec::new(),
        )?;
        let baseline = guard.send(&baseline).await?;
        let last_modified = baseline
            .mastodon
            .headers
            .get(LAST_MODIFIED)
            .cloned()
            .ok_or("Mastodon media response has no Last-Modified header")?;
        for (label, value) in [
            ("exact", last_modified),
            (
                "future",
                HeaderValue::from_static("Fri, 01 Jan 2100 00:00:00 GMT"),
            ),
        ] {
            let mut headers = stable_request_headers();
            headers.insert(reqwest::header::IF_MODIFIED_SINCE, value);
            let request = RequestSpec::new(Method::GET, range_path, None, headers, Vec::new())?;
            let responses = guard.send(&request).await?;
            compare_status_headers_and_body(
                &responses.mastodon,
                &responses.rust,
                &[
                    CONTENT_TYPE,
                    LAST_MODIFIED,
                    CACHE_CONTROL,
                    HeaderName::from_static("content-security-policy"),
                    HeaderName::from_static("x-content-type-options"),
                ],
            )
            .map_err(|error| format!("{label} If-Modified-Since: {error}"))?;
        }
        let mut headers = stable_request_headers();
        headers.insert(
            reqwest::header::IF_MODIFIED_SINCE,
            HeaderValue::from_static("Fri, 01 Jan 2100 00:00:00 GMT"),
        );
        headers.insert(RANGE, HeaderValue::from_static("bytes=0-15"));
        let request = RequestSpec::new(Method::GET, range_path, None, headers, Vec::new())?;
        let responses = guard.send(&request).await?;
        compare_status_headers_and_body(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, CONTENT_LENGTH, CONTENT_RANGE, LAST_MODIFIED],
        )
        .map_err(|error| format!("future If-Modified-Since with range: {error}"))?;

        for path in [
            "/system/accounts/avatars/116/844/606/259/201/001/static/0112603425bb49c1.png",
            "/system/cache/accounts/avatars/116/844/606/259/201/001/original/0112603425bb49c1.png",
            "/system/accounts/avatars/116/844/606/259/201/001/original/wrong.png",
            "/system/%2e%2e/accounts/avatars/116/844/606/259/201/001/original/0112603425bb49c1.png",
        ] {
            let request = RequestSpec::new(
                Method::GET,
                path,
                None,
                stable_request_headers(),
                Vec::new(),
            )?;
            let responses = guard.send(&request).await?;
            if responses.mastodon.status == 200 {
                return Err(
                    format!("Mastodon unexpectedly served invalid media path: {path}").into(),
                );
            }
            if responses.rust.status != 404 {
                return Err(format!("unsafe or unauthorized media path was served: {path}").into());
            }
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    let read_only = guard.finish().await;
    comparisons?;
    read_only
}

async fn run_local_paperclip_deleted_media_case(
    config: &DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn std::error::Error>> {
    const STATUS_ID: i64 = 116_844_842_188_805_001;
    const MEDIA_PATH: &str =
        "/system/media_attachments/files/116/844/842/188/806/001/original/cd63911ad76f4d5d.jpg";
    let owner = config
        .rust_owner_database
        .as_ref()
        .ok_or("Rust owner database is required for the media authorization case")?;
    let owner_pool = sqlx::PgPool::connect(owner.url()).await?;
    let original_deleted_at: Option<sqlx::types::chrono::NaiveDateTime> =
        sqlx::query_scalar("SELECT deleted_at FROM statuses WHERE id = $1")
            .bind(STATUS_ID)
            .fetch_one(&owner_pool)
            .await?;
    sqlx::query("UPDATE statuses SET deleted_at = clock_timestamp() WHERE id = $1")
        .bind(STATUS_ID)
        .execute(&owner_pool)
        .await?;

    let result = async {
        let anonymous = RequestSpec::new(
            Method::GET,
            MEDIA_PATH,
            None,
            stable_request_headers(),
            Vec::new(),
        )?;
        let anonymous_response = send_single(rust_url, &anonymous, "Rust").await?;
        if anonymous_response.status != 404 {
            return Err(format!(
                "anonymous discarded-media read returned {}",
                anonymous_response.status
            )
            .into());
        }

        let mut moderator_headers = stable_request_headers();
        moderator_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-bearer-api-moderator-v4-6-5"),
        );
        let moderator =
            RequestSpec::new(Method::GET, MEDIA_PATH, None, moderator_headers, Vec::new())?;
        let moderator_response = send_single(rust_url, &moderator, "Rust").await?;
        if moderator_response.status != 200 {
            return Err(format!(
                "manage_reports discarded-media read returned {}",
                moderator_response.status
            )
            .into());
        }
        if moderator_response.body.is_empty() {
            return Err("manage_reports discarded-media read returned an empty body".into());
        }
        if moderator_response
            .headers
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            != Some("private, no-store")
        {
            return Err("authorized discarded-media response was publicly cacheable".into());
        }
        if moderator_response
            .headers
            .get(VARY)
            .and_then(|value| value.to_str().ok())
            != Some("Authorization, Cookie, Signature")
        {
            return Err("authorized discarded-media response did not vary on credentials".into());
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    sqlx::query("UPDATE statuses SET deleted_at = $2 WHERE id = $1")
        .bind(STATUS_ID)
        .bind(original_deleted_at)
        .execute(&owner_pool)
        .await?;
    result
}

#[allow(clippy::too_many_lines)]
async fn run_status_authorization_matrix_case(
    config: DifferentialConfig,
    rust_url: &Url,
) -> Result<(), Box<dyn std::error::Error>> {
    let guard = ReadOnlyGuard::begin(config, rust_url).await?;
    for (label, status_id, token, expected_status) in [
        ("anonymous public", 116_844_842_188_805_001_i64, None, 200),
        ("anonymous unlisted", 116_844_846_120_965_002, None, 200),
        ("anonymous private", 116_844_850_053_125_003, None, 404),
        ("anonymous direct", 116_844_853_985_285_004, None, 404),
        ("anonymous limited", 116_844_857_917_445_005, None, 404),
        (
            "owner private",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "owner direct",
            116_844_853_985_285_004,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "owner limited",
            116_844_857_917_445_005,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "unrelated private",
            116_844_850_053_125_003,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "unrelated direct",
            116_844_853_985_285_004,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "unrelated limited",
            116_844_857_917_445_005,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "current follower private",
            116_844_850_053_125_003,
            Some("fixture-bearer-api-moderator-v4-6-5"),
            200,
        ),
        (
            "active mention direct",
            116_844_853_985_285_004,
            Some("fixture-bearer-api-moderator-v4-6-5"),
            200,
        ),
        (
            "silent mention limited",
            116_844_857_917_445_005,
            Some("fixture-bearer-api-moderator-v4-6-5"),
            200,
        ),
        (
            "former follower silent mention",
            -311,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "former follower without mention",
            -312,
            Some("fixture-bearer-token-v4-6-5"),
            404,
        ),
        (
            "author-side block",
            -313,
            Some("fixture-bearer-api-moderator-v4-6-5"),
            404,
        ),
        ("suspended author", -310, None, 404),
        ("soft-deleted status", 116_846_257_766_400_501, None, 404),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            format!("/api/v1/statuses/{status_id}"),
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
        if responses.rust.status != expected_status {
            return Err(format!(
                "{label}: expected HTTP {expected_status}, got {}",
                responses.rust.status
            )
            .into());
        }
    }
    for (label, token, forbidden_ids) in [
        (
            "anonymous account statuses",
            None,
            &[
                116_844_850_053_125_003_i64,
                116_844_853_985_285_004,
                116_844_857_917_445_005,
            ][..],
        ),
        (
            "unrelated account statuses",
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            &[
                116_844_850_053_125_003_i64,
                116_844_853_985_285_004,
                116_844_857_917_445_005,
            ][..],
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            "/api/v1/accounts/116844606259201001/statuses",
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE, LINK],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
        let body: Value = serde_json::from_slice(&responses.rust.body)?;
        for forbidden_id in forbidden_ids {
            if body.as_array().is_some_and(|statuses| {
                statuses.iter().any(|status| {
                    status["id"].as_str().and_then(|id| id.parse().ok()) == Some(*forbidden_id)
                })
            }) {
                return Err(format!("{label}: leaked status {forbidden_id}").into());
            }
        }
    }
    for (label, status_id, token, expected_status) in [
        (
            "anonymous private context",
            116_844_850_053_125_003_i64,
            None,
            404,
        ),
        (
            "unrelated private context",
            116_844_850_053_125_003,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "owner private context",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "unrelated direct context",
            116_844_853_985_285_004,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "active mention direct context",
            116_844_853_985_285_004,
            Some("fixture-bearer-api-moderator-v4-6-5"),
            200,
        ),
        (
            "unrelated limited context",
            116_844_857_917_445_005,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "silent mention limited context",
            116_844_857_917_445_005,
            Some("fixture-bearer-api-moderator-v4-6-5"),
            200,
        ),
    ] {
        let mut headers = stable_request_headers();
        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }
        let request = RequestSpec::new(
            Method::GET,
            format!("/api/v1/statuses/{status_id}/context"),
            None,
            headers,
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        compare_responses(
            &responses.mastodon,
            &responses.rust,
            &[CONTENT_TYPE],
            &[],
            DEFAULT_MISMATCH_LIMIT,
        )
        .map_err(|error| format!("{label}: {error}"))?;
        if responses.rust.status != expected_status {
            return Err(format!(
                "{label}: expected HTTP {expected_status}, got {}",
                responses.rust.status
            )
            .into());
        }
    }
    for (label, suffix, status_id, token, expected_status) in [
        (
            "public source owner",
            "source",
            116_844_842_188_805_001_i64,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "private source unrelated",
            "source",
            116_844_850_053_125_003,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "direct source active mention",
            "source",
            116_844_853_985_285_004,
            Some("fixture-bearer-api-moderator-v4-6-5"),
            200,
        ),
        (
            "public history anonymous",
            "history",
            116_844_842_188_805_001,
            None,
            200,
        ),
        (
            "private history anonymous",
            "history",
            116_844_850_053_125_003,
            None,
            404,
        ),
        (
            "private history owner",
            "history",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "private history unrelated",
            "history",
            116_844_850_053_125_003,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "direct history active mention",
            "history",
            116_844_853_985_285_004,
            Some("fixture-bearer-api-moderator-v4-6-5"),
            200,
        ),
        (
            "deleted history owner",
            "history",
            116_846_257_766_400_501,
            Some("fixture-bearer-token-v4-6-5"),
            404,
        ),
    ] {
        compare_status_matrix_json(
            &guard,
            label,
            format!("/api/v1/statuses/{status_id}/{suffix}"),
            None,
            token,
            expected_status,
            &[],
        )
        .await?;
    }
    for (label, status_id, token, expected_status) in [
        (
            "public quotes owner",
            116_844_842_188_805_001_i64,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "private quotes owner",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "private quotes unrelated",
            116_844_850_053_125_003,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "deleted quotes owner",
            116_846_257_766_400_501,
            Some("fixture-bearer-token-v4-6-5"),
            404,
        ),
    ] {
        compare_status_matrix_json(
            &guard,
            label,
            format!("/api/v1/statuses/{status_id}/quotes"),
            Some("limit=1"),
            token,
            expected_status,
            &[116_844_850_053_125_003, 116_846_257_766_400_501],
        )
        .await?;
    }
    for (label, suffix, status_id, token, expected_status) in [
        (
            "public favourited-by anonymous",
            "favourited_by",
            116_844_842_188_805_001_i64,
            None,
            200,
        ),
        (
            "private favourited-by anonymous",
            "favourited_by",
            116_844_850_053_125_003,
            None,
            404,
        ),
        (
            "private favourited-by unrelated",
            "favourited_by",
            116_844_850_053_125_003,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "private favourited-by owner",
            "favourited_by",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "author-blocked favourited-by moderator",
            "favourited_by",
            -313,
            Some("fixture-bearer-api-moderator-v4-6-5"),
            404,
        ),
        (
            "public reblogged-by anonymous",
            "reblogged_by",
            116_844_842_188_805_001,
            None,
            200,
        ),
        (
            "private reblogged-by anonymous",
            "reblogged_by",
            116_844_850_053_125_003,
            None,
            404,
        ),
        (
            "private reblogged-by unrelated",
            "reblogged_by",
            116_844_850_053_125_003,
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            404,
        ),
        (
            "private reblogged-by owner",
            "reblogged_by",
            116_844_850_053_125_003,
            Some("fixture-bearer-token-v4-6-5"),
            200,
        ),
        (
            "deleted reblogged-by owner",
            "reblogged_by",
            116_846_257_766_400_501,
            Some("fixture-bearer-token-v4-6-5"),
            404,
        ),
    ] {
        compare_status_matrix_json(
            &guard,
            label,
            format!("/api/v1/statuses/{status_id}/{suffix}"),
            Some("limit=1"),
            token,
            expected_status,
            &[],
        )
        .await?;
    }
    for (label, path, query, token, expected_status, forbidden_ids) in [
        (
            "account statuses owner",
            "/api/v1/accounts/116844606259201001/statuses",
            Some("limit=40"),
            Some("fixture-bearer-token-v4-6-5"),
            200,
            &[116_846_257_766_400_501_i64, -310][..],
        ),
        (
            "public timeline anonymous",
            "/api/v1/timelines/public",
            Some("local=true&max_id=0&limit=2"),
            None,
            200,
            &[
                116_844_850_053_125_003_i64,
                116_844_853_985_285_004,
                116_844_857_917_445_005,
                116_846_257_766_400_501,
                -310,
            ][..],
        ),
        (
            "public timeline unrelated",
            "/api/v1/timelines/public",
            Some("max_id=0&limit=2"),
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            200,
            &[
                116_844_850_053_125_003_i64,
                116_844_853_985_285_004,
                116_844_857_917_445_005,
                116_846_257_766_400_501,
                -310,
            ][..],
        ),
        (
            "tag timeline anonymous",
            "/api/v1/timelines/tag/fixturetag",
            Some("local=true&max_id=0&limit=2"),
            None,
            200,
            &[
                116_844_850_053_125_003_i64,
                116_844_853_985_285_004,
                116_844_857_917_445_005,
                116_846_257_766_400_501,
                -310,
            ][..],
        ),
        (
            "tag timeline unrelated",
            "/api/v1/timelines/tag/fixturetag",
            Some("max_id=0&limit=2"),
            Some("fixture-bearer-matrix-viewer-v4-6-5"),
            200,
            &[
                116_844_850_053_125_003_i64,
                116_844_853_985_285_004,
                116_844_857_917_445_005,
                116_846_257_766_400_501,
                -310,
            ][..],
        ),
        (
            "home timeline owner",
            "/api/v1/timelines/home",
            Some("limit=40"),
            Some("fixture-bearer-token-v4-6-5"),
            200,
            &[116_846_257_766_400_501_i64, -310][..],
        ),
        (
            "list timeline owner",
            "/api/v1/timelines/list/9001",
            Some("limit=40"),
            Some("fixture-bearer-read-lists-v4-6-5"),
            200,
            &[116_846_257_766_400_501_i64, -310][..],
        ),
    ] {
        compare_status_matrix_json(
            &guard,
            label,
            path,
            query,
            token,
            expected_status,
            forbidden_ids,
        )
        .await?;
    }
    for (label, path, token, expected_status) in [
        (
            "favourites owner",
            "/api/v1/favourites",
            "fixture-bearer-read-favourites-v4-6-5",
            200,
        ),
        (
            "favourites broad owner",
            "/api/v1/favourites",
            "fixture-bearer-token-v4-6-5",
            200,
        ),
        (
            "bookmarks owner",
            "/api/v1/bookmarks",
            "fixture-bearer-read-bookmarks-v4-6-5",
            200,
        ),
        (
            "bookmarks broad owner",
            "/api/v1/bookmarks",
            "fixture-bearer-token-v4-6-5",
            200,
        ),
    ] {
        compare_status_matrix_json(
            &guard,
            label,
            path,
            Some("limit=40"),
            Some(token),
            expected_status,
            &[116_846_257_766_400_501, -310],
        )
        .await?;
    }
    compare_status_matrix_json(
        &guard,
        "notifications owner",
        "/api/v1/notifications",
        Some("limit=40"),
        Some("fixture-bearer-token-v4-6-5"),
        200,
        &[116_846_257_766_400_501, -310],
    )
    .await?;
    compare_status_matrix_json(
        &guard,
        "notification show owner",
        "/api/v1/notifications/10003",
        None,
        Some("fixture-bearer-token-v4-6-5"),
        200,
        &[],
    )
    .await?;
    compare_status_matrix_json(
        &guard,
        "notification show wrong owner",
        "/api/v1/notifications/10003",
        None,
        Some("fixture-bearer-api-moderator-v4-6-5"),
        404,
        &[],
    )
    .await?;
    compare_status_matrix_json(
        &guard,
        "conversations owner",
        "/api/v1/conversations",
        Some("limit=40"),
        Some("fixture-bearer-token-v4-6-5"),
        200,
        &[116_846_257_766_400_501, -310],
    )
    .await?;
    compare_status_matrix_json(
        &guard,
        "conversations missing token",
        "/api/v1/conversations",
        None,
        None,
        401,
        &[],
    )
    .await?;

    for (label, path, expected_status) in [
        (
            "public remote media proxy anonymous",
            "/media_proxy/116845105643526106/original",
            200,
        ),
        (
            "public remote media proxy small anonymous",
            "/media_proxy/116845105643526106/small",
            200,
        ),
        (
            "deleted media proxy owner",
            "/media_proxy/-98/original",
            404,
        ),
    ] {
        let request = RequestSpec::new(
            Method::GET,
            path,
            None,
            stable_request_headers(),
            Vec::new(),
        )?;
        let responses = guard.send(&request).await?;
        if responses.mastodon.status == 404 && expected_status == 200 {
            return Err(format!("{label}: Mastodon denied the public media fixture").into());
        }
        if responses.rust.status != expected_status {
            return Err(format!(
                "{label}: expected HTTP {expected_status}, got {}",
                responses.rust.status
            )
            .into());
        }
    }
    guard.finish().await
}

async fn compare_status_matrix_json(
    guard: &ReadOnlyGuard,
    label: &str,
    path: impl Into<String>,
    query: Option<&str>,
    token: Option<&str>,
    expected_status: u16,
    forbidden_status_ids: &[i64],
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut headers = stable_request_headers();
    if let Some(token) = token {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
    }
    let request = RequestSpec::new(
        Method::GET,
        path,
        query.map(str::to_owned),
        headers,
        Vec::new(),
    )?;
    let responses = guard.send(&request).await?;
    compare_responses(
        &responses.mastodon,
        &responses.rust,
        &[CONTENT_TYPE, LINK],
        &[],
        DEFAULT_MISMATCH_LIMIT,
    )
    .map_err(|error| format!("{label}: {error}"))?;
    if responses.rust.status != expected_status {
        return Err(format!(
            "{label}: expected HTTP {expected_status}, got {}",
            responses.rust.status
        )
        .into());
    }
    let body: Value = serde_json::from_slice(&responses.rust.body)
        .map_err(|error| format!("{label}: invalid JSON response: {error}"))?;
    for status_id in forbidden_status_ids {
        if json_contains_string(&body, &status_id.to_string()) {
            return Err(format!("{label}: response contains forbidden status {status_id}").into());
        }
    }
    Ok(body)
}

fn json_contains_string(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(value) => value == expected,
        Value::Number(value) => value.to_string() == expected,
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains_string(value, expected)),
        Value::Object(values) => values
            .values()
            .any(|value| json_contains_string(value, expected)),
        Value::Null | Value::Bool(_) => false,
    }
}

fn stable_request_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        HOST,
        HeaderValue::from_static("fixture-v4-6-5.rustodon.invalid"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    headers
}

fn response_contains_fixture_credential(response: &CapturedResponse) -> bool {
    const CANARIES: [&[u8]; 14] = [
        b"fixture-bearer",
        b"bearer-token-v4",
        b"read-statuses-v4",
        b"read-accounts-v4",
        b"bearer-unknown-v4",
        b"bearer-revoked-v4",
        b"bearer-expired-v4",
        b"bearer-insufficient-v4",
        b"application-only-v4",
        b"disabled-user-v4",
        b"missing-2fa-v4",
        b"fixture-only-client-secret-v4-6-5",
        b"rustodon-fixture-client-v4-6-5",
        b"fixture-refresh-token-v4-6-5",
    ];
    let contains_canary = |bytes: &[u8]| {
        CANARIES
            .iter()
            .any(|canary| bytes.windows(canary.len()).any(|window| window == *canary))
    };
    contains_canary(&response.body)
        || response
            .headers
            .values()
            .any(|value| contains_canary(value.as_bytes()))
}

#[test]
fn credential_detection_checks_bodies_and_headers_without_matching_the_fixture_domain() {
    let clean = CapturedResponse {
        status: 200,
        headers: HeaderMap::new(),
        body: br#"{"url":"https://fixture-v4-6-5.rustodon.invalid/@alice"}"#.to_vec(),
    };
    assert!(!response_contains_fixture_credential(&clean));

    let mut body_leak = clean.clone();
    body_leak.body = b"leaked application-only-v4 credential fragment".to_vec();
    assert!(response_contains_fixture_credential(&body_leak));

    let mut header_leak = clean;
    header_leak.headers.insert(
        "x-oauth-diagnostic",
        HeaderValue::from_static("leaked read-statuses-v4 fragment"),
    );
    assert!(response_contains_fixture_credential(&header_leak));
}

fn checked_instance_fixture() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/differential/instance-v2.json").to_vec();
    let value: Value = serde_json::from_slice(&bytes)?;
    if value["domain"] != "fixture-v4-6-5.rustodon.invalid"
        || value["version"] != "4.6.5"
        || value["configuration"]["vapid"]["public_key"]
            != "BB37UCyc8LLX4PNQSe-04vSFvpUWGrENubUaslVFM_l5TxcGVMY0C3RXPeUJAQHKYlcOM2P4vTYmkoo0VZGZTM4="
        || !value["wrapstodon"].is_null()
    {
        return Err("instance-v2 fixture does not match the pinned deterministic baseline".into());
    }
    Ok(bytes)
}

#[tokio::test]
#[ignore = "requires guarded PostgreSQL clones from tools/mastodon-fixture"]
async fn browser_recovery_fences() -> Result<(), Box<dyn std::error::Error>> {
    let config =
        DifferentialConfig::from_process_environment(&PathBuf::from(env!("CARGO_MANIFEST_DIR")))?;
    tokio::time::timeout(
        std::time::Duration::from_mins(2),
        differential::reauth::recovery_fences(config),
    )
    .await?
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "requires guarded PostgreSQL clones from tools/mastodon-fixture"]
async fn browser_reauthentication_limits() -> Result<(), Box<dyn std::error::Error>> {
    let config =
        DifferentialConfig::from_process_environment(&PathBuf::from(env!("CARGO_MANIFEST_DIR")))?;
    tokio::time::timeout(
        std::time::Duration::from_mins(3),
        differential::reauth_limits::run(config),
    )
    .await?
}

#[tokio::test]
#[ignore = "requires guarded disposable fixture databases and owner URL"]
async fn extended_description() -> Result<(), Box<dyn std::error::Error>> {
    differential::extended_description::extended_description_http_contract().await
}

#[tokio::test]
#[ignore = "requires guarded disposable fixture databases and owner URL"]
async fn extended_description_limited_mode() -> Result<(), Box<dyn std::error::Error>> {
    differential::extended_description::extended_description_limited_mode_contract().await
}
