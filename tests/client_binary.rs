//! End-to-end test: the `stow` binary's client subcommands against a
//! `stow serve` process, including `STOW_ADDR` and exit codes.

#![cfg(unix)]

use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Path to the compiled `stow` binary.
const STOW: &str = env!("CARGO_BIN_EXE_stow");

/// A `stow serve` child process, killed on drop.
struct Server {
    /// The running process.
    child: Child,
    /// Address it listens on.
    addr: SocketAddr,
}

impl Server {
    /// Starts `stow serve` on a free port and waits until it accepts
    /// connections.
    fn start() -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = Command::new(STOW)
            .args(["-q", "serve", "--port", &port.to_string()])
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

    /// Runs `stow <args>` with `STOW_ADDR` pointing at this server.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(STOW)
            .args(args)
            .env("STOW_ADDR", self.addr.to_string())
            .output()
            .unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn client_commands_work_through_the_binary() {
    let server = Server::start();
    assert_eq!(
        server.run(&["set", "greeting", "hello"]).status.code(),
        Some(0)
    );
    let get = server.run(&["get", "greeting"]);
    assert_eq!(get.status.code(), Some(0));
    assert_eq!(get.stdout, b"hello");
    assert_eq!(server.run(&["get", "missing"]).status.code(), Some(1));
    assert_eq!(server.run(&["keys"]).stdout, b"greeting\n");
    assert_eq!(server.run(&["del", "greeting"]).status.code(), Some(0));
}

#[test]
fn set_reads_the_value_from_stdin_when_given_a_dash() {
    let server = Server::start();
    let mut child = Command::new(STOW)
        .args(["set", "note", "-"])
        .env("STOW_ADDR", server.addr.to_string())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"from stdin\n")
        .unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert_eq!(server.run(&["get", "note"]).stdout, b"from stdin\n");
}

#[test]
fn lock_prints_the_default_owner_and_a_second_lock_conflicts() {
    let server = Server::start();
    let first = server.run(&["lock", "lock/build", "--ttl", "60"]);
    assert_eq!(first.status.code(), Some(0));
    let owner = String::from_utf8(first.stdout).unwrap();
    assert!(owner.starts_with("stow-"), "{owner}");
    assert_eq!(
        server
            .run(&["lock", "lock/build", "--ttl", "60"])
            .status
            .code(),
        Some(2)
    );
    let unlock = server.run(&["unlock", "lock/build", "--owner", owner.trim()]);
    assert_eq!(unlock.status.code(), Some(0));
}

#[test]
fn unreachable_server_exits_3() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let output = Command::new(STOW)
        .args(["get", "k", "--addr", &format!("127.0.0.1:{port}")])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
}
