# Agents

## Project

frork is a Fennel-based configuration management tool inspired by [bork](https://bork.sh/), rewritten in Rust. It evaluates declarative Fennel scripts that describe the desired state of a system (symlinks, directories, git repos, Homebrew packages) and can check or apply (create or update) each assertion.

## Repository layout

```
Cargo.toml                # Workspace root (frork-cli, frork-lib, frork-lua)
Cargo.lock
flake.nix                  # Dev shell, plus frork and frork-lua packages
flake.lock
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
    release.yml            # CalVer release on CI success: macOS + Linux tarballs, DotSlash
frork-cli/
  Cargo.toml               # Binary crate — the `frork` CLI; vendors Lua
  src/
    bin/frork/
      main.rs              # Entrypoint — clap CLI and status rendering only
  tests/
    cli.rs                 # Integration tests via assert_cmd
frork-lib/
  Cargo.toml               # The engine, shared by frork-cli and frork-lua
  build.rs                 # Sets FRORK_VERSION
  fennel-1.6.0.lua         # Vendored Fennel compiler
  src/
    lib.rs                 # Library root — re-exports modules
    assertions.rs          # Built-in providers and the Assertion trait
    error.rs               # thiserror + miette::Diagnostic enum (FrorkError)
    registry.rs            # Provider name to factory dispatch
    report.rs              # Status wording, apply actions, and update-prompt parsing
    runtime.rs             # Lua/Fennel setup, the frork module, run_code/run_script
    utils.rs               # Shell helpers, path expansion, Lua bindings
  tests/
    property.rs            # Hegel property tests across the library's public API
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
- All domain logic lives in frork-lib; `main.rs` parses arguments and performs I/O, and depends on the library rather than re-declaring its modules. `bin/coverage` ignores it and `.cargo/mutants.toml` excludes it, so anything with a decision in it goes in a library module — `report.rs` holds the status wording, the choice of what `apply` does, and the update-prompt parsing for exactly that reason.
- `just mutants` passes with zero survivors. Treat a new survivor as a missing assertion, not a reason to widen the exclusions.
- Error handling: `miette` in the binary, `thiserror` in library code. Library errors derive `miette::Diagnostic`.
- Logging uses `tracing` with `tracing-subscriber`. Use `tracing::info`, `tracing::debug`, etc. — not `println!` for diagnostic output.
- Lua integration via `mlua` with Lua 5.4: vendored by frork-cli, the host's for frork-lua. The Fennel compiler is vendored as a Lua source file in frork-lib.
- Filesystem operations use `fs-err` instead of `std::fs`. The `.clippy.toml` disallows bare `std::fs` types and methods so this is enforced at lint time.
- `.clippy.toml` also bans `Iterator::for_each` and `try_for_each` — use `for` loops for side effects, and `String::from_utf8_lossy`, which silently corrupts non-UTF-8 bytes from the OS.
- `[workspace.lints.clippy]` in the root `Cargo.toml` warns on `unwrap`, `expect`, `panic!`, slice indexing, and unchecked arithmetic; every member crate opts in via `[lints] workspace = true`. Opt out per call site with `#[allow(...)]` and a comment explaining why the invariant holds. Test code is exempt via `#![cfg_attr(test, allow(...))]`.
- All CI checks must pass: `cargo fmt --all --check`, `cargo clippy`, `cargo test`.

## Versioning and release

`frork-lib/build.rs` sets a `FRORK_VERSION` env var that `frork-lib::VERSION` is built from — both `frork --version` and the Lua module's `VERSION` report it. CI passes CalVer (`YYYY-MM-DD+SHORT_SHA`); local builds substitute the jj change ID and append `-dev`, so a development binary is never mistaken for a release. The crate version in `Cargo.toml` is not what ships.

Releases run on green main builds and attach tarballs for aarch64 macOS and aarch64/x86_64 Linux, each built on a native runner, plus a [DotSlash](https://dotslash-cli.com) launcher.

## Coverage

`bin/coverage` measures library line coverage with grcov and gates on `COVERAGE_THRESHOLD`, which defaults to 100. The library is at 100% — keep it there rather than lowering the gate.

Unimplemented stubs are excluded rather than counted: the `--excl-line` pattern covers `todo!` alongside `unreachable!` and the explicit `cov-excl-line` marker. An unimplemented method sits inside a `cov-excl-start`/`cov-excl-stop` pair because a function signature carries its own counter that no line pattern can match — the per-impl `update` stubs all do this. A trailing `// cov-excl-line` on the signature won't work either: rustfmt moves it to its own line inside the body.

Coverage regions land on their own line in two cases worth knowing, since both look like untestable code and are neither:

- A `?` on a multi-line call puts the error path on the closing `)?;` line. Bind the arguments first so the call fits on one line.
- A lazily-formatted `debug!` argument may never be counted where it sits. Bind it to a local before the macro — and if the arguments are already plain locals, nightly still maps phantom zero-count regions onto the args line, so mark that line `cov-excl-line` (the subscriber test in `utils.rs` does exercise them at runtime).

Three workspace-specific details the script depends on:

- `LLVM_PROFILE_FILE` must be an absolute path. Cargo runs test binaries with the working directory set to the package root, so a relative path scatters `.profraw` files into each crate's own `target/` and grcov silently reports 0%.
- `--keep-only 'frork-*/src/**'` scopes the report to workspace crates. A bare `src/**` matches nothing here, and `*/src/**` also picks up dependency sources from the cargo registry.
- `GRCOV_LLVM_PATH`, when set, reaches grcov as `--llvm-path`. Without it, grcov only finds rustup's `llvm-tools` component, which nixpkgs' rustc lacks, so the Nix dev shell points it at the LLVM that rustc was built with.

## Architecture notes

### CLI

The CLI (`frork`) uses clap with two subcommands, each taking either a `.fnl` script or inline Fennel code via `-e`/`--eval`. The two are mutually exclusive, and exactly one is required.

- `check <script>` / `check -e <code>` — report the status of each assertion
- `apply <script>` / `apply -e <code>` — create absent assertions, update drifted ones after an `update? [y/N]` prompt, and remove present or drifted `no` assertions without one

A `--completions <shell>` flag generates shell completions and exits.

### Provider model

Each line in a script is an assertion, declared with `ok` when it should hold or `no` when it should be gone, and its first argument names the provider that inspects and changes that sort of system state. Each built-in is a struct holding one assertion's arguments and implementing the `Assertion` trait (`status`, `create`, `update`, `remove`). Status returns `Present`, `Absent`, or `Drifted(Drift)`, where `Drift` carries `expected` and `actual` strings.

Built-in providers:

- `symlink` — manages symlinks (target, source)
- `directory` — ensures a directory exists
- `git` — clones a git repo to a directory, checks the remote URL
- `brew` — checks whether Homebrew is installed
- `brew-bundle` — runs `brew bundle check`/`install` against a Brewfile (macOS only)
- `debug` — accepts Lua functions for status/create/remove, used for testing and one-off assertions

Every `remove` deletes only what its provider would have created, and anything else is an error rather than a skip. `Symlink` removes the link when it points to the source; `Directory` removes only an empty directory; `Git` removes a clone only when it has the declared remote and `Git::unsaved_work` finds no uncommitted or untracked files and no commits missing from every remote. A drifted target, a non-empty directory, or a symlinked clone is `FrorkError::RemoveRefused`, unsaved work in a clone is `RemoveUnsaved`, and `Brew` and `BrewBundle` always return `RemoveUnsupported`. `Debug` calls its `remove` function and errors without one.

`Brew` and `BrewBundle` reach the shell through a `CommandRunner` generic that defaults to `SystemRunner`, so tests substitute a fake instead of running Homebrew. The default type parameter keeps `TypedFactory::<Brew>::new()` and every Fennel script unchanged. `Git` deliberately does not use the seam — its tests run real `git` against a local bare repo, which is offline and tests the actual invocations.

The `Providers` struct in `registry.rs` dispatches provider names. It checks providers defined in Lua first, then falls back to the built-ins. Each built-in is built through a `TypedFactory<T>`, whose `ProviderFactory::build` handles Lua argument conversion via `FromLuaMulti`.

### Lua/Fennel bridge

`runtime.rs` holds everything that touches Lua. `setup_lua` creates a Lua VM, loads the Fennel compiler, and registers a `frork` module with:

- `frork.ok(provider, ...)` — assert that a condition holds (dispatches through `Providers`)
- `frork.no(provider, ...)` — assert that a condition does not hold, the same way
- `frork.provide(name, {status=fn, create=fn, update=fn, remove=fn})` — define a provider from Fennel
- `frork.utils` — utility functions exposed to Lua/Fennel scripts

`run_code` also sets `ok` and `no` as globals for `-e` code; a script requires them from `frork`. Both declarations go through `Frork::declare`, which builds the assertion, evaluates its status, and passes the `Declaration` (`Ok` or `No`) with the status and assertion to the `StatusHandler`. The handler belongs to the caller: `report::render` turns the three into the printed lines, and `report::action` maps a declaration and status to an `Action` (`Nothing`, `Create`, `PromptUpdate`, or `Remove`), so frork-cli and frork-lua share the decision and differ only in how they carry it out.

### Utils module

`frork.utils` exposes these functions to Lua:

- `expand-path` — expands `~` and `$ENV_VAR` in paths
- `dirname` — returns the parent directory of a path
- `chomp` — trims trailing newlines
- `platform` — returns the lowercase OS name (via `uname -s`)
- `sh` — runs a shell command, returns `(stdout, stderr, exit_code)` or `(nil, nil, -1)` on failure
- `sh!` — like `sh` but propagates errors to Lua
- `assert-bin` — checks that a binary exists in PATH

On the Rust side, `ExpandedPath` handles `~` and `$ENV_VAR` expansion at the Lua/Rust boundary and implements `FromLua` for transparent conversion.

### Custom providers

Fennel scripts can define providers via `frork.provide(name, {status=fn, create=fn, update=fn, remove=fn, display=fn})`. Each becomes a `LuaProvider` holding those hooks, and every assertion that names it is a `LuaAssertion` dispatched through the same `Assertion` trait. The status function returns `present`, `absent`, or `drifted` plus an `{expected, actual}` table. The optional `update` function runs on a `drifted` status when the user answers yes to the update prompt; when absent, updating falls back to `create`. The optional `remove` function runs when applying a `no` assertion that is `present` or `drifted`; when absent, removing is an error. The optional `display` function controls how the assertion is printed.

### frork-lib, frork-cli and frork-lua

frork-lib is the engine and depends on neither of the others. frork-cli wraps it in the `frork` CLI with a vendored Lua and is the primary interface. frork-lua exposes it as a Lua module (`require("frork")`): a `cdylib`, and a `staticlib` for linking into a program that hosts its own Lua, such as one from `fennel --compile-binary --native-module`; mlua runs in `module` mode there, against the host's Lua. frork-cli and frork-lua cannot be built together due to those conflicting mlua features.

frork-lua's handler only reports unless the program was run with `--apply`, and then it creates absent `ok` assertions and removes present or drifted `no` ones. It never prompts, so drifted `ok` assertions are reported and left alone.
