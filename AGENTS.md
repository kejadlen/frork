# Agents

## Project

frork is a Fennel-based configuration management tool inspired by [bork](https://bork.sh/), rewritten in Rust. It evaluates declarative Fennel scripts that describe the desired state of a system (symlinks, directories, git repos, Homebrew packages) and can check status or satisfy (install/fix) each assertion.

## Repository layout

```
Cargo.toml                # Workspace root (frork-cli, frork-lib, frork-lua)
Cargo.lock
README.md
justfile                   # Local dev tasks (fmt, clippy, coverage, mutants, install)
bin/coverage               # grcov coverage report and threshold gate
.clippy.toml               # Enforces fs-err over std::fs, bans for_each
.envrc                     # direnv config
.gitignore
.cargo/
  config.toml              # macOS linker flags for cdylib, rust-analyzer target dir
  mutants.toml             # cargo-mutants exclusions
.ramekin/
  Dockerfile               # Agent container (Node.js + pi + Rust nightly)
.github/
  dotslash-config.json     # DotSlash launcher config for releases
  workflows/
    ci.yml                 # fmt + clippy + coverage
    release.yml            # CalVer release on CI success, plus DotSlash
frork-cli/
  Cargo.toml               # Binary crate — the `frork` CLI
  build.rs                 # Sets FRORK_VERSION
  fennel-1.6.0.lua         # Vendored Fennel compiler
  src/
    main.rs                # Entrypoint — clap CLI and status rendering only
    lib.rs                 # Library root — re-exports modules
    assertions.rs          # Assertion types and the AssertionType trait
    error.rs               # thiserror + miette::Diagnostic enum (FrorkError)
    registry.rs            # Assertion type name to factory dispatch
    report.rs              # Status wording and upgrade-prompt parsing
    runtime.rs             # Lua/Fennel setup, the frork module, run_code/run_script
    utils.rs               # Shell helpers, path expansion, Lua bindings
  tests/
    cli.rs                 # Integration tests via assert_cmd
frork-lib/
  Cargo.toml               # Shared library crate (currently empty)
  src/lib.rs
frork-lua/
  Cargo.toml               # Lua C module crate (cdylib)
  src/lib.rs               # mlua module exposing frork to Lua
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
just                        # default: fmt + clippy + coverage
just mutants                # mutation testing via cargo-mutants
just install                # cargo install --locked --path frork-cli
```

`cargo check --workspace` fails because frork-cli uses mlua's `vendored` feature and frork-lua uses `module` — these are mutually exclusive. Always check crates individually.

## Conventions

- Rust edition 2024, resolver v3 workspace.
- All domain logic lives in the library; `main.rs` parses arguments and performs I/O, and depends on the library rather than re-declaring its modules. `bin/coverage` ignores it and `.cargo/mutants.toml` excludes it, so anything with a decision in it goes in a library module — `report.rs` holds the status wording and the upgrade-prompt parsing for exactly that reason.
- `just mutants` passes with zero survivors. Treat a new survivor as a missing assertion, not a reason to widen the exclusions.
- Error handling: `miette` in the binary, `thiserror` in library code. Library errors derive `miette::Diagnostic`.
- Logging uses `tracing` with `tracing-subscriber`. Use `tracing::info`, `tracing::debug`, etc. — not `println!` for diagnostic output.
- Lua integration via `mlua` with vendored Lua 5.4. The Fennel compiler is vendored as a Lua source file.
- Filesystem operations use `fs-err` instead of `std::fs`. The `.clippy.toml` disallows bare `std::fs` types and methods so this is enforced at lint time.
- `.clippy.toml` also bans `Iterator::for_each` and `try_for_each` — use `for` loops for side effects, and `String::from_utf8_lossy`, which silently corrupts non-UTF-8 bytes from the OS.
- `[lints.clippy]` in `frork-cli/Cargo.toml` warns on `unwrap`, `expect`, `panic!`, slice indexing, and unchecked arithmetic. Opt out per call site with `#[allow(...)]` and a comment explaining why the invariant holds. Test code is exempt via `#![cfg_attr(test, allow(...))]`.
- All CI checks must pass: `cargo fmt --all --check`, `cargo clippy`, `cargo test`.

## Versioning and release

`frork-cli/build.rs` sets a `FRORK_VERSION` env var that `--version` reports. CI passes CalVer (`YYYY-MM-DD+SHORT_SHA`); local builds substitute the jj change ID and append `-dev`, so a development binary is never mistaken for a release. The crate version in `Cargo.toml` is not what ships.

Releases run on green main builds and attach a macOS aarch64 tarball plus a [DotSlash](https://dotslash-cli.com) launcher.

## Coverage

`bin/coverage` measures library line coverage with grcov and gates on `COVERAGE_THRESHOLD`, which defaults to 100. The library is at 100% — keep it there rather than lowering the gate.

Unimplemented stubs are excluded rather than counted: the `--excl-line` pattern covers `todo!` alongside `unreachable!` and the explicit `cov-excl-line` marker. An unimplemented method sits inside a `cov-excl-start`/`cov-excl-stop` pair because a function signature carries its own counter that no line pattern can match — the per-impl `upgrade` stubs and the default `remove` on `AssertionType` all do this. A trailing `// cov-excl-line` on the signature won't work either: rustfmt moves it to its own line inside the body.

Coverage regions land on their own line in two cases worth knowing, since both look like untestable code and are neither:

- A `?` on a multi-line call puts the error path on the closing `)?;` line. Bind the arguments first so the call fits on one line.
- A lazily-formatted `debug!` argument may never be counted where it sits. Bind it to a local before the macro.

Two workspace-specific details the script depends on:

- `LLVM_PROFILE_FILE` must be an absolute path. Cargo runs test binaries with the working directory set to the package root, so a relative path scatters `.profraw` files into each crate's own `target/` and grcov silently reports 0%.
- `--keep-only 'frork-*/src/**'` scopes the report to workspace crates. A bare `src/**` matches nothing here, and `*/src/**` also picks up dependency sources from the cargo registry.

## Architecture notes

### CLI

The CLI (`frork`) uses clap with four subcommands:

- `check <code>` — evaluate inline Fennel code, report status only
- `do <code>` — evaluate inline Fennel code, satisfy missing assertions
- `status <script>` — evaluate a `.fnl` script file, report status only
- `satisfy <script>` — evaluate a `.fnl` script file, satisfy missing assertions

A `--completions <shell>` flag generates shell completions and exits.

### Assertion model

Each assertion type implements the `AssertionType` trait (`status`, `install`, `upgrade`, `remove`). Status returns `Ok`, `Missing`, or `ConflictUpgrade`.

Built-in assertion types:

- `symlink` — manages symlinks (target, source)
- `directory` — ensures a directory exists
- `git` — clones a git repo to a directory, checks the remote URL
- `brew` — checks whether Homebrew is installed
- `brew-bundle` — runs `brew bundle check`/`install` against a Brewfile (macOS only)
- `debug` — accepts Lua functions for status/install, used for testing and one-off assertions

`Brew` and `BrewBundle` reach the shell through a `CommandRunner` generic that defaults to `SystemRunner`, so tests substitute a fake instead of running Homebrew. The default type parameter keeps `TypedFactory::<Brew>::new()` and every Fennel script unchanged. `Git` deliberately does not use the seam — its tests run real `git` against a local bare repo, which is offline and tests the actual invocations.

The `Registry` struct in `registry.rs` dispatches assertion types. It checks Lua-registered types first, then falls back to built-in types. Each built-in type is created through a `TypedFactory<T>` that handles Lua argument conversion via `FromLuaMulti`.

### Lua/Fennel bridge

`runtime.rs` holds everything that touches Lua. `setup_lua` creates a Lua VM, loads the Fennel compiler, and registers a `frork` module with:

- `frork.ok(type, ...)` — assert that a condition holds (dispatches through the registry)
- `frork.register(name, {status=fn, install=fn})` — register a custom assertion type from Fennel
- `frork.utils` — utility functions exposed to Lua/Fennel scripts

### Utils module

`frork.utils` exposes these functions to Lua:

- `expand-path` — expands `~` and `$ENV_VAR` in paths
- `dirname` — returns the parent directory of a path
- `chomp` — trims trailing newlines
- `platform` — returns the lowercase OS name (via `uname -s`)
- `sh` — runs a shell command, returns `(stdout, exit_code)` or `(nil, -1)` on failure
- `sh!` — like `sh` but propagates errors to Lua
- `assert-bin` — checks that a binary exists in PATH

On the Rust side, `ExpandedPath` handles `~` and `$ENV_VAR` expansion at the Lua/Rust boundary and implements `FromLua` for transparent conversion.

### Custom assertions

Fennel scripts can register new assertion types via `frork.register(name, {status=fn, install=fn, display=fn})`. These become `LuaAssertionType` values dispatched through the same `AssertionType` trait. The optional `display` function controls how the assertion is printed.

### frork-lua vs frork-cli

frork-lua builds a `cdylib` for use as a standalone Lua module (`require("frork")`). frork-cli embeds Lua/Fennel and is the primary interface. The two crates cannot be built together due to conflicting mlua features.
