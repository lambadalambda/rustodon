#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let message = match cli.command {
        ProcessMode::Web => "web process is not implemented yet",
        ProcessMode::Worker => "worker process is not implemented yet",
        ProcessMode::Admin => "administrative commands are not implemented yet",
    };

    eprintln!("{message}");
    ExitCode::FAILURE
}
