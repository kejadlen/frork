# frork

A Fennel-based configuration management tool inspired by
[bork](https://bork.sh/). Describe the desired state of your system in
declarative Fennel scripts, then check status or satisfy each assertion.

## Quick start

```fennel
;; dotfiles.fnl
(local {: ok} (require :frork))

(ok :symlink "~/.gitconfig" "~/.dotfiles/.gitconfig")
(ok :directory "~/.config/nvim")
(ok :git "~/src/project" "https://github.com/user/project.git")
```

Check what's missing:

```
$ frork status dotfiles.fnl
ok: symlink /Users/alpha/.gitconfig /Users/alpha/.dotfiles/.gitconfig
missing: directory /Users/alpha/.config/nvim
missing: git /Users/alpha/src/project https://github.com/user/project.git
```

Fix everything:

```
$ frork satisfy dotfiles.fnl
ok: symlink /Users/alpha/.gitconfig /Users/alpha/.dotfiles/.gitconfig
ok: directory /Users/alpha/.config/nvim
ok: git /Users/alpha/src/project https://github.com/user/project.git
```

## Installation

```
cargo install --locked --path frork-cli
```

Or use `just install`.

## Built-in assertion types

| Type | Arguments | Description |
|------|-----------|-------------|
| `symlink` | target, source | Ensure a symlink exists pointing to the source |
| `directory` | path | Ensure a directory exists |
| `git` | directory, remote URL | Ensure a git clone exists with the correct remote |
| `brew` | — | Ensure Homebrew is installed (macOS) |
| `brew-bundle` | Brewfile path | Ensure a Brewfile's packages are installed (macOS) |

Paths support `~` and `$ENV_VAR` expansion.

## Custom assertion types

Register new assertion types from Fennel using `frork.register`:

```fennel
(local frork (require :frork))

(frork.register :my-thing
  {:status (fn [name]
             (if (check-thing name) :ok :missing))
   :install (fn [name]
              (install-thing name))})

(frork.ok :my-thing "widget")
```

The table passed to `register` must include `status` and `install`
functions. An optional `display` function controls how the assertion
is printed.

## Inline assertions

For one-off checks without a script file:

```
$ frork check '(ok :directory "~/src")'
$ frork do '(ok :directory "~/src")'
```

## Design goal: compiled Fennel binaries

A long-term goal is compiling Fennel scripts into standalone binaries —
no Rust toolchain or frork CLI needed at runtime. The frork-lua crate
exists to support this: it builds as a C dynamic library that exposes
frork's assertion engine as a Lua module, which Fennel's
`--compile-binary` could link against a static Lua library.

This doesn't work yet, but the desire for it drives the split between
frork-cli (embedded Lua, the primary interface) and frork-lua (cdylib,
future compilation target).

## Development

```
cargo check -p frork-cli
cargo test -p frork-cli
just              # fmt + clippy + coverage
```

Check crates individually — `cargo check --workspace` fails because
frork-cli and frork-lua use mutually exclusive mlua features.
