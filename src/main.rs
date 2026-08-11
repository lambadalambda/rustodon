#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use rustodon::config::Config;
use rustodon::preflight;
use std::process::ExitCode;

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
    Admin,
    /// Validate a Mastodon cutover without mutating its data or media
    Preflight,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let message = match cli.command {
        ProcessMode::Web => "web process is not implemented yet",
        ProcessMode::Worker => "worker process is not implemented yet",
        ProcessMode::Admin => "administrative commands are not implemented yet",
        ProcessMode::Preflight => return run_preflight().await,
    };

    eprintln!("{message}");
    ExitCode::FAILURE
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
