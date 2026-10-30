# Contributing

Code style rules live in [CLAUDE.md](CLAUDE.md).

## Pull request checks

Every pull request must pass these checks, defined in `.github/workflows/ci.yml`. Run the commands before opening a PR:

```sh
# lint
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings  # also fails on missing doc comments

# tests
cargo test --locked --verbose

# rustdoc (broken intra-doc links and other doc warnings fail)
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items --locked

# release build
cargo build --release --locked
./target/release/stow --help
```
