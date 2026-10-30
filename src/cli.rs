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
    after_help = "Examples:\n  stow serve\n  stow serve --port 8080\n  stow serve --host 127.0.0.1 --port 9000\n  stow -q serve"
)]
pub struct Cli {
    /// Suppress status messages (startup and shutdown). Errors still print to
    /// stderr and the exit code is unchanged. Accepted before or after the
    /// subcommand.
    #[arg(short = 'q', long = "silent", global = true)]
    pub silent: bool,

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
    /// Address to bind. Loopback by default; the server has no authentication,
    /// so wildcard addresses (0.0.0.0, ::) are rejected.
    #[arg(long, default_value = "127.0.0.1", value_parser = parse_host)]
    pub host: IpAddr,

    /// TCP port to listen on.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub port: u16,
}

/// Parses `--host`, rejecting wildcard addresses.
///
/// # Errors
///
/// Returns a message if `value` is not an IP address or is `0.0.0.0` / `::`,
/// which would expose the unauthenticated server on every interface.
fn parse_host(value: &str) -> Result<IpAddr, String> {
    let ip: IpAddr = value.parse().map_err(|err| format!("{err}"))?;
    if ip.is_unspecified() {
        return Err(format!(
            "{ip} would expose stow on every network interface; stow has no authentication and supports loopback use only"
        ));
    }
    Ok(ip)
}
