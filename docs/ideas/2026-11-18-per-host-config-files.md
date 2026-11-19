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
- **An override projects.** frork encodes the data to an out file and
  links the real path to that. Drift shows up as a key-level diff to
  fold back into the Fennel.
- **Generating data is just Fennel.** frork supplies `encode` and
  `decode` per format, not a transform language.
- **`:settings` is a separate tool** for files that apps rewrite
  themselves, patching only the keys frork manages.
- **No saved state.** Everything is derived from the repo and the
  files on disk.

Rule of thumb: if you'd commit the whole file, use `:layered`. If
you'd only commit some of its keys, use `:settings`.

## `:layered`: base plus host override

```fennel
(local {: ok} (require :frork))

(ok :layered "~/.config/foo.toml"
  {:base "foo/base.toml"
   :override (. overrides (utils.hostname))})   ; nil means identity
```

An override is a function or a table to merge in:

```fennel
(local overrides
  {:work {:editor {:font-size 15} :telemetry false}
   :laptop (fn [cfg] (doto cfg (tset :theme :light)))})
```

A convention could load `overrides/<hostname>.fnl` next to the script,
so adding a host override is just adding a file.

| Case        | On disk                                              | Where edits go                      |
|-------------|------------------------------------------------------|-------------------------------------|
| No override | `~/.config/foo.toml` → `foo/base.toml`               | Straight into the base file         |
| Override    | `~/.config/foo.toml` → `out/foo.toml`, written from `encode(override(decode(base)))` | The out file; status reports drift |

The identity case links the base file itself. It never decodes and
re-encodes, so frork doesn't reformat a file nobody overrode.

### Drift and the diff loop

Status compares the decoded out file with the data the code produces.
The diff is key-level, and `merge` provenance points at the Fennel
line that set the value:

```
conflict (upgradable): layered ~/.config/foo.toml
  editor.font-size: code 15, file 16  (overrides.work, dotfiles.fnl:12)
```

1. `satisfy` asks `Upgrade? [y/N]`.
2. Yes discards the edit and re-projects.
3. No keeps the edit. Update the Fennel until the diff is empty, and
   status returns to ok.

If an app's atomic save replaced the symlink with a regular file, that
file is what gets diffed, and upgrade restores the link.

## `:settings`: patching files apps own

For files you only partly own, such as editor settings the app
rewrites, frork patches the keys it manages in place and leaves the
rest alone.

```fennel
(local {: ok : settings} (require :frork))
(local {: delete : includes : where : merge} settings)

(ok :settings "~/.config/zed/settings.json"
  {:vim_mode true
   :buffer_font_size 14
   :theme {:dark "One Dark"}
   :features {:copilot delete}})
```

`(ok :settings path managed ?opts)` picks the format from the
extension; `{:format :toml}` overrides it for files like `.gitconfig`.

| Value in `managed`             | Meaning                                    |
|--------------------------------|--------------------------------------------|
| scalar                         | Key is set to this value                   |
| nested table                   | Recurse; keys not mentioned are left alone |
| `settings.delete`              | Key must be absent                         |
| sequential table               | List is exactly these elements             |
| `(includes [:a :b])`           | List contains at least these               |
| `(where {:name "prod"} {...})` | Apply edits to list elements that match    |

Status rolls up per file, with a line per offending key. Any conflict
makes the file a conflict; otherwise a missing key or file makes it
missing. Install and upgrade both patch the managed keys, and a
missing file is created as an empty document of its format.

## Implementation sketch

`:layered` needs only `frork.<fmt>` encode and decode per format, plus
the symlink and file logic. `:settings` needs more: edits apply to each
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
  `(settings.empty-map)` and `(settings.empty-list)`?
- INI is strings only and TOML has dates. Compare loosely as strings
  to start?
- KDL nodes carry positional args as well as props. A reserved `:args`
  key, such as `{:pane {:args [1] :size 50}}`?
- Should a sequential table mean exact or includes by default?
- Without state, frork can't notice a key you stop managing. Explicit
  `delete` is the only route for now.
- Two assertions on one file: merged, or applied in order?
- Where out files live: a gitignored dir in the repo (leaning this
  way, edits sit next to the code) or XDG state.
- Name: `:layered` or `:derived`.
- Should frork load `overrides/<hostname>.fnl` by convention, or leave
  lookup to the script?

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
