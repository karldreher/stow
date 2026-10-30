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

1. Set `version` in `Cargo.toml`, refresh `Cargo.lock` (`cargo build`), and add the version's entry to `CHANGELOG.md`.
2. Merge to `main`, then tag and push: `git tag vX.Y.Z && git push origin vX.Y.Z`. The workflow fails if the tag and `Cargo.toml` version differ.
3. The workflow creates a **draft** release with four archives and `SHA256SUMS`. Download them with `gh release download vX.Y.Z --dir <empty dir>` and try the install script against them: `STOW_BASE_URL=file://<dir> STOW_VERSION=X.Y.Z STOW_INSTALL_DIR=<tmp> sh scripts/install.sh`.
4. Publish the draft from the GitHub Releases page.
