//! Tests for command-line argument validation.

use clap::Parser;
use stow::cli::{Cli, Command, ServeArgs, WaitFor};

/// Parses `args` and returns the `serve` arguments, failing the test for any
/// other subcommand.
fn serve_args(args: &[&str]) -> ServeArgs {
    match Cli::try_parse_from(args).unwrap().command {
        Command::Serve(serve) => serve,
        other => panic!("expected serve, got {other:?}"),
    }
}

#[test]
fn host_rejects_wildcard_addresses() {
    for host in ["0.0.0.0", "::"] {
        let err = Cli::try_parse_from(["stow", "serve", "--host", host]).unwrap_err();
        assert!(err.to_string().contains("every network interface"), "{err}");
    }
}

#[test]
fn host_accepts_loopback_addresses() {
    for host in ["127.0.0.1", "::1"] {
        assert!(Cli::try_parse_from(["stow", "serve", "--host", host]).is_ok());
    }
}

#[test]
fn silent_flag_is_accepted_before_and_after_the_subcommand() {
    for args in [["stow", "-q", "serve"], ["stow", "serve", "--silent"]] {
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(cli.silent, "{args:?}");
    }
    assert!(!Cli::try_parse_from(["stow", "serve"]).unwrap().silent);
}

#[test]
fn max_value_size_defaults_to_1024_and_is_configurable() {
    let size = |args: &[&str]| serve_args(args).max_value_size;
    assert_eq!(size(&["stow", "serve"]), 1024);
    assert_eq!(size(&["stow", "serve", "--max-value-size", "65536"]), 65536);
}

#[test]
fn max_value_size_is_capped_at_one_mebibyte() {
    let parse = |size: &str| Cli::try_parse_from(["stow", "serve", "--max-value-size", size]);
    assert_eq!(
        serve_args(&["stow", "serve", "--max-value-size", "1048576"]).max_value_size,
        1_048_576
    );
    let err = parse("1048577").unwrap_err();
    assert!(err.to_string().contains("1 MiB"), "{err}");
}

#[test]
fn openapi_flag_defaults_to_off() {
    let openapi = |args: &[&str]| serve_args(args).openapi;
    assert!(!openapi(&["stow", "serve"]));
    assert!(openapi(&["stow", "serve", "--openapi"]));
}

#[test]
fn save_file_is_optional_and_takes_a_path() {
    let without = serve_args(&["stow", "serve"]);
    let with = serve_args(&["stow", "serve", "--save-file", "dir/state.bin"]);
    assert_eq!(without.save_file, None);
    assert_eq!(with.save_file, Some("dir/state.bin".into()));
}

#[test]
fn version_flag_prints_the_crate_version() {
    let err = Cli::try_parse_from(["stow", "--version"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")), "{err}");
}

#[test]
fn wait_for_accepts_the_three_forms() {
    let parse = |cond: &str| match Cli::try_parse_from(["stow", "get", "k", "--wait-for", cond])
        .unwrap()
        .command
    {
        Command::Get(get) => get.wait_for,
        other => panic!("expected get, got {other:?}"),
    };
    assert_eq!(parse("present"), Some(WaitFor::Present));
    assert_eq!(parse("absent"), Some(WaitFor::Absent));
    assert_eq!(parse("value=done"), Some(WaitFor::Value("done".into())));
    assert_eq!(parse("value="), Some(WaitFor::Value(String::new())));
}

#[test]
fn wait_for_rejects_other_values() {
    let err = Cli::try_parse_from(["stow", "get", "k", "--wait-for", "gone"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    assert!(err.to_string().contains("value=<v>"), "{err}");
}

#[test]
fn wait_options_default_to_60_seconds_and_250_millis() {
    let Command::Get(get) = Cli::try_parse_from(["stow", "get", "k"]).unwrap().command else {
        panic!("expected get");
    };
    assert_eq!(get.wait_for, None);
    assert_eq!((get.wait.timeout, get.wait.interval), (60, 250));
}

#[test]
fn interval_must_be_positive() {
    assert!(Cli::try_parse_from(["stow", "get", "k", "--interval", "0"]).is_err());
}

#[test]
fn client_addr_defaults_to_loopback_and_is_configurable() {
    let addr = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
        Command::Keys(keys) => keys.client.addr.to_string(),
        other => panic!("expected keys, got {other:?}"),
    };
    assert_eq!(addr(&["stow", "keys"]), "127.0.0.1:7319");
    assert_eq!(
        addr(&["stow", "keys", "--addr", "127.0.0.1:9000"]),
        "127.0.0.1:9000"
    );
}

#[test]
fn lock_requires_a_ttl_and_unlock_requires_an_owner() {
    assert!(Cli::try_parse_from(["stow", "lock", "k"]).is_err());
    assert!(Cli::try_parse_from(["stow", "lock", "k", "--ttl", "60"]).is_ok());
    assert!(Cli::try_parse_from(["stow", "unlock", "k"]).is_err());
    assert!(Cli::try_parse_from(["stow", "unlock", "k", "--owner", "me"]).is_ok());
}

#[test]
fn set_rejects_if_absent_with_if_value() {
    let args = ["stow", "set", "k", "v", "--if-absent", "--if-value", "x"];
    assert!(Cli::try_parse_from(args).is_err());
}
