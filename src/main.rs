#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use rustodon::config::Config;
use rustodon::config::PaperclipRootUrl;
use rustodon::jobs::{Queue, connect_pool};
use rustodon::mastodon::Repository;
use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::operational_schema;
use rustodon::preflight;
use rustodon::startup;
use rustodon::web::{self, WebState};
use rustodon::worker::{infrastructure_handlers, run_until_shutdown};
use sqlx::{Connection, PgConnection};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Parser)]
#[command(version, about = "A mostly-in-place Mastodon replacement")]
struct Cli {
    #[command(subcommand)]
    command: ProcessMode,
}

#[derive(Debug, Subcommand)]
enum ProcessMode {
    /// Serve HTTP API, web, federation, and streaming requests
    Web,
    /// Process durable background and scheduled work
    Worker,
    /// Run administrative and compatibility commands
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Validate a Mastodon cutover without mutating its data or media
    Preflight,
}

#[derive(Debug, Subcommand)]
enum AdminCommand {
    /// Create or upgrade the separately owned Rustodon operational schema
    MigrateOperationalSchema,
    /// Report worker lane coverage, scheduler liveness, queue depth, and dead letters
    WorkerReadiness,
    /// List bounded dead-letter metadata without job arguments
    DeadJobs {
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        ProcessMode::Web => return run_web().await,
        ProcessMode::Worker => return run_worker().await,
        ProcessMode::Admin { command } => return run_admin(command).await,
        ProcessMode::Preflight => return run_preflight().await,
    }
}

