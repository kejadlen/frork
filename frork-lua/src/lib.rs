use frork_lib::assertions::AssertionType;
use frork_lib::assertions::Status;
use frork_lib::report;
use frork_lib::runtime::frork_module;
use mlua::prelude::*;

// The frork engine as a Lua module, `require("frork")`. Linked statically
// into a binary from `fennel --compile-binary --native-module`, it turns a
// Fennel script into a standalone executable: `--satisfy` installs missing
// assertions, otherwise they are only reported.

#[mlua::lua_module]
fn frork(lua: &Lua) -> LuaResult<LuaTable> {
    let satisfy = std::env::args().any(|arg| arg == "--satisfy");
    let handler = move |status: &Status, assertion: &dyn AssertionType| {
        for line in report::render(status, assertion) {
            println!("{line}");
        }
        if satisfy && matches!(status, Status::Missing) {
            assertion.install()?;
            println!("ok: {assertion}");
        }
        Ok(())
    };
    let exports = frork_module(lua, handler)?;
    exports.set("VERSION", env!("CARGO_PKG_VERSION"))?;
    Ok(exports)
}
