//! Integration tests for the key-value endpoints, driven over raw HTTP.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use stow::cli::DEFAULT_MAX_VALUE_SIZE;
use stow::server;
use stow::store::Store;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

/// A running server that shuts down when [`TestServer::stop`] is called.
struct TestServer {
    /// Address the server listens on.
    addr: SocketAddr,
    /// The store the server runs over, for white-box assertions.
    store: Arc<Store>,
    /// Signals graceful shutdown.
    shutdown: oneshot::Sender<()>,
    /// The serve task.
    handle: tokio::task::JoinHandle<Result<(), server::ServerError>>,
}

/// A parsed HTTP response.
struct Response {
    /// Numeric status code.
    status: u16,
    /// Lowercased header block, for content-type checks.
    headers: String,
    /// Raw body bytes.
    body: Vec<u8>,
}

/// Sends one request to `addr` and reads the whole response.
async fn send(
    addr: SocketAddr,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Response {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(content_type) = content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(raw[..split].to_vec()).unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Response {
        status,
        headers: head.to_lowercase(),
        body: raw[split + 4..].to_vec(),
    }
}

impl Response {
    /// Body parsed as JSON.
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

impl TestServer {
    /// Starts a server on an ephemeral loopback port.
    async fn start(max_value_size: usize) -> Self {
        let listener = server::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, rx) = oneshot::channel::<()>();
        let store = Arc::new(Store::default());
        let handle = tokio::spawn(server::serve(
            listener,
            store.clone(),
            async move {
                let _ = rx.await;
            },
            max_value_size,
            None,
            true,
        ));
        Self {
            addr,
            store,
            shutdown,
            handle,
        }
    }

    /// Sends one request and reads the whole response.
    async fn request(
        &self,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: &[u8],
    ) -> Response {
        send(self.addr, method, path, content_type, body).await
    }

    /// `GET path`.
    async fn get(&self, path: &str) -> Response {
        self.request("GET", path, None, b"").await
    }

    /// `PUT path` with a raw body.
    async fn put(&self, path: &str, body: &[u8]) -> Response {
        self.request("PUT", path, None, body).await
    }

    /// `POST /api/v1/set` with a JSON body.
    async fn post_json(&self, body: &serde_json::Value) -> Response {
        self.request(
            "POST",
            "/api/v1/set",
            Some("application/json"),
            body.to_string().as_bytes(),
        )
        .await
    }

    /// `DELETE path`.
    async fn delete(&self, path: &str) -> Response {
        self.request("DELETE", path, None, b"").await
    }

    /// Shuts the server down and waits for it to drain.
    async fn stop(self) {
        self.shutdown.send(()).unwrap();
        self.handle.await.unwrap().unwrap();
    }
}

/// Asserts `response` is an error envelope with the given status and code.
fn assert_error(response: &Response, status: u16, code: &str) {
    assert_eq!(
        response.status,
        status,
        "{:?}",
        String::from_utf8_lossy(&response.body)
    );
    let json = response.json();
    assert_eq!(json["error"]["code"], code);
    assert!(json["error"]["message"].is_string());
}

#[tokio::test]
async fn put_then_get_returns_raw_value() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    let put = server.put("/api/v1/set/greeting", b"hello").await;
    assert_eq!(put.status, 200);
    assert_eq!(
        put.json(),
        serde_json::json!({"key": "greeting", "version": 1})
    );

    let get = server.get("/api/v1/get/greeting").await;
    assert_eq!(get.status, 200);
    assert!(
        get.headers
            .contains("content-type: application/octet-stream"),
        "{}",
        get.headers
    );
    assert_eq!(get.body, b"hello");
    server.stop().await;
}

#[tokio::test]
async fn get_with_format_json_returns_key_value_version() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    server.put("/api/v1/set/k", b"v").await;
    let get = server.get("/api/v1/get/k?format=json").await;
    assert_eq!(get.status, 200);
    assert_eq!(
        get.json(),
        serde_json::json!({"key": "k", "value": "v", "version": 1, "ttl_remaining_ms": null})
    );
    server.stop().await;
}