async fn run_admin(command: AdminCommand) -> ExitCode {
    let config = match Config::from_process_environment() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("admin configuration failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Ok(options) = preflight::postgres_options(&config) else {
        eprintln!("admin database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(mut connection) = PgConnection::connect_with(&options).await else {
        eprintln!("admin database connection failed");
        return ExitCode::FAILURE;
    };
    match command {
        AdminCommand::MigrateOperationalSchema => {
            if let Err(error) = operational_schema::migrate(&mut connection).await {
                eprintln!("operational schema migration failed: {error}");
                return ExitCode::FAILURE;
            }
            eprintln!(
                "Rustodon operational schema is at version {}",
                operational_schema::CURRENT_VERSION
            );
        }
        AdminCommand::WorkerReadiness => {
            let Ok(pool) = connect_pool(options, config.database.pool_size).await else {
                eprintln!("worker readiness database connection failed");
                return ExitCode::FAILURE;
            };
            let queue = Queue::new(pool);
            let freshness = chrono::Duration::seconds(
                i64::from(config.worker.heartbeat_seconds).saturating_mul(3),
            );
            let readiness = match queue.readiness(&config.worker.lanes, freshness).await {
                Ok(readiness) => readiness,
                Err(error) => {
                    eprintln!("worker readiness inspection failed: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let lanes = readiness
                .missing_lanes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            eprintln!(
                "worker readiness: ready={} scheduler_alive={} missing_lanes={} queued_jobs={} dead_letters={} oldest_queued_at={}",
                readiness.ready(),
                readiness.scheduler_alive,
                lanes,
                readiness.queued_jobs,
                readiness.dead_letters,
                readiness
                    .oldest_queued_at
                    .map_or_else(|| "none".to_owned(), |value| value.to_rfc3339()),
            );
            if !readiness.ready() {
                return ExitCode::FAILURE;
            }
        }
        AdminCommand::DeadJobs { limit } => {
            let Ok(pool) = connect_pool(options, config.database.pool_size).await else {
                eprintln!("dead-letter database connection failed");
                return ExitCode::FAILURE;
            };
            let queue = Queue::new(pool);
            let dead = match queue.dead_letters(i64::from(limit)).await {
                Ok(dead) => dead,
                Err(error) => {
                    eprintln!("dead-letter inspection failed: {error}");
                    return ExitCode::FAILURE;
                }
            };
            for job in dead {
                eprintln!(
                    "dead job: id={} lane={} kind={} attempts={}/{} dead_at={} last_error={}",
                    job.id,
                    job.lane,
                    job.kind,
                    job.attempts,
                    job.max_attempts,
                    job.dead_at.to_rfc3339(),
                    job.last_error.as_deref().unwrap_or("none"),
                );
            }
        }
    }
    ExitCode::SUCCESS
}

async fn run_worker() -> ExitCode {
    let config = match Config::from_process_environment() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("worker configuration failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Ok(options) = preflight::postgres_options(&config) else {
        eprintln!("worker database configuration failed");
        return ExitCode::FAILURE;
    };
    let report = startup::validate(&config).await;
    if !report.is_success() {
        eprintln!("{report}");
        return ExitCode::FAILURE;
    }
    let required_connections = config.worker.concurrency.saturating_add(3);
    let pool_size = config.database.pool_size.max(required_connections);
    let Ok(pool) = connect_pool(options, pool_size).await else {
        eprintln!("worker pool connection failed");
        return ExitCode::FAILURE;
    };
    let queue = Queue::new(pool);
    let handlers = match infrastructure_handlers(&queue) {
        Ok(handlers) => handlers,
        Err(error) => {
            eprintln!("worker handler configuration failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let process_id = worker_process_id();
    if let Err(error) = run_until_shutdown(
        queue,
        handlers,
        config.worker,
        process_id,
        shutdown_signal(),
    )
    .await
    {
        eprintln!("worker failed: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn worker_process_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    format!("{}-{timestamp}", std::process::id())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            result = tokio::signal::ctrl_c() => { let _ = result; }
            value = terminate.recv() => { let _ = value; }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn run_web() -> ExitCode {
    let config = match Config::from_process_environment() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("web configuration failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Ok(options) = preflight::postgres_options(&config) else {
        eprintln!("web database configuration failed");
        return ExitCode::FAILURE;
    };
    let report = startup::validate(&config).await;
    if !report.is_success() {
        eprintln!("{report}");
        return ExitCode::FAILURE;
    }
    let Ok(repository) = Repository::connect_with(options).await else {
        eprintln!("web database connection failed");
        return ExitCode::FAILURE;
    };
    let media_root_url = match &config.paperclip.root_url {
        PaperclipRootUrl::RootRelative(value) => value.clone(),
        PaperclipRootUrl::Absolute(value) => value.to_string(),
    };
    let public_vapid_key = config
        .secrets
        .vapid
        .as_ref()
        .map_or_else(String::new, |vapid| {
            vapid.public_key.expose_secret().to_owned()
        });
    let domain = config.domains.web_domain.clone();
    let mut allowed_hosts = vec![
        config.domains.web_domain.clone(),
        config.domains.local_domain.clone(),
    ];
    allowed_hosts.extend(config.domains.alternate_domains.iter().cloned());
    if let PaperclipRootUrl::Absolute(url) = &config.paperclip.root_url {
        let media_authority = url.port().map_or_else(
            || url.host_str().unwrap_or_default().to_owned(),
            |port| format!("{}:{port}", url.host_str().unwrap_or_default()),
        );
        allowed_hosts.push(media_authority);
    }
    allowed_hosts.sort();
    allowed_hosts.dedup();
    let runtime = InstanceRuntimeConfig {
        domain: domain.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        source_url: "https://github.com/rustodon/rustodon".to_owned(),
        streaming_api: format!("wss://{domain}"),
        vapid_public_key: public_vapid_key,
        thumbnail_url: config
            .domains
            .canonical_origin
            .join("packs/assets/preview.png")
            .map_or_else(
                |_| format!("https://{domain}/packs/assets/preview.png"),
                |url| url.to_string(),
            ),
        thumbnail_description: String::new(),
        thumbnail_blurhash: None,
        thumbnail_versions: None,
        icons: Vec::new(),
        languages: vec!["en".to_owned()],
        active_month: 0,
        active_halfyear: 0,
        translation_enabled: false,
        limited_federation: false,
        single_user_mode: false,
        terms_of_service_url: None,
        sso_signup_url: None,
        wrapstodon: None,
    };
    let Ok(state) = WebState::new(
        repository,
        config.domains.canonical_origin.clone(),
        config.domains.local_domain,
        media_root_url,
        config.paperclip.root_path,
        runtime,
        config.trusted_proxies,
        allowed_hosts,
    ) else {
        eprintln!("web media root could not be opened safely");
        return ExitCode::FAILURE;
    };
    let address = SocketAddr::new(config.web.bind, config.web.port);
    if web::serve(address, state, shutdown_signal()).await.is_err() {
        eprintln!("web server failed");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run_preflight() -> ExitCode {
    let config = match Config::from_process_environment() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("preflight configuration failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let report = preflight::run(&config).await;
    eprintln!("{report}");
    if report.is_success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
