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
  created
ok git /Users/alpha/src/project https://github.com/user/project.git: absent
  created
```

A line with no suffix holds, and anything `apply` does about an
assertion is indented under it. When something is there but different,
the line ends in `drifted` and shows what differs, and `apply` asks
before updating it:

```
$ frork apply dotfiles.fnl
ok symlink /Users/alpha/.gitconfig /Users/alpha/.dotfiles/.gitconfig
ok directory /Users/alpha/.config/nvim
ok git /Users/alpha/src/project https://github.com/user/project.git: drifted
  expected: https://github.com/user/project.git
    actual: current remote: https://example.com/other.git
  update? [y/N] n
  kept
```

To say something should be gone, declare it with `no` instead of `ok`:

```fennel
(local {: ok : no} (require :frork))

(no :symlink "~/.vimrc" "~/.dotfiles/.vimrc")
(no :directory "~/.config/old-editor")
```

A `no` line with no suffix means the thing is already gone. Otherwise
it ends in `present`, and `apply` removes it without asking, since the
`no` already says it should go. A line ending in `drifted` means
something else is in the way, which the built-in providers refuse to
remove:

```
$ frork apply dotfiles.fnl
no symlink /Users/alpha/.vimrc /Users/alpha/.dotfiles/.vimrc: present
  removed
no directory /Users/alpha/.config/old-editor
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

Answering yes to the update prompt changes only what the provider
manages, and refuses anything else with an error that stops the run.
`symlink` repoints a link that points elsewhere, and refuses a file or
directory at the target. `git` points the clone's `origin` remote at
the declared URL, leaving its files and commits alone, and refuses a
file or a directory with no `origin` remote. The other built-in
providers can't update yet.

A `no` assertion removes only what the provider would have created,
and refuses anything else with an error that stops the run. `symlink`
removes the link only when it points to the source. `directory`
removes only an empty directory. `git` removes a clone only when it has
the declared remote and holds no uncommitted or untracked files and no
commits missing from every remote; ignored files don't count. A
symlinked clone is refused too. `brew` and `brew-bundle` don't support
removal, so a `no` on either errors when it finds Homebrew or the
packages installed.

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
             (install-thing name))
   :remove (fn [name]
             (uninstall-thing name))})

(frork.ok :my-thing "widget")
```

The table passed to `provide` must include `status` and `create`
functions. `status` returns `:present`, `:absent`, or `:drifted` along
with a table of `expected` and `actual` strings. An optional `update`
function runs when you answer yes to the update prompt for a drifted
assertion; without one, frork runs `create` again. An optional
`remove` function runs when `apply` meets a `no` assertion that is
present or drifted; without one, removing it is an error. An optional
`display` function controls how the assertion is printed.

## Inline assertions

For one-off checks without a script file, pass the code with `-e`:

```
$ frork check -e '(ok :directory "~/src")'
$ frork apply -e '(ok :directory "~/src")'
```

## Coming from bork

frork keeps bork's `ok` and `no` declarations and renames most of the
rest. See [bork's documentation](https://bork.sh/) for its side of the
table.

| bork | frork |
|------|-------|
| assertion type | provider |
| `register` | `frork.provide` |
| `status`, and `check` for a single command | `check`, with `-e` for inline code |
| `satisfy`, and `do` for a single command | `apply`, with `-e` for inline code |
| `ok` and `no` declarations | `ok` and `no` |
| `install` and `upgrade` actions | `create` and `update` hooks |
| `ok` status | no suffix, or `present` from a provider's `status` |
| `missing` | `absent` |
| `outdated`, `mismatch (upgradable)`, and `conflict (upgradable)` | `drifted` |

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
