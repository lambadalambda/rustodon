#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use rustodon::bootstrap::{self, BootstrapOutcome, BootstrapRequest};
use rustodon::config::Config;
use rustodon::config::PaperclipRootUrl;
use rustodon::crypto::ActiveRecordEncryptionConfig;
use rustodon::jobs::{Queue, connect_pool};
use rustodon::mail::MailConfig;
use rustodon::mastodon::rest::InstanceRuntimeConfig;
use rustodon::mastodon::{Repository, WriteRepository, random_auth_token};
use rustodon::operational_schema;
use rustodon::paperclip::PaperclipRoot;
use rustodon::preflight;
use rustodon::startup;
use rustodon::web::{self, WebState};
use rustodon::worker::{
    ActivityPubDeliveryConfig, infrastructure_handlers_with_writer_and_mail_and_federation,
    run_until_shutdown,
};
use sqlx::{Connection, PgConnection};
use std::io::Read;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroize::Zeroize;

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
    /// Refresh one existing remote account by its stored canonical actor ID and queue cache repair
    RefreshRemoteAccount {
        #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
        account_id: i64,
    },
    /// Create or upgrade the separately owned Rustodon operational schema
    MigrateOperationalSchema {
        /// Writer role whose operational grants should be normalized without exposing credentials
        #[arg(long)]
        writer_role: Option<String>,
    },
    /// Initialize or verify a standalone instance in a fresh `PostgreSQL` 14 database
    BootstrapInstance {
        #[arg(long)]
        admin_username: String,
        #[arg(long)]
        admin_email: String,
        #[arg(long, default_value = "Rustodon")]
        site_title: String,
        /// Existing least-privilege NOINHERIT login role used by web and worker processes
        #[arg(long)]
        runtime_role: String,
        /// Existing least-privilege login role used for Mastodon-compatible writes
        #[arg(long)]
        writer_role: String,
        /// Read the first Owner password from stdin when omitted to avoid process-list exposure
        #[arg(long)]
        password: Option<String>,
    },
    /// Report worker lane coverage, scheduler liveness, queue depth, and dead letters
    WorkerReadiness,
    /// List bounded dead-letter metadata without job arguments
    DeadJobs {
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
    },
    /// Replace an existing user's password and revoke their sessions/tokens
    ResetPassword {
        #[arg(long)]
        email: String,
        /// Read the password from stdin when omitted to avoid process-list exposure
        #[arg(long)]
        password: Option<String>,
    },
    /// Create a confirmed local user, or queue confirmation mail when SMTP is configured
    CreateUser {
        #[arg(long)]
        email: String,
        #[arg(long)]
        username: String,
        /// Read the password from stdin when omitted to avoid process-list exposure
        #[arg(long)]
        password: Option<String>,
    },
    /// Resolve a report, or reopen it when --reopen is supplied
    ResolveReport {
        #[arg(long)]
        report_id: i64,
        #[arg(long)]
        actor_account_id: i64,
        #[arg(long)]
        reopen: bool,
    },
    /// Delete a local status on behalf of an account with manage-reports permission
    DeleteStatus {
        #[arg(long)]
        status_id: i64,
        #[arg(long)]
        actor_account_id: i64,
        #[arg(long)]
        delete_media: bool,
    },
    /// Recompute one account's denormalized counters on behalf of an administrator
    ReconcileAccountStats {
        #[arg(long)]
        account_id: i64,
        #[arg(long)]
        actor_account_id: i64,
    },
    /// Suspend a local or remote account on behalf of a moderator
    SuspendAccount {
        #[arg(long)]
        account_id: i64,
        #[arg(long)]
        actor_account_id: i64,
    },
    /// Remove a local moderation suspension from an account
    UnsuspendAccount {
        #[arg(long)]
        account_id: i64,
        #[arg(long)]
        actor_account_id: i64,
    },
    /// Create or update a global domain policy (0=silence, 1=suspend, 2=noop)
    BlockDomain {
        #[arg(long)]
        domain: String,
        #[arg(long, value_parser = clap::value_parser!(i32).range(0..=2))]
        severity: i32,
        #[arg(long)]
        reject_media: bool,
        #[arg(long)]
        reject_reports: bool,
        #[arg(long)]
        actor_account_id: i64,
    },
    /// Remove a global domain policy
    UnblockDomain {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        actor_account_id: i64,
    },
    /// Queue a full purge of all remote accounts and custom emoji for a domain
    PurgeDomain {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        actor_account_id: i64,
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

#[allow(clippy::too_many_lines)]
async fn run_admin(command: AdminCommand) -> ExitCode {
    let config = match Config::from_process_environment() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("admin configuration failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let AdminCommand::RefreshRemoteAccount { account_id } = &command {
        return run_admin_refresh_remote_account(&config, *account_id).await;
    }
    if let AdminCommand::ResolveReport {
        report_id,
        actor_account_id,
        reopen,
    } = &command
    {
        return run_admin_resolve_report(&config, *report_id, *actor_account_id, !*reopen).await;
    }
    if let AdminCommand::DeleteStatus {
        status_id,
        actor_account_id,
        delete_media,
    } = &command
    {
        return run_admin_delete_status(&config, *status_id, *actor_account_id, *delete_media)
            .await;
    }
    if let AdminCommand::ReconcileAccountStats {
        account_id,
        actor_account_id,
    } = &command
    {
        return run_admin_reconcile_account_stats(&config, *account_id, *actor_account_id).await;
    }
    if let AdminCommand::SuspendAccount {
        account_id,
        actor_account_id,
    } = &command
    {
        return run_admin_set_account_suspension(&config, *account_id, *actor_account_id, true)
            .await;
    }
    if let AdminCommand::UnsuspendAccount {
        account_id,
        actor_account_id,
    } = &command
    {
        return run_admin_set_account_suspension(&config, *account_id, *actor_account_id, false)
            .await;
    }
    if let AdminCommand::BlockDomain {
        domain,
        severity,
        reject_media,
        reject_reports,
        actor_account_id,
    } = &command
    {
        return run_admin_block_domain(
            &config,
            domain,
            *severity,
            *reject_media,
            *reject_reports,
            *actor_account_id,
        )
        .await;
    }
    if let AdminCommand::UnblockDomain {
        domain,
        actor_account_id,
    } = &command
    {
        return run_admin_unblock_domain(&config, domain, *actor_account_id).await;
    }
    if let AdminCommand::PurgeDomain {
        domain,
        actor_account_id,
    } = &command
    {
        return run_admin_purge_domain(&config, domain, *actor_account_id).await;
    }
    let Ok(options) = preflight::postgres_options(&config) else {
        eprintln!("admin database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(mut connection) = PgConnection::connect_with(&options).await else {
        eprintln!("admin database connection failed");
        return ExitCode::FAILURE;
    };
    match command {
        AdminCommand::MigrateOperationalSchema { writer_role } => {
            let writer_role = writer_role.or_else(|| {
                config
                    .write_database
                    .as_ref()
                    .and_then(preflight::postgres_username_for)
            });
            if let Err(error) = operational_schema::migrate_with_writer_role(
                &mut connection,
                writer_role.as_deref(),
            )
            .await
            {
                eprintln!("operational schema migration failed: {error}");
                return ExitCode::FAILURE;
            }
            eprintln!(
                "Rustodon operational schema is at version {}",
                operational_schema::CURRENT_VERSION
            );
        }
        AdminCommand::BootstrapInstance {
            admin_username,
            admin_email,
            site_title,
            runtime_role,
            writer_role,
            password,
        } => {
            let mut password = match admin_password(password) {
                Ok(password) => password,
                Err(message) => {
                    eprintln!("{message}");
                    return ExitCode::FAILURE;
                }
            };
            let result = bootstrap::bootstrap_instance(
                &mut connection,
                &BootstrapRequest {
                    admin_username: &admin_username,
                    admin_email: &admin_email,
                    admin_password: &password,
                    site_title: &site_title,
                    runtime_role: &runtime_role,
                    writer_role: &writer_role,
                    media_root: &config.paperclip.root_path,
                },
            )
            .await;
            password.zeroize();
            match result {
                Ok(BootstrapOutcome::Installed) => {
                    eprintln!("standalone Rustodon instance initialized");
                }
                Ok(BootstrapOutcome::Verified) => {
                    eprintln!("standalone Rustodon instance already initialized and verified");
                }
                Err(error) => {
                    eprintln!("standalone instance bootstrap failed: {error}");
                    return ExitCode::FAILURE;
                }
            }
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
            if config.write_database.is_some()
                && !rustodon::worker::local_uploads::ready(&queue, freshness)
                    .await
                    .unwrap_or(false)
            {
                eprintln!("worker readiness: local upload processor is not ready");
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
        AdminCommand::ResetPassword { email, password } => {
            return run_admin_reset_password(&config, &email, password).await;
        }
        AdminCommand::CreateUser {
            email,
            username,
            password,
        } => {
            return run_admin_create_user(&config, &email, &username, password).await;
        }
        AdminCommand::RefreshRemoteAccount { .. } => unreachable!("remote refresh handled above"),
        AdminCommand::ResolveReport { .. } => unreachable!("report resolution handled above"),
        AdminCommand::DeleteStatus { .. } => unreachable!("status deletion handled above"),
        AdminCommand::ReconcileAccountStats { .. } => {
            unreachable!("account reconciliation handled above")
        }
        AdminCommand::SuspendAccount { .. } => {
            unreachable!("account suspension handled above")
        }
        AdminCommand::UnsuspendAccount { .. } => {
            unreachable!("account unsuspension handled above")
        }
        AdminCommand::BlockDomain { .. } => unreachable!("domain blocking handled above"),
        AdminCommand::UnblockDomain { .. } => unreachable!("domain unblocking handled above"),
        AdminCommand::PurgeDomain { .. } => unreachable!("domain purge handled above"),
    }
    ExitCode::SUCCESS
}

async fn run_admin_refresh_remote_account(config: &Config, account_id: i64) -> ExitCode {
    let database = config.write_database.as_ref().unwrap_or(&config.database);
    let Ok(options) = preflight::postgres_options_for(database) else {
        eprintln!("remote refresh database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(pool) = connect_pool(options, database.pool_size).await else {
        eprintln!("remote refresh database connection failed");
        return ExitCode::FAILURE;
    };
    // This command only refreshes metadata and enqueues work. The worker opens the
    // media root and performs bounded downloads; operator CLI never writes files.
    let federation = ActivityPubDeliveryConfig {
        origin: config.domains.canonical_origin.clone(),
        local_domain: config.domains.local_domain.clone(),
        media_root_url: String::new(),
        media_root: None,
        limited_federation: config.limited_federation,
        #[cfg(feature = "test-support")]
        remote_media_endpoint: None,
        #[cfg(feature = "test-support")]
        remote_delivery_endpoint: None,
        #[cfg(feature = "test-support")]
        remote_fetch_endpoint: None,
    };
    match rustodon::worker::refresh_remote_account(pool, &federation, account_id).await {
        Ok(()) => {
            println!(
                "Remote account {account_id} refreshed; profile cache checks queued for the worker"
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("remote refresh failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_admin_resolve_report(
    config: &Config,
    report_id: i64,
    actor_account_id: i64,
    resolved: bool,
) -> ExitCode {
    let database = config.write_database.as_ref().unwrap_or(&config.database);
    let Ok(options) = preflight::postgres_options_for(database) else {
        eprintln!("report resolution database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(writer) = WriteRepository::connect_with_pool_size(options, database.pool_size).await
    else {
        eprintln!("report resolution database connection failed");
        return ExitCode::FAILURE;
    };
    match writer
        .set_report_resolution(actor_account_id, report_id, resolved)
        .await
    {
        Ok(()) => {
            let action = if resolved { "resolved" } else { "reopened" };
            eprintln!("report {report_id} {action} by account {actor_account_id}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("report resolution failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_admin_delete_status(
    config: &Config,
    status_id: i64,
    actor_account_id: i64,
    delete_media: bool,
) -> ExitCode {
    let database = config.write_database.as_ref().unwrap_or(&config.database);
    let Ok(options) = preflight::postgres_options_for(database) else {
        eprintln!("status deletion database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(writer) = WriteRepository::connect_with_pool_size(options, database.pool_size).await
    else {
        eprintln!("status deletion database connection failed");
        return ExitCode::FAILURE;
    };
    let removed_media = match writer
        .delete_status_as_moderator(actor_account_id, status_id, delete_media)
        .await
    {
        Ok(media) => media,
        Err(error) => {
            eprintln!("status deletion failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "status {status_id} deleted by account {actor_account_id}; removed_media={}",
        removed_media.len()
    );
    ExitCode::SUCCESS
}

async fn run_admin_reconcile_account_stats(
    config: &Config,
    account_id: i64,
    actor_account_id: i64,
) -> ExitCode {
    let database = config.write_database.as_ref().unwrap_or(&config.database);
    let Ok(options) = preflight::postgres_options_for(database) else {
        eprintln!("account reconciliation database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(writer) = WriteRepository::connect_with_pool_size(options, database.pool_size).await
    else {
        eprintln!("account reconciliation database connection failed");
        return ExitCode::FAILURE;
    };
    match writer
        .reconcile_account_stats(actor_account_id, account_id)
        .await
    {
        Ok(()) => {
            eprintln!(
                "account stats reconciled for account {account_id} by account {actor_account_id}"
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("account reconciliation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_admin_set_account_suspension(
    config: &Config,
    account_id: i64,
    actor_account_id: i64,
    suspended: bool,
) -> ExitCode {
    let database = config.write_database.as_ref().unwrap_or(&config.database);
    let Ok(options) = preflight::postgres_options_for(database) else {
        eprintln!("account suspension database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(writer) = WriteRepository::connect_with_pool_size(options, database.pool_size).await
    else {
        eprintln!("account suspension database connection failed");
        return ExitCode::FAILURE;
    };
    match writer
        .set_account_suspension(
            actor_account_id,
            account_id,
            suspended,
            config.domains.canonical_origin.as_str(),
        )
        .await
    {
        Ok(()) => {
            let action = if suspended {
                "suspended"
            } else {
                "unsuspended"
            };
            eprintln!("account {account_id} {action} by account {actor_account_id}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("account suspension failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_admin_block_domain(
    config: &Config,
    domain: &str,
    severity: i32,
    reject_media: bool,
    reject_reports: bool,
    actor_account_id: i64,
) -> ExitCode {
    let database = config.write_database.as_ref().unwrap_or(&config.database);
    let Ok(options) = preflight::postgres_options_for(database) else {
        eprintln!("domain block database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(writer) = WriteRepository::connect_with_pool_size(options, database.pool_size).await
    else {
        eprintln!("domain block database connection failed");
        return ExitCode::FAILURE;
    };
    match writer
        .set_domain_block(
            actor_account_id,
            domain,
            severity,
            reject_media,
            reject_reports,
            config.domains.canonical_origin.as_str(),
        )
        .await
    {
        Ok(domain_block_id) => {
            eprintln!(
                "domain {domain} blocked as {severity} (row {domain_block_id}) by account {actor_account_id}"
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("domain block failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_admin_unblock_domain(
    config: &Config,
    domain: &str,
    actor_account_id: i64,
) -> ExitCode {
    let database = config.write_database.as_ref().unwrap_or(&config.database);
    let Ok(options) = preflight::postgres_options_for(database) else {
        eprintln!("domain unblock database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(writer) = WriteRepository::connect_with_pool_size(options, database.pool_size).await
    else {
        eprintln!("domain unblock database connection failed");
        return ExitCode::FAILURE;
    };
    match writer.unblock_domain(actor_account_id, domain).await {
        Ok(()) => {
            eprintln!("domain {domain} unblocked by account {actor_account_id}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("domain unblock failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_admin_purge_domain(config: &Config, domain: &str, actor_account_id: i64) -> ExitCode {
    let database = config.write_database.as_ref().unwrap_or(&config.database);
    let Ok(options) = preflight::postgres_options_for(database) else {
        eprintln!("domain purge database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(writer) = WriteRepository::connect_with_pool_size(options, database.pool_size).await
    else {
        eprintln!("domain purge database connection failed");
        return ExitCode::FAILURE;
    };
    match writer.request_domain_purge(actor_account_id, domain).await {
        Ok(()) => {
            eprintln!("domain purge queued for {domain} by account {actor_account_id}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("domain purge failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_admin_reset_password(
    config: &Config,
    email: &str,
    password: Option<String>,
) -> ExitCode {
    let database = config.write_database.as_ref().unwrap_or(&config.database);
    let Ok(options) = preflight::postgres_options_for(database) else {
        eprintln!("password reset database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(writer) = WriteRepository::connect_with_pool_size(options, database.pool_size).await
    else {
        eprintln!("password reset database connection failed");
        return ExitCode::FAILURE;
    };
    let password = match admin_password(password) {
        Ok(password) => password,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    match writer.reset_user_password_by_email(email, &password).await {
        Ok(true) => eprintln!("password reset for {email}"),
        Ok(false) => {
            eprintln!("no user matched {email}");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("password reset failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

async fn run_admin_create_user(
    config: &Config,
    email: &str,
    username: &str,
    password: Option<String>,
) -> ExitCode {
    let database = config.write_database.as_ref().unwrap_or(&config.database);
    let Ok(options) = preflight::postgres_options_for(database) else {
        eprintln!("user creation database configuration failed");
        return ExitCode::FAILURE;
    };
    let Ok(writer) = WriteRepository::connect_with_pool_size(options, database.pool_size).await
    else {
        eprintln!("user creation database connection failed");
        return ExitCode::FAILURE;
    };
    let password = match admin_password(password) {
        Ok(password) => password,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let mail_config = MailConfig::new(
        config.smtp.clone(),
        config.domains.canonical_origin.clone(),
        config.secrets.secret_key_base.clone(),
    );
    if mail_config.is_enabled() && mail_config.runtime().is_err() {
        eprintln!("confirmation mail configuration failed");
        return ExitCode::FAILURE;
    }
    let result = if mail_config.is_enabled() {
        let confirmation_token = random_auth_token(32);
        let confirmation_job = match mail_config.confirmation_job(email, &confirmation_token) {
            Ok(job) => job,
            Err(error) => {
                eprintln!("confirmation mail configuration failed: {error}");
                return ExitCode::FAILURE;
            }
        };
        writer
            .create_local_user_with_confirmation(
                email,
                username,
                &password,
                &confirmation_token,
                Some(config.secrets.secret_key_base.expose_secret()),
                &confirmation_job,
            )
            .await
    } else {
        writer.create_local_user(email, username, &password).await
    };
    match result {
        Ok(user) if user.confirmed => {
            eprintln!(
                "created confirmed local user {} (account {})",
                user.user_id, user.account_id
            );
            ExitCode::SUCCESS
        }
        Ok(user) => {
            eprintln!(
                "created local user {} (account {}); confirmation mail queued",
                user.user_id, user.account_id
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("user creation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn admin_password(password: Option<String>) -> Result<String, &'static str> {
    if let Some(password) = password {
        return Ok(password);
    }
    let mut password = String::new();
    std::io::stdin()
        .read_to_string(&mut password)
        .map_err(|_| "password could not be read from stdin")?;
    Ok(password.trim_end_matches(['\r', '\n']).to_owned())
}

#[allow(clippy::too_many_lines)]
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
    let mastodon_writer = match config.write_database.as_ref() {
        None => None,
        Some(database) => {
            let Ok(options) = preflight::postgres_options_for(database) else {
                eprintln!("worker write database configuration failed");
                return ExitCode::FAILURE;
            };
            if let Ok(pool) = connect_pool(options, database.pool_size).await {
                Some(pool)
            } else {
                eprintln!("worker write database connection failed");
                return ExitCode::FAILURE;
            }
        }
    };
    let mail_config = MailConfig::new(
        config.smtp.clone(),
        config.domains.canonical_origin.clone(),
        config.secrets.secret_key_base.clone(),
    );
    let mail_runtime = match mail_config.runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("worker mail configuration failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let media_root_url = match &config.paperclip.root_url {
        PaperclipRootUrl::RootRelative(value) => value.clone(),
        PaperclipRootUrl::Absolute(value) => value.to_string(),
    };
    let Ok(media_root) = PaperclipRoot::open(&config.paperclip.root_path) else {
        eprintln!("worker media root could not be opened safely");
        return ExitCode::FAILURE;
    };
    let federation = ActivityPubDeliveryConfig {
        origin: config.domains.canonical_origin.clone(),
        local_domain: config.domains.local_domain.clone(),
        media_root_url,
        media_root: Some(media_root),
        limited_federation: config.limited_federation,
        #[cfg(feature = "test-support")]
        remote_media_endpoint: None,
        #[cfg(feature = "test-support")]
        remote_delivery_endpoint: None,
        #[cfg(feature = "test-support")]
        remote_fetch_endpoint: None,
    };
    let poll_expiration_writer = mastodon_writer.clone();
    let handlers = match infrastructure_handlers_with_writer_and_mail_and_federation(
        &queue,
        mastodon_writer,
        mail_runtime,
        Some(federation),
    ) {
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
        poll_expiration_writer,
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

#[allow(clippy::too_many_lines)]
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
    let encryption = ActiveRecordEncryptionConfig::new(
        config.secrets.active_record_encryption.primary_key.clone(),
        config
            .secrets
            .active_record_encryption
            .deterministic_key
            .clone(),
        config
            .secrets
            .active_record_encryption
            .key_derivation_salt
            .clone(),
    )
    .expect("validated Active Record encryption configuration");
    let report = startup::validate(&config).await;
    if !report.is_success() {
        eprintln!("{report}");
        return ExitCode::FAILURE;
    }
    let Ok(queue_pool) = connect_pool(options.clone(), config.database.pool_size).await else {
        eprintln!("web queue connection failed");
        return ExitCode::FAILURE;
    };
    let Ok(repository) = Repository::connect_with(options).await else {
        eprintln!("web database connection failed");
        return ExitCode::FAILURE;
    };
    let repository = repository.with_active_record_encryption(encryption.clone());
    let write_repository = match configured_write_repository(&config, encryption).await {
        Ok(repository) => repository,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let media_root_url = match &config.paperclip.root_url {
        PaperclipRootUrl::RootRelative(value) => value.clone(),
        PaperclipRootUrl::Absolute(value) => value.to_string(),
    };
    let public_vapid_key = config
        .secrets
        .vapid
        .as_ref()
        .map(|vapid| vapid.public_key.expose_secret().to_owned());
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
        source_url: env!("CARGO_PKG_REPOSITORY").to_owned(),
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
        translation_enabled: false,
        limited_federation: config.limited_federation,
        single_user_mode: false,
        terms_of_service_url: None,
        sso_signup_url: None,
        wrapstodon: None,
    };
    let Ok(mut state) = WebState::new(
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
    state = state.with_csrf_signing_secret(&config.secrets.secret_key_base);
    state = state.with_queue(Queue::new(queue_pool));
    let mail_config = MailConfig::new(
        config.smtp.clone(),
        config.domains.canonical_origin.clone(),
        config.secrets.secret_key_base.clone(),
    );
    if mail_config.is_enabled() && mail_config.runtime().is_err() {
        eprintln!("web mail configuration failed");
        return ExitCode::FAILURE;
    }
    state = state.with_mail_config(mail_config);
    if let Some(write_repository) = write_repository {
        state = state.with_write_repository(write_repository);
    }
    let address = SocketAddr::new(config.web.bind, config.web.port);
    if web::serve(address, state, shutdown_signal()).await.is_err() {
        eprintln!("web server failed");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn configured_write_repository(
    config: &Config,
    encryption: ActiveRecordEncryptionConfig,
) -> Result<Option<WriteRepository>, &'static str> {
    let Some(database) = config.write_database.as_ref() else {
        return Ok(None);
    };
    let options = preflight::postgres_options_for(database)
        .map_err(|_| "web write database configuration failed")?;
    let repository = WriteRepository::connect_with_pool_size(options, database.pool_size)
        .await
        .map_err(|_| "web write database connection failed")?
        .with_active_record_encryption(encryption);
    let startup_repair = tokio::time::timeout(
        Duration::from_secs(3),
        repository.repair_missing_account_stats_startup_batch(),
    )
    .await;
    let repair_in_background = match startup_repair {
        Ok(Ok((_, may_have_more))) => may_have_more,
        Ok(Err(error)) => {
            eprintln!("bounded startup account stats repair failed: {error}");
            true
        }
        Err(_) => {
            eprintln!("bounded startup account stats repair timed out");
            true
        }
    };
    if repair_in_background {
        const MAX_BACKGROUND_ACCOUNT_STATS_BATCHES: usize = 24;
        let background_repository = repository.clone();
        let _account_stats_repair = tokio::spawn(async move {
            for batch in 0..MAX_BACKGROUND_ACCOUNT_STATS_BATCHES {
                let repair = tokio::time::timeout(
                    Duration::from_secs(3),
                    background_repository.repair_missing_account_stats_startup_batch(),
                )
                .await;
                match repair {
                    Ok(Ok((_, true))) if batch + 1 < MAX_BACKGROUND_ACCOUNT_STATS_BATCHES => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Ok(Ok((_, true))) => {
                        eprintln!(
                            "background account stats repair reached its startup batch limit; \
                             remaining rows will heal on later startup or mutation"
                        );
                        break;
                    }
                    Ok(Ok((_, false))) => break,
                    Ok(Err(error)) => {
                        eprintln!("background account stats repair failed: {error}");
                        break;
                    }
                    Err(_) => {
                        eprintln!("background account stats repair timed out");
                        break;
                    }
                }
            }
        });
    }
    Ok(Some(repository))
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
