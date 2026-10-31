//! Client subcommands: `get`, `set`, `del`, `keys`, `lock`, and `unlock`.
//!
//! Each command opens one short-lived HTTP/1.1 connection per request and
//! maps the response to an [`ExitStatus`]. Waiting is client-side polling
//! over `GET /api/v1/get/{key}`, so the server needs no long-poll or watch
//! support and its idle memory is unchanged.

use std::fmt;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{CONNECTION, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::time::Instant;

use crate::cli::{DelArgs, GetArgs, KeysArgs, LockArgs, SetArgs, UnlockArgs, WaitArgs, WaitFor};
use crate::server::KeysResponse;

/// Longest pause between polls, in milliseconds. Keeps a waiter at about
/// half a request per second once it has backed off fully.
const MAX_INTERVAL_MS: u64 = 2000;

/// How a client command ended; each variant maps to a process exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    /// The command did what was asked (exit 0).
    Success,
    /// The key does not exist (exit 1).
    NotFound,
    /// A condition did not hold, so the server answered 409 (exit 2).
    Conflict,
    /// The server could not be reached (exit 3).
    Unreachable,
    /// `--timeout` passed while waiting (exit 4).
    TimedOut,
    /// Any other failure, such as a rejected request or a write error (exit 1).
    Failure,
}

impl From<ExitStatus> for ExitCode {
    fn from(status: ExitStatus) -> Self {
        Self::from(match status {
            ExitStatus::Success => 0,
            ExitStatus::NotFound | ExitStatus::Failure => 1,
            ExitStatus::Conflict => 2,
            ExitStatus::Unreachable => 3,
            ExitStatus::TimedOut => 4,
        })
    }
}

/// The server did not answer: the connection failed or broke mid-request.
struct Unreachable {
    /// Address that was contacted.
    addr: SocketAddr,
    /// What went wrong, for the error message.
    cause: String,
}

impl Unreachable {
    /// Prints the error to stderr and returns the matching exit status.
    fn report(self) -> ExitStatus {
        eprintln!("error: cannot reach stow at {}: {}", self.addr, self.cause);
        ExitStatus::Unreachable
    }
}

/// A complete HTTP response.
struct Reply {
    /// Response status.
    status: StatusCode,
    /// Response body.
    body: Bytes,
}

impl Reply {
    /// Prints the server's error message and maps the status to an exit
    /// status. Only meaningful for responses that are not the expected one.
    fn failure(&self) -> ExitStatus {
        let message = serde_json::from_slice::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|json| json["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| self.status.to_string());
        eprintln!("error: {message}");
        match self.status {
            StatusCode::NOT_FOUND => ExitStatus::NotFound,
            StatusCode::CONFLICT => ExitStatus::Conflict,
            _ => ExitStatus::Failure,
        }
    }
}

/// Connection details for one stow server.
struct Client {
    /// Address of the server.
    addr: SocketAddr,
}

impl Client {
    /// Sends one request on a fresh connection and reads the whole response.
    ///
    /// # Errors
    ///
    /// Returns [`Unreachable`] if the connection cannot be made or breaks
    /// before a full response arrives.
    async fn request(
        &self,
        method: Method,
        target: &str,
        body: Bytes,
    ) -> Result<Reply, Unreachable> {
        let unreachable = |cause: &dyn fmt::Display| Unreachable {
            addr: self.addr,
            cause: cause.to_string(),
        };
        let stream = TcpStream::connect(self.addr)
            .await
            .map_err(|err| unreachable(&err))?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|err| unreachable(&err))?;
        tokio::spawn(connection);
        let request = Request::builder()
            .method(method)
            .uri(target)
            .header(HOST, self.addr.to_string())
            .header(CONNECTION, "close")
            .body(Full::new(body))
            .expect("method, target, and headers are valid");
        let response = sender
            .send_request(request)
            .await
            .map_err(|err| unreachable(&err))?;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|err| unreachable(&err))?
            .to_bytes();
        Ok(Reply { status, body })
    }

    /// `GET /api/v1/get/{key}`.
    async fn get(&self, key: &str) -> Result<Reply, Unreachable> {
        let target = format!("/api/v1/get/{}", encode(key, true));
        self.request(Method::GET, &target, Bytes::new()).await
    }
}

