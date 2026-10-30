# Contributing

## Pull request checks

Every pull request must pass these checks, defined in `.github/workflows/ci.yml`. Run the commands before opening a PR:

```sh
# lint
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings

# rustdoc (missing doc comments and broken intra-doc links fail)
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items --locked

# tests
cargo test --locked --verbose

# release build
cargo build --release --locked
./target/release/stow --help
```

