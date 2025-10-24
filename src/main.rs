use clap::{Parser, Subcommand};
use mlua::prelude::*;

#[derive(Parser)]
#[command(name = "frork")]
#[command(about = "A Fennel-based configuration management tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Check { script: String },
}

fn main() -> LuaResult<()> {
    let cli = Cli::parse();

    let lua = Lua::new();

    let fennel_code = include_str!("../fennel-1.6.0.lua");
    let fennel_module = lua.load(fennel_code).eval::<LuaValue>()?;
    lua.register_module("fennel", fennel_module)?;

    match &cli.command {
        Commands::Check { script } => {
            lua.load(format!(
                r#"require("fennel").install().dofile("{}")"#,
                script
            ))
            .exec()?;
        }
    }

    Ok(())
}