/// Percent-encodes `text` for use in a URL. Slashes pass through when
/// `keep_slash` is set, because keys use them as separators in the path.
fn encode(text: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (keep_slash && byte == b'/')
        {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Joins `params` into a query string, or returns an empty string if there
/// are none.
fn query(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}

/// Writes `bytes` to `out` and reports whether that succeeded.
fn print(out: &mut impl Write, bytes: &[u8]) -> ExitStatus {
    match out.write_all(bytes).and_then(|()| out.flush()) {
        Ok(()) => ExitStatus::Success,
        Err(err) => {
            eprintln!("error: cannot write output: {err}");
            ExitStatus::Failure
        }
    }
}

/// A non-zero seed for [`Waiter`]'s jitter.
///
/// It mixes the clock with a per-process counter, because waiters created in
/// the same instant must still draw different sequences. A random-number
/// dependency is not needed since the jitter only has to desynchronize
/// waiters.
fn seed() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64);
    nanos
        .wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed))
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        | 1
}

/// Poll timing for waiting commands: exponential back-off up to
/// [`MAX_INTERVAL_MS`], bounded by an optional deadline.
struct Waiter {
    /// When to give up; `None` waits indefinitely.
    deadline: Option<Instant>,
    /// How long the next pause lasts, before jitter.
    interval: Duration,
    /// State of the xorshift generator that produces the jitter.
    rng: u64,
}

impl Waiter {
    /// Starts the clock for the given `--timeout` and `--interval`.
    fn new(args: &WaitArgs) -> Self {
        Self {
            deadline: (args.timeout > 0)
                .then(|| Instant::now() + Duration::from_secs(args.timeout)),
            interval: Duration::from_millis(args.interval),
            rng: seed(),
        }
    }

    /// A pseudo-random duration in `[0, max]`.
    fn jitter(&mut self, max: Duration) -> Duration {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        // Jitter is far below 2^64 nanoseconds, so the cast never truncates.
        Duration::from_nanos((u128::from(self.rng) % (max.as_nanos() + 1)) as u64)
    }

    /// Sleeps before the next poll and returns `true`, or returns `false`
    /// without sleeping once the deadline has passed. The sleep is cut short
    /// at the deadline so the last poll happens right at it.
    ///
    /// Each sleep is the current interval plus up to 50% jitter. Waiters
    /// started together would otherwise back off in lockstep and wake
    /// together, so a freed lock would go unclaimed until the next shared
    /// wake-up and each hand-off would cost a full interval.
    async fn pause(&mut self) -> bool {
        let mut sleep = self.interval + self.jitter(self.interval / 2);
        if let Some(deadline) = self.deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            sleep = sleep.min(remaining);
        }
        tokio::time::sleep(sleep).await;
        self.interval = (self.interval * 2)
            .min(Duration::from_millis(MAX_INTERVAL_MS))
            .max(self.interval);
        true
    }

    /// Reports a timeout on stderr and returns the matching exit status.
    fn timed_out() -> ExitStatus {
        eprintln!("error: timed out");
        ExitStatus::TimedOut
    }
}

/// Runs `stow get`: prints the value, optionally after waiting for
/// `--wait-for` to hold.
///
/// With `--wait-for absent` nothing is printed. A poll that sees the key in
/// any other state than the one asked for just waits again.
pub async fn get(args: GetArgs, out: &mut impl Write) -> ExitStatus {
    let client = Client {
        addr: args.client.addr,
    };
    let mut waiter = Waiter::new(&args.wait);
    loop {
        let reply = match client.get(&args.key).await {
            Ok(reply) => reply,
            Err(err) => return err.report(),
        };
        let found = reply.status == StatusCode::OK;
        let missing = reply.status == StatusCode::NOT_FOUND;
        match &args.wait_for {
            None if found => return print(out, &reply.body),
            None => return reply.failure(),
            Some(WaitFor::Absent) if missing => return ExitStatus::Success,
            Some(WaitFor::Present) if found => return print(out, &reply.body),
            Some(WaitFor::Value(expected)) if found && reply.body == expected.as_bytes() => {
                return print(out, &reply.body);
            }
            Some(_) if found || missing => {}
            Some(_) => return reply.failure(),
        }
        if !waiter.pause().await {
            return Waiter::timed_out();
        }
    }
}

