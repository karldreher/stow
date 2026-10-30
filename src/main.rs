//! Thin binary entry point: parses arguments and starts the server.

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(clippy::missing_docs_in_private_items)]

use std::net::SocketAddr;
use std::process::ExitCode;

use clap::Parser;
use stow::cli::{Cli, Command, ServeArgs};
use stow::server::{self, ServerError};

/// Parses arguments, runs the selected command, and maps failure to exit code 1.
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Serve(args) => serve(args).await,
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
async fn serve(args: ServeArgs) -> Result<(), ServerError> {
    let listener = server::bind(SocketAddr::new(args.host, args.port)).await?;
    server::serve(listener, server::shutdown_signal()).await
}
