//! Integration test: starts the server on an ephemeral port and checks `/api/v1/health`.

use std::net::SocketAddr;

use stow::server;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

#[tokio::test]
async fn health_returns_ok_and_version() {
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
        stow::cli::DEFAULT_MAX_VALUE_SIZE,
        false,
        true,
    ));

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = response.split("\r\n\r\n").nth(1).unwrap();
    let json: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));

    tx.send(()).unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn bind_reports_port_in_use() {
    let first = server::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = first.local_addr().unwrap();
    let err = server::bind(addr).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "port {} is already in use; pass --port to choose another",
            addr.port()
        )
    );
}
