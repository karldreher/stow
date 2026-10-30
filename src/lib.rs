//! stow: a minimal local in-memory key-value server for agent coordination.
//!
//! # Architecture
//!
//! Control flows in one direction: [`cli`] → [`server`] → store.
//!
//! - [`cli`] parses command-line arguments into typed structs.
//! - [`server`] binds a loopback TCP listener and serves the HTTP/JSON API
//!   under `/api/v1/`.
//! - The store (added by a later issue) holds the key-value state.
//!
//! # Concurrency model
//!
//! The binary runs on a single-threaded tokio runtime (`current_thread`).
//! Concurrency comes from async I/O. The store will sit behind one lock, so
//! every mutation is serialized and therefore linearizable.
//!
//! # Extension points
//!
//! - KV routes are added to [`server::router`].
//! - `--save-file` persistence runs from [`server::on_shutdown`].
//! - `--openapi` adds a route in [`server::router`] and a flag in
//!   [`cli::ServeArgs`].

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(clippy::missing_docs_in_private_items)]

pub mod cli;
pub mod server;
