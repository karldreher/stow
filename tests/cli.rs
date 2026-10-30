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
fn max_value_size_is_capped_at_one_mebibyte() {
    use stow::cli::Command;
    let parse = |size: &str| Cli::try_parse_from(["stow", "serve", "--max-value-size", size]);
    let Command::Serve(serve) = parse("1048576").unwrap().command;
    assert_eq!(serve.max_value_size, 1_048_576);
    let err = parse("1048577").unwrap_err();
    assert!(err.to_string().contains("1 MiB"), "{err}");
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

#[test]
fn save_file_is_optional_and_takes_a_path() {
    let without = Cli::try_parse_from(["stow", "serve"]).unwrap();
    let with = Cli::try_parse_from(["stow", "serve", "--save-file", "dir/state.bin"]).unwrap();
    let (stow::cli::Command::Serve(without), stow::cli::Command::Serve(with)) =
        (without.command, with.command);
    assert_eq!(without.save_file, None);
    assert_eq!(with.save_file, Some("dir/state.bin".into()));
}

#[test]
fn version_flag_prints_the_crate_version() {
    let err = Cli::try_parse_from(["stow", "--version"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")), "{err}");
}
