//! Integration tests for the client subcommands, run against a live server.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use stow::cli::{Cli, Command};
use stow::client::{self, ExitStatus};
use stow::server;
use stow::store::Store;
use tokio::sync::oneshot;

/// A running server on an ephemeral loopback port.
struct TestServer {
    /// Address the server listens on.
    addr: SocketAddr,
    /// Keeps the server alive until the test ends.
    _shutdown: oneshot::Sender<()>,
}

impl TestServer {
    /// Starts a server with the default value limit.
    async fn start() -> Self {
        let listener = server::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, rx) = oneshot::channel::<()>();
        tokio::spawn(server::serve(
            listener,
            Arc::new(Store::default()),
            async move {
                let _ = rx.await;
            },
            1024,
            false,
            None,
            true,
        ));
        Self {
            addr,
            _shutdown: shutdown,
        }
    }

    /// Runs `stow <args> --addr <this server>` and returns the exit status
    /// with whatever the command printed to stdout.
    async fn run(&self, args: &[&str]) -> (ExitStatus, String) {
        run(self.addr, args).await
    }
}

/// Runs `stow <args> --addr <addr>` through the same parser and dispatch as
/// the binary, capturing stdout.
async fn run(addr: SocketAddr, args: &[&str]) -> (ExitStatus, String) {
    let addr = addr.to_string();
    let argv = ["stow"]
        .into_iter()
        .chain(args.iter().copied())
        .chain(["--addr", &addr]);
    let mut out = Vec::new();
    let status = match Cli::try_parse_from(argv).unwrap().command {
        Command::Get(args) => client::get(args, &mut out).await,
        Command::Set(args) => client::set(args).await,
        Command::Del(args) => client::del(args).await,
        Command::Keys(args) => client::keys(args, &mut out).await,
        Command::Lock(args) => client::lock(args, &mut out).await,
        Command::Unlock(args) => client::unlock(args).await,
        Command::Serve(_) => panic!("serve is not a client command"),
    };
    (status, String::from_utf8(out).unwrap())
}

/// An address with nothing listening on it.
fn closed_addr() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

#[tokio::test]
async fn set_get_keys_and_del_round_trip() {
    let server = TestServer::start().await;
    assert_eq!(
        server.run(&["set", "greeting", "hello"]).await.0,
        ExitStatus::Success
    );
    assert_eq!(
        server.run(&["get", "greeting"]).await,
        (ExitStatus::Success, "hello".into())
    );
    assert_eq!(
        server.run(&["set", "lock/a", "1"]).await.0,
        ExitStatus::Success
    );
    assert_eq!(
        server.run(&["keys", "--prefix", "lock/"]).await,
        (ExitStatus::Success, "lock/a\n".into())
    );
    assert_eq!(server.run(&["keys"]).await.1, "greeting\nlock/a\n");
    assert_eq!(
        server.run(&["del", "greeting"]).await.0,
        ExitStatus::Success
    );
    assert_eq!(
        server.run(&["get", "greeting"]).await.0,
        ExitStatus::NotFound
    );
    assert_eq!(
        server.run(&["del", "greeting"]).await.0,
        ExitStatus::NotFound
    );
}

#[tokio::test]
async fn keys_and_values_with_special_characters_survive_encoding() {
    let server = TestServer::start().await;
    let key = "a b/c?d&e=é";
    assert_eq!(
        server.run(&["set", key, "x y&z=1"]).await.0,
        ExitStatus::Success
    );
    assert_eq!(
        server.run(&["get", key]).await,
        (ExitStatus::Success, "x y&z=1".into())
    );
    assert_eq!(server.run(&["keys"]).await.1, format!("{key}\n"));
    let guarded = ["set", key, "next", "--if-value", "x y&z=1"];
    assert_eq!(server.run(&guarded).await.0, ExitStatus::Success);
}

