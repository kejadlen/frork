use std::cell::RefCell;
use std::rc::Rc;

use miette::Result;
use miette::miette;
use mlua::prelude::*;
use tracing::info;

use crate::assertions::AssertionType;
use crate::assertions::LuaAssertionType;
use crate::assertions::Status;
use crate::error::FrorkError;
use crate::registry::Registry;
use crate::utils::Utils;

/// Handles each assertion's status once it has been evaluated. The binary
/// supplies the rendering and any prompting; nothing here writes to stdout.
pub trait StatusHandler: Fn(&Status, &dyn AssertionType) -> Result<()> + Clone + 'static {}

impl<T> StatusHandler for T where T: Fn(&Status, &dyn AssertionType) -> Result<()> + Clone + 'static {}

/// Evaluates inline Fennel code.
pub fn run_code(code: &str, handle_status: impl StatusHandler) -> Result<()> {
    let (lua, frork_module, fennel_module) = setup_lua(handle_status)?;

    let ok_fn: LuaFunction = frork_module
        .get("ok")
        .map_err(|e| miette!("Failed to get frork.ok: {e}"))?;
    lua.globals()
        .set("ok", ok_fn)
        .map_err(|e| miette!("Failed to set ok global: {e}"))?;

    let eval_fn: LuaFunction = fennel_module
        .get("eval")
        .map_err(|e| miette!("Failed to get fennel.eval: {e}"))?;

    eval_fn
        .call::<()>(code)
        .map_err(|e| miette!("Failed to execute fennel code: {e}"))?;

    Ok(())
}

/// Evaluates a `.fnl` script file, with its directory on the search path.
pub fn run_script(script: &str, handle_status: impl StatusHandler) -> Result<()> {
    let (lua, _frork_module, fennel_module) = setup_lua(handle_status)?;

    // Add script directory to Lua and Fennel search paths.
    let script_path = std::path::Path::new(script);
    if let Some(script_dir) = script_path.parent()
        && let Some(script_dir_str) = script_dir.to_str()
    {
        let package_table: LuaTable = lua
            .globals()
            .get("package")
            .map_err(|e| miette!("Failed to get package table: {e}"))?;

        // Add to Lua search path.
        let current_path: String = package_table
            .get("path")
            .map_err(|e| miette!("Failed to get current Lua path: {e}"))?;
        let new_path = format!(
            "{};{}/?.lua;{}/?/init.lua",
            current_path, script_dir_str, script_dir_str
        );
        package_table
            .set("path", new_path)
            .map_err(|e| miette!("Failed to set Lua path: {e}"))?;

        // Add to Fennel search path.
        let current_fennel_path: String = fennel_module
            .get("path")
            .unwrap_or_else(|_| "./?.fnl;./?/init.fnl".to_string());
        let new_fennel_path = format!(
            "{};{}/?.fnl;{}/?/init.fnl",
            current_fennel_path, script_dir_str, script_dir_str
        );
        fennel_module
            .set("path", new_fennel_path)
            .map_err(|e| miette!("Failed to set Fennel path: {e}"))?;
    }

    lua.load(format!(
        r#"require("fennel").install().dofile("{}")"#,
        script
    ))
    .exec()
    .map_err(|e| miette!("Failed to execute script: {e}"))?;

    Ok(())
}

fn setup_lua(handle_status: impl StatusHandler) -> Result<(Lua, LuaTable, LuaTable)> {
    let lua = Lua::new();

    let fennel_code = include_str!("../fennel-1.6.0.lua");
    let fennel_module: LuaTable = lua
        .load(fennel_code)
        .eval()
        .map_err(|e| miette!("Failed to load Fennel: {e}"))?;
    lua.register_module("fennel", &fennel_module)
        .map_err(|e| miette!("Failed to register fennel module: {e}"))?;

    let frork_value = Frork::new(handle_status, lua.clone())
        .into_lua(&lua)
        .map_err(|e| miette!("Failed to create frork table: {e}"))?;
    let LuaValue::Table(frork_table) = frork_value else {
        // Frork::into_lua always returns a table.
        unreachable!();
    };
    lua.register_module("frork", &frork_table)
        .map_err(|e| miette!("Failed to register frork module: {e}"))?;

    Ok((lua, frork_table, fennel_module))
}

struct Frork<F> {
    // RefCell needed for interior mutability — register() adds new assertion
    // types at runtime when called from Lua/Fennel code.
    registry: RefCell<Registry>,
    handle_status: F,
    lua: Lua,
}

impl<F> Frork<F> {
    fn new(handle_status: F, lua: Lua) -> Self {
        Self {
            registry: RefCell::new(Registry::default()),
            handle_status,
            lua,
        }
    }
}

