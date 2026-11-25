use std::cell::RefCell;
use std::rc::Rc;

use mlua::prelude::*;
use tracing::info;

use crate::assertions::Assertion;
use crate::assertions::LuaProvider;
use crate::assertions::Status;
use crate::error::Cause;
use crate::error::FrorkError;
use crate::registry::Providers;
use crate::utils::Utils;

/// Which way a script declares an assertion: `ok` wants it to hold, `no`
/// wants it gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declaration {
    Ok,
    No,
}

impl std::fmt::Display for Declaration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Declaration::Ok => "ok",
            Declaration::No => "no",
        })
    }
}

/// Handles each assertion's status once it has been evaluated. The binary
/// supplies the rendering and any prompting; nothing here writes to stdout.
/// Its errors are `FrorkError`s like any other, so they cross back through
/// Lua with their type intact.
pub trait StatusHandler:
    Fn(Declaration, &Status, &dyn Assertion) -> Result<(), FrorkError> + Clone + 'static
{
}

impl<T> StatusHandler for T where
    T: Fn(Declaration, &Status, &dyn Assertion) -> Result<(), FrorkError> + Clone + 'static
{
}

/// Wraps a call into the Lua bridge so a failure names the step that broke.
fn bridge<T>(step: &'static str, result: LuaResult<T>) -> Result<T, FrorkError> {
    // Two steps, so the closure fits on one line: no bridge step fails in
    // a test, and a multi-line closure strands uncoverable lines.
    let result = result.map_err(Cause::from);
    result.map_err(|source| FrorkError::LuaBridge { step, source })
}

/// Fennel options for every compile: correlated output keeps each Lua line
/// on its Fennel line, so Lua's error positions and tracebacks, and the
/// locations `declare` reports, name lines in the script.
fn fennel_options(lua: &Lua) -> LuaResult<LuaTable> {
    let options = lua.create_table()?;
    options.set("correlate", true)?;
    Ok(options)
}

/// The filename inline code compiles under, so its locations read as
/// `eval:3` instead of a quoted copy of the code.
const EVAL_FILENAME: &str = "eval";

/// Evaluates inline Fennel code.
pub fn run_code(code: &str, handle_status: impl StatusHandler) -> Result<(), FrorkError> {
    let (lua, frork_module, fennel_module) = setup_lua(handle_status)?;

    let ok_fn: LuaFunction = bridge("get frork.ok", frork_module.get("ok"))?;
    bridge("set ok global", lua.globals().set("ok", ok_fn))?;

    let no_fn: LuaFunction = bridge("get frork.no", frork_module.get("no"))?;
    bridge("set no global", lua.globals().set("no", no_fn))?;

    let eval_fn: LuaFunction = bridge("get fennel.eval", fennel_module.get("eval"))?;

    let options = bridge("build Fennel options", fennel_options(&lua))?;
    bridge("name inline code", options.set("filename", EVAL_FILENAME))?;
    let result = eval_fn.call::<()>((code, options));
    result.map_err(|source| FrorkError::ScriptEval(source.into()))?;

    Ok(())
}