#[tokio::test]
async fn unknown_format_is_rejected() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    server.put("/api/v1/set/k", b"v").await;
    assert_error(
        &server.get("/api/v1/get/k?format=xml").await,
        400,
        "invalid_query",
    );
    server.stop().await;
}

#[tokio::test]
async fn format_json_on_non_utf8_value_is_not_acceptable() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    server.put("/api/v1/set/bin", &[0xff, 0xfe, 0x00]).await;
    assert_error(
        &server.get("/api/v1/get/bin?format=json").await,
        406,
        "not_acceptable",
    );
    let raw = server.get("/api/v1/get/bin").await;
    assert_eq!(raw.body, [0xff, 0xfe, 0x00]);
    server.stop().await;
}

#[tokio::test]
async fn overwrite_increments_version() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    assert_eq!(server.put("/api/v1/set/k", b"1").await.json()["version"], 1);
    assert_eq!(server.put("/api/v1/set/k", b"2").await.json()["version"], 2);
    let get = server.get("/api/v1/get/k?format=json").await;
    assert_eq!(get.json()["version"], 2);
    assert_eq!(get.json()["value"], "2");
    server.stop().await;
}

#[tokio::test]
async fn post_json_sets_value() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    let post = server
        .post_json(&serde_json::json!({"key": "a", "value": "b"}))
        .await;
    assert_eq!(post.status, 200);
    assert_eq!(post.json(), serde_json::json!({"key": "a", "version": 1}));
    assert_eq!(server.get("/api/v1/get/a").await.body, b"b");
    server.stop().await;
}

#[tokio::test]
async fn post_with_malformed_json_is_bad_request() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    let response = server
        .request("POST", "/api/v1/set", Some("application/json"), b"{nope")
        .await;
    assert_error(&response, 400, "invalid_body");
    server.stop().await;
}

#[tokio::test]
async fn keys_may_contain_slashes() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    server.put("/api/v1/set/lock/build", b"x").await;
    let get = server.get("/api/v1/get/lock/build").await;
    assert_eq!(get.status, 200);
    assert_eq!(get.body, b"x");
    assert_eq!(
        server.get("/api/v1/keys").await.json(),
        serde_json::json!({"keys": ["lock/build"]})
    );
    server.stop().await;
}

#[tokio::test]
async fn percent_encoded_keys_are_decoded() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    server.put("/api/v1/set/a%20b", b"x").await;
    assert_eq!(
        server.get("/api/v1/keys").await.json(),
        serde_json::json!({"keys": ["a b"]})
    );
    server.stop().await;
}

#[tokio::test]
async fn missing_key_returns_404_envelope() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    assert_error(&server.get("/api/v1/get/nope").await, 404, "not_found");
    assert_error(
        &server.delete("/api/v1/delete/nope").await,
        404,
        "not_found",
    );
    server.stop().await;
}

#[tokio::test]
async fn delete_removes_key() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    server.put("/api/v1/set/k", b"v").await;
    let delete = server.delete("/api/v1/delete/k").await;
    assert_eq!(delete.status, 200);
    assert_eq!(delete.json(), serde_json::json!({"deleted": true}));
    assert_error(&server.get("/api/v1/get/k").await, 404, "not_found");
    server.stop().await;
}

#[tokio::test]
async fn keys_filters_by_optional_prefix() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    for key in ["lock/b", "lock/a", "other"] {
        server.put(&format!("/api/v1/set/{key}"), b"x").await;
    }
    let all = server.get("/api/v1/keys").await;
    assert_eq!(all.status, 200);
    assert_eq!(
        all.json(),
        serde_json::json!({"keys": ["lock/a", "lock/b", "other"]})
    );
    let locks = server.get("/api/v1/keys?prefix=lock/").await;
    assert_eq!(
        locks.json(),
        serde_json::json!({"keys": ["lock/a", "lock/b"]})
    );
    server.stop().await;
}