impl<F: StatusHandler> Frork<F> {
    fn register(&self, name: &str, lua_assertion_type: LuaAssertionType) -> LuaResult<()> {
        self.registry
            .borrow_mut()
            .register(name, lua_assertion_type);
        info!("Registered assertion type: {}", name);
        Ok(())
    }

    fn ok(&self, args: LuaMultiValue) -> LuaResult<()> {
        if args.is_empty() {
            return Err(LuaError::external(FrorkError::NoOperation));
        }

        let mut args_iter = args.into_iter();
        let assertion_type = args_iter
            .next()
            .and_then(|v| v.to_string().ok())
            .ok_or_else(|| LuaError::external(FrorkError::MissingAssertionType))?;

        let assertion_args: LuaMultiValue = args_iter.collect();

        let factory = self
            .registry
            .borrow()
            .get_factory(&assertion_type)
            .map_err(LuaError::external)?;
        let assertion = factory
            .create(&self.lua, assertion_args)
            .map_err(LuaError::external)?;
        let status = assertion.status().map_err(LuaError::external)?;

        (self.handle_status)(&status, assertion.as_ref()).map_err(LuaError::external)?;
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
        let register = move |_lua: &Lua, (name, assertion_type): (String, LuaAssertionType)| {
            frork_clone.register(&name, assertion_type)
        };
        let register_fn = lua.create_function(register)?;
        frork_table.set("register", register_fn)?;

        let frork_clone = frork.clone();
        let ok_fn = lua.create_function(move |_lua, args: LuaMultiValue| frork_clone.ok(args))?;
        frork_table.set("ok", ok_fn)?;

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
        let handler = move |status: &Status, assertion: &dyn AssertionType| {
            sink.borrow_mut().push(format!("{status:?}: {assertion}"));
            Ok(())
        };
        (seen, handler)
    }

    #[test]
    fn test_run_code_evaluates_fennel_and_reports_status() {
        let (seen, handler) = recorder();

        run_code(r#"(ok :directory "/tmp")"#, handler).unwrap();

        assert_eq!(seen.borrow().as_slice(), ["Ok: directory /tmp"]);
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
                "Ok: directory /tmp",
                "Missing: directory /frork-does-not-exist",
            ]
        );
    }

    #[test]
    fn test_register_adds_a_type_usable_from_fennel() {
        let (seen, handler) = recorder();

        run_code(
            r#"(local frork (require :frork))
               (frork.register :always-missing
                 {:status (fn [] "missing")
                  :install (fn [] nil)
                  :display (fn [a] (.. "custom " a))})
               (ok :always-missing "thing")"#,
            handler,
        )
        .unwrap();

        assert_eq!(seen.borrow().as_slice(), ["Missing: custom thing"]);
    }

    #[test]
    fn test_ok_without_arguments_is_rejected() {
        let (_seen, handler) = recorder();

        let error = run_code("(ok)", handler).unwrap_err();
        assert!(error.to_string().contains("Failed to execute fennel code"));
    }

    #[test]
    fn test_ok_rejects_unknown_assertion_types() {
        let (seen, handler) = recorder();

        assert!(run_code(r#"(ok :not-a-real-type)"#, handler).is_err());
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn test_run_code_propagates_handler_failures() {
        let handler =
            |_status: &Status, _assertion: &dyn AssertionType| Err(miette!("handler said no"));

        assert!(run_code(r#"(ok :directory "/tmp")"#, handler).is_err());
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

        assert_eq!(seen.borrow().as_slice(), ["Ok: directory /tmp"]);
    }

    #[test]
    fn test_run_script_evaluates_a_file() {
        let dir = TempDir::new().unwrap();
        let script = dir.path().join("check.fnl");
        fs::write(
            &script,
            r#"(local frork (require :frork))
               (frork.ok :directory "/tmp")"#,
        )
        .unwrap();

        let (seen, handler) = recorder();
        run_script(script.to_str().unwrap(), handler).unwrap();

        assert_eq!(seen.borrow().as_slice(), ["Ok: directory /tmp"]);
    }

    #[test]
    fn test_run_script_puts_the_script_directory_on_the_search_path() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("helper.fnl"), r#"{:target "/tmp"}"#).unwrap();
        let script = dir.path().join("main.fnl");
        fs::write(
            &script,
            r#"(local frork (require :frork))
               (local helper (require :helper))
               (frork.ok :directory helper.target)"#,
        )
        .unwrap();

        let (seen, handler) = recorder();
        run_script(script.to_str().unwrap(), handler).unwrap();

        assert_eq!(seen.borrow().as_slice(), ["Ok: directory /tmp"]);
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
}
