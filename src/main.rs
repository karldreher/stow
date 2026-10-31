//! Thin binary entry point: parses arguments and starts the server.

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(clippy::missing_docs_in_private_items)]

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use clap::Parser;
use stow::cli::{Cli, Command, ServeArgs};
use stow::client;
use stow::persist;
use stow::server::{self, ServerError};
use stow::store::Store;

/// Parses arguments and runs the selected command.
///
/// `serve` failures exit 1. Client commands use the exit codes documented on
/// [`client::ExitStatus`].
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut stdout = std::io::stdout();
    let status = match cli.command {
        Command::Serve(args) => {
            return match serve(args, cli.silent).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Get(args) => client::get(args, &mut stdout).await,
        Command::Set(args) => client::set(args).await,
        Command::Del(args) => client::del(args).await,
        Command::Keys(args) => client::keys(args, &mut stdout).await,
        Command::Lock(args) => client::lock(args, &mut stdout).await,
        Command::Unlock(args) => client::unlock(args).await,
    };
    status.into()
}

/// Runs `stow serve`.
///
/// The save file is loaded before binding so an unreadable file stops startup
/// before anything could overwrite it at shutdown.
async fn serve(args: ServeArgs, silent: bool) -> Result<(), ServerError> {
    let store = Arc::new(Store::default());
    if let Some(path) = &args.save_file {
        match persist::load(path, &store, Instant::now())? {
            None if !silent => {
                eprintln!("no save file at {}, starting empty", path.display());
            }
            Some(count) if !silent => {
                eprintln!("loaded {count} entries from {}", path.display());
            }
            _ => {}
        }
    }
    let listener = server::bind(SocketAddr::new(args.host, args.port)).await?;
    server::serve(
        listener,
        store,
        server::shutdown_signal(),
        args.max_value_size,
        args.openapi,
        args.save_file,
        silent,
    )
    .await
}
