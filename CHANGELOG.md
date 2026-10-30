# Changelog

All notable changes to stow are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-10-29

First release.

### Added

- `stow serve` starts a local in-memory key-value server on `127.0.0.1:7319`.
- HTTP endpoints to set, get, list, and delete keys, plus conditional writes and expiring leases for coordinating processes.
- `--save-file <PATH>` restores data on the next start.
- `--openapi` serves an OpenAPI 3.1 description of the API.
- `-q` / `--silent` hides startup and shutdown messages.
- Prebuilt binaries for macOS (arm64, x86_64) and Linux (arm64, x86_64, static musl) on each GitHub Release, with `SHA256SUMS`.
- `scripts/install.sh` downloads the right binary, checks its checksum, and installs it to `~/.local/bin`.
