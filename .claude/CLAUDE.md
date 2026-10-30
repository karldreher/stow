# stow code style

Rules for agents and contributors. CI enforces the documentation rules.

- **Rustdoc everywhere.** Every module (`//!`), struct, enum, field, function, and constant, public or private, has a `///` doc comment saying what it does and why it exists. Add `# Errors` and `# Panics` sections where they apply.
- **Why-only comments.** Inline `//` comments explain hidden constraints, invariants, or platform workarounds. They never narrate what the code does.
- **Lib/bin split.** All logic lives in `src/lib.rs` and its modules. `src/main.rs` only parses arguments and starts the server.
- **Loopback by default.** The server binds `127.0.0.1` unless `--host` says otherwise.
- **Minimal dependencies.** Disable default features and enable only what is used. No logging framework; use `eprintln!` for startup and shutdown messages.
- **No speculation.** No abstractions, flags, or error handling for cases that cannot happen.
- **No dead code.** `dead_code` and `clippy::allow_attributes` are denied in `Cargo.toml`, so unused items fail the build and `#[allow(...)]` attributes are rejected. Delete unused items rather than silencing lints.
- **Conventional commits.** Lowercase, imperative, prefixed `feat:`, `fix:`, `refactor:`, etc., under 72 characters.
- **Distribution.** stow ships as a GitHub Release binary and is never published to crates.io (`publish = false`).
