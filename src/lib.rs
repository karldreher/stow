//! stow: a minimal local in-memory key-value server for process coordination.
//!
//! # Architecture
//!
//! Control flows in one direction: [`cli`] → [`server`] → [`store`]. The
//! client subcommands branch off after [`cli`] and talk to a running server
//! over HTTP instead.
//!
//! - [`cli`] parses command-line arguments into typed structs.
//! - [`client`] implements `get`, `set`, `del`, `keys`, `lock`, and `unlock`
//!   as HTTP requests against a running server, including the polling loops
//!   behind `get --wait-for` and `lock --wait`.
//! - [`server`] binds a loopback TCP listener and serves the HTTP/JSON API
//!   under `/api/v1/`.
//! - [`persist`] reads and writes the optional `--save-file`.
//! - [`store`] holds the key-value state in memory.
//!
//! # Concurrency model
//!
//! The binary runs on a single-threaded tokio runtime (`current_thread`).
//! Concurrency comes from async I/O. The store sits behind one lock, so
//! every mutation is serialized and therefore linearizable.
//!
//! # Extension points
//!
//! - New routes are added to [`server::router`].
//! - `--save-file` is loaded in `main` before binding and saved from
//!   [`server::on_shutdown`].
//! - New client subcommands add a variant to [`cli::Command`] and a function in
//!   [`client`].
//! - `--openapi` adds a route in [`server::router`] and a flag in
//!   [`cli::ServeArgs`].

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(clippy::missing_docs_in_private_items)]

pub mod cli;
pub mod client;
pub mod persist;
pub mod server;
pub mod store;