#[tokio::test]
async fn value_over_limit_returns_413() {
    let server = TestServer::start(8).await;
    assert_eq!(server.put("/api/v1/set/k", b"12345678").await.status, 200);
    assert_error(
        &server.put("/api/v1/set/k", b"123456789").await,
        413,
        "payload_too_large",
    );
    let post = server
        .post_json(&serde_json::json!({"key": "k", "value": "123456789"}))
        .await;
    assert_error(&post, 413, "payload_too_large");
    let huge = "x".repeat(5000);
    let post = server
        .post_json(&serde_json::json!({"key": "k", "value": huge}))
        .await;
    assert_error(&post, 413, "payload_too_large");
    server.stop().await;
}

#[tokio::test]
async fn default_limit_is_1024_bytes() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    assert_eq!(server.put("/api/v1/set/k", &[b'x'; 1024]).await.status, 200);
    assert_eq!(server.put("/api/v1/set/k", &[b'x'; 1025]).await.status, 413);
    server.stop().await;
}

#[tokio::test]
async fn invalid_keys_return_400() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    let long = "k".repeat(33);
    assert_error(
        &server.put(&format!("/api/v1/set/{long}"), b"v").await,
        400,
        "invalid_key",
    );
    assert_error(
        &server.get(&format!("/api/v1/get/{long}")).await,
        400,
        "invalid_key",
    );
    assert_error(
        &server.delete(&format!("/api/v1/delete/{long}")).await,
        400,
        "invalid_key",
    );
    assert_error(
        &server
            .post_json(&serde_json::json!({"key": "", "value": "v"}))
            .await,
        400,
        "invalid_key",
    );
    assert_error(
        &server.put("/api/v1/set/%ff", b"v").await,
        400,
        "invalid_key",
    );
    let max = "k".repeat(32);
    assert_eq!(
        server.put(&format!("/api/v1/set/{max}"), b"v").await.status,
        200
    );
    server.stop().await;
}

#[tokio::test]
async fn unknown_route_returns_json_envelope() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    assert_error(&server.get("/api/v1/nope").await, 404, "not_found");
    server.stop().await;
}

#[tokio::test]
async fn if_absent_creates_then_conflicts_with_current() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    let first = server.put("/api/v1/set/c?if_absent=true", b"one").await;
    assert_eq!(first.status, 200);
    let second = server.put("/api/v1/set/c?if_absent=true", b"two").await;
    assert_error(&second, 409, "conflict");
    assert_eq!(
        second.json()["current"],
        serde_json::json!({"value": "one", "version": 1, "ttl_remaining_ms": null})
    );
    assert_eq!(server.get("/api/v1/get/c").await.body, b"one");
    server.stop().await;
}

#[tokio::test]
async fn if_value_on_set_is_compare_and_set() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    assert_error(
        &server.put("/api/v1/set/k?if_value=a", b"b").await,
        404,
        "not_found",
    );
    server.put("/api/v1/set/k", b"a").await;
    assert_error(
        &server.put("/api/v1/set/k?if_value=x", b"b").await,
        409,
        "conflict",
    );
    let ok = server.put("/api/v1/set/k?if_value=a", b"b").await;
    assert_eq!(ok.json()["version"], 2);
    assert_eq!(server.get("/api/v1/get/k").await.body, b"b");
    server.stop().await;
}

#[tokio::test]
async fn if_value_on_delete_only_removes_a_match() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    assert_error(
        &server.delete("/api/v1/delete/k?if_value=a").await,
        404,
        "not_found",
    );
    server.put("/api/v1/set/k", b"a").await;
    assert_error(
        &server.delete("/api/v1/delete/k?if_value=x").await,
        409,
        "conflict",
    );
    assert_eq!(server.get("/api/v1/get/k").await.status, 200);
    assert_eq!(
        server.delete("/api/v1/delete/k?if_value=a").await.status,
        200
    );
    assert_eq!(server.get("/api/v1/get/k").await.status, 404);
    server.stop().await;
}

#[tokio::test]
async fn post_accepts_the_same_conditions() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    let body = serde_json::json!({"key": "k", "value": "v"}).to_string();
    let path = "/api/v1/set?if_absent=true";
    let first = server
        .request("POST", path, Some("application/json"), body.as_bytes())
        .await;
    assert_eq!(first.status, 200);
    let second = server
        .request("POST", path, Some("application/json"), body.as_bytes())
        .await;
    assert_error(&second, 409, "conflict");
    server.stop().await;
}

