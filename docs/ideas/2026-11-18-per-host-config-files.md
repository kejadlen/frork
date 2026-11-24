# Per-host config files

Status: idea. Nothing here is committed to or built.

Many config files share a base but need a few host-specific values
inside them. frork can't express that today: its only file mechanism
is `symlink`, which is identical on every host. The mechanism should
use one syntax across TOML, KDL, JSON, YAML and INI, and it should
keep the ability to edit a file in place and have the change land in
the repo.

## Background

lit handles the text version with `{{}}` placeholders and `inside=`.
That doesn't carry over: a text slot or comment-marked region can't be
the shared mechanism, because JSON has no comments and a base with
holes in it isn't a valid file on its own.

As of 980fbea, frork has no rendered-file type, no `utils.hostname`,
and every built-in `upgrade` is `todo!()`.

## Direction

- **Optimize for local overrides.** An override is a Fennel function
  from the decoded base to new data. A table is shorthand for a deep
  `merge`. No override means identity.
- **Identity is a plain symlink.** With no override, the real path
  links straight to the base file, so edits flow back into the repo
  exactly as they do today.
- **An override writes a file and links to it.** That is two existing
  kinds of assertion, `:file` and `:symlink`, not a new one. Drift on
  the written file shows up as a diff to fold back into the Fennel.
- **Generating data is just Fennel.** frork encodes and decodes by
  file extension; there is no transform language.
- **`:patch` is a separate tool** for files that apps rewrite
  themselves, patching only the keys frork manages.
- **No saved state.** Everything is derived from the repo and the
  files on disk.
- **The script picks the override, not frork.** A machine's role is
  the script it runs, so a private overlay repo's script passes its
  own overrides in. A hostname-keyed file in a public repo would
  publish private hostnames and config.
- **Apps that save atomically get `:file` at the real path.** An
  atomic save replaces a symlink with a regular file, so linking buys
  nothing; frork writes the file itself and reconciles any drift.

Rule of thumb: if you'd commit the whole file, use `:file` and
`:symlink`. If the app saves it atomically, drop the `:symlink` and
point `:file` at the real path. If you'd only commit some of its keys,
use `:patch`.

## `:file` plus `:symlink`: base plus host override

With no override, nothing new is needed:

```fennel
(ok :symlink "~/.config/foo.toml" "~/.dotfiles/foo/base.toml")
```

With one, write the file and link to it:

```fennel
(local {: ok : read} (require :frork))

(ok :file "~/.dotfiles/out/foo.toml" (work (read "foo/base.toml")))
(ok :symlink "~/.config/foo.toml" "~/.dotfiles/out/foo.toml")
```

`:file` asserts that a path has some content, and picks what to do
from what it's given:

| Content             | Written as                                                        |
|---------------------|-------------------------------------------------------------------|
| string              | Raw bytes                                                         |
| table               | Encoded by the target's extension                                 |
| table plus `{:format :ini}` | Encoded as that format, for files like `gitconfig`        |
| table, no known format | Error, rather than a guess                                     |

```fennel
(ok :file "~/.dotfiles/out/gitconfig" data {:format :ini})
(ok :file "~/.dotfiles/out/motd" "hello\n")
```

`read` is the mirror image: it decodes a file by its extension, or by
`{:format ...}`, so base files need no format named either.

An override is a function or a table to merge in:

```fennel
(local overrides
  {:work {:editor {:font-size 15} :telemetry false}
   :laptop (fn [cfg] (doto cfg (tset :theme :light)))})
```

A small Fennel helper can wrap the pair. It is a function, not an
assertion type, so status only ever shows `file` and `symlink`:

```fennel
(layered "~/.config/foo.toml" "foo/base.toml" (. overrides (utils.hostname)))
;; nil override: just the symlink to the base
;; otherwise:    a :file in out/ plus a :symlink to it
```

frork doesn't look overrides up by convention. The script that calls
`layered` decides where they come from, whether that's a hostname
table like the one above or a private repo's role-specific script.

| Case        | On disk                                                       | Where edits go                      |
|-------------|---------------------------------------------------------------|-------------------------------------|
| No override | `~/.config/foo.toml` → `foo/base.toml`                        | Straight into the base file         |
| Override    | `~/.config/foo.toml` → `out/foo.toml`, written by `:file`     | The out file; status reports drift  |

The identity case links the base file itself. It never decodes and
re-encodes, so frork doesn't reformat a file nobody overrode.

Encoding has to be deterministic. Lua tables have no key order, so a
naive encode could shuffle keys between runs and cause spurious
conflicts and git noise. Encoding with sorted keys fixes that, but it
also means a base file's own key order doesn't survive an override.

### Drift and the diff loop

When the file on disk differs from the content, `:file` reports a
conflict with a diff. For a format frork knows, the diff is key-level,
and `merge` provenance points at the Fennel line that set the value.
A string falls back to a line diff.

```
conflict (upgradable): file ~/.dotfiles/out/foo.toml
  editor.font-size: code 15, file 16  (overrides.work, dotfiles.fnl:12)
```

1. `satisfy` asks `Upgrade? [y/N]`.
2. Yes discards the edit and rewrites the file.
3. No keeps the edit. Update the Fennel until the diff is empty, and
   status returns to ok.

