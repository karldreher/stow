//! Tests for command-line argument validation.

use clap::Parser;
use stow::cli::Cli;

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
    use stow::cli::Command;
    let size = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
        Command::Serve(serve) => serve.max_value_size,
    };
    assert_eq!(size(&["stow", "serve"]), 1024);
    assert_eq!(size(&["stow", "serve", "--max-value-size", "65536"]), 65536);
}

#[test]
fn openapi_flag_defaults_to_off() {
    use stow::cli::Command;
    let openapi = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
        Command::Serve(serve) => serve.openapi,
    };
    assert!(!openapi(&["stow", "serve"]));
    assert!(openapi(&["stow", "serve", "--openapi"]));
}
