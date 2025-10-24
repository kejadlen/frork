use mlua::prelude::*;
use std::env;
use std::fs;
use std::process;

fn main() -> LuaResult<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() != 2 {
        eprintln!("Usage: {} <script.lua>", args[0]);
        process::exit(1);
    }

    let filename = &args[1];
    let script_content = fs::read_to_string(filename).unwrap_or_else(|err| {
        eprintln!("Error reading file '{}': {}", filename, err);
        process::exit(1);
    });

    let lua = Lua::new();

    let fennel_code = include_str!("../fennel-1.6.0.lua");
    lua.load(fennel_code).exec()?;

    lua.load(&script_content).exec()?;

    Ok(())
}