/// Runs `stow set`. A value of `-` is read from stdin.
pub async fn set(args: SetArgs) -> ExitStatus {
    let value = if args.value == "-" {
        let mut buf = Vec::new();
        if let Err(err) = std::io::stdin().read_to_end(&mut buf) {
            eprintln!("error: cannot read stdin: {err}");
            return ExitStatus::Failure;
        }
        buf
    } else {
        args.value.into_bytes()
    };
    let mut params = Vec::new();
    if let Some(ttl) = args.ttl {
        params.push(format!("ttl={ttl}"));
    }
    if args.if_absent {
        params.push("if_absent=true".to_owned());
    }
    if let Some(expected) = &args.if_value {
        params.push(format!("if_value={}", encode(expected, false)));
    }
    let target = format!("/api/v1/set/{}{}", encode(&args.key, true), query(&params));
    let client = Client {
        addr: args.client.addr,
    };
    match client
        .request(Method::PUT, &target, Bytes::from(value))
        .await
    {
        Ok(reply) if reply.status == StatusCode::OK => ExitStatus::Success,
        Ok(reply) => reply.failure(),
        Err(err) => err.report(),
    }
}

/// Sends `DELETE` for `key`, optionally guarded by `if_value`.
async fn delete(client: &Client, key: &str, if_value: Option<&str>) -> ExitStatus {
    let params: Vec<String> = if_value
        .map(|value| format!("if_value={}", encode(value, false)))
        .into_iter()
        .collect();
    let target = format!("/api/v1/delete/{}{}", encode(key, true), query(&params));
    match client.request(Method::DELETE, &target, Bytes::new()).await {
        Ok(reply) if reply.status == StatusCode::OK => ExitStatus::Success,
        Ok(reply) => reply.failure(),
        Err(err) => err.report(),
    }
}

/// Runs `stow del`.
pub async fn del(args: DelArgs) -> ExitStatus {
    let client = Client {
        addr: args.client.addr,
    };
    delete(&client, &args.key, args.if_value.as_deref()).await
}

/// Runs `stow keys`: prints one key per line.
pub async fn keys(args: KeysArgs, out: &mut impl Write) -> ExitStatus {
    let params: Vec<String> = args
        .prefix
        .iter()
        .map(|prefix| format!("prefix={}", encode(prefix, false)))
        .collect();
    let target = format!("/api/v1/keys{}", query(&params));
    let client = Client {
        addr: args.client.addr,
    };
    let reply = match client.request(Method::GET, &target, Bytes::new()).await {
        Ok(reply) => reply,
        Err(err) => return err.report(),
    };
    if reply.status != StatusCode::OK {
        return reply.failure();
    }
    match serde_json::from_slice::<KeysResponse>(&reply.body) {
        Ok(response) => {
            let lines: String = response.keys.iter().map(|key| format!("{key}\n")).collect();
            print(out, lines.as_bytes())
        }
        Err(err) => {
            eprintln!("error: unexpected response from server: {err}");
            ExitStatus::Failure
        }
    }
}

/// Runs `stow lock`: prints the owner token once the lock is held.
///
/// Acquisition is always the atomic `if_absent` write. With `--wait` the
/// preceding read only decides when to try; a 409 from the write means
/// another waiter won the race, so it goes back to waiting.
pub async fn lock(args: LockArgs, out: &mut impl Write) -> ExitStatus {
    let owner = args
        .owner
        .unwrap_or_else(|| format!("stow-{}", std::process::id()));
    let client = Client {
        addr: args.client.addr,
    };
    let target = format!(
        "/api/v1/set/{}?if_absent=true&ttl={}",
        encode(&args.key, true),
        args.ttl
    );
    let mut waiter = Waiter::new(&args.wait_args);
    loop {
        if args.wait {
            let reply = match client.get(&args.key).await {
                Ok(reply) => reply,
                Err(err) => return err.report(),
            };
            match reply.status {
                StatusCode::OK => {
                    if !waiter.pause().await {
                        return Waiter::timed_out();
                    }
                    continue;
                }
                StatusCode::NOT_FOUND => {}
                _ => return reply.failure(),
            }
        }
        let reply = match client
            .request(Method::PUT, &target, Bytes::from(owner.clone()))
            .await
        {
            Ok(reply) => reply,
            Err(err) => return err.report(),
        };
        match reply.status {
            StatusCode::OK => return print(out, format!("{owner}\n").as_bytes()),
            StatusCode::CONFLICT if args.wait => {
                if !waiter.pause().await {
                    return Waiter::timed_out();
                }
            }
            _ => return reply.failure(),
        }
    }
}

/// Runs `stow unlock`: deletes the key only if `--owner` still holds it.
pub async fn unlock(args: UnlockArgs) -> ExitStatus {
    let client = Client {
        addr: args.client.addr,
    };
    delete(&client, &args.key, Some(&args.owner)).await
}
