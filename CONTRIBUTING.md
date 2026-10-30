# Contributing

Code style rules live in [CLAUDE.md](CLAUDE.md).

```sh
cargo build
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items
```
