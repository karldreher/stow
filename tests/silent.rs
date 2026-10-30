//! End-to-end test: `--silent` suppresses status output but not errors.

#![cfg(unix)]

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Path to the compiled `stow` binary.
const STOW: &str = env!("CARGO_BIN_EXE_stow");

/// Runs `stow <args>` until it accepts connections on `port`, sends SIGTERM,
/// and returns its output.
fn run_until_listening(args: &[&str], port: u16) -> Output {
    let child = Command::new(STOW)
        .args(args)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < deadline, "stow did not start listening");
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    child.wait_with_output().unwrap()
}

/// Returns a port that is free at the time of the call.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn silent_serve_prints_nothing() {
    let port = free_port();
    let output = run_until_listening(&["-q", "serve", "--port", &port.to_string()], port);
    assert!(output.status.success());
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
}

#[test]
fn serve_prints_status_messages_by_default() {
    let port = free_port();
    let output = run_until_listening(&["serve", "--port", &port.to_string()], port);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("stow listening on"), "{stderr}");
    assert!(stderr.contains("stow stopped"), "{stderr}");
}

#[test]
fn silent_serve_still_reports_errors() {
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let output = Command::new(STOW)
        .args(["-q", "serve", "--port", &port.to_string()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("already in use"), "{stderr}");
}
