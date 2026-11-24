# frork

A Fennel-based configuration management tool inspired by
[bork](https://bork.sh/). Describe the desired state of your system in
declarative Fennel scripts, then check each assertion or apply it.

## Quick start

```fennel
;; dotfiles.fnl
(local {: ok} (require :frork))

(ok :symlink "~/.gitconfig" "~/.dotfiles/.gitconfig")
(ok :directory "~/.config/nvim")
(ok :git "~/src/project" "https://github.com/user/project.git")
```

Check what's absent:

```
$ frork check dotfiles.fnl
ok symlink /Users/alpha/.gitconfig /Users/alpha/.dotfiles/.gitconfig
ok directory /Users/alpha/.config/nvim: absent
ok git /Users/alpha/src/project https://github.com/user/project.git: absent
```

Create everything that's absent:

```
$ frork apply dotfiles.fnl
ok symlink /Users/alpha/.gitconfig /Users/alpha/.dotfiles/.gitconfig
ok directory /Users/alpha/.config/nvim: absent
ok directory /Users/alpha/.config/nvim
ok git /Users/alpha/src/project https://github.com/user/project.git: absent
ok git /Users/alpha/src/project https://github.com/user/project.git
```

A line with no suffix holds. When something is there but different,
the line ends in `drifted` and shows what differs, and `apply` asks
before updating it:

```
$ frork apply dotfiles.fnl
ok symlink /Users/alpha/.gitconfig /Users/alpha/.dotfiles/.gitconfig
ok directory /Users/alpha/.config/nvim
ok git /Users/alpha/src/project https://github.com/user/project.git: drifted
  expected: https://github.com/user/project.git
    actual: current remote: https://example.com/other.git
Update git /Users/alpha/src/project https://github.com/user/project.git? [y/N] n
kept: git /Users/alpha/src/project https://github.com/user/project.git
```

## Installation

```
cargo install --locked --path frork-cli
```

Or use `just install`.

With Nix, install the CLI from the flake:

```
nix profile install github:kejadlen/frork
```

The `frork-lua` package builds the Lua module, with `frork.so` in
`lib/lua/5.4` for Lua's `package.cpath`.

## Built-in providers

Each `ok` line is an assertion, and its first argument names the
provider that knows how to inspect and change that sort of thing.

| Provider | Arguments | Description |
|------|-----------|-------------|
| `symlink` | target, source | Ensure a symlink exists pointing to the source |
| `directory` | path | Ensure a directory exists |
| `git` | directory, remote URL | Ensure a git clone exists with the correct remote |
| `brew` | — | Ensure Homebrew is installed (macOS) |
| `brew-bundle` | Brewfile path | Ensure a Brewfile's packages are installed (macOS) |

Paths support `~` and `$ENV_VAR` expansion.

## Writing a provider

Add a provider from Fennel using `frork.provide`:

```fennel
(local frork (require :frork))

(frork.provide :my-thing
  {:status (fn [name]
             (case (thing-version name)
               nil :absent
               "1.0" :present
               v (values :drifted {:expected "1.0" :actual v})))
   :create (fn [name]
             (install-thing name))})

(frork.ok :my-thing "widget")
```

The table passed to `provide` must include `status` and `create`
functions. `status` returns `:present`, `:absent`, or `:drifted` along
with a table of `expected` and `actual` strings. An optional `update`
function runs when you answer yes to the update prompt for a drifted
assertion; without one, frork runs `create` again. An optional
`display` function controls how the assertion is printed.

## Inline assertions

For one-off checks without a script file, pass the code with `-e`:

```
$ frork check -e '(ok :directory "~/src")'
$ frork apply -e '(ok :directory "~/src")'
```

## Coming from bork

frork keeps bork's `ok` declaration and renames most of the rest. See
[bork's documentation](https://bork.sh/) for its side of the table.

| bork | frork |
|------|-------|
| assertion type | provider |
| `register` | `frork.provide` |
| `status`, and `check` for a single command | `check`, with `-e` for inline code |
| `satisfy`, and `do` for a single command | `apply`, with `-e` for inline code |
| `install` and `upgrade` actions | `create` and `update` hooks |
| `ok` status | no suffix, or `present` from a provider's `status` |
| `missing` | `absent` |
| `outdated`, `mismatch (upgradable)`, and `conflict (upgradable)` | `drifted` |

frork doesn't have bork's `no` declaration yet, or a way to remove
anything.

## Design goal: compiled Fennel binaries

A long-term goal is compiling Fennel scripts into standalone binaries —
no Rust toolchain or frork CLI needed at runtime. The frork-lua crate
exists to support this: it builds frork's assertion engine as a Lua
module, both a C dynamic library and a static one that Fennel's
`--compile-binary --native-module` can link into a standalone binary.

## Development

`nix develop` provides the Rust toolchain and the tools that `just`
needs, including grcov.

```
cargo check -p frork-cli -p frork-lib
cargo test -p frork-cli -p frork-lib
just              # fmt + clippy + coverage
```

Check crates individually — `cargo check --workspace` fails because
frork-cli and frork-lua use mutually exclusive mlua features.
