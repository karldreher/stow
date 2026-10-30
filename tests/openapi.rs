//! The OpenAPI document is served only when `--openapi` is set.

use std::net::SocketAddr;

use stow::cli::DEFAULT_MAX_VALUE_SIZE;
use stow::server;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

/// Starts a server with the given `openapi` setting, requests the spec, and
/// returns the raw head and body of the response.
async fn fetch_spec(openapi: bool) -> (String, String) {
    let listener = server::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(server::serve(
        listener,
        Default::default(),
        async move {
            let _ = rx.await;
        },
        DEFAULT_MAX_VALUE_SIZE,
        openapi,
        None,
        true,
    ));

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            b"GET /api/v1/openapi.json HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let _ = tx.send(());
    handle.await.unwrap().unwrap();

    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    (head.to_string(), body.to_string())
}

#[tokio::test]
async fn spec_is_served_as_openapi_3_1_json_with_the_flag() {
    let (head, body) = fetch_spec(true).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: application/json"),
        "{head}"
    );

    let spec: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(spec["openapi"].as_str().unwrap().starts_with("3.1"));
    for path in [
        "/api/v1/health",
        "/api/v1/get/{key}",
        "/api/v1/set/{key}",
        "/api/v1/set",
        "/api/v1/delete/{key}",
        "/api/v1/keys",
    ] {
        assert!(spec["paths"][path].is_object(), "missing {path}");
    }
    assert!(spec["components"]["schemas"]["ErrorEnvelope"].is_object());
    for param in ["ttl", "if_absent", "if_value", "format", "prefix"] {
        assert!(body.contains(&format!("\"name\":\"{param}\"")), "{param}");
    }
}

#[tokio::test]
async fn spec_route_is_404_without_the_flag() {
    let (head, body) = fetch_spec(false).await;
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(envelope["error"]["code"], "not_found");
}
