//! HTTP server: routing, binding, and graceful shutdown.

use std::fmt;
use std::future::Future;
use std::io;
use std::net::SocketAddr;

use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::net::TcpListener;

/// Failure to start the server.
#[derive(Debug)]
pub enum ServerError {
    /// The requested port is already bound by another process.
    PortInUse(u16),
    /// Any other I/O failure while binding or serving.
    Io(io::Error),
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PortInUse(port) => {
                write!(
                    f,
                    "port {port} is already in use; pass --port to choose another"
                )
            }
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ServerError {}

impl From<io::Error> for ServerError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Body of `GET /api/v1/health`.
#[derive(Debug, Serialize)]
struct Health {
    /// Always `"ok"` while the server can answer requests.
    status: &'static str,
    /// Crate version, so clients can detect which build they talk to.
    version: &'static str,
}

/// Handles `GET /api/v1/health`.
async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Builds the application router.
pub fn router() -> Router {
    Router::new().route("/api/v1/health", get(health))
}

/// Binds a TCP listener on `addr`.
///
/// # Errors
///
/// Returns [`ServerError::PortInUse`] if the port is taken, or
/// [`ServerError::Io`] for any other bind failure.
pub async fn bind(addr: SocketAddr) -> Result<TcpListener, ServerError> {
    TcpListener::bind(addr)
        .await
        .map_err(|err| match err.kind() {
            io::ErrorKind::AddrInUse => ServerError::PortInUse(addr.port()),
            _ => ServerError::Io(err),
        })
}

/// Serves the API on `listener` until `shutdown` completes.
///
/// In-flight requests finish before this returns. [`on_shutdown`] runs after
/// the server has drained. When `silent` is true, the startup and shutdown
/// messages are not printed.
///
/// # Errors
///
/// Returns [`ServerError::Io`] if the listener fails.
pub async fn serve(
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
    silent: bool,
) -> Result<(), ServerError> {
    let addr = listener.local_addr()?;
    if !silent {
        eprintln!("stow listening on http://{addr}");
    }
    axum::serve(listener, router())
        .with_graceful_shutdown(shutdown)
        .await?;
    on_shutdown();
    if !silent {
        eprintln!("stow stopped");
    }
    Ok(())
}

/// Hook that runs once after the server has drained, before the process exits.
///
/// Empty for now; the `--save-file` feature persists the store here.
pub fn on_shutdown() {}

/// Completes when the process receives SIGINT or SIGTERM.
///
/// # Panics
///
/// Panics if the signal handlers cannot be installed.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install SIGINT handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