/// Evaluates a `.fnl` script file, with its directory on the search path.
pub fn run_script(script: &str, handle_status: impl StatusHandler) -> Result<(), FrorkError> {
    let (lua, _frork_module, fennel_module) = setup_lua(handle_status)?;

    if let Some(script_dir_str) = script_dir(script) {
        let package: LuaResult<LuaTable> = lua.globals().get("package");
        let package_table: LuaTable = bridge("get package table", package)?;

        let path: LuaResult<String> = package_table.get("path");
        let current_path: String = bridge("get current Lua path", path)?;
        let new_path = format!(
            "{};{}/?.lua;{}/?/init.lua",
            current_path, script_dir_str, script_dir_str
        );
        let set_path = package_table.set("path", new_path);
        bridge("set Lua path", set_path)?;

        let fennel_path: LuaResult<String> = fennel_module.get("path");
        let current_fennel_path =
            fennel_path.unwrap_or_else(|_| "./?.fnl;./?/init.fnl".to_string());
        let new_fennel_path = format!(
            "{};{}/?.fnl;{}/?/init.fnl",
            current_fennel_path, script_dir_str, script_dir_str
        );
        let fennel_set = fennel_module.set("path", new_fennel_path);
        bridge("set Fennel path", fennel_set)?;
    }

    // fennel.dofile and the searcher each name the chunk after its file.
    let options = bridge("build Fennel options", fennel_options(&lua))?;

    let install_fn: LuaResult<LuaFunction> = fennel_module.get("install");
    let install_fn: LuaFunction = bridge("get fennel.install", install_fn)?;
    let installed = install_fn.call::<()>(&options);
    bridge("install the Fennel searcher", installed)?;

    let dofile_fn: LuaResult<LuaFunction> = fennel_module.get("dofile");
    let dofile_fn: LuaFunction = bridge("get fennel.dofile", dofile_fn)?;
    let result = dofile_fn.call::<()>((script, options));
    result.map_err(|source| FrorkError::ScriptRun {
        script: script.to_string(),
        source: source.into(),
    })?;

    Ok(())
}

/// The directory a script names, if any. A bare filename's parent is empty,
/// and the default search paths already cover `./`.
fn script_dir(script: &str) -> Option<&str> {
    std::path::Path::new(script)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .and_then(std::path::Path::to_str)
}

/// Builds the `frork` module table on an existing Lua state, for hosts that
/// embed frork as a native module instead of using `setup_lua`.
pub fn frork_module(lua: &Lua, handle_status: impl StatusHandler) -> LuaResult<LuaTable> {
    match Frork::new(handle_status).into_lua(lua)? {
        LuaValue::Table(table) => Ok(table),
        // Frork::into_lua always returns a table.
        _ => unreachable!(),
    }
}

/// The `file:line` of the Lua code calling the current Rust function, if
/// it was called from Lua code that has a line.
fn caller_location(lua: &Lua) -> Option<String> {
    let location = lua.inspect_stack(1, |frame| {
        let line = frame.current_line()?;
        let file = frame.source().short_src?.into_owned();
        Some(format!("{file}:{line}"))
    });
    location.flatten()
}

fn setup_lua(handle_status: impl StatusHandler) -> Result<(Lua, LuaTable, LuaTable), FrorkError> {
    let lua = Lua::new();

    let fennel_code = include_str!("../fennel-1.6.0.lua");
    // Named, or tracebacks through the compiler cite this Rust file.
    let loaded = lua.load(fennel_code).set_name("@fennel-1.6.0.lua").eval();
    let fennel_module: LuaTable = bridge("load Fennel", loaded)?;
    let registered = lua.register_module("fennel", &fennel_module);
    bridge("register fennel module", registered)?;

    let lua_value = Frork::new(handle_status).into_lua(&lua);
    let frork_value = bridge("create frork table", lua_value)?;
    let LuaValue::Table(frork_table) = frork_value else {
        // Frork::into_lua always returns a table.
        unreachable!();
    };
    let registered = lua.register_module("frork", &frork_table);
    bridge("register frork module", registered)?;

    Ok((lua, frork_table, fennel_module))
}

struct Frork<F> {
    // RefCell needed for interior mutability — provide() adds new
    // types at runtime when called from Lua/Fennel code.
    providers: RefCell<Providers>,
    handle_status: F,
}

impl<F> Frork<F> {
    fn new(handle_status: F) -> Self {
        Self {
            providers: RefCell::new(Providers::default()),
            handle_status,
        }
    }
}

impl<F: StatusHandler> Frork<F> {
    fn provide(&self, name: &str, lua_provider: LuaProvider) -> LuaResult<()> {
        self.providers.borrow_mut().provide(name, lua_provider);
        info!("Added provider: {}", name);
        Ok(())
    }

