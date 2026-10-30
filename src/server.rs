//! HTTP server: routing, request handling, binding, and graceful shutdown.

use std::fmt;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::rejection::{BytesRejection, JsonRejection, PathRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use utoipa::{IntoParams, OpenApi, ToSchema};

use crate::store::{Condition, Current, DeleteOutcome, MAX_KEY_LEN, SetOutcome, Store};

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
#[derive(Debug, Serialize, ToSchema)]
struct Health {
    /// Always `"ok"` while the server can answer requests.
    status: &'static str,
    /// Crate version, so clients can detect which build they talk to.
    version: &'static str,
}

/// Handles `GET /api/v1/health`.
#[utoipa::path(
    get,
    path = "/api/v1/health",
    responses((status = 200, description = "The server is up.", body = Health))
)]
async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Shared state handed to every request handler.
#[derive(Clone)]
struct AppState {
    /// The key-value store.
    store: Arc<Store>,
    /// Largest accepted value, in bytes.
    max_value_size: usize,
}

/// A failed request, rendered as `{"error":{"code","message"}}`.
#[derive(Debug)]
struct ApiError {
    /// HTTP status of the response.
    status: StatusCode,
    /// Stable machine-readable error code.
    code: &'static str,
    /// Human-readable detail.
    message: String,
    /// The entry that blocked a conditional write, for 409 responses.
    current: Option<CurrentBody>,
}

/// The `current` object of a 409 response.
#[derive(Debug, Serialize, ToSchema)]
struct CurrentBody {
    /// The blocking value; `null` when it is not valid UTF-8 (fetch it with a
    /// plain GET instead).
    value: Option<String>,
    /// Version of the blocking entry.
    version: u64,
    /// Milliseconds until the blocking entry expires; `null` if it has no TTL.
    ttl_remaining_ms: Option<u64>,
}

/// Milliseconds until `current` expires, or `None` if it has no TTL.
fn ttl_remaining_ms(current: &Current) -> Option<u64> {
    // TTLs are capped at MAX_TTL_SECS, so milliseconds always fit in u64.
    current.ttl_remaining.map(|ttl| ttl.as_millis() as u64)
}

impl ApiError {
    /// Builds an error with the given status, code, and message.
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            current: None,
        }
    }

    /// 409 for a failed precondition, carrying the entry that blocked it.
    fn conflict(current: &Current) -> Self {
        let mut err = Self::new(
            StatusCode::CONFLICT,
            "conflict",
            "the key does not satisfy the requested condition",
        );
        err.current = Some(CurrentBody {
            value: String::from_utf8(current.value.to_vec()).ok(),
            version: current.version,
            ttl_remaining_ms: ttl_remaining_ms(current),
        });
        err
    }

    /// Error for a body larger than the configured value limit.
    fn too_large(max_value_size: usize) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            format!("value exceeds the maximum of {max_value_size} bytes"),
        )
    }
}

/// Body of an error response.
#[derive(Serialize, ToSchema)]
struct ErrorEnvelope<'a> {
    /// The error detail.
    error: ErrorBody<'a>,
    /// The blocking entry, present only on 409 responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<&'a CurrentBody>,
}

/// Inner object of `ErrorEnvelope`.
#[derive(Serialize, ToSchema)]
struct ErrorBody<'a> {
    /// Stable machine-readable error code.
    code: &'a str,
    /// Human-readable detail.
    message: &'a str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorEnvelope {
            error: ErrorBody {
                code: self.code,
                message: &self.message,
            },
            current: self.current.as_ref(),
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<PathRejection> for ApiError {
    fn from(err: PathRejection) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_key", err.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(err: QueryRejection) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_query", err.body_text())
    }
}

impl From<BytesRejection> for ApiError {
    fn from(err: BytesRejection) -> Self {
        // Only a body-limit failure reports 413; everything else is a read error.
        if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "request body is too large",
            )
        } else {
            Self::new(StatusCode::BAD_REQUEST, "invalid_body", err.body_text())
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(err: JsonRejection) -> Self {
        match err {
            JsonRejection::BytesRejection(inner) => inner.into(),
            other => Self::new(StatusCode::BAD_REQUEST, "invalid_body", other.body_text()),
        }
    }
}

/// Checks that `key` is 1 to [`MAX_KEY_LEN`] bytes.
///
/// UTF-8 validity is already guaranteed by the `String` the key arrives in.
fn validate_key(key: &str) -> Result<(), ApiError> {
    if key.is_empty() || key.len() > MAX_KEY_LEN {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_key",
            format!("key must be 1 to {MAX_KEY_LEN} bytes"),
        ));
    }
    Ok(())
}

/// Response of both set endpoints.
#[derive(Serialize, ToSchema)]
struct SetResponse {
    /// The key that was written.
    key: String,
    /// Version after the write.
    version: u64,
}