#[tokio::test]
async fn ttl_is_reported_and_validated() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    server.put("/api/v1/set/k?ttl=60", b"v").await;
    let get = server.get("/api/v1/get/k?format=json").await.json();
    let remaining = get["ttl_remaining_ms"].as_u64().unwrap();
    assert!(remaining > 55_000 && remaining <= 60_000, "{remaining}");

    server.put("/api/v1/set/k", b"v").await;
    let get = server.get("/api/v1/get/k?format=json").await.json();
    assert!(
        get["ttl_remaining_ms"].is_null(),
        "set without ttl clears it"
    );

    for ttl in ["0", "86401", "abc"] {
        let response = server.put(&format!("/api/v1/set/k?ttl={ttl}"), b"v").await;
        assert_error(&response, 400, "invalid_query");
    }
    assert_eq!(
        server.put("/api/v1/set/k?ttl=86400", b"v").await.status,
        200
    );
    server.stop().await;
}

#[tokio::test]
async fn if_absent_and_if_value_cannot_be_combined() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    let response = server
        .put("/api/v1/set/k?if_absent=true&if_value=a", b"v")
        .await;
    assert_error(&response, 400, "invalid_query");
    server.stop().await;
}

#[tokio::test]
async fn expired_entries_are_swept_from_the_store() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    server.put("/api/v1/set/k?ttl=1", b"v").await;
    assert_eq!(server.store.len(), 1);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_error(&server.get("/api/v1/get/k").await, 404, "not_found");
    assert!(
        server.get("/api/v1/keys").await.json()["keys"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    for _ in 0..40 {
        if server.store.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        server.store.is_empty(),
        "sweep did not free the expired entry"
    );
    server.stop().await;
}

#[tokio::test]
async fn fifty_concurrent_if_absent_writes_have_exactly_one_winner() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    let addr = server.addr;
    let tasks: Vec<_> = (0..50)
        .map(|id| {
            tokio::spawn(async move {
                let value = format!("client-{id}");
                let response = send(
                    addr,
                    "PUT",
                    "/api/v1/set/race?if_absent=true",
                    None,
                    value.as_bytes(),
                )
                .await;
                (value, response)
            })
        })
        .collect();
    let mut winners = Vec::new();
    let mut losers = Vec::new();
    for task in tasks {
        let (value, response) = task.await.unwrap();
        match response.status {
            200 => winners.push(value),
            409 => losers.push(response),
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!(winners.len(), 1);
    assert_eq!(losers.len(), 49);
    for loser in &losers {
        assert_eq!(loser.json()["current"]["value"], winners[0].as_str());
    }
    server.stop().await;
}

#[tokio::test]
async fn expired_lease_can_be_taken_and_old_holder_cannot_release_it() {
    let server = TestServer::start(DEFAULT_MAX_VALUE_SIZE).await;
    let lock = "/api/v1/set/lock/x";
    assert_eq!(
        server
            .put(&format!("{lock}?if_absent=true&ttl=1"), b"a")
            .await
            .status,
        200
    );
    assert_error(
        &server
            .put(&format!("{lock}?if_absent=true&ttl=1"), b"b")
            .await,
        409,
        "conflict",
    );

    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        server
            .put(&format!("{lock}?if_absent=true&ttl=60"), b"b")
            .await
            .status,
        200
    );

    let release = server.delete("/api/v1/delete/lock/x?if_value=a").await;
    assert_error(&release, 409, "conflict");
    assert_eq!(server.get("/api/v1/get/lock/x").await.body, b"b");

    let renew = server.put(&format!("{lock}?if_value=b&ttl=60"), b"b").await;
    assert_eq!(renew.status, 200);
    assert_eq!(
        server
            .delete("/api/v1/delete/lock/x?if_value=b")
            .await
            .status,
        200
    );
    server.stop().await;
}