    fn declare(&self, declaration: Declaration, lua: &Lua, args: LuaMultiValue) -> LuaResult<()> {
        let location = caller_location(lua);
        // Every failure names the declaration and where the script made it,
        // which a recovered error would otherwise have lost with the Lua
        // traceback.
        let fail = |subject: Option<String>, source: FrorkError| {
            let error = FrorkError::Declaration {
                declaration,
                subject,
                location: location.clone(),
                source: source.into(),
            };
            error.into_lua_error()
        };

        let mut args_iter = args.into_iter();
        let provider = args_iter.next().and_then(|v| v.to_string().ok());
        let provider =
            provider.ok_or_else(|| fail(None, FrorkError::MissingAssertionType { declaration }))?;
        let named = || Some(provider.clone());

        let assertion_args: LuaMultiValue = args_iter.collect();

        let factory = self.providers.borrow().get_factory(&provider);
        let factory = factory.map_err(|e| fail(named(), e))?;
        let assertion = factory.build(&provider, lua, assertion_args);
        let assertion = assertion.map_err(|e| fail(named(), e))?;
        let described = || Some(assertion.to_string());
        let status = assertion.status();
        let status = status.map_err(|e| fail(described(), e))?;

        let handled = (self.handle_status)(declaration, &status, assertion.as_ref());
        handled.map_err(|e| fail(described(), e))?;
        Ok(())
    }
}

impl<F: StatusHandler> IntoLua for Frork<F> {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let frork_table = lua.create_table()?;
        let frork = Rc::new(self);

        let frork_clone = frork.clone();
        // The &Lua annotation is required: without it the closure infers a
        // single concrete lifetime and no longer satisfies create_function.
        let provide = move |_lua: &Lua, (name, provider): (String, LuaProvider)| {
            frork_clone.provide(&name, provider)
        };
        let provide_fn = lua.create_function(provide)?;
        frork_table.set("provide", provide_fn)?;

        for declaration in [Declaration::Ok, Declaration::No] {
            let frork_clone = frork.clone();
            let declare =
                move |lua: &Lua, args: LuaMultiValue| frork_clone.declare(declaration, lua, args);
            frork_table.set(declaration.to_string(), lua.create_function(declare)?)?;
        }

        frork_table.set("utils", Utils {})?;

        Ok(LuaValue::Table(frork_table))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err as fs;
    use tempfile::TempDir;

    type Recorded = Rc<RefCell<Vec<String>>>;

    /// A status handler that records what it was handed, standing in for the
    /// binary's printing.
    fn recorder() -> (Recorded, impl StatusHandler) {
        let seen: Recorded = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        let handler =
            move |declaration: Declaration, status: &Status, assertion: &dyn Assertion| {
                sink.borrow_mut()
                    .push(format!("{declaration} {status:?}: {assertion}"));
                Ok(())
            };
        (seen, handler)
    }