/// Request body of `POST /api/v1/set`.
#[derive(Deserialize, ToSchema)]
struct SetRequest {
    /// Key to write.
    key: String,
    /// Value to store, as UTF-8 text.
    value: String,
}

/// Representation requested through `?format=`.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum Format {
    /// `{"key","value","version"}` instead of the raw bytes.
    Json,
}

/// Longest accepted TTL, in seconds (24 hours).
const MAX_TTL_SECS: u64 = 86_400;

/// Query string of both set endpoints.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct SetQuery {
    /// Lease length in seconds, 1 to 86400 (24 hours); no expiry when absent.
    ttl: Option<u64>,
    /// Write only if the key is absent.
    if_absent: Option<bool>,
    /// Write only if the current value equals this string.
    if_value: Option<String>,
}

impl SetQuery {
    /// Validated lease length.
    fn ttl(&self) -> Result<Option<Duration>, ApiError> {
        match self.ttl {
            None => Ok(None),
            Some(secs @ 1..=MAX_TTL_SECS) => Ok(Some(Duration::from_secs(secs))),
            Some(_) => Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_query",
                format!("ttl must be 1 to {MAX_TTL_SECS} seconds"),
            )),
        }
    }

    /// The precondition requested by `if_absent` / `if_value`.
    fn condition(&self) -> Result<Condition<'_>, ApiError> {
        match (self.if_absent == Some(true), &self.if_value) {
            (true, Some(_)) => Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_query",
                "if_absent and if_value cannot be combined",
            )),
            (true, None) => Ok(Condition::IfAbsent),
            (false, Some(value)) => Ok(Condition::IfValue(value.as_bytes())),
            (false, None) => Ok(Condition::Always),
        }
    }
}

/// Query string of `DELETE /api/v1/delete/{key}`.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct DeleteQuery {
    /// Delete only if the current value equals this string.
    if_value: Option<String>,
}

/// Query string of `GET /api/v1/get/{key}`.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct GetQuery {
    /// Optional response representation; raw bytes when absent.
    format: Option<Format>,
}

/// Body of `GET /api/v1/get/{key}?format=json`.
#[derive(Serialize, ToSchema)]
struct ValueResponse<'a> {
    /// The requested key.
    key: &'a str,
    /// The value, which must be valid UTF-8 to appear here.
    value: &'a str,
    /// Current version of the key.
    version: u64,
    /// Milliseconds until the key expires; `null` if it has no TTL.
    ttl_remaining_ms: Option<u64>,
}

/// Query string of `GET /api/v1/keys`.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct KeysQuery {
    /// Only keys starting with this prefix are listed.
    prefix: Option<String>,
}

/// Body of `DELETE /api/v1/delete/{key}`.
#[derive(Serialize, ToSchema)]
struct DeleteResponse {
    /// Always `true`; failures use the error envelope instead.
    deleted: bool,
}

/// Body of `GET /api/v1/keys`.
#[derive(Serialize, ToSchema)]
struct KeysResponse {
    /// Live keys matching the prefix.
    keys: Vec<String>,
}

/// Handles `GET /api/v1/get/{*key}`.
#[utoipa::path(
    get,
    path = "/api/v1/get/{key}",
    params(
        GetQuery,
        ("key" = String, Path, description = "Key of 1 to 32 bytes; may contain `/`.")
    ),
    responses(
        (
            status = 200,
            description = "The value: raw bytes, or JSON when `format=json`.",
            content(
                (String = "application/octet-stream"),
                (ValueResponse = "application/json")
            )
        ),
        (status = 400, description = "Invalid key or query.", body = ErrorEnvelope),
        (status = 404, description = "The key does not exist.", body = ErrorEnvelope),
        (status = 406, description = "`format=json` on a value that is not UTF-8.", body = ErrorEnvelope)
    )
)]
async fn get_value(
    State(state): State<AppState>,
    key: Result<Path<String>, PathRejection>,
    query: Result<Query<GetQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Path(key) = key?;
    let Query(query) = query?;
    validate_key(&key)?;
    let current = state
        .store
        .get(&key, Instant::now())
        .ok_or_else(|| not_found_error(&key))?;
    match query.format {
        None => Ok(current.value.into_response()),
        Some(Format::Json) => {
            let value = std::str::from_utf8(&current.value).map_err(|_| {
                ApiError::new(
                    StatusCode::NOT_ACCEPTABLE,
                    "not_acceptable",
                    "value is not valid UTF-8; request it without format=json",
                )
            })?;
            Ok(Json(ValueResponse {
                key: &key,
                value,
                version: current.version,
                ttl_remaining_ms: ttl_remaining_ms(&current),
            })
            .into_response())
        }
    }
}

