# Agents

## Project

frork is a Fennel-based configuration management tool inspired by [bork](https://bork.sh/), rewritten in Rust. It evaluates declarative Fennel scripts that describe the desired state of a system (symlinks, directories, git repos, Homebrew packages) and can check status or satisfy (install/fix) each assertion.

## Repository layout

```
Cargo.toml              # Workspace root (frork-cli, frork-lib, frork-lua)
Dockerfile              # Ramekin agent container (Node.js + pi + Rust nightly)
frork-cli/
  Cargo.toml            # Binary crate — the `frork` CLI
  fennel-1.6.0.lua      # Vendored Fennel compiler
  src/
    main.rs             # Entrypoint, clap CLI, Lua/Fennel setup
    lib.rs              # Library root — re-exports modules
    assertions.rs       # Assertion types (symlink, directory, git, brew, lua)
    errors.rs           # thiserror enum (FrorkError)
    utils.rs            # Shell helpers, path expansion, Lua bindings
  tests/
    cli.rs              # Integration tests via assert_cmd
frork-lib/
  Cargo.toml            # Shared library crate (currently empty)
  src/lib.rs
frork-lua/
  Cargo.toml            # Lua C module crate (cdylib)
  src/lib.rs            # mlua module exposing frork to Lua
justfile                # Local dev tasks (fmt, check, clippy, coverage)
.github/workflows/      # CI (fmt + clippy + coverage) and CalVer release
```

## Build and test

```sh
cargo check -p frork-cli    # Type-check the CLI (workspace has mlua feature conflict)
cargo fmt --all             # Format all code
cargo clippy -p frork-cli   # Lint
cargo test -p frork-cli     # Run tests
```

Or use `just` which runs fmt, clippy, and coverage together:

```sh
just
```

**Note:** `cargo check --workspace` fails due to mutually exclusive mlua features (`vendored` in frork-cli vs `module` in frork-lua). Check crates individually.

## Conventions

- **Rust edition 2024**, resolver v3 workspace.
- Error handling: `miette` in the binary, `thiserror` in library code. Library errors derive `miette::Diagnostic`.
- Logging uses `tracing` with `tracing-subscriber`. Use `tracing::info`, `tracing::debug`, etc. — not `println!` for diagnostic output.
- Lua integration via `mlua` with vendored Lua 5.4. Fennel compiler is vendored as a Lua source file.
- All CI checks must pass: `cargo fmt --all --check`, `cargo clippy`, `cargo test`.

## Architecture notes

- **Assertion model:** Each assertion type implements the `AssertionType` trait (`status`, `install`, `upgrade`, `remove`). Status returns `Ok`, `Missing`, or `ConflictUpgrade`.
- **Lua/Fennel bridge:** `setup_lua` creates a Lua VM, loads the Fennel compiler, and registers a `frork` module with `ok` (assert) and `register` (define custom assertion types) functions.
- **Custom assertions:** Fennel scripts can register new assertion types via `frork.register(name, {status=fn, install=fn})` — these become `LuaAssertionType` values dispatched through the same trait.
- **Path expansion:** `ExpandedPath` handles `~` and `$ENV_VAR` expansion at the Lua/Rust boundary.
- **frork-lua vs frork-cli:** frork-lua builds a `cdylib` for use as a standalone Lua module (`require("frork")`). frork-cli embeds Lua/Fennel and is the primary interface. The two crates cannot be built together due to conflicting mlua features.
