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
use stow::persist;
use stow::server::{self, ServerError};
use stow::store::Store;

/// Parses arguments, runs the selected command, and maps failure to exit code 1.
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Serve(args) => serve(args, cli.silent).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
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
