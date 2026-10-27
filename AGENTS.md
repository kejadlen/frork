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
    release.yml            # CalVer release on CI success, per-target binaries, DotSlash
    standalone.yml         # Compiles the example for Linux/macOS, runs each
frork-cli/
  Cargo.toml               # Binary crate — the `frork` CLI; vendors Lua
  build.rs                 # Sets FRORK_VERSION
  src/
    bin/frork/
      main.rs              # Entrypoint — clap CLI and status rendering only
  tests/
    cli.rs                 # Integration tests via assert_cmd
frork-lib/
  Cargo.toml               # The engine, shared by frork-cli and frork-lua
  fennel-1.6.0.lua         # Vendored Fennel compiler
  src/
    lib.rs                 # Library root — re-exports modules
    assertions.rs          # Assertion types and the AssertionType trait
    compile.rs             # `frork compile`: Fennel to Lua, payload bundling, runner lookup
    error.rs               # thiserror + miette::Diagnostic enum (FrorkError)
    registry.rs            # Assertion type name to factory dispatch
    report.rs              # Status wording and upgrade-prompt parsing
    runtime.rs             # Lua/Fennel setup, the frork module, run_code/run_script
    utils.rs               # Shell helpers, path expansion, Lua bindings
  tests/
    property.rs            # Hegel property tests (chomp, upgrade prompt)
frork-lua/
  Cargo.toml               # Lua C module crate (cdylib + staticlib)
  src/lib.rs               # mlua module exposing frork-lib to Lua
```

## Build and test

```sh
cargo check -p frork-cli -p frork-lib   # Type-check (workspace has mlua feature conflict)
cargo fmt --all                         # Format all code
cargo clippy -p frork-cli -p frork-lib  # Lint; `just clippy` also lints frork-lua
cargo test -p frork-cli -p frork-lib    # Run tests
```

Or use `just` which runs fmt, clippy, and coverage together:

```sh
just                        # default: fmt + clippy + coverage
just mutants                # mutation testing via cargo-mutants
just install                # cargo install --locked --path frork-cli
```

`cargo check --workspace` fails because frork-cli uses mlua's `vendored` feature and frork-lua uses `module` — these are mutually exclusive. frork-lib picks neither and takes whichever the crate building it does, so check it alongside frork-cli and check frork-lua on its own.

## Conventions

- Rust edition 2024, resolver v3 workspace.
- All domain logic lives in frork-lib; `main.rs` parses arguments and performs I/O, and depends on the library rather than re-declaring its modules. `bin/coverage` ignores it and `.cargo/mutants.toml` excludes it, so anything with a decision in it goes in a library module — `report.rs` holds the status wording and the upgrade-prompt parsing for exactly that reason.
- `just mutants` passes with zero survivors. Treat a new survivor as a missing assertion, not a reason to widen the exclusions.
- Error handling: `miette` in the binary, `thiserror` in library code. Library errors derive `miette::Diagnostic`.
- Logging uses `tracing` with `tracing-subscriber`. Use `tracing::info`, `tracing::debug`, etc. — not `println!` for diagnostic output.
- Lua integration via `mlua` with Lua 5.4: vendored by frork-cli, the host's for frork-lua. The Fennel compiler is vendored as a Lua source file in frork-lib.
- Filesystem operations use `fs-err` instead of `std::fs`. The `.clippy.toml` disallows bare `std::fs` types and methods so this is enforced at lint time.
- `.clippy.toml` also bans `Iterator::for_each` and `try_for_each` — use `for` loops for side effects, and `String::from_utf8_lossy`, which silently corrupts non-UTF-8 bytes from the OS.
- `[workspace.lints.clippy]` in the root `Cargo.toml` warns on `unwrap`, `expect`, `panic!`, slice indexing, and unchecked arithmetic; every member crate opts in via `[lints] workspace = true`. Opt out per call site with `#[allow(...)]` and a comment explaining why the invariant holds. Test code is exempt via `#![cfg_attr(test, allow(...))]`.
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

The CLI (`frork`) uses clap with five subcommands:

- `check <code>` — evaluate inline Fennel code, report status only
- `do <code>` — evaluate inline Fennel code, satisfy missing assertions
- `status <script>` — evaluate a `.fnl` script file, report status only
- `satisfy <script>` — evaluate a `.fnl` script file, satisfy missing assertions
- `compile <script> -o <out>` — compile a script into a standalone executable (see below)

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
- `frork.register(name, {status=fn, install=fn, upgrade=fn})` — register a custom assertion type from Fennel
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

Fennel scripts can register new assertion types via `frork.register(name, {status=fn, install=fn, upgrade=fn, display=fn})`. These become `LuaAssertionType` values dispatched through the same `AssertionType` trait. The optional `upgrade` function handles a `conflict-upgrade` status when the user answers the upgrade prompt; when absent, upgrading falls back to `install`. The optional `display` function controls how the assertion is printed.

### frork-lib, frork-cli and frork-lua

frork-lib is the engine and depends on neither of the others. frork-cli wraps it in the `frork` CLI with a vendored Lua and is the primary interface. frork-lua exposes it as a Lua module (`require("frork")`): a `cdylib`, and a `staticlib` for linking into a program that hosts its own Lua, such as one from `fennel --compile-binary --native-module`; mlua runs in `module` mode there, against the host's Lua. frork-cli and frork-lua cannot be built together due to those conflicting mlua features.

### Standalone binaries

A standalone binary is a frork binary for the target followed by a payload: the script compiled to Lua (Fennel's `requireAsInclude` inlines sibling modules, and macros expand at compile time), its length as a little-endian `u64`, and an 8-byte magic. `main.rs` checks its own executable for that trailer before parsing arguments and, when it finds one, runs the payload instead of the CLI, satisfying when given `--satisfy`. `frork compile` uses its own executable for the host target, `--runner` when given, or else downloads `frork-<target>.tar.gz` from the release matching its version. Compiling is only Fennel and copying bytes, so no C compiler is involved for any target.