Out files are tracked, not ignored, on a private branch that never
gets pushed (in jj, `git.private-commits`). The last render is then a
commit, so `jj diff` shows drift line by line alongside frork's
key-level report. The rendered content can hold private overrides,
which is why the branch stays local.

Reconciliation starts two-way: the file on disk against the current
render, as above. That catches all drift but can't tell an app's edit
from an override change, so every difference prompts. If that proves
noisy, the tracked render can serve as the base of a three-way merge,
which resolves one-sided changes on its own. The base lives in the
repo, not in frork, so frork stays stateless either way.

An app that saves atomically would replace the symlink with a regular
file, so those files skip the link: `:file` writes the real path
directly, and the same diff loop reconciles it.

```fennel
(ok :file "~/.config/foo.toml" (work (read "foo/base.toml")))
```

## `:patch`: patching files apps own

For files you only partly own, such as editor settings the app
rewrites, frork patches the keys it manages in place and leaves the
rest alone.

```fennel
(local {: ok : patch} (require :frork))
(local {: delete : includes : where : merge} patch)

(ok :patch "~/.config/zed/settings.json"
  {:vim_mode true
   :buffer_font_size 14
   :theme {:dark "One Dark"}
   :features {:copilot delete}})
```

`(ok :patch path managed ?opts)` picks the format from the
extension; `{:format :toml}` overrides it for files like `.gitconfig`.

| Value in `managed`             | Meaning                                    |
|--------------------------------|--------------------------------------------|
| scalar                         | Key is set to this value                   |
| nested table                   | Recurse; keys not mentioned are left alone |
| `patch.delete`              | Key must be absent                         |
| sequential table               | List is exactly these elements             |
| `(includes [:a :b])`           | List contains at least these               |
| `(where {:name "prod"} {...})` | Apply edits to list elements that match    |

Status rolls up per file, with a line per offending key. Any conflict
makes the file a conflict; otherwise a missing key or file makes it
missing. Install and upgrade both patch the managed keys, and a
missing file is created as an empty document of its format.

## Implementation sketch

`:file` and `read` need only encode and decode per format, chosen by
extension, plus a key-level diff over decoded values. `:patch` needs more: edits apply to each
format's native document, and only comparisons go through a neutral
`Value`, so each format's crate keeps comments and layout where it can.

```rust
trait Format {
    type Doc;
    fn parse(src: &str) -> Result<Self::Doc>; // e.g. toml_edit::DocumentMut
    fn empty() -> Self::Doc;
    fn get(doc: &Self::Doc, path: &[Seg]) -> Option<Value>;
    fn set(doc: &mut Self::Doc, path: &[Seg], v: Value) -> Result<()>;
    fn remove(doc: &mut Self::Doc, path: &[Seg]) -> Result<()>;
    fn emit(doc: &Self::Doc) -> String;
}
```

| Format | Likely crate                  | Round-trip                         | Notes                                  |
|--------|-------------------------------|------------------------------------|----------------------------------------|
| TOML   | `toml_edit`                   | Keeps comments and layout          | Easiest; build first                   |
| JSON   | `serde_json` (preserve_order) | Keeps key order; no comments       | Build first                            |
| KDL    | `kdl`                         | Keeps formatting                   | Node model needs its own path segments |
| YAML   | to be chosen                  | Likely loses comments and anchors  | Weakest; verify crates first           |
| INI    | to be chosen                  | Most crates drop comments          | Strings only; two-level tree           |

## Open questions

- Lua writes an empty map and an empty list both as `{}`. Explicit
  `(patch.empty-map)` and `(patch.empty-list)`?
- INI is strings only and TOML has dates. Compare loosely as strings
  to start?
- KDL nodes carry positional args as well as props. A reserved `:args`
  key, such as `{:pane {:args [1] :size 50}}`?
- Should a sequential table mean exact or includes by default?
- Without state, frork can't notice a key you stop managing. Explicit
  `delete` is the only route for now.
- Two assertions on one file: merged, or applied in order?
- Name and home for the `layered` helper: part of `frork`, or left to
  scripts?

## Alternatives considered

| Option                                         | Why not                                                         |
|------------------------------------------------|-----------------------------------------------------------------|
| lit-style named slots in a text base           | Base isn't a valid file; doesn't work across formats            |
| Comment-marked regions with defaults           | JSON has no comments                                            |
| Whole-file ownership with a diff prompt        | Loses edits apps or people make to other keys                   |
| Three-way merge with a saved snapshot          | Adds state to frork, which can go stale                         |
| Copying edits back with a command (re-add)     | Symlinks cover the identity case; unclear layer for overrides   |
| Full XSLT or a selector DSL                    | Overkill; Fennel tables plus `where` cover it                   |
| Writing edits back into Fennel automatically   | Arbitrary Fennel can't be inverted; the diff loop covers it     |
| Projecting every file, even with no override   | Reformats files nobody overrode and loses native edit-back      |
| A `:layered` assertion type                    | A name for a mechanism, not a thing on disk; `:file` plus `:symlink` already say it |
| An `:override` option on `:symlink`            | The link would point somewhere other than its stated source     |
