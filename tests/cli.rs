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
