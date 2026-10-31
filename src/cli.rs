//! Command-line interface definitions.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Default TCP port the server listens on.
pub const DEFAULT_PORT: u16 = 7319;

/// Default maximum value size in bytes.
pub const DEFAULT_MAX_VALUE_SIZE: usize = 1024;

/// Default `--timeout` for waiting commands, in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Default `--interval` for waiting commands, in milliseconds.
pub const DEFAULT_INTERVAL_MS: u64 = 250;

/// Hard ceiling for `--max-value-size`, in bytes (1 MiB). stow is a
/// coordination primitive, not a blob store, so larger values are refused at
/// startup rather than accepted.
pub const MAX_VALUE_SIZE_CEILING: usize = 1024 * 1024;

/// Top-level `stow` command.
#[derive(Debug, Parser)]
#[command(
    name = "stow",
    version,
    about = "Minimal local in-memory key-value server",
    arg_required_else_help = true,
    after_help = "Examples:\n  stow serve\n  stow serve --port 8080\n  stow serve --host 127.0.0.1 --port 9000\n  stow -q serve\n  stow set greeting hello\n  stow lock lock/build --ttl 60 --wait"
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
    #[command(
        after_help = "Examples:\n  stow serve\n  stow serve --port 8080\n  stow serve --max-value-size 65536\n  stow serve --save-file ./stow.bin\n  stow serve --openapi"
    )]
    Serve(ServeArgs),

    /// Print a value to stdout, optionally waiting for it to change.
    ///
    /// Exits 0 when the value is printed, 1 if the key does not exist, 3 if
    /// the server is unreachable, and 4 if `--wait-for` times out.
    ///
    /// `--wait-for` only observes a key; it never changes it and cannot
    /// acquire anything. To take a mutex, use `stow lock --wait`. Running
    /// `stow get K --wait-for absent && stow lock K ...` is racy: every
    /// waiter wakes together and the losers get a conflict.
    #[command(
        after_help = "Wait conditions:\n  present      the key exists\n  absent       the key does not exist (deleted or expired)\n  value=<v>    the value equals <v> byte for byte\n\nExamples:\n  stow get greeting\n  stow get build-sha --wait-for present\n  stow get task-42/status --wait-for value=done --timeout 300"
    )]
    Get(GetArgs),

    /// Write a value; pass `-` as the value to read it from stdin.
    ///
    /// Exits 0 on success, 2 if `--if-absent` or `--if-value` did not hold,
    /// 1 if `--if-value` was given and the key does not exist, and 3 if the
    /// server is unreachable.
    #[command(
        after_help = "Examples:\n  stow set greeting hello\n  echo hello | stow set greeting -\n  stow set decision plan-b --if-absent\n  stow set session abc --ttl 60"
    )]
    Set(SetArgs),

    /// Delete a key.
    ///
    /// Exits 0 on success, 1 if the key does not exist, 2 if `--if-value` did
    /// not match, and 3 if the server is unreachable.
    #[command(
        after_help = "Examples:\n  stow del greeting\n  stow del lock/build --if-value agent-1"
    )]
    Del(DelArgs),

    /// List keys, one per line.
    #[command(after_help = "Examples:\n  stow keys\n  stow keys --prefix lock/")]
    Keys(KeysArgs),

    /// Take a mutex with a lease and print the owner token.
    ///
    /// The check and the write are one atomic step, so exactly one caller
    /// holds the key. Without `--wait` this makes a single attempt and exits
    /// 2 if someone else holds it. With `--wait` it polls until the key is
    /// free, backing off from `--interval` up to 2000 ms, and exits 4 if
    /// `--timeout` passes first. Without `--owner`, the token is `stow-<pid>`.
    #[command(
        after_help = "Examples:\n  stow lock lock/build --ttl 60\n  stow lock lock/build --ttl 60 --wait\n  stow lock lock/build --ttl 60 --wait --timeout 300 --owner agent-1"
    )]
    Lock(LockArgs),

    /// Release a mutex taken with `stow lock`, only if you still own it.
    ///
    /// Exits 0 on success, 1 if the key no longer exists, and 2 if another
    /// owner holds it.
    #[command(after_help = "Examples:\n  stow unlock lock/build --owner agent-1")]
    Unlock(UnlockArgs),
}

/// Arguments shared by every client subcommand.
#[derive(Debug, Args)]
pub struct ClientArgs {
    /// Address of the stow server.
    #[arg(
        long,
        env = "STOW_ADDR",
        default_value_t = SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT))
    )]
    pub addr: SocketAddr,
}

/// Polling options for commands that can wait.
#[derive(Debug, Args)]
pub struct WaitArgs {
    /// Seconds to wait before giving up; 0 waits indefinitely.
    #[arg(long, value_name = "SECONDS", default_value_t = DEFAULT_TIMEOUT_SECS)]
    pub timeout: u64,

    /// Initial poll interval in milliseconds. It doubles after each poll up
    /// to a cap of 2000 ms.
    #[arg(
        long,
        value_name = "MILLIS",
        default_value_t = DEFAULT_INTERVAL_MS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub interval: u64,
}

