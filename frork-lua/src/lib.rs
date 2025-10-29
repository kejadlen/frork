use mlua::prelude::*;

fn hello(name: Option<String>) -> String {
    match name {
        Some(name) => format!("Hello, {}!", name),
        None => "Hello, World!".to_string(),
    }
}

fn add(x: i64, y: i64) -> i64 {
    x + y
}

#[mlua::lua_module]
fn frork(lua: &Lua) -> LuaResult<LuaTable> {
    let exports = lua.create_table()?;

    // Simple hello function
    exports.set(
        "hello",
        lua.create_function(|_, name: Option<String>| Ok(hello(name)))?,
    )?;

    // Simple add function
    exports.set(
        "add",
        lua.create_function(|_, (x, y): (i64, i64)| Ok(add(x, y)))?,
    )?;

    // A table with some constants
    let constants = lua.create_table()?;
    constants.set("VERSION", "1.0.0")?;
    constants.set("AUTHOR", "Frork Team")?;
    exports.set("constants", constants)?;

    Ok(exports)
}