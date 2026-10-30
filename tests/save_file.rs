//! End-to-end tests: `--save-file` restores state across restarts and never
//! overwrites a file it could not read.

#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Path to the compiled `stow` binary.
const STOW: &str = env!("CARGO_BIN_EXE_stow");

/// A running `stow serve` process on a known port.
struct Server {
    /// The child process.
    child: Child,
    /// The loopback address it listens on.
    addr: SocketAddr,
}

impl Server {
    /// Starts `stow serve` with `extra` arguments and waits until it accepts
    /// connections.
    fn start(extra: &[&str]) -> Self {
        let port = free_port();
        let child = Command::new(STOW)
            .args(["serve", "--port", &port.to_string()])
            .args(extra)
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + Duration::from_secs(10);
        while TcpStream::connect(addr).is_err() {
            assert!(Instant::now() < deadline, "stow did not start listening");
            std::thread::sleep(Duration::from_millis(20));
        }
        Self { child, addr }
    }

    /// Sends SIGTERM and returns the process output once it exits.
    fn terminate(self) -> Output {
        let status = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        self.child.wait_with_output().unwrap()
    }

    /// Sends one request and returns the response body.
    fn request(&self, method: &str, path: &str, body: &[u8]) -> String {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        raw.split_once("\r\n\r\n").unwrap().1.to_string()
    }
}

/// Returns a port that is free at the time of the call.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A fresh directory path under the system temp directory, unique per test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stow-save-file-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// Renders `path` for use as a command-line argument.
fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn state_survives_a_restart() {
    let dir = temp_dir("restart");
    let path = dir.join("nested/state.bin");

    let first = Server::start(&["--save-file", arg(&path)]);
    first.request("PUT", "/api/v1/set/k", b"one");
    first.request("PUT", "/api/v1/set/k", b"two");
    first.request("PUT", "/api/v1/set/ttl?ttl=60", b"v");
    let output = first.terminate();
    assert!(output.status.success());
    assert!(path.is_file());

    let second = Server::start(&["--save-file", arg(&path)]);
    let body = second.request("GET", "/api/v1/get/k?format=json", b"");
    assert!(body.contains(r#""value":"two""#), "{body}");
    assert!(body.contains(r#""version":2"#), "{body}");
    let body = second.request("GET", "/api/v1/get/ttl?format=json", b"");
    assert!(!body.contains(r#""ttl_remaining_ms":null"#), "{body}");
    second.terminate();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn missing_file_starts_empty_and_is_logged() {
    let dir = temp_dir("missing");
    let path = dir.join("state.bin");

    let server = Server::start(&["--save-file", arg(&path)]);
    let output = server.terminate();

    let stderr = String::from_utf8(output.stderr).unwrap();
    let expected = format!("no save file at {}, starting empty", path.display());
    assert!(stderr.contains(&expected), "{stderr}");
    assert!(path.is_file(), "an empty store is still saved on shutdown");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unreadable_file_exits_non_zero_and_is_left_untouched() {
    let dir = temp_dir("bad-magic");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.bin");
    fs::write(&path, b"NOPE and then some bytes").unwrap();

    let output = Command::new(STOW)
        .args(["serve", "--port", &free_port().to_string()])
        .args(["--save-file", arg(&path)])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(arg(&path)), "{stderr}");
    assert_eq!(fs::read(&path).unwrap(), b"NOPE and then some bytes");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn without_the_flag_no_file_is_written() {
    let dir = temp_dir("no-flag");
    fs::create_dir_all(&dir).unwrap();

    let mut child = Command::new(STOW)
        .args(["-q", "serve", "--port", &free_port().to_string()])
        .current_dir(&dir)
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    child.wait().unwrap();

    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    let _ = fs::remove_dir_all(&dir);
}
