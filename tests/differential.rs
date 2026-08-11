mod differential {
    pub mod artifacts;
    pub mod comparison;
    pub mod database;
    pub mod harness;
    pub mod normalization;
    pub mod safety;
}

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use differential::artifacts::{MediaSnapshot, compare_media, compare_media_with_labels};
use differential::comparison::{DEFAULT_MISMATCH_LIMIT, compare_responses};
use differential::database::{
    TableSelection, compare_database_snapshots, compare_database_snapshots_with_labels,
    snapshot_database,
};
use differential::harness::{RequestSpec, send_identically};
use differential::safety::{DifferentialConfig, HttpTargets};
use reqwest::Method;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HOST, HeaderMap, HeaderValue};
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

async fn fixture_instance(State(body): State<Arc<Vec<u8>>>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(body.as_ref().clone()))
        .expect("the checked fixture response is valid")
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
