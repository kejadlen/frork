use frork_lib::assertions::Provider;
use frork_lib::assertions::Status;
use frork_lib::report;
use frork_lib::runtime::frork_module;
use mlua::prelude::*;

// The frork engine as a Lua module, `require("frork")`. Linked statically
// into a binary from `fennel --compile-binary --native-module`, it turns a
// Fennel script into a standalone executable: `--apply` creates absent
// assertions, otherwise they are only reported.

#[mlua::lua_module]
fn frork(lua: &Lua) -> LuaResult<LuaTable> {
    let apply = std::env::args().any(|arg| arg == "--apply");
    let handler = move |status: &Status, assertion: &dyn Provider| {
        print_lines(&report::render(status, assertion));
        if apply && matches!(status, Status::Absent) {
            assertion.create()?;
            print_lines(&report::render(&Status::Present, assertion));
        }
        Ok(())
    };
    let exports = frork_module(lua, handler)?;
    exports.set("VERSION", frork_lib::VERSION)?;
    Ok(exports)
}

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}
