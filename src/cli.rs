//! Command-line interface definitions.

use std::net::IpAddr;

use clap::{Args, Parser, Subcommand};

/// Default TCP port the server listens on.
pub const DEFAULT_PORT: u16 = 7319;

/// Top-level `stow` command.
#[derive(Debug, Parser)]
#[command(
    name = "stow",
    version,
    about = "Minimal local in-memory key-value server for agent coordination",
    arg_required_else_help = true,
    after_help = "Examples:\n  stow serve\n  stow serve --port 8080\n  stow serve --host 127.0.0.1 --port 9000"
)]
pub struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Subcommands of `stow`.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the HTTP server in the foreground.
    #[command(after_help = "Examples:\n  stow serve\n  stow serve --port 8080")]
    Serve(ServeArgs),
}

/// Arguments for `stow serve`.
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Address to bind. Loopback by default; the server has no authentication.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: IpAddr,

    /// TCP port to listen on.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub port: u16,
}