#[tokio::test]
async fn conditional_flags_map_to_the_api() {
    let server = TestServer::start().await;
    assert_eq!(
        server.run(&["set", "k", "a", "--if-absent"]).await.0,
        ExitStatus::Success
    );
    assert_eq!(
        server.run(&["set", "k", "b", "--if-absent"]).await.0,
        ExitStatus::Conflict
    );
    assert_eq!(
        server.run(&["set", "k", "b", "--if-value", "nope"]).await.0,
        ExitStatus::Conflict
    );
    assert_eq!(
        server.run(&["set", "k", "b", "--if-value", "a"]).await.0,
        ExitStatus::Success
    );
    assert_eq!(
        server.run(&["del", "k", "--if-value", "a"]).await.0,
        ExitStatus::Conflict
    );
    assert_eq!(
        server.run(&["del", "k", "--if-value", "b"]).await.0,
        ExitStatus::Success
    );
    assert_eq!(
        server.run(&["set", "t", "v", "--ttl", "1"]).await.0,
        ExitStatus::Success
    );
    assert_eq!(
        server.run(&["set", "t", "v", "--ttl", "0"]).await.0,
        ExitStatus::Failure
    );
}

#[tokio::test]
async fn unreachable_server_exits_3_including_while_waiting() {
    let addr = closed_addr();
    assert_eq!(run(addr, &["get", "k"]).await.0, ExitStatus::Unreachable);
    assert_eq!(
        run(addr, &["set", "k", "v"]).await.0,
        ExitStatus::Unreachable
    );
    let started = Instant::now();
    let waiting = ["get", "k", "--wait-for", "present", "--timeout", "30"];
    assert_eq!(run(addr, &waiting).await.0, ExitStatus::Unreachable);
    assert!(started.elapsed() < Duration::from_secs(5));
    let locking = ["lock", "k", "--ttl", "5", "--wait", "--timeout", "30"];
    assert_eq!(run(addr, &locking).await.0, ExitStatus::Unreachable);
}

#[tokio::test]
async fn wait_for_present_returns_the_value_once_set() {
    let server = Arc::new(TestServer::start().await);
    let writer = server.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        writer.run(&["set", "build-sha", "abc123"]).await;
    });
    let result = server
        .run(&[
            "get",
            "build-sha",
            "--wait-for",
            "present",
            "--interval",
            "20",
        ])
        .await;
    assert_eq!(result, (ExitStatus::Success, "abc123".into()));
}

#[tokio::test]
async fn wait_for_value_ignores_other_values_until_the_match() {
    let server = Arc::new(TestServer::start().await);
    server.run(&["set", "task-42/status", "pending"]).await;
    let writer = server.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        writer.run(&["set", "task-42/status", "running"]).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        writer.run(&["set", "task-42/status", "done"]).await;
    });
    let args = [
        "get",
        "task-42/status",
        "--wait-for",
        "value=done",
        "--interval",
        "20",
    ];
    assert_eq!(
        server.run(&args).await,
        (ExitStatus::Success, "done".into())
    );
}

#[tokio::test]
async fn wait_for_absent_is_satisfied_by_delete() {
    let server = Arc::new(TestServer::start().await);
    server.run(&["set", "k", "v"]).await;
    let deleter = server.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        deleter.run(&["del", "k"]).await;
    });
    let result = server
        .run(&["get", "k", "--wait-for", "absent", "--interval", "20"])
        .await;
    assert_eq!(result, (ExitStatus::Success, String::new()));
}

#[tokio::test]
async fn wait_for_absent_is_satisfied_by_ttl_expiry() {
    let server = TestServer::start().await;
    server.run(&["set", "k", "v", "--ttl", "1"]).await;
    let started = Instant::now();
    let result = server
        .run(&["get", "k", "--wait-for", "absent", "--interval", "50"])
        .await;
    assert_eq!(result, (ExitStatus::Success, String::new()));
    assert!(started.elapsed() >= Duration::from_millis(500));
}

#[tokio::test]
async fn satisfied_conditions_return_without_sleeping() {
    let server = TestServer::start().await;
    server.run(&["set", "k", "v"]).await;
    let started = Instant::now();
    for (condition, key) in [("present", "k"), ("value=v", "k"), ("absent", "missing")] {
        let args = ["get", key, "--wait-for", condition, "--interval", "5000"];
        let status = server.run(&args).await.0;
        assert_eq!(status, ExitStatus::Success, "{condition}");
    }
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn wait_for_times_out_with_status_4() {
    let server = TestServer::start().await;
    let started = Instant::now();
    let args = [
        "get",
        "k",
        "--wait-for",
        "present",
        "--timeout",
        "1",
        "--interval",
        "50",
    ];
    assert_eq!(server.run(&args).await.0, ExitStatus::TimedOut);
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(1) && elapsed < Duration::from_secs(3),
        "{elapsed:?}"
    );
}

