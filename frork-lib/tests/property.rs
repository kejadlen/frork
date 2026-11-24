// Property tests exercise invariants that hold across all inputs; unit tests
// in the modules pin down known-good fixtures.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]

use std::cell::RefCell;
use std::env;
use std::path::Path;
use std::rc::Rc;

use frork_lib::assertions::AssertionType;
use frork_lib::assertions::Conflict;
use frork_lib::assertions::Directory;
use frork_lib::assertions::Git;
use frork_lib::assertions::LuaAssertion;
use frork_lib::assertions::LuaAssertionType;
use frork_lib::assertions::Status;
use frork_lib::assertions::Symlink;
use frork_lib::error::FrorkError;
use frork_lib::registry::Registry;
use frork_lib::report;
use frork_lib::runtime;
use frork_lib::utils::ExpandedPath;
use frork_lib::utils::Utils;
use fs_err as fs;
use hegel::TestCase;
use hegel::generators::text;
use hegel::generators::{self as gs, Generator};
use mlua::prelude::*;
use tempfile::TempDir;

const BUILT_IN_TYPES: [&str; 6] = [
    "brew",
    "brew-bundle",
    "debug",
    "directory",
    "git",
    "symlink",
];

/// Every character `str::trim` strips: the Unicode `White_Space` property.
const WHITESPACE: &str = " \t\n\u{0B}\u{0C}\r\u{85}\u{A0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}";

/// A single filename that `expand_path` leaves alone and any filesystem
/// accepts: no separator, NUL, or `$`, not `.` or `..`, short enough to stay
/// under the 255-byte name limit, and no unassigned codepoints or
/// noncharacters, which APFS refuses.
fn path_component() -> impl Generator<String> {
    text()
        .min_size(1)
        .max_size(16)
        .exclude_categories(&["Cn"])
        .exclude_characters("/$")
        .filter(|s: &String| !s.contains('\0') && s != "." && s != "..")
}