/// Validates the request and applies a conditional write to the store.
fn write(
    state: &AppState,
    key: String,
    value: Bytes,
    query: &SetQuery,
) -> Result<Json<SetResponse>, ApiError> {
    validate_key(&key)?;
    let ttl = query.ttl()?;
    let condition = query.condition()?;
    match state
        .store
        .set(key.clone(), value, ttl, condition, Instant::now())
    {
        SetOutcome::Written(version) => Ok(Json(SetResponse { key, version })),
        SetOutcome::Conflict(current) => Err(ApiError::conflict(&current)),
        SetOutcome::NotFound => Err(not_found_error(&key)),
    }
}

/// Handles `PUT /api/v1/set/{*key}`; the body is the raw value.
#[utoipa::path(
    put,
    path = "/api/v1/set/{key}",
    params(
        SetQuery,
        ("key" = String, Path, description = "Key of 1 to 32 bytes; may contain `/`.")
    ),
    request_body(
        content = String,
        content_type = "application/octet-stream",
        description = "The raw value."
    ),
    responses(
        (status = 200, description = "The value was written.", body = SetResponse),
        (status = 400, description = "Invalid key, query, or body.", body = ErrorEnvelope),
        (status = 404, description = "`if_value` was given and the key does not exist.", body = ErrorEnvelope),
        (status = 409, description = "The condition failed; `current` holds the blocking entry.", body = ErrorEnvelope),
        (status = 413, description = "The value exceeds `--max-value-size`.", body = ErrorEnvelope)
    )
)]
async fn put_value(
    State(state): State<AppState>,
    key: Result<Path<String>, PathRejection>,
    query: Result<Query<SetQuery>, QueryRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<SetResponse>, ApiError> {
    let Path(key) = key?;
    let Query(query) = query?;
    let value = body.map_err(|err| {
        let err = ApiError::from(err);
        if err.status == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::too_large(state.max_value_size)
        } else {
            err
        }
    })?;
    write(&state, key, value, &query)
}

/// Handles `POST /api/v1/set` with a JSON `{"key","value"}` body.
#[utoipa::path(
    post,
    path = "/api/v1/set",
    params(SetQuery),
    request_body = SetRequest,
    responses(
        (status = 200, description = "The value was written.", body = SetResponse),
        (status = 400, description = "Invalid key, query, or body.", body = ErrorEnvelope),
        (status = 404, description = "`if_value` was given and the key does not exist.", body = ErrorEnvelope),
        (status = 409, description = "The condition failed; `current` holds the blocking entry.", body = ErrorEnvelope),
        (status = 413, description = "The value exceeds `--max-value-size`.", body = ErrorEnvelope)
    )
)]
async fn post_value(
    State(state): State<AppState>,
    query: Result<Query<SetQuery>, QueryRejection>,
    body: Result<Json<SetRequest>, JsonRejection>,
) -> Result<Json<SetResponse>, ApiError> {
    let Query(query) = query?;
    let Json(request) = body?;
    if request.value.len() > state.max_value_size {
        return Err(ApiError::too_large(state.max_value_size));
    }
    write(&state, request.key, Bytes::from(request.value), &query)
}

/// Handles `DELETE /api/v1/delete/{*key}`.
#[utoipa::path(
    delete,
    path = "/api/v1/delete/{key}",
    params(
        DeleteQuery,
        ("key" = String, Path, description = "Key of 1 to 32 bytes; may contain `/`.")
    ),
    responses(
        (status = 200, description = "The key was deleted.", body = DeleteResponse),
        (status = 400, description = "Invalid key or query.", body = ErrorEnvelope),
        (status = 404, description = "The key does not exist.", body = ErrorEnvelope),
        (status = 409, description = "`if_value` did not match; `current` holds the blocking entry.", body = ErrorEnvelope)
    )
)]
async fn delete_value(
    State(state): State<AppState>,
    key: Result<Path<String>, PathRejection>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<Json<DeleteResponse>, ApiError> {
    let Path(key) = key?;
    let Query(query) = query?;
    validate_key(&key)?;
    let outcome = state.store.delete(
        &key,
        query.if_value.as_ref().map(String::as_bytes),
        Instant::now(),
    );
    match outcome {
        DeleteOutcome::Deleted => Ok(Json(DeleteResponse { deleted: true })),
        DeleteOutcome::Conflict(current) => Err(ApiError::conflict(&current)),
        DeleteOutcome::NotFound => Err(not_found_error(&key)),
    }
}

/// Handles `GET /api/v1/keys`.
#[utoipa::path(
    get,
    path = "/api/v1/keys",
    params(KeysQuery),
    responses(
        (status = 200, description = "Live keys matching the prefix.", body = KeysResponse),
        (status = 400, description = "Invalid query.", body = ErrorEnvelope)
    )
)]
async fn list_keys(
    State(state): State<AppState>,
    query: Result<Query<KeysQuery>, QueryRejection>,
) -> Result<Json<KeysResponse>, ApiError> {
    let Query(query) = query?;
    let keys = state.store.keys(query.prefix.as_deref(), Instant::now());
    Ok(Json(KeysResponse { keys }))
}