#[tokio::test]
async fn get_without_wait_for_returns_immediately() {
    let server = TestServer::start().await;
    assert_eq!(
        server.run(&["get", "missing"]).await.0,
        ExitStatus::NotFound
    );
}

#[tokio::test]
async fn lock_and_unlock_follow_the_owner_token() {
    let server = TestServer::start().await;
    let lock = ["lock", "lock/build", "--ttl", "60", "--owner", "agent-1"];
    assert_eq!(
        server.run(&lock).await,
        (ExitStatus::Success, "agent-1\n".into())
    );
    let other = ["lock", "lock/build", "--ttl", "60", "--owner", "agent-2"];
    assert_eq!(server.run(&other).await.0, ExitStatus::Conflict);
    let wrong = ["unlock", "lock/build", "--owner", "agent-2"];
    assert_eq!(server.run(&wrong).await.0, ExitStatus::Conflict);
    let right = ["unlock", "lock/build", "--owner", "agent-1"];
    assert_eq!(server.run(&right).await.0, ExitStatus::Success);
    assert_eq!(server.run(&right).await.0, ExitStatus::NotFound);
}

#[tokio::test]
async fn lock_wait_hands_the_lock_to_one_client_at_a_time() {
    let server = Arc::new(TestServer::start().await);
    let holders = Arc::new(AtomicUsize::new(0));
    let acquired = Arc::new(AtomicUsize::new(0));
    let clients: Vec<_> = (0..50)
        .map(|id| {
            let (server, holders, acquired) = (server.clone(), holders.clone(), acquired.clone());
            tokio::spawn(async move {
                let owner = format!("client-{id}");
                let lock = [
                    "lock",
                    "lock/build",
                    "--ttl",
                    "30",
                    "--wait",
                    "--interval",
                    "10",
                    "--owner",
                    &owner,
                ];
                let (status, printed) = server.run(&lock).await;
                assert_eq!(status, ExitStatus::Success);
                assert_eq!(printed, format!("{owner}\n"));
                assert_eq!(
                    holders.fetch_add(1, Ordering::SeqCst),
                    0,
                    "two holders at once"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
                holders.fetch_sub(1, Ordering::SeqCst);
                acquired.fetch_add(1, Ordering::SeqCst);
                let unlock = ["unlock", "lock/build", "--owner", &owner];
                assert_eq!(server.run(&unlock).await.0, ExitStatus::Success);
            })
        })
        .collect();
    for client in clients {
        client.await.unwrap();
    }
    assert_eq!(acquired.load(Ordering::SeqCst), 50);
}

#[tokio::test]
async fn lock_wait_acquires_after_the_holders_ttl_expires() {
    let server = TestServer::start().await;
    let held = ["lock", "lock/build", "--ttl", "1", "--owner", "holder"];
    assert_eq!(server.run(&held).await.0, ExitStatus::Success);
    let started = Instant::now();
    let waiter = [
        "lock",
        "lock/build",
        "--ttl",
        "30",
        "--wait",
        "--interval",
        "50",
        "--owner",
        "waiter",
    ];
    assert_eq!(
        server.run(&waiter).await,
        (ExitStatus::Success, "waiter\n".into())
    );
    assert!(started.elapsed() >= Duration::from_millis(500));
    assert_eq!(server.run(&["get", "lock/build"]).await.1, "waiter");
}

#[tokio::test]
async fn lock_wait_times_out_with_status_4() {
    let server = TestServer::start().await;
    let held = ["lock", "lock/build", "--ttl", "60", "--owner", "holder"];
    assert_eq!(server.run(&held).await.0, ExitStatus::Success);
    let waiter = [
        "lock",
        "lock/build",
        "--ttl",
        "60",
        "--wait",
        "--timeout",
        "1",
        "--interval",
        "50",
    ];
    assert_eq!(server.run(&waiter).await.0, ExitStatus::TimedOut);
    assert_eq!(server.run(&["get", "lock/build"]).await.1, "holder");
}
