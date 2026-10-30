# Contributing

Code style rules live in [.claude/CLAUDE.md](.claude/CLAUDE.md).

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

## Releasing

Releases are built by `.github/workflows/release.yml` when a semver tag is pushed.

1. Set `version` in `Cargo.toml` and refresh `Cargo.lock` (`cargo build`).
2. Merge to `main`, then tag and push: `git tag vX.Y.Z && git push origin vX.Y.Z`. The workflow fails if the tag and `Cargo.toml` version differ.
3. The workflow publishes the release with four archives, `SHA256SUMS`, and notes generated from merged PR titles. Confirm it by running the install script from the README.