    #[test]
    fn test_run_code_evaluates_fennel_and_reports_status() {
        let (seen, handler) = recorder();

        run_code(r#"(ok :directory "/tmp")"#, handler).unwrap();

        assert_eq!(seen.borrow().as_slice(), ["ok Present: directory /tmp"]);
    }

    #[test]
    fn test_run_code_reports_each_assertion_in_order() {
        let (seen, handler) = recorder();

        run_code(
            r#"(ok :directory "/tmp")
               (ok :directory "/frork-does-not-exist")"#,
            handler,
        )
        .unwrap();

        assert_eq!(
            seen.borrow().as_slice(),
            [
                "ok Present: directory /tmp",
                "ok Absent: directory /frork-does-not-exist",
            ]
        );
    }

    #[test]
    fn test_provide_adds_a_type_usable_from_fennel() {
        let (seen, handler) = recorder();

        run_code(
            r#"(local frork (require :frork))
               (frork.provide :always-absent
                 {:status (fn [] "absent")
                  :create (fn [] nil)
                  :display (fn [a] (.. "custom " a))})
               (ok :always-absent "thing")"#,
            handler,
        )
        .unwrap();

        assert_eq!(seen.borrow().as_slice(), ["ok Absent: custom thing"]);
    }

    #[test]
    fn test_no_reports_through_the_same_providers_as_ok() {
        let (seen, handler) = recorder();

        run_code(
            r#"(local frork (require :frork))
               (no :debug {:status (fn [] "present")})
               (frork.no :debug {:status (fn [] "absent")})"#,
            handler,
        )
        .unwrap();

        assert_eq!(
            seen.borrow().as_slice(),
            ["no Present: debug", "no Absent: debug"]
        );
    }

    #[test]
    fn test_no_rejects_missing_and_unknown_providers() {
        let (seen, handler) = recorder();

        assert!(run_code("(no)", handler.clone()).is_err());
        assert!(run_code("(no :not-a-real-type)", handler).is_err());
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn test_a_declaration_without_a_type_names_the_declaration() {
        let (_seen, handler) = recorder();

        // The error crosses the Lua boundary and is recovered whole, so
        // callers can match on it, with its help text intact.
        let error = run_code("(ok)", handler).unwrap_err();
        let FrorkError::MissingAssertionType { declaration } = error.root() else {
            panic!("expected a missing-type error, got {error:?}"); // cov-excl-line
        };
        assert_eq!(*declaration, Declaration::Ok);
        assert_eq!(error.root().to_string(), "ok is missing an assertion type");
        use miette::Diagnostic as _;
        assert!(error.help().is_some());
    }

    #[test]
    fn test_ok_with_a_non_string_type_is_rejected() {
        let (_seen, handler) = recorder();

        // A table where the provider name goes is not a type either.
        assert!(run_code("(ok {})", handler).is_err());
    }

    #[test]
    fn test_declaration_displays_as_its_fennel_name() {
        assert_eq!(Declaration::Ok.to_string(), "ok");
        assert_eq!(Declaration::No.to_string(), "no");
    }

    #[test]
    fn test_ok_without_arguments_is_rejected() {
        let (_seen, handler) = recorder();

        // `no` gets its own missing-type error naming the declaration.
        let error = run_code("(no)", handler).unwrap_err();
        let FrorkError::MissingAssertionType { declaration } = error.root() else {
            panic!("expected a missing-type error, got {error:?}"); // cov-excl-line
        };
        assert_eq!(*declaration, Declaration::No);
    }

    #[test]
    fn test_an_assertion_with_bad_arguments_names_the_provider() {
        let (_seen, handler) = recorder();

        // `directory` with no path: the provider build failure is recovered
        // with the Lua conversion failure as its source.
        let error = run_code("(ok :directory)", handler).unwrap_err();
        let FrorkError::ScriptEval(cause) = &error else {
            panic!("expected an eval failure, got {error:?}"); // cov-excl-line
        };
        let Some(FrorkError::Declaration { source, .. }) = cause.frork() else {
            panic!("expected a declaration failure, got {cause:?}"); // cov-excl-line
        };
        let Some(build) = source.frork() else {
            panic!("expected a build failure, got {source:?}"); // cov-excl-line
        };
        let FrorkError::AssertionBuild { provider, .. } = build else {
            panic!("expected a build failure, got {build:?}"); // cov-excl-line
        };
        assert_eq!(provider, "directory");
        assert_eq!(
            build.to_string(),
            "Failed to create the directory assertion"
        );
        use std::error::Error as _;
        assert!(build.source().is_some());
    }

    #[test]
    fn test_ok_rejects_unknown_assertion_types() {
        let (seen, handler) = recorder();

        assert!(run_code(r#"(ok :not-a-real-type)"#, handler).is_err());
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn test_run_code_propagates_handler_failures() {
        let handler = |_declaration: Declaration, _status: &Status, _assertion: &dyn Assertion| {
            Err(FrorkError::MissingHome)
        };

        // The handler's own error comes back typed, under the declaration.
        let error = run_code(r#"(ok :directory "/tmp")"#, handler).unwrap_err();
        assert!(matches!(error.root(), FrorkError::MissingHome));
        assert_eq!(
            error.full_message(),
            "Failed to execute fennel code: eval:1: ok directory /tmp: \
             HOME environment variable not set"
        );
    }

    #[test]
    fn test_run_code_rejects_invalid_fennel() {
        let (_seen, handler) = recorder();

        assert!(run_code("(this is not (valid", handler).is_err());
    }

    #[test]
    fn test_frork_utils_are_exposed_to_fennel() {
        let (seen, handler) = recorder();

        run_code(
            r#"(local frork (require :frork))
               (ok :directory (frork.utils.dirname "/tmp/b.txt"))"#,
            handler,
        )
        .unwrap();

        assert_eq!(seen.borrow().as_slice(), ["ok Present: directory /tmp"]);
    }

    #[test]
    fn test_frork_module_works_on_a_host_lua() {
        let (seen, handler) = recorder();
        let lua = Lua::new();

        let frork = frork_module(&lua, handler).unwrap();
        lua.globals().set("frork", frork).unwrap();
        lua.load(
            r#"frork.ok("directory", "/tmp")
               frork.no("directory", "/tmp")"#,
        )
        .exec()
        .unwrap();

        assert_eq!(
            seen.borrow().as_slice(),
            ["ok Present: directory /tmp", "no Present: directory /tmp"]
        );
    }

    #[test]
    fn test_frork_module_lets_the_lua_state_drop() {
        let (_seen, handler) = recorder();
        let lua = Lua::new();
        let weak = lua.weak();

        let frork = frork_module(&lua, handler).unwrap();
        lua.globals().set("frork", frork).unwrap();
        lua.load(
            r#"frork.provide("custom", {
                 status = function() return "present" end,
                 create = function() end,
               })
               frork.ok("custom")"#,
        )
        .exec()
        .unwrap();
        drop(lua);

        assert!(weak.try_upgrade().is_none());
    }

    #[test]
    fn test_run_script_accepts_a_path_with_a_double_quote() {
        let dir = TempDir::new().unwrap();
        let quoted = dir.path().join(r#"we"ird"#);
        fs::create_dir(&quoted).unwrap();
        let script = quoted.join("check.fnl");
        fs::write(
            &script,
            r#"(local frork (require :frork))
               (frork.ok :directory "/tmp")"#,
        )
        .unwrap();

        let (seen, handler) = recorder();
        run_script(script.to_str().unwrap(), handler).unwrap();

        assert_eq!(seen.borrow().as_slice(), ["ok Present: directory /tmp"]);
    }

    #[test]
    fn test_script_dir_skips_a_bare_filename() {
        assert_eq!(script_dir("main.fnl"), None);
        assert_eq!(script_dir(""), None);
        assert_eq!(script_dir("conf/main.fnl"), Some("conf"));
        assert_eq!(script_dir("/etc/frork/main.fnl"), Some("/etc/frork"));
    }

    #[test]
    fn test_run_script_without_a_parent_directory() {
        let (_seen, handler) = recorder();

        // An empty path has no parent, so the search-path block is skipped.
        assert!(run_script("", handler).is_err());
    }

    #[test]
    fn test_run_script_reports_a_missing_file() {
        let (_seen, handler) = recorder();

        let error = run_script("/frork-does-not-exist/nope.fnl", handler).unwrap_err();
        assert!(error.to_string().contains("Failed to execute script"));
    }
    #[test]
    fn test_a_failure_names_the_assertion_and_its_line() {
        let (_seen, handler) = recorder();

        // Blank lines and a comment: correlated output keeps Fennel's line.
        let error = run_code(
            r#"(local frork (require :frork))

               ;; a provider whose status fails
               (frork.provide :p {:status (fn [] (error "boom")) :create (fn [])})
               (ok :p "thing")"#,
            handler,
        )
        .unwrap_err();

        let FrorkError::ScriptEval(cause) = &error else {
            panic!("expected an eval failure, got {error:?}"); // cov-excl-line
        };
        let declared = cause.frork().unwrap();
        assert_eq!(declared.to_string(), "eval:5: ok p thing");
        let FrorkError::Declaration { source, .. } = declared else {
            panic!("expected a declaration failure, got {declared:?}"); // cov-excl-line
        };
        assert_eq!(source.to_string(), "status hook of provider :p failed");
    }

    #[test]
    fn test_an_unbuilt_assertion_is_named_by_its_provider() {
        let (_seen, handler) = recorder();

        let error = run_code("(no :not-a-real-type)", handler).unwrap_err();
        assert!(error.full_message().starts_with(
            "Failed to execute fennel code: eval:1: no not-a-real-type: unknown provider"
        ));
    }

    #[test]
    fn test_an_unnamed_declaration_is_named_by_itself() {
        let (_seen, handler) = recorder();

        let error = run_code("\n(ok)", handler).unwrap_err();
        assert!(
            error
                .full_message()
                .starts_with("Failed to execute fennel code: eval:2: ok: ok is missing")
        );
    }

    #[test]
    fn test_a_declaration_without_a_line_has_no_location() {
        let (_seen, handler) = recorder();

        // Called through pcall, the caller is a C function with no line.
        run_code(
            r#"(local frork (require :frork))
               (local (ok? err) (pcall frork.ok :nope))
               (assert (not ok?))
               (assert (= 1 (string.find (tostring err) "ok nope: unknown provider :nope" 1 true))
                       (tostring err))"#,
            handler,
        )
        .unwrap();
    }

    #[test]
    fn test_lua_sees_the_whole_error_and_its_help() {
        let (_seen, handler) = recorder();

        run_code(
            r#"(local (ok? err) (pcall #(ok :nope)))
               (assert (not ok?))
               (local message (tostring err))
               (assert (string.find message "eval:1: ok nope: unknown provider :nope" 1 true) message)
               (assert (string.find message "\nhelp: the built-ins are" 1 true) message)"#,
            handler,
        )
        .unwrap();
    }

    #[test]
    fn test_a_required_module_reports_its_own_file_and_line() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("helper.fnl"),
            "(local frork (require :frork))\n\n\n(frork.ok :directory \"$FRORK_TEST_UNSET_VAR\")\n",
        )
        .unwrap();
        let script = dir.path().join("main.fnl");
        fs::write(&script, ";; main\n(require :helper)\n").unwrap();

        let (_seen, handler) = recorder();
        let error = run_script(script.to_str().unwrap(), handler).unwrap_err();

        let FrorkError::ScriptRun { source, .. } = &error else {
            panic!("expected a script failure, got {error:?}"); // cov-excl-line
        };
        let declared = source.frork().unwrap().to_string();
        assert!(
            declared.ends_with("helper.fnl:4: ok directory"),
            "{declared}"
        );
        assert!(matches!(error.root(), FrorkError::UnsetVariables { .. }));
    }

    #[test]
    fn test_a_parse_error_skips_the_compiler_traceback() {
        let (_seen, handler) = recorder();

        let error = run_code("(ok :directory", handler).unwrap_err();
        let message = error.full_message();
        assert!(message.contains("eval:1:14: Parse error"), "{message}");
        assert!(!message.contains("stack traceback"), "{message}");
    }

    #[test]
    fn test_the_fennel_compiler_is_named_in_tracebacks() {
        let (_seen, handler) = recorder();

        // An error raised inside the compiler's own code: its traceback
        // cites the vendored file by name.
        let error = run_code(
            r#"(local fennel (require :fennel))
               (fennel.dofile "/frork-does-not-exist.fnl")
               nil"#,
            handler,
        )
        .unwrap_err();
        let message = error.full_message();
        assert!(message.contains("fennel-1.6.0.lua:"), "{message}");
        assert!(!message.contains("runtime.rs"), "{message}");
    }
}
