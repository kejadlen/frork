# Vocabulary

Status: the renames are built; `(no …)`, removal, and the prompt's cost text are not.

frork's words grew one at a time, and a few of them now pull in
different directions. This proposes a single vocabulary that holds
together as frork gains `:file`, `:patch` and removal.

## What's wrong today

- `ok` is both the declaration, `(ok :symlink …)`, and a status,
  `ok: symlink …`.
- "Upgrade" really means "overwrite whatever drifted". For `:file`,
  saying yes discards a local edit, which is not an upgrade, and the
  prompt doesn't say what it will clobber.
- `status`/`satisfy` (scripts) and `check`/`do` (inline code) are two
  pairs of commands for one pair of jobs.
- "conflict (upgradable)" implies a non-upgradable conflict that
  doesn't exist.
- `install` is an odd verb for a symlink or a directory.
- "Assertion type" is a mouthful.
- `remove` exists on the trait but there is no way to ask for it.

## Principle

No puns, no metaphor. Names of real things on disk stay literal:
`:symlink`, `:directory`, `:git`, `:file`, `:patch`, and the `layered`
helper.

## Three layers

frork separates what you want, what is on disk, and what it does about
the difference.

### What you want: `ok` and `no`

```fennel
(local {: ok : no} (require :frork))

(ok :symlink "~/.gitconfig" "~/.dotfiles/.gitconfig")
(no :symlink "~/.vimrc")
```

`ok` asserts the thing should be there; `no` asserts it shouldn't.
Each line is an **assertion**, and `:symlink` names its **provider**.

### What is on disk: a provider's `status`

A provider's `status` reports only what it finds, with no knowledge of
whether the assertion was `ok` or `no`:

| Status    | Meaning                                         |
|-----------|-------------------------------------------------|
| `present` | Matches the assertion's arguments               |
| `absent`  | Not there at all                                |
| `drifted` | There, but different; carries a diff            |

### What frork does: `create`, `update`, `remove`

frork compares the declaration with the status:

| Declared | Status             | `apply` does             |
|----------|--------------------|--------------------------|
| `ok`     | `present`          | nothing                  |
| `ok`     | `absent`           | `create`                 |
| `ok`     | `drifted`          | asks, then `update`      |
| `no`     | `absent`           | nothing                  |
| `no`     | `present`          | `remove`                 |
| `no`     | `drifted`          | asks, then `remove`      |

A provider's hooks are `status` / `create` / `update` / `remove`, the
same shape as read/create/update/delete. `update` means "make the
existing thing match the declaration", which covers a `:git` remote
that was never the declared URL, not only a return to an earlier
state.

`drifted` always asks, because it means someone touched the thing
since frork last wrote it. Open: should removing a `present` thing ask
too, since removal is harder to undo than creation?

## Providers

"Assertion type" becomes **provider**, in Puppet's sense: the thing
that knows how to inspect and change one sort of system state.

```fennel
(local frork (require :frork))

(frork.provide :tmux-plugin
  {:status (fn [name] (if (installed? name) :present :absent))
   :create (fn [name] (install name))
   :update (fn [name] (reinstall name))
   :remove (fn [name] (uninstall name))
   :display (fn [name] name)})
```

- `frork.register` becomes `frork.provide`.
- Errors read `unknown provider :symlnk`.
- Docs say "Built-in providers" and "Writing a provider".
- In Rust, `AssertionType` becomes `Provider` and `Registry` becomes
  `Providers`.

Terraform uses "provider" for a whole plugin bundle, which is the one
echo to be aware of. Considered and passed over: `kind` (too generic),
`resource` (sounds like the thing on disk, and "no resource" reads
badly), `fixture` (collides with test fixtures), `subject` (fits the
sentence shape but sounds academic).

## Commands

One pair, each taking a script or inline code:

```
frork check dotfiles.fnl
frork check -e '(ok :directory "~/src")'
frork apply dotfiles.fnl
frork apply -e '(ok :directory "~/src")'
```

`check` reports; `apply` creates, updates and removes.

## Output

Each line repeats the declaration. A line with no suffix holds; a
suffix names the status that doesn't:

```
ok symlink ~/.gitconfig → ~/.dotfiles/.gitconfig
ok directory ~/.config/nvim: absent
ok file ~/.dotfiles/out/foo.toml: drifted
  editor.font-size: code 15, file 16  (overrides.work, dotfiles.fnl:12)
no symlink ~/.vimrc: present
```

Prompts name what they will destroy:

```
Update ~/.dotfiles/out/foo.toml? Overwrites the local change [y/N]
Remove ~/.vimrc? [y/N]
```

Declining prints `kept: …` rather than `skipped: …`.

## Fit with per-host config files

[Per-host config files](2026-11-18-per-host-config-files.md) carries
over unchanged in shape:

- `layered` emits `ok :file` plus `ok :symlink`, or just the symlink.
- The drift loop is `drifted` → `update` or keep and fold the edit
  back into the Fennel.
- `no` gives cleanup a home: `(no :file "~/.dotfiles/out/foo.toml")`
  when a host drops its override.
- `:patch`'s `delete` marker stays a key-level "must be absent"; `no`
  is the whole-assertion version.

## Rename table

| Today                          | Proposed                     |
|--------------------------------|------------------------------|
| `(ok …)`                       | `(ok …)`, plus `(no …)`      |
| assertion type                 | provider                     |
| `frork.register`               | `frork.provide`              |
| `status` / `install` / `upgrade` / `remove` hooks | `status` / `create` / `update` / `remove` |
| `:ok` / `:missing` / `:conflict-upgrade` | `:present` / `:absent` / `:drifted` |
| `frork status` / `frork check` | `frork check [-e]`           |
| `frork satisfy` / `frork do`   | `frork apply [-e]`           |
| `conflict (upgradable)`        | `: drifted`                  |
| `Upgrade? [y/N]`               | `Update <path>? <cost> [y/N]`|
| `skipped`                      | `kept`                       |
