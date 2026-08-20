mod differential {
    pub mod artifacts;
    pub mod comparison;
    pub mod database;
    pub mod federation;
    pub mod harness;
    pub mod normalization;
    pub mod read_only;
    pub mod safety;
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
use differential::federation::run_federation_discovery_case;
use differential::harness::{RequestSpec, send_identically};
use differential::read_only::ReadOnlyGuard;
use differential::safety::{DifferentialConfig, HttpTargets};
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
    BearerAuthenticator, OAuthAuthenticationError, READ_ACCOUNTS, READ_FOLLOWS, READ_NOTIFICATIONS,
    READ_STATUSES, Repository, RequiredScopes,
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
    let authenticator = BearerAuthenticator::new(repository.clone());
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
    let app = web_router(production_state).merge(
        Router::new()
            .route(
                "/api/v1/featured_tags/suggestions",
                get(fixture_featured_tag_suggestions),
            )
            .with_state(authenticator),
    );
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
    let fixture_routes = Router::new()
        .route("/api/v1/notifications", get(fixture_notifications))
        .route("/api/v2/notifications", get(fixture_grouped_notifications))
        .with_state(state);
    let app = web_router(production_state).merge(fixture_routes);
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

    let result = run_federation_discovery_case(config, &rust_url).await;
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

async fn fixture_featured_tag_suggestions(
    State(authenticator): State<BearerAuthenticator>,
    headers: AxumHeaderMap,
) -> Response {
    fixture_authenticated_response(authenticator, headers, READ_ACCOUNTS, b"[]").await
}

async fn fixture_authenticated_response(
    authenticator: BearerAuthenticator,
    headers: AxumHeaderMap,
    required: RequiredScopes,
    success_body: &'static [u8],
) -> Response {
    let mut response = match authenticator.authenticate(&headers, required).await {
        Ok(authenticated) => match authenticated.require_user() {
            Ok(_) => Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "application/json; charset=utf-8")
                .header(CACHE_CONTROL, "private, no-store")
                .body(Body::from(success_body))
                .expect("static OAuth fixture response is valid"),
            Err(error) => error.into_http_response().map(Body::from),
        },
        Err(OAuthAuthenticationError::OAuth(error)) => error.into_http_response().map(Body::from),
        Err(OAuthAuthenticationError::Repository(_)) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
            .body(Body::from(r#"{"error":"OAuth token lookup failed"}"#))
            .expect("static OAuth fixture error is valid"),
    };
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Authorization, Origin"));
    response
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
        vapid_public_key:
            "BB37UCyc8LLX4PNQSe-04vSFvpUWGrENubUaslVFM_l5TxcGVMY0C3RXPeUJAQHKYlcOM2P4vTYmkoo0VZGZTM4="
                .to_owned(),
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

async fn fixture_notifications(
    State(state): State<CoreRestFixture>,
    RawQuery(query): RawQuery,
    headers: AxumHeaderMap,
) -> Response {
    let authenticated = match state
        .authenticator
        .authenticate(&headers, READ_NOTIFICATIONS)
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
    let requested_types = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .filter(|(name, _)| name == "types[]" || name == "types")
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    let include_filtered = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .any(|(name, value)| name == "include_filtered" && value == "true");
    let supported_types = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .filter(|(name, _)| name == "supported_types[]" || name == "supported_types")
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    let supported_types = (!supported_types.is_empty()).then_some(supported_types);
    let loader = RestProjectionLoader::new(
        state.repository,
        Some(owner.account_id()),
        "fixture-v4-6-5.rustodon.invalid",
    );
    let Ok(notifications) = loader
        .notifications(owner.account_id(), &requested_types, include_filtered)
        .await
    else {
        return fixture_internal_error();
    };
    let serializer = RestSerializer::new(
        &state.origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    let notifications = notifications
        .iter()
        .map(|notification| serializer.notification(notification, supported_types.as_deref()))
        .collect::<Result<Vec<_>, _>>();
    match notifications.and_then(|notifications| {
        serde_json::to_vec(&notifications)
            .map_err(|_| rustodon::mastodon::rest::RestError::InvalidRemoteUrl(owner.account_id()))
    }) {
        Ok(body) => fixture_json_response(StatusCode::OK, body),
        Err(_) => fixture_internal_error(),
    }
}

async fn fixture_grouped_notifications(
    State(state): State<CoreRestFixture>,
    RawQuery(query): RawQuery,
    headers: AxumHeaderMap,
) -> Response {
    let authenticated = match state
        .authenticator
        .authenticate(&headers, READ_NOTIFICATIONS)
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
    let requested_types = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .filter(|(name, _)| name == "types[]" || name == "types")
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    let include_filtered = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .any(|(name, value)| name == "include_filtered" && value == "true");
    let partial_avatars = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .any(|(name, value)| name == "expand_accounts" && value == "partial_avatars");
    let supported_types = query
        .as_deref()
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .filter(|(name, _)| name == "supported_types[]" || name == "supported_types")
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    let supported_types = (!supported_types.is_empty()).then_some(supported_types);
    let loader = RestProjectionLoader::new(
        state.repository,
        Some(owner.account_id()),
        "fixture-v4-6-5.rustodon.invalid",
    );
    let Ok(grouped) = loader
        .grouped_notifications(owner.account_id(), &requested_types, include_filtered)
        .await
    else {
        return fixture_internal_error();
    };
    let serializer = RestSerializer::new(
        &state.origin,
        "fixture-v4-6-5.rustodon.invalid",
        "/system",
        chrono::Utc::now().naive_utc(),
    );
    match serializer
        .grouped_notifications(&grouped, partial_avatars, supported_types.as_deref())
        .and_then(|grouped| {
            serde_json::to_vec(&grouped).map_err(|_| {
                rustodon::mastodon::rest::RestError::InvalidRemoteUrl(owner.account_id())
            })
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
    for (label, path, query, token) in [
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
    let mut search_preflight_headers = stable_request_headers();
    search_preflight_headers.insert(ORIGIN, HeaderValue::from_static("https://client.example"));
    search_preflight_headers.insert(
        ACCESS_CONTROL_REQUEST_METHOD,
        HeaderValue::from_static("GET"),
    );
    search_preflight_headers.insert(
        ACCESS_CONTROL_REQUEST_HEADERS,
        HeaderValue::from_static("authorization"),
    );
    let request = RequestSpec::new(
        Method::OPTIONS,
        "/api/v1/accounts/search",
        None,
        search_preflight_headers,
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
    .map_err(|error| format!("account search CORS preflight: {error}"))?;
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
    for path in ["/api/v1/accounts/familiar_followers"] {
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
    guard.finish().await
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
    const CANARIES: [&[u8]; 11] = [
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
