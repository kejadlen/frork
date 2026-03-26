use mlua::prelude::*;

// This crate builds a cdylib that can be loaded as a Lua module via
// `require("frork")`. The goal is to expose frork's assertion engine
// so Fennel's `--compile-binary` can produce standalone binaries
// without the Rust toolchain at runtime.
//
// The assertion engine currently lives in frork-cli. Once it moves
// to frork-lib, this module will re-export it to Lua.

#[mlua::lua_module]
fn frork(lua: &Lua) -> LuaResult<LuaTable> {
    let exports = lua.create_table()?;

    exports.set("VERSION", env!("CARGO_PKG_VERSION"))?;

    Ok(exports)
}