/// Error for a key that is not in the store.
fn not_found_error(key: &str) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("key {key:?} not found"),
    )
}

/// Fallback for unmatched routes, so every error uses the JSON envelope.
async fn fallback() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such route")
}

/// OpenAPI document generated from the handler and type annotations.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "stow",
        description = "Minimal local in-memory key-value server for process coordination."
    ),
    paths(health, get_value, put_value, post_value, delete_value, list_keys),
    components(schemas(CurrentBody, ErrorBody, ErrorEnvelope, Format))
)]
struct ApiDoc;

/// Builds the application router over `store`.
///
/// Values larger than `max_value_size` bytes are rejected with 413. When
/// `openapi` is true, `GET /api/v1/openapi.json` serves the OpenAPI document,
/// which is rendered once here.
///
/// # Panics
///
/// Panics if `openapi` is true and the generated document cannot be
/// serialized, which would be a bug in the annotations.
pub fn router(store: Arc<Store>, max_value_size: usize, openapi: bool) -> Router {
    let state = AppState {
        store,
        max_value_size,
    };
    // A JSON string escapes at most 6 bytes per input byte (`\u00XX`), so the
    // POST body cap leaves room for escaping; the decoded value is re-checked
    // exactly in `post_value`.
    let post_limit = 6 * (max_value_size + MAX_KEY_LEN) + 1024;
    let mut router = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/get/{*key}", get(get_value))
        .route(
            "/api/v1/set/{*key}",
            put(put_value).layer(DefaultBodyLimit::max(max_value_size)),
        )
        .route(
            "/api/v1/set",
            post(post_value).layer(DefaultBodyLimit::max(post_limit)),
        )
        .route("/api/v1/delete/{*key}", delete(delete_value))
        .route("/api/v1/keys", get(list_keys));
    if openapi {
        let spec = Bytes::from(
            ApiDoc::openapi()
                .to_json()
                .expect("the OpenAPI document serializes"),
        );
        router = router.route(
            "/api/v1/openapi.json",
            get(move || {
                let spec = spec.clone();
                async move { ([(header::CONTENT_TYPE, "application/json")], spec) }
            }),
        );
    }
    router.fallback(fallback).with_state(state)
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

/// How often the background sweep frees expired entries.
const SWEEP_INTERVAL: Duration = Duration::from_secs(1);

/// Frees expired entries every [`SWEEP_INTERVAL`] so dead leases do not
/// accumulate in a long-lived process. Runs until the task is aborted.
async fn sweep_loop(store: Arc<Store>) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        ticker.tick().await;
        store.sweep(Instant::now());
    }
}

/// Serves the API over `store` on `listener` until `shutdown` completes.
///
/// A background task sweeps expired entries from `store` while serving.
/// In-flight requests finish before this returns. [`on_shutdown`] runs after
/// the server has drained. Values larger than `max_value_size` bytes are
/// rejected. When `openapi` is true, the OpenAPI document is served at
/// `GET /api/v1/openapi.json`. When `silent` is true, the startup and shutdown
/// messages are not printed.
///
/// # Errors
///
/// Returns [`ServerError::Io`] if the listener fails.
pub async fn serve(
    listener: TcpListener,
    store: Arc<Store>,
    shutdown: impl Future<Output = ()> + Send + 'static,
    max_value_size: usize,
    openapi: bool,
    silent: bool,
) -> Result<(), ServerError> {
    let addr = listener.local_addr()?;
    if !silent {
        eprintln!("stow listening on http://{addr}");
    }
    let sweeper = tokio::spawn(sweep_loop(store.clone()));
    let result = axum::serve(listener, router(store, max_value_size, openapi))
        .with_graceful_shutdown(shutdown)
        .await;
    sweeper.abort();
    result?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_key_accepts_1_to_32_bytes() {
        assert!(validate_key("a").is_ok());
        assert!(validate_key(&"a".repeat(MAX_KEY_LEN)).is_ok());
    }

    #[test]
    fn validate_key_rejects_empty_and_oversize_keys() {
        assert!(validate_key("").is_err());
        assert!(validate_key(&"a".repeat(MAX_KEY_LEN + 1)).is_err());
    }

    #[test]
    fn validate_key_counts_bytes_not_chars() {
        // 17 two-byte characters are 34 bytes.
        assert!(validate_key(&"é".repeat(17)).is_err());
    }
}
