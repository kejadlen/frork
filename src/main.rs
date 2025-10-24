use mlua::prelude::*;
use std::env;
use std::fs;
use std::process;

fn main() -> LuaResult<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() != 2 {
        eprintln!("Usage: {} <script.fnl>", args[0]);
        process::exit(1);
    }

    let filename = &args[1];

    let lua = Lua::new();

    let fennel_code = include_str!("../fennel-1.6.0.lua");
    let fennel_module = lua.load(fennel_code).eval::<LuaValue>()?;

    lua.globals().set("fennel", fennel_module.clone())?;
    lua.load(r#"package.preload["fennel"] = function() return fennel end"#).exec()?;

    let dofile_code = format!(r#"require("fennel").install().dofile("{}")"#, filename);
    lua.load(&dofile_code).exec()?;

    Ok(())
}