/// Text salted with the fragments `expand_path` treats specially, so `$` and
/// `~` turn up often instead of almost never.
fn pathish(tc: &TestCase, fragments: &[&'static str]) -> String {
    let mut all = vec!["$", "~", "$$", "$1", "$-", "${", "/"];
    all.extend_from_slice(fragments);
    let pieces = tc.draw(gs::vecs(gs::sampled_from(all)).max_size(6));
    let mut path = String::new();
    for piece in pieces {
        path.push_str(piece);
        path.push_str(&tc.draw(text().max_size(4)));
    }
    path
}

fn references_a_variable(s: &str) -> bool {
    s.split('$')
        .skip(1)
        .any(|rest| rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'))
}

fn expanded(path: &Path) -> ExpandedPath {
    path.to_str().unwrap().parse().unwrap()
}

fn lua_strings(lua: &Lua, values: &[String]) -> LuaMultiValue {
    LuaMultiValue::from_vec(
        values
            .iter()
            .map(|v| LuaValue::String(lua.create_string(v).unwrap()))
            .collect(),
    )
}

fn lua_type(lua: &Lua, code: &str) -> LuaAssertionType {
    let value = lua.load(code).eval::<LuaValue>().unwrap();
    LuaAssertionType::from_lua(value, lua).unwrap()
}

fn conflict_table(lua: &Lua, expected: &str, actual: &str) -> LuaValue {
    let table = lua.create_table().unwrap();
    table.set("expected", expected).unwrap();
    table.set("actual", actual).unwrap();
    LuaValue::Table(table)
}

type Recorded = Rc<RefCell<Vec<String>>>;

fn recorder() -> (Recorded, impl runtime::StatusHandler) {
    let seen: Recorded = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let handler = move |status: &Status, assertion: &dyn AssertionType| {
        sink.borrow_mut().push(format!("{status:?}: {assertion}"));
        Ok(())
    };
    (seen, handler)
}

/// An assertion that is nothing but its display text, for testing rendering.
struct Named(String);

impl std::fmt::Display for Named {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AssertionType for Named {
    fn status(&self) -> miette::Result<Status> {
        Ok(Status::Ok)
    }

    fn install(&self) -> miette::Result<()> {
        Ok(())
    }

    fn upgrade(&self) -> miette::Result<()> {
        Ok(())
    }
}

// --- chomp ---

#[hegel::test]
fn chomp_strips_all_trailing_newlines(tc: TestCase) {
    let s = tc.draw(text());
    let chomped = Utils::chomp(&s);
    assert!(
        !chomped.ends_with('\n') && !chomped.ends_with('\r'),
        "chomp({s:?}) = {chomped:?}, which still ends in a newline"
    );
}

#[hegel::test]
fn chomp_is_idempotent(tc: TestCase) {
    let s = tc.draw(text());
    assert_eq!(
        Utils::chomp(&Utils::chomp(&s)),
        Utils::chomp(&s),
        "chomping {s:?} twice differs from chomping once"
    );
}

#[hegel::test]
fn chomp_only_removes_from_the_end(tc: TestCase) {
    let s = tc.draw(text());
    assert!(
        s.starts_with(&Utils::chomp(&s)),
        "chomp({s:?}) is not a prefix of its input"
    );
}

#[hegel::test]
fn chomp_never_grows_its_input(tc: TestCase) {
    let s = tc.draw(text());
    assert!(
        Utils::chomp(&s).len() <= s.len(),
        "chomp({s:?}) grew its input"
    );
}

#[hegel::test]
fn chomp_removes_exactly_the_trailing_newlines(tc: TestCase) {
    let body = tc.draw(text().filter(|s: &String| !s.ends_with(['\n', '\r'])));
    let newlines = tc.draw(text().alphabet("\r\n"));
    assert_eq!(Utils::chomp(&format!("{body}{newlines}")), body);
}

// --- upgrade prompt ---

#[hegel::test]
fn wants_upgrade_accepts_only_y_or_yes(tc: TestCase) {
    let s = tc.draw(text());
    if report::wants_upgrade(&s) {
        let normalized = s.trim().to_lowercase();
        assert!(
            normalized == "y" || normalized == "yes",
            "wants_upgrade({s:?}) accepted something other than y/yes"
        );
    }
}

#[hegel::test]
fn wants_upgrade_accepts_y_or_yes_in_any_case_and_padding(tc: TestCase) {
    let word = tc.draw(gs::sampled_from(vec!["y", "yes"]));
    let uppercase = tc.draw(
        gs::vecs(gs::booleans())
            .min_size(word.len())
            .max_size(word.len()),
    );
    let cased: String = word
        .chars()
        .zip(uppercase)
        .map(|(c, upper)| if upper { c.to_ascii_uppercase() } else { c })
        .collect();
    let before = tc.draw(text().alphabet(WHITESPACE));
    let after = tc.draw(text().alphabet(WHITESPACE));

    let reply = format!("{before}{cased}{after}");
    assert!(report::wants_upgrade(&reply), "{reply:?} was declined");
}

// --- expand_path ---

#[hegel::test]
fn expand_path_leaves_paths_without_tilde_or_variables_alone(tc: TestCase) {
    let path = pathish(&tc, &["a", "_"]);
    tc.assume(!path.starts_with('~') && !references_a_variable(&path));
    assert_eq!(Utils::expand_path(&path).unwrap(), path);
}

#[hegel::test]
fn expand_path_replaces_a_leading_tilde_with_home(tc: TestCase) {
    let rest = tc.draw(text().exclude_characters("$"));
    let home = env::var("HOME").unwrap();
    assert_eq!(
        Utils::expand_path(&format!("~/{rest}")).unwrap(),
        format!("{home}/{rest}")
    );
}

#[hegel::test]
fn expand_path_substitutes_a_variable_with_its_value_verbatim(tc: TestCase) {
    // A `$` reference inside the value must survive: expansion is one pass.
    let value = format!(
        "{}{}",
        tc.draw(text().filter(|s: &String| !s.contains('\0'))),
        tc.draw(gs::sampled_from(vec!["", "$HOME", "$FRORK_PBT_UNSET"]))
    );
    let prefix = tc.draw(
        text()
            .exclude_characters("$")
            .filter(|s: &String| !s.starts_with('~')),
    );
    let suffix = tc.draw(
        text()
            .exclude_characters("$")
            .filter(|s: &String| !s.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')),
    );
    let lookup = |name: &str| (name == "FRORK_PBT_SET").then(|| value.clone());

    assert_eq!(
        Utils::expand_path_with(&format!("{prefix}$FRORK_PBT_SET{suffix}"), lookup).unwrap(),
        format!("{prefix}{value}{suffix}")
    );
}

#[hegel::test]
fn expand_path_names_every_missing_variable(tc: TestCase) {
    let names: Vec<String> = tc
        .draw(
            gs::vecs(gs::from_regex("[A-Z0-9_]{1,8}"))
                .min_size(1)
                .max_size(4)
                .unique(true),
        )
        .into_iter()
        .map(|n| format!("FRORK_PBT_UNSET_{n}"))
        .collect();
    let path: String = names.iter().map(|n| format!("/${n}")).collect();

    let error = Utils::expand_path_with(&path, |_| None).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Environment variables not found: {}", names.join(", "))
    );
}

#[hegel::test]
fn expanded_path_entry_points_agree_with_expand_path(tc: TestCase) {
    let path = pathish(&tc, &["$HOME", "$FRORK_PBT_UNSET_AGREE"]);
    let expected = Utils::expand_path(&path).ok();

    let lua = Lua::new();
    let lua_path = LuaValue::String(lua.create_string(&path).unwrap());
    let results = [
        ExpandedPath::new(&path).ok(),
        path.parse::<ExpandedPath>().ok(),
        ExpandedPath::from_lua(lua_path, &lua).ok(),
    ];
    for result in results {
        let as_string = result.map(|p| {
            let views = [p.as_str().to_string(), p.to_string(), (*p).to_string()];
            assert!(views.iter().all(|v| *v == p.as_str()), "{views:?} differ");
            p.into_string()
        });
        assert_eq!(as_string, expected, "for {path:?}");
    }
}

// --- dirname ---

#[hegel::test]
fn dirname_returns_the_parent_directory(tc: TestCase) {
    let parents = tc.draw(gs::vecs(path_component()).max_size(4));
    let name = tc.draw(path_component());
    let trailing_slash = if tc.draw(gs::booleans()) { "/" } else { "" };
    let parent = format!("/{}", parents.join("/"));
    let path = if parents.is_empty() {
        format!("/{name}{trailing_slash}")
    } else {
        format!("{parent}/{name}{trailing_slash}")
    };

    assert_eq!(Utils::dirname(&path).unwrap(), Some(parent));
}

// --- Lua utils bindings ---

#[hegel::test]
fn lua_utils_agree_with_their_rust_functions(tc: TestCase) {
    let input = pathish(&tc, &["\n", "\r", "$HOME", "$FRORK_PBT_UNSET_LUA"]);
    let lua = Lua::new();
    let LuaValue::Table(utils) = Utils.into_lua(&lua).unwrap() else {
        panic!("utils is not a table");
    };
    let call = |name: &str| -> Option<Option<String>> {
        let function: LuaFunction = utils.get(name).unwrap();
        function.call::<Option<String>>(input.as_str()).ok()
    };

    assert_eq!(call("chomp"), Some(Some(Utils::chomp(&input))));
    assert_eq!(
        call("expand-path"),
        Utils::expand_path(&input).ok().map(Some)
    );
    assert_eq!(call("dirname"), Utils::dirname(&input).ok());
}

// --- report::render ---

#[hegel::test]
fn render_prefixes_ok_and_missing_with_their_status(tc: TestCase) {
    let display = tc.draw(text());
    let assertion = Named(display.clone());

    assert_eq!(
        report::render(&Status::Ok, &assertion),
        [format!("ok: {display}")]
    );
    assert_eq!(
        report::render(&Status::Missing, &assertion),
        [format!("missing: {display}")]
    );
}

#[hegel::test]
fn render_shows_both_sides_of_a_conflict_verbatim(tc: TestCase) {
    let display = tc.draw(text());
    let expected = tc.draw(text());
    let actual = tc.draw(text());
    let status = Status::ConflictUpgrade(Conflict {
        expected: expected.clone(),
        actual: actual.clone(),
    });

    assert_eq!(
        report::render(&status, &Named(display.clone())),
        [
            format!("conflict (upgradable): {display}"),
            format!("  expected: {expected}"),
            format!("    actual: {actual}"),
        ]
    );
}

// --- Status from Lua ---

#[hegel::test]
fn status_rejects_any_string_but_ok_or_missing(tc: TestCase) {
    let s = tc.draw(text().filter(|s: &String| s != "ok" && s != "missing"));
    let lua = Lua::new();
    assert!(Status::from_lua_multi(lua_strings(&lua, &[s]), &lua).is_err());
}

#[hegel::test]
fn status_keeps_both_sides_of_a_conflict(tc: TestCase) {
    let expected = tc.draw(text());
    let actual = tc.draw(text());
    let lua = Lua::new();
    let values = LuaMultiValue::from_vec(vec![
        LuaValue::String(lua.create_string("conflict-upgrade").unwrap()),
        conflict_table(&lua, &expected, &actual),
    ]);

    match Status::from_lua_multi(values, &lua).unwrap() {
        Status::ConflictUpgrade(conflict) => {
            assert_eq!((conflict.expected, conflict.actual), (expected, actual));
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[hegel::test]
fn status_rejects_a_conflict_under_any_other_name(tc: TestCase) {
    let s = tc.draw(text().filter(|s: &String| s != "conflict-upgrade"));
    let lua = Lua::new();
    let values = LuaMultiValue::from_vec(vec![
        LuaValue::String(lua.create_string(&s).unwrap()),
        conflict_table(&lua, "expected", "actual"),
    ]);

    assert!(Status::from_lua_multi(values, &lua).is_err());
}

// --- LuaAssertion ---

const NO_DISPLAY: &str =
    r#"return {status = function() return "ok" end, install = function() end}"#;

#[hegel::test]
fn lua_assertion_displays_its_name_and_args_by_default(tc: TestCase) {
    let name = tc.draw(text());
    let args = tc.draw(gs::vecs(text()).max_size(4));
    let lua = Lua::new();
    let assertion = LuaAssertion::new(&name, lua_strings(&lua, &args), lua_type(&lua, NO_DISPLAY));

    let words: Vec<&str> = std::iter::once(name.as_str())
        .chain(args.iter().map(String::as_str))
        .collect();
    assert_eq!(assertion.to_string(), words.join(" "));
}

#[hegel::test]
fn lua_assertion_falls_back_to_the_default_display_when_display_fails(tc: TestCase) {
    let name = tc.draw(text());
    let args = tc.draw(gs::vecs(text()).max_size(4));
    let broken_display = tc.draw(gs::sampled_from(vec![
        r#"function() error("boom") end"#,
        "function() return nil end",
        "function() return {} end",
    ]));
    let lua = Lua::new();
    let broken = lua_type(
        &lua,
        &format!(
            r#"return {{status = function() return "ok" end, install = function() end, display = {broken_display}}}"#
        ),
    );

    let with_broken = LuaAssertion::new(&name, lua_strings(&lua, &args), broken);
    let without = LuaAssertion::new(&name, lua_strings(&lua, &args), lua_type(&lua, NO_DISPLAY));
    assert_eq!(with_broken.to_string(), without.to_string());
}

#[hegel::test]
fn lua_assertion_hands_every_hook_its_args_verbatim(tc: TestCase) {
    let args = tc.draw(gs::vecs(text()).max_size(4));
    let lua = Lua::new();
    let recording = lua_type(
        &lua,
        r#"seen = {}
           return {
             display = function(...) seen.display = table.pack(...); return "" end,
             status = function(...) seen.status = table.pack(...); return "ok" end,
             install = function(...) seen.install = table.pack(...) end,
             upgrade = function(...) seen.upgrade = table.pack(...) end,
           }"#,
    );
    let assertion = LuaAssertion::new("recording", lua_strings(&lua, &args), recording);

    let _ = assertion.to_string();
    assertion.status().unwrap();
    assertion.install().unwrap();
    assertion.upgrade().unwrap();

    let seen: LuaTable = lua.globals().get("seen").unwrap();
    for hook in ["display", "status", "install", "upgrade"] {
        let packed: LuaTable = seen.get(hook).unwrap();
        let n: usize = packed.get("n").unwrap();
        let received: Vec<String> = (1..=n).map(|i| packed.get(i).unwrap()).collect();
        assert_eq!(received, args, "{hook} received different args");
    }
}

// --- Registry ---

#[hegel::test]
fn registry_rejects_any_name_it_does_not_know(tc: TestCase) {
    let name = tc.draw(text().filter(|n: &String| !BUILT_IN_TYPES.contains(&n.as_str())));
    let Err(error) = Registry::default().get_factory(&name) else {
        panic!("{name:?} resolved to a factory");
    };
    assert_eq!(error.to_string(), format!("Unknown assertion type: {name}"));
}

#[hegel::test]
fn registry_prefers_a_registered_type_under_any_name(tc: TestCase) {
    let name = if tc.draw(gs::booleans()) {
        tc.draw(gs::sampled_from(BUILT_IN_TYPES.to_vec()))
            .to_string()
    } else {
        tc.draw(text())
    };
    let lua = Lua::new();
    let mut registry = Registry::default();
    registry.register(
        &name,
        lua_type(
            &lua,
            r#"return {display = function() return "registered" end,
                       status = function() return "ok" end,
                       install = function() end}"#,
        ),
    );

    let factory = registry.get_factory(&name).unwrap();
    let assertion = factory.create(&lua, LuaMultiValue::new()).unwrap();
    assert_eq!(assertion.to_string(), "registered");
}

// --- FrorkError ---

fn frork_error(tc: &TestCase) -> FrorkError {
    let detail = tc.draw(text());
    match tc.draw(gs::integers::<u8>().max_value(4)) {
        0 => FrorkError::NoOperation,
        1 => FrorkError::UnknownAssertionType {
            assertion_type: detail,
        },
        2 => FrorkError::InvalidArguments(detail),
        3 => FrorkError::MissingAssertionType,
        _ => FrorkError::Lua(detail),
    }
}

fn raising(lua: &Lua, error: &FrorkError) -> LuaFunction {
    let error = error.clone();
    lua.create_function(move |_lua, ()| -> LuaResult<()> { Err(LuaError::external(error.clone())) })
        .unwrap()
}

#[hegel::test]
fn frork_error_survives_a_rust_callback(tc: TestCase) {
    let error = frork_error(&tc);
    let lua = Lua::new();

    let caught = raising(&lua, &error).call::<()>(()).unwrap_err();
    assert_eq!(FrorkError::from(caught), error);
}

#[hegel::test]
fn frork_error_survives_a_lua_caller(tc: TestCase) {
    let error = frork_error(&tc);
    let lua = Lua::new();
    let caller: LuaFunction = lua.load("return function(f) f() end").eval().unwrap();

    let caught = caller.call::<()>(raising(&lua, &error)).unwrap_err();
    assert_eq!(FrorkError::from(caught), error);
}

// --- Filesystem assertions ---

#[hegel::test]
fn directory_install_satisfies_any_nested_path(tc: TestCase) {
    let parts = tc.draw(gs::vecs(path_component()).min_size(1).max_size(4));
    let root = TempDir::new().unwrap();
    let directory = Directory {
        path: expanded(&root.path().join(parts.join("/"))),
    };

    assert!(matches!(directory.status().unwrap(), Status::Missing));
    directory.install().unwrap();
    assert!(matches!(directory.status().unwrap(), Status::Ok));
}

#[hegel::test]
fn symlink_install_satisfies_status_whether_or_not_the_source_exists(tc: TestCase) {
    let root = TempDir::new().unwrap();
    let sources = root.path().join("sources");
    fs::create_dir(&sources).unwrap();
    let source = sources.join(tc.draw(path_component()));
    if tc.draw(gs::booleans()) {
        fs::write(&source, "").unwrap();
    }
    let symlink = Symlink {
        target: expanded(&root.path().join("link")),
        source: expanded(&source),
    };

    assert!(matches!(symlink.status().unwrap(), Status::Missing));
    symlink.install().unwrap();
    assert!(matches!(symlink.status().unwrap(), Status::Ok));
}

/// A remote URL git will store and hand back unchanged: no control
/// characters, and no leading `-` that `git config` would parse as a flag.
fn remote_url() -> impl Generator<String> {
    text()
        .min_size(1)
        .exclude_categories(&["Cc"])
        .filter(|u: &String| !u.starts_with('-'))
}

fn repo_with_origin(root: &Path, url: &str) -> ExpandedPath {
    let repo = root.join("repo");
    let repo_str = repo.to_str().unwrap();
    let (_out, code) = Utils::sh("git", &["init", "-q", repo_str]).unwrap();
    assert_eq!(code, 0);
    let config = ["-C", repo_str, "config", "remote.origin.url", url];
    let (_out, code) = Utils::sh("git", &config).unwrap();
    assert_eq!(code, 0, "git config rejected {url:?}");
    expanded(&repo)
}

#[hegel::test(test_cases = 25)]
fn git_status_is_ok_for_the_configured_remote(tc: TestCase) {
    let url = tc.draw(remote_url());
    let root = TempDir::new().unwrap();
    let git = Git {
        dir: repo_with_origin(root.path(), &url),
        remote_url: url,
    };

    assert!(matches!(git.status().unwrap(), Status::Ok));
}

#[hegel::test(test_cases = 25)]
fn git_status_reports_any_other_remote_as_a_conflict(tc: TestCase) {
    let configured = tc.draw(remote_url());
    let wanted = tc.draw(remote_url());
    tc.assume(configured != wanted);
    let root = TempDir::new().unwrap();
    let git = Git {
        dir: repo_with_origin(root.path(), &configured),
        remote_url: wanted.clone(),
    };

    match git.status().unwrap() {
        Status::ConflictUpgrade(conflict) => assert_eq!(
            (conflict.expected, conflict.actual),
            (wanted, format!("current remote: {configured}"))
        ),
        other => panic!("expected a conflict, got {other:?}"),
    }
}

// --- run_script ---

#[hegel::test]
fn run_script_runs_a_script_at_any_path(tc: TestCase) {
    let dir_name = tc.draw(path_component());
    let file_name = tc.draw(path_component());
    let root = TempDir::new().unwrap();
    let script_dir = root.path().join(dir_name);
    fs::create_dir(&script_dir).unwrap();
    let script = script_dir.join(file_name);
    fs::write(
        &script,
        r#"(local frork (require :frork))
           (frork.ok :directory "/tmp")"#,
    )
    .unwrap();

    let (seen, handler) = recorder();
    runtime::run_script(script.to_str().unwrap(), handler).unwrap();
    assert_eq!(seen.borrow().as_slice(), ["Ok: directory /tmp"]);
}

// `;` separates search-path entries and `?` is the module placeholder, and
// Lua has no escape for either, so a directory named with one can't be on
// the path.
#[hegel::test]
fn run_script_requires_modules_beside_the_script(tc: TestCase) {
    let dir_name = tc.draw(path_component().filter(|d: &String| !d.contains([';', '?'])));
    let (module_file, module_source) = tc.draw(gs::sampled_from(vec![
        ("helper.fnl", r#"{:target "/tmp"}"#),
        ("helper.lua", r#"return {target = "/tmp"}"#),
    ]));
    let root = TempDir::new().unwrap();
    let script_dir = root.path().join(dir_name);
    fs::create_dir(&script_dir).unwrap();
    fs::write(script_dir.join(module_file), module_source).unwrap();
    let script = script_dir.join("main.fnl");
    fs::write(
        &script,
        r#"(local frork (require :frork))
           (local helper (require :helper))
           (frork.ok :directory helper.target)"#,
    )
    .unwrap();

    let (seen, handler) = recorder();
    runtime::run_script(script.to_str().unwrap(), handler).unwrap();
    assert_eq!(seen.borrow().as_slice(), ["Ok: directory /tmp"]);
}