/// A condition `stow get --wait-for` blocks on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitFor {
    /// The key exists and has not expired.
    Present,
    /// The key does not exist, because it was deleted or expired.
    Absent,
    /// The current value equals this string byte for byte.
    Value(String),
}

/// Arguments for `stow get`.
#[derive(Debug, Args)]
pub struct GetArgs {
    /// Server to contact.
    #[command(flatten)]
    pub client: ClientArgs,

    /// Key to read.
    pub key: String,

    /// Block until the condition holds: `present`, `absent`, or `value=<v>`.
    #[arg(long, value_name = "CONDITION", value_parser = parse_wait_for)]
    pub wait_for: Option<WaitFor>,

    /// Polling options for `--wait-for`.
    #[command(flatten)]
    pub wait: WaitArgs,
}

/// Arguments for `stow set`.
#[derive(Debug, Args)]
pub struct SetArgs {
    /// Server to contact.
    #[command(flatten)]
    pub client: ClientArgs,

    /// Key to write.
    pub key: String,

    /// Value to store, or `-` to read it from stdin.
    pub value: String,

    /// Expire the key after this many seconds (1 to 86400).
    #[arg(long, value_name = "SECONDS")]
    pub ttl: Option<u64>,

    /// Write only if the key does not exist.
    #[arg(long, conflicts_with = "if_value")]
    pub if_absent: bool,

    /// Write only if the current value equals this string.
    #[arg(long, value_name = "VALUE")]
    pub if_value: Option<String>,
}

/// Arguments for `stow del`.
#[derive(Debug, Args)]
pub struct DelArgs {
    /// Server to contact.
    #[command(flatten)]
    pub client: ClientArgs,

    /// Key to delete.
    pub key: String,

    /// Delete only if the current value equals this string.
    #[arg(long, value_name = "VALUE")]
    pub if_value: Option<String>,
}

/// Arguments for `stow keys`.
#[derive(Debug, Args)]
pub struct KeysArgs {
    /// Server to contact.
    #[command(flatten)]
    pub client: ClientArgs,

    /// List only keys that start with this prefix.
    #[arg(long)]
    pub prefix: Option<String>,
}

/// Arguments for `stow lock`.
#[derive(Debug, Args)]
pub struct LockArgs {
    /// Server to contact.
    #[command(flatten)]
    pub client: ClientArgs,

    /// Key to lock.
    pub key: String,

    /// Lease length in seconds (1 to 86400); the lock frees itself after
    /// this long if the holder never unlocks.
    #[arg(long, value_name = "SECONDS")]
    pub ttl: u64,

    /// Owner token to store as the value. Defaults to `stow-<pid>`.
    #[arg(long, value_name = "TOKEN")]
    pub owner: Option<String>,

    /// Block until the lock is acquired instead of making one attempt.
    #[arg(long)]
    pub wait: bool,

    /// Polling options for `--wait`.
    #[command(flatten)]
    pub wait_args: WaitArgs,
}

/// Arguments for `stow unlock`.
#[derive(Debug, Args)]
pub struct UnlockArgs {
    /// Server to contact.
    #[command(flatten)]
    pub client: ClientArgs,

    /// Key to unlock.
    pub key: String,

    /// Owner token printed by `stow lock`.
    #[arg(long, value_name = "TOKEN")]
    pub owner: String,
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

    /// Largest value, in bytes, a set request may carry, up to 1048576 (1
    /// MiB). Larger values get a 413 response.
    #[arg(long, default_value_t = DEFAULT_MAX_VALUE_SIZE, value_parser = parse_max_value_size)]
    pub max_value_size: usize,

    /// Serve the OpenAPI 3.1 document at `GET /api/v1/openapi.json`.
    #[arg(long)]
    pub openapi: bool,

    /// File to load at startup and write on SIGINT/SIGTERM. Without it, stow
    /// never touches disk. A missing file starts empty; an unreadable one is
    /// an error and is never overwritten.
    #[arg(long, value_name = "PATH")]
    pub save_file: Option<PathBuf>,
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

/// Parses `--max-value-size`, rejecting anything above
/// [`MAX_VALUE_SIZE_CEILING`].
///
/// # Errors
///
/// Returns a message if `value` is not a whole number of bytes or exceeds the
/// 1 MiB ceiling.
fn parse_max_value_size(value: &str) -> Result<usize, String> {
    let size: usize = value.parse().map_err(|err| format!("{err}"))?;
    if size > MAX_VALUE_SIZE_CEILING {
        return Err(format!(
            "{size} is above the {MAX_VALUE_SIZE_CEILING}-byte (1 MiB) limit; stow is for small coordination values, not large payloads"
        ));
    }
    Ok(size)
}

/// Parses `--wait-for`.
///
/// # Errors
///
/// Returns a message listing the accepted forms if `value` is not `present`,
/// `absent`, or `value=<v>`.
fn parse_wait_for(value: &str) -> Result<WaitFor, String> {
    match value {
        "present" => Ok(WaitFor::Present),
        "absent" => Ok(WaitFor::Absent),
        _ => match value.strip_prefix("value=") {
            Some(expected) => Ok(WaitFor::Value(expected.to_owned())),
            None => Err(format!(
                "{value:?} is not a wait condition; use present, absent, or value=<v>"
            )),
        },
    }
}
